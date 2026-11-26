//! ActionRuntime — 编排路由、调用和生命周期。

use crate::handler::ActionHandler;
use crate::plugin::{Plugin, PluginRegistrationError, SurfaceError, SurfaceSession};
use crate::registry::ActionRegistry;
use flick_domain::{
    ActionContext, ActionDescriptor, ActionError, ActionId, ActionInput, ActionOutcome,
    ActionRequest, InputContext, InputType, InvocationId, RouteItem, SurfaceContent,
    SurfaceEventKind, SurfaceSessionId,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

/// 活动调用映射中的条目。
///
/// 仅存储 [`CancellationToken`]，以便外部调用方
/// 能够请求取消。 [`tokio::task::JoinHandle`] 则保留在
/// `invoke()` 局部，并在 `tokio::select!` 中等待。
struct ActiveEntry {
    cancel_token: CancellationToken,
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
    host_info: std::sync::RwLock<Option<Arc<flick_domain::HostInfo>>>,
    /// 活动 Surface 会话映射（会话 id → 状态）。
    surface_sessions: Arc<RwLock<HashMap<SurfaceSessionId, SurfaceSessionState>>>,
}

impl ActionRuntime {
    /// 创建一个具有空注册表的新运行时。
    pub fn new() -> Self {
        Self {
            registry: Arc::new(RwLock::new(ActionRegistry::new())),
            active: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            host_info: std::sync::RwLock::new(None),
            surface_sessions: Arc::new(RwLock::new(HashMap::new())),
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

    /// 搜索阶段执行：只调用 `search_invocable` Action，固定 150ms 超时。
    ///
    /// 这些 Action 必须只读、无副作用；每个 Action 的结果经
    /// [`execution_route_items`] 归一化为 [`RouteItem`] 执行项。
    /// 错误与超时只记录日志并丢弃，不影响其它 Action。
    pub async fn search_invoke(&self, query: &str, input_kind: InputType) -> Vec<RouteItem> {
        if query.trim().is_empty() {
            return Vec::new();
        }
        let action_ids = {
            let reg = self.registry.read().await;
            reg.search_invocable_actions(input_kind)
        };
        let mut out = Vec::new();
        for action_id in action_ids {
            let request = ActionRequest {
                invocation_id: InvocationId::new(),
                action_id: action_id.clone(),
                input: ActionInput::plain_text(query),
                caller: flick_domain::Caller::ui("search-route"),
                source: flick_domain::ActionSource::SearchRoute,
                deadline: flick_domain::Deadline {
                    timeout: std::time::Duration::from_millis(150),
                },
            };
            let outcome = self.invoke(request).await;
            out.extend(execution_route_items(action_id, outcome));
        }
        out
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
    /// 1. 在注册表中查找处理器（读锁，立即
    ///    释放）。
    /// 2. 使用新的 [`CancellationToken`] 生成处理器任务。
    /// 3. 将处理器的完成与请求截止时间和
    ///    外部取消进行竞速。
    /// 4. 完成后清理活动条目。
    pub async fn invoke(&self, mut request: ActionRequest) -> ActionOutcome {
        let invocation_id = request.invocation_id;
        let deadline = request.deadline.timeout;

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
        let token = CancellationToken::new();
        let ctx = ActionContext {
            cancellation: token.clone(),
            host_info: self.host_info.read().unwrap().clone(),
        };

        // 存入活动映射，以便 `cancel()` 能找到它。
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
            // 分支 C — 外部取消（有人调用了 `cancel()`）
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
        {
            let mut active = self.active.lock().await;
            active.remove(&invocation_id);
        }

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

impl Default for ActionRuntime {
    fn default() -> Self {
        Self::new()
    }
}

/// 将一次 `search_invocable` Action 的结果归一化为 [`RouteItem`] 执行项。
///
/// 数组结果逐项展开，单项结果作为单条执行项；`Error`/`OpenSurface`
/// 不产生执行项（仅记录日志）。标题按 `title`/`name`/`label` 回退。
fn execution_route_items(parent_action_id: ActionId, outcome: ActionOutcome) -> Vec<RouteItem> {
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
            invocation_id: InvocationId::new(),
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
    items
}
