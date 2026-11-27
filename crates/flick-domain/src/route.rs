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

/// 候选去重用的稳定身份。
///
/// - 普通 Action 候选以 `ActionId` 为身份；
/// - Execution 项以「父 Action + 项内稳定 id」为身份。
///
/// 这里刻意不用 `invocation_id`：`search_invoke` 每次查询都会为父 Action
/// 重新生成 `InvocationId`，用它做身份会让同一个结果项在连续两次查询里
/// 被判定为两个不同候选，累积层就无法去重。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RouteItemKey {
    /// 普通 Action 候选。
    Action(ActionId),
    /// 某父 Action 的搜索阶段执行项。
    Execution {
        /// 产出该条目的父 Action。
        parent_action_id: ActionId,
        /// 条目在父 Action 内的稳定标识。
        item_id: String,
    },
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

    /// 计算该候选的稳定身份，供累积去重使用。
    ///
    /// Execution 项缺少 `parent_action_id` 时回退到 `action_id`
    /// （`RouteItem::execution` 正常路径下二者相同）。
    pub fn key(&self) -> RouteItemKey {
        match self.kind {
            RouteItemKind::Execution => RouteItemKey::Execution {
                parent_action_id: self
                    .parent_action_id
                    .clone()
                    .unwrap_or_else(|| self.action_id.clone()),
                item_id: self.item_id.clone(),
            },
            RouteItemKind::Action => RouteItemKey::Action(self.action_id.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::ExecutionItem;

    /// Action 候选的身份就是它的 `ActionId`。
    #[test]
    fn action_item_key_is_action_id() {
        let item = RouteItem::action(
            ActionId::from_static("flick.builtin.echo.v1.echo"),
            "Echo",
            1.0,
        );
        assert_eq!(
            item.key(),
            RouteItemKey::Action(ActionId::from_static("flick.builtin.echo.v1.echo"))
        );
    }

    /// Execution 项的身份是「父 Action + 项内稳定 id」，与 invocation_id 无关。
    #[test]
    fn execution_item_key_uses_parent_and_item_id() {
        let parent = ActionId::from_static("flick.builtin.apps.search");
        let execution = ExecutionItem {
            item_id: "app:firefox".into(),
            parent_action_id: parent.clone(),
            invocation_id: crate::id::InvocationId::new(),
            title: "Firefox".into(),
            subtitle: None,
            icon: None,
            badge: None,
            output: Default::default(),
        };
        let item = RouteItem::execution(execution, 0.0);
        assert_eq!(
            item.key(),
            RouteItemKey::Execution {
                parent_action_id: parent,
                item_id: "app:firefox".into(),
            }
        );
    }

    /// 两次不同 invocation 的同一执行项必须得到相同的 key（累积去重依赖这一点）。
    #[test]
    fn same_execution_item_across_invocations_has_same_key() {
        let parent = ActionId::from_static("flick.builtin.apps.search");
        let make = || {
            let execution = ExecutionItem {
                item_id: "app:firefox".into(),
                parent_action_id: parent.clone(),
                invocation_id: crate::id::InvocationId::new(),
                title: "Firefox".into(),
                subtitle: None,
                icon: None,
                badge: None,
                output: Default::default(),
            };
            RouteItem::execution(execution, 0.0)
        };
        assert_eq!(make().key(), make().key());
    }
}
