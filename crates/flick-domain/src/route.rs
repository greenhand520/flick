//! Runtime 路由器返回的路由项。

use crate::id::ActionId;
use serde::{Deserialize, Serialize};

/// 路由器针对搜索查询返回的候选动作路由。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteItem {
    pub action_id: ActionId,
    pub title: String,
    /// 用于排序的分数（越高 = 匹配越好）。
    pub score: f64,
}
