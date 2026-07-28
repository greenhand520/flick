//! Portal GlobalShortcuts lifecycle policy.
//!
//! This module owns state transitions, cancellation, and recovery policy. A
//! [`PortalClient`] owns platform-specific Portal/D-Bus objects and reports
//! only normalized bindings, errors, and events to this adapter.

use crate::portal_client::{PortalClient, PortalEvent};
use crate::session::{ShortcutBinding, ShortcutError, show_search_shortcut_id};
use crate::shortcut_state::{
    PortalDiagnosticError, PortalSnapshot, RecoveryBackoff, ShortcutState,
};
use flick_domain::UiIntent;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Trait for forwarding UI intents (show/hide search panel).
pub trait IntentSink: Send + Sync {
    fn send_intent(&self, intent: UiIntent);
}

/// The Portal GlobalShortcuts adapter.
///
/// It enforces the Phase 0 invariants: every Portal session attempts a bind at
/// most once; only an explicit configuration command can bind; automatic
/// recovery merely creates a session and lists shortcuts.
#[derive(Clone)]
pub struct PortalShortcutAdapter {
    state: Arc<Mutex<PortalInner>>,
    /// Serializes lifecycle operation startup, recovery task registration, and
    /// shutdown. State mutexes are never held across Portal client awaits.
    operation_lock: Arc<Mutex<()>>,
    lifecycle: CancellationToken,
    intent_sink: Arc<dyn IntentSink>,
    client: Arc<dyn PortalClient>,
}

struct PortalInner {
    state: ShortcutState,
    session_active: bool,
    shortcut_count: u32,
    last_error: Option<PortalDiagnosticError>,
    reconnect_attempt: Option<u32>,
    backoff: RecoveryBackoff,
    bind_attempted_in_current_session: bool,
    /// Changes whenever a session is opened or invalidated. Bind completion
    /// must match this generation before it may publish `Bound`.
    session_generation: u64,
    /// Changes whenever recovery is scheduled, manually invalidated, or shut
    /// down. Delayed tasks must match it before opening a session.
    recovery_generation: u64,
    event_consumer_started: bool,
    recovery_scheduled: bool,
    shutting_down: bool,
    event_task: Option<JoinHandle<()>>,
    recovery_task: Option<JoinHandle<()>>,
}

impl PortalInner {
    fn snapshot(&self) -> PortalSnapshot {
        PortalSnapshot {
            state: self.state.clone(),
            session_active: self.session_active,
            shortcut_count: self.shortcut_count,
            last_error: self.last_error.clone(),
            reconnect_attempt: self.reconnect_attempt,
        }
    }

    fn session_is_current(&self, generation: u64) -> bool {
        !self.shutting_down && self.session_generation == generation
    }

    fn recovery_is_current(&self, generation: u64) -> bool {
        !self.shutting_down && self.recovery_scheduled && self.recovery_generation == generation
    }
}

impl PortalShortcutAdapter {
    /// Builds an adapter that reports Portal unavailable until Task 3 wires a
    /// real ashpd client. It never simulates a successful binding.
    pub fn new(intent_sink: Arc<dyn IntentSink>) -> Self {
        Self::new_with_client(intent_sink, Arc::new(UnavailablePortalClient::new()))
    }

    /// Builds an adapter with an injected platform client.
    pub fn new_with_client(
        intent_sink: Arc<dyn IntentSink>,
        client: Arc<dyn PortalClient>,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(PortalInner {
                state: ShortcutState::Unavailable {
                    reason: "Portal client is not configured".into(),
                },
                session_active: false,
                shortcut_count: 0,
                last_error: None,
                reconnect_attempt: None,
                backoff: RecoveryBackoff::new(),
                bind_attempted_in_current_session: false,
                session_generation: 0,
                recovery_generation: 0,
                event_consumer_started: false,
                recovery_scheduled: false,
                shutting_down: false,
                event_task: None,
                recovery_task: None,
            })),
            operation_lock: Arc::new(Mutex::new(())),
            lifecycle: CancellationToken::new(),
            intent_sink,
            client,
        }
    }

    /// Returns the operator-visible shortcut state.
    pub async fn state(&self) -> ShortcutState {
        self.state.lock().await.state.clone()
    }

    /// Starts one Portal event consumer, creates a session, and lists bindings.
    /// It never opens authorization UI or attempts a bind.
    pub async fn initialize(&self) -> Result<PortalSnapshot, ShortcutError> {
        let _operation = self.operation_lock.lock().await;
        self.ensure_running().await?;
        self.ensure_event_consumer_locked().await?;
        self.open_session_and_list().await
    }

    /// Explicitly binds the show-search shortcut with the requested trigger.
    pub async fn configure_search_shortcut(
        &self,
        preferred_trigger: &str,
    ) -> Result<ShortcutBinding, ShortcutError> {
        let _operation = self.operation_lock.lock().await;
        self.ensure_running().await?;
        self.invalidate_pending_recovery_locked().await;
        self.ensure_event_consumer_locked().await?;

        let needs_fresh_session = {
            let inner = self.state.lock().await;
            inner.bind_attempted_in_current_session || !inner.session_active
        };
        if needs_fresh_session {
            self.open_session_and_list().await?;
        }

        let bind_session_generation = {
            let mut inner = self.state.lock().await;
            if self.lifecycle.is_cancelled() || inner.shutting_down {
                return Err(shutdown_error());
            }
            inner.state = ShortcutState::Configuring;
            // This must happen before awaiting Portal: BindShortcuts is
            // single-use for the session even when its request fails.
            inner.bind_attempted_in_current_session = true;
            inner.session_generation
        };

        let bind_result = tokio::select! {
            _ = self.lifecycle.cancelled() => return Err(shutdown_error()),
            result = self.client.bind_show_search(preferred_trigger) => result,
        };

        if !self.bind_session_is_current(bind_session_generation).await {
            return Err(ShortcutError::SessionError {
                reason: "Portal session closed before shortcut binding completed".into(),
            });
        }

        match bind_result {
            Ok(bindings) => {
                let Some(binding) = bindings
                    .iter()
                    .find(|binding| binding.id == show_search_shortcut_id())
                    .cloned()
                else {
                    let error = ShortcutError::RequestEnded {
                        reason: "BindShortcuts response did not include show-search".into(),
                    };
                    self.record_error_for_session(
                        "BindShortcuts",
                        &error,
                        bind_session_generation,
                        true,
                    )
                    .await;
                    return Err(error);
                };

                let mut inner = self.state.lock().await;
                if self.lifecycle.is_cancelled()
                    || !inner.session_is_current(bind_session_generation)
                    || !inner.session_active
                {
                    return Err(ShortcutError::SessionError {
                        reason: "Portal session closed before shortcut binding completed".into(),
                    });
                }
                inner.state = ShortcutState::Bound;
                inner.shortcut_count = bindings.len() as u32;
                info!("Portal show-search shortcut configured");
                Ok(binding)
            }
            Err(error @ ShortcutError::UserCancelled) => {
                let mut inner = self.state.lock().await;
                if self.lifecycle.is_cancelled()
                    || !inner.session_is_current(bind_session_generation)
                    || !inner.session_active
                {
                    return Err(ShortcutError::SessionError {
                        reason: "Portal session closed before shortcut binding completed".into(),
                    });
                }
                inner.state = ShortcutState::Denied;
                inner.last_error = Some(PortalDiagnosticError {
                    operation: "BindShortcuts".into(),
                    category: error.category().into(),
                    message: error.diagnostic_message(),
                });
                Err(error)
            }
            Err(error) => {
                self.record_error_for_session(
                    "BindShortcuts",
                    &error,
                    bind_session_generation,
                    true,
                )
                .await;
                Err(error)
            }
        }
    }

    /// Explicitly opens a new Portal session and lists current bindings.
    /// This method intentionally never calls `bind_show_search`.
    pub async fn reconnect(&self) -> Result<PortalSnapshot, ShortcutError> {
        let _operation = self.operation_lock.lock().await;
        self.ensure_running().await?;
        self.invalidate_pending_recovery_locked().await;
        self.ensure_event_consumer_locked().await?;
        self.open_session_and_list().await
    }

    /// Returns the current operator-facing diagnostics snapshot.
    pub async fn diagnostics(&self) -> PortalSnapshot {
        self.state.lock().await.snapshot()
    }

    /// Stops local event/recovery tasks and closes the platform session.
    ///
    /// Shutdown is announced and cancellation is signalled *before* waiting
    /// for `operation_lock`: a Portal call that currently owns that lock must
    /// be able to leave through its cancellation branch. Task creation and
    /// handle registration remain serialized by `operation_lock`, so after
    /// cancellation this method can safely collect every registered task.
    pub async fn shutdown(&self) {
        let should_close = {
            let mut inner = self.state.lock().await;
            if inner.shutting_down {
                false
            } else {
                inner.shutting_down = true;
                inner.session_active = false;
                inner.state = ShortcutState::Unavailable {
                    reason: "Portal shortcut adapter is shut down".into(),
                };
                inner.recovery_scheduled = false;
                inner.session_generation = inner.session_generation.wrapping_add(1);
                inner.recovery_generation = inner.recovery_generation.wrapping_add(1);
                true
            }
        };
        if !should_close {
            return;
        }

        self.lifecycle.cancel();

        // Lifecycle task creation and handle registration occur while normal
        // operations hold this lock. Cancellation above guarantees that a
        // currently blocked Portal call yields it promptly instead of making
        // shutdown wait for external Portal I/O forever.
        let _operation = self.operation_lock.lock().await;
        let (event_task, recovery_task) = {
            let mut inner = self.state.lock().await;
            (inner.event_task.take(), inner.recovery_task.take())
        };

        abort_and_join(event_task).await;
        abort_and_join(recovery_task).await;
        self.client.close_session().await;
    }

    async fn ensure_running(&self) -> Result<(), ShortcutError> {
        if self.lifecycle.is_cancelled() || self.state.lock().await.shutting_down {
            return Err(shutdown_error());
        }
        Ok(())
    }

    /// Called only while `operation_lock` is held.
    async fn invalidate_pending_recovery_locked(&self) {
        let task = {
            let mut inner = self.state.lock().await;
            inner.recovery_generation = inner.recovery_generation.wrapping_add(1);
            inner.recovery_scheduled = false;
            inner.recovery_task.take()
        };
        abort_and_join(task).await;
    }

    /// Called only while `operation_lock` is held.
    async fn ensure_event_consumer_locked(&self) -> Result<(), ShortcutError> {
        let mut inner = self.state.lock().await;
        if inner.event_consumer_started {
            return Ok(());
        }
        if self.lifecycle.is_cancelled() || inner.shutting_down {
            return Err(shutdown_error());
        }

        let Some(mut receiver) = self.client.take_event_receiver() else {
            let error = ShortcutError::Internal {
                message: "Portal client event receiver was already taken".into(),
            };
            inner.state = ShortcutState::Error {
                operation: "EventSubscription".into(),
                message: error.diagnostic_message(),
            };
            inner.last_error = Some(PortalDiagnosticError {
                operation: "EventSubscription".into(),
                category: error.category().into(),
                message: error.diagnostic_message(),
            });
            return Err(error);
        };

        inner.event_consumer_started = true;
        drop(inner);

        let adapter = self.clone();
        let cancellation = self.lifecycle.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation.cancelled() => break,
                    event = receiver.recv() => match event {
                        Some(event) => adapter.handle_portal_event(event).await,
                        None => break,
                    },
                }
            }
        });

        // `operation_lock` is still held, so shutdown cannot observe a
        // spawn-before-registration window.
        self.state.lock().await.event_task = Some(task);
        Ok(())
    }

    async fn open_session_and_list(&self) -> Result<PortalSnapshot, ShortcutError> {
        let session_generation = {
            let mut inner = self.state.lock().await;
            if self.lifecycle.is_cancelled() || inner.shutting_down {
                return Err(shutdown_error());
            }
            inner.session_generation = inner.session_generation.wrapping_add(1);
            inner.session_active = false;
            inner.session_generation
        };

        let list_result = tokio::select! {
            _ = self.lifecycle.cancelled() => return Err(shutdown_error()),
            result = self.client.open_session_and_list() => result,
        };

        match list_result {
            Ok(bindings) => {
                let has_show_search = bindings
                    .iter()
                    .any(|binding| binding.id == show_search_shortcut_id());
                let mut inner = self.state.lock().await;
                if self.lifecycle.is_cancelled() || !inner.session_is_current(session_generation) {
                    return Err(ShortcutError::SessionError {
                        reason: "Portal session changed while listing shortcuts".into(),
                    });
                }
                inner.state = if has_show_search {
                    ShortcutState::Bound
                } else {
                    ShortcutState::Unconfigured
                };
                inner.session_active = true;
                inner.shortcut_count = bindings.len() as u32;
                inner.reconnect_attempt = None;
                inner.bind_attempted_in_current_session = false;
                Ok(inner.snapshot())
            }
            Err(error) => {
                self.record_error_for_session(
                    "CreateSession/ListShortcuts",
                    &error,
                    session_generation,
                    true,
                )
                .await;
                Err(error)
            }
        }
    }

    async fn bind_session_is_current(&self, generation: u64) -> bool {
        let inner = self.state.lock().await;
        !self.lifecycle.is_cancelled()
            && inner.session_is_current(generation)
            && inner.session_active
    }

    async fn handle_portal_event(&self, event: PortalEvent) {
        match event {
            PortalEvent::Activated {
                shortcut_id,
                activation_token,
            } if shortcut_id == show_search_shortcut_id() => {
                if self.lifecycle.is_cancelled() || self.state.lock().await.shutting_down {
                    return;
                }
                debug!(shortcut_id = %shortcut_id, "Portal shortcut activated");
                self.intent_sink
                    .send_intent(UiIntent::ShowSearchPanel { activation_token });
            }
            PortalEvent::Activated { shortcut_id, .. } => {
                if !self.lifecycle.is_cancelled() {
                    debug!(shortcut_id = %shortcut_id, "Ignoring unrelated Portal shortcut activation");
                }
            }
            PortalEvent::SessionClosed => {
                self.begin_recovery("SessionClosed", "session", "Portal session closed")
                    .await;
            }
            PortalEvent::TransportError { message } => {
                self.begin_recovery("TransportError", "transport", &message)
                    .await;
            }
        }
    }

    /// Invalidates the current session immediately, then registers one delayed
    /// recovery while holding `operation_lock`. The immediate invalidation is
    /// what prevents an in-flight bind from later publishing `Bound`.
    async fn begin_recovery(&self, operation: &str, category: &str, message: &str) {
        let (recovery_generation, attempt, delay) = {
            let mut inner = self.state.lock().await;
            if self.lifecycle.is_cancelled() || inner.shutting_down || inner.recovery_scheduled {
                return;
            }

            let attempt = inner.reconnect_attempt.map_or(0, |current| current + 1);
            let delay = inner.backoff.delay_for_attempt(attempt);
            inner.session_generation = inner.session_generation.wrapping_add(1);
            inner.recovery_generation = inner.recovery_generation.wrapping_add(1);
            let recovery_generation = inner.recovery_generation;
            inner.state = ShortcutState::Recovering { attempt };
            inner.session_active = false;
            inner.bind_attempted_in_current_session = false;
            inner.reconnect_attempt = Some(attempt);
            inner.last_error = Some(PortalDiagnosticError {
                operation: operation.into(),
                category: category.into(),
                message: message.into(),
            });
            inner.recovery_scheduled = true;
            (recovery_generation, attempt, delay)
        };

        let _operation = self.operation_lock.lock().await;
        if !self.recovery_is_current(recovery_generation).await {
            return;
        }

        warn!(
            attempt,
            delay_secs = delay.as_secs(),
            operation,
            "Portal recovery scheduled"
        );
        let adapter = self.clone();
        let cancellation = self.lifecycle.clone();
        let task = tokio::spawn(async move {
            tokio::select! {
                _ = cancellation.cancelled() => return,
                _ = sleep(delay) => {}
            }

            let _operation = adapter.operation_lock.lock().await;
            if !adapter.recovery_is_current(recovery_generation).await {
                return;
            }

            let result = adapter.open_session_and_list().await;
            if let Err(error) = result
                && !adapter.lifecycle.is_cancelled()
            {
                warn!(error = %error, "Portal recovery open/list failed");
            }
            adapter.finish_recovery(recovery_generation).await;
        });

        // `operation_lock` is still held, so shutdown cannot observe a
        // spawn-before-registration window.
        let mut inner = self.state.lock().await;
        if inner.recovery_is_current(recovery_generation) && !self.lifecycle.is_cancelled() {
            inner.recovery_task = Some(task);
        } else {
            task.abort();
        }
    }

    async fn recovery_is_current(&self, generation: u64) -> bool {
        !self.lifecycle.is_cancelled() && self.state.lock().await.recovery_is_current(generation)
    }

    async fn finish_recovery(&self, generation: u64) {
        let mut inner = self.state.lock().await;
        if inner.recovery_is_current(generation) {
            inner.recovery_scheduled = false;
            inner.recovery_task = None;
        }
    }

    async fn record_error_for_session(
        &self,
        operation: &str,
        error: &ShortcutError,
        session_generation: u64,
        deactivate_session: bool,
    ) {
        let message = error.diagnostic_message();
        let mut inner = self.state.lock().await;
        if self.lifecycle.is_cancelled() || !inner.session_is_current(session_generation) {
            return;
        }
        if deactivate_session {
            inner.session_active = false;
        }
        inner.last_error = Some(PortalDiagnosticError {
            operation: operation.into(),
            category: error.category().into(),
            message: message.clone(),
        });
        inner.state = match error {
            ShortcutError::Unavailable { reason } => ShortcutState::Unavailable {
                reason: reason.clone(),
            },
            ShortcutError::UserCancelled => ShortcutState::Denied,
            _ => ShortcutState::Error {
                operation: operation.into(),
                message,
            },
        };
    }
}

async fn abort_and_join(task: Option<JoinHandle<()>>) {
    if let Some(task) = task {
        task.abort();
        let _ = task.await;
    }
}

fn shutdown_error() -> ShortcutError {
    ShortcutError::SessionError {
        reason: "Portal shortcut adapter is shutting down".into(),
    }
}

/// Deliberately non-functional client used until Task 3 supplies ashpd.
struct UnavailablePortalClient {
    receiver: StdMutex<Option<mpsc::UnboundedReceiver<PortalEvent>>>,
}

impl UnavailablePortalClient {
    fn new() -> Self {
        let (_sender, receiver) = mpsc::unbounded_channel();
        Self {
            receiver: StdMutex::new(Some(receiver)),
        }
    }
}

#[async_trait::async_trait]
impl PortalClient for UnavailablePortalClient {
    async fn open_session_and_list(&self) -> Result<Vec<ShortcutBinding>, ShortcutError> {
        Err(ShortcutError::Unavailable {
            reason: "No Portal client is configured; ashpd integration is pending".into(),
        })
    }

    async fn bind_show_search(
        &self,
        _preferred_trigger: &str,
    ) -> Result<Vec<ShortcutBinding>, ShortcutError> {
        Err(ShortcutError::Unavailable {
            reason: "No Portal client is configured; ashpd integration is pending".into(),
        })
    }

    async fn close_session(&self) {}

    fn take_event_receiver(&self) -> Option<mpsc::UnboundedReceiver<PortalEvent>> {
        self.receiver.lock().unwrap().take()
    }
}
