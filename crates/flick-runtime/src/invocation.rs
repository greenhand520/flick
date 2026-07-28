//! InvocationHandle — tracks a running Action invocation.

use flick_domain::InvocationId;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Handle to a running invocation.
///
/// Allows the runtime (or caller) to cancel an in-flight action.
/// The inner task is expected to be spawned with
/// [`tokio::spawn`] and produces an
/// `Result<ActionOutcome, ActionError>`.
pub struct InvocationHandle {
    /// Unique id for this invocation.
    pub id: InvocationId,
    /// Token that is signalled when the invocation should stop.
    cancellation: CancellationToken,
    /// The spawned task.
    handle: JoinHandle<Result<flick_domain::ActionOutcome, flick_domain::ActionError>>,
}

impl InvocationHandle {
    /// Create a new handle.
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

    /// Request cooperative cancellation of this invocation.
    ///
    /// Returns `true` if the token was newly cancelled (i.e. it was
    /// not already cancelled). The handler must cooperatively observe
    /// the token for this to take effect.
    pub fn cancel(&self) -> bool {
        if self.cancellation.is_cancelled() {
            false
        } else {
            self.cancellation.cancel();
            true
        }
    }

    /// Consume the handle and await the task result.
    ///
    /// Returns `Err(JoinError)` if the task panicked.
    pub async fn into_result(
        self,
    ) -> Result<
        Result<flick_domain::ActionOutcome, flick_domain::ActionError>,
        tokio::task::JoinError,
    > {
        self.handle.await
    }

    /// Abort the underlying task immediately.
    pub fn abort(&self) {
        self.handle.abort();
    }
}
