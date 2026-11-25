//! IPC 协议消息类型。
//!
//! Flick IPC 协议在 Unix 域套接字上使用简单的请求/响应模型。
//! 每条消息都是一个长度前缀的 JSON 帧（参见 [`crate::codec`]）。

use serde::{Deserialize, Serialize};

/// 当前协议版本。线格式变更时递增。
pub const IPC_PROTOCOL_VERSION: u16 = 2;

// ─── 信封 ─────────────────────────────────────────────────────────────────────

/// 从客户端 → 服务端发送的请求信封。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcRequestEnvelope {
    pub protocol_version: u16,
    pub request_id: String,
    pub command: IpcCommand,
}

/// 从服务端 → 客户端发送的响应信封。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcResponseEnvelope {
    pub protocol_version: u16,
    pub request_id: String,
    pub result: IpcResult,
}

// ─── 命令 ─────────────────────────────────────────────────────────────────────

/// 客户端可以发出的 IPC 命令。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
#[serde(rename_all = "snake_case")]
pub enum IpcCommand {
    /// 切换 Flick UI 的可见性。
    Toggle,
    /// 通过 ID 触发特定动作，并带有可选的文本输入。
    Trigger { action_id: String, text: String },
    /// 查询守护进程状态。
    Status,
    /// 运行诊断。
    Doctor,
    /// 使用首选的按键触发器（重新）配置全局快捷键。
    ShortcutsConfigure { preferred_trigger: String },
    /// 重新连接到快捷键 Portal。
    ShortcutsReconnect,
    /// 优雅地关闭守护进程。
    Shutdown,
    /// 直接显示搜索面板（绕过 Portal）。
    Show,
    /// 直接隐藏搜索面板。
    Hide,
}

// ─── 结果 ─────────────────────────────────────────────────────────────────────

/// 分派一条 IPC 命令的结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum IpcResult {
    Ok(serde_json::Value),
    Error { code: IpcErrorCode, message: String },
}

impl IpcResult {
    /// 用于构造成功结果的便捷构造函数。
    pub fn ok(value: serde_json::Value) -> Self {
        Self::Ok(value)
    }

    /// 用于构造错误结果的便捷构造函数。
    pub fn error(code: IpcErrorCode, message: impl Into<String>) -> Self {
        Self::Error {
            code,
            message: message.into(),
        }
    }
}

// ─── 错误码 ───────────────────────────────────────────────────────────────────

/// IPC 错误的机器可读错误码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcErrorCode {
    /// 守护进程未运行，或套接字不可达。
    DaemonUnavailable,
    /// 客户端的协议版本与服务端的不匹配。
    ProtocolVersionMismatch,
    /// 该命令无法识别。
    UnknownCommand,
    /// 在注册表中未找到请求的动作 ID。
    ActionNotFound,
    /// 命令负载未通过校验。
    InvalidInput,
    /// 所需的图形桌面会话或 Portal 不可用。
    DesktopUnavailable,
    /// 发生了意外的内部错误。
    Internal,
    /// 该调用被取消。
    Cancelled,
}

impl IpcErrorCode {
    /// 机器可读的错误码字符串。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DaemonUnavailable => "daemon_unavailable",
            Self::ProtocolVersionMismatch => "protocol_version_mismatch",
            Self::UnknownCommand => "unknown_command",
            Self::ActionNotFound => "action_not_found",
            Self::InvalidInput => "invalid_input",
            Self::DesktopUnavailable => "desktop_unavailable",
            Self::Internal => "internal",
            Self::Cancelled => "cancelled",
        }
    }
}
