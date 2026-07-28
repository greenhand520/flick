//! Structured error model for Action execution.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors that can occur during Action invocation or system operation.
///
/// Each variant carries a stable machine-readable `code` and a
/// user-safe `message` that does not leak internal details.
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
    /// Stable machine-readable error code for IPC/JSON consumers.
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

    /// User-safe message (no D-Bus paths, socket addresses, or raw context).
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
