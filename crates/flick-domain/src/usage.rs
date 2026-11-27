//! 使用频率（frecency）统计与学习接口。
//!
//! 设计文档《搜索匹配算法设计与约束》§4.6：frecency 由 Application 层注入，
//! 未启用时返回 `0`，不影响其它分数。这里只定义纯数据类型与纯公式，
//! 具体存储由上层实现。

use crate::id::ActionId;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// 单个动作的使用统计。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageStat {
    /// 累计使用次数。
    pub count: u64,
    /// 累计衰减分（每次使用前先按时间衰减再加 1）。
    pub score: f64,
    /// 最近一次使用时间（Unix 秒）。
    pub last_used_secs: Option<u64>,
}

impl UsageStat {
    /// 当前时间下的衰减分：`score * 0.5 ^ (age / 30 days)`。
    pub fn decayed_score(&self, now_secs: u64) -> f64 {
        let age_secs = now_secs.saturating_sub(self.last_used_secs.unwrap_or(now_secs)) as f64;
        self.score * 0.5f64.powf(age_secs / (30.0 * 86_400.0))
    }

    /// 记录一次使用：先衰减，再累加 1，并更新最近使用时间。
    pub fn record(&mut self, now_secs: u64) {
        let decayed = self.decayed_score(now_secs);
        self.score = decayed + 1.0;
        self.count += 1;
        self.last_used_secs = Some(now_secs);
    }

    /// 计算 frecency 加成：`min(180, round(recencyBonus + frequencyBonus))`。
    ///
    /// - `recencyBonus = 120 / (1 + hoursSinceLastUse / 4)`
    /// - `frequencyBonus = min(100, 25 * ln(1 + decayedScore))`
    pub fn frecency_boost(&self, now_secs: u64) -> f64 {
        let Some(last_used) = self.last_used_secs else {
            return 0.0;
        };
        let hours = now_secs.saturating_sub(last_used) as f64 / 3_600.0;
        let recency = 120.0 / (1.0 + hours / 4.0);
        let frequency = (25.0 * (1.0 + self.decayed_score(now_secs)).ln()).min(100.0);
        (recency + frequency).min(180.0).round()
    }
}

/// 当前 Unix 秒。
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// 使用频率存储接口；由 Application 层实现并注入 Runtime。
///
/// 未注入时 Runtime 视所有动作加成为 `0`。
pub trait UsageStore: Send + Sync {
    /// 返回某个动作当前的 frecency 加成（`0..=180`）。
    fn frecency_boost(&self, action_id: &ActionId) -> f64;
    /// 记录一次成功使用。
    fn record_use(&self, action_id: &ActionId);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 刚使用一次的动作：recency 满额 120，frequency = 25·ln(2) ≈ 17。
    #[test]
    fn fresh_use_gets_recency_plus_frequency() {
        let mut stat = UsageStat::default();
        stat.record(1_000);
        assert_eq!(stat.frecency_boost(1_000), 137.0);
    }

    /// 时间越久加成越低。
    #[test]
    fn stale_use_decays() {
        let mut stat = UsageStat::default();
        stat.record(0);
        let fresh = stat.frecency_boost(0);
        let day = stat.frecency_boost(86_400);
        let month = stat.frecency_boost(30 * 86_400);
        assert!(fresh > day, "{fresh} > {day}");
        assert!(day > month, "{day} > {month}");
    }

    /// 频繁使用会叠加频率分，但总加成封顶 180。
    #[test]
    fn frequent_use_caps_at_180() {
        let mut stat = UsageStat::default();
        for _ in 0..50 {
            stat.record(1_000);
        }
        assert_eq!(stat.count, 50);
        assert!(stat.frecency_boost(1_000) <= 180.0);
        assert_eq!(stat.frecency_boost(1_000), 180.0);
    }

    /// 从未使用过的动作加成为 0。
    #[test]
    fn unused_action_has_no_boost() {
        assert_eq!(UsageStat::default().frecency_boost(1_000), 0.0);
    }
}
