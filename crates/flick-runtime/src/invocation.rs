//! InvocationHandle — 跟踪正在运行的 Action 调用。

use flick_domain::InvocationId;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// 指向正在运行的调用的句柄。
///
/// 允许运行时（或调用方）取消正在执行中的动作。
/// 内部任务预期使用 [`tokio::spawn`] 生成，
/// 并产生一个
/// `Result<ActionOutcome, ActionError>`。
pub struct InvocationHandle {
    /// 此调用的唯一 id。
    pub id: InvocationId,
    /// 当调用应当停止时会收到信号的令牌。
    cancellation: CancellationToken,
    /// 生成的任务。
    handle: JoinHandle<Result<flick_domain::ActionOutcome, flick_domain::ActionError>>,
}

impl InvocationHandle {
    /// 创建一个新的句柄。
    pub fn new(
        id: InvocationId,
        cancellation: CancellationToken,
        handle: JoinHandle<Result<flick_domain::ActionOutcome, flick_domain::ActionError>>,
    ) -> Self {
        Self {
            id,
            cancellation,
            handle,
        }
    }

    /// 请求对此调用进行协作式取消。
    ///
    /// 如果令牌是首次被取消（即此前
    /// 尚未取消），则返回 `true`。处理器必须协作式地观察
    /// 该令牌，此操作才会生效。
    pub fn cancel(&self) -> bool {
        if self.cancellation.is_cancelled() {
            false
        } else {
            self.cancellation.cancel();
            true
        }
    }

    /// 消费该句柄并等待任务结果。
    ///
    /// 如果任务发生 panic，则返回 `Err(JoinError)`。
    pub async fn into_result(
        self,
    ) -> Result<
        Result<flick_domain::ActionOutcome, flick_domain::ActionError>,
        tokio::task::JoinError,
    > {
        self.handle.await
    }

    /// 立即中止底层任务。
    pub fn abort(&self) {
        self.handle.abort();
    }
}
