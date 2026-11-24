//! Portal 会话生命周期类型。

use serde::{Deserialize, Serialize};
use std::fmt;

/// 关于 show-search 快捷键绑定的信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutBinding {
    pub id: String,
    pub description: String,
    pub preferred_trigger: String,
    pub actual_trigger: Option<String>,
}

/// 来自 Portal 的激活事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutActivation {
    pub shortcut_id: String,
    pub activation_token: Option<String>,
}

/// 来自 Portal/快捷键操作的错误。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ShortcutError {
    /// Portal 接口不可用。
    Unavailable { reason: String },
    /// 用户取消了绑定对话框。
    UserCancelled,
    /// Portal 请求在未完成绑定的情况下结束。
    RequestEnded { reason: String },
    /// 会话或传输错误（可恢复）。
    SessionError { reason: String },
    /// 内部错误。
    Internal { message: String },
}

impl ShortcutError {
    /// 用于诊断的、面向操作员的稳定类别。
    pub fn category(&self) -> &'static str {
        match self {
            Self::Unavailable { .. } => "unavailable",
            Self::UserCancelled => "user_cancelled",
            Self::RequestEnded { .. } => "request_ended",
            Self::SessionError { .. } => "session",
            Self::Internal { .. } => "internal",
        }
    }

    /// 不含用户载荷的可读错误消息。
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

// 以函数形式定义快捷键（Rust 不支持 const String）。
pub fn show_search_shortcut_id() -> &'static str {
    "show-search"
}

pub fn show_search_shortcut_description() -> &'static str {
    "显示 Flick 搜索面板"
}

pub fn show_search_preferred_trigger() -> &'static str {
    "Super+Space"
}
