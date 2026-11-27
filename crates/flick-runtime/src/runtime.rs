//! ActionRuntime — 编排路由、调用和生命周期。

use crate::registry::ActionRegistry;
use flick_domain::{
    ActionContext, ActionDescriptor, ActionError, ActionId, ActionInput, ActionOutcome,
    ActionRequest, DependencyStatus, InputContext, InputType, InvocationId, QueryGeneration,
    RouteItem, SurfaceContent, SurfaceEventKind, SurfaceSessionId,
};
use flick_plugin::{
    ActionHandler, Plugin, PluginRegistrationError, SurfaceError, SurfaceSession,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

/// 搜索阶段单个 Action 最多展开多少条执行项（设计文档 §7.4 / §9 约束 8）。
pub const MAX_EXECUTION_ITEMS_PER_ACTION: usize = 20;

/// 搜索阶段调用单个 Action 的截止时间（毫秒）。
///
/// 由宿主固定，插件不能通过 manifest 或 descriptor 延长（设计文档 §9 约束 9）。
pub const SEARCH_INVOKE_TIMEOUT_MS: u64 = 150;

/// 把 `file://` URI 转为本地路径；非该前缀或含非法字符时返回 `None`。
///
/// 只做最小编码处理：去掉 `file://`、`file://localhost` 前缀，并做一次
/// 常见百分号解码（`%20` 等），不引入 URL 解析依赖。
fn uri_to_path(uri: &str) -> Option<std::path::PathBuf> {
    let rest = uri
        .strip_prefix("file://localhost")
        .or_else(|| uri.strip_prefix("file://"))?;
    let decoded = percent_decode(rest);
    (!decoded.is_empty()).then(|| std::path::PathBuf::from(decoded))
}

/// 极简百分号解码，仅覆盖路径中常见的转义；非法序列按原样保留。
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(value) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 调用入口的显式输入归一化：`PathCandidate` 若指向真实存在的文件/文件夹，
/// 升级为 `InputType::File` 并回填 `FileInput.is_dir`（设计文档「搜索匹配算法」§2.3）。
///
/// 只在真正的调用入口执行一次 `stat`；搜索阶段（`ActionSource::SearchRoute`）
/// 跳过，保持「匹配阶段无 I/O」。
fn normalize_path_input(input: &mut ActionInput) {
    if input.kind != InputType::PathCandidate {
        return;
    }
    let raw = input
        .files
        .first()
        .map(|file| file.uri.clone())
        .unwrap_or_else(|| input.text.clone());
    let Some(path) = uri_to_path(&raw) else {
        return;
    };
    let Ok(metadata) = std::fs::metadata(&path) else {
        return;
    };
    input.kind = InputType::File;
    if let Some(file) = input.files.first_mut() {
        file.is_dir = metadata.is_dir();
    }
}

/// 活动调用映射中的条目。
///
/// 仅存储 [`CancellationToken`]，以便外部调用方
/// 能够请求取消。 [`tokio::task::JoinHandle`] 则保留在
/// `invoke()` 局部，并在 `tokio::select!` 中等待。
struct ActiveEntry {
    cancel_token: CancellationToken,
}

/// `active` 表的自动清理守卫。
///
/// `invoke()` 可能在 `tokio::select!` 中途被丢弃（例如搜索阶段的 `JoinSet`
/// 被 abort）。此时函数末尾的清理代码不会执行，`InvocationId` 会残留在
/// `active` 表中泄漏取消令牌。把清理放进 `Drop`，保证无论正常返回还是 future
/// 被丢弃都能移除条目。
struct ActiveGuard {
    /// 与 `InvocationCore::active` 共享的活动调用表。
    active: Arc<tokio::sync::Mutex<HashMap<InvocationId, ActiveEntry>>>,
    /// 需要移除的调用标识。
    invocation_id: InvocationId,
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        let active = Arc::clone(&self.active);
        let invocation_id = self.invocation_id;
        // 绝大多数情况可以立刻拿到锁。
        if let Ok(mut map) = active.try_lock() {
            map.remove(&invocation_id);
            return;
        }
        // 锁被占用：交给后台任务异步移除。没有运行时上下文时只能放弃
        // （此时进程正在退出或测试已结束，残留无实际影响）。
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                active.lock().await.remove(&invocation_id);
            });
        }
    }
}

/// 一个活动 Surface 会话的运行时状态。
struct SurfaceSessionState {
    session: SurfaceSession,
}

/// Flick 运行时。
///
/// 将 [`ActionRegistry`] 保存在 `Arc<RwLock<_>>` 之后，以便
/// 在任务之间共享，并允许动态重新注册。
///
/// 活动调用记录在 `tokio::sync::Mutex<HashMap>` 中，
/// 因此可以从任意任务中取消它们。
pub struct ActionRuntime {
    registry: Arc<RwLock<ActionRegistry>>,
    active: Arc<tokio::sync::Mutex<HashMap<InvocationId, ActiveEntry>>>,
    /// 宿主信息；用 `Arc` 包裹以便随 [`InvocationCore`] 克隆进并行任务。
    host_info: Arc<std::sync::RwLock<Option<Arc<flick_domain::HostInfo>>>>,
    /// 活动 Surface 会话映射（会话 id → 状态）。
    surface_sessions: Arc<RwLock<HashMap<SurfaceSessionId, SurfaceSessionState>>>,
    /// 搜索代次取消槽：记录当前搜索代次与其取消令牌。
    search_cancel: std::sync::Mutex<Option<(QueryGeneration, CancellationToken)>>,
}

/// 一次动作调用所需的最小运行时上下文。
///
/// 抽出来是为了让调用逻辑能被 `tokio::task::JoinSet::spawn`：
/// `JoinSet` 要求任务 `'static`，而 `&ActionRuntime` 不是。
#[derive(Clone)]
struct InvocationCore {
    /// 动作注册表。
    registry: Arc<RwLock<ActionRegistry>>,
    /// 活动调用表（用于外部取消）。
    active: Arc<tokio::sync::Mutex<HashMap<InvocationId, ActiveEntry>>>,
    /// 宿主信息。
    host_info: Arc<std::sync::RwLock<Option<Arc<flick_domain::HostInfo>>>>,
    /// 活动 Surface 会话表（`OpenSurface` 回填需要）。
    surface_sessions: Arc<RwLock<HashMap<SurfaceSessionId, SurfaceSessionState>>>,
}

impl ActionRuntime {
    /// 创建一个具有空注册表的新运行时。
    pub fn new() -> Self {
        Self {
            registry: Arc::new(RwLock::new(ActionRegistry::new())),
            active: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            host_info: Arc::new(std::sync::RwLock::new(None)),
            surface_sessions: Arc::new(RwLock::new(HashMap::new())),
            search_cancel: std::sync::Mutex::new(None),
        }
    }

    /// 克隆出一次调用所需的最小上下文。
    fn core(&self) -> InvocationCore {
        InvocationCore {
            registry: Arc::clone(&self.registry),
            active: Arc::clone(&self.active),
            host_info: Arc::clone(&self.host_info),
            surface_sessions: Arc::clone(&self.surface_sessions),
        }
    }

    /// 注入启动时采集的宿主信息。
    pub fn set_host_info(&self, info: Arc<flick_domain::HostInfo>) {
        *self.host_info.write().unwrap() = Some(info);
    }

    /// 注册一个新动作（委托给内部注册表）。
    ///
    /// 获取注册表上的写锁，但**不会**在
    /// 任何 `.await` 期间持有它。
    pub async fn register(&self, descriptor: ActionDescriptor, handler: Arc<dyn ActionHandler>) {
        let mut reg = self.registry.write().await;
        reg.register(descriptor, handler);
    }

    /// 异步注册一个插件（委托给内部注册表）。
    ///
    /// 获取注册表上的写锁，在注册期间持有，注册完成后立即释放。
    /// 返回注册过程中出现的任何 [`PluginRegistrationError`]。
    pub async fn register_plugin(
        &self,
        plugin: &dyn Plugin,
    ) -> Result<(), PluginRegistrationError> {
        let mut registry = self.registry.write().await;
        registry.register_plugin(plugin)
    }

    /// 启动时统一激活所有有生命周期的插件；单个失败只记录，不阻止其它插件。
    pub async fn activate_all_plugins(&self) {
        let lifecycles = {
            let reg = self.registry.read().await;
            reg.plugin_lifecycles()
        };
        for lifecycle in lifecycles {
            if let Err(error) = lifecycle.activate().await {
                tracing::warn!(?error, "plugin activate failed");
            }
        }
    }

    /// 解析所有插件的依赖关系并应用到注册表。
    ///
    /// 必需依赖缺失的插件会被标记为不可用，其动作从注册表移除
    /// （不可搜索、不可调用）；manifest 保留以便诊断。返回全部
    /// `DependencyMissing` 错误，不短路。
    pub async fn resolve_dependencies(&self) -> Vec<PluginRegistrationError> {
        let mut registry = self.registry.write().await;
        registry.resolve_dependencies()
    }

    /// 查询某插件的依赖解析结果；未注册或没有依赖时返回空。
    pub async fn dependency_status(&self, plugin_id: &str) -> Vec<DependencyStatus> {
        let registry = self.registry.read().await;
        registry.dependency_status(plugin_id)
    }

    /// 返回所有已注册插件 id（升序）。
    pub async fn registered_plugin_ids(&self) -> Vec<String> {
        let registry = self.registry.read().await;
        registry.plugin_ids()
    }

    /// 为搜索查询返回匹配的 [`RouteItem`]s。
    ///
    /// 返回前会释放注册表锁。
    pub async fn route(&self, query: &str, input_kind: InputType) -> Vec<RouteItem> {
        let reg = self.registry.read().await;
        reg.search(query, input_kind)
    }

    /// 按完整输入上下文路由；使用 facet 过滤、分层匹配与确定性排序。
    ///
    /// 旧的 [`Self::route`] 保留给尚未迁移的调用方。
    pub async fn route_input(&self, context: &InputContext) -> Vec<RouteItem> {
        let reg = self.registry.read().await;
        reg.search_input(context)
    }

    /// 搜索阶段执行：对所有 `search_invocable` Action **并行**调用。
    ///
    /// 这些 Action 必须只读、无副作用；每个 Action 的结果经
    /// [`execution_route_items`] 归一化为 [`RouteItem`] 执行项。
    /// 错误、超时与取消只记录日志并丢弃，不影响其它 Action。
    ///
    /// 新代次进来时先取消旧代次的在途调用（`generation` 由协调器透传）。
    /// 跨父 Action 的顺序不在这里保证：`JoinSet` 先完成的先返回，
    /// 排序由 Presenter 按父项位置决定。
    pub async fn search_invoke(
        &self,
        generation: QueryGeneration,
        input: ActionInput,
    ) -> Vec<RouteItem> {
        // 空文本且无附件时无事可做。
        if input.text.trim().is_empty() && input.files.is_empty() && input.image.is_none() {
            return Vec::new();
        }

        // 新代次进来先取消旧代次。
        let parent = self.begin_search_generation(generation);

        let action_ids = {
            let reg = self.registry.read().await;
            let mut action_ids = reg.search_invocable_actions(input.kind);
            // 与 action 候选上限保持一致，避免插件过多时一次性并发打爆宿主（设计文档 §9 风险表）。
            action_ids.truncate(crate::registry::MAX_ACTION_RESULTS);
            action_ids
        };
        if action_ids.is_empty() {
            return Vec::new();
        }

        let core = self.core();
        let mut set = tokio::task::JoinSet::new();
        for action_id in action_ids {
            let core = core.clone();
            let input = input.clone();
            let parent = parent.clone();
            set.spawn(async move {
                let invocation_id = InvocationId::new();
                let request = ActionRequest {
                    invocation_id,
                    action_id: action_id.clone(),
                    input,
                    caller: flick_domain::Caller::ui("search-route"),
                    source: flick_domain::ActionSource::SearchRoute,
                    deadline: flick_domain::Deadline {
                        timeout: std::time::Duration::from_millis(SEARCH_INVOKE_TIMEOUT_MS),
                    },
                };
                let outcome = core.invoke(request, Some(parent)).await;
                (action_id, invocation_id, outcome)
            });
        }

        let mut out = Vec::new();
        while let Some(joined) = set.join_next().await {
            // 任务 panic 已由 `InvocationCore::invoke` 转成 Internal 错误，
            // 这里只防 join 失败（例如任务被 abort）。
            let Ok((action_id, invocation_id, outcome)) = joined else {
                continue;
            };
            out.extend(execution_route_items(action_id, invocation_id, outcome));
        }
        out
    }

    /// 开始一个新的搜索代次，返回该代次的父取消令牌。
    ///
    /// 若槽里存的是**别的**代次，先 `cancel()` 旧令牌再替换；
    /// 同代次重复调用则复用原令牌。
    fn begin_search_generation(&self, generation: QueryGeneration) -> CancellationToken {
        let mut slot = self.search_cancel.lock().unwrap();
        if let Some((current, token)) = slot.as_ref() {
            if *current == generation {
                return token.clone();
            }
            token.cancel();
        }
        let token = CancellationToken::new();
        *slot = Some((generation, token.clone()));
        token
    }

    /// 用户对 Execution 项操作时，交给父 Action 的 handler 处理。
    ///
    /// 固定 30s 超时；未实现或出错时返回 `None`（仅记录日志）。
    pub async fn execution_event(
        &self,
        parent_action_id: ActionId,
        item_id: &str,
        event: flick_domain::ExecutionEventKind,
    ) -> Option<ActionOutcome> {
        let handler = {
            let reg = self.registry.read().await;
            reg.find_by_id(&parent_action_id)
                .map(|(_, h)| Arc::clone(h))
        }?;
        let ctx = ActionContext {
            cancellation: CancellationToken::new(),
            host_info: self.host_info.read().unwrap().clone(),
        };
        match tokio::time::timeout(
            std::time::Duration::from_secs(30),
            handler.execution_event(&parent_action_id, item_id, event, ctx),
        )
        .await
        {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(error)) => {
                tracing::warn!(%parent_action_id, %item_id, ?error, "execution_event failed");
                None
            }
            Err(_) => {
                tracing::warn!(%parent_action_id, %item_id, "execution_event timed out");
                None
            }
        }
    }

    /// 调用一个动作。
    ///
    /// 委托给 [`InvocationCore::invoke`]，不附带父取消令牌。
    pub async fn invoke(&self, request: ActionRequest) -> ActionOutcome {
        self.core().invoke(request, None).await
    }

    /// 向会话的插件处理器转发一次 Surface 交互事件，返回更新后的内容。
    pub async fn surface_event(
        &self,
        session_id: SurfaceSessionId,
        event: SurfaceEventKind,
    ) -> Result<SurfaceContent, SurfaceError> {
        let session = {
            let sessions = self.surface_sessions.read().await;
            sessions
                .get(&session_id)
                .map(|s| s.session.clone())
                .ok_or(SurfaceError {
                    message: "surface session not found".into(),
                })?
        };
        let handler = {
            let reg = self.registry.read().await;
            reg.surface_handler_for(&session.plugin_id)
        }
        .ok_or(SurfaceError {
            message: "surface handler not found".into(),
        })?;
        handler.on_event(&session, event).await
    }

    /// 关闭一个 Surface 会话（幂等）：触发 `on_close` 并移除会话。
    pub async fn surface_closed(&self, session_id: SurfaceSessionId) {
        let removed = {
            let mut sessions = self.surface_sessions.write().await;
            sessions.remove(&session_id)
        };
        if let Some(state) = removed {
            let handler = {
                let reg = self.registry.read().await;
                reg.surface_handler_for(&state.session.plugin_id)
            };
            if let Some(h) = handler {
                h.on_close(&state.session).await;
            }
        }
    }

    /// 取消正在执行中的调用。
    ///
    /// 如果找到了该调用且其取消令牌是首次收到信号
    /// （即此前尚未取消），则返回 `true`。
    pub async fn cancel(&self, invocation_id: InvocationId) -> bool {
        let active = self.active.lock().await;
        match active.get(&invocation_id) {
            Some(entry) => {
                if entry.cancel_token.is_cancelled() {
                    false
                } else {
                    entry.cancel_token.cancel();
                    true
                }
            }
            None => false,
        }
    }
}

impl InvocationCore {
    /// 调用一个动作。
    ///
    /// 1. 在注册表中查找处理器（读锁，立即释放）。
    /// 2. 使用取消令牌（`parent_token` 存在时取其子令牌）生成处理器任务。
    /// 3. 将处理器的完成与请求截止时间、外部取消进行竞速。
    /// 4. 完成后清理活动条目。
    async fn invoke(
        &self,
        mut request: ActionRequest,
        parent_token: Option<CancellationToken>,
    ) -> ActionOutcome {
        let invocation_id = request.invocation_id;
        let deadline = request.deadline.timeout;

        // 调用入口做一次显式输入归一化（搜索阶段不做 I/O）。
        if request.source != flick_domain::ActionSource::SearchRoute {
            normalize_path_input(&mut request.input);
        }

        // ── 1. 查找处理器（仅短暂持有读锁） ────────
        let (handler, accepts_no_input): (Arc<dyn ActionHandler>, bool) = {
            let reg = self.registry.read().await;
            match reg.find_by_id(&request.action_id) {
                Some((desc, h)) => {
                    // 语义：`supported_inputs == [None]` 表示该动作不接受输入。
                    // 搜索框里的文本只用于路由，不应作为插件输入传入；这里统一归一化为空输入。
                    let no_input = !desc.supported_inputs.is_empty()
                        && desc
                            .supported_inputs
                            .iter()
                            .all(|kind| *kind == InputType::None);
                    (Arc::clone(h), no_input)
                }
                None => {
                    return ActionOutcome::Error(ActionError::UnknownAction {
                        action_id: request.action_id.to_string(),
                    });
                }
            }
        };
        // 注册表的读锁在此处释放
        if accepts_no_input && request.input.kind != InputType::None {
            request.input = ActionInput::none();
        }

        // ── 2. 准备取消令牌并生成任务 ───────────────────
        // 有父令牌时用子令牌，父令牌取消会级联到本次调用。
        let token = match parent_token {
            Some(parent) => parent.child_token(),
            None => CancellationToken::new(),
        };
        let ctx = ActionContext {
            cancellation: token.clone(),
            host_info: self.host_info.read().unwrap().clone(),
        };

        // 存入活动映射，以便 `cancel()` 能找到它。
        // 用守卫托管清理：即使本 future 中途被丢弃，也会移除条目。
        let _active_guard = ActiveGuard {
            active: Arc::clone(&self.active),
            invocation_id,
        };
        {
            let mut active = self.active.lock().await;
            active.insert(
                invocation_id,
                ActiveEntry {
                    cancel_token: token.clone(),
                },
            );
        }

        // 生成处理器任务。
        let req_for_task = request.clone();
        let join_handle = tokio::spawn(async move { handler.invoke(req_for_task, ctx).await });

        // ── 3. 竞速：处理器完成 vs 截止时间 vs 外部取消
        let outcome = tokio::select! {
            // 分支 A — 处理器自然完成
            result = join_handle => {
                match result {
                    Ok(inner) => inner.unwrap_or_else(ActionOutcome::Error),
                    Err(join_err) => {
                        tracing::error!(invocation_id = %invocation_id, "handler panicked: {join_err}");
                        ActionOutcome::Error(ActionError::Internal {
                            message: "handler task panicked".into(),
                        })
                    }
                }
            }
            // 分支 B — 超过截止时间
            _ = tokio::time::sleep(deadline) => {
                token.cancel();
                ActionOutcome::Error(ActionError::DeadlineExceeded)
            }
            // 分支 C — 外部取消（有人调用了 `cancel()` 或父令牌被取消）
            _ = token.cancelled() => {
                ActionOutcome::Error(ActionError::Cancelled)
            }
        };

        // ── 3.5 若返回 OpenSurface，则生成会话并回填 ──────────────
        // 搜索阶段（SearchRoute）必须只读、无副作用：跳过会话创建与 on_open，
        // 原样返回 outcome，由 `execution_route_items` 记录并丢弃 OpenSurface。
        let outcome = if request.source == flick_domain::ActionSource::SearchRoute {
            outcome
        } else {
            self.backfill_open_surface(outcome, &request).await
        };

        // ── 4. 清理 ────────────────────────────────────────────
        // 由 `_active_guard` 的 `Drop` 负责移除（正常返回与 future 丢弃两条路径统一）。

        outcome
    }

    /// 若结果为 `OpenSurface` 且尚无会话，则生成会话 id、按 manifest
    /// 回填 `needs_search`，注册会话并触发 `on_open`。
    async fn backfill_open_surface(
        &self,
        outcome: ActionOutcome,
        request: &ActionRequest,
    ) -> ActionOutcome {
        let (surface_id, content) = match outcome {
            ActionOutcome::OpenSurface {
                surface_id,
                content,
                session_id: None,
                ..
            } => (surface_id, content),
            other => return other,
        };
        let session_id = SurfaceSessionId::new();
        let (needs_search, plugin_id, handler) = {
            let reg = self.registry.read().await;
            let manifest = reg.plugin_for_action(&request.action_id);
            let needs_search = manifest.map(|m| m.needs_search).unwrap_or(false);
            let plugin_id = manifest.map(|m| m.id.clone()).unwrap_or_default();
            let handler = reg.surface_handler_for(&plugin_id);
            (needs_search, plugin_id, handler)
        };
        let session = SurfaceSession {
            id: session_id,
            plugin_id,
            surface_id: surface_id.clone(),
        };
        if let Some(h) = &handler {
            h.on_open(&session).await;
        }
        {
            let mut sessions = self.surface_sessions.write().await;
            sessions.insert(session_id, SurfaceSessionState { session });
        }
        ActionOutcome::OpenSurface {
            surface_id,
            content,
            session_id: Some(session_id),
            needs_search,
        }
    }
}

impl Default for ActionRuntime {
    fn default() -> Self {
        Self::new()
    }
}

/// 将一次 `search_invocable` Action 的结果归一化为 [`RouteItem`] 执行项。
///
/// 数组结果逐项展开，单项结果作为单条执行项；`Error`/`OpenSurface`
/// 不产生执行项（仅记录日志）。标题按 `title`/`name`/`label` 回退。
/// 结果按 [`MAX_EXECUTION_ITEMS_PER_ACTION`] 截断。
///
/// 分数统一为 `0.0`：Runtime 没有父 Action 的分数，由 Presenter 合并时继承。
fn execution_route_items(
    parent_action_id: ActionId,
    invocation_id: InvocationId,
    outcome: ActionOutcome,
) -> Vec<RouteItem> {
    let mut items = Vec::new();
    let mut push = |raw: serde_json::Value, fallback_title: String, index: usize| {
        let object = raw.as_object();
        let item_id = object
            .and_then(|o| o.get("id"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("{parent_action_id}:index:{index}"));
        let title = object
            .and_then(|o| {
                o.get("title")
                    .or_else(|| o.get("name"))
                    .or_else(|| o.get("label"))
            })
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or(fallback_title);
        let subtitle = object
            .and_then(|o| o.get("subtitle").or_else(|| o.get("description")))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let icon = object
            .and_then(|o| o.get("icon"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let badge = object
            .and_then(|o| o.get("badge"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let item = flick_domain::ExecutionItem {
            item_id,
            parent_action_id: parent_action_id.clone(),
            invocation_id,
            title,
            subtitle,
            icon,
            badge,
            output: raw,
        };
        items.push(RouteItem::execution(item, 0.0));
    };
    match outcome {
        ActionOutcome::Data(serde_json::Value::Array(values)) => {
            for (i, value) in values.into_iter().enumerate() {
                push(value, format!("结果 {}", i + 1), i);
            }
        }
        ActionOutcome::Data(value) => push(value.clone(), value.to_string(), 0),
        ActionOutcome::Text(text) => push(serde_json::Value::String(text.clone()), text, 0),
        ActionOutcome::Notice { message } => {
            push(serde_json::Value::String(message.clone()), message, 0)
        }
        ActionOutcome::Url(url) => push(serde_json::Value::String(url.clone()), url, 0),
        ActionOutcome::File(file) => push(
            serde_json::to_value(file).unwrap_or_default(),
            "文件".into(),
            0,
        ),
        ActionOutcome::Error(error) => {
            tracing::warn!(?error, "search_invoke error dropped");
        }
        ActionOutcome::OpenSurface { .. } => {
            tracing::warn!("search_invoke returned OpenSurface; dropped");
        }
    }
    // 单个 Action 最多展开 20 条执行项，避免插件返回大列表时撑爆候选面板。
    items.truncate(MAX_EXECUTION_ITEMS_PER_ACTION);
    items
}

#[cfg(test)]
mod active_guard_tests {
    //! 校验 [`ActiveGuard`] 在 future 被丢弃时仍会清理 `active` 表。

    use super::*;
    use flick_domain::*;
    use flick_plugin::ActionSpec;
    use std::time::Duration;

    /// 永不自主完成的处理器，用于模拟「调用尚未结束」。
    struct HangingHandler;

    #[async_trait::async_trait]
    impl ActionHandler for HangingHandler {
        async fn invoke(
            &self,
            _request: ActionRequest,
            _ctx: ActionContext,
        ) -> Result<ActionOutcome, ActionError> {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok(ActionOutcome::Text("late".into()))
        }
    }

    /// 持有一个挂起 Action 的插件。
    struct HangingPlugin;

    impl Plugin for HangingPlugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                id: "flick.test.hang".into(),
                name: "Hang".into(),
                author: "Flick".into(),
                version: "1.0.0".into(),
                kind: PluginKind::BuiltInRust,
                keywords: vec![],
                icon: None,
                profile: PluginProfile::default(),
                permissions: vec![],
                plugin_dependencies: vec![],
                settings: vec![],
                surfaces: vec![],
                supported_os: vec![],
                supported_architectures: vec![],
                needs_search: true,
            }
        }

        fn actions(&self) -> Vec<ActionSpec> {
            let mut descriptor = ActionDescriptor::new(
                ActionId::from_static("flick.test.hang.v1.hang"),
                "挂起",
                vec![],
                ConcurrencyPolicy::Parallel,
            );
            descriptor.supported_inputs = vec![InputType::None];
            vec![ActionSpec {
                descriptor,
                handler: Arc::new(HangingHandler),
            }]
        }
    }

    /// future 被丢弃时，`active` 表中的条目必须被移除（不泄漏取消令牌）。
    #[tokio::test]
    async fn dropped_invoke_removes_active_entry() {
        let runtime = ActionRuntime::new();
        runtime.register_plugin(&HangingPlugin).await.unwrap();

        let request = ActionRequest {
            invocation_id: InvocationId::new(),
            action_id: ActionId::from_static("flick.test.hang.v1.hang"),
            input: ActionInput::none(),
            caller: Caller::ui("test"),
            source: ActionSource::Ui,
            deadline: Deadline::from_secs(30),
        };

        // 50ms 后放弃等待：模拟上层 select!/JoinSet abort 丢弃 future。
        let result = tokio::time::timeout(Duration::from_millis(50), runtime.invoke(request)).await;
        assert!(result.is_err(), "invoke should still be running when dropped");

        assert!(
            runtime.active.lock().await.is_empty(),
            "dropped invoke must not leak its active entry"
        );
    }
}

#[cfg(test)]
mod path_input_tests {
    //! 校验调用入口的 `PathCandidate -> File` 归一化。

    use super::*;
    use flick_domain::*;
    use flick_plugin::ActionSpec;
    use std::sync::Mutex;

    /// 记录最后一次调用收到的输入类型。
    #[derive(Clone, Default)]
    struct Seen(Arc<Mutex<Option<InputType>>>);

    struct CapturingHandler {
        seen: Seen,
    }

    #[async_trait::async_trait]
    impl ActionHandler for CapturingHandler {
        async fn invoke(
            &self,
            request: ActionRequest,
            _ctx: ActionContext,
        ) -> Result<ActionOutcome, ActionError> {
            *self.seen.0.lock().unwrap() = Some(request.input.kind);
            Ok(ActionOutcome::Text("ok".into()))
        }
    }

    struct CapturePlugin {
        seen: Seen,
    }

    impl Plugin for CapturePlugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                id: "flick.test.path".into(),
                name: "Path".into(),
                author: "Flick".into(),
                version: "1.0.0".into(),
                kind: PluginKind::BuiltInRust,
                keywords: vec![],
                icon: None,
                profile: PluginProfile::default(),
                permissions: vec![],
                plugin_dependencies: vec![],
                settings: vec![],
                surfaces: vec![],
                supported_os: vec![],
                supported_architectures: vec![],
                needs_search: false,
            }
        }

        fn actions(&self) -> Vec<ActionSpec> {
            let mut descriptor = ActionDescriptor::new(
                ActionId::from_static("flick.test.path.v1.open"),
                "打开路径",
                vec![],
                ConcurrencyPolicy::Parallel,
            );
            descriptor.supported_inputs = vec![
                InputType::PathCandidate,
                InputType::File,
                InputType::Files,
            ];
            vec![ActionSpec {
                descriptor,
                handler: Arc::new(CapturingHandler {
                    seen: self.seen.clone(),
                }),
            }]
        }
    }

    fn path_request(uri: Option<String>) -> ActionRequest {
        ActionRequest {
            invocation_id: InvocationId::new(),
            action_id: ActionId::from_static("flick.test.path.v1.open"),
            input: ActionInput {
                kind: InputType::PathCandidate,
                text: uri.clone().unwrap_or_default(),
                image: None,
                files: uri
                    .map(|uri| {
                        vec![FileInput {
                            uri,
                            display_name: "x".into(),
                            extension: None,
                            is_dir: false,
                            mime: None,
                        }]
                    })
                    .unwrap_or_default(),
            },
            caller: Caller::ui("test"),
            source: ActionSource::Ui,
            deadline: Deadline::from_secs(5),
        }
    }

    #[test]
    fn uri_to_path_strips_prefix_and_decodes() {
        assert_eq!(
            uri_to_path("file:///tmp/a%20b.txt"),
            Some(std::path::PathBuf::from("/tmp/a b.txt"))
        );
        assert_eq!(
            uri_to_path("file://localhost/tmp/x"),
            Some(std::path::PathBuf::from("/tmp/x"))
        );
        assert_eq!(uri_to_path("/tmp/plain"), None);
        assert_eq!(uri_to_path("file://"), None);
    }

    #[tokio::test]
    async fn existing_path_is_upgraded_to_file() {
        let seen = Seen::default();
        let runtime = ActionRuntime::new();
        runtime
            .register_plugin(&CapturePlugin { seen: seen.clone() })
            .await
            .unwrap();

        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("flick_norm_{unique}.txt"));
        std::fs::write(&path, b"x").unwrap();

        let uri = format!("file://{}", path.display());
        let _ = runtime.invoke(path_request(Some(uri))).await;

        assert_eq!(
            *seen.0.lock().unwrap(),
            Some(InputType::File),
            "存在的路径应升级为 InputType::File"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn missing_path_stays_path_candidate() {
        let seen = Seen::default();
        let runtime = ActionRuntime::new();
        runtime
            .register_plugin(&CapturePlugin { seen: seen.clone() })
            .await
            .unwrap();

        let _ = runtime
            .invoke(path_request(Some(
                "file:///tmp/flick-definitely-missing-file".into(),
            )))
            .await;

        assert_eq!(
            *seen.0.lock().unwrap(),
            Some(InputType::PathCandidate),
            "不存在的路径保持 PathCandidate"
        );
    }
}
