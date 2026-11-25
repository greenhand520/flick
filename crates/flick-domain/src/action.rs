//! 核心动作类型：描述符、请求、输入、结果。

use crate::caller::Caller;
use crate::error::ActionError;
use crate::id::{ActionId, InvocationId};
use serde::{Deserialize, Serialize};
use std::time::Duration;

// ─── 描述符 ───────────────────────────────────────────────────────────────

/// 描述已注册的动作，用于路由和展示。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionDescriptor {
    pub id: ActionId,
    pub title: String,
    /// 用于纯文本搜索召回的关键词。
    pub keywords: Vec<String>,
    pub matcher: Matcher,
    pub concurrency: ConcurrencyPolicy,
    pub supported_inputs: Vec<InputType>,
    /// 仅 `InputType::File` 有效；小写、不带点。
    pub file_extensions: Vec<String>,
    pub output_types: Vec<OutputType>,
    pub visibility: ActionVisibility,
    pub risk_level: RiskLevel,
    pub invocation_policy: InvocationPolicy,
}

impl ActionDescriptor {
    pub fn new(
        id: ActionId,
        title: impl Into<String>,
        keywords: Vec<String>,
        matcher: Matcher,
        concurrency: ConcurrencyPolicy,
    ) -> Self {
        Self {
            id,
            title: title.into(),
            keywords,
            matcher,
            concurrency,
            supported_inputs: vec![InputType::PlainText],
            file_extensions: Vec::new(),
            output_types: vec![OutputType::Data],
            visibility: ActionVisibility::Public,
            risk_level: RiskLevel::ReadOnly,
            invocation_policy: InvocationPolicy::NonInteractive,
        }
    }
}

/// 此动作如何与用户输入进行匹配。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Matcher {
    /// 基于关键词从纯文本输入中召回。
    Keyword,
    /// 精确前缀命令，采用词边界匹配。
    /// 例如，`echo` 匹配 "echo hello"，但不匹配 "echohello"。
    PrefixCommand { command: String },
}

impl Matcher {
    /// 前缀命令匹配器的便捷构造函数。
    pub fn prefix_command(command: &str) -> Self {
        Self::PrefixCommand {
            command: command.to_owned(),
        }
    }

    /// 如果这是 `PrefixCommand` 匹配器，则返回命令字符串。
    pub fn as_prefix_command(&self) -> Option<&str> {
        match self {
            Self::PrefixCommand { command } => Some(command),
            _ => None,
        }
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
    pub kind: InputKind,
    pub text: String,
}

impl ActionInput {
    pub fn plain_text(text: impl Into<String>) -> Self {
        Self {
            kind: InputKind::PlainText,
            text: text.into(),
        }
    }
}

/// Phase 0 输入类型分类器。
///
/// Phase 1 将用更丰富的 InputFacet 模型替换/增强此实现。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputKind {
    PlainText,
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

/// 动作的风险等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RiskLevel {
    #[default]
    ReadOnly,
    SideEffecting,
    Destructive,
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

/// 调用期间提供给 ActionHandler 的运行时上下文。
///
/// 携带一个取消令牌，用于协作式取消。
pub struct ActionContext {
    pub cancellation: tokio_util::sync::CancellationToken,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caller::CallerKind;

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

    #[test]
    fn matcher_prefix_command_round_trips() {
        let m = Matcher::prefix_command("echo");
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("echo"));
        let back: Matcher = serde_json::from_str(&json).unwrap();
        assert_eq!(back.as_prefix_command(), Some("echo"));
    }
}
