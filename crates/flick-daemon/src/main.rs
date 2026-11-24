//! flick-daemon：组合根目录和入口点。

mod bootstrap;
mod headless;
mod lifecycle;

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

fn socket_path(
    custom: Option<PathBuf>,
    mode: lifecycle::SocketMode,
) -> Result<PathBuf, lifecycle::SocketPathError> {
    lifecycle::resolve_socket_path(custom, mode, |key| std::env::var(key).ok())
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let path = match socket_path(
        args.socket,
        if args.headless {
            lifecycle::SocketMode::Headless
        } else {
            lifecycle::SocketMode::Graphical
        },
    ) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("Error: {error}");
            process::exit(2);
        }
    };

    if args.headless {
        let rt = tokio::runtime::Runtime::new().expect("failed to create runtime");
        match rt.block_on(bootstrap::run_headless(path)) {
            Ok(()) => process::exit(0),
            Err(message) => {
                eprintln!("Error: {message}");
                process::exit(1);
            }
        }
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

    use crate::lifecycle::{DaemonLifecycle, ShutdownReason};
    use flick_adapters_linux::portal::PortalShortcutAdapter;
    use flick_adapters_linux::portal_client::AshpdPortalClient;
    use flick_application::coordinator::ApplicationCoordinator;
    use flick_application::ports::RuntimePort;
    use flick_domain::*;
    use flick_runtime::ActionRuntime;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };
    use tokio::signal::unix::{SignalKind, signal};
    use tokio::sync::mpsc;

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
    let lifecycle = DaemonLifecycle::new();
    let background_lifecycle = lifecycle.clone();
    let background_error = Arc::new(Mutex::new(None));
    let background_error_for_thread = Arc::clone(&background_error);
    let background_panicked = Arc::new(AtomicBool::new(false));
    let background_panicked_for_thread = Arc::clone(&background_panicked);
    let panic_lifecycle = background_lifecycle.clone();
    let panic_host_event_tx = host_event_tx.clone();
    let panic_error = Arc::clone(&background_error);
    let socket_path_for_cleanup = socket_path.clone();

    // 后台线程：所有由 Tokio 拥有的服务（IPC、Portal、协调器，
    // 以及工作线程）。Iced 事件循环保留在进程主线程上。
    let bg_thread = std::thread::spawn(move || {
        let result = catch_unwind(AssertUnwindSafe(|| {
            let rt = tokio::runtime::Runtime::new().expect("failed to create bg runtime");
            rt.block_on(async move {
                let lifecycle = background_lifecycle;
                let runtime = Arc::new(ActionRuntime::new());
                flick_builtins::register_phase0_builtins(&runtime).await;

                let event_sink = Arc::new(bootstrap::ChannelEventSink {
                    tx: host_event_tx.clone(),
                });
                let runtime_port = Arc::new(bootstrap::RuntimeAdapter {
                    runtime: runtime.clone(),
                }) as Arc<dyn RuntimePort>;
                let coordinator = Arc::new(ApplicationCoordinator::new(runtime_port, event_sink));

                let ui_worker_lifecycle = lifecycle.clone();
                let ui_coordinator = coordinator.clone();
                let ui_worker = tokio::spawn(async move {
                    let mut rx = ui_cmd_rx;
                    loop {
                        tokio::select! {
                            _ = ui_worker_lifecycle.cancelled() => break,
                            command = rx.recv() => match command {
                                Some(command) => ui_coordinator.handle_ui(command).await,
                                None => break,
                            }
                        }
                    }
                });

                let (intent_tx, mut intent_rx) = mpsc::unbounded_channel::<UiIntent>();
                let portal = Arc::new(PortalShortcutAdapter::new_with_client(
                    Arc::new(bootstrap::ChannelIntentSink {
                        tx: intent_tx.clone(),
                    }),
                    Arc::new(AshpdPortalClient::new()),
                ));
                match portal.initialize().await {
                    Ok(snapshot) => tracing::info!(?snapshot, "Portal initialized"),
                    Err(error) => {
                        let snapshot = portal.diagnostics().await;
                        tracing::warn!(
                            ?error,
                            ?snapshot,
                            "Portal init failed; IPC remains available"
                        );
                    }
                }

                let intent_worker_lifecycle = lifecycle.clone();
                let intent_coordinator = coordinator.clone();
                let intent_worker = tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            _ = intent_worker_lifecycle.cancelled() => break,
                            intent = intent_rx.recv() => match intent {
                                Some(intent) => intent_coordinator.handle_intent(intent),
                                None => break,
                            }
                        }
                    }
                });

                let dispatcher = bootstrap::create_ipc_dispatcher(
                    runtime,
                    coordinator,
                    host_event_tx.clone(),
                    intent_tx,
                    Some(Arc::clone(&portal)),
                    socket_path.clone(),
                    lifecycle.clone(),
                );
                let handle =
                    match bootstrap::bind_and_run_ipc(&socket_path, dispatcher, lifecycle.token())
                        .await
                    {
                        Ok(handle) => Some(handle),
                        Err(error) => {
                            let message = format!("IPC: {error}");
                            tracing::error!("{message}");
                            *background_error_for_thread
                                .lock()
                                .expect("background error mutex poisoned") = Some(message);
                            lifecycle.request_shutdown(ShutdownReason::StartupFailed);
                            None
                        }
                    };

                let mut sigterm = match signal(SignalKind::terminate()) {
                    Ok(signal) => Some(signal),
                    Err(error) => {
                        tracing::warn!(%error, "SIGTERM handling is unavailable");
                        None
                    }
                };

                if let Some(sigterm) = sigterm.as_mut() {
                    tokio::select! {
                        result = tokio::signal::ctrl_c() => {
                            if let Err(error) = result {
                                tracing::warn!(%error, "Ctrl+C handling failed");
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
                                tracing::warn!(%error, "Ctrl+C handling failed");
                            }
                            lifecycle.request_shutdown(ShutdownReason::Signal);
                        }
                        _ = lifecycle.cancelled() => {}
                    }
                }

                let _ = host_event_tx.send(HostToUi::Shutdown);
                portal.shutdown().await;
                if let Some(handle) = handle {
                    bootstrap::shutdown(handle).await;
                }
                let _ = ui_worker.await;
                let _ = intent_worker.await;
            });
        }));

        if result.is_err() {
            let message = "background runtime panicked".to_string();
            tracing::error!("{message}");
            *panic_error.lock().expect("background error mutex poisoned") = Some(message);
            background_panicked_for_thread.store(true, Ordering::Release);
            panic_lifecycle.request_shutdown(ShutdownReason::StartupFailed);
            let _ = panic_host_event_tx.send(HostToUi::Shutdown);
        }
    });

    tracing::info!("Launching GPUI on main thread");
    flick_shell_gpui::run(ui_cmd_tx, host_event_rx);
    lifecycle.request_shutdown(ShutdownReason::ShellExited);
    let background_thread_panicked = bg_thread.join().is_err();
    if background_thread_panicked || background_panicked.load(Ordering::Acquire) {
        let _ = std::fs::remove_file(&socket_path_for_cleanup);
    }

    match background_error
        .lock()
        .expect("background error mutex poisoned")
        .take()
    {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
