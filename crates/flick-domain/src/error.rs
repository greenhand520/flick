//! 动作执行的结构化错误模型。

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 动作调用或系统操作期间可能发生的错误。
///
/// 每个变体都带有稳定的机器可读 `code`，以及
/// 不会泄露内部细节的用户安全 `message`。
#[derive(Debug, Clone, PartialEq, Error, Serialize, Deserialize)]
pub enum ActionError {
    #[error("unknown action: {action_id}")]
    UnknownAction { action_id: String },

    #[error("invalid input: {message}")]
    InvalidInput { message: String },

    #[error("deadline exceeded for invocation")]
    DeadlineExceeded,

    #[error("invocation was cancelled")]
    Cancelled,

    #[error("action unavailable: {reason}")]
    Unavailable { reason: String },

    #[error("unsupported session: {reason}")]
    UnsupportedSession { reason: String },

    #[error("internal error: {message}")]
    Internal { message: String },
}

impl ActionError {
    /// 供 IPC/JSON 使用者使用的稳定的机器可读错误码。
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownAction { .. } => "UNKNOWN_ACTION",
            Self::InvalidInput { .. } => "INVALID_INPUT",
            Self::DeadlineExceeded => "DEADLINE_EXCEEDED",
            Self::Cancelled => "CANCELLED",
            Self::Unavailable { .. } => "UNAVAILABLE",
            Self::UnsupportedSession { .. } => "UNSUPPORTED_SESSION",
            Self::Internal { .. } => "INTERNAL",
        }
    }

    /// 用户安全的消息（不包含 D-Bus 路径、套接字地址或原始上下文）。
    pub fn user_message(&self) -> String {
        match self {
            Self::UnknownAction { action_id } => {
                format!("Unknown action: {action_id}")
            }
            Self::InvalidInput { message } => message.clone(),
            Self::DeadlineExceeded => "The operation timed out.".into(),
            Self::Cancelled => "The operation was cancelled.".into(),
            Self::Unavailable { reason } => {
                format!("Action unavailable: {reason}")
            }
            Self::UnsupportedSession { reason } => {
                format!("Unsupported session: {reason}")
            }
            Self::Internal { .. } => "An internal error occurred.".into(),
        }
    }
}
