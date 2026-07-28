//! flick-daemon: composition root and entry point.

mod bootstrap;
mod headless;

use clap::Parser;
use std::path::PathBuf;
use std::process;

#[derive(Parser)]
#[command(name = "flick-daemon", about = "Flick launcher daemon")]
struct Args {
    #[arg(long)]
    foreground: bool,
    #[arg(long)]
    headless: bool,
    #[arg(long)]
    socket: Option<PathBuf>,
}

fn socket_path(custom: Option<PathBuf>) -> PathBuf {
    if let Some(p) = custom {
        return p;
    }
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime_dir).join("flick").join("flick.sock")
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let path = socket_path(args.socket);

    if args.headless {
        let rt = tokio::runtime::Runtime::new().expect("failed to create runtime");
        rt.block_on(bootstrap::run_headless(path));
        process::exit(0);
    } else {
        match run_graphical(path) {
            Ok(()) => process::exit(0),
            Err(msg) => {
                eprintln!("Error: {msg}");
                process::exit(1);
            }
        }
    }
}

fn run_graphical(socket_path: PathBuf) -> Result<(), String> {
    use flick_adapters_linux::diagnostics::is_wayland_session;

    #[allow(unused_imports)]
    use flick_adapters_linux::portal::{IntentSink, PortalShortcutAdapter};
    use flick_application::coordinator::ApplicationCoordinator;
    use flick_application::ports::RuntimePort;
    use flick_domain::*;
    use flick_runtime::ActionRuntime;
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    if !is_wayland_session() {
        let diag = flick_adapters_linux::diagnostics::probe_session();
        return Err(format!(
            "UnsupportedSession: requires Wayland. Session: {:?}. Use --headless.",
            diag.session_type
        ));
    }
    tracing::info!("Starting in graphical mode");

    let (host_event_tx, host_event_rx) = mpsc::unbounded_channel::<HostToUi>();
    let (ui_cmd_tx, ui_cmd_rx) = mpsc::unbounded_channel::<UiToHost>();

    // Background thread: ALL Tokio async work (IPC + Portal + Coordinator)
    let bg_thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("failed to create bg runtime");
        rt.block_on(async move {
            let runtime = Arc::new(ActionRuntime::new());
            flick_builtins::register_phase0_builtins(&runtime).await;

            let event_sink = Arc::new(bootstrap::ChannelEventSink {
                tx: host_event_tx.clone(),
            });
            let runtime_port = Arc::new(bootstrap::RuntimeAdapter {
                runtime: runtime.clone(),
            }) as Arc<dyn RuntimePort>;
            let coordinator = Arc::new(ApplicationCoordinator::new(runtime_port, event_sink));

            let coord = coordinator.clone();
            let mut rx = ui_cmd_rx;
            tokio::spawn(async move {
                while let Some(cmd) = rx.recv().await {
                    coord.handle_ui(cmd).await;
                }
            });

            let (intent_tx, mut intent_rx) = mpsc::unbounded_channel::<UiIntent>();
            let portal = PortalShortcutAdapter::new(Arc::new(bootstrap::ChannelIntentSink {
                tx: intent_tx.clone(),
            }));
            if let Err(e) = portal.initialize().await {
                tracing::warn!("Portal init failed: {e:?}");
            }
            let portal = Arc::new(portal);
            let coord = coordinator.clone();
            tokio::spawn(async move {
                while let Some(intent) = intent_rx.recv().await {
                    coord.handle_intent(intent);
                }
            });

            let dispatcher = bootstrap::create_ipc_dispatcher(
                runtime,
                coordinator,
                host_event_tx,
                intent_tx,
                Some(portal),
            );
            let cancel = CancellationToken::new();
            let handle =
                match bootstrap::bind_and_run_ipc(&socket_path, dispatcher, cancel.clone()).await {
                    Ok(h) => h,
                    Err(e) => {
                        tracing::error!("IPC: {e}");
                        return;
                    }
                };
            tracing::info!("Background services running");

            tokio::signal::ctrl_c().await.ok();
            cancel.cancel();
            bootstrap::shutdown(handle).await;
        });
    });

    tracing::info!("Launching Iced on main thread");
    let iced_result = flick_shell_iced::app::run(ui_cmd_tx, host_event_rx);
    let _ = bg_thread.join();

    match iced_result {
        Ok(()) => Ok(()),
        Err(e) => Err(format!("Iced: {e}")),
    }
}
