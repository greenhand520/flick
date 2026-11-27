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
    fn set_usage_store(&self, store: std::sync::Arc<dyn flick_domain::UsageStore>) {
        self.runtime.set_usage_store(store);
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
                let data = match &self.settings {
                    Some(service) => {
                        let dirs = service.storage().directories();
                        serde_json::json!({
                            "root": dirs.data_root().display().to_string(),
                            "size_bytes": dir_size(dirs.data_root()),
                        })
                    }
                    None => serde_json::json!({ "root": "", "size_bytes": 0 }),
                };
                IpcResult::Ok(serde_json::json!({
                    "session": probe_session(),
                    "portal": portal,
                    "ipc": { "socket": self.socket_path.display().to_string() },
                    "plugins": plugins,
                    "settings": settings,
                    "data": data,
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

/// 把 [`SettingsService`] 适配成运行时需要的两个句柄：
/// [`flick_plugin::SettingsProvider`]（快照 + 订阅）与
/// [`flick_domain::settings::SettingSecretReader`]（实时解密 secret）。
pub struct SettingsAdapter {
    service: Arc<SettingsService>,
}

impl SettingsAdapter {
    /// 用共享的设置服务构造适配器。
    pub fn new(service: Arc<SettingsService>) -> Self {
        Self { service }
    }

    /// 解析某插件的快照：合并声明默认值、**跳过 secret**。
    fn snapshot_of(service: &SettingsService, plugin_id: &str) -> ConfigSnapshot {
        let Some(schema) = service.schema(plugin_id) else {
            return ConfigSnapshot::empty();
        };
        let namespace = SettingNamespace::Plugin(plugin_id.to_string());
        let mut values = std::collections::BTreeMap::new();
        for decl in &schema.declarations {
            if decl.secret {
                continue;
            }
            let key = SettingKey::new(namespace.clone(), decl.key.clone());
            if let Ok(value) = service.resolved(&key) {
                values.insert(decl.key.clone(), value);
            }
        }
        let revision = service.list(plugin_id).map(|s| s.revision).unwrap_or(0);
        ConfigSnapshot {
            namespace: plugin_id.to_string(),
            revision,
            values,
        }
    }
}

impl flick_plugin::SettingsProvider for SettingsAdapter {
    fn snapshot(&self, plugin_id: &str) -> ConfigSnapshot {
        Self::snapshot_of(&self.service, plugin_id)
    }

    fn subscribe(&self, plugin_id: &str, listener: Arc<dyn Fn(ConfigSnapshot) + Send + Sync>) {
        let weak = Arc::downgrade(&self.service);
        let plugin = plugin_id.to_string();
        let mut receiver = self.service.subscribe();
        tokio::spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(change) => {
                        if change.namespace.as_str() != plugin {
                            continue;
                        }
                        let Some(service) = weak.upgrade() else { break };
                        listener(SettingsAdapter::snapshot_of(&service, &plugin));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(plugin = %plugin, skipped, "设置变更订阅落后，重新推送快照");
                        let Some(service) = weak.upgrade() else { break };
                        listener(SettingsAdapter::snapshot_of(&service, &plugin));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

impl flick_domain::settings::SettingSecretReader for SettingsAdapter {
    fn secret(&self, plugin_id: &str, key: &str) -> Option<String> {
        let key = SettingKey::new(SettingNamespace::Plugin(plugin_id.to_string()), key);
        match self.service.get(&key) {
            Ok(Some(flick_domain::settings::SettingValue::String(value))) => Some(value),
            Ok(_) => None,
            Err(error) => {
                tracing::debug!(%error, plugin = %plugin_id, "读取 secret 失败");
                None
            }
        }
    }
}

/// 每日清理一次过期卸载数据的后台任务。
///
/// 返回的句柄在 `cancel` 触发后结束；每次清理放到 blocking 线程池，
/// 避免 SQLite 读写阻塞异步执行器。
/// 把设置系统的变更事件转发给 UI（主题、结果条数上限热生效）。
///
/// 只有 `core` 命名空间的变化才会重读受 UI 关注的键；且仅当值真的变了才发事件，
/// 避免每次写入都触发一次无谓的重绘。任务在 `cancel` 触发或广播通道关闭时退出。
pub fn spawn_settings_forwarder(
    service: Arc<SettingsService>,
    host_tx: tokio::sync::mpsc::UnboundedSender<HostToUi>,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    // 同步订阅：若在任务体内订阅，调用方在 spawn 之后立刻写设置可能先于订阅发布，
    // 变更会被广播丢弃（没有接收者）。
    let mut rx = service.subscribe();
    let mut last_theme = theme_mode(&service);
    let mut last_limit = result_limit(&service);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                changed = rx.recv() => match changed {
                    Ok(change) => {
                        if change.namespace != flick_domain::SettingNamespace::Core {
                            continue;
                        }
                        let theme = theme_mode(&service);
                        if theme != last_theme {
                            last_theme = theme;
                            if let Some(mode) = theme {
                                let _ = host_tx.send(HostToUi::ThemeChanged { mode: mode.into() });
                            }
                        }
                        let limit = result_limit(&service);
                        if limit != last_limit {
                            last_limit = limit;
                            let _ = host_tx.send(HostToUi::ResultLimitChanged { limit });
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                },
            }
        }
    })
}

pub fn spawn_retention_worker(
    service: Arc<SettingsService>,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(24 * 60 * 60));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // 首轮立即执行一次：进程可能长期不重启。
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = ticker.tick() => {
                    let service = Arc::clone(&service);
                    let result = tokio::task::spawn_blocking(move || {
                        service
                            .storage()
                            .purge_uninstalled_older_than(UNINSTALLED_RETENTION_DAYS)
                    })
                    .await;
                    match result {
                        Ok(Ok(purged)) if !purged.is_empty() => {
                            tracing::info!(count = purged.len(), ?purged, "已清理过期卸载数据");
                        }
                        Ok(Ok(_)) => {}
                        Ok(Err(error)) => tracing::warn!(%error, "清理过期卸载数据失败"),
                        Err(error) => tracing::warn!(%error, "清理任务 join 失败"),
                    }
                }
            }
        }
    })
}

/// 目录总大小（字节）；读取失败的文件按 0 计。
pub fn dir_size(path: &std::path::Path) -> u64 {
    let mut total = 0u64;
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            total = total.saturating_add(dir_size(&entry.path()));
        } else {
            total = total.saturating_add(metadata.len());
        }
    }
    total
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
                    Some(HostToUi::ThemeChanged { mode }) => {
                        info!("UI: theme changed to {mode:?}")
                    }
                    Some(HostToUi::ResultLimitChanged { limit }) => {
                        info!("UI: result limit changed to {limit}")
                    }
                }
            }
        }
    });

    let settings = init_settings(&runtime).await;
    if let Some(service) = &settings {
        spawn_retention_worker(Arc::clone(service), lifecycle.token());
    }
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
    // 路径能力与设置存储解耦：即使 core.sqlite3 打开失败，插件仍可拿到自己的目录。
    runtime.set_plugin_paths(Arc::new(dirs.clone()) as Arc<dyn PluginPaths>);

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

    let adapter = Arc::new(SettingsAdapter::new(Arc::clone(&service)));
    runtime.set_settings_provider(Arc::clone(&adapter) as Arc<dyn flick_plugin::SettingsProvider>);
    runtime
        .set_settings_secrets(Arc::clone(&adapter) as Arc<dyn flick_domain::settings::SettingSecretReader>);

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
/// 读取外壳启动配置：主题模式 + 结果条数上限。
///
/// 设置系统不可用时整体回退：主题走自动探测，条数上限用声明默认值，
/// 保证外壳仍能启动。
pub fn load_shell_config() -> flick_shell_gpui::ShellConfig {
    let dirs = match flick_data::DataDirectories::discover() {
        Ok(dirs) => dirs,
        Err(_) => {
            return flick_shell_gpui::ShellConfig {
                theme: flick_shell_gpui::detect_initial_theme(),
                ..flick_shell_gpui::ShellConfig::default()
            }
        }
    };
    let keys: flick_data::SharedKeySource = if std::env::var_os("FLICK_MASTER_KEY").is_some() {
        Arc::new(flick_data::EnvMasterKeySource)
    } else {
        Arc::new(flick_data::UnavailableMasterKeySource)
    };
    shell_config_at(dirs, keys)
}

/// 在指定目录上读取外壳启动配置（便于测试注入）。
pub fn shell_config_at(
    dirs: flick_data::DataDirectories,
    keys: flick_data::SharedKeySource,
) -> flick_shell_gpui::ShellConfig {
    let fallback = flick_shell_gpui::ShellConfig {
        theme: flick_shell_gpui::detect_initial_theme(),
        ..flick_shell_gpui::ShellConfig::default()
    };
    let storage = match StorageManager::open(dirs.clone(), keys) {
        Ok(storage) => Arc::new(storage),
        Err(error) => {
            tracing::warn!(%error, "读取外壳配置失败：设置存储不可用，回退默认值");
            return fallback;
        }
    };
    let service = SettingsService::new(storage);
    let version = env!("CARGO_PKG_VERSION");
    if let Err(error) = service.register_schema(core_schema(), version, version) {
        tracing::warn!(%error, "读取外壳配置失败：core schema 注册失败");
        return fallback;
    }
    flick_shell_gpui::ShellConfig {
        theme: theme_mode(&service).unwrap_or_else(flick_shell_gpui::detect_initial_theme),
        result_limit: result_limit(&service),
    }
}

/// 从设置系统读取结果条数上限；未设置或取值非法时返回声明默认值。
pub fn result_limit(service: &SettingsService) -> usize {
    let key =
        flick_domain::SettingKey::new(flick_domain::SettingNamespace::Core, "search.result_limit");
    match service.resolved(&key) {
        Ok(flick_domain::settings::SettingValue::Integer(value)) if value >= 1 => value as usize,
        _ => flick_application::presenter::DEFAULT_RESULT_LIMIT,
    }
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

    /// 设置变更应转发成 UI 事件（主题 + 结果上限），且只发值真的变了的那一个。
    #[tokio::test]
    async fn settings_changes_are_forwarded_to_ui() {
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
                bytes: [9u8; 32],
            },
        ));
        let service = Arc::new(SettingsService::new(Arc::new(
            StorageManager::open(dirs.clone(), keys).unwrap(),
        )));
        let version = env!("CARGO_PKG_VERSION");
        service
            .register_schema(core_schema(), version, version)
            .unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel::<HostToUi>();
        let cancel = CancellationToken::new();
        let worker = spawn_settings_forwarder(Arc::clone(&service), tx, cancel.clone());

        // 主题变化 → 只收到 ThemeChanged。
        let theme_key = flick_domain::SettingKey::new(
            flick_domain::SettingNamespace::Core,
            "theme.mode",
        );
        service
            .set(&theme_key, flick_domain::settings::SettingValue::Enum("light".into()))
            .unwrap();
        match rx.recv().await {
            Some(HostToUi::ThemeChanged { mode }) => {
                assert_eq!(mode, flick_domain::ThemeMode::Light)
            }
            other => panic!("期望 ThemeChanged，实际 {other:?}"),
        }

        // 上限变化 → 只收到 ResultLimitChanged。
        let limit_key = flick_domain::SettingKey::new(
            flick_domain::SettingNamespace::Core,
            "search.result_limit",
        );
        service
            .set(&limit_key, flick_domain::settings::SettingValue::Integer(5))
            .unwrap();
        match rx.recv().await {
            Some(HostToUi::ResultLimitChanged { limit }) => assert_eq!(limit, 5),
            other => panic!("期望 ResultLimitChanged，实际 {other:?}"),
        }

        cancel.cancel();
        let _ = worker.await;
    }

    /// 设置值应进入外壳配置：主题 + 结果条数上限。
    #[test]
    fn shell_config_reads_result_limit_from_settings() {
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
                bytes: [7u8; 32],
            },
        ));
        {
            let storage = Arc::new(StorageManager::open(dirs.clone(), keys.clone()).unwrap());
            let service = SettingsService::new(storage);
            let version = env!("CARGO_PKG_VERSION");
            service.register_schema(core_schema(), version, version).unwrap();
            let key = flick_domain::SettingKey::new(
                flick_domain::SettingNamespace::Core,
                "search.result_limit",
            );
            service
                .set(&key, flick_domain::settings::SettingValue::Integer(7))
                .unwrap();
        }
        let config = shell_config_at(dirs, keys);
        assert_eq!(config.result_limit, 7);
    }

    #[test]
    fn dir_size_accumulates_nested_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::write(root.join("a.bin"), vec![0u8; 10]).unwrap();
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join("nested").join("b.bin"), vec![0u8; 32]).unwrap();
        assert_eq!(dir_size(root), 42);
        assert_eq!(dir_size(&root.join("missing")), 0);
    }

    fn adapter_service(temp: &tempfile::TempDir) -> Arc<SettingsService> {
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
                bytes: [7u8; 32],
            },
        ));
        Arc::new(SettingsService::new(Arc::new(
            StorageManager::open(dirs, keys).unwrap(),
        )))
    }

    fn plugin_schema() -> flick_domain::settings::SettingSchema {
        use flick_domain::settings::{SettingDecl, SettingKind, SettingSchema, SettingValue};
        let decl = |key: &str, kind: SettingKind, secret: bool| SettingDecl {
            key: key.to_string(),
            renamed_from: None,
            label: key.to_string(),
            description: String::new(),
            kind,
            default: None,
            secret,
        };
        SettingSchema {
            namespace: SettingNamespace::Plugin("flick.demo".into()),
            declarations: vec![
                SettingDecl {
                    default: Some(SettingValue::Bool(true)),
                    ..decl("enabled", SettingKind::Bool, false)
                },
                decl("api_token", SettingKind::String, true),
            ],
            schema_version: 1,
        }
    }

    #[test]
    fn adapter_snapshot_merges_defaults_and_skips_secret() {
        use flick_domain::settings::SettingValue;
        let temp = tempfile::tempdir().unwrap();
        let service = adapter_service(&temp);
        service
            .register_schema(plugin_schema(), "1.0.0", "1.0.0")
            .unwrap();
        let adapter = SettingsAdapter::new(Arc::clone(&service));

        use flick_plugin::SettingsProvider;
        let snapshot = adapter.snapshot("flick.demo");
        assert_eq!(snapshot.namespace, "flick.demo");
        assert_eq!(snapshot.bool("enabled"), Some(true));
        assert_eq!(snapshot.value("api_token"), None, "secret 不得进入快照");

        // 写入 secret 后可通过 reader 实时取到，但快照仍然没有。
        let key = SettingKey::new(SettingNamespace::Plugin("flick.demo".into()), "api_token");
        service.set(&key, SettingValue::String("tok".into())).unwrap();
        use flick_domain::settings::SettingSecretReader;
        assert_eq!(adapter.secret("flick.demo", "api_token"), Some("tok".into()));
        assert_eq!(adapter.snapshot("flick.demo").value("api_token"), None);
    }

    #[test]
    fn adapter_snapshot_is_empty_for_unknown_plugin() {
        let temp = tempfile::tempdir().unwrap();
        let service = adapter_service(&temp);
        let adapter = SettingsAdapter::new(service);
        use flick_plugin::SettingsProvider;
        let snapshot = adapter.snapshot("flick.unknown");
        assert!(snapshot.is_empty());
        use flick_domain::settings::SettingSecretReader;
        assert_eq!(adapter.secret("flick.unknown", "x"), None);
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
