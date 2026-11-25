//! Echo 内置动作 —— Flick 动作的 "hello world"。

use std::sync::Arc;

use flick_domain::{
    ActionContext, ActionDescriptor, ActionError, ActionId, ActionOutcome, ActionRequest, Category,
    ConcurrencyPolicy, InputType, Matcher, PluginKind, PluginManifest, PluginProfile,
};
use flick_runtime::{ActionHandler, ActionSpec, Plugin};

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
        if request.input.kind != InputType::PlainText {
            return Err(ActionError::InvalidInput {
                message: "Echo only accepts plain-text input".into(),
            });
        }

        let text = request.input.text;
        Ok(ActionOutcome::Data(serde_json::json!({ "echo": text })))
    }
}

/// 构造 Echo 动作的描述符（供 [`EchoPlugin`] 使用）。
pub fn echo_descriptor() -> ActionDescriptor {
    ActionDescriptor::new(
        ActionId::from_static(ECHO_ACTION_ID),
        "Echo",
        vec!["echo".into(), "回显".into(), "repeater".into()],
        Matcher::prefix_command("echo"),
        ConcurrencyPolicy::Parallel,
    )
}

/// Echo 插件：将 Phase 0 的 echo 动作作为 [`Plugin`] 暴露。
pub struct EchoPlugin;

impl Plugin for EchoPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "flick.builtin.echo".into(),
            name: "Echo".into(),
            version: "1.0.0".into(),
            kind: PluginKind::BuiltInRust,
            keywords: vec!["echo".into(), "回显".into(), "repeater".into()],
            profile: PluginProfile {
                description: "Echo 内置动作：将输入原样回显为结构化数据。".into(),
                categories: vec![Category::Developer],
                tags: vec!["echo".into(), "text".into()],
                ..Default::default()
            },
            capabilities: vec![],
            plugin_dependencies: vec![],
            settings: vec![],
            surfaces: vec![],
            // 纯逻辑插件：不限平台与架构。
            supported_os: vec![],
            supported_architectures: vec![],
        }
    }

    fn actions(&self) -> Vec<ActionSpec> {
        vec![ActionSpec {
            descriptor: echo_descriptor(),
            handler: Arc::new(EchoHandler),
        }]
    }
}
