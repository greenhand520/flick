//! Typed boundary between Portal lifecycle policy and a platform client.

use crate::session::{
    ShortcutBinding, ShortcutError, show_search_shortcut_description, show_search_shortcut_id,
};
use ashpd::desktop::{
    ResponseError, Session,
    global_shortcuts::{Activated, GlobalShortcuts, NewShortcut, Shortcut},
};
use futures_util::{Stream, StreamExt};
use std::{
    pin::Pin,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::{
    sync::{Mutex, mpsc},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use tracing::debug;

/// Stable host-level identity for a Portal shortcut session.
///
/// It is deliberately unrelated to a D-Bus object path, so no path crosses the
/// platform/client boundary or appears in adapter policy and diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PortalSessionId(u64);

impl PortalSessionId {
    /// Creates a stable session identity. Primarily useful for host fakes.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}

/// Bindings listed from one newly opened Portal shortcut session.
#[derive(Debug, Clone)]
pub struct PortalSession {
    pub id: PortalSessionId,
    pub bindings: Vec<ShortcutBinding>,
}

/// Normalized events emitted by a Portal client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortalEvent {
    /// A shortcut registered by the identified Portal session was activated.
    Activated {
        session_id: PortalSessionId,
        shortcut_id: String,
        activation_token: Option<String>,
    },
    /// The identified Portal session was closed by the desktop or Portal.
    SessionClosed { session_id: PortalSessionId },
    /// The Portal transport stopped delivering events for the identified session.
    TransportError {
        session_id: PortalSessionId,
        message: String,
    },
}

/// Platform-facing Portal client.
///
/// The client owns platform-specific session objects. The adapter only sees
/// normalized bindings, errors, session identities, and events, keeping Portal
/// implementation details out of the shortcut policy and diagnostics model.
#[async_trait::async_trait]
pub trait PortalClient: Send + Sync {
    /// Creates a session and lists its existing shortcut bindings.
    async fn open_session_and_list(&self) -> Result<PortalSession, ShortcutError>;

    /// Enables forwarding for an atomically published session and returns any
    /// normalized events buffered while forwarding was gated.
    async fn activate_session_events(
        &self,
        session_id: PortalSessionId,
    ) -> Result<Vec<PortalEvent>, ShortcutError>;

    /// Makes the one explicit bind attempt for the show-search shortcut.
    async fn bind_show_search(
        &self,
        preferred_trigger: &str,
    ) -> Result<Vec<ShortcutBinding>, ShortcutError>;

    /// Closes the currently active platform session, if any.
    async fn close_session(&self);

    /// Returns the one receiver supplying normalized Portal events.
    ///
    /// Returning `None` signals an internal client lifecycle/programming error.
    fn take_event_receiver(&self) -> Option<tokio::sync::mpsc::UnboundedReceiver<PortalEvent>>;
}

type ActivatedStream = Pin<Box<dyn Stream<Item = Activated> + Send>>;
type ClosedStream = Pin<Box<dyn Stream<Item = zbus::Message> + Send>>;

/// Real xdg-desktop-portal GlobalShortcuts client backed by ashpd.
///
/// ashpd and zbus values are deliberately contained in this implementation;
/// callers interact only through [`PortalClient`]'s normalized types.
pub struct AshpdPortalClient {
    state: Arc<Mutex<AshpdPortalState>>,
    /// Serializes complete public Portal operations. In particular, replacing
    /// a session cannot race a bind or close operation.
    operation_lock: Mutex<()>,
    event_tx: mpsc::UnboundedSender<PortalEvent>,
    event_rx: StdMutex<Option<mpsc::UnboundedReceiver<PortalEvent>>>,
}

struct AshpdPortalState {
    /// A session that has been created remotely but is still preparing its
    /// listener streams and ListShortcuts response. Keeping it client-owned
    /// makes cancellation-safe close possible before it becomes active.
    pending: Option<PendingSession>,
    active: Option<ActiveSession>,
    next_session_id: u64,
}

struct PendingSession {
    session: Arc<Session<GlobalShortcuts>>,
}

struct ActiveSession {
    id: PortalSessionId,
    /// Owns the global-shortcuts proxy for all calls made against this session.
    portal: Arc<GlobalShortcuts>,
    /// Owns the ashpd session object until it is replaced or closed.
    session: Arc<Session<GlobalShortcuts>>,
    session_handle: String,
    listener_cancel: CancellationToken,
    listener_tasks: Vec<JoinHandle<()>>,
    /// Listener streams are subscribed immediately, but their normalized
    /// events remain here until the adapter publishes this id as active.
    events_activated: bool,
    pending_events: Vec<PortalEvent>,
}

#[derive(Clone)]
struct ListenerContext {
    state: Arc<Mutex<AshpdPortalState>>,
    event_tx: mpsc::UnboundedSender<PortalEvent>,
    session_id: PortalSessionId,
    session_handle: String,
    cancellation: CancellationToken,
    transport_sent: Arc<AtomicBool>,
}

impl AshpdPortalClient {
    /// Creates a Portal client with one normalized event receiver.
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        Self {
            state: Arc::new(Mutex::new(AshpdPortalState {
                pending: None,
                active: None,
                next_session_id: 1,
            })),
            operation_lock: Mutex::new(()),
            event_tx,
            event_rx: StdMutex::new(Some(event_rx)),
        }
    }

    /// Takes every client-owned remote session before closing it outside the
    /// state mutex. `pending` and `active` are mutually exclusive during a
    /// normal open, but both are taken defensively so replacement and shutdown
    /// cannot leave a remote session behind.
    async fn take_registered_sessions(&self) -> (Option<PendingSession>, Option<ActiveSession>) {
        let mut state = self.state.lock().await;
        (state.pending.take(), state.active.take())
    }

    async fn close_registered_sessions(&self) {
        let (pending, active) = self.take_registered_sessions().await;
        if let Some(active) = active {
            close_active_session(active).await;
        }
        if let Some(pending) = pending {
            close_pending_session(pending).await;
        }
    }

    /// Registers the session immediately after CreateSession succeeds. No
    /// signal subscription or ListShortcuts await may happen before this
    /// succeeds, because shutdown must be able to find and close this handle
    /// if the open future is cancelled.
    async fn register_pending_session(
        &self,
        session: Arc<Session<GlobalShortcuts>>,
    ) -> Result<(), ShortcutError> {
        let mut state = self.state.lock().await;
        if state.pending.is_some() || state.active.is_some() {
            return Err(ShortcutError::SessionError {
                reason: "Portal session replacement was not serialized".into(),
            });
        }
        state.pending = Some(PendingSession { session });
        Ok(())
    }

    /// Takes a still-pending session only when it is the one being prepared.
    /// This makes failure cleanup idempotent: after shutdown/replacement has
    /// taken it, this returns `None` rather than issuing a second Close.
    async fn take_matching_pending_session(
        &self,
        session: &Arc<Session<GlobalShortcuts>>,
    ) -> Option<PendingSession> {
        let mut state = self.state.lock().await;
        if state
            .pending
            .as_ref()
            .is_some_and(|pending| Arc::ptr_eq(&pending.session, session))
        {
            state.pending.take()
        } else {
            None
        }
    }

    async fn close_matching_pending_session(
        &self,
        session: &Arc<Session<GlobalShortcuts>>,
    ) -> bool {
        let Some(pending) = self.take_matching_pending_session(session).await else {
            return false;
        };
        close_pending_session(pending).await;
        true
    }

    /// Transitions a registered pending session into active listener ownership.
    /// The streams are already subscribed by the caller; forwarding remains
    /// gated until the adapter acknowledges publication with
    /// `activate_session_events`.
    async fn install_listeners(
        &self,
        portal: GlobalShortcuts,
        session: Arc<Session<GlobalShortcuts>>,
        session_handle: String,
        activated_stream: ActivatedStream,
        closed_stream: ClosedStream,
    ) -> Result<PortalSessionId, ShortcutError> {
        let listener_cancel = CancellationToken::new();
        let session_id = {
            let mut state = self.state.lock().await;
            if state.active.is_some() {
                None
            } else if let Some(pending) = state.pending.take() {
                if !Arc::ptr_eq(&pending.session, &session) {
                    state.pending = Some(pending);
                    None
                } else {
                    let session_id = PortalSessionId(state.next_session_id);
                    state.next_session_id = state.next_session_id.wrapping_add(1);
                    state.active = Some(ActiveSession {
                        id: session_id,
                        portal: Arc::new(portal),
                        session: pending.session,
                        session_handle: session_handle.clone(),
                        listener_cancel: listener_cancel.clone(),
                        listener_tasks: Vec::new(),
                        events_activated: false,
                        pending_events: Vec::new(),
                    });
                    Some(session_id)
                }
            } else {
                None
            }
        };

        let Some(session_id) = session_id else {
            if !self.close_matching_pending_session(&session).await {
                close_unregistered_session(session.as_ref()).await;
            }
            return Err(ShortcutError::SessionError {
                reason: "Portal session replacement was not serialized".into(),
            });
        };

        let listener = ListenerContext {
            state: self.state.clone(),
            event_tx: self.event_tx.clone(),
            session_id,
            session_handle,
            cancellation: listener_cancel.clone(),
            transport_sent: Arc::new(AtomicBool::new(false)),
        };
        let closed_sent = Arc::new(AtomicBool::new(false));
        let activated_task = tokio::spawn(forward_activated(activated_stream, listener.clone()));
        let closed_task = tokio::spawn(forward_closed(closed_stream, listener, closed_sent));
        let tasks = vec![activated_task, closed_task];

        let is_current = self
            .state
            .lock()
            .await
            .active
            .as_ref()
            .is_some_and(|active| active.id == session_id)
            && !listener_cancel.is_cancelled();
        if is_current {
            let mut state = self.state.lock().await;
            let active = state
                .active
                .as_mut()
                .expect("active session disappeared before listener registration");
            active.listener_tasks = tasks;
            return Ok(session_id);
        }

        listener_cancel.cancel();
        abort_and_join_all(tasks).await;
        let stale = {
            let mut state = self.state.lock().await;
            if state
                .active
                .as_ref()
                .is_some_and(|active| active.id == session_id)
            {
                state.active.take()
            } else {
                None
            }
        };
        if let Some(stale) = stale {
            close_active_session(stale).await;
        }
        Err(ShortcutError::SessionError {
            reason: "Portal session changed while installing listeners".into(),
        })
    }
}

impl Default for AshpdPortalClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl PortalClient for AshpdPortalClient {
    async fn open_session_and_list(&self) -> Result<PortalSession, ShortcutError> {
        let _operation = self.operation_lock.lock().await;
        self.close_registered_sessions().await;

        let portal = GlobalShortcuts::new()
            .await
            .map_err(|error| unavailable_error("GlobalShortcuts", error))?;
        let session = Arc::new(
            portal
                .create_session(Default::default())
                .await
                .map_err(|error| unavailable_error("CreateSession", error))?,
        );
        if let Err(error) = self.register_pending_session(session.clone()).await {
            close_unregistered_session(session.as_ref()).await;
            return Err(error);
        }

        let preparation = async {
            let session_handle = session_handle(session.as_ref())?;

            // Both signals are registered before any listener task is spawned
            // and before ListShortcuts makes this session visible to Flick.
            let activated_stream =
                session_error("ReceiveActivated", portal.receive_activated().await)?;
            let closed_stream = session_closed_stream(&portal, &session_handle).await?;

            let request = session_error(
                "ListShortcuts",
                portal
                    .list_shortcuts(session.as_ref(), Default::default())
                    .await,
            )?;
            let response = session_error("ListShortcuts", request.response())?;
            Ok::<_, ShortcutError>((
                session_handle,
                Box::pin(activated_stream) as ActivatedStream,
                closed_stream,
                shortcut_bindings(response.shortcuts()),
            ))
        }
        .await;

        match preparation {
            Ok((session_handle, activated_stream, closed_stream, bindings)) => {
                let id = self
                    .install_listeners(
                        portal,
                        session,
                        session_handle,
                        activated_stream,
                        closed_stream,
                    )
                    .await?;
                Ok(PortalSession { id, bindings })
            }
            Err(error) => {
                if !self.close_matching_pending_session(&session).await {
                    close_unregistered_session(session.as_ref()).await;
                }
                Err(error)
            }
        }
    }

    async fn activate_session_events(
        &self,
        session_id: PortalSessionId,
    ) -> Result<Vec<PortalEvent>, ShortcutError> {
        let _operation = self.operation_lock.lock().await;
        let mut state = self.state.lock().await;
        let Some(active) = state.active.as_mut() else {
            return Err(ShortcutError::Unavailable {
                reason: "Portal shortcut session is not initialized".into(),
            });
        };
        if active.id != session_id {
            return Err(ShortcutError::SessionError {
                reason: "Portal shortcut session changed before event activation".into(),
            });
        }
        if active.events_activated {
            return Ok(Vec::new());
        }

        active.events_activated = true;
        Ok(std::mem::take(&mut active.pending_events))
    }

    async fn bind_show_search(
        &self,
        preferred_trigger: &str,
    ) -> Result<Vec<ShortcutBinding>, ShortcutError> {
        let _operation = self.operation_lock.lock().await;
        let (portal, session) = {
            let state = self.state.lock().await;
            let Some(active) = state.active.as_ref() else {
                return Err(ShortcutError::Unavailable {
                    reason: "Portal shortcut session is not initialized".into(),
                });
            };
            (active.portal.clone(), active.session.clone())
        };

        let shortcut = NewShortcut::new(
            show_search_shortcut_id(),
            show_search_shortcut_description(),
        )
        .preferred_trigger(Some(preferred_trigger));
        let request = portal
            .bind_shortcuts(session.as_ref(), &[shortcut], None, Default::default())
            .await
            .map_err(bind_error)?;
        let response = request.response().map_err(bind_error)?;
        validate_bind_response(shortcut_bindings(response.shortcuts()))
    }

    async fn close_session(&self) {
        let _operation = self.operation_lock.lock().await;
        self.close_registered_sessions().await;
    }

    fn take_event_receiver(&self) -> Option<mpsc::UnboundedReceiver<PortalEvent>> {
        self.event_rx.lock().unwrap().take()
    }
}

async fn session_closed_stream(
    portal: &GlobalShortcuts,
    session_handle: &str,
) -> Result<ClosedStream, ShortcutError> {
    // ashpd 0.13.13's `Session::receive_closed` conservatively captures its
    // receiver lifetime. Use the same session object path and D-Bus connection
    // to create a private listener proxy, while retaining the ashpd Session as
    // the authoritative object used for bind and close operations.
    let session_proxy = zbus::Proxy::new_owned(
        portal.connection().clone(),
        "org.freedesktop.portal.Desktop",
        session_handle.to_owned(),
        "org.freedesktop.portal.Session",
    )
    .await
    .map_err(|error| session_zbus_error("ReceiveClosed", error))?;
    let stream = session_proxy
        .receive_signal("Closed")
        .await
        .map_err(|error| session_zbus_error("ReceiveClosed", error))?;
    Ok(Box::pin(stream))
}

async fn forward_activated(mut stream: ActivatedStream, listener: ListenerContext) {
    loop {
        tokio::select! {
            _ = listener.cancellation.cancelled() => return,
            event = stream.next() => match event {
                Some(event) => {
                    if event.session_handle().as_str() == listener.session_handle {
                        queue_or_forward_event(
                            &listener,
                            PortalEvent::Activated {
                                session_id: listener.session_id,
                                shortcut_id: event.shortcut_id().to_owned(),
                                // Portal activation tokens are intentionally not exposed.
                                activation_token: None,
                            },
                        ).await;
                    }
                }
                None => {
                    send_transport_once(&listener, "Activated listener ended").await;
                    return;
                }
            }
        }
    }
}

async fn forward_closed(
    mut stream: ClosedStream,
    listener: ListenerContext,
    closed_sent: Arc<AtomicBool>,
) {
    tokio::select! {
        _ = listener.cancellation.cancelled() => {}
        event = stream.next() => match event {
            Some(_) => {
                if !closed_sent.swap(true, Ordering::AcqRel) {
                    queue_or_forward_event(
                        &listener,
                        PortalEvent::SessionClosed {
                            session_id: listener.session_id,
                        },
                    ).await;
                    listener.cancellation.cancel();
                }
            }
            None => {
                send_transport_once(&listener, "Closed listener ended").await;
            }
        }
    }
}

/// Appends an event while the adapter has not yet published this session. Once
/// the host acknowledgement is received, live events flow to the adapter.
async fn queue_or_forward_event(listener: &ListenerContext, event: PortalEvent) {
    if listener.cancellation.is_cancelled() {
        return;
    }

    let should_forward = {
        let mut state = listener.state.lock().await;
        let Some(active) = state.active.as_mut() else {
            return;
        };
        if active.id != listener.session_id || active.session_handle != listener.session_handle {
            return;
        }
        if active.events_activated {
            true
        } else {
            active.pending_events.push(event.clone());
            false
        }
    };

    if should_forward {
        let _ = listener.event_tx.send(event);
    }
}

async fn send_transport_once(listener: &ListenerContext, message: &str) {
    if !listener.cancellation.is_cancelled()
        && !listener.transport_sent.swap(true, Ordering::AcqRel)
    {
        queue_or_forward_event(
            listener,
            PortalEvent::TransportError {
                session_id: listener.session_id,
                message: message.into(),
            },
        )
        .await;
    }
}

async fn abort_and_join_all(tasks: Vec<JoinHandle<()>>) {
    for task in tasks {
        task.abort();
        let _ = task.await;
    }
}

async fn close_active_session(mut active: ActiveSession) {
    active.listener_cancel.cancel();
    abort_and_join_all(std::mem::take(&mut active.listener_tasks)).await;
    if active.session.close().await.is_err() {
        debug!(
            operation = "CloseSession",
            error_category = "session",
            "Portal session close failed"
        );
    }
}

async fn close_pending_session(pending: PendingSession) {
    close_unregistered_session(pending.session.as_ref()).await;
}

async fn close_unregistered_session(session: &Session<GlobalShortcuts>) {
    if session.close().await.is_err() {
        debug!(
            operation = "CloseSession",
            error_category = "session",
            "Portal session close failed"
        );
    }
}

fn shortcut_bindings(shortcuts: &[Shortcut]) -> Vec<ShortcutBinding> {
    shortcuts
        .iter()
        .map(|shortcut| ShortcutBinding {
            id: shortcut.id().to_owned(),
            description: shortcut.description().to_owned(),
            preferred_trigger: String::new(),
            actual_trigger: Some(shortcut.trigger_description().to_owned()),
        })
        .collect()
}

fn session_handle(session: &Session<GlobalShortcuts>) -> Result<String, ShortcutError> {
    // ashpd 0.13.13 does not expose a public Session object-path accessor.
    // This workaround relies on that locked version's `Serialize` representation;
    // review it on every ashpd upgrade and prefer a future public path API.
    serde_json::to_value(session)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .ok_or_else(|| ShortcutError::SessionError {
            reason: "Portal CreateSession did not provide a usable session handle".into(),
        })
}

fn unavailable_error(operation: &str, _error: ashpd::Error) -> ShortcutError {
    debug!(
        operation,
        error_category = "unavailable",
        "Portal client operation unavailable"
    );
    ShortcutError::Unavailable {
        reason: format!("Portal GlobalShortcuts is unavailable during {operation}"),
    }
}

fn session_error<T>(operation: &str, result: Result<T, ashpd::Error>) -> Result<T, ShortcutError> {
    result.map_err(|_error| {
        debug!(
            operation,
            error_category = "session",
            "Portal session operation failed"
        );
        ShortcutError::SessionError {
            reason: format!("Portal {operation} failed"),
        }
    })
}

fn session_zbus_error(operation: &str, _error: zbus::Error) -> ShortcutError {
    debug!(
        operation,
        error_category = "session",
        "Portal session listener operation failed"
    );
    ShortcutError::SessionError {
        reason: format!("Portal {operation} failed"),
    }
}

fn validate_bind_response(
    bindings: Vec<ShortcutBinding>,
) -> Result<Vec<ShortcutBinding>, ShortcutError> {
    if bindings
        .iter()
        .any(|binding| binding.id == show_search_shortcut_id())
    {
        Ok(bindings)
    } else {
        Err(ShortcutError::RequestEnded {
            reason: "BindShortcuts response did not include show-search".into(),
        })
    }
}

fn bind_error(error: ashpd::Error) -> ShortcutError {
    let normalized = classify_bind_error(&error);
    debug!(
        operation = "BindShortcuts",
        error_category = normalized.category(),
        "Portal shortcut binding failed"
    );
    normalized
}

fn classify_bind_error(error: &ashpd::Error) -> ShortcutError {
    match error {
        ashpd::Error::Response(ResponseError::Cancelled)
        | ashpd::Error::Portal(ashpd::PortalError::Cancelled(_)) => ShortcutError::UserCancelled,
        _ => ShortcutError::SessionError {
            reason: "Portal BindShortcuts failed".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_cancelled_response_is_user_cancelled() {
        assert!(matches!(
            classify_bind_error(&ashpd::Error::Response(ResponseError::Cancelled)),
            ShortcutError::UserCancelled
        ));
        assert!(matches!(
            classify_bind_error(&ashpd::Error::Portal(ashpd::PortalError::Cancelled(
                "cancelled".into(),
            ))),
            ShortcutError::UserCancelled
        ));
    }

    #[test]
    fn bind_other_response_is_a_session_failure() {
        assert!(matches!(
            classify_bind_error(&ashpd::Error::Response(ResponseError::Other)),
            ShortcutError::SessionError { ref reason } if reason == "Portal BindShortcuts failed"
        ));
    }

    #[test]
    fn bind_response_without_show_search_is_request_ended() {
        assert!(matches!(
            validate_bind_response(Vec::new()),
            Err(ShortcutError::RequestEnded { ref reason })
                if reason == "BindShortcuts response did not include show-search"
        ));
    }
}
