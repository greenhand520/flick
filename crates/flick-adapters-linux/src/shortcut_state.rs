//! Portal GlobalShortcuts 状态与诊断模型。

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// 操作员可见的 Portal GlobalShortcuts 适配器状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShortcutState {
    /// 当前无法使用桌面 Portal 能力。
    Unavailable { reason: String },
    /// Portal 会话可用，但未列出 show-search。
    Unconfigured,
    /// 用户显式请求的绑定操作正在等待 Portal 响应。
    Configuring,
    /// 当前活跃的 Portal 会话列出了 show-search 快捷键。
    Bound,
    /// 用户取消或拒绝了 Portal 绑定请求。
    Denied,
    /// 已关闭/失败的会话已安排一次“打开并列出”恢复操作。
    Recovering { attempt: u32 },
    /// Portal、传输或协议操作失败。
    Error { operation: String, message: String },
}

/// 为操作员诊断保留的最近一次 Portal 错误。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortalDiagnosticError {
    pub operation: String,
    pub category: String,
    pub message: String,
}

/// 快捷键状态及其活跃 Portal 会话的可序列化快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortalSnapshot {
    pub state: ShortcutState,
    pub session_active: bool,
    pub shortcut_count: u32,
    pub last_error: Option<PortalDiagnosticError>,
    pub reconnect_attempt: Option<u32>,
}

/// Portal 失败后一次自动恢复的退避计划。
pub struct RecoveryBackoff {
    schedule: Vec<Duration>,
    max_interval: Duration,
}

impl RecoveryBackoff {
    pub fn new() -> Self {
        Self {
            schedule: vec![
                Duration::from_secs(2),
                Duration::from_secs(5),
                Duration::from_secs(15),
                Duration::from_secs(60),
            ],
            max_interval: Duration::from_secs(300),
        }
    }

    /// 获取给定尝试次数对应的延迟（从 0 开始计数）。
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let idx = attempt as usize;
        if idx < self.schedule.len() {
            self.schedule[idx]
        } else {
            self.max_interval
        }
    }
}

impl Default for RecoveryBackoff {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_backoff_follows_schedule() {
        let bo = RecoveryBackoff::new();
        assert_eq!(bo.delay_for_attempt(0), Duration::from_secs(2));
        assert_eq!(bo.delay_for_attempt(1), Duration::from_secs(5));
        assert_eq!(bo.delay_for_attempt(2), Duration::from_secs(15));
        assert_eq!(bo.delay_for_attempt(3), Duration::from_secs(60));
        assert_eq!(bo.delay_for_attempt(10), Duration::from_secs(300));
    }

    #[test]
    fn denied_and_error_are_distinct_operator_states() {
        assert_ne!(
            ShortcutState::Denied,
            ShortcutState::Error {
                operation: "BindShortcuts".into(),
                message: "Portal request ended".into(),
            }
        );
    }
}
