//! Portal GlobalShortcuts state machine.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// The state of the Portal GlobalShortcuts adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShortcutState {
    /// Portal capability is not available.
    Unavailable,
    /// No shortcuts configured yet.
    Unconfigured,
    /// Currently in the process of binding shortcuts.
    Binding,
    /// Shortcuts are bound and active.
    Bound,
    /// User cancelled the binding UI.
    UserCancelled,
    /// The binding request ended without success.
    RequestEnded,
    /// Session was created but no shortcuts are bound.
    NoShortcutBound,
    /// Recovering from a session/transport failure.
    Recovering { attempt: u32 },
}

/// Backoff schedule for recovery attempts.
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
            max_interval: Duration::from_secs(300), // 5 minutes
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
    fn states_are_distinct() {
        assert_ne!(ShortcutState::UserCancelled, ShortcutState::RequestEnded);
        assert_ne!(ShortcutState::RequestEnded, ShortcutState::NoShortcutBound);
        assert_ne!(ShortcutState::UserCancelled, ShortcutState::NoShortcutBound);
    }
}
