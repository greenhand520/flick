//! Application coordinator — the central hub between UI and Runtime.
//!
//! The `ApplicationCoordinator` receives UI events and intents, delegates
//! work to the `RuntimePort`, and emits `HostToUi` events through the
//! `HostEventSink`. It is runtime-agnostic: it uses `async_trait` and
//! does not depend on any particular async runtime.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use flick_domain::{ActionRequest, Caller, Deadline, HostToUi, InputKind, UiIntent, UiToHost};

use crate::ports::{HostEventSink, RuntimePort};
use crate::query::QueryTracker;

/// Central coordinator that bridges UI events and the Flick Runtime.
///
/// # Thread safety
///
/// All fields are `Send + Sync`. The coordinator is designed to be wrapped
/// in an `Arc` and shared across async tasks.
pub struct ApplicationCoordinator {
    runtime: Arc<dyn RuntimePort>,
    sink: Arc<dyn HostEventSink>,
    query_tracker: QueryTracker,
    panel_visible: AtomicBool,
}

impl ApplicationCoordinator {
    /// Create a new coordinator with the given runtime and event sink.
    pub fn new(runtime: Arc<dyn RuntimePort>, sink: Arc<dyn HostEventSink>) -> Self {
        Self {
            runtime,
            sink,
            query_tracker: QueryTracker::new(),
            panel_visible: AtomicBool::new(false),
        }
    }

    // ── UI → Host message handling ────────────────────────────────────────

    /// Handle a message from the UI.
    ///
    /// Each variant of `UiToHost` is dispatched to the appropriate
    /// runtime / sink call.
    pub async fn handle_ui(&self, event: UiToHost) {
        match event {
            UiToHost::QueryChanged { generation, text } => {
                // Record the latest generation before calling the runtime.
                self.query_tracker.set_current(generation);

                let items = self.runtime.route(&text, InputKind::PlainText).await;

                // Only emit results if this generation is still current.
                if self.query_tracker.is_current(generation) {
                    self.sink.send(HostToUi::Routes { generation, items });
                }
                // else: a newer query superseded this one — drop silently.
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
                // Record that the panel is no longer visible.
                self.panel_visible.store(false, Ordering::SeqCst);
            }
        }
    }

    // ── Intent handling (from Portal / external triggers) ─────────────────

    /// Handle a `UiIntent` from Portal or other external triggers.
    ///
    /// Intents are **not** action invocations. `ShowSearchPanel` only
    /// emits a `HostToUi::ShowSearchPanel` event — it does **not** call
    /// `runtime.invoke`.
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
        }
    }

    // ── Panel toggle ──────────────────────────────────────────────────────

    /// Toggle the search panel between shown and hidden states.
    ///
    /// This is the keyboard-shortcut entry point: it flips the current
    /// visibility and emits the corresponding `HostToUi` event. It does
    /// **not** interact with the runtime.
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
