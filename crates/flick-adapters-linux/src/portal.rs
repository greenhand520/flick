//! Portal GlobalShortcuts lifecycle policy.
//!
//! This module owns state transitions, cancellation, and recovery policy. A
//! [`PortalClient`] owns platform-specific Portal/D-Bus objects and reports
//! only normalized bindings, errors, and events to this adapter.

use crate::portal_client::{PortalClient, PortalEvent, PortalSessionId};
use crate::session::{ShortcutBinding, ShortcutError, show_search_shortcut_id};
use crate::shortcut_state::{
    PortalDiagnosticError, PortalSnapshot, RecoveryBackoff, ShortcutState,
};
use flick_domain::UiIntent;
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex as StdMutex},
};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Trait for forwarding UI intents (show/hide search panel).
pub trait IntentSink: Send + Sync {
    fn send_intent(&self, intent: UiIntent);
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessedPortalEventKind {
    Activated,
    SessionClosed,
    TransportError,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProcessedPortalEvent {
    session_id: PortalSessionId,
    kind: ProcessedPortalEventKind,
}

#[cfg(test)]
impl From<&PortalEvent> for ProcessedPortalEvent {
    fn from(event: &PortalEvent) -> Self {
        match event {
            PortalEvent::Activated { session_id, .. } => Self {
                session_id: *session_id,
                kind: ProcessedPortalEventKind::Activated,
            },
            PortalEvent::SessionClosed { session_id } => Self {
                session_id: *session_id,
                kind: ProcessedPortalEventKind::SessionClosed,
            },
            PortalEvent::TransportError { session_id, .. } => Self {
                session_id: *session_id,
                kind: ProcessedPortalEventKind::TransportError,
            },
        }
    }
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
    /// Stable host identity of the session that may affect current state.
    active_portal_session_id: Option<PortalSessionId>,
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
    #[cfg(test)]
    processed_event_observer: Option<mpsc::UnboundedSender<ProcessedPortalEvent>>,
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
    /// Builds an adapter with a deliberately unavailable placeholder client.
    ///
    /// Task 7 will wire the real [`crate::portal_client::AshpdPortalClient`]
    /// into the daemon; this constructor never simulates a successful binding.
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
                active_portal_session_id: None,
                session_generation: 0,
                recovery_generation: 0,
                event_consumer_started: false,
                recovery_scheduled: false,
                shutting_down: false,
                event_task: None,
                recovery_task: None,
                #[cfg(test)]
                processed_event_observer: None,
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

    #[cfg(test)]
    async fn observe_processed_portal_events(
        &self,
    ) -> mpsc::UnboundedReceiver<ProcessedPortalEvent> {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.state.lock().await.processed_event_observer = Some(sender);
        receiver
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
                inner.active_portal_session_id = None;
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
            inner.active_portal_session_id = None;
            inner.session_generation
        };

        let list_result = tokio::select! {
            _ = self.lifecycle.cancelled() => return Err(shutdown_error()),
            result = self.client.open_session_and_list() => result,
        };

        match list_result {
            Ok(session) => {
                let has_show_search = session
                    .bindings
                    .iter()
                    .any(|binding| binding.id == show_search_shortcut_id());
                let published = {
                    let mut inner = self.state.lock().await;
                    if self.lifecycle.is_cancelled()
                        || !inner.session_is_current(session_generation)
                    {
                        false
                    } else {
                        inner.state = if has_show_search {
                            ShortcutState::Bound
                        } else {
                            ShortcutState::Unconfigured
                        };
                        inner.session_active = true;
                        inner.shortcut_count = session.bindings.len() as u32;
                        inner.active_portal_session_id = Some(session.id);
                        inner.reconnect_attempt = None;
                        inner.bind_attempted_in_current_session = false;
                        true
                    }
                };
                if !published {
                    self.client.close_session().await;
                    return Err(ShortcutError::SessionError {
                        reason: "Portal session changed while listing shortcuts".into(),
                    });
                }

                let buffered_events = tokio::select! {
                    _ = self.lifecycle.cancelled() => {
                        self.client.close_session().await;
                        return Err(shutdown_error());
                    }
                    result = self.client.activate_session_events(session.id) => result,
                };
                let buffered_events = match buffered_events {
                    Ok(events) => events,
                    Err(error) => {
                        self.record_error_for_session(
                            "ActivateSessionEvents",
                            &error,
                            session_generation,
                            true,
                        )
                        .await;
                        self.client.close_session().await;
                        return Err(error);
                    }
                };

                // Forwarding was gated until the state/id publication above.
                // Process anything already observed by the subscribed streams
                // while this lifecycle operation still owns `operation_lock`,
                // so a pre-publication close cannot escape as a transient
                // Bound/Unconfigured result.
                for event in buffered_events {
                    self.handle_portal_event_while_operation_locked(event).await;
                }

                let snapshot = {
                    let inner = self.state.lock().await;
                    if self.lifecycle.is_cancelled()
                        || !inner.session_is_current(session_generation)
                        || !inner.session_active
                        || inner.active_portal_session_id != Some(session.id)
                    {
                        None
                    } else {
                        Some(inner.snapshot())
                    }
                };
                if let Some(snapshot) = snapshot {
                    Ok(snapshot)
                } else {
                    self.client.close_session().await;
                    Err(ShortcutError::SessionError {
                        reason: "Portal session closed while activating shortcut events".into(),
                    })
                }
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
        self.handle_portal_event_inner(event, false).await;
    }

    /// Handles events returned by the client activation handshake while the
    /// caller already owns `operation_lock`. Recovery scheduling must not try
    /// to acquire that non-reentrant lock again.
    async fn handle_portal_event_while_operation_locked(&self, event: PortalEvent) {
        self.handle_portal_event_inner(event, true).await;
    }

    async fn handle_portal_event_inner(&self, event: PortalEvent, operation_lock_held: bool) {
        #[cfg(test)]
        let processed_event = ProcessedPortalEvent::from(&event);

        match event {
            PortalEvent::Activated {
                session_id,
                shortcut_id,
                ..
            } => {
                if self.event_matches_active_session(session_id).await {
                    if shortcut_id == show_search_shortcut_id() {
                        debug!(shortcut_id = %shortcut_id, "Portal shortcut activated");
                        // Portal activation tokens are never surfaced to Flick UI.
                        self.intent_sink.send_intent(UiIntent::ShowSearchPanel {
                            activation_token: None,
                        });
                    } else {
                        debug!(shortcut_id = %shortcut_id, "Ignoring unrelated Portal shortcut activation");
                    }
                }
            }
            PortalEvent::SessionClosed { session_id } => {
                if operation_lock_held {
                    self.begin_recovery_for_session_while_operation_locked(
                        session_id,
                        "SessionClosed",
                        "session",
                        "Portal session closed",
                    )
                    .await;
                } else {
                    self.begin_recovery_for_session(
                        session_id,
                        "SessionClosed",
                        "session",
                        "Portal session closed",
                    )
                    .await;
                }
            }
            PortalEvent::TransportError {
                session_id,
                message,
            } => {
                if operation_lock_held {
                    self.begin_recovery_for_session_while_operation_locked(
                        session_id,
                        "TransportError",
                        "transport",
                        &message,
                    )
                    .await;
                } else {
                    self.begin_recovery_for_session(
                        session_id,
                        "TransportError",
                        "transport",
                        &message,
                    )
                    .await;
                }
            }
        }

        #[cfg(test)]
        self.notify_portal_event_processed(processed_event).await;
    }

    #[cfg(test)]
    async fn notify_portal_event_processed(&self, event: ProcessedPortalEvent) {
        let observer = self.state.lock().await.processed_event_observer.clone();
        if let Some(observer) = observer {
            let _ = observer.send(event);
        }
    }

    async fn event_matches_active_session(&self, session_id: PortalSessionId) -> bool {
        let inner = self.state.lock().await;
        !self.lifecycle.is_cancelled()
            && !inner.shutting_down
            && inner.session_active
            && inner.active_portal_session_id == Some(session_id)
    }

    /// Invalidates the current session immediately, then registers one delayed
    /// recovery. The invalidation is what prevents an in-flight bind from later
    /// publishing `Bound`.
    async fn begin_recovery_for_session(
        &self,
        session_id: PortalSessionId,
        operation: &str,
        category: &str,
        message: &str,
    ) {
        let Some(recovery) = self
            .prepare_recovery_for_session(session_id, operation, category, message)
            .await
        else {
            return;
        };

        let _operation = self.operation_lock.lock().await;
        self.schedule_recovery_while_operation_locked(recovery, operation)
            .await;
    }

    /// Variant for activation-handshake events when the caller already holds
    /// `operation_lock`.
    async fn begin_recovery_for_session_while_operation_locked(
        &self,
        session_id: PortalSessionId,
        operation: &str,
        category: &str,
        message: &str,
    ) {
        let Some(recovery) = self
            .prepare_recovery_for_session(session_id, operation, category, message)
            .await
        else {
            return;
        };
        self.schedule_recovery_while_operation_locked(recovery, operation)
            .await;
    }

    /// Performs the identity check and every recovery-visible state mutation
    /// in one state critical section.
    async fn prepare_recovery_for_session(
        &self,
        session_id: PortalSessionId,
        operation: &str,
        category: &str,
        message: &str,
    ) -> Option<(u64, u32, std::time::Duration)> {
        let mut inner = self.state.lock().await;
        if self.lifecycle.is_cancelled()
            || inner.shutting_down
            || inner.recovery_scheduled
            || !inner.session_active
            || inner.active_portal_session_id != Some(session_id)
        {
            return None;
        }

        let attempt = inner.reconnect_attempt.map_or(0, |current| current + 1);
        let delay = inner.backoff.delay_for_attempt(attempt);
        inner.session_generation = inner.session_generation.wrapping_add(1);
        inner.recovery_generation = inner.recovery_generation.wrapping_add(1);
        let recovery_generation = inner.recovery_generation;
        inner.state = ShortcutState::Recovering { attempt };
        inner.session_active = false;
        inner.active_portal_session_id = None;
        inner.bind_attempted_in_current_session = false;
        inner.reconnect_attempt = Some(attempt);
        inner.last_error = Some(PortalDiagnosticError {
            operation: operation.into(),
            category: category.into(),
            message: message.into(),
        });
        inner.recovery_scheduled = true;
        Some((recovery_generation, attempt, delay))
    }

    /// Called only while `operation_lock` is held.
    fn schedule_recovery_while_operation_locked<'a>(
        &'a self,
        (recovery_generation, attempt, delay): (u64, u32, std::time::Duration),
        operation: &'a str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
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

                let result = Box::pin(adapter.open_session_and_list()).await;
                if let Err(error) = result
                    && !adapter.lifecycle.is_cancelled()
                {
                    warn!(
                        error_category = error.category(),
                        "Portal recovery open/list failed"
                    );
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
        })
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
            inner.active_portal_session_id = None;
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

/// Deliberately non-functional client used by [`PortalShortcutAdapter::new`]
/// until Task 7 wires [`crate::portal_client::AshpdPortalClient`] into the daemon.
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
    async fn open_session_and_list(
        &self,
    ) -> Result<crate::portal_client::PortalSession, ShortcutError> {
        Err(ShortcutError::Unavailable {
            reason: "No Portal client is configured; ashpd integration is pending".into(),
        })
    }

    async fn activate_session_events(
        &self,
        _session_id: PortalSessionId,
    ) -> Result<Vec<PortalEvent>, ShortcutError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portal_client::PortalSession;
    use crate::session::show_search_shortcut_description;
    use async_trait::async_trait;
    use std::collections::VecDeque;
    use std::sync::Mutex as StdMutex;

    struct TestIntentSink {
        intents: StdMutex<Vec<UiIntent>>,
    }

    impl TestIntentSink {
        fn new() -> Self {
            Self {
                intents: StdMutex::new(Vec::new()),
            }
        }

        fn intents(&self) -> Vec<UiIntent> {
            self.intents.lock().unwrap().clone()
        }
    }

    impl IntentSink for TestIntentSink {
        fn send_intent(&self, intent: UiIntent) {
            self.intents.lock().unwrap().push(intent);
        }
    }

    struct TestPortalClient {
        sessions: StdMutex<VecDeque<PortalSession>>,
        open_and_list_calls: StdMutex<u32>,
        bind_calls: StdMutex<u32>,
        activated_session_ids: StdMutex<Vec<PortalSessionId>>,
        buffered_before_activation: StdMutex<VecDeque<PortalEvent>>,
        event_sender: mpsc::UnboundedSender<PortalEvent>,
        event_receiver: StdMutex<Option<mpsc::UnboundedReceiver<PortalEvent>>>,
    }

    impl TestPortalClient {
        fn new(sessions: impl IntoIterator<Item = PortalSession>) -> Self {
            let (event_sender, event_receiver) = mpsc::unbounded_channel();
            Self {
                sessions: StdMutex::new(sessions.into_iter().collect()),
                open_and_list_calls: StdMutex::new(0),
                bind_calls: StdMutex::new(0),
                activated_session_ids: StdMutex::new(Vec::new()),
                buffered_before_activation: StdMutex::new(VecDeque::new()),
                event_sender,
                event_receiver: StdMutex::new(Some(event_receiver)),
            }
        }

        fn emit(&self, event: PortalEvent) {
            self.event_sender.send(event).unwrap();
        }

        fn open_and_list_calls(&self) -> u32 {
            *self.open_and_list_calls.lock().unwrap()
        }

        fn bind_calls(&self) -> u32 {
            *self.bind_calls.lock().unwrap()
        }

        fn activated_session_ids(&self) -> Vec<PortalSessionId> {
            self.activated_session_ids.lock().unwrap().clone()
        }

        /// Models a Portal signal that the subscribed listener observed before
        /// Flick confirmed publication of the returned host session id.
        fn buffer_before_activation(&self, event: PortalEvent) {
            self.buffered_before_activation
                .lock()
                .unwrap()
                .push_back(event);
        }
    }

    #[async_trait]
    impl PortalClient for TestPortalClient {
        async fn open_session_and_list(&self) -> Result<PortalSession, ShortcutError> {
            *self.open_and_list_calls.lock().unwrap() += 1;
            self.sessions
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| ShortcutError::SessionError {
                    reason: "test session queue exhausted".into(),
                })
        }

        async fn activate_session_events(
            &self,
            session_id: PortalSessionId,
        ) -> Result<Vec<PortalEvent>, ShortcutError> {
            self.activated_session_ids.lock().unwrap().push(session_id);
            Ok(self
                .buffered_before_activation
                .lock()
                .unwrap()
                .drain(..)
                .collect())
        }

        async fn bind_show_search(
            &self,
            _preferred_trigger: &str,
        ) -> Result<Vec<ShortcutBinding>, ShortcutError> {
            *self.bind_calls.lock().unwrap() += 1;
            Ok(Vec::new())
        }

        async fn close_session(&self) {}

        fn take_event_receiver(&self) -> Option<mpsc::UnboundedReceiver<PortalEvent>> {
            self.event_receiver.lock().unwrap().take()
        }
    }

    fn show_search_binding(trigger: &str) -> ShortcutBinding {
        ShortcutBinding {
            id: show_search_shortcut_id().into(),
            description: show_search_shortcut_description().into(),
            preferred_trigger: trigger.into(),
            actual_trigger: Some(trigger.into()),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn buffered_b_close_before_event_activation_cannot_publish_bound() {
        let session_a = PortalSessionId::new(51);
        let session_b = PortalSessionId::new(52);
        let client = Arc::new(TestPortalClient::new([
            PortalSession {
                id: session_a,
                bindings: Vec::new(),
            },
            PortalSession {
                id: session_b,
                bindings: vec![show_search_binding("Ctrl+Space")],
            },
        ]));
        let intent_sink = Arc::new(TestIntentSink::new());
        let adapter = PortalShortcutAdapter::new_with_client(intent_sink.clone(), client.clone());

        adapter.initialize().await.unwrap();
        // The fake has subscribed to B already, but keeps this signal private
        // until the adapter publishes B and explicitly activates forwarding.
        client.buffer_before_activation(PortalEvent::SessionClosed {
            session_id: session_b,
        });

        let reconnect =
            tokio::time::timeout(std::time::Duration::from_secs(1), adapter.reconnect())
                .await
                .expect("event activation handshake did not complete");
        assert!(matches!(reconnect, Err(ShortcutError::SessionError { .. })));

        let snapshot = adapter.diagnostics().await;
        assert_eq!(snapshot.state, ShortcutState::Recovering { attempt: 0 });
        assert!(!snapshot.session_active);
        assert_eq!(client.open_and_list_calls(), 2);
        assert_eq!(client.bind_calls(), 0);
        assert_eq!(client.activated_session_ids(), vec![session_a, session_b]);
        assert!(intent_sink.intents().is_empty());
    }

    #[tokio::test]
    async fn stale_session_events_after_b_reconnect_cannot_schedule_recovery() {
        let session_a = PortalSessionId::new(41);
        let session_b = PortalSessionId::new(42);
        let client = Arc::new(TestPortalClient::new([
            PortalSession {
                id: session_a,
                bindings: Vec::new(),
            },
            PortalSession {
                id: session_b,
                bindings: vec![show_search_binding("Ctrl+Space")],
            },
        ]));
        let intent_sink = Arc::new(TestIntentSink::new());
        let adapter = PortalShortcutAdapter::new_with_client(intent_sink.clone(), client.clone());

        adapter.initialize().await.unwrap();
        let snapshot = adapter.reconnect().await.unwrap();
        assert_eq!(snapshot.state, ShortcutState::Bound);

        let mut processed_events = adapter.observe_processed_portal_events().await;
        client.emit(PortalEvent::SessionClosed {
            session_id: session_a,
        });
        client.emit(PortalEvent::TransportError {
            session_id: session_a,
            message: "old session listener ended".into(),
        });
        client.emit(PortalEvent::Activated {
            session_id: session_a,
            shortcut_id: show_search_shortcut_id().into(),
            activation_token: None,
        });
        client.emit(PortalEvent::Activated {
            session_id: session_b,
            shortcut_id: show_search_shortcut_id().into(),
            activation_token: None,
        });

        let target = ProcessedPortalEvent {
            session_id: session_b,
            kind: ProcessedPortalEventKind::Activated,
        };
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while let Some(event) = processed_events.recv().await {
                if event == target {
                    return;
                }
            }
            panic!("Portal event receiver closed before B activation was processed");
        })
        .await
        .expect("B activation was not processed");

        let after = adapter.diagnostics().await;
        assert_eq!(after.state, ShortcutState::Bound);
        assert!(after.session_active);
        assert_eq!(after.shortcut_count, 1);
        assert_eq!(client.open_and_list_calls(), 2);
        assert_eq!(client.bind_calls(), 0);
        assert!(matches!(
            intent_sink.intents().as_slice(),
            [UiIntent::ShowSearchPanel {
                activation_token: None
            }]
        ));
    }
}
