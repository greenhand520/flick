//! 路由候选。

use serde::{Deserialize, Serialize};

use crate::execution::ExecutionItem;
use crate::id::{ActionId, InvocationId};
use crate::input::InputFacetKind;

/// 候选是普通 Action 还是某 Action 的搜索阶段执行结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RouteItemKind {
    /// 普通 Action 候选。
    #[default]
    Action,
    /// 某 Action 的搜索阶段执行结果。
    Execution,
}

/// 搜索命中字段；用于记录最佳匹配来自哪一类声明字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchField {
    /// 动作标题或插件名。
    Name,
    /// 动作/插件关键词或插件标签。
    Keyword,
    /// 动作 id。
    Id,
    /// 插件描述。
    Description,
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
    /// 该候选消费的最具体输入 facet。
    #[serde(default)]
    pub facet: Option<InputFacetKind>,
    /// 最佳匹配命中的字段。
    #[serde(default)]
    pub matched_field: Option<MatchField>,
    /// 最佳匹配在字段中的字符区间，供 UI 高亮使用。
    #[serde(default)]
    pub matched_range: Option<(usize, usize)>,
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
            facet: None,
            matched_field: None,
            matched_range: None,
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
            facet: None,
            matched_field: None,
            matched_range: None,
        }
    }
}
