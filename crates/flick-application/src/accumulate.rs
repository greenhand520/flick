//! 搜索结果累积。
//!
//! 本模块只做纯数据合并，不接触 UI 或 Runtime：`PanelState` 在收到
//! `HostToUi::Routes` 时调用 [`merge_batch`]，把新批次并入会话累积。

use flick_domain::{RouteItem, RouteItemKey};

/// 累积结果上限；超出时按分数保留高分项（设计文档 §9 不变量 8）。
pub const MAX_ACCUMULATED_RESULTS: usize = 200;

/// 把一批新结果并入已有累积。
///
/// 规则（设计文档 §6.1）：
/// 1. 以 [`RouteItemKey`] 去重；
/// 2. 旧项保持原位置与顺序，新项追加在末尾；
/// 3. 同一 key 再次出现时保留分数更高者，但不改变它在列表中的位置；
/// 4. 合并后超过 [`MAX_ACCUMULATED_RESULTS`] 时按分数保留前 N 项，
///    并恢复这些项的原始相对顺序。
pub fn merge_batch(accumulated: &mut Vec<RouteItem>, batch: Vec<RouteItem>) {
    for item in batch {
        let key = item.key();
        match accumulated
            .iter()
            .position(|existing| existing.key() == key)
        {
            Some(index) => {
                // 同 key：只有更高分才替换内容，位置保持不变。
                if item.score > accumulated[index].score {
                    accumulated[index] = item;
                }
            }
            None => accumulated.push(item),
        }
    }
    truncate_by_score(accumulated);
}

/// 超过上限时按分数保留前 `MAX_ACCUMULATED_RESULTS` 项，并恢复原始相对顺序。
fn truncate_by_score(items: &mut Vec<RouteItem>) {
    if items.len() <= MAX_ACCUMULATED_RESULTS {
        return;
    }
    let mut order: Vec<usize> = (0..items.len()).collect();
    // 分数降序；`total_cmp` 让 NaN 也有确定顺序，`sort_by` 是稳定排序。
    order.sort_by(|a, b| items[*b].score.total_cmp(&items[*a].score));
    order.truncate(MAX_ACCUMULATED_RESULTS);
    // 恢复这些幸存项的原始相对顺序。
    order.sort_unstable();
    *items = order.into_iter().map(|index| items[index].clone()).collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use flick_domain::{ActionId, RouteItemKind};

    /// 构造一个普通 Action 候选。
    fn action(id: &'static str, title: &str, score: f64) -> RouteItem {
        RouteItem::action(ActionId::from_static(id), title, score)
    }

    /// 新批次追加在旧项之后，旧项顺序不变。
    #[test]
    fn new_batch_appends_after_existing_items() {
        let mut accumulated = vec![action("a.v1.one", "A", 1.0)];
        merge_batch(
            &mut accumulated,
            vec![action("a.v1.two", "B", 2.0), action("a.v1.three", "C", 0.5)],
        );
        let titles: Vec<&str> = accumulated.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, vec!["A", "B", "C"]);
    }

    /// 同一 Action 在旧/新批次重复出现时只保留一条，位置不变。
    #[test]
    fn repeated_action_is_deduplicated_in_place() {
        let mut accumulated = vec![action("a.v1.one", "A", 1.0), action("a.v1.two", "B", 2.0)];
        merge_batch(&mut accumulated, vec![action("a.v1.one", "A again", 3.0)]);
        assert_eq!(accumulated.len(), 2);
        assert_eq!(accumulated[0].title, "A again");
        assert_eq!(accumulated[0].score, 3.0);
        assert_eq!(accumulated[1].title, "B");
    }

    /// 同 key 但新分更低时保留旧项（不降级）。
    #[test]
    fn lower_score_duplicate_does_not_replace() {
        let mut accumulated = vec![action("a.v1.one", "A", 5.0)];
        merge_batch(&mut accumulated, vec![action("a.v1.one", "A weak", 1.0)]);
        assert_eq!(accumulated.len(), 1);
        assert_eq!(accumulated[0].title, "A");
        assert_eq!(accumulated[0].score, 5.0);
    }

    /// 超过上限时按分数保留前 N 项，并恢复原始相对顺序。
    #[test]
    fn over_limit_keeps_highest_scores_in_original_order() {
        let mut accumulated: Vec<RouteItem> = (0..MAX_ACCUMULATED_RESULTS)
            .map(|i| {
                // 分数 = 序号，序号越大分越高。
                RouteItem::action(
                    ActionId::new(format!("a.v1.item{i}")).unwrap(),
                    format!("item{i}"),
                    i as f64,
                )
            })
            .collect();
        // 追加 10 个高分项，应挤掉序号最小的 10 个。
        let batch: Vec<RouteItem> = (0..10)
            .map(|i| {
                RouteItem::action(
                    ActionId::new(format!("a.v1.extra{i}")).unwrap(),
                    format!("extra{i}"),
                    1000.0 + i as f64,
                )
            })
            .collect();
        merge_batch(&mut accumulated, batch);

        assert_eq!(accumulated.len(), MAX_ACCUMULATED_RESULTS);
        // 序号 0..10 被挤掉。
        assert_eq!(accumulated[0].title, "item10");
        // 高分新项按追加顺序排在末尾。
        assert_eq!(accumulated[MAX_ACCUMULATED_RESULTS - 1].title, "extra9");
    }

    /// Execution 项按「父 Action + item_id」去重，与 invocation 无关。
    #[test]
    fn execution_items_deduplicate_by_parent_and_item_id() {
        let parent = ActionId::from_static("flick.builtin.apps.search");
        let make = |title: &str| {
            let execution = flick_domain::ExecutionItem {
                item_id: "app:firefox".into(),
                parent_action_id: parent.clone(),
                invocation_id: flick_domain::InvocationId::new(),
                title: title.into(),
                subtitle: None,
                icon: None,
                badge: None,
                output: Default::default(),
            };
            RouteItem::execution(execution, 0.0)
        };
        let mut accumulated = vec![make("Firefox")];
        merge_batch(&mut accumulated, vec![make("Firefox")]);
        assert_eq!(accumulated.len(), 1);
        assert_eq!(accumulated[0].kind, RouteItemKind::Execution);
    }
}
