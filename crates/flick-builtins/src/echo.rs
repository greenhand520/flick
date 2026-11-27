//! Echo 内置动作 —— Flick 动作的 "hello world"。

use std::sync::Arc;

use flick_domain::{
    ActionDescriptor, ActionError, ActionId, ActionOutcome, ActionRequest, Category,
    ConcurrencyPolicy, InputType, PluginContext, PluginKind, PluginManifest, PluginProfile,
    SettingDecl, SettingKind, SettingValue,
};
use flick_runtime::{ActionHandler, ActionSpec, Plugin};

/// Echo 动作的稳定标识符。
pub const ECHO_ACTION_ID: &str = "flick.builtin.echo.v1.echo";

/// Echo 插件设置：回显前缀（短 key `prefix`）。
///
/// 演示「插件读取自己的设置快照」：值来自 `PluginContext.settings()`，
/// 一次调用内 revision 固定；设置不可用时按空前缀降级。
pub const PREFIX_SETTING_KEY: &str = "prefix";

/// 一个将文本输入原样回显为结构化数据的简单动作处理器。
///
/// Phase 0 实现：仅接受 `PlainText` 输入，返回
/// `ActionOutcome::Data({"echo": "<prefix><text>"})`。不执行任何
/// 文件、网络或 UI 操作。不根据 `CallerKind` 改变输出。
pub struct EchoHandler;

#[async_trait::async_trait]
impl ActionHandler for EchoHandler {
    async fn invoke(
        &self,
        request: ActionRequest,
        ctx: PluginContext,
    ) -> Result<ActionOutcome, ActionError> {
        // Phase 0：仅接受纯文本输入。
        if request.input.kind != InputType::PlainText {
            return Err(ActionError::InvalidInput {
                message: "Echo only accepts plain-text input".into(),
            });
        }

        // 读取本插件命名空间下的 `prefix`；没有设置或设置不可用时按空串降级。
        let prefix = ctx.settings().str(PREFIX_SETTING_KEY).unwrap_or("");
        let text = request.input.text;
        Ok(ActionOutcome::Data(
            serde_json::json!({ "echo": format!("{prefix}{text}") }),
        ))
    }
}

/// 构造 Echo 动作的描述符（供 [`EchoPlugin`] 使用）。
pub fn echo_descriptor() -> ActionDescriptor {
    ActionDescriptor::new(
        ActionId::from_static(ECHO_ACTION_ID),
        "Echo",
        vec!["echo".into(), "回显".into(), "repeater".into()],
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
            author: "Flick".into(),
            version: "1.0.0".into(),
            kind: PluginKind::BuiltInRust,
            icon: None,
            keywords: vec!["echo".into(), "回显".into(), "repeater".into()],
            profile: PluginProfile {
                description: "Echo 内置动作：将输入原样回显为结构化数据。".into(),
                categories: vec![Category::Developer],
                tags: vec!["echo".into(), "text".into()],
                ..Default::default()
            },
            permissions: vec![],
            plugin_dependencies: vec![],
            settings: vec![SettingDecl {
                key: PREFIX_SETTING_KEY.into(),
                renamed_from: None,
                label: "回显前缀".into(),
                description: "回显结果前拼接的文本，默认不添加任何前缀。".into(),
                kind: SettingKind::String,
                default: Some(SettingValue::String(String::new())),
                secret: false,
            }],
            surfaces: vec![],
            // 纯逻辑插件：不限平台与架构。
            supported_os: vec![],
            supported_architectures: vec![],
            needs_search: false,
        }
    }

    fn actions(&self) -> Vec<ActionSpec> {
        vec![ActionSpec {
            descriptor: echo_descriptor(),
            handler: Arc::new(EchoHandler),
        }]
    }
}
