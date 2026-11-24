//! 查询代次跟踪。
//!
//! `QueryTracker` 确保在较旧的查询完成之前提交了较新的查询时，
//! 陈旧的路由结果会被丢弃。

use std::sync::atomic::{AtomicU64, Ordering};

use flick_domain::QueryGeneration;

/// 跟踪当前查询代次，以丢弃乱序到达的结果。
///
/// UI 为每条 `QueryChanged` 消息分配一个单调递增的 `QueryGeneration`。
/// 协调器在调用 `runtime.route()` 之前记录最新的代次，随后检查
/// 它是否仍为最新。如果在此期间有较新的查询到达，陈旧的结果
/// 将被静默丢弃。
///
/// 此类型为 `Send + Sync`，可安全地在异步任务之间共享。
pub struct QueryTracker {
    /// 存储当前 `QueryGeneration` 的 `u64` 值。
    /// `0` 表示"尚未提交任何查询"。
    current: AtomicU64,
}

impl QueryTracker {
    /// 创建一个没有活动代次的跟踪器。
    pub const fn new() -> Self {
        Self {
            current: AtomicU64::new(0),
        }
    }

    /// 将 `generation` 记录为当前（最新）代次。
    pub fn set_current(&self, generation: QueryGeneration) {
        self.current.store(generation.value(), Ordering::SeqCst);
    }

    /// 如果 `generation` 仍是最新代次，则返回 `true`。
    pub fn is_current(&self, generation: QueryGeneration) -> bool {
        self.current.load(Ordering::SeqCst) == generation.value()
    }

    /// 返回当前代次的原始值（0 表示无）。
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
