//! 路由候选。

use serde::{Deserialize, Serialize};

use crate::execution::ExecutionItem;
use crate::id::{ActionId, InvocationId};

/// 候选是普通 Action 还是某 Action 的搜索阶段执行结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RouteItemKind {
    /// 普通 Action 候选。
    #[default]
    Action,
    /// 某 Action 的搜索阶段执行结果。
    Execution,
}

/// Host 返回的扁平候选。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteItem {
    /// 候选对应的 Action。
    pub action_id: ActionId,
    /// 展示标题。
    pub title: String,
    /// 用于排序的分数（越高 = 匹配越好）。
    pub score: f64,
    /// 候选种类：普通 Action 或搜索阶段执行结果。
    #[serde(default)]
    pub kind: RouteItemKind,
    /// 候选唯一标识；Action 候选为 `action:<action_id>`。
    #[serde(default)]
    pub item_id: String,
    /// 次级说明。
    #[serde(default)]
    pub subtitle: Option<String>,
    /// 执行结果所属的父 Action。
    #[serde(default)]
    pub parent_action_id: Option<ActionId>,
    /// 父 Action 的调用标识。
    #[serde(default)]
    pub invocation_id: Option<InvocationId>,
    /// 完整执行项；仅 `kind == Execution` 时存在。
    #[serde(default)]
    pub execution: Option<ExecutionItem>,
}

impl RouteItem {
    /// 普通 Action 候选；`item_id` 固定为 `action:<action_id>`。
    pub fn action(action_id: ActionId, title: impl Into<String>, score: f64) -> Self {
        let item_id = format!("action:{action_id}");
        Self {
            action_id,
            title: title.into(),
            score,
            kind: RouteItemKind::Action,
            item_id,
            subtitle: None,
            parent_action_id: None,
            invocation_id: None,
            execution: None,
        }
    }

    /// 父 Action 的搜索阶段执行结果。
    pub fn execution(item: ExecutionItem, score: f64) -> Self {
        Self {
            action_id: item.parent_action_id.clone(),
            title: item.title.clone(),
            score,
            kind: RouteItemKind::Execution,
            item_id: item.item_id.clone(),
            subtitle: item.subtitle.clone(),
            parent_action_id: Some(item.parent_action_id.clone()),
            invocation_id: Some(item.invocation_id),
            execution: Some(item),
        }
    }
}
