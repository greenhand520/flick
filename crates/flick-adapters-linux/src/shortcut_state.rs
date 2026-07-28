//! Portal GlobalShortcuts state and diagnostics models.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Operator-visible state of the Portal GlobalShortcuts adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShortcutState {
    /// The desktop Portal capability cannot currently be used.
    Unavailable { reason: String },
    /// A Portal session is available but does not list show-search.
    Unconfigured,
    /// An explicit user-requested binding operation is awaiting Portal.
    Configuring,
    /// The active Portal session lists the show-search shortcut.
    Bound,
    /// The user cancelled or rejected the Portal binding request.
    Denied,
    /// A closed/failed session has one scheduled open-and-list recovery.
    Recovering { attempt: u32 },
    /// A Portal, transport, or protocol operation failed.
    Error { operation: String, message: String },
}

/// Last Portal error retained for operator diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortalDiagnosticError {
    pub operation: String,
    pub category: String,
    pub message: String,
}

/// Serializable snapshot of shortcut state and its active Portal session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortalSnapshot {
    pub state: ShortcutState,
    pub session_active: bool,
    pub shortcut_count: u32,
    pub last_error: Option<PortalDiagnosticError>,
    pub reconnect_attempt: Option<u32>,
}

/// Backoff schedule for one automatic recovery after a Portal failure.
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

    /// Get the delay for the given attempt (0-indexed).
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
