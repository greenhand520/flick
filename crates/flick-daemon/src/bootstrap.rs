//! flick-daemon 的引导辅助函数。
//!
//! main.rs 共享的类型和函数，用于图形和无头两种模式。

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
use flick_data::StorageManager;
use flick_settings::{SettingsService, core_schema};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

// ─── 适配器类型 ───────────────────────────────────────────────────────────

pub struct RuntimeAdapter {
    pub runtime: Arc<ActionRuntime>,
}

#[async_trait::async_trait]
impl RuntimePort for RuntimeAdapter {
    async fn route(&self, query: &str, input_kind: InputType) -> Vec<RouteItem> {
        self.runtime.route(query, input_kind).await
    }
    async fn route_input(&self, context: &InputContext) -> Vec<RouteItem> {
        self.runtime.route_input(context).await
    }
    async fn search_invoke(
        &self,
        generation: QueryGeneration,
        input: ActionInput,
    ) -> Vec<RouteItem> {
        self.runtime.search_invoke(generation, input).await
    }
    async fn execution_event(
        &self,
        parent_action_id: ActionId,
        item_id: &str,
        event: ExecutionEventKind,
    ) -> Option<ActionOutcome> {
        self.runtime
            .execution_event(parent_action_id, item_id, event)
            .await
    }
    async fn invoke(&self, request: ActionRequest) -> ActionOutcome {
        self.runtime.invoke(request).await
    }
    async fn cancel(&self, invocation_id: InvocationId) -> bool {
        self.runtime.cancel(invocation_id).await
    }
    fn set_usage_store(&self, store: std::sync::Arc<dyn flick_domain::UsageStore>) {
        self.runtime.set_usage_store(store);
    }
    async fn surface_event(
        &self,
        session: SurfaceSessionId,
        kind: SurfaceEventKind,
    ) -> Result<SurfaceContent, String> {
        self.runtime
            .surface_event(session, kind)
            .await
            .map_err(|e| e.message)
    }
    async fn surface_closed(&self, session: SurfaceSessionId) {
        self.runtime.surface_closed(session).await;
    }
    async fn dependency_status(&self, plugin_id: &str) -> Vec<DependencyStatus> {
        self.runtime.dependency_status(plugin_id).await
    }
    async fn registered_plugin_ids(&self) -> Vec<String> {
        self.runtime.registered_plugin_ids().await
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

// ─── IPC 调度器 ──────────────────────────────────────────────────────────

pub struct DaemonIpcDispatcher {
    pub runtime: Arc<dyn RuntimePort>,
    /// 设置服务；`None` 表示设置系统不可用（启动时打开存储失败）。
    pub settings: Option<Arc<SettingsService>>,
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
                    source: flick_domain::ActionSource::Ui,
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
                    ActionOutcome::Text(text) => IpcResult::Ok(serde_json::json!({ "text": text })),
                    ActionOutcome::Url(url) => IpcResult::Ok(serde_json::json!({ "url": url })),
                    ActionOutcome::File(file) => IpcResult::Ok(
                        serde_json::to_value(&file).unwrap_or(serde_json::Value::Null),
                    ),
                    ActionOutcome::OpenSurface {
                        surface_id,
                        content,
                        ..
                    } => IpcResult::Ok(
                        serde_json::json!({ "surface_id": surface_id, "content": content }),
                    ),
                }
            }
            IpcCommand::Status => IpcResult::Ok(serde_json::json!({ "daemon_status": "running" })),
            IpcCommand::Doctor => {
                let portal = match &self.portal {
                    Some(portal) => portal_snapshot_value(&portal.diagnostics().await),
                    None => serde_json::json!({ "state": "headless_not_started" }),
                };
                let plugins = self.plugin_dependency_report().await;
                let settings = match &self.settings {
                    Some(service) => serde_json::json!({
                        "available": true,
                        "namespaces": service.registered_namespaces(),
                    }),
                    None => serde_json::json!({ "available": false, "namespaces": [] }),
                };
                IpcResult::Ok(serde_json::json!({
                    "session": probe_session(),
                    "portal": portal,
                    "ipc": { "socket": self.socket_path.display().to_string() },
                    "plugins": plugins,
                    "settings": settings,
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
            IpcCommand::SettingsList { namespace } => {
                let Some(settings) = &self.settings else {
                    return settings_unavailable();
                };
                match settings.list(&namespace) {
                    Ok(snapshot) => IpcResult::Ok(serde_json::json!({
                        "namespace": snapshot.namespace.as_str(),
                        "revision": snapshot.revision,
                        "values": snapshot.values,
                    })),
                    Err(error) => settings_error(error),
                }
            }
            IpcCommand::SettingsGet { key } => {
                let Some(settings) = &self.settings else {
                    return settings_unavailable();
                };
                let parsed = match SettingKey::parse(&key) {
                    Ok(parsed) => parsed,
                    Err(error) => {
                        return IpcResult::Error {
                            code: IpcErrorCode::InvalidInput,
                            message: error.to_string(),
                        };
                    }
                };
                match settings.get(&parsed) {
                    Ok(Some(value)) => IpcResult::Ok(serde_json::json!({
                        "key": key,
                        "value": value,
                        "is_default": false,
                    })),
                    Ok(None) => match settings.resolved(&parsed) {
                        Ok(value) => IpcResult::Ok(serde_json::json!({
                            "key": key,
                            "value": value,
                            "is_default": true,
                        })),
                        Err(error) => settings_error(error),
                    },
                    Err(error) => settings_error(error),
                }
            }
            IpcCommand::SettingsSet { key, value } => {
                let Some(settings) = &self.settings else {
                    return settings_unavailable();
                };
                let parsed = match SettingKey::parse(&key) {
                    Ok(parsed) => parsed,
                    Err(error) => {
                        return IpcResult::Error {
                            code: IpcErrorCode::InvalidInput,
                            message: error.to_string(),
                        };
                    }
                };
                match settings.set(&parsed, value) {
                    Ok(revision) => IpcResult::Ok(serde_json::json!({
                        "key": key,
                        "revision": revision,
                    })),
                    Err(error) => settings_error(error),
                }
            }
            IpcCommand::SettingsReset { key, namespace } => {
                let Some(settings) = &self.settings else {
                    return settings_unavailable();
                };
                match (key, namespace) {
                    (Some(key), None) => {
                        let parsed = match SettingKey::parse(&key) {
                            Ok(parsed) => parsed,
                            Err(error) => {
                                return IpcResult::Error {
                                    code: IpcErrorCode::InvalidInput,
                                    message: error.to_string(),
                                };
                            }
                        };
                        match settings.reset_key(&parsed) {
                            Ok(revision) => {
                                IpcResult::Ok(serde_json::json!({ "key": key, "revision": revision }))
                            }
                            Err(error) => settings_error(error),
                        }
                    }
                    (None, Some(namespace)) => match settings.reset_namespace(&namespace) {
                        Ok(revision) => IpcResult::Ok(serde_json::json!({
                            "namespace": namespace,
                            "revision": revision,
                        })),
                        Err(error) => settings_error(error),
                    },
                    _ => IpcResult::Error {
                        code: IpcErrorCode::InvalidInput,
                        message: "settings reset 需要且只接受 key 或 namespace 之一".into(),
                    },
                }
            }
        }
    }

    async fn after_response(&self, command: &IpcCommand) {
        if matches!(command, IpcCommand::Shutdown) {
            self.lifecycle.request_shutdown(ShutdownReason::Ipc);
        }
    }
}

impl DaemonIpcDispatcher {
    /// 汇总所有已注册插件的依赖状态，供 `flick doctor` 输出。
    ///
    /// 插件按 id 升序排列（注册表用 `BTreeMap` 存 manifest），
    /// 保证同一台机器上多次 doctor 的输出稳定。
    async fn plugin_dependency_report(&self) -> Vec<serde_json::Value> {
        let mut report = Vec::new();
        for plugin_id in self.runtime.registered_plugin_ids().await {
            let dependencies = self.runtime.dependency_status(&plugin_id).await;
            report.push(serde_json::json!({
                "plugin_id": plugin_id,
                "dependencies": dependencies,
            }));
        }
        report
    }
}

/// 设置系统不可用时的统一错误。
fn settings_unavailable() -> IpcResult {
    IpcResult::Error {
        code: IpcErrorCode::SettingsUnavailable,
        message: "设置系统不可用（守护进程未能打开设置存储）".into(),
    }
}

/// 把设置层错误映射为 IPC 错误码。
fn settings_error(error: flick_settings::SettingsError) -> IpcResult {
    use flick_settings::SettingsError;
    let code = match &error {
        SettingsError::UnknownNamespace(_) | SettingsError::UnknownKey { .. } => {
            IpcErrorCode::InvalidInput
        }
        SettingsError::InvalidValue { .. } | SettingsError::InvalidSchema { .. } => {
            IpcErrorCode::InvalidInput
        }
        SettingsError::CorruptValue { .. }
        | SettingsError::SchemaVersionUnsupported { .. }
        | SettingsError::PermissionDenied { .. }
        | SettingsError::SubscriptionClosed
        | SettingsError::Storage(_) => IpcErrorCode::Internal,
    };
    IpcResult::Error {
        code,
        message: error.to_string(),
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

// ─── 公共辅助函数 ──────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub fn create_ipc_dispatcher(
    runtime: Arc<ActionRuntime>,
    settings: Option<Arc<SettingsService>>,
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
        settings,
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
    // Task 4 有意只移除套接字。运行时目录的所有权
    // 跟踪属于 Task 5，因此此处绝不删除父目录。
    let _ = tokio::fs::remove_file(&handle.socket_path).await;
    info!("flick-daemon stopped");
}

// ─── 无头模式 ──────────────────────────────────────────────────────────

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
                    Some(HostToUi::ExecutionResults { generation, items }) => {
                        info!(
                            "UI: ExecutionResults for gen {generation}: {} items",
                            items.len()
                        );
                    }
                    Some(HostToUi::InvocationCompleted { invocation_id, .. }) => {
                        info!("UI: Invocation {invocation_id} completed");
                    }
                    Some(HostToUi::RuntimeStatus(status)) => info!("UI: Runtime status: {status:?}"),
                    Some(HostToUi::SurfaceUpdated { session_id, .. }) => {
                        info!("UI: Surface {session_id} updated")
                    }
                    Some(HostToUi::SurfaceFailed { session_id, message }) => {
                        info!("UI: Surface {session_id} failed: {message}")
                    }
                }
            }
        }
    });

    let settings = init_settings(&runtime).await;
    let dispatcher = create_ipc_dispatcher(
        runtime,
        settings,
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

// ─── 设置系统装配 ─────────────────────────────────────────────────────────

/// 软卸载数据保留期（天）。
pub const UNINSTALLED_RETENTION_DAYS: u64 = 30;

/// 初始化设置服务。
///
/// 返回 `None` 表示设置系统不可用（例如主进程无法打开设置存储）；
/// 调用方应继续启动，相关 IPC 命令返回不可用即可。
pub async fn init_settings(runtime: &ActionRuntime) -> Option<Arc<SettingsService>> {
    let dirs = match flick_data::DataDirectories::discover() {
        Ok(dirs) => dirs,
        Err(error) => {
            tracing::warn!(%error, "设置系统不可用：无法解析数据目录");
            return None;
        }
    };
    let keys: flick_data::SharedKeySource = if std::env::var_os("FLICK_MASTER_KEY").is_some() {
        Arc::new(flick_data::EnvMasterKeySource)
    } else {
        // 密钥来源缺失时仍可启动：明文字段可用，secret 读写会明确报错。
        Arc::new(flick_data::UnavailableMasterKeySource)
    };
    init_settings_at(runtime, dirs, keys).await
}

/// 在指定目录与主密钥来源上初始化设置服务（便于测试注入）。
pub async fn init_settings_at(
    runtime: &ActionRuntime,
    dirs: flick_data::DataDirectories,
    keys: flick_data::SharedKeySource,
) -> Option<Arc<SettingsService>> {
    let storage = match StorageManager::open(dirs, keys) {
        Ok(storage) => Arc::new(storage),
        Err(error) => {
            tracing::warn!(%error, "设置系统不可用：无法打开设置存储");
            return None;
        }
    };

    let service = Arc::new(SettingsService::new(storage));

    let version = env!("CARGO_PKG_VERSION");
    if let Err(error) = service.register_schema(core_schema(), version, version) {
        tracing::warn!(%error, "核心设置 schema 注册失败");
    }

    for manifest in runtime.plugin_manifests().await {
        let schema = SettingSchema {
            namespace: SettingNamespace::Plugin(manifest.id.clone()),
            declarations: manifest.settings.clone(),
            schema_version: 1,
        };
        if let Err(error) =
            service.register_schema(schema, &manifest.version, &manifest.version)
        {
            tracing::warn!(plugin = %manifest.id, %error, "插件设置 schema 注册失败");
        }
    }

    match service
        .storage()
        .purge_uninstalled_older_than(UNINSTALLED_RETENTION_DAYS)
    {
        Ok(purged) if !purged.is_empty() => {
            tracing::info!(count = purged.len(), ?purged, "已清理过期卸载数据");
        }
        Ok(_) => {}
        Err(error) => tracing::warn!(%error, "清理过期卸载数据失败"),
    }

    Some(service)
}

/// 读取初始主题。
///
/// 顺序：设置系统 `core.theme.mode` > 旧 `shell-gpui.toml` 迁移 > shell 自带探测。
pub fn load_initial_theme() -> flick_shell_gpui::ThemeMode {
    let dirs = match flick_data::DataDirectories::discover() {
        Ok(dirs) => dirs,
        Err(_) => return flick_shell_gpui::detect_initial_theme(),
    };
    let keys: flick_data::SharedKeySource = if std::env::var_os("FLICK_MASTER_KEY").is_some() {
        Arc::new(flick_data::EnvMasterKeySource)
    } else {
        Arc::new(flick_data::UnavailableMasterKeySource)
    };
    initial_theme_at(dirs, keys)
}

/// 在指定目录上读取初始主题（便于测试注入）。
pub fn initial_theme_at(
    dirs: flick_data::DataDirectories,
    keys: flick_data::SharedKeySource,
) -> flick_shell_gpui::ThemeMode {
    let storage = match StorageManager::open(dirs.clone(), keys) {
        Ok(storage) => Arc::new(storage),
        Err(error) => {
            tracing::warn!(%error, "读取主题失败：设置存储不可用，回退自动探测");
            return flick_shell_gpui::detect_initial_theme();
        }
    };
    let service = SettingsService::new(storage);
    let version = env!("CARGO_PKG_VERSION");
    if let Err(error) = service.register_schema(core_schema(), version, version) {
        tracing::warn!(%error, "读取主题失败：core schema 注册失败");
        return flick_shell_gpui::detect_initial_theme();
    }
    migrate_legacy_theme(&service, &dirs);
    theme_mode(&service).unwrap_or_else(flick_shell_gpui::detect_initial_theme)
}

/// 从设置系统读取主题模式；未设置或取值非法时返回 `None`。
pub fn theme_mode(service: &SettingsService) -> Option<flick_shell_gpui::ThemeMode> {
    let key = flick_domain::SettingKey::new(flick_domain::SettingNamespace::Core, "theme.mode");
    match service.resolved(&key).ok()? {
        flick_domain::settings::SettingValue::Enum(mode) if mode == "light" => {
            Some(flick_shell_gpui::ThemeMode::Light)
        }
        flick_domain::settings::SettingValue::Enum(mode) if mode == "dark" => {
            Some(flick_shell_gpui::ThemeMode::Dark)
        }
        _ => None,
    }
}

/// 把旧的 `shell-gpui.toml`（仅 `mode` 字段）迁移进设置系统，然后重命名文件。
///
/// 只在设置系统里还没有显式值时写入；无论是否写入，迁移完成后都会把文件
/// 重命名为 `shell-gpui.toml.migrated`，避免下次重复迁移。
pub fn migrate_legacy_theme(service: &SettingsService, dirs: &flick_data::DataDirectories) {
    let path = dirs.config_root().join("shell-gpui.toml");
    if !path.exists() {
        return;
    }
    let key = flick_domain::SettingKey::new(flick_domain::SettingNamespace::Core, "theme.mode");
    let already_set = matches!(service.get(&key), Ok(Some(_)));
    if !already_set {
        if let Some(mode) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| parse_legacy_theme_mode(&text))
        {
            let value = flick_domain::settings::SettingValue::Enum(mode);
            match service.set(&key, value) {
                Ok(_) => tracing::info!("已把旧主题配置迁移进设置系统"),
                Err(error) => tracing::warn!(%error, "迁移旧主题配置失败"),
            }
        }
    }
    let migrated = path.with_extension("toml.migrated");
    if let Err(error) = std::fs::rename(&path, &migrated) {
        tracing::warn!(%error, "重命名旧主题配置文件失败");
    }
}

/// 解析旧主题文件里的 `mode = "dark" | "light"`。
fn parse_legacy_theme_mode(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line.split_once('=')?;
        if key.trim() != "mode" {
            continue;
        }
        let value = value.trim().trim_matches('"').trim_matches('\'');
        return match value {
            "dark" | "light" => Some(value.to_string()),
            _ => None,
        };
    }
    None
}

#[cfg(test)]
mod init_settings_tests {
    use super::*;

    async fn runtime_with_builtins() -> Arc<ActionRuntime> {
        let runtime = Arc::new(ActionRuntime::new());
        flick_builtins::register_builtin(&runtime)
            .await
            .expect("register builtin plugins");
        runtime.activate_all_plugins().await;
        runtime
    }

    #[test]
    fn legacy_theme_is_migrated_and_file_renamed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let dirs = flick_data::DataDirectories::with_roots(
            root.join("config"),
            root.join("data"),
            root.join("cache"),
            root.join("runtime"),
        );
        std::fs::create_dir_all(dirs.config_root()).unwrap();
        let legacy = dirs.config_root().join("shell-gpui.toml");
        std::fs::write(&legacy, "mode = \"light\"\n").unwrap();

        let keys: flick_data::SharedKeySource = Arc::new(flick_data::FixedMasterKeySource(
            flick_data::MasterKey {
                key_id: "test".into(),
                bytes: [4u8; 32],
            },
        ));
        let theme = initial_theme_at(dirs.clone(), keys);
        assert_eq!(theme, flick_shell_gpui::ThemeMode::Light);
        assert!(!legacy.exists(), "旧文件应被重命名");
        assert!(dirs.config_root().join("shell-gpui.toml.migrated").exists());
    }

    #[tokio::test]
    async fn registers_core_and_plugin_settings() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let dirs = flick_data::DataDirectories::with_roots(
            root.join("config"),
            root.join("data"),
            root.join("cache"),
            root.join("runtime"),
        );
        let keys: flick_data::SharedKeySource = Arc::new(flick_data::FixedMasterKeySource(
            flick_data::MasterKey {
                key_id: "test".into(),
                bytes: [3u8; 32],
            },
        ));
        let runtime = runtime_with_builtins().await;

        let service = init_settings_at(&runtime, dirs, keys)
            .await
            .expect("设置服务应可用");
        assert!(service.registered_namespaces().iter().any(|ns| ns == "core"));

        let mode =
            flick_domain::SettingKey::new(flick_domain::SettingNamespace::Core, "theme.mode");
        assert_eq!(
            service.resolved(&mode).unwrap(),
            flick_domain::SettingValue::Enum("dark".into())
        );

        for plugin_id in runtime.registered_plugin_ids().await {
            assert!(
                service.schema(&plugin_id).is_some(),
                "插件 {plugin_id} 的设置 schema 应已注册"
            );
        }
    }
}
