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

/// 托盘事件在 daemon 侧的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayAction {
    /// 唤起搜索面板。
    ShowPanel,
    /// 请求退出 daemon。
    Shutdown,
    /// 占位菜单项，仅记日志。
    Placeholder(&'static str),
}

/// 把托盘事件映射成 daemon 动作（纯函数，便于单测）。
fn tray_action(event: flick_adapters_linux::tray::TrayEvent) -> TrayAction {
    use flick_adapters_linux::tray::TrayEvent;
    match event {
        TrayEvent::Activate => TrayAction::ShowPanel,
        TrayEvent::Quit => TrayAction::Shutdown,
        TrayEvent::Home => TrayAction::Placeholder("主页"),
        TrayEvent::Settings => TrayAction::Placeholder("设置"),
        TrayEvent::About => TrayAction::Placeholder("关于"),
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

    /// 启动时自动绑定 show-search（会弹一次系统授权；可用 FLICK_DISABLE_AUTO_BIND=1 关闭）。
    fn spawn_startup_auto_bind(portal: Arc<PortalShortcutAdapter>) {
        if std::env::var_os("FLICK_DISABLE_AUTO_BIND").is_some() {
            return;
        }
        tokio::spawn(async move {
            let trigger = flick_adapters_linux::session::show_search_preferred_trigger();
            match portal.configure_search_shortcut(trigger).await {
                Ok(binding) => {
                    tracing::info!(?binding, "Portal show-search shortcut bound at startup")
                }
                Err(error) => tracing::warn!(
                    ?error,
                    "Portal startup binding failed; run `flick shortcuts configure` to bind manually"
                ),
            }
        });
    }

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
    // 以及工作线程）。UI 事件循环保留在进程主线程上。
    let bg_thread = std::thread::spawn(move || {
        let result = catch_unwind(AssertUnwindSafe(|| {
            let rt = tokio::runtime::Runtime::new().expect("failed to create bg runtime");
            rt.block_on(async move {
                let lifecycle = background_lifecycle;
                let dirs = flick_data::DataDirectories::discover().ok();
                let paths: std::sync::Arc<dyn flick_domain::PluginPaths> = match &dirs {
                    Some(dirs) => std::sync::Arc::new(dirs.clone()),
                    None => std::sync::Arc::new(flick_domain::NoopPaths),
                };
                let runtime = Arc::new(ActionRuntime::new(
                    std::sync::Arc::new(flick_adapters_linux::host_info::collect_host_info()),
                    paths,
                ));
                flick_builtins::register_builtin(&runtime)
                    .await
                    .expect("register builtin plugins");
                runtime.activate_all_plugins().await;

                let event_sink = Arc::new(bootstrap::ChannelEventSink {
                    tx: host_event_tx.clone(),
                });
                let runtime_port = Arc::new(bootstrap::RuntimeAdapter {
                    runtime: runtime.clone(),
                }) as Arc<dyn RuntimePort>;
                let coordinator = Arc::new(ApplicationCoordinator::new(runtime_port, event_sink));

                // 设置系统先于 UI 工作线程初始化：UI 线程要处理 `SetTheme`，
                // 还要把设置变更热转发给 shell（主题、结果条数上限）。
                let settings = bootstrap::init_settings(&runtime, dirs).await;
                if let Some(service) = &settings {
                    bootstrap::spawn_retention_worker(Arc::clone(service), lifecycle.token());
                    bootstrap::spawn_settings_forwarder(
                        Arc::clone(service),
                        host_event_tx.clone(),
                        lifecycle.token(),
                    );
                }

                let ui_worker_lifecycle = lifecycle.clone();
                let ui_coordinator = coordinator.clone();
                let ui_settings = settings.clone();
                let ui_worker = tokio::spawn(async move {
                    let mut rx = ui_cmd_rx;
                    loop {
                        tokio::select! {
                            _ = ui_worker_lifecycle.cancelled() => break,
                            command = rx.recv() => match command {
                                // 主题切换需要落库：写设置系统后由变更转发广播给所有窗口。
                                Some(UiToHost::SetTheme { mode }) => {
                                    if let Some(service) = &ui_settings {
                                        let key = flick_domain::SettingKey::new(
                                            flick_domain::SettingNamespace::Core,
                                            "theme.mode",
                                        );
                                        let value = flick_domain::settings::SettingValue::Enum(
                                            mode.as_str().to_string(),
                                        );
                                        if let Err(error) = service.set(&key, value) {
                                            tracing::warn!(%error, "持久化主题设置失败");
                                        }
                                    }
                                }
                                Some(command) => ui_coordinator.handle_ui(command).await,
                                None => break,
                            }
                        }
                    }
                });

                let (intent_tx, mut intent_rx) = mpsc::unbounded_channel::<UiIntent>();

                // 系统托盘：与全局快捷键共用 `intent_tx` 唤起面板。
                if flick_adapters_linux::tray::tray_enabled() {
                    let (tray_tx, mut tray_rx) = mpsc::unbounded_channel();
                    match flick_adapters_linux::tray::spawn_tray(tray_tx) {
                        Ok(()) => {
                            let tray_lifecycle = lifecycle.clone();
                            let tray_intents = intent_tx.clone();
                            tokio::spawn(async move {
                                loop {
                                    tokio::select! {
                                        _ = tray_lifecycle.cancelled() => break,
                                        event = tray_rx.recv() => match event {
                                            Some(event) => match tray_action(event) {
                                                TrayAction::ShowPanel => {
                                                    let _ = tray_intents.send(UiIntent::ShowSearchPanel {
                                                        activation_token: None,
                                                    });
                                                }
                                                TrayAction::Shutdown => {
                                                    tray_lifecycle
                                                        .request_shutdown(ShutdownReason::TrayQuit);
                                                    break;
                                                }
                                                TrayAction::Placeholder(name) => {
                                                    tracing::info!(menu = name, "托盘菜单项尚未实现");
                                                }
                                            },
                                            None => break,
                                        }
                                    }
                                }
                            });
                        }
                        Err(error) => tracing::warn!(%error, "系统托盘未启动"),
                    }
                }
                let portal = Arc::new(PortalShortcutAdapter::new_with_client(
                    Arc::new(bootstrap::ChannelIntentSink {
                        tx: intent_tx.clone(),
                    }),
                    Arc::new(AshpdPortalClient::new()),
                ));
                match portal.initialize().await {
                    Ok(snapshot) => {
                        tracing::info!(?snapshot, "Portal initialized");
                        // 启动即申请全局快捷键：GNOME 的 GlobalShortcuts 走
                        // BindShortcuts，会弹一次系统授权对话框。放到后台任务里
                        // 执行，避免阻塞 IPC/UI 启动。设置 FLICK_DISABLE_AUTO_BIND=1
                        // 可关闭自动绑定（改用 `flick shortcuts configure`）。
                        spawn_startup_auto_bind(Arc::clone(&portal));
                    }
                    Err(error) => {
                        let snapshot = portal.diagnostics().await;
                        tracing::warn!(
                            ?error,
                            ?snapshot,
                            "Portal init failed; IPC remains available"
                        );
                        // 启动竞态自愈：登录时 xdg-desktop-portal / GNOME 后端可能
                        // 尚未就绪，首轮 CreateSession 会以 unavailable 失败。
                        // 后台有限次重试，成功后自动绑定（FLICK_DISABLE_PORTAL_RETRY=1 可关闭）。
                        if std::env::var_os("FLICK_DISABLE_PORTAL_RETRY").is_none() {
                            let retry_portal = Arc::clone(&portal);
                            tokio::spawn(async move {
                                for attempt in 1..=10u32 {
                                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                                    match retry_portal.reconnect().await {
                                        Ok(snapshot) => {
                                            tracing::info!(
                                                ?snapshot,
                                                attempt,
                                                "Portal reconnected after startup race"
                                            );
                                            spawn_startup_auto_bind(retry_portal);
                                            return;
                                        }
                                        Err(retry_error) => tracing::debug!(
                                            ?retry_error,
                                            attempt,
                                            "Portal startup retry failed"
                                        ),
                                    }
                                }
                                tracing::warn!(
                                    "Portal startup retries exhausted; run `flick shortcuts reconnect` to retry"
                                );
                            });
                        }
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
                    settings,
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
    let shell_config = bootstrap::load_shell_config();
    flick_shell_gpui::run_with_config(ui_cmd_tx, host_event_rx, shell_config);
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

#[cfg(test)]
mod tray_tests {
    use super::{TrayAction, tray_action};
    use flick_adapters_linux::tray::TrayEvent;

    #[test]
    fn activate_shows_panel() {
        assert_eq!(tray_action(TrayEvent::Activate), TrayAction::ShowPanel);
    }

    #[test]
    fn quit_requests_shutdown() {
        assert_eq!(tray_action(TrayEvent::Quit), TrayAction::Shutdown);
    }

    #[test]
    fn placeholders_are_logged_only() {
        assert_eq!(
            tray_action(TrayEvent::Home),
            TrayAction::Placeholder("主页")
        );
        assert_eq!(
            tray_action(TrayEvent::Settings),
            TrayAction::Placeholder("设置")
        );
        assert_eq!(
            tray_action(TrayEvent::About),
            TrayAction::Placeholder("关于")
        );
    }
}
