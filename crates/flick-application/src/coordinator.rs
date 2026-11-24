//! 应用协调器 —— UI 与 Runtime 之间的中央枢纽。
//!
//! `ApplicationCoordinator` 接收 UI 事件和意图，将工作委托给
//! `RuntimePort`，并通过 `HostEventSink` 发出 `HostToUi` 事件。
//! 它与运行时无关：使用 `async_trait`，不依赖任何特定的异步运行时。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use flick_domain::{ActionRequest, Caller, Deadline, HostToUi, InputKind, UiIntent, UiToHost};

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
    panel_visible: AtomicBool,
}

impl ApplicationCoordinator {
    /// 使用给定的运行时和事件接收器创建一个新的协调器。
    pub fn new(runtime: Arc<dyn RuntimePort>, sink: Arc<dyn HostEventSink>) -> Self {
        Self {
            runtime,
            sink,
            query_tracker: QueryTracker::new(),
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
            UiToHost::QueryChanged { generation, text } => {
                // 在调用运行时之前记录最新的代次。
                self.query_tracker.set_current(generation);

                let items = self.runtime.route(&text, InputKind::PlainText).await;

                // 仅当此代次仍为最新时才发出结果。
                if self.query_tracker.is_current(generation) {
                    self.sink.send(HostToUi::Routes { generation, items });
                }
                // 否则：有更新的查询取代了此查询 —— 静默丢弃。
            }

            UiToHost::Invoke {
                invocation_id,
                action_id,
                input,
            } => {
                let request = ActionRequest {
                    invocation_id,
                    action_id,
                    input,
                    caller: Caller::ui("search-panel"),
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
