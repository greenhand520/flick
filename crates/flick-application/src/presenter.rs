//! 搜索面板状态机（presenter）。
//!
//! 本模块持有逻辑面板状态，与原生 UI 窗口无关，也不依赖任何 GUI 框架
//! （只用 `flick-domain` 的协议类型）。各 UI 外壳（gpui / iced 等）应直接复用
//! 这里的 `PanelState`/`PanelEvent`/`PanelAction`，从而做到「面板交互逻辑只有一份、
//! 且能在无 GUI 下单元测试」。
//!
//! 关闭原生窗口会隐藏面板，但会在一段短暂且有界的恢复期内
//! 保留其逻辑会话。

use flick_domain::*;
use std::time::{Duration, Instant};

/// 隐藏的面板在此持续时间内保留其逻辑会话。
pub const PANEL_RESUME_WINDOW: Duration = Duration::from_secs(10);

/// [`PanelState`] 接受的事件。
#[derive(Debug, Clone)]
pub enum PanelEvent {
    Show {
        activation_token: Option<String>,
    },
    Hide,
    InputChanged(String),
    /// 针对某个输入 revision 的 UI 端防抖计时器已到期。
    DebounceFlush {
        revision: u64,
    },
    Submitted,
    EscapePressed,
    SelectionUp,
    SelectionDown,
    WindowClosed,
    Routes {
        generation: QueryGeneration,
        items: Vec<RouteItem>,
    },
    InvocationCompleted {
        invocation_id: InvocationId,
        outcome: ActionOutcome,
    },
    RuntimeStatus(RuntimeStatus),
}

/// [`PanelState`] 发出的出站命令。
#[derive(Debug, Clone, PartialEq)]
pub enum PanelAction {
    SendToHost(UiToHost),
    StartDebounce { revision: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionState {
    pub index: usize,
    pub total: usize,
}

impl SelectionState {
    pub fn new() -> Self {
        Self { index: 0, total: 0 }
    }

    pub fn set_total(&mut self, total: usize) {
        self.total = total;
        if total == 0 {
            self.index = 0;
        } else if self.index >= total {
            self.index = total - 1;
        }
    }

    pub fn move_up(&mut self) {
        if self.total > 0 && self.index > 0 {
            self.index -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.total > 0 && self.index + 1 < self.total {
            self.index += 1;
        }
    }

    pub fn selected<'a>(&self, routes: &'a [RouteItem]) -> Option<&'a RouteItem> {
        (self.total > 0).then(|| routes.get(self.index)).flatten()
    }
}

impl Default for SelectionState {
    fn default() -> Self {
        Self::new()
    }
}

/// 一次搜索面板会话的逻辑状态。
#[derive(Debug)]
pub struct PanelState {
    pub visible: bool,
    pub created: bool,
    /// 实时的、受控文本输入值。
    pub query: String,
    /// Phase 0 期间保留的兼容性字段；始终镜像 [`Self::query`]。
    pub pending_query: String,
    pub generation: QueryGeneration,
    pub routes: Vec<RouteItem>,
    pub selection: SelectionState,
    pub active_invocation: Option<InvocationId>,
    pub last_outcome: Option<ActionOutcome>,
    pub runtime_status: RuntimeStatus,
    pub activation_token: Option<String>,
    pub activation_token_applied: bool,
    debounce_revision: u64,
    flushed_debounce_revision: Option<u64>,
    hidden_at: Option<Instant>,
}

impl PanelState {
    pub fn new() -> Self {
        Self {
            visible: false,
            created: false,
            query: String::new(),
            pending_query: String::new(),
            generation: QueryGeneration::INITIAL,
            routes: Vec::new(),
            selection: SelectionState::new(),
            active_invocation: None,
            last_outcome: None,
            runtime_status: RuntimeStatus::Running,
            activation_token: None,
            activation_token_applied: false,
            debounce_revision: 0,
            flushed_debounce_revision: None,
            hidden_at: None,
        }
    }

    /// 当前的输入防抖 revision，供 UI 任务接线和测试使用。
    pub fn debounce_revision(&self) -> u64 {
        self.debounce_revision
    }

    /// 使用当前单调时间应用一个事件。
    ///
    /// 生产环境中的调用方通常使用此方法。[`Self::update_at`] 让
    /// 会话过期边界在测试和嵌入代码中保持确定性。
    pub fn update(&mut self, event: PanelEvent) -> Vec<PanelAction> {
        self.update_at(event, Instant::now())
    }

    /// 在调用方提供的单调时间戳处应用一个事件。
    pub fn update_at(&mut self, event: PanelEvent, now: Instant) -> Vec<PanelAction> {
        match event {
            PanelEvent::Show { activation_token } => self.on_show(activation_token, now),
            PanelEvent::Hide | PanelEvent::WindowClosed | PanelEvent::EscapePressed => {
                self.on_hide(now)
            }
            PanelEvent::InputChanged(text) => self.on_input_changed(text),
            PanelEvent::DebounceFlush { revision } => self.on_debounce_flush(revision),
            PanelEvent::Submitted => self.on_submitted(),
            PanelEvent::SelectionUp => {
                self.selection.move_up();
                vec![]
            }
            PanelEvent::SelectionDown => {
                self.selection.move_down();
                vec![]
            }
            PanelEvent::Routes { generation, items } => self.on_routes(generation, items),
            PanelEvent::InvocationCompleted {
                invocation_id,
                outcome,
            } => self.on_invocation_completed(invocation_id, outcome),
            PanelEvent::RuntimeStatus(status) => {
                self.runtime_status = status;
                vec![]
            }
        }
    }

    fn on_show(&mut self, activation_token: Option<String>, now: Instant) -> Vec<PanelAction> {
        if self
            .hidden_at
            .is_some_and(|hidden| now.saturating_duration_since(hidden) > PANEL_RESUME_WINDOW)
        {
            self.reset_expired_session();
        }

        self.created = true;
        self.visible = true;
        self.hidden_at = None;
        self.activation_token = activation_token;
        self.activation_token_applied = false;
        vec![]
    }

    fn on_hide(&mut self, now: Instant) -> Vec<PanelAction> {
        // 焦点丢失之后可能紧接着合成器的关闭请求。
        // 第一个事件负责该状态转换，并且只向宿主报告一次关闭。
        if !self.visible {
            return vec![];
        }

        self.visible = false;
        self.hidden_at = Some(now);
        // 尚未完成的 UI 防抖任务无法取消。使它们的 revision
        // 失效可让任何迟到的完成操作变得无害。
        self.debounce_revision = self.debounce_revision.wrapping_add(1);
        self.flushed_debounce_revision = None;
        vec![PanelAction::SendToHost(UiToHost::SearchPanelClosed)]
    }

    fn on_input_changed(&mut self, text: String) -> Vec<PanelAction> {
        // `query` 是受控的 UI TextInput 值。它必须在与用户输入
        // 同一个事件轮次中更新，这样光标编辑和长时间按住 Backspace
        // 重复输入都不会观察到过期的模型文本。
        self.query = text;
        self.pending_query.clone_from(&self.query);
        // QueryGeneration 用于防范异步的运行时结果；下方的本地 revision
        // 则独立地防范 UI 防抖任务。
        self.generation = self.generation.next();
        self.debounce_revision = self.debounce_revision.wrapping_add(1);
        self.flushed_debounce_revision = None;
        self.routes.clear();
        self.selection.set_total(0);
        self.last_outcome = None;
        self.active_invocation = None;

        vec![PanelAction::StartDebounce {
            revision: self.debounce_revision,
        }]
    }

    fn on_debounce_flush(&mut self, revision: u64) -> Vec<PanelAction> {
        if revision != self.debounce_revision
            || self.flushed_debounce_revision == Some(revision)
            || !self.visible
        {
            return vec![];
        }

        self.flushed_debounce_revision = Some(revision);
        vec![PanelAction::SendToHost(UiToHost::QueryChanged {
            generation: self.generation,
            text: self.query.clone(),
        })]
    }

    fn on_submitted(&mut self) -> Vec<PanelAction> {
        if self.query.is_empty() || self.routes.is_empty() {
            return vec![];
        }
        let Some(route) = self.selection.selected(&self.routes) else {
            return vec![];
        };

        let invocation_id = InvocationId::new();
        self.active_invocation = Some(invocation_id);
        vec![PanelAction::SendToHost(UiToHost::Invoke {
            invocation_id,
            action_id: route.action_id.clone(),
            input: ActionInput::plain_text(&self.query),
        })]
    }

    fn on_routes(
        &mut self,
        generation: QueryGeneration,
        items: Vec<RouteItem>,
    ) -> Vec<PanelAction> {
        if generation == self.generation {
            self.routes = items;
            self.selection.set_total(self.routes.len());
        }
        vec![]
    }

    fn on_invocation_completed(
        &mut self,
        invocation_id: InvocationId,
        outcome: ActionOutcome,
    ) -> Vec<PanelAction> {
        // 调用可能在新查询或会话过期之后才完成。不要让它
        // 复活过期的结果界面。
        if self.active_invocation != Some(invocation_id) {
            return vec![];
        }

        self.active_invocation = None;
        self.last_outcome = Some(outcome);
        vec![]
    }

    fn reset_expired_session(&mut self) {
        self.query.clear();
        self.pending_query.clear();
        self.generation = self.generation.next();
        self.debounce_revision = self.debounce_revision.wrapping_add(1);
        self.flushed_debounce_revision = None;
        self.routes.clear();
        self.selection.set_total(0);
        self.active_invocation = None;
        self.last_outcome = None;
    }
}

impl Default for PanelState {
    fn default() -> Self {
        Self::new()
    }
}
