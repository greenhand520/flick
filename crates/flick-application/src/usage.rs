//! 使用频率（frecency）存储的内存实现。
//!
//! Application 层持有该存储并在动作成功调用后记录使用；Runtime 通过
//! [`flick_domain::UsageStore`] 读取加成。第一版只保存在内存中，进程退出即丢失。

use flick_domain::{ActionId, UsageStat, UsageStore, now_secs};
use std::collections::HashMap;
use std::sync::Mutex;

/// 内存中的使用频率存储。
#[derive(Default)]
pub struct InMemoryUsageStore {
    /// 动作 id → 使用统计。
    stats: Mutex<HashMap<ActionId, UsageStat>>,
}

impl InMemoryUsageStore {
    /// 创建一个空存储。
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回某个动作当前的原始统计快照（测试与诊断用）。
    pub fn stat(&self, action_id: &ActionId) -> Option<UsageStat> {
        self.stats
            .lock()
            .ok()
            .and_then(|stats| stats.get(action_id).cloned())
    }
}

impl UsageStore for InMemoryUsageStore {
    fn frecency_boost(&self, action_id: &ActionId) -> f64 {
        let now = now_secs();
        self.stats
            .lock()
            .ok()
            .and_then(|stats| stats.get(action_id).map(|stat| stat.frecency_boost(now)))
            .unwrap_or(0.0)
    }

    fn record_use(&self, action_id: &ActionId) {
        if let Ok(mut stats) = self.stats.lock() {
            stats.entry(action_id.clone()).or_default().record(now_secs());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 记录一次使用后，同一动作立即获得非零加成。
    #[test]
    fn record_use_produces_boost() {
        let store = InMemoryUsageStore::new();
        let action = ActionId::from_static("flick.test.usage.v1.run");
        assert_eq!(store.frecency_boost(&action), 0.0);
        store.record_use(&action);
        assert!(store.frecency_boost(&action) >= 120.0);
        assert_eq!(store.stat(&action).map(|stat| stat.count), Some(1));
    }

    /// 不同动作互不影响。
    #[test]
    fn boosts_are_per_action() {
        let store = InMemoryUsageStore::new();
        let used = ActionId::from_static("flick.test.usage.v1.used");
        let other = ActionId::from_static("flick.test.usage.v1.other");
        store.record_use(&used);
        assert!(store.frecency_boost(&used) > 0.0);
        assert_eq!(store.frecency_boost(&other), 0.0);
    }
}
