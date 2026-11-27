//! 搜索结果累积。
//!
//! 本模块只做纯数据合并，不接触 UI 或 Runtime：`PanelState` 在收到
//! `HostToUi::Routes` 时调用 [`merge_batch`]，把新批次并入会话累积。

use flick_domain::{RouteItem, UiInputKind};

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

/// 一次搜索面板会话的累积状态。
///
/// 生命周期与 `PanelState` 的可见性对齐：面板显示时 [`Self::begin`]，
/// 隐藏/关闭时 [`Self::end`]。只有手动输入（`InputSource::Manual`）
/// 的批次才累积；粘贴、拖拽、剪贴板图片只替换。
#[derive(Debug, Clone)]
pub struct SearchSession {
    /// 已累积的结果（旧 → 新）。
    accumulated: Vec<RouteItem>,
    /// 本次会话是否处于手动累积模式。
    manual: bool,
    /// 会话当前的输入种类；种类变化会重置累积。
    kind: UiInputKind,
    /// 会话是否有效；隐藏/关闭之后失效，拒绝迟到的批次。
    active: bool,
}

impl SearchSession {
    /// 创建一个未激活的空会话。
    pub fn new() -> Self {
        Self {
            accumulated: Vec::new(),
            manual: false,
            kind: UiInputKind::Text,
            active: false,
        }
    }

    /// 开始一个新会话；已激活时只更新模式与种类。
    pub fn begin(&mut self, kind: UiInputKind, manual: bool) {
        if self.active {
            self.note_kind(kind, manual);
            return;
        }
        self.accumulated.clear();
        self.kind = kind;
        self.manual = manual;
        self.active = true;
    }

    /// 结束会话并清空累积。
    pub fn end(&mut self) {
        self.accumulated.clear();
        self.manual = false;
        self.active = false;
    }

    /// 只清空累积，保持会话有效（输入被清空时使用）。
    pub fn clear(&mut self) {
        self.accumulated.clear();
    }

    /// 会话是否有效。
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// 会话是否处于手动累积模式；失效会话永远返回 `false`。
    pub fn is_manual(&self) -> bool {
        self.active && self.manual
    }

    /// 通知当前输入种类与来源；种类或模式变化时清空累积。
    ///
    /// 对应设计文档 §6.1 的重置条件 3：输入类型在
    /// `Text` ↔ `Files`/`ClipboardImage` 之间切换即重置。
    pub fn note_kind(&mut self, kind: UiInputKind, manual: bool) {
        if self.kind != kind || self.manual != manual {
            self.accumulated.clear();
            self.kind = kind;
            self.manual = manual;
        }
    }

    /// 并入一批新结果并返回当前累积视图。
    ///
    /// 手动模式走累积合并；非手动模式直接替换（§6.1「对非手动输入不累积」）。
    pub fn merge(&mut self, batch: Vec<RouteItem>) -> Vec<RouteItem> {
        if !self.is_manual() {
            self.accumulated = batch;
            return self.accumulated.clone();
        }
        merge_batch(&mut self.accumulated, batch);
        self.accumulated.clone()
    }

    /// 当前累积结果。
    pub fn items(&self) -> &[RouteItem] {
        &self.accumulated
    }
}

impl Default for SearchSession {
    fn default() -> Self {
        Self::new()
    }
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

    /// 会话开始后处于有效状态，`end` 之后失效且累积被清空。
    #[test]
    fn session_lifecycle_begin_and_end() {
        let mut session = SearchSession::new();
        assert!(!session.is_active());
        session.begin(UiInputKind::Text, true);
        assert!(session.is_active());
        assert!(session.is_manual());

        session.merge(vec![action("a.v1.one", "A", 1.0)]);
        assert_eq!(session.items().len(), 1);

        session.end();
        assert!(!session.is_active());
        assert!(!session.is_manual());
        assert!(session.items().is_empty());
    }

    /// 手动模式：新批次与旧批次合并。
    #[test]
    fn manual_session_merges_batches() {
        let mut session = SearchSession::new();
        session.begin(UiInputKind::Text, true);
        session.merge(vec![action("a.v1.one", "A", 1.0)]);
        let merged = session.merge(vec![action("a.v1.two", "B", 1.0)]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].title, "A");
        assert_eq!(merged[1].title, "B");
    }

    /// 非手动模式：新批次替换旧批次。
    #[test]
    fn non_manual_session_replaces_batches() {
        let mut session = SearchSession::new();
        session.begin(UiInputKind::Text, true);
        session.merge(vec![action("a.v1.one", "A", 1.0)]);
        session.begin(UiInputKind::Files, false);
        let replaced = session.merge(vec![action("a.v1.two", "B", 1.0)]);
        assert_eq!(replaced.len(), 1);
        assert_eq!(replaced[0].title, "B");
    }

    /// 输入种类切换会清空累积（Text → Files，或反向）。
    #[test]
    fn kind_switch_clears_accumulation() {
        let mut session = SearchSession::new();
        session.begin(UiInputKind::Text, true);
        session.merge(vec![action("a.v1.one", "A", 1.0)]);
        assert_eq!(session.items().len(), 1);

        session.note_kind(UiInputKind::Files, false);
        assert!(session.items().is_empty());

        session.note_kind(UiInputKind::Files, false);
        assert!(session.items().is_empty());
    }

    /// `clear` 只清累积，不结束会话。
    #[test]
    fn clear_keeps_session_active() {
        let mut session = SearchSession::new();
        session.begin(UiInputKind::Text, true);
        session.merge(vec![action("a.v1.one", "A", 1.0)]);
        session.clear();
        assert!(session.is_active());
        assert!(session.items().is_empty());
    }
}
