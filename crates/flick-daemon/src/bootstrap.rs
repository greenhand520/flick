//! Bootstrap helpers for flick-daemon.
//!
//! Shared types and functions used by main.rs for both graphical and headless modes.

use crate::lifecycle::{DaemonLifecycle, ShutdownReason};
use flick_adapters_linux::diagnostics::probe_session;
use flick_adapters_linux::portal::{IntentSink, PortalShortcutAdapter};
use flick_adapters_linux::session::ShortcutError;
use flick_adapters_linux::shortcut_state::{PortalSnapshot, ShortcutState};
use flick_application::coordinator::ApplicationCoordinator;
use flick_application::ports::{HostEventSink, RuntimePort};
use flick_domain::*;
use flick_ipc::message::*;
use flick_ipc::server::IpcServer;
use flick_ipc::unix::{TransportError, UnixSocketServer};
use flick_runtime::ActionRuntime;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

// ─── Adapter types ───────────────────────────────────────────────────────────

pub struct RuntimeAdapter {
    pub runtime: Arc<ActionRuntime>,
}

#[async_trait::async_trait]
impl RuntimePort for RuntimeAdapter {
    async fn route(&self, query: &str, input_kind: InputKind) -> Vec<RouteItem> {
        self.runtime.route(query, input_kind).await
    }
    async fn invoke(&self, request: ActionRequest) -> ActionOutcome {
        self.runtime.invoke(request).await
    }
    async fn cancel(&self, invocation_id: InvocationId) -> bool {
        self.runtime.cancel(invocation_id).await
    }
}

pub struct ChannelEventSink {
    pub tx: mpsc::UnboundedSender<HostToUi>,
}

impl HostEventSink for ChannelEventSink {
    fn send(&self, event: HostToUi) {
        let _ = self.tx.send(event);
    }
}

pub struct ChannelIntentSink {
    pub tx: mpsc::UnboundedSender<UiIntent>,
}

impl IntentSink for ChannelIntentSink {
    fn send_intent(&self, intent: UiIntent) {
        let _ = self.tx.send(intent);
    }
}

// ─── IPC Dispatcher ──────────────────────────────────────────────────────────

pub struct DaemonIpcDispatcher {
    pub runtime: Arc<dyn RuntimePort>,
    pub coordinator: Arc<ApplicationCoordinator>,
    #[allow(dead_code)]
    pub intent_tx: mpsc::UnboundedSender<UiIntent>,
    pub portal: Option<Arc<PortalShortcutAdapter>>,
    pub socket_path: PathBuf,
    pub lifecycle: DaemonLifecycle,
}

#[async_trait::async_trait]
impl flick_ipc::server::IpcDispatcher for DaemonIpcDispatcher {
    async fn dispatch(&self, command: IpcCommand) -> IpcResult {
        match command {
            IpcCommand::Toggle => {
                self.coordinator.toggle_search_panel();
                IpcResult::Ok(serde_json::json!({ "toggled": true }))
            }
            IpcCommand::Trigger { action_id, text } => {
                let action_id = match ActionId::new(&action_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return IpcResult::Error {
                            code: IpcErrorCode::InvalidInput,
                            message: e.user_message(),
                        };
                    }
                };
                let request = ActionRequest {
                    invocation_id: InvocationId::new(),
                    action_id,
                    input: ActionInput::plain_text(&text),
                    caller: Caller::cli("flick-cli"),
                    deadline: Deadline::from_secs(30),
                };
                let outcome = self.runtime.invoke(request).await;
                match outcome {
                    ActionOutcome::Data(val) => IpcResult::Ok(val),
                    ActionOutcome::Notice { message } => {
                        IpcResult::Ok(serde_json::json!({ "notice": message }))
                    }
                    ActionOutcome::Error(err) => {
                        let code = match &err {
                            ActionError::UnknownAction { .. } => IpcErrorCode::ActionNotFound,
                            ActionError::InvalidInput { .. } => IpcErrorCode::InvalidInput,
                            ActionError::Cancelled => IpcErrorCode::Cancelled,
                            _ => IpcErrorCode::Internal,
                        };
                        IpcResult::Error {
                            code,
                            message: err.user_message(),
                        }
                    }
                }
            }
            IpcCommand::Status => IpcResult::Ok(serde_json::json!({ "daemon_status": "running" })),
            IpcCommand::Doctor => {
                let portal = match &self.portal {
                    Some(portal) => portal_snapshot_value(&portal.diagnostics().await),
                    None => serde_json::json!({ "state": "headless_not_started" }),
                };
                IpcResult::Ok(serde_json::json!({
                    "session": probe_session(),
                    "portal": portal,
                    "ipc": { "socket": self.socket_path.display().to_string() },
                }))
            }
            IpcCommand::ShortcutsConfigure { preferred_trigger } => {
                info!("Shortcut configuration requested");
                let Some(portal) = &self.portal else {
                    return IpcResult::Error {
                        code: IpcErrorCode::DesktopUnavailable,
                        message: "Global shortcut configuration is unavailable.".into(),
                    };
                };

                match portal.configure_search_shortcut(&preferred_trigger).await {
                    Ok(binding) => {
                        info!("Portal shortcut configured");
                        IpcResult::Ok(serde_json::json!({
                            "state": "bound",
                            "shortcut": binding,
                        }))
                    }
                    Err(error) => shortcut_configuration_error(error),
                }
            }
            IpcCommand::ShortcutsReconnect => {
                let Some(portal) = &self.portal else {
                    return IpcResult::Error {
                        code: IpcErrorCode::DesktopUnavailable,
                        message: "Global shortcut recovery is unavailable.".into(),
                    };
                };

                match portal.reconnect().await {
                    Ok(snapshot) => IpcResult::Ok(portal_snapshot_value(&snapshot)),
                    Err(error) => shortcut_configuration_error(error),
                }
            }
            IpcCommand::Shutdown => IpcResult::Ok(serde_json::json!({ "shutdown": true })),
            IpcCommand::Show => {
                self.coordinator.handle_intent(UiIntent::ShowSearchPanel {
                    activation_token: None,
                });
                IpcResult::Ok(serde_json::json!({ "show": true }))
            }
            IpcCommand::Hide => {
                self.coordinator.handle_intent(UiIntent::HideSearchPanel);
                IpcResult::Ok(serde_json::json!({ "hide": true }))
            }
        }
    }

    async fn after_response(&self, command: &IpcCommand) {
        if matches!(command, IpcCommand::Shutdown) {
            self.lifecycle.request_shutdown(ShutdownReason::Ipc);
        }
    }
}

fn shortcut_configuration_error(error: ShortcutError) -> IpcResult {
    let (code, message) = match error {
        ShortcutError::UserCancelled => (
            IpcErrorCode::Cancelled,
            "Shortcut configuration was cancelled.",
        ),
        ShortcutError::Unavailable { .. } => (
            IpcErrorCode::DesktopUnavailable,
            "Global shortcut configuration is unavailable.",
        ),
        ShortcutError::RequestEnded { .. } => (
            IpcErrorCode::Internal,
            "Shortcut configuration did not complete.",
        ),
        ShortcutError::SessionError { .. } => (
            IpcErrorCode::DesktopUnavailable,
            "Global shortcut session is unavailable. Try configuring again.",
        ),
        ShortcutError::Internal { .. } => {
            (IpcErrorCode::Internal, "Shortcut configuration failed.")
        }
    };
    warn!(
        category = error.category(),
        "Portal shortcut configuration failed"
    );
    IpcResult::Error {
        code,
        message: message.into(),
    }
}

fn portal_snapshot_value(snapshot: &PortalSnapshot) -> serde_json::Value {
    let (state, state_detail) = match &snapshot.state {
        ShortcutState::Unavailable { reason } => {
            ("unavailable", serde_json::json!({ "reason": reason }))
        }
        ShortcutState::Unconfigured => ("unconfigured", serde_json::Value::Null),
        ShortcutState::Configuring => ("configuring", serde_json::Value::Null),
        ShortcutState::Bound => ("bound", serde_json::Value::Null),
        ShortcutState::Denied => ("denied", serde_json::Value::Null),
        ShortcutState::Recovering { attempt } => {
            ("recovering", serde_json::json!({ "attempt": attempt }))
        }
        ShortcutState::Error { operation, message } => (
            "error",
            serde_json::json!({ "operation": operation, "message": message }),
        ),
    };

    serde_json::json!({
        "state": state,
        "state_detail": state_detail,
        "session_active": snapshot.session_active,
        "shortcut_count": snapshot.shortcut_count,
        "last_error": snapshot.last_error,
        "reconnect_attempt": snapshot.reconnect_attempt,
    })
}

// ─── Public helpers ──────────────────────────────────────────────────────────

pub fn create_ipc_dispatcher(
    runtime: Arc<ActionRuntime>,
    coordinator: Arc<ApplicationCoordinator>,
    _host_event_tx: mpsc::UnboundedSender<HostToUi>,
    intent_tx: mpsc::UnboundedSender<UiIntent>,
    portal: Option<Arc<PortalShortcutAdapter>>,
    socket_path: PathBuf,
    lifecycle: DaemonLifecycle,
) -> Arc<DaemonIpcDispatcher> {
    let runtime_port = Arc::new(RuntimeAdapter { runtime }) as Arc<dyn RuntimePort>;
    Arc::new(DaemonIpcDispatcher {
        runtime: runtime_port,
        coordinator,
        intent_tx,
        portal,
        socket_path,
        lifecycle,
    })
}

pub struct DaemonHandle {
    pub cancel_token: CancellationToken,
    pub socket_path: std::path::PathBuf,
    pub ipc_handle: Option<tokio::task::JoinHandle<Result<(), TransportError>>>,
}

pub async fn bind_and_run_ipc(
    socket_path: &Path,
    dispatcher: Arc<DaemonIpcDispatcher>,
    cancel: CancellationToken,
) -> Result<DaemonHandle, String> {
    let server = UnixSocketServer::bind(socket_path).map_err(|e| format!("{e}"))?;
    info!("IPC socket bound at {}", socket_path.display());

    let ipc_server = IpcServer::new(server, dispatcher);
    let server_cancel = cancel.clone();
    let handle = tokio::spawn(async move { ipc_server.run(server_cancel).await });

    Ok(DaemonHandle {
        cancel_token: cancel,
        socket_path: socket_path.to_path_buf(),
        ipc_handle: Some(handle),
    })
}

pub async fn shutdown(mut handle: DaemonHandle) {
    handle.cancel_token.cancel();
    if let Some(task) = handle.ipc_handle.take() {
        match task.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => error!(%error, "IPC server stopped with an error"),
            Err(error) => error!(%error, "IPC server task panicked"),
        }
    }
    // Task 4 intentionally removes only the socket. Runtime-directory ownership
    // tracking belongs to Task 5, so never delete a parent directory here.
    let _ = tokio::fs::remove_file(&handle.socket_path).await;
    info!("flick-daemon stopped");
}

// ─── Headless mode ──────────────────────────────────────────────────────────

pub async fn run_headless(socket_path: std::path::PathBuf) -> Result<(), String> {
    use crate::headless::HeadlessRuntime;
    use tokio::signal::unix::{SignalKind, signal};

    info!("Starting in headless mode");

    let lifecycle = DaemonLifecycle::new();
    let headless = HeadlessRuntime::new().await;
    let runtime = headless.runtime;

    let (host_event_tx, mut host_event_rx) = mpsc::unbounded_channel::<HostToUi>();
    let (intent_tx, _intent_rx) = mpsc::unbounded_channel::<UiIntent>();

    let event_sink = Arc::new(ChannelEventSink {
        tx: host_event_tx.clone(),
    });
    let runtime_port = Arc::new(RuntimeAdapter {
        runtime: runtime.clone(),
    }) as Arc<dyn RuntimePort>;
    let coordinator = Arc::new(ApplicationCoordinator::new(runtime_port, event_sink));

    let event_lifecycle = lifecycle.clone();
    let event_worker = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = event_lifecycle.cancelled() => break,
                event = host_event_rx.recv() => match event {
                    Some(HostToUi::Shutdown) | None => break,
                    Some(HostToUi::ShowSearchPanel { .. }) => info!("UI: Show search panel"),
                    Some(HostToUi::HideSearchPanel) => info!("UI: Hide search panel"),
                    Some(HostToUi::Routes { generation, items }) => {
                        info!("UI: Routes for gen {generation}: {} items", items.len());
                    }
                    Some(HostToUi::InvocationCompleted { invocation_id, .. }) => {
                        info!("UI: Invocation {invocation_id} completed");
                    }
                    Some(HostToUi::RuntimeStatus(status)) => info!("UI: Runtime status: {status:?}"),
                }
            }
        }
    });

    let dispatcher = create_ipc_dispatcher(
        runtime,
        coordinator,
        host_event_tx.clone(),
        intent_tx,
        None,
        socket_path.clone(),
        lifecycle.clone(),
    );
    let handle = match bind_and_run_ipc(&socket_path, dispatcher, lifecycle.token()).await {
        Ok(handle) => handle,
        Err(e) => {
            error!("IPC bind failed: {e}");
            lifecycle.request_shutdown(ShutdownReason::StartupFailed);
            let _ = event_worker.await;
            return Err(format!("IPC: {e}"));
        }
    };

    info!("Headless daemon running");
    let mut sigterm = match signal(SignalKind::terminate()) {
        Ok(signal) => Some(signal),
        Err(error) => {
            warn!(%error, "SIGTERM handling is unavailable");
            None
        }
    };

    if let Some(sigterm) = sigterm.as_mut() {
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    warn!(%error, "Ctrl+C handling failed");
                }
                lifecycle.request_shutdown(ShutdownReason::Signal);
            }
            _ = sigterm.recv() => lifecycle.request_shutdown(ShutdownReason::Signal),
            _ = lifecycle.cancelled() => {}
        }
    } else {
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    warn!(%error, "Ctrl+C handling failed");
                }
                lifecycle.request_shutdown(ShutdownReason::Signal);
            }
            _ = lifecycle.cancelled() => {}
        }
    }

    let _ = host_event_tx.send(HostToUi::Shutdown);
    shutdown(handle).await;
    let _ = event_worker.await;
    Ok(())
}
