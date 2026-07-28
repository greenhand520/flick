//! Portal GlobalShortcuts adapter.
//!
//! Manages the full lifecycle: session creation, ListShortcuts, BindShortcuts,
//! recovery, and activation forwarding.

use crate::session::{
    show_search_preferred_trigger, show_search_shortcut_description,
    show_search_shortcut_id, ShortcutActivation, ShortcutBinding, ShortcutError,
};
use crate::shortcut_state::{RecoveryBackoff, ShortcutState};
use flick_domain::UiIntent;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

/// Trait for forwarding UI intents (show/hide search panel).
pub trait IntentSink: Send + Sync {
    fn send_intent(&self, intent: UiIntent);
}

/// The Portal GlobalShortcuts adapter.
///
/// Manages the Portal session lifecycle, shortcut binding, and activation.
/// Follows the rules:
/// - One BindShortcuts per session
/// - No automatic re-bind on user cancel
/// - Recovery only does CreateSession + ListShortcuts
/// - Activation forwards as UiIntent::ShowSearchPanel
pub struct PortalShortcutAdapter {
    state: Arc<Mutex<PortalInner>>,
    intent_sink: Arc<dyn IntentSink>,
}

struct PortalInner {
    state: ShortcutState,
    session_created_at: Option<Instant>,
    shortcut_count: u32,
    last_error: Option<String>,
    reconnect_attempt: Option<u32>,
    next_reconnect_at: Option<Instant>,
    backoff: RecoveryBackoff,
    bound_in_current_session: bool,
}

impl PortalShortcutAdapter {
    pub fn new(intent_sink: Arc<dyn IntentSink>) -> Self {
        Self {
            state: Arc::new(Mutex::new(PortalInner {
                state: ShortcutState::Unavailable,
                session_created_at: None,
                shortcut_count: 0,
                last_error: None,
                reconnect_attempt: None,
                next_reconnect_at: None,
                backoff: RecoveryBackoff::new(),
                bound_in_current_session: false,
            })),
            intent_sink,
        }
    }

    /// Get the current shortcut state.
    pub async fn state(&self) -> ShortcutState {
        self.state.lock().await.state.clone()
    }

    /// Initialize the Portal session and query existing shortcuts.
    ///
    /// This is called on startup and during recovery. It does NOT call
    /// BindShortcuts — that requires explicit user action.
    pub async fn initialize(&self) -> Result<(), ShortcutError> {
        let mut inner = self.state.lock().await;

        // In real implementation, this would:
        // 1. Check Portal interface availability via D-Bus
        // 2. Create a GlobalShortcuts session
        // 3. Call ListShortcuts to check for existing bindings
        // 4. Subscribe to activation/close events

        // For now, simulate the state transitions
        inner.state = ShortcutState::Unconfigured;
        inner.session_created_at = Some(Instant::now());
        inner.reconnect_attempt = None;
        inner.bound_in_current_session = false;

        info!("Portal session initialized, state: Unconfigured");
        Ok(())
    }

    /// Configure (bind) the search shortcut.
    ///
    /// Only call this on explicit user action (e.g., `flick shortcuts configure`).
    /// A session can only attempt BindShortcuts once.
    pub async fn configure_search_shortcut(&self) -> Result<ShortcutBinding, ShortcutError> {
        let mut inner = self.state.lock().await;

        if inner.state == ShortcutState::UserCancelled {
            return Err(ShortcutError::UserCancelled);
        }

        if inner.bound_in_current_session {
            // Already attempted BindShortcuts in this session
            return Err(ShortcutError::SessionError {
                reason: "BindShortcuts already attempted in this session".into(),
            });
        }

        inner.state = ShortcutState::Binding;
        drop(inner);

        // In real implementation, this would call Portal BindShortcuts.
        // Simulate success for now.
        let mut inner = self.state.lock().await;
        inner.state = ShortcutState::Bound;
        inner.shortcut_count = 1;
        inner.bound_in_current_session = true;

        let binding = ShortcutBinding {
            id: show_search_shortcut_id().into(),
            description: show_search_shortcut_description().into(),
            preferred_trigger: show_search_preferred_trigger().into(),
            actual_trigger: Some(show_search_preferred_trigger().into()),
        };

        info!("Search shortcut configured: {:?}", binding.actual_trigger);
        Ok(binding)
    }

    /// Handle user cancellation of the binding dialog.
    pub async fn on_user_cancelled(&self) {
        let mut inner = self.state.lock().await;
        inner.state = ShortcutState::UserCancelled;
        warn!("User cancelled shortcut configuration");
    }

    /// Handle Portal request ending without success.
    pub async fn on_request_ended(&self, reason: String) {
        let mut inner = self.state.lock().await;
        inner.state = ShortcutState::RequestEnded;
        inner.last_error = Some(reason.clone());
        warn!(reason, "Portal request ended without binding");
    }

    /// Handle session/transport failure and begin recovery.
    ///
    /// Recovery creates a new session and calls ListShortcuts, but does NOT
    /// call BindShortcuts.
    pub async fn on_session_error(&self, reason: String) {
        let mut inner = self.state.lock().await;
        let attempt = inner.reconnect_attempt.map_or(0, |a| a + 1);
        let delay = inner.backoff.delay_for_attempt(attempt);

        inner.state = ShortcutState::Recovering { attempt };
        inner.reconnect_attempt = Some(attempt);
        inner.next_reconnect_at = Some(Instant::now() + delay);
        inner.last_error = Some(reason.clone());
        inner.bound_in_current_session = false;

        warn!(
            attempt,
            delay_secs = delay.as_secs(),
            reason,
            "Portal session error, starting recovery"
        );
    }

    /// Attempt to recover: create new session + ListShortcuts.
    ///
    /// This is called by the recovery timer. Does NOT call BindShortcuts.
    pub async fn attempt_recovery(&self) -> Result<(), ShortcutError> {
        let mut inner = self.state.lock().await;

        if !matches!(inner.state, ShortcutState::Recovering { .. }) {
            return Ok(());
        }

        // In real implementation: CreateSession + ListShortcuts
        // Simulate: check if shortcuts exist from previous binding
        inner.state = ShortcutState::Unconfigured;
        inner.session_created_at = Some(Instant::now());
        inner.bound_in_current_session = false;

        info!("Recovery: new session created, checking existing bindings");
        Ok(())
    }

    /// Handle an activation event from Portal.
    pub async fn on_activated(&self, activation: ShortcutActivation) {
        debug!(shortcut_id = %activation.shortcut_id, "Shortcut activated");

        self.intent_sink
            .send_intent(UiIntent::ShowSearchPanel {
                activation_token: activation.activation_token,
            });
    }

    /// Attempt reconnect (user-triggered).
    pub async fn reconnect(&self) -> Result<(), ShortcutError> {
        self.on_session_error("user-requested reconnect".into())
            .await;
        self.attempt_recovery().await
    }

    /// Get diagnostic information.
    pub async fn diagnostics(&self) -> PortalDiagnostics {
        let inner = self.state.lock().await;
        PortalDiagnostics {
            state: inner.state.clone(),
            shortcut_count: inner.shortcut_count,
            session_age: inner
                .session_created_at
                .map(|t| t.elapsed()),
            last_error: inner.last_error.clone(),
            reconnect_attempt: inner.reconnect_attempt,
        }
    }
}

/// Diagnostic snapshot of the Portal adapter.
#[derive(Debug, Clone)]
pub struct PortalDiagnostics {
    pub state: ShortcutState,
    pub shortcut_count: u32,
    pub session_age: Option<std::time::Duration>,
    pub last_error: Option<String>,
    pub reconnect_attempt: Option<u32>,
}
