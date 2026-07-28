//! Search panel state and message types.
//!
//! In Phase 0, the Iced application is started by the daemon.
//! This module provides the state machine and message types that the
//! daemon uses to wire the UI to the coordinator.

use flick_domain::*;
use tokio::sync::mpsc;

// ─── Messages ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Message {
    /// User typed in the search box.
    InputChanged(String),
    /// A host-to-ui event arrived.
    HostEvent(HostToUi),
    /// User pressed Enter on the input.
    Submitted,
    /// User pressed Escape.
    EscapePressed,
    /// Window was closed.
    WindowClosed,
    /// No-op for initialization.
    None,
}

// ─── Host Event Channel ──────────────────────────────────────────────────────

/// Sender side for Host → UI events (given to the coordinator).
pub type HostEventSender = mpsc::UnboundedSender<HostToUi>;

/// Receiver side for Host → UI events (consumed by Iced subscription).
pub type HostEventReceiver = mpsc::UnboundedReceiver<HostToUi>;

// ─── Search Panel State ──────────────────────────────────────────────────────

/// State of the search panel, managed by the Iced application.
///
/// This struct holds all the data needed to render the search UI.
/// The daemon's Iced application owns this state and updates it
/// in response to `Message` events.
#[derive(Debug)]
pub struct SearchPanelState {
    pub visible: bool,
    pub query: String,
    pub generation: QueryGeneration,
    pub routes: Vec<RouteItem>,
    pub selected_index: usize,
    pub last_outcome: Option<ActionOutcome>,
}

impl SearchPanelState {
    pub fn new() -> Self {
        Self {
            visible: false,
            query: String::new(),
            generation: QueryGeneration::INITIAL,
            routes: Vec::new(),
            selected_index: 0,
            last_outcome: None,
        }
    }
}

impl Default for SearchPanelState {
    fn default() -> Self {
        Self::new()
    }
}

/// A thin wrapper that provides the search panel logic.
///
/// In Phase 0, this is used by the daemon to manage panel state.
/// The actual Iced application (daemon mode) is started in the daemon
/// composition root.
pub struct SearchPanel {
    pub state: SearchPanelState,
    ui_command_tx: Option<mpsc::UnboundedSender<UiToHost>>,
}

impl SearchPanel {
    pub fn new(ui_command_tx: mpsc::UnboundedSender<UiToHost>) -> Self {
        Self {
            state: SearchPanelState::new(),
            ui_command_tx: Some(ui_command_tx),
        }
    }

    fn send_to_host(&self, msg: UiToHost) {
        if let Some(tx) = &self.ui_command_tx {
            let _ = tx.send(msg);
        }
    }

    /// Process a host event and update state.
    pub fn handle_host_event(&mut self, event: HostToUi) {
        match event {
            HostToUi::Routes { generation, items } => {
                if generation == self.state.generation {
                    self.state.routes = items;
                    self.state.selected_index = 0;
                }
            }
            HostToUi::InvocationCompleted { outcome, .. } => {
                self.state.last_outcome = Some(outcome);
            }
            HostToUi::ShowSearchPanel { .. } => {
                self.state.visible = true;
            }
            HostToUi::HideSearchPanel => {
                self.state.visible = false;
            }
            HostToUi::RuntimeStatus(_) => {}
        }
    }

    /// Process user input change.
    pub fn handle_input_changed(&mut self, text: String) {
        self.state.query = text;
        self.state.generation = self.state.generation.next();
        self.send_to_host(UiToHost::QueryChanged {
            generation: self.state.generation,
            text: self.state.query.clone(),
        });
    }

    /// Process user submission (Enter key).
    pub fn handle_submitted(&mut self) {
        if !self.state.query.is_empty() && !self.state.routes.is_empty() {
            let route = &self.state.routes[self.state.selected_index];
            self.send_to_host(UiToHost::Invoke {
                invocation_id: InvocationId::new(),
                action_id: route.action_id.clone(),
                input: ActionInput::plain_text(&self.state.query),
            });
        }
    }

    /// Process Escape key.
    pub fn handle_escape(&mut self) {
        self.state.visible = false;
        self.send_to_host(UiToHost::SearchPanelClosed);
    }

    /// Toggle visibility (IPC fallback).
    pub fn toggle(&mut self) {
        self.state.visible = !self.state.visible;
        if !self.state.visible {
            self.send_to_host(UiToHost::SearchPanelClosed);
        }
    }
}

/// Create the host event channel pair.
pub fn create_event_channels() -> (
    mpsc::UnboundedSender<HostToUi>,
    mpsc::UnboundedReceiver<HostToUi>,
) {
    mpsc::unbounded_channel()
}

/// Create the UI command channel pair.
pub fn create_command_channels() -> (
    mpsc::UnboundedSender<UiToHost>,
    mpsc::UnboundedReceiver<UiToHost>,
) {
    mpsc::unbounded_channel()
}
