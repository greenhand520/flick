//! Bootstrap helpers for flick-daemon.
//!
//! Shared types and functions used by main.rs for both graphical and headless modes.

use flick_adapters_linux::diagnostics::probe_session;
use flick_adapters_linux::portal::{IntentSink, PortalShortcutAdapter};
use flick_application::coordinator::ApplicationCoordinator;
use flick_application::ports::{HostEventSink, RuntimePort};
use flick_domain::*;
use flick_ipc::message::*;
use flick_ipc::server::IpcServer;
use flick_ipc::unix::UnixSocketServer;
use flick_runtime::ActionRuntime;
use std::path::Path;
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
            IpcCommand::Doctor => IpcResult::Ok(serde_json::json!(probe_session())),
            IpcCommand::ShortcutsConfigure { preferred_trigger } => {
                info!("Shortcut config requested, preferred: {preferred_trigger}");
                // Call Portal adapter if available
                if let Some(portal) = &self.portal {
                    let portal = portal.clone();
                    // Spawn async Portal call
                    tokio::spawn(async move {
                        match portal.configure_search_shortcut().await {
                            Ok(binding) => {
                                info!("Portal shortcut bound: {:?}", binding.actual_trigger)
                            }
                            Err(e) => warn!("Portal shortcut config failed: {e}"),
                        }
                    });
                }
                self.coordinator.handle_intent(UiIntent::ShowSearchPanel {
                    activation_token: None,
                });
                IpcResult::Ok(serde_json::json!({
                    "configure": "shortcut_bind_requested",
                    "shortcut_id": "show-search",
                    "preferred_trigger": preferred_trigger,
                    "portal_available": self.portal.is_some()
                }))
            }
            IpcCommand::ShortcutsReconnect => {
                IpcResult::Ok(serde_json::json!({ "reconnect": "requested" }))
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
}

// ─── Public helpers ──────────────────────────────────────────────────────────

pub fn create_ipc_dispatcher(
    runtime: Arc<ActionRuntime>,
    coordinator: Arc<ApplicationCoordinator>,
    _host_event_tx: mpsc::UnboundedSender<HostToUi>,
    intent_tx: mpsc::UnboundedSender<UiIntent>,
    portal: Option<Arc<PortalShortcutAdapter>>,
) -> Arc<DaemonIpcDispatcher> {
    let runtime_port = Arc::new(RuntimeAdapter { runtime }) as Arc<dyn RuntimePort>;
    Arc::new(DaemonIpcDispatcher {
        runtime: runtime_port,
        coordinator,
        intent_tx,
        portal,
    })
}

pub struct DaemonHandle {
    pub cancel_token: CancellationToken,
    pub socket_path: std::path::PathBuf,
    pub ipc_handle: Option<tokio::task::JoinHandle<()>>,
}

pub async fn bind_and_run_ipc(
    socket_path: &Path,
    dispatcher: Arc<DaemonIpcDispatcher>,
    cancel: CancellationToken,
) -> Result<DaemonHandle, String> {
    let server = UnixSocketServer::bind(socket_path).map_err(|e| format!("{e}"))?;
    info!("IPC socket bound at {}", socket_path.display());

    let ipc_server = IpcServer::new(server, dispatcher);
    let cancel_clone = cancel.clone();
    let handle = tokio::spawn(async move {
        tokio::select! {
            result = ipc_server.run() => {
                if let Err(e) = result { error!("IPC server error: {e}"); }
            }
            _ = cancel_clone.cancelled() => { info!("IPC server shutting down"); }
        }
    });

    Ok(DaemonHandle {
        cancel_token: cancel,
        socket_path: socket_path.to_path_buf(),
        ipc_handle: Some(handle),
    })
}

pub async fn shutdown(mut handle: DaemonHandle) {
    handle.cancel_token.cancel();
    if let Some(h) = handle.ipc_handle.take() {
        let _ = h.await;
    }
    let _ = tokio::fs::remove_file(&handle.socket_path).await;
    if let Some(parent) = handle.socket_path.parent() {
        let _ = tokio::fs::remove_dir(parent).await;
    }
    info!("flick-daemon stopped");
}

// ─── Headless mode ──────────────────────────────────────────────────────────

pub async fn run_headless(socket_path: std::path::PathBuf) {
    use crate::headless::HeadlessRuntime;

    info!("Starting in headless mode");

    let headless = HeadlessRuntime::new().await;
    let runtime = headless.runtime;

    let (host_event_tx, host_event_rx) = mpsc::unbounded_channel::<HostToUi>();
    let (intent_tx, _intent_rx) = mpsc::unbounded_channel::<UiIntent>();

    let event_sink = Arc::new(ChannelEventSink {
        tx: host_event_tx.clone(),
    });
    let runtime_port = Arc::new(RuntimeAdapter {
        runtime: runtime.clone(),
    }) as Arc<dyn RuntimePort>;
    let coordinator = Arc::new(ApplicationCoordinator::new(runtime_port, event_sink));

    // Log host events
    tokio::spawn(async move {
        let mut rx = host_event_rx;
        while let Some(event) = rx.recv().await {
            match &event {
                HostToUi::ShowSearchPanel { .. } => info!("UI: Show search panel"),
                HostToUi::HideSearchPanel => info!("UI: Hide search panel"),
                HostToUi::Routes { generation, items } => {
                    info!("UI: Routes for gen {generation}: {} items", items.len())
                }
                HostToUi::InvocationCompleted { invocation_id, .. } => {
                    info!("UI: Invocation {invocation_id} completed")
                }
                HostToUi::RuntimeStatus(status) => info!("UI: Runtime status: {status:?}"),
            }
        }
    });

    let dispatcher = create_ipc_dispatcher(runtime, coordinator, host_event_tx, intent_tx, None);
    let cancel = CancellationToken::new();
    let handle = match bind_and_run_ipc(&socket_path, dispatcher, cancel.clone()).await {
        Ok(h) => h,
        Err(e) => {
            error!("IPC bind failed: {e}");
            return;
        }
    };

    info!("Headless daemon running");
    tokio::signal::ctrl_c().await.ok();
    cancel.cancel();
    shutdown(handle).await;
}
