//! IcedShell: the top-level shell that owns PanelState, the message bridge,
//! and the outbound command channel.
//!
//! This is the main entry point for the Iced application. It processes
//! `ShellMessage` events, delegates to `PanelState`, and forwards
//! `PanelAction`s to the host via the outbound channel.

use crate::bridge::IcedHostBridge;
use crate::message::ShellMessage;
use crate::panel::{PanelAction, PanelEvent, PanelState};
use flick_domain::*;
use tokio::sync::mpsc;

/// The window role — Phase 0 only has SearchPanel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowRole {
    SearchPanel,
}

/// The Iced shell: owns all UI state and communication channels.
pub struct IcedShell {
    /// The search panel state machine.
    pub panel: PanelState,
    /// Inbound bridge from the host.
    bridge: IcedHostBridge,
    /// Outbound channel to the host (coordinator).
    host_tx: mpsc::UnboundedSender<UiToHost>,
    /// Whether debounce is currently active.
    debounce_active: bool,
}

impl IcedShell {
    /// Create a new shell.
    pub fn new(
        bridge: IcedHostBridge,
        host_tx: mpsc::UnboundedSender<UiToHost>,
    ) -> Self {
        Self {
            panel: PanelState::new(),
            bridge,
            host_tx,
            debounce_active: false,
        }
    }

    /// Process a `ShellMessage` and execute any resulting actions.
    pub fn update(&mut self, message: ShellMessage) {
        let event = match message {
            ShellMessage::InputChanged(text) => PanelEvent::InputChanged(text),
            ShellMessage::Submitted => PanelEvent::Submitted,
            ShellMessage::EscapePressed => PanelEvent::EscapePressed,
            ShellMessage::SelectionUp => PanelEvent::SelectionUp,
            ShellMessage::SelectionDown => PanelEvent::SelectionDown,
            ShellMessage::WindowClosed => PanelEvent::WindowClosed,
            ShellMessage::HostEvent(host_event) => {
                match host_event {
                    HostToUi::Routes { generation, items } => {
                        PanelEvent::Routes { generation, items }
                    }
                    HostToUi::InvocationCompleted { invocation_id, outcome } => {
                        PanelEvent::InvocationCompleted { invocation_id, outcome }
                    }
                    HostToUi::RuntimeStatus(status) => PanelEvent::RuntimeStatus(status),
                    HostToUi::ShowSearchPanel { activation_token } => {
                        PanelEvent::Show { activation_token }
                    }
                    HostToUi::HideSearchPanel => PanelEvent::Hide,
                }
            }
            ShellMessage::DebounceFlush => PanelEvent::DebounceFlush,
        };

        let actions = self.panel.update(event);
        for action in actions {
            self.execute_action(action);
        }
    }

    /// Poll for any pending host events (non-blocking).
    pub fn poll_host_events(&mut self) {
        while let Some(msg) = self.bridge.try_recv() {
            self.update(msg);
        }
    }

    /// Execute a panel action.
    fn execute_action(&mut self, action: PanelAction) {
        match action {
            PanelAction::SendToHost(msg) => {
                let _ = self.host_tx.send(msg);
            }
            PanelAction::StartDebounce => {
                self.debounce_active = true;
            }
            PanelAction::CancelDebounce => {
                self.debounce_active = false;
            }
        }
    }

    /// Whether the panel is currently visible.
    pub fn is_visible(&self) -> bool {
        self.panel.visible
    }

    /// Whether debounce is active.
    pub fn is_debounce_active(&self) -> bool {
        self.debounce_active
    }
}
