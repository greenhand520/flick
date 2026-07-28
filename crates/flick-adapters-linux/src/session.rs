//! Portal session lifecycle types.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Information about the show-search shortcut binding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutBinding {
    pub id: String,
    pub description: String,
    pub preferred_trigger: String,
    pub actual_trigger: Option<String>,
}

/// Activation event from Portal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutActivation {
    pub shortcut_id: String,
    pub activation_token: Option<String>,
}

/// Errors from Portal/shortcut operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ShortcutError {
    /// Portal interface not available.
    Unavailable { reason: String },
    /// User cancelled the binding dialog.
    UserCancelled,
    /// The Portal request ended without binding.
    RequestEnded { reason: String },
    /// Session or transport error (recoverable).
    SessionError { reason: String },
    /// Internal error.
    Internal { message: String },
}

impl ShortcutError {
    /// Stable operator-facing category used in diagnostics.
    pub fn category(&self) -> &'static str {
        match self {
            Self::Unavailable { .. } => "unavailable",
            Self::UserCancelled => "user_cancelled",
            Self::RequestEnded { .. } => "request_ended",
            Self::SessionError { .. } => "session",
            Self::Internal { .. } => "internal",
        }
    }

    /// Human-readable error message without user payloads.
    pub fn diagnostic_message(&self) -> String {
        match self {
            Self::Unavailable { reason }
            | Self::RequestEnded { reason }
            | Self::SessionError { reason }
            | Self::Internal { message: reason } => reason.clone(),
            Self::UserCancelled => "User cancelled shortcut configuration".into(),
        }
    }
}

impl fmt::Display for ShortcutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable { reason } => write!(f, "Portal unavailable: {reason}"),
            Self::UserCancelled => write!(f, "User cancelled shortcut configuration"),
            Self::RequestEnded { reason } => {
                write!(f, "Shortcut request ended: {reason}")
            }
            Self::SessionError { reason } => write!(f, "Session error: {reason}"),
            Self::Internal { message } => write!(f, "Internal error: {message}"),
        }
    }
}

impl std::error::Error for ShortcutError {}

// Shortcut definitions as functions (const String not supported in Rust).
pub fn show_search_shortcut_id() -> &'static str {
    "show-search"
}

pub fn show_search_shortcut_description() -> &'static str {
    "显示 Flick 搜索面板"
}

pub fn show_search_preferred_trigger() -> &'static str {
    "Super+Space"
}
