//! IPC protocol message types.
//!
//! The Flick IPC protocol uses a simple request/response model over Unix
//! domain sockets.  Each message is a length-prefixed JSON frame (see
//! [`crate::codec`]).

use serde::{Deserialize, Serialize};

/// Current protocol version.  Bump when the wire format changes.
pub const IPC_PROTOCOL_VERSION: u16 = 1;

// ─── Envelopes ────────────────────────────────────────────────────────────────

/// A request envelope sent from client → server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcRequestEnvelope {
    pub protocol_version: u16,
    pub request_id: String,
    pub command: IpcCommand,
}

/// A response envelope sent from server → client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcResponseEnvelope {
    pub protocol_version: u16,
    pub request_id: String,
    pub result: IpcResult,
}

// ─── Commands ─────────────────────────────────────────────────────────────────

/// IPC commands that a client can issue.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
#[serde(rename_all = "snake_case")]
pub enum IpcCommand {
    /// Toggle the Flick UI visibility.
    Toggle,
    /// Trigger a specific action by ID with optional text input.
    Trigger {
        action_id: String,
        text: String,
    },
    /// Query daemon status.
    Status,
    /// Run diagnostics.
    Doctor,
    /// (Re)configure global shortcuts with a preferred key trigger.
    ShortcutsConfigure {
        preferred_trigger: String,
    },
    /// Reconnect to the shortcut portal.
    ShortcutsReconnect,
    /// Gracefully shut down the daemon.
    Shutdown,
    /// Show the search panel directly (bypass Portal).
    Show,
    /// Hide the search panel directly.
    Hide,
}

// ─── Results ──────────────────────────────────────────────────────────────────

/// The result of dispatching an IPC command.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum IpcResult {
    Ok(serde_json::Value),
    Error {
        code: IpcErrorCode,
        message: String,
    },
}

impl IpcResult {
    /// Convenience constructor for successful results.
    pub fn ok(value: serde_json::Value) -> Self {
        Self::Ok(value)
    }

    /// Convenience constructor for error results.
    pub fn error(code: IpcErrorCode, message: impl Into<String>) -> Self {
        Self::Error {
            code,
            message: message.into(),
        }
    }
}

// ─── Error codes ──────────────────────────────────────────────────────────────

/// Machine-readable error codes for IPC errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcErrorCode {
    /// The daemon is not running or the socket is unreachable.
    DaemonUnavailable,
    /// The client's protocol version does not match the server's.
    ProtocolVersionMismatch,
    /// The command is not recognised.
    UnknownCommand,
    /// The requested action ID was not found in the registry.
    ActionNotFound,
    /// The command payload failed validation.
    InvalidInput,
    /// An unexpected internal error occurred.
    Internal,
    /// The invocation was cancelled.
    Cancelled,
}

impl IpcErrorCode {
    /// Machine-readable error code string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DaemonUnavailable => "daemon_unavailable",
            Self::ProtocolVersionMismatch => "protocol_version_mismatch",
            Self::UnknownCommand => "unknown_command",
            Self::ActionNotFound => "action_not_found",
            Self::InvalidInput => "invalid_input",
            Self::Internal => "internal",
            Self::Cancelled => "cancelled",
        }
    }
}
