//! Query generation tracking.
//!
//! `QueryTracker` ensures that stale routing results are discarded when a
//! newer query has been submitted before the older one completed.

use std::sync::atomic::{AtomicU64, Ordering};

use flick_domain::QueryGeneration;

/// Tracks the current query generation to discard out-of-order results.
///
/// The UI assigns a monotonically increasing `QueryGeneration` to each
/// `QueryChanged` message. The coordinator records the latest generation
/// before calling `runtime.route()`, and checks afterwards whether it is
/// still current. If a newer query arrived in the meantime the stale
/// results are silently dropped.
///
/// This type is `Send + Sync` and safe to share across async tasks.
pub struct QueryTracker {
    /// Stores the `u64` value of the current `QueryGeneration`.
    /// `0` means "no query has been submitted yet".
    current: AtomicU64,
}

impl QueryTracker {
    /// Create a tracker with no active generation.
    pub const fn new() -> Self {
        Self {
            current: AtomicU64::new(0),
        }
    }

    /// Record `generation` as the current (latest) generation.
    pub fn set_current(&self, generation: QueryGeneration) {
        self.current.store(generation.value(), Ordering::SeqCst);
    }

    /// Returns `true` if `generation` is still the most recent generation.
    pub fn is_current(&self, generation: QueryGeneration) -> bool {
        self.current.load(Ordering::SeqCst) == generation.value()
    }

    /// Returns the raw value of the current generation (0 = none).
    pub fn current_value(&self) -> u64 {
        self.current.load(Ordering::SeqCst)
    }
}

impl Default for QueryTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_tracker_has_no_generation() {
        let tracker = QueryTracker::new();
        assert_eq!(tracker.current_value(), 0);
    }

    #[test]
    fn set_current_and_check() {
        let tracker = QueryTracker::new();
        let generation = QueryGeneration::INITIAL;
        tracker.set_current(generation);
        assert!(tracker.is_current(generation));
    }

    #[test]
    fn newer_generation_invalidates_old() {
        let tracker = QueryTracker::new();
        let gen1 = QueryGeneration::INITIAL;
        let gen2 = gen1.next();
        tracker.set_current(gen1);
        assert!(tracker.is_current(gen1));
        tracker.set_current(gen2);
        assert!(!tracker.is_current(gen1));
        assert!(tracker.is_current(gen2));
    }
}
