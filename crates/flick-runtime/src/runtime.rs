//! ActionRuntime — 编排路由、调用和生命周期。

use crate::handler::ActionHandler;
use crate::plugin::{Plugin, PluginRegistrationError};
use crate::registry::ActionRegistry;
use flick_domain::{
    ActionContext, ActionDescriptor, ActionError, ActionOutcome, ActionRequest, InputKind,
    InvocationId, RouteItem,
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
}

impl ActionRuntime {
    /// 创建一个具有空注册表的新运行时。
    pub fn new() -> Self {
        Self {
            registry: Arc::new(RwLock::new(ActionRegistry::new())),
            active: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
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
    pub async fn register_plugin(&self, plugin: &dyn Plugin) -> Result<(), PluginRegistrationError> {
        let mut registry = self.registry.write().await;
        registry.register_plugin(plugin)
    }

    /// 为搜索查询返回匹配的 [`RouteItem`]s。
    ///
    /// 返回前会释放注册表锁。
    pub async fn route(&self, query: &str, input_kind: InputKind) -> Vec<RouteItem> {
        let reg = self.registry.read().await;
        reg.search(query, input_kind)
    }

    /// 调用一个动作。
    ///
    /// 1. 在注册表中查找处理器（读锁，立即
    ///    释放）。
    /// 2. 使用新的 [`CancellationToken`] 生成处理器任务。
    /// 3. 将处理器的完成与请求截止时间和
    ///    外部取消进行竞速。
    /// 4. 完成后清理活动条目。
    pub async fn invoke(&self, request: ActionRequest) -> ActionOutcome {
        let invocation_id = request.invocation_id;
        let deadline = request.deadline.timeout;

        // ── 1. 查找处理器（仅短暂持有读锁） ────────
        let handler: Arc<dyn ActionHandler> = {
            let reg = self.registry.read().await;
            match reg.find_by_id(&request.action_id) {
                Some((_desc, h)) => Arc::clone(h),
                None => {
                    return ActionOutcome::Error(ActionError::UnknownAction {
                        action_id: request.action_id.to_string(),
                    });
                }
            }
        };
        // 注册表的读锁在此处释放

        // ── 2. 准备取消令牌并生成任务 ───────────────────
        let token = CancellationToken::new();
        let ctx = ActionContext {
            cancellation: token.clone(),
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

        // ── 4. 清理 ────────────────────────────────────────────
        {
            let mut active = self.active.lock().await;
            active.remove(&invocation_id);
        }

        outcome
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
