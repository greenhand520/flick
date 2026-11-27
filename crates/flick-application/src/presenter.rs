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

use crate::accumulate::SearchSession;

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
    /// UI 采集到完整输入快照（含文件/图片附件）。
    InputCaptured(SearchInputSnapshot),
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
    /// Surface 内容更新（来自宿主对 SurfaceEvent 的回应）。
    SurfaceUpdated {
        content: SurfaceContent,
    },
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
    /// 最近一次 UI 采集到的完整输入快照（文本/文件/图片）。
    pub input: SearchInputSnapshot,
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
    /// 当前打开的 Surface 会话（若有）。
    surface_session: Option<SurfaceSessionId>,
    /// Surface 是否处于「搜索模式」（输入变化只触发 SearchChanged，不清结果）。
    surface_search: bool,
    debounce_revision: u64,
    flushed_debounce_revision: Option<u64>,
    hidden_at: Option<Instant>,
    /// 本次面板会话的结果累积状态。
    session: SearchSession,
}

impl PanelState {
    pub fn new() -> Self {
        Self {
            visible: false,
            created: false,
            query: String::new(),
            input: SearchInputSnapshot::text(String::new(), InputSource::Manual),
            pending_query: String::new(),
            generation: QueryGeneration::INITIAL,
            routes: Vec::new(),
            selection: SelectionState::new(),
            active_invocation: None,
            last_outcome: None,
            runtime_status: RuntimeStatus::Running,
            activation_token: None,
            activation_token_applied: false,
            surface_session: None,
            surface_search: false,
            debounce_revision: 0,
            flushed_debounce_revision: None,
            hidden_at: None,
            session: SearchSession::new(),
        }
    }

    /// 当前的输入防抖 revision，供 UI 任务接线和测试使用。
    pub fn debounce_revision(&self) -> u64 {
        self.debounce_revision
    }

    /// 当前打开的 Surface 会话（若有）。
    pub fn surface_session(&self) -> Option<SurfaceSessionId> {
        self.surface_session
    }

    /// Surface 是否处于搜索模式。
    pub fn surface_search(&self) -> bool {
        self.surface_search
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
            PanelEvent::Hide | PanelEvent::WindowClosed => self.on_hide(now),
            PanelEvent::EscapePressed => self.on_escape(now),
            PanelEvent::InputChanged(text) => self.on_input_changed(text),
            PanelEvent::InputCaptured(snapshot) => self.on_input_captured(snapshot),
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
            PanelEvent::SurfaceUpdated { content } => self.on_surface_updated(content),
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
        // 面板显示即开启一次新会话；隐藏期间的旧累积不会带进来。
        self.session
            .begin(self.input.kind, self.input.source == InputSource::Manual);
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
        self.session.end();
        self.hidden_at = Some(now);
        // 尚未完成的 UI 防抖任务无法取消。使它们的 revision
        // 失效可让任何迟到的完成操作变得无害。
        self.debounce_revision = self.debounce_revision.wrapping_add(1);
        self.flushed_debounce_revision = None;

        let mut actions = vec![PanelAction::SendToHost(UiToHost::SearchPanelClosed)];
        // Surface 会话随面板隐藏而关闭；清空本地会话状态。
        if let Some(session) = self.surface_session.take() {
            self.surface_search = false;
            actions.push(PanelAction::SendToHost(UiToHost::SurfaceClosed {
                session_id: session,
            }));
        }
        actions
    }

    /// Esc：清空查询、附件与累积，然后按隐藏处理。
    ///
    /// 与 `Hide` 的区别在于 Esc 是「用户主动放弃本次搜索」，
    /// 因此设计文档 §6.1 重置条件 4 要求同时清空输入与累积。
    fn on_escape(&mut self, now: Instant) -> Vec<PanelAction> {
        if !self.visible {
            return vec![];
        }
        self.query.clear();
        self.input = SearchInputSnapshot::text(String::new(), InputSource::Manual);
        self.pending_query.clear();
        self.session.end();
        self.routes.clear();
        self.selection.set_total(0);
        self.on_hide(now)
    }

    fn on_input_changed(&mut self, text: String) -> Vec<PanelAction> {
        // `query` 是受控的 UI TextInput 值。它必须在与用户输入
        // 同一个事件轮次中更新，这样光标编辑和长时间按住 Backspace
        // 重复输入都不会观察到过期的模型文本。
        self.query = text;
        // 手动键入始终回到纯文本输入：立即清空此前的文件/图片附件，
        // 并切换 `source: Manual`。不因为文本看起来像路径/URL 而变成 Files。
        self.input = SearchInputSnapshot::text(self.query.clone(), InputSource::Manual);
        self.pending_query.clone_from(&self.query);
        self.debounce_revision = self.debounce_revision.wrapping_add(1);
        self.flushed_debounce_revision = None;

        // Surface 搜索模式：结果面板仍由 Surface 会话持有，
        // 因此输入变化只更新查询并重新防抖，绝不清空结果。
        if self.surface_search {
            return vec![PanelAction::StartDebounce {
                revision: self.debounce_revision,
            }];
        }

        // 手动键入始终是手动模式；附件若存在则在此之前已被清空。
        self.session.note_kind(self.input.kind, true);
        // 输入被清空即重置累积（§6.1 重置条件 1），但会话仍有效。
        if self.query.is_empty() {
            self.session.clear();
        }

        // QueryGeneration 用于防范异步的运行时结果；上方的本地 revision
        // 则独立地防范 UI 防抖任务。
        self.generation = self.generation.next();
        // 手动累积模式下保留已有结果行，等新批次到达后合并；
        // 非手动输入仍然立刻清空，避免旧附件结果残留。
        if !self.session.is_manual() {
            self.routes.clear();
            self.selection.set_total(0);
        }
        self.last_outcome = None;
        self.active_invocation = None;

        vec![PanelAction::StartDebounce {
            revision: self.debounce_revision,
        }]
    }

    /// 应用一次完整输入快照：文件/图片输入也能推进查询代次并触发防抖。
    fn on_input_captured(&mut self, snapshot: SearchInputSnapshot) -> Vec<PanelAction> {
        self.query = snapshot.text.clone();
        self.pending_query.clone_from(&self.query);
        self.input = snapshot;
        self.debounce_revision = self.debounce_revision.wrapping_add(1);
        self.flushed_debounce_revision = None;

        if self.surface_search {
            return vec![PanelAction::StartDebounce {
                revision: self.debounce_revision,
            }];
        }

        self.session
            .note_kind(self.input.kind, self.input.source == InputSource::Manual);

        self.generation = self.generation.next();
        if !self.session.is_manual() {
            self.routes.clear();
            self.selection.set_total(0);
        }
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

        if self.surface_search {
            let Some(session) = self.surface_session else {
                return vec![];
            };
            return vec![PanelAction::SendToHost(UiToHost::SurfaceEvent {
                session_id: session,
                kind: SurfaceEventKind::SearchChanged {
                    text: self.pending_query.clone(),
                },
            })];
        }

        vec![PanelAction::SendToHost(UiToHost::QueryChanged {
            generation: self.generation,
            input: self.input.clone(),
        })]
    }

    fn on_submitted(&mut self) -> Vec<PanelAction> {
        if self.surface_search {
            let Some(session) = self.surface_session else {
                return vec![];
            };
            return vec![PanelAction::SendToHost(UiToHost::SurfaceEvent {
                session_id: session,
                kind: SurfaceEventKind::Submitted {
                    node_id: "__search__".to_string(),
                    text: self.query.clone(),
                },
            })];
        }

        let has_input =
            !self.query.is_empty() || !self.input.files.is_empty() || self.input.image.is_some();
        if !has_input || self.routes.is_empty() {
            return vec![];
        }
        let Some(route) = self.selection.selected(&self.routes) else {
            return vec![];
        };

        // 搜索阶段 Execution 项不是独立 Action：交给产出它的父 Action 处理。
        if route.kind == RouteItemKind::Execution {
            let invocation_id = route.invocation_id.unwrap_or_else(InvocationId::new);
            let parent_action_id = route
                .parent_action_id
                .clone()
                .or_else(|| {
                    route
                        .execution
                        .as_ref()
                        .map(|item| item.parent_action_id.clone())
                })
                .unwrap_or_else(|| route.action_id.clone());
            let item_id = if route.item_id.is_empty() {
                route.execution.as_ref().map(|item| item.item_id.clone())
            } else {
                Some(route.item_id.clone())
            };
            self.active_invocation = Some(invocation_id);
            return vec![PanelAction::SendToHost(UiToHost::ExecutionActivated {
                generation: self.generation,
                parent_action_id,
                invocation_id,
                item_id,
                event: ExecutionEventKind::Activate,
            })];
        }

        let invocation_id = InvocationId::new();
        self.active_invocation = Some(invocation_id);
        let input = self.input.to_action_input();
        vec![PanelAction::SendToHost(UiToHost::Invoke {
            invocation_id,
            action_id: route.action_id.clone(),
            input,
        })]
    }

    fn on_routes(
        &mut self,
        generation: QueryGeneration,
        items: Vec<RouteItem>,
    ) -> Vec<PanelAction> {
        // 迟到/未来代次一律丢弃：`Routes` 不带查询文本，无法做
        // 「文本仍在链中」的判定，安全侧选择直接丢弃。
        if generation != self.generation {
            return vec![];
        }
        self.routes = self.session.merge(items);
        self.selection.set_total(self.routes.len());
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

        // 记录/清除 Surface 会话：只有打开 Surface 才进入会话模式。
        if let ActionOutcome::OpenSurface {
            session_id,
            needs_search,
            ..
        } = &outcome
        {
            self.surface_session = *session_id;
            self.surface_search = *needs_search;
        } else {
            self.surface_session = None;
            self.surface_search = false;
        }

        self.last_outcome = Some(outcome);
        vec![]
    }

    fn on_surface_updated(&mut self, content: SurfaceContent) -> Vec<PanelAction> {
        if let Some(session) = self.surface_session {
            if let Some(ActionOutcome::OpenSurface {
                session_id: Some(current),
                content: slot,
                ..
            }) = self.last_outcome.as_mut()
            {
                if *current == session {
                    *slot = content;
                }
            }
        }
        vec![]
    }

    fn reset_expired_session(&mut self) {
        self.query.clear();
        self.input = SearchInputSnapshot::text(String::new(), InputSource::Manual);
        self.pending_query.clear();
        self.generation = self.generation.next();
        self.debounce_revision = self.debounce_revision.wrapping_add(1);
        self.flushed_debounce_revision = None;
        self.session.end();
        self.routes.clear();
        self.selection.set_total(0);
        self.active_invocation = None;
        self.last_outcome = None;
        self.surface_session = None;
        self.surface_search = false;
    }
}

impl Default for PanelState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_content(text: &str) -> SurfaceContent {
        SurfaceContent {
            schema_version: 1,
            title: None,
            body: vec![SurfaceNode::Text {
                id: None,
                text: text.to_string(),
                muted: false,
            }],
        }
    }

    fn open_surface_outcome(
        session: SurfaceSessionId,
        needs_search: bool,
        text: &str,
    ) -> ActionOutcome {
        ActionOutcome::OpenSurface {
            surface_id: "surface".into(),
            content: make_content(text),
            session_id: Some(session),
            needs_search,
        }
    }

    fn state_in_surface_search(session: SurfaceSessionId) -> PanelState {
        let mut st = PanelState::new();
        st.visible = true;
        st.active_invocation = Some(InvocationId::new());
        st.surface_session = Some(session);
        st.surface_search = true;
        st.last_outcome = Some(open_surface_outcome(session, true, "old"));
        st
    }

    #[test]
    fn surface_input_keeps_results_and_flush_emits_search_changed() {
        let session = SurfaceSessionId::new();
        let mut st = state_in_surface_search(session);
        let before_active = st.active_invocation;

        let actions = st.update(PanelEvent::InputChanged("abc".into()));
        assert!(
            matches!(actions.as_slice(), [PanelAction::StartDebounce { .. }]),
            "surface input should start a debounce, got {actions:?}"
        );
        // 关键：Surface 搜索模式下输入变化绝不清空当前结果。
        assert!(
            matches!(st.last_outcome, Some(ActionOutcome::OpenSurface { .. })),
            "surface input must not clear last_outcome"
        );
        assert_eq!(st.active_invocation, before_active);

        let revision = st.debounce_revision();
        let actions = st.update(PanelEvent::DebounceFlush { revision });
        assert_eq!(
            actions,
            vec![PanelAction::SendToHost(UiToHost::SurfaceEvent {
                session_id: session,
                kind: SurfaceEventKind::SearchChanged { text: "abc".into() },
            })]
        );
    }

    #[test]
    fn surface_hide_emits_surface_closed_and_clears_state() {
        let session = SurfaceSessionId::new();
        let mut st = state_in_surface_search(session);

        let actions = st.update(PanelEvent::Hide);
        assert!(
            actions.contains(&PanelAction::SendToHost(UiToHost::SurfaceClosed {
                session_id: session,
            }))
        );
        assert!(actions.contains(&PanelAction::SendToHost(UiToHost::SearchPanelClosed)));
        assert_eq!(st.surface_session(), None);
        assert!(!st.surface_search());
    }

    #[test]
    fn surface_updated_replaces_open_surface_content() {
        let session = SurfaceSessionId::new();
        let mut st = state_in_surface_search(session);
        let new_content = make_content("new");

        let actions = st.update(PanelEvent::SurfaceUpdated {
            content: new_content.clone(),
        });
        assert!(actions.is_empty());
        match st.last_outcome.as_ref().expect("last_outcome present") {
            ActionOutcome::OpenSurface {
                content,
                session_id,
                ..
            } => {
                assert_eq!(*content, new_content);
                assert_eq!(*session_id, Some(session));
            }
            other => panic!("expected OpenSurface, got {other:?}"),
        }
        assert_eq!(st.surface_session(), Some(session));
    }

    #[test]
    fn surface_submitted_sends_submitted_event() {
        let session = SurfaceSessionId::new();
        let mut st = state_in_surface_search(session);
        st.query = "hello".into();

        let actions = st.update(PanelEvent::Submitted);
        assert_eq!(
            actions,
            vec![PanelAction::SendToHost(UiToHost::SurfaceEvent {
                session_id: session,
                kind: SurfaceEventKind::Submitted {
                    node_id: "__search__".into(),
                    text: "hello".into(),
                },
            })]
        );
    }

    /// Execution 项按 Enter 必须发 `ExecutionActivated`，而不是把它当普通 Action Invoke。
    #[test]
    fn submitted_execution_item_emits_execution_activated() {
        let mut st = PanelState::new();
        st.visible = true;
        st.query = "fire".into();
        let parent = ActionId::from_static("flick.builtin.apps.search");
        let invocation_id = InvocationId::new();
        let item = ExecutionItem {
            item_id: "app:firefox".into(),
            parent_action_id: parent.clone(),
            invocation_id,
            title: "Firefox".into(),
            subtitle: Some("firefox".into()),
            icon: None,
            badge: None,
            output: Default::default(),
        };
        st.routes = vec![RouteItem::execution(item, 0.0)];
        st.selection.set_total(1);

        let actions = st.update(PanelEvent::Submitted);
        assert_eq!(st.active_invocation, Some(invocation_id));
        match actions.as_slice() {
            [
                PanelAction::SendToHost(UiToHost::ExecutionActivated {
                    generation,
                    parent_action_id,
                    invocation_id: sent_invocation,
                    item_id,
                    event,
                }),
            ] => {
                assert_eq!(*generation, st.generation);
                assert_eq!(parent_action_id, &parent);
                assert_eq!(*sent_invocation, invocation_id);
                assert_eq!(item_id.as_deref(), Some("app:firefox"));
                assert_eq!(*event, ExecutionEventKind::Activate);
            }
            other => panic!("expected ExecutionActivated, got {other:?}"),
        }
    }

    #[test]
    fn invocation_completed_records_surface_session_and_search() {
        let session = SurfaceSessionId::new();
        let mut st = PanelState::new();
        st.visible = true;
        let invocation_id = InvocationId::new();
        st.active_invocation = Some(invocation_id);

        let actions = st.update(PanelEvent::InvocationCompleted {
            invocation_id,
            outcome: open_surface_outcome(session, true, "x"),
        });
        assert!(actions.is_empty());
        assert_eq!(st.surface_session(), Some(session));
        assert!(st.surface_search());
    }

    #[test]
    fn captured_file_snapshot_keeps_attachments_and_queries_host() {
        let mut state = PanelState::new();
        state.visible = true;
        let file = flick_domain::FileInput {
            uri: "file:///tmp/a.txt".into(),
            display_name: "a.txt".into(),
            extension: Some("txt".into()),
            is_dir: false,
            mime: Some("text/plain".into()),
        };
        let snapshot = SearchInputSnapshot::files(vec![file.clone()], InputSource::DragDrop);

        let actions = state.update_at(PanelEvent::InputCaptured(snapshot.clone()), Instant::now());
        assert_eq!(state.input, snapshot);
        assert_eq!(state.query, "file:///tmp/a.txt");
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, PanelAction::StartDebounce { .. }))
        );

        let sent = state.update_at(
            PanelEvent::DebounceFlush {
                revision: state.debounce_revision(),
            },
            Instant::now(),
        );
        assert!(sent.iter().any(|a| matches!(
            a,
            PanelAction::SendToHost(UiToHost::QueryChanged { input, .. })
                if input.files == vec![file.clone()]
        )));
    }

    #[test]
    fn manual_typing_updates_snapshot_text() {
        let mut state = PanelState::new();
        state.visible = true;
        state.update_at(PanelEvent::InputChanged("abc".into()), Instant::now());
        assert_eq!(state.input.kind, UiInputKind::Text);
        assert_eq!(state.input.text, "abc");
    }

    #[test]
    fn manual_typing_clears_attachment_and_switches_to_manual() {
        let mut state = PanelState::new();
        state.visible = true;
        let file = flick_domain::FileInput {
            uri: "file:///tmp/a.txt".into(),
            display_name: "a.txt".into(),
            extension: Some("txt".into()),
            is_dir: false,
            mime: Some("text/plain".into()),
        };
        state.update_at(
            PanelEvent::InputCaptured(SearchInputSnapshot::files(
                vec![file],
                InputSource::ClipboardRecent,
            )),
            Instant::now(),
        );
        assert!(!state.input.files.is_empty());

        state.update_at(PanelEvent::InputChanged("abc".into()), Instant::now());
        assert_eq!(state.input.kind, UiInputKind::Text);
        assert_eq!(state.input.source, InputSource::Manual);
        assert!(state.input.files.is_empty());
        assert!(state.input.image.is_none());
    }

    /// 构造一个普通 Action 候选。
    fn action_route(id: &'static str, title: &str, score: f64) -> RouteItem {
        RouteItem::action(ActionId::from_static(id), title, score)
    }

    /// 让面板进入可见的手动输入状态。
    fn visible_manual_panel() -> PanelState {
        let mut st = PanelState::new();
        st.update_at(
            PanelEvent::Show {
                activation_token: None,
            },
            Instant::now(),
        );
        st
    }

    /// 输入 `ho` 得到 A，再输入 `st` 得到 B，结果区应是 A+B（旧 → 新）。
    #[test]
    fn manual_queries_accumulate_previous_results() {
        let mut st = visible_manual_panel();

        st.update_at(PanelEvent::InputChanged("ho".into()), Instant::now());
        st.update_at(
            PanelEvent::Routes {
                generation: st.generation,
                items: vec![action_route("a.v1.host", "Host", 1.0)],
            },
            Instant::now(),
        );
        assert_eq!(st.routes.len(), 1);

        st.update_at(PanelEvent::InputChanged("host".into()), Instant::now());
        // 手动模式下输入变化不清空已有结果行。
        assert_eq!(st.routes.len(), 1, "manual typing must keep previous rows");

        st.update_at(
            PanelEvent::Routes {
                generation: st.generation,
                items: vec![action_route("a.v1.hosts", "Hosts", 2.0)],
            },
            Instant::now(),
        );
        let titles: Vec<&str> = st.routes.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, vec!["Host", "Hosts"]);
        assert_eq!(st.selection.total, 2);
    }

    /// 同一 Action 在连续两批中出现时只保留一条。
    #[test]
    fn repeated_action_across_batches_keeps_single_row() {
        let mut st = visible_manual_panel();

        st.update_at(PanelEvent::InputChanged("ho".into()), Instant::now());
        st.update_at(
            PanelEvent::Routes {
                generation: st.generation,
                items: vec![action_route("a.v1.host", "Host", 1.0)],
            },
            Instant::now(),
        );

        st.update_at(PanelEvent::InputChanged("host".into()), Instant::now());
        st.update_at(
            PanelEvent::Routes {
                generation: st.generation,
                items: vec![action_route("a.v1.host", "Host", 2.0)],
            },
            Instant::now(),
        );

        assert_eq!(st.routes.len(), 1);
        assert_eq!(st.routes[0].score, 2.0);
    }

    /// Esc 清空查询与累积，然后仍然隐藏面板（照常发 SearchPanelClosed）。
    #[test]
    fn escape_clears_query_and_accumulated_results() {
        let mut st = visible_manual_panel();

        st.update_at(PanelEvent::InputChanged("ho".into()), Instant::now());
        st.update_at(
            PanelEvent::Routes {
                generation: st.generation,
                items: vec![action_route("a.v1.host", "Host", 1.0)],
            },
            Instant::now(),
        );

        let actions = st.update_at(PanelEvent::EscapePressed, Instant::now());
        assert!(st.query.is_empty());
        assert!(st.routes.is_empty());
        assert_eq!(st.selection.total, 0);
        assert!(!st.visible);
        assert!(
            actions.contains(&PanelAction::SendToHost(UiToHost::SearchPanelClosed)),
            "Esc must still report panel closed, got {actions:?}"
        );
    }

    /// 隐藏面板结束会话：重新显示后第一批结果不再与旧累积合并。
    #[test]
    fn hide_ends_session_and_clears_accumulation() {
        let mut st = visible_manual_panel();

        st.update_at(PanelEvent::InputChanged("ho".into()), Instant::now());
        st.update_at(
            PanelEvent::Routes {
                generation: st.generation,
                items: vec![action_route("a.v1.host", "Host", 1.0)],
            },
            Instant::now(),
        );

        st.update_at(PanelEvent::Hide, Instant::now());
        st.update_at(
            PanelEvent::Show {
                activation_token: None,
            },
            Instant::now(),
        );
        st.update_at(PanelEvent::InputChanged("host".into()), Instant::now());
        st.update_at(
            PanelEvent::Routes {
                generation: st.generation,
                items: vec![action_route("a.v1.hosts", "Hosts", 2.0)],
            },
            Instant::now(),
        );

        let titles: Vec<&str> = st.routes.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, vec!["Hosts"], "hidden panel must drop old accumulation");
    }

    /// 附件输入（非手动）只替换，不累积。
    #[test]
    fn attachment_input_replaces_instead_of_accumulating() {
        let mut st = visible_manual_panel();

        st.update_at(PanelEvent::InputChanged("ho".into()), Instant::now());
        st.update_at(
            PanelEvent::Routes {
                generation: st.generation,
                items: vec![action_route("a.v1.host", "Host", 1.0)],
            },
            Instant::now(),
        );

        let file = flick_domain::FileInput {
            uri: "file:///tmp/a.txt".into(),
            display_name: "a.txt".into(),
            extension: Some("txt".into()),
            is_dir: false,
            mime: Some("text/plain".into()),
        };
        st.update_at(
            PanelEvent::InputCaptured(SearchInputSnapshot::files(
                vec![file],
                InputSource::DragDrop,
            )),
            Instant::now(),
        );
        // 非手动输入必须立刻清掉旧结果。
        assert!(st.routes.is_empty());

        st.update_at(
            PanelEvent::Routes {
                generation: st.generation,
                items: vec![action_route("a.v1.file", "File Action", 1.0)],
            },
            Instant::now(),
        );
        let titles: Vec<&str> = st.routes.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, vec!["File Action"]);
    }

    /// 旧代次的迟到批次必须被丢弃。
    #[test]
    fn late_old_generation_batch_is_dropped() {
        let mut st = visible_manual_panel();

        st.update_at(PanelEvent::InputChanged("ho".into()), Instant::now());
        let old_generation = st.generation;
        st.update_at(PanelEvent::InputChanged("host".into()), Instant::now());

        st.update_at(
            PanelEvent::Routes {
                generation: old_generation,
                items: vec![action_route("a.v1.stale", "Stale", 9.0)],
            },
            Instant::now(),
        );
        assert!(st.routes.is_empty());
    }
}
