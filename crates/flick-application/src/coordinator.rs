//! 应用协调器 —— UI 与 Runtime 之间的中央枢纽。
//!
//! `ApplicationCoordinator` 接收 UI 事件和意图，将工作委托给
//! `RuntimePort`，并通过 `HostEventSink` 发出 `HostToUi` 事件。
//! 它与运行时无关：使用 `async_trait`，不依赖任何特定的异步运行时。

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use flick_domain::{
    ActionError, ActionId, ActionOutcome, ActionRequest, Caller, Deadline, HostToUi, InputContext,
    QueryGeneration, RouteItem, RouteItemKind, UiIntent, UiToHost,
};

use crate::ports::{HostEventSink, RuntimePort};
use crate::query::QueryTracker;

/// 连接 UI 事件与 Flick Runtime 的中央协调器。
///
/// # 线程安全
///
/// 所有字段均为 `Send + Sync`。协调器设计为可被包装在 `Arc` 中，
/// 并在异步任务之间共享。
pub struct ApplicationCoordinator {
    runtime: Arc<dyn RuntimePort>,
    sink: Arc<dyn HostEventSink>,
    query_tracker: QueryTracker,
    /// 最近一次发出的路由快照（同步锁，临界区内不做 `.await`）。
    routes: Mutex<RouteSnapshot>,
    panel_visible: AtomicBool,
}

/// 合并普通 Action 候选与搜索阶段 Execution 项。
///
/// 同一父 Action 的 Execution 项紧跟在该 Action 之后；若父 Action 本身
/// 没有命中关键词（例如只靠 `search_invocable` 产出的结果），则追加在末尾。
/// todo: 这个合并逻辑是否可以放到runtime？另外这2种结果真的会重复吗？
fn merge_search_results(
    action_items: Vec<RouteItem>,
    execution_items: Vec<RouteItem>,
) -> Vec<RouteItem> {
    let mut remaining: Vec<Option<RouteItem>> = execution_items.into_iter().map(Some).collect();
    let mut merged = Vec::with_capacity(action_items.len() + remaining.len());

    for action in action_items {
        let parent_id = action.action_id.clone();
        merged.push(action);
        for slot in &mut remaining {
            let is_child = slot.as_ref().is_some_and(|item| {
                item.kind == RouteItemKind::Execution
                    && item.parent_action_id.as_ref() == Some(&parent_id)
            });
            if is_child {
                if let Some(item) = slot.take() {
                    merged.push(item);
                }
            }
        }
    }

    merged.extend(remaining.into_iter().flatten());
    merged
}

/// 最近一次发给 UI 的路由快照，用于校验 UI 回传的候选身份。
///
/// 设计文档 §12.2-3：`Invoke` 必须携带 `generation + item_id`，
/// 宿主只接受属于当前代次、且仍在最近一次结果集中的候选。
#[derive(Debug, Default)]
struct RouteSnapshot {
    /// 快照所属的查询代次；`None` 表示尚未发出任何结果。
    generation: Option<QueryGeneration>,
    /// 该代次的候选列表。
    items: Vec<RouteItem>,
}

impl RouteSnapshot {
    /// 记录一次已发出的路由结果。
    fn record(&mut self, generation: QueryGeneration, items: &[RouteItem]) {
        self.generation = Some(generation);
        self.items = items.to_vec();
    }

    /// 候选是否属于当前代次且仍在结果集中。
    fn contains(&self, generation: QueryGeneration, item_id: &str, action_id: &ActionId) -> bool {
        self.generation == Some(generation)
            && self
                .items
                .iter()
                .any(|item| item.item_id == item_id && &item.action_id == action_id)
    }
}

impl ApplicationCoordinator {
    /// 使用给定的运行时和事件接收器创建一个新的协调器。
    pub fn new(runtime: Arc<dyn RuntimePort>, sink: Arc<dyn HostEventSink>) -> Self {
        Self {
            runtime,
            sink,
            query_tracker: QueryTracker::new(),
            routes: Mutex::new(RouteSnapshot::default()),
            panel_visible: AtomicBool::new(false),
        }
    }

    // ── UI → Host 消息处理 ────────────────────────────────────────

    /// 处理来自 UI 的消息。
    ///
    /// `UiToHost` 的每个变体都会被分派到相应的
    /// 运行时 / 接收器调用。
    pub async fn handle_ui(&self, event: UiToHost) {
        match event {
            UiToHost::QueryChanged { generation, input } => {
                // 在调用运行时之前记录最新的代次。
                self.query_tracker.set_current(generation);

                let action_input = input.to_action_input();
                let context = InputContext::from_snapshot(generation, &input);
                // 首屏路由（纯内存）与搜索阶段执行（最多 150ms）并行进行；
                // 后者的结果作为 Execution 项排在各自父 Action 之后。
                let (action_items, execution_items) = tokio::join!(
                    self.runtime.route_input(&context),
                    self.runtime
                        .search_invoke(&action_input.text, action_input.kind),
                );
                let items = merge_search_results(action_items, execution_items);

                // 仅当此代次仍为最新时才发出结果，并留下快照供
                // 后续 `Invoke` 做候选身份校验。
                if self.query_tracker.is_current(generation) {
                    if let Ok(mut snapshot) = self.routes.lock() {
                        snapshot.record(generation, &items);
                    }
                    self.sink.send(HostToUi::Routes { generation, items });
                }
                // 否则：有更新的查询取代了此查询 —— 静默丢弃。
            }

            UiToHost::ExecutionActivated {
                generation,
                parent_action_id,
                invocation_id,
                item_id,
                event,
            } => {
                // 旧代次的 Execution 项已被新搜索替换，激活事件直接丢弃。
                if !self.query_tracker.is_current(generation) {
                    return;
                }
                let item_id = item_id.unwrap_or_default();
                if let Some(outcome) = self
                    .runtime
                    .execution_event(parent_action_id, &item_id, event)
                    .await
                {
                    self.sink.send(HostToUi::InvocationCompleted {
                        invocation_id,
                        outcome,
                    });
                }
                // `None` 表示父 Action 未实现该事件（no-op），无需回送错误。
            }

            UiToHost::Invoke {
                generation,
                item_id,
                invocation_id,
                action_id,
                input,
            } => {
                // 候选身份校验：拒绝陈旧代次与不在结果集中的候选。
                // 拒绝也要回 `InvocationCompleted`，否则 UI 会一直停在
                // 「调用中」状态（`active_invocation` 不会被清掉）。
                let known = self
                    .routes
                    .lock()
                    .map(|snapshot| snapshot.contains(generation, &item_id, &action_id))
                    .unwrap_or(false);
                if !known {
                    self.sink.send(HostToUi::InvocationCompleted {
                        invocation_id,
                        outcome: ActionOutcome::Error(ActionError::UnknownAction {
                            action_id: action_id.to_string(),
                        }),
                    });
                    return;
                }

                let request = ActionRequest {
                    invocation_id,
                    action_id,
                    input,
                    caller: Caller::ui("search-panel"),
                    source: flick_domain::ActionSource::Ui,
                    deadline: Deadline::from_secs(30),
                };
                let outcome = self.runtime.invoke(request).await;
                self.sink.send(HostToUi::InvocationCompleted {
                    invocation_id,
                    outcome,
                });
            }

            UiToHost::Cancel { invocation_id } => {
                self.runtime.cancel(invocation_id).await;
            }

            UiToHost::SearchPanelClosed => {
                // 记录面板不再可见。
                self.panel_visible.store(false, Ordering::SeqCst);
            }

            // Surface 事件转发给运行时；结果作为 SurfaceUpdated 回送 UI，
            // 失败则回送 SurfaceFailed（保留 UI 既有内容）。
            UiToHost::SurfaceEvent { session_id, kind } => {
                match self.runtime.surface_event(session_id, kind).await {
                    Ok(content) => {
                        self.sink.send(HostToUi::SurfaceUpdated {
                            session_id,
                            content,
                        });
                    }
                    Err(message) => {
                        self.sink.send(HostToUi::SurfaceFailed {
                            session_id,
                            message,
                        });
                    }
                }
            }
            // Surface 关闭：通知运行时清理会话，无需回送 UI。
            UiToHost::SurfaceClosed { session_id } => {
                self.runtime.surface_closed(session_id).await;
            }
        }
    }

    // ── 意图处理（来自 Portal / 外部触发器） ─────────────────

    /// 处理来自 Portal 或其他外部触发器的 `UiIntent`。
    ///
    /// 意图**不是**动作调用。`ShowSearchPanel` 仅发出
    /// 一个 `HostToUi::ShowSearchPanel` 事件 —— 它**不会**调用
    /// `runtime.invoke`。
    pub fn handle_intent(&self, intent: UiIntent) {
        match intent {
            UiIntent::ShowSearchPanel { activation_token } => {
                self.panel_visible.store(true, Ordering::SeqCst);
                self.sink
                    .send(HostToUi::ShowSearchPanel { activation_token });
            }
            UiIntent::HideSearchPanel => {
                self.panel_visible.store(false, Ordering::SeqCst);
                self.sink.send(HostToUi::HideSearchPanel);
            }
            UiIntent::ToggleSearchPanel => {
                // 快捷键切换：可见则收起、否则唤出。
                self.toggle_search_panel();
            }
        }
    }

    // ── 面板切换 ──────────────────────────────────────────────────────

    /// 在显示与隐藏状态之间切换搜索面板。
    ///
    /// 这是快捷键入口：它会翻转当前的可见状态，并发出相应的
    /// `HostToUi` 事件。它**不会**与运行时交互。
    pub fn toggle_search_panel(&self) {
        let was_visible = self.panel_visible.fetch_xor(true, Ordering::SeqCst);
        if was_visible {
            self.sink.send(HostToUi::HideSearchPanel);
        } else {
            self.sink.send(HostToUi::ShowSearchPanel {
                activation_token: None,
            });
        }
    }
}
