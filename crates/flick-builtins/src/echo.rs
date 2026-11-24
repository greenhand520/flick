//! Echo 内置动作 —— Flick 动作的 "hello world"。

use flick_domain::{ActionContext, ActionError, ActionOutcome, ActionRequest, InputKind};
use flick_runtime::ActionHandler;

/// Echo 动作的稳定标识符。
pub const ECHO_ACTION_ID: &str = "flick.builtin.echo.v1.echo";

/// 一个将文本输入原样回显为结构化数据的简单动作处理器。
///
/// Phase 0 实现：仅接受 `PlainText` 输入，返回
/// `ActionOutcome::Data({"echo": "<text>"})`。不执行任何
/// 文件、网络或 UI 操作。不根据 `CallerKind` 改变输出。
pub struct EchoHandler;

#[async_trait::async_trait]
impl ActionHandler for EchoHandler {
    async fn invoke(
        &self,
        request: ActionRequest,
        _ctx: ActionContext,
    ) -> Result<ActionOutcome, ActionError> {
        // Phase 0：仅接受纯文本输入。
        if request.input.kind != InputKind::PlainText {
            return Err(ActionError::InvalidInput {
                message: "Echo only accepts plain-text input".into(),
            });
        }

        let text = request.input.text;
        Ok(ActionOutcome::Data(serde_json::json!({ "echo": text })))
    }
}
