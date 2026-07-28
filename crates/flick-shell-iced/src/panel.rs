//! Search panel state machine.
//!
//! Manages the visual and logical state of the search panel:
//! - Window visibility lifecycle: NotCreated → Visible → Hidden → Visible
//! - Query input with generation tracking
//! - Route item list with selection navigation
//! - Invocation result display
//! - Runtime status display
//!
//! This module has NO dependency on Iced, Runtime, or Portal.
//! It is a pure state machine driven by `PanelEvent` and producing
//! `PanelAction` (outbound commands).

use flick_domain::*;

// ─── Panel events (input to the state machine) ──────────────────────────────

/// Events that drive the panel state machine.
#[derive(Debug, Clone)]
pub enum PanelEvent {
    /// Show the search panel (with optional activation token from Portal).
    Show { activation_token: Option<String> },
    /// Hide the search panel.
    Hide,
    /// User changed the query text.
    InputChanged(String),
    /// Debounce timer expired; flush the pending query.
    DebounceFlush,
    /// User pressed Enter to invoke.
    Submitted,
    /// User pressed Escape.
    EscapePressed,
    /// User pressed ArrowUp.
    SelectionUp,
    /// User pressed ArrowDown.
    SelectionDown,
    /// Window was closed by the WM.
    WindowClosed,
    /// Host sent routes for a generation.
    Routes { generation: QueryGeneration, items: Vec<RouteItem> },
    /// Host completed an invocation.
    InvocationCompleted { invocation_id: InvocationId, outcome: ActionOutcome },
    /// Runtime status update.
    RuntimeStatus(RuntimeStatus),
}

/// Actions emitted by the panel state machine (outbound commands).
#[derive(Debug, Clone, PartialEq)]
pub enum PanelAction {
    /// Send a message to the host.
    SendToHost(UiToHost),
    /// Start or restart the debounce timer.
    StartDebounce,
    /// Cancel the debounce timer.
    CancelDebounce,
}

// ─── Selection state ────────────────────────────────────────────────────────

/// Which item is currently selected in the route list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionState {
    /// Index into the routes list. 0 when list is empty.
    pub index: usize,
    /// Total number of routes. 0 when list is empty.
    pub total: usize,
}

impl SelectionState {
    pub fn new() -> Self {
        Self { index: 0, total: 0 }
    }

    /// Update total count and clamp index.
    pub fn set_total(&mut self, total: usize) {
        self.total = total;
        if self.index >= total && total > 0 {
            self.index = total - 1;
        } else if total == 0 {
            self.index = 0;
        }
    }

    /// Move selection up (towards index 0). No-op if empty or already at top.
    pub fn move_up(&mut self) {
        if self.total > 0 && self.index > 0 {
            self.index -= 1;
        }
    }

    /// Move selection down (towards total-1). No-op if empty or already at bottom.
    pub fn move_down(&mut self) {
        if self.total > 0 && self.index < self.total - 1 {
            self.index += 1;
        }
    }

    /// Get the selected route, if any.
    pub fn selected<'a>(&self, routes: &'a [RouteItem]) -> Option<&'a RouteItem> {
        if self.total > 0 && self.index < routes.len() {
            routes.get(self.index)
        } else {
            None
        }
    }
}

impl Default for SelectionState {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Panel state ────────────────────────────────────────────────────────────

/// The complete state of the search panel.
///
/// This is a pure data struct. Rendering is handled separately.
#[derive(Debug)]
pub struct PanelState {
    /// Whether the window exists and is visible.
    pub visible: bool,
    /// Whether the window has ever been created.
    pub created: bool,
    /// Current query text.
    pub query: String,
    /// Pending query (not yet flushed by debounce).
    pub pending_query: String,
    /// Current generation counter.
    pub generation: QueryGeneration,
    /// Current route items from the host.
    pub routes: Vec<RouteItem>,
    /// Selection state.
    pub selection: SelectionState,
    /// The most recent invocation ID (if any).
    pub active_invocation: Option<InvocationId>,
    /// The most recent invocation result.
    pub last_outcome: Option<ActionOutcome>,
    /// Current runtime status.
    pub runtime_status: RuntimeStatus,
    /// Activation token from Portal (if any).
    pub activation_token: Option<String>,
    /// Whether the activation token was applied (Phase 0: always false).
    pub activation_token_applied: bool,
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
        }
    }

    /// Process an event and return any actions to execute.
    pub fn update(&mut self, event: PanelEvent) -> Vec<PanelAction> {
        match event {
            PanelEvent::Show { activation_token } => self.on_show(activation_token),
            PanelEvent::Hide => self.on_hide(),
            PanelEvent::InputChanged(text) => self.on_input_changed(text),
            PanelEvent::DebounceFlush => self.on_debounce_flush(),
            PanelEvent::Submitted => self.on_submitted(),
            PanelEvent::EscapePressed => self.on_escape(),
            PanelEvent::SelectionUp => {
                self.selection.move_up();
                vec![]
            }
            PanelEvent::SelectionDown => {
                self.selection.move_down();
                vec![]
            }
            PanelEvent::WindowClosed => self.on_hide(),
            PanelEvent::Routes { generation, items } => self.on_routes(generation, items),
            PanelEvent::InvocationCompleted { invocation_id, outcome } => {
                self.on_invocation_completed(invocation_id, outcome)
            }
            PanelEvent::RuntimeStatus(status) => {
                self.runtime_status = status;
                vec![]
            }
        }
    }

    fn on_show(&mut self, activation_token: Option<String>) -> Vec<PanelAction> {
        self.created = true;
        self.visible = true;
        self.activation_token = activation_token;
        // Phase 0: token is not applied to any Wayland API
        self.activation_token_applied = false;
        vec![]
    }

    fn on_hide(&mut self) -> Vec<PanelAction> {
        self.visible = false;
        vec![PanelAction::SendToHost(UiToHost::SearchPanelClosed)]
    }

    fn on_input_changed(&mut self, text: String) -> Vec<PanelAction> {
        self.pending_query = text;
        vec![PanelAction::StartDebounce]
    }

    fn on_debounce_flush(&mut self) -> Vec<PanelAction> {
        if self.pending_query == self.query {
            return vec![];
        }
        self.query = self.pending_query.clone();
        self.generation = self.generation.next();
        vec![PanelAction::SendToHost(UiToHost::QueryChanged {
            generation: self.generation,
            text: self.query.clone(),
        })]
    }

    fn on_submitted(&mut self) -> Vec<PanelAction> {
        if self.query.is_empty() || self.routes.is_empty() {
            return vec![];
        }
        if let Some(route) = self.selection.selected(&self.routes) {
            let invocation_id = InvocationId::new();
            self.active_invocation = Some(invocation_id);
            vec![PanelAction::SendToHost(UiToHost::Invoke {
                invocation_id,
                action_id: route.action_id.clone(),
                input: ActionInput::plain_text(&self.query),
            })]
        } else {
            vec![]
        }
    }

    fn on_escape(&mut self) -> Vec<PanelAction> {
        self.visible = false;
        vec![
            PanelAction::CancelDebounce,
            PanelAction::SendToHost(UiToHost::SearchPanelClosed),
        ]
    }

    fn on_routes(&mut self, generation: QueryGeneration, items: Vec<RouteItem>) -> Vec<PanelAction> {
        // Only accept routes for the current generation.
        if generation == self.generation {
            self.routes = items;
            self.selection.set_total(self.routes.len());
        }
        // else: stale generation — silently discard
        vec![]
    }

    fn on_invocation_completed(
        &mut self,
        invocation_id: InvocationId,
        outcome: ActionOutcome,
    ) -> Vec<PanelAction> {
        if self.active_invocation == Some(invocation_id) {
            self.active_invocation = None;
        }
        self.last_outcome = Some(outcome);
        vec![]
    }
}

impl Default for PanelState {
    fn default() -> Self {
        Self::new()
    }
}
