//! ActionHandler trait — 每个 Action 都必须实现的执行契约。

use async_trait::async_trait;
use flick_domain::{ActionContext, ActionError, ActionOutcome, ActionRequest};

/// 每个注册的 Action 所实现的 trait。
///
/// 实现者接收一个 [`ActionRequest`] 和一个 [`ActionContext`]
/// （其中携带一个 [`CancellationToken`]），并返回一个
/// [`ActionOutcome`]。
///
/// 运行时使用 `tokio::select!` 让处理器与请求截止时间
/// 和外部取消进行竞速，因此处理器应在长时间运行的工作中
/// 协作式地检查取消令牌。
#[async_trait]
pub trait ActionHandler: Send + Sync {
    /// 执行 `request` 所描述的动作。
    ///
    /// `ctx` 提供一个 [`CancellationToken`]，处理器应在长时间运行
    /// 的工作中轮询它，以便外部取消能够及时
    /// 得到响应。
    async fn invoke(
        &self,
        request: ActionRequest,
        ctx: ActionContext,
    ) -> Result<ActionOutcome, ActionError>;
}
