//! Echo builtin action — the "hello world" of Flick actions.

use flick_domain::{ActionContext, ActionError, ActionOutcome, ActionRequest, InputKind};
use flick_runtime::ActionHandler;

/// Stable identifier for the Echo action.
pub const ECHO_ACTION_ID: &str = "flick.builtin.echo.v1.echo";

/// A trivial action handler that echoes its text input back as structured data.
///
/// Phase 0 implementation: accepts only `PlainText` input, returns
/// `ActionOutcome::Data({"echo": "<text>"})`.  Does not perform any
/// file, network, or UI operations.  Does not vary output based on
/// `CallerKind`.
pub struct EchoHandler;

#[async_trait::async_trait]
impl ActionHandler for EchoHandler {
    async fn invoke(
        &self,
        request: ActionRequest,
        _ctx: ActionContext,
    ) -> Result<ActionOutcome, ActionError> {
        // Phase 0: only accept plain-text input.
        if request.input.kind != InputKind::PlainText {
            return Err(ActionError::InvalidInput {
                message: "Echo only accepts plain-text input".into(),
            });
        }

        let text = request.input.text;
        Ok(ActionOutcome::Data(serde_json::json!({ "echo": text })))
    }
}
