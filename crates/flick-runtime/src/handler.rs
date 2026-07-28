//! ActionHandler trait — the execution contract every Action must implement.

use async_trait::async_trait;
use flick_domain::{ActionContext, ActionError, ActionOutcome, ActionRequest};

/// Trait implemented by each registered Action.
///
/// Implementors receive an [`ActionRequest`] and an [`ActionContext`]
/// (which carries a [`CancellationToken`]) and return an
/// [`ActionOutcome`].
///
/// The runtime uses `tokio::select!` to race the handler against the
/// request deadline and external cancellation, so handlers should
/// cooperatively check the cancellation token for long-running work.
#[async_trait]
pub trait ActionHandler: Send + Sync {
    /// Execute the action described by `request`.
    ///
    /// `ctx` provides a [`CancellationToken`] that the handler should
    /// poll during long-running work so that external cancellation is
    /// responsive.
    async fn invoke(
        &self,
        request: ActionRequest,
        ctx: ActionContext,
    ) -> Result<ActionOutcome, ActionError>;
}
