//! 应用协调器 —— UI 与 Runtime 之间的中央枢纽。
//!
//! `ApplicationCoordinator` 接收 UI 事件和意图，将工作委托给
//! `RuntimePort`，并通过 `HostEventSink` 发出 `HostToUi` 事件。
//! 它与运行时无关：使用 `async_trait`，不依赖任何特定的异步运行时。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use flick_domain::{
    ActionError, ActionId, ActionOutcome, ActionRequest, Caller, Deadline, HostToUi, InputContext,
    QueryGeneration, RouteItem, UiIntent, UiToHost,
};

use crate::ports::{HostEventSink, RuntimePort};
use crate::query::QueryTracker;
use crate::usage::InMemoryUsageStore;

/// 连接 UI 事件与 Flick Runtime 的中央协调器。
///
/// # 线程安全
///
/// 所有字段均为 `Send + Sync`。协调器设计为可被包装在 `Arc` 中，
/// 并在异步任务之间共享。
pub struct ApplicationCoordinator {
    runtime: Arc<dyn RuntimePort>,
    sink: Arc<dyn HostEventSink>,
    query_tracker: Arc<QueryTracker>,
    /// 最近一次发出的路由快照（同步锁，临界区内不做 `.await`）。
    routes: Mutex<RouteSnapshot>,
    panel_visible: AtomicBool,
    /// 使用频率存储（frecency）；与注入 Runtime 的是同一份。
    usage: Arc<dyn flick_domain::UsageStore>,
}

/// 本面板会话内已发出的候选并集，用于校验 UI 回传的候选身份。
///
/// 设计文档 §12.2-3 要求 `Invoke` 携带 `generation + item_id`。由于手动输入
/// 会把多代候选累积展示，校验必须覆盖整个会话内的并集，而非仅最新一批；
/// 面板会话结束（`SearchPanelClosed`）时清空。
#[derive(Debug, Default)]
struct RouteSnapshot {
    /// 已发出过的全部候选（按 `(item_id, action_id)` 去重）。
    items: Vec<RouteItem>,
    /// 见过的最大代次，用于拒绝 UI 不可能持有的「未来代次」。
    latest_generation: Option<QueryGeneration>,
}

impl RouteSnapshot {
    /// 并入一次已发出的路由结果。
    fn record(&mut self, generation: QueryGeneration, items: &[RouteItem]) {
        self.latest_generation = Some(match self.latest_generation {
            Some(latest) if latest >= generation => latest,
            _ => generation,
        });
        for item in items {
            let known = self.items.iter().any(|existing| {
                existing.item_id == item.item_id && existing.action_id == item.action_id
            });
            if !known {
                self.items.push(item.clone());
            }
        }
    }

    /// 候选是否属于本会话已发出的集合；未来代次一律拒绝。
    fn contains(&self, generation: QueryGeneration, item_id: &str, action_id: &ActionId) -> bool {
        if self
            .latest_generation
            .is_some_and(|latest| generation > latest)
        {
            return false;
        }
        self.items
            .iter()
            .any(|item| item.item_id == item_id && &item.action_id == action_id)
    }

    /// 面板会话结束时清空。
    fn clear(&mut self) {
        self.items.clear();
        self.latest_generation = None;
    }
}

impl ApplicationCoordinator {
    /// 使用给定的运行时和事件接收器创建一个新的协调器。
    pub fn new(runtime: Arc<dyn RuntimePort>, sink: Arc<dyn HostEventSink>) -> Self {
        let usage: Arc<dyn flick_domain::UsageStore> = Arc::new(InMemoryUsageStore::new());
        // 让 Runtime 的检索也能读到同一份 frecency 数据。
        runtime.set_usage_store(Arc::clone(&usage));
        Self {
            runtime,
            sink,
            query_tracker: Arc::new(QueryTracker::new()),
            routes: Mutex::new(RouteSnapshot::default()),
            panel_visible: AtomicBool::new(false),
            usage,
        }
    }

    /// 返回使用频率存储（测试与诊断用）。
    pub fn usage_store(&self) -> Arc<dyn flick_domain::UsageStore> {
        Arc::clone(&self.usage)
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

                // 阶段一：纯内存路由，立刻出首屏。
                let action_items = self.runtime.route_input(&context).await;
                if !self.query_tracker.is_current(generation) {
                    // 有更新的查询取代了此查询 —— 静默丢弃。
                    return;
                }
                // 留下快照供后续 `Invoke` 做候选身份校验（§12.2-3）。
                // 快照只记录 action candidates：执行项走 `ExecutionActivated` 分支。
                if let Ok(mut snapshot) = self.routes.lock() {
                    snapshot.record(generation, &action_items);
                }
                self.sink.send(HostToUi::Routes {
                    generation,
                    items: action_items,
                });

                // 阶段二：搜索阶段执行放到后台任务，让更新的代次能立刻取消在途搜索
                // （取消令牌由 runtime 内部管理，协调器只负责传代次）。
                let runtime = Arc::clone(&self.runtime);
                let sink = Arc::clone(&self.sink);
                let tracker = Arc::clone(&self.query_tracker);
                tokio::spawn(async move {
                    let execution_items = runtime.search_invoke(generation, action_input).await;
                    if execution_items.is_empty() || !tracker.is_current(generation) {
                        return;
                    }
                    sink.send(HostToUi::ExecutionResults {
                        generation,
                        items: execution_items,
                    });
                });
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
                    .execution_event(parent_action_id.clone(), &item_id, event)
                    .await
                {
                    self.record_usage_on_success(&parent_action_id, &outcome);
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
                    action_id: action_id.clone(),
                    input,
                    caller: Caller::ui("search-panel"),
                    source: flick_domain::ActionSource::Ui,
                    deadline: Deadline::from_secs(30),
                };
                let outcome = self.runtime.invoke(request).await;
                self.record_usage_on_success(&action_id, &outcome);
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
                // 面板会话结束：清空候选并集，避免下一会话误接受上一会话的候选。
                if let Ok(mut snapshot) = self.routes.lock() {
                    snapshot.clear();
                }
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
            // 主题持久化由宿主外壳在投递到协调器之前拦截；协调器不关心主题。
            UiToHost::SetTheme { .. } => {}
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

    /// 调用成功时记录一次使用（frecency）；`Error` 结果不计入。
    fn record_usage_on_success(&self, action_id: &ActionId, outcome: &ActionOutcome) {
        if !matches!(outcome, ActionOutcome::Error(_)) {
            self.usage.record_use(action_id);
        }
    }

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
