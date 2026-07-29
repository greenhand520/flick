//! Search panel state machine.
//!
//! This module owns logical panel state, independent of the native Iced
//! window. Closing the native window hides the panel while retaining its
//! logical session for a short, bounded resume period.

use flick_domain::*;
use std::time::{Duration, Instant};

/// A hidden panel retains its logical session for this duration.
pub const PANEL_RESUME_WINDOW: Duration = Duration::from_secs(10);

/// Events accepted by [`PanelState`].
#[derive(Debug, Clone)]
pub enum PanelEvent {
    Show {
        activation_token: Option<String>,
    },
    Hide,
    InputChanged(String),
    /// The Iced-side debounce timer for a particular input revision expired.
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

/// Outbound commands emitted by [`PanelState`].
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

/// Logical state for one search-panel session.
#[derive(Debug)]
pub struct PanelState {
    pub visible: bool,
    pub created: bool,
    /// The real-time, controlled text input value.
    pub query: String,
    /// Compatibility field retained during Phase 0; always mirrors [`Self::query`].
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

    /// Current input-debounce revision, exposed for Iced task wiring and tests.
    pub fn debounce_revision(&self) -> u64 {
        self.debounce_revision
    }

    /// Applies an event using the current monotonic time.
    ///
    /// Production callers generally use this method. [`Self::update_at`] keeps
    /// the session-expiry boundary deterministic in tests and embedding code.
    pub fn update(&mut self, event: PanelEvent) -> Vec<PanelAction> {
        self.update_at(event, Instant::now())
    }

    /// Applies an event at a caller-supplied monotonic timestamp.
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
        // A focus loss can be followed by the compositor's close request. The
        // first event owns the transition and reports exactly one close to host.
        if !self.visible {
            return vec![];
        }

        self.visible = false;
        self.hidden_at = Some(now);
        // Outstanding Iced debounce tasks are not cancellable. Invalidating
        // their revision makes any late completion harmless.
        self.debounce_revision = self.debounce_revision.wrapping_add(1);
        self.flushed_debounce_revision = None;
        vec![PanelAction::SendToHost(UiToHost::SearchPanelClosed)]
    }

    fn on_input_changed(&mut self, text: String) -> Vec<PanelAction> {
        // `query` is the controlled Iced TextInput value. It must update in the
        // same event turn as user input so cursor editing and long Backspace
        // repeats never observe stale model text.
        self.query = text;
        self.pending_query.clone_from(&self.query);
        // QueryGeneration protects against asynchronous runtime results; the
        // local revision below independently protects Iced debounce tasks.
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
        // An invocation may finish after a new query or an expired session. Do
        // not let it resurrect stale result UI.
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
