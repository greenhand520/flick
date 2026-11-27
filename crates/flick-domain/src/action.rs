//! 核心动作类型：描述符、请求、输入、结果。

use crate::caller::Caller;
use crate::error::ActionError;
use crate::execution::ActionSource;
use crate::id::{ActionId, InvocationId};
use serde::{Deserialize, Serialize};
use std::time::Duration;

// ─── 描述符 ───────────────────────────────────────────────────────────────

/// 描述已注册的动作，用于路由和展示。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionDescriptor {
    pub id: ActionId,
    /// 动作版本号，默认 `1.0.0`。
    pub version: String,
    pub title: String,
    /// 用于纯文本搜索召回的关键词。
    pub keywords: Vec<String>,
    pub concurrency: ConcurrencyPolicy,
    /// 该动作是否可作为搜索阶段被调用，返回 `ActionOutcome::Data` 归一化为执行项。
    pub search_invocable: bool,
    pub supported_inputs: Vec<InputType>,
    /// 仅 `InputType::File` 有效；小写、不带点。
    pub file_extensions: Vec<String>,
    pub output_types: Vec<OutputType>,
    pub visibility: ActionVisibility,
    /// 动作额外声明的权限；实际生效权限 = 插件权限 ∪ 动作权限。
    #[serde(default)]
    pub permissions: Vec<String>,
    pub invocation_policy: InvocationPolicy,
}

impl ActionDescriptor {
    pub fn new(
        id: ActionId,
        title: impl Into<String>,
        keywords: Vec<String>,
        concurrency: ConcurrencyPolicy,
    ) -> Self {
        Self {
            id,
            version: "1.0.0".into(),
            title: title.into(),
            keywords,
            concurrency,
            search_invocable: false,
            // 空列表表示该动作接受全部输入类型；具体过滤由 Runtime 的 route_input 完成。
            supported_inputs: Vec::new(),
            file_extensions: Vec::new(),
            output_types: vec![OutputType::Data],
            visibility: ActionVisibility::Public,
            permissions: Vec::new(),
            invocation_policy: InvocationPolicy::NonInteractive,
        }
    }

    /// 计算动作实际拥有的权限：插件声明的权限自动继承，再与动作自身权限合并去重。
    pub fn effective_permissions(&self, plugin_permissions: &[String]) -> Vec<String> {
        let mut result = Vec::new();
        for permission in plugin_permissions.iter().chain(self.permissions.iter()) {
            if !result.iter().any(|existing| existing == permission) {
                result.push(permission.clone());
            }
        }
        result
    }
}

/// 动作的并发策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConcurrencyPolicy {
    /// 多次调用可以并行运行。
    Parallel,
    /// 同一时间只允许一次调用；新请求将排队或被拒绝。
    Exclusive,
}

// ─── 输入 ────────────────────────────────────────────────────────────────────

/// 动作调用的输入。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionInput {
    pub kind: InputType,
    pub text: String,
    /// 仅当 `kind == ClipboardImage` 时携带；图片字节来自剪贴板，不要求是磁盘文件。
    #[serde(default)]
    pub image: Option<ImageData>,
    /// 多文件/文件夹输入；`text` 仍保留首个 file:// URI。
    #[serde(default)]
    pub files: Vec<FileInput>,
}

impl ActionInput {
    pub fn plain_text(text: impl Into<String>) -> Self {
        Self {
            kind: InputType::PlainText,
            text: text.into(),
            image: None,
            files: Vec::new(),
        }
    }

    /// 构造一张来自剪贴板的图片输入。图片不要求落盘为文件。
    pub fn clipboard_image(mime: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            kind: InputType::ClipboardImage,
            text: String::new(),
            image: Some(ImageData {
                mime: mime.into(),
                bytes,
            }),
            files: Vec::new(),
        }
    }

    /// 构造“无输入”的归一化输入（供 `InputType::None` 的动作使用）。
    pub fn none() -> Self {
        Self {
            kind: InputType::None,
            text: String::new(),
            image: None,
            files: Vec::new(),
        }
    }
}

/// 图片输入数据。`mime` 为 MIME 类型，`bytes` 为原始图片字节。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageData {
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// 多文件/文件夹输入项；`text` 仍保留首个 file:// URI。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileInput {
    /// `file://` URI。
    pub uri: String,
    /// 面向用户展示的文件名。
    pub display_name: String,
    /// 小写扩展名（不带点）。
    pub extension: Option<String>,
    /// 是否为目录。
    pub is_dir: bool,
    /// 探测到的 MIME 类型。
    pub mime: Option<String>,
}

/// Phase 1 输入类型分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum InputType {
    #[default]
    PlainText,
    Url,
    PathCandidate,
    MathExpression,
    File,
    Files,
    /// 剪贴板中的临时图片（例如截图后复制），不要求对应磁盘文件。
    ClipboardImage,
    /// 没有输入类型（动作不接受输入）
    None,
}

/// 动作产出的结果类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum OutputType {
    #[default]
    Data,
    Text,
    Notice,
    Url,
    File,
    Surface,
    Error,
}

/// 指向文件的引用（仅接受 `file://` URI）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRef {
    pub uri: String, // 仅接受 file://
    pub display_name: String,
    pub mime: Option<String>,
}

/// 动作对用户的可见性。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ActionVisibility {
    #[default]
    Public,
    Internal,
}

/// 动作的调用策略（是否需要交互）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum InvocationPolicy {
    #[default]
    NonInteractive,
    InteractiveOptional,
    InteractiveRequired,
}

// ─── 请求 ──────────────────────────────────────────────────────────────────

/// 执行已注册动作的请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRequest {
    pub invocation_id: InvocationId,
    pub action_id: ActionId,
    pub input: ActionInput,
    pub caller: Caller,
    /// 发起该请求的阶段，默认 UI。
    #[serde(default)]
    pub source: ActionSource,
    pub deadline: Deadline,
}

impl ActionRequest {
    /// 创建 Echo 动作请求（Phase 0 便捷方法）。
    pub fn new_echo(text: &str, caller: Caller) -> Self {
        Self {
            invocation_id: InvocationId::new(),
            action_id: ActionId::from_static("flick.builtin.echo.v1.echo"),
            input: ActionInput::plain_text(text),
            caller,
            source: ActionSource::Ui,
            deadline: Deadline::from_secs(30),
        }
    }
}

// ─── 截止时间 ─────────────────────────────────────────────────────────────────

/// 调用的截止时间。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Deadline {
    pub timeout: Duration,
}

impl Deadline {
    pub fn from_secs(secs: u64) -> Self {
        Self {
            timeout: Duration::from_secs(secs),
        }
    }
}

// ─── 上下文 ──────────────────────────────────────────────────────────────────

/// 调用期间提供给插件的运行时上下文。
///
/// 携带取消令牌、宿主信息、**调用入口固定的设置快照**，以及可选的 secret
/// 读取器（实时解密，不放进快照，避免 `Debug` 泄露明文）。
#[derive(Default)]
pub struct PluginContext {
    /// 协作式取消令牌。
    pub cancellation: tokio_util::sync::CancellationToken,
    /// 启动时注入的宿主信息（由适配器采集）。
    pub host_info: Option<std::sync::Arc<crate::host::HostInfo>>,
    /// 本次调用固定的设置快照（只含非 secret 值，已合并声明默认值）。
    pub settings: crate::settings::ConfigSnapshot,
    /// 当前动作所属插件 id；`None` 表示非插件注册的动作。
    pub plugin_id: Option<String>,
    /// secret 值读取器；宿主未提供服务时为 `None`。
    pub secrets: Option<std::sync::Arc<dyn crate::settings::SettingSecretReader>>,
}

impl PluginContext {
    /// 用取消令牌构造一个其余字段全空的上下文。
    pub fn new(cancellation: tokio_util::sync::CancellationToken) -> Self {
        Self {
            cancellation,
            ..Self::default()
        }
    }

    /// 设置快照（链式）。
    pub fn with_settings(mut self, settings: crate::settings::ConfigSnapshot) -> Self {
        self.settings = settings;
        self
    }

    /// 设置插件 id（链式）。
    pub fn with_plugin_id(mut self, plugin_id: Option<String>) -> Self {
        self.plugin_id = plugin_id;
        self
    }

    /// 设置 secret 读取器（链式）。
    pub fn with_secrets(
        mut self,
        secrets: Option<std::sync::Arc<dyn crate::settings::SettingSecretReader>>,
    ) -> Self {
        self.secrets = secrets;
        self
    }

    /// 设置宿主信息（链式）。
    pub fn with_host_info(
        mut self,
        host_info: Option<std::sync::Arc<crate::host::HostInfo>>,
    ) -> Self {
        self.host_info = host_info;
        self
    }

    /// 本次调用的设置快照。
    pub fn settings(&self) -> &crate::settings::ConfigSnapshot {
        &self.settings
    }

    /// 当前动作所属插件 id。
    pub fn plugin_id(&self) -> Option<&str> {
        self.plugin_id.as_deref()
    }

    /// 读取一个 secret 值；无读取器或不存在时返回 `None`。
    pub fn secret(&self, key: &str) -> Option<String> {
        let plugin_id = self.plugin_id.as_deref()?;
        self.secrets.as_ref()?.secret(plugin_id, key)
    }
}

// ─── 结果 ──────────────────────────────────────────────────────────────────

/// 动作调用的结果。
///
/// 与框架无关：UI 和 CLI 都会使用该结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ActionOutcome {
    /// 结构化数据结果。
    Data(serde_json::Value),
    /// 人类可读的通知（非错误）。
    Notice { message: String },
    /// 执行期间发生错误。
    Error(ActionError),
    /// 纯文本结果。
    Text(String),
    /// URL 结果。
    Url(String),
    /// 文件结果。
    File(FileRef),
    /// 请求打开一个 Surface（内容为强类型的数据驱动节点树）。
    OpenSurface {
        surface_id: String,
        content: crate::surface::SurfaceContent,
        /// 会话 id；由插件构造时为 None，由 `ActionRuntime::invoke` 回填。
        #[serde(default)]
        session_id: Option<crate::id::SurfaceSessionId>,
        /// 该动作所属插件是否需要宿主搜索框；由运行时按 manifest 回填。
        #[serde(default)]
        needs_search: bool,
    },
}

impl ActionOutcome {
    /// 构造一个 `OpenSurface` 结果。
    ///
    /// 插件 handler 应使用此 helper；`session_id` 保持 `None`、`needs_search`
    /// 保持 `false`，随后由 `ActionRuntime::invoke` 回填真实值。
    pub fn open_surface(
        surface_id: impl Into<String>,
        content: crate::surface::SurfaceContent,
    ) -> Self {
        Self::OpenSurface {
            surface_id: surface_id.into(),
            content,
            session_id: None,
            needs_search: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caller::CallerKind;

    #[test]
    fn effective_permissions_inherit_plugin_permissions_and_dedupe() {
        let mut descriptor = ActionDescriptor::new(
            ActionId::from_static("demo.action"),
            "Demo",
            vec!["demo".into()],
            ConcurrencyPolicy::Parallel,
        );
        descriptor.permissions = vec!["fs.read".into(), "network.http".into()];
        let effective =
            descriptor.effective_permissions(&["fs.read".into(), "clipboard.read".into()]);
        assert_eq!(
            effective,
            vec![
                "fs.read".to_string(),
                "clipboard.read".to_string(),
                "network.http".to_string()
            ]
        );
    }

    #[test]
    fn clipboard_image_input_round_trips() {
        let input = ActionInput::clipboard_image("image/png", vec![1, 2, 3, 4]);
        assert_eq!(input.kind, InputType::ClipboardImage);
        assert_eq!(input.image.as_ref().unwrap().mime, "image/png");
        assert_eq!(input.image.as_ref().unwrap().bytes, vec![1, 2, 3, 4]);
        let json = serde_json::to_string(&input).unwrap();
        let back: ActionInput = serde_json::from_str(&json).unwrap();
        assert_eq!(input, back);
    }

    #[test]
    fn old_action_input_without_image_still_decodes() {
        let input: ActionInput =
            serde_json::from_value(serde_json::json!({"kind":"PlainText","text":"hi"})).unwrap();
        assert_eq!(input, ActionInput::plain_text("hi"));
    }

    #[test]
    fn action_request_echo_has_ui_caller() {
        let req = ActionRequest::new_echo("hello", Caller::ui("search-panel"));
        assert_eq!(req.caller.kind, CallerKind::Ui);
        assert_eq!(req.caller.id, "search-panel");
    }

    #[test]
    fn action_request_echo_has_unique_invocation() {
        let a = ActionRequest::new_echo("test", Caller::cli("cli"));
        let b = ActionRequest::new_echo("test", Caller::cli("cli"));
        assert_ne!(a.invocation_id, b.invocation_id);
    }

    #[test]
    fn action_outcome_data_serializes() {
        let outcome = ActionOutcome::Data(serde_json::json!({"echo": "hello"}));
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(json.contains("hello"));
    }

    #[test]
    fn action_outcome_round_trips() {
        let outcomes = vec![
            ActionOutcome::Data(serde_json::json!({"echo": "hi"})),
            ActionOutcome::Notice {
                message: "done".into(),
            },
            ActionOutcome::Error(ActionError::Cancelled),
        ];
        for outcome in outcomes {
            let json = serde_json::to_string(&outcome).unwrap();
            let back: ActionOutcome = serde_json::from_str(&json).unwrap();
            // 调试比较
            assert_eq!(format!("{outcome:?}"), format!("{back:?}"));
        }
    }

    fn empty_surface() -> crate::surface::SurfaceContent {
        crate::surface::SurfaceContent {
            schema_version: crate::surface::SURFACE_SCHEMA_VERSION,
            title: None,
            body: vec![],
        }
    }

    #[test]
    fn open_surface_helper_sets_placeholders() {
        match ActionOutcome::open_surface("info", empty_surface()) {
            ActionOutcome::OpenSurface {
                surface_id,
                session_id,
                needs_search,
                ..
            } => {
                assert_eq!(surface_id, "info");
                assert!(session_id.is_none());
                assert!(!needs_search);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn legacy_open_surface_json_without_new_fields_decodes() {
        // 旧版载荷：不含 session_id / needs_search。
        let legacy = serde_json::json!({
            "OpenSurface": {
                "surface_id": "info",
                "content": { "schema_version": 1, "body": [] }
            }
        });
        let outcome: ActionOutcome = serde_json::from_value(legacy).unwrap();
        match outcome {
            ActionOutcome::OpenSurface {
                session_id,
                needs_search,
                ..
            } => {
                assert!(session_id.is_none());
                assert!(!needs_search);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn open_surface_with_session_round_trips() {
        let outcome = ActionOutcome::OpenSurface {
            surface_id: "info".into(),
            content: empty_surface(),
            session_id: Some(crate::id::SurfaceSessionId::new()),
            needs_search: true,
        };
        let json = serde_json::to_string(&outcome).unwrap();
        let back: ActionOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(outcome, back);
    }
}
