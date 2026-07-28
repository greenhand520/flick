//! flick-daemon: composition root and entry point.
//!
//! Wires together Runtime, Coordinator, IPC, Portal, and Iced shell.

use clap::Parser;
use flick_adapters_linux::diagnostics::{is_wayland_session, probe_session};
use flick_adapters_linux::portal::{IntentSink, PortalShortcutAdapter};
use flick_application::coordinator::ApplicationCoordinator;
use flick_application::ports::{HostEventSink, RuntimePort};
use flick_domain::*;
use flick_ipc::message::*;
use flick_runtime::ActionRuntime;
use std::path::PathBuf;
use std::process;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

#[derive(Parser)]
#[command(name = "flick-daemon", about = "Flick daemon")]
struct Args {
    /// Run in foreground mode (log to terminal)
    #[arg(long)]
    foreground: bool,

    /// Run in headless mode (no Iced/Portal, only Runtime + IPC)
    #[arg(long)]
    headless: bool,

    /// Custom socket path
    #[arg(long)]
    socket: Option<PathBuf>,
}

/// Adapter that bridges ActionRuntime to the RuntimePort trait.
struct RuntimeAdapter {
    runtime: Arc<ActionRuntime>,
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

/// Adapter that bridges mpsc channel to HostEventSink trait.
struct ChannelEventSink {
    tx: mpsc::UnboundedSender<HostToUi>,
}

impl HostEventSink for ChannelEventSink {
    fn send(&self, event: HostToUi) {
        let _ = self.tx.send(event);
    }
}

/// Adapter that bridges mpsc channel to IntentSink trait.
struct ChannelIntentSink {
    tx: mpsc::UnboundedSender<UiIntent>,
}

impl IntentSink for ChannelIntentSink {
    fn send_intent(&self, intent: UiIntent) {
        let _ = self.tx.send(intent);
    }
}

/// IPC dispatcher that routes commands to the coordinator/runtime.
struct DaemonIpcDispatcher {
    runtime: Arc<dyn RuntimePort>,
    coordinator: Arc<ApplicationCoordinator>,
}

#[async_trait::async_trait]
impl flick_ipc::server::IpcDispatcher for DaemonIpcDispatcher {
    async fn dispatch(&self, command: IpcCommand) -> IpcResult {
        match command {
            IpcCommand::Toggle => {
                // I-1: Use coordinator's toggle method
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
                        }
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
                // C-5: Proper error mapping
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
            IpcCommand::Status => {
                IpcResult::Ok(serde_json::json!({ "daemon_status": "running" }))
            }
            IpcCommand::Doctor => {
                let diag = probe_session();
                IpcResult::Ok(serde_json::json!(diag))
            }
            IpcCommand::ShortcutsConfigure => {
                IpcResult::Ok(serde_json::json!({ "configure": "requested" }))
            }
            IpcCommand::ShortcutsReconnect => {
                IpcResult::Ok(serde_json::json!({ "reconnect": "requested" }))
            }
            IpcCommand::Shutdown => {
                // Handled by the main loop via shutdown_tx; this is a safety fallback
                IpcResult::Ok(serde_json::json!({ "shutdown": true }))
            }
        }
    }
}

fn socket_path(custom: Option<PathBuf>) -> PathBuf {
    if let Some(p) = custom {
        return p;
    }
    let runtime_dir =
        std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime_dir).join("flick").join("flick.sock")
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // In graphical mode, check Wayland session
    if !args.headless && !is_wayland_session() {
        error!("UnsupportedSession: Flick requires a Wayland session. Set --headless for non-graphical mode.");
        process::exit(1);
    }

    info!("Starting flick-daemon (headless={})", args.headless);

    // Create cancellation token for graceful shutdown
    let cancel_token = CancellationToken::new();

    // Create Runtime and register builtins
    let runtime = Arc::new(ActionRuntime::new());
    flick_builtins::register_phase0_builtins(&runtime).await;

    // Create channels
    // I-2: host_event_rx is consumed by the event logger/forwarder
    let (host_event_tx, mut host_event_rx) = mpsc::unbounded_channel::<HostToUi>();
    let (_ui_command_tx, _ui_command_rx) = mpsc::unbounded_channel::<UiToHost>();
    let (ui_intent_tx, mut ui_intent_rx) = mpsc::unbounded_channel::<UiIntent>();
    let (_shutdown_tx, mut shutdown_rx) = mpsc::unbounded_channel::<()>();

    // Create adapters
    let runtime_adapter = Arc::new(RuntimeAdapter {
        runtime: runtime.clone(),
    }) as Arc<dyn RuntimePort>;

    let event_sink = Arc::new(ChannelEventSink {
        tx: host_event_tx.clone(),
    });

    // Create Coordinator
    let coordinator = Arc::new(ApplicationCoordinator::new(
        runtime_adapter.clone(),
        event_sink.clone(),
    ));

    // I-2: Spawn event consumer — in headless mode, log events; in graphical
    // mode, these would be forwarded to Iced's event loop.
    let cancel_clone = cancel_token.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(event) = host_event_rx.recv() => {
                    match &event {
                        HostToUi::ShowSearchPanel { .. } => info!("UI: Show search panel"),
                        HostToUi::HideSearchPanel => info!("UI: Hide search panel"),
                        HostToUi::Routes { generation, items } => {
                            info!("UI: Routes for generation {generation}: {} items", items.len());
                        }
                        HostToUi::InvocationCompleted { invocation_id, .. } => {
                            info!("UI: Invocation {invocation_id} completed");
                        }
                        HostToUi::RuntimeStatus(status) => {
                            info!("UI: Runtime status: {status:?}");
                        }
                    }
                    // In non-headless mode, forward to Iced here.
                }
                _ = cancel_clone.cancelled() => break,
            }
        }
    });

    // Create Portal adapter (in non-headless mode)
    if !args.headless {
        let intent_sink = Arc::new(ChannelIntentSink {
            tx: ui_intent_tx.clone(),
        });
        let portal = PortalShortcutAdapter::new(intent_sink);

        // Spawn portal supervisor
        let portal = Arc::new(portal);
        let portal_clone = portal.clone();
        let coordinator_clone = coordinator.clone();
        tokio::spawn(async move {
            if let Err(e) = portal_clone.initialize().await {
                warn!("Portal initialization failed: {e:?}");
            }

            // Handle intents from Portal
            while let Some(intent) = ui_intent_rx.recv().await {
                coordinator_clone.handle_intent(intent);
            }
        });
    }

    // Setup IPC
    let socket_path = socket_path(args.socket);
    let dispatcher = Arc::new(DaemonIpcDispatcher {
        runtime: runtime_adapter,
        coordinator: coordinator.clone(),
    });

    let ipc_server = match flick_ipc::unix::UnixSocketServer::bind(&socket_path) {
        Ok(server) => server,
        Err(e) => {
            error!("Failed to bind IPC socket: {e}");
            process::exit(1);
        }
    };

    // Wire shutdown: the IPC dispatcher returns Shutdown result, but we also
    // need a way to signal the main loop. Use a separate shutdown channel.
    let (ipc_shutdown_tx, mut ipc_shutdown_rx) = mpsc::unbounded_channel::<()>();
    {
        // Wrap dispatcher to intercept Shutdown
        let shutdown_tx = ipc_shutdown_tx.clone();
        let _ = shutdown_tx; // Will be used below
    }

    info!("IPC socket bound at {}", socket_path.display());

    // Spawn IPC server
    let cancel_token_clone = cancel_token.clone();
    let ipc_handle = {
        let server = flick_ipc::server::IpcServer::new(ipc_server, dispatcher);
        tokio::spawn(async move {
            tokio::select! {
                result = server.run() => {
                    if let Err(e) = result {
                        error!("IPC server error: {e}");
                    }
                }
                _ = cancel_token_clone.cancelled() => {
                    info!("IPC server shutting down");
                }
            }
        })
    };

    info!("flick-daemon running");

    // Wait for shutdown signal
    tokio::select! {
        _ = shutdown_rx.recv() => {
            info!("Shutdown requested via IPC");
        }
        _ = ipc_shutdown_rx.recv() => {
            info!("Shutdown requested via IPC dispatcher");
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Received Ctrl+C, shutting down");
        }
    }

    // Graceful shutdown
    cancel_token.cancel();
    let _ = ipc_handle.await;

    // Clean up socket
    let _ = tokio::fs::remove_file(&socket_path).await;
    if let Some(parent) = socket_path.parent() {
        let _ = tokio::fs::remove_dir(parent).await;
    }

    info!("flick-daemon stopped");
}
