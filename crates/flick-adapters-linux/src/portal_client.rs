//! Portal 生命周期策略与平台客户端之间的类型化边界。

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

/// Portal 快捷键会话的稳定宿主级标识。
///
/// 它刻意与 D-Bus 对象路径无关，因此没有任何路径跨越平台/客户端边界，
/// 也不会出现在适配器策略或诊断中。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PortalSessionId(u64);

impl PortalSessionId {
    /// 创建稳定的会话标识。主要用于宿主侧的假实现。
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}

/// 从新打开的 Portal 快捷键会话中列出的绑定。
#[derive(Debug, Clone)]
pub struct PortalSession {
    pub id: PortalSessionId,
    pub bindings: Vec<ShortcutBinding>,
}

/// 由 Portal 客户端发出的规范化事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortalEvent {
    /// 由所标识的 Portal 会话注册的快捷键被激活。
    Activated {
        session_id: PortalSessionId,
        shortcut_id: String,
        activation_token: Option<String>,
    },
    /// 所标识的 Portal 会话被桌面环境或 Portal 关闭。
    SessionClosed { session_id: PortalSessionId },
    /// Portal 传输层停止为所标识的会话投递事件。
    TransportError {
        session_id: PortalSessionId,
        message: String,
    },
}

/// 面向平台的 Portal 客户端。
///
/// 客户端持有特定于平台的会话对象。适配器只能看到规范化的绑定、错误、
/// 会话标识和事件，从而将 Portal 的实现细节排除在快捷键策略和诊断模型之外。
#[async_trait::async_trait]
pub trait PortalClient: Send + Sync {
    /// 创建会话并列出其已有的快捷键绑定。
    async fn open_session_and_list(&self) -> Result<PortalSession, ShortcutError>;

    /// 为已原子发布的会话启用转发，并返回转发被门控期间缓存的所有规范化事件。
    async fn activate_session_events(
        &self,
        session_id: PortalSessionId,
    ) -> Result<Vec<PortalEvent>, ShortcutError>;

    /// 对 show-search 快捷键执行唯一的一次显式绑定尝试。
    async fn bind_show_search(
        &self,
        preferred_trigger: &str,
    ) -> Result<Vec<ShortcutBinding>, ShortcutError>;

    /// 关闭当前活跃的平台会话（如果存在）。
    async fn close_session(&self);

    /// 返回提供规范化 Portal 事件的唯一接收端。
    ///
    /// 返回 `None` 表示客户端内部出现生命周期/编程错误。
    fn take_event_receiver(&self) -> Option<tokio::sync::mpsc::UnboundedReceiver<PortalEvent>>;
}

type ActivatedStream = Pin<Box<dyn Stream<Item = Activated> + Send>>;
type ClosedStream = Pin<Box<dyn Stream<Item = zbus::Message> + Send>>;

/// 由 ashpd 支撑的真正 xdg-desktop-portal GlobalShortcuts 客户端。
///
/// ashpd 和 zbus 的值被刻意限制在此实现内部；
/// 调用方仅通过 [`PortalClient`] 的规范化类型进行交互。
pub struct AshpdPortalClient {
    state: Arc<Mutex<AshpdPortalState>>,
    /// 串行化完整的公开 Portal 操作。特别是，替换会话不会与绑定或关闭操作产生竞态。
    operation_lock: Mutex<()>,
    event_tx: mpsc::UnboundedSender<PortalEvent>,
    event_rx: StdMutex<Option<mpsc::UnboundedReceiver<PortalEvent>>>,
    /// 进程级只做一次 host-app 注册（写出 `.desktop` 并 `Register`）。
    app_id_registered: AtomicBool,
    /// 供 portal 注册与会话共享的**专用** D-Bus 连接。
    ///
    /// 不复用 ashpd 的进程级全局连接：gpui 等组件可能先经 ashpd 触碰全局连接，
    /// 使其被 portal 以「空 app id」关联，从而让 `CreateSession` 报
    /// "An app id is required"。专用连接可确保「注册」与「CreateSession」一致。
    connection: StdMutex<Option<zbus::Connection>>,
}

struct AshpdPortalState {
    /// 已在远端创建但仍在准备其监听器流和 ListShortcuts 响应的会话。
    /// 将其保持在客户端所有，可在其变为活跃之前实现可安全取消的关闭。
    pending: Option<PendingSession>,
    active: Option<ActiveSession>,
    next_session_id: u64,
}

struct PendingSession {
    session: Arc<Session<GlobalShortcuts>>,
}

struct ActiveSession {
    id: PortalSessionId,
    /// 持有针对此会话进行的所有调用所使用的 global-shortcuts 代理。
    portal: Arc<GlobalShortcuts>,
    /// 持有 ashpd 会话对象，直到它被替换或关闭。
    session: Arc<Session<GlobalShortcuts>>,
    session_handle: String,
    listener_cancel: CancellationToken,
    listener_tasks: Vec<JoinHandle<()>>,
    /// 监听器流会立即被订阅，但其规范化事件会一直保留在这里，
    /// 直到适配器将此 id 发布为活跃状态。
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
    /// 创建一个带有一个规范化事件接收端的 Portal 客户端。
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
            app_id_registered: AtomicBool::new(false),
            connection: StdMutex::new(None),
        }
    }

    /// 取得（必要时创建）本客户端专用的 session-bus 连接。
    async fn connection(&self) -> Result<zbus::Connection, ShortcutError> {
        if let Some(existing) = self
            .connection
            .lock()
            .expect("portal connection slot poisoned")
            .clone()
        {
            return Ok(existing);
        }
        let connection =
            zbus::Connection::session()
                .await
                .map_err(|error| ShortcutError::Unavailable {
                    reason: format!("Failed to connect to the session bus:{error}"),
                })?;
        let mut slot = self.connection.lock().expect("portal connection slot poisoned");
        Ok(slot.get_or_insert(connection).clone())
    }

    /// 首次使用时在专用连接上注册 host-app 身份。GNOME 下若缺少这一步，
    /// `CreateSession` 会直接以 "An app id is required" 失败。
    async fn ensure_app_id_registered(&self, connection: zbus::Connection) {
        if self.app_id_registered.load(Ordering::SeqCst) {
            return;
        }
        match crate::portal_identity::register_portal_app_id_with_connection(connection).await {
            Ok(()) => {
                self.app_id_registered.store(true, Ordering::SeqCst);
                tracing::info!("portal host-app 身份已注册");
            }
            Err(error) => {
                tracing::warn!(
                    ?error,
                    "portal host-app 注册失败；GlobalShortcuts 可能不可用（需可解析的 .desktop，Exec 指向存在的二进制）"
                );
            }
        }
    }

    /// 在状态互斥锁之外关闭之前，取出客户端持有的所有远端会话。
    /// 在正常打开过程中，`pending` 和 `active` 是互斥的，但这里出于防御性
    /// 将两者都取出，以确保替换和关闭不会遗留远端会话。
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

    /// 在 CreateSession 成功后立即注册会话。在此之前不得进行任何信号订阅或
    /// 等待 ListShortcuts，因为如果打开 future 被取消，关闭操作必须能够
    /// 找到并关闭此句柄。
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

    /// 仅当待处理会话正是正在准备的那一个时才取出它。
    /// 这使得失败清理具有幂等性：在关闭/替换已将其取走之后，本方法返回 `None`，
    /// 而不是发出第二次 Close。
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

    /// 将已注册的待处理会话转换为活跃的监听器所有权。
    /// 流已由调用方订阅；在适配器通过 `activate_session_events` 确认发布之前，
    /// 转发仍处于门控状态。
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
        let connection = self.connection().await?;
        self.ensure_app_id_registered(connection.clone()).await;

        let portal = GlobalShortcuts::with_connection(connection)
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

            // 在生成任何监听器任务之前，以及在 ListShortcuts 使此会话对 Flick
            // 可见之前，两个信号都已完成注册。
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
    // ashpd 0.13.13 的 `Session::receive_closed` 会保守地捕获其接收端生命周期。
    // 这里使用相同的会话对象路径和 D-Bus 连接创建一个私有的监听器代理，
    // 同时保留 ashpd Session 作为绑定和关闭操作所使用的权威对象。
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
                                // Portal 激活令牌有意不对外暴露。
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

/// 在适配器尚未发布此会话期间追加事件。一旦收到宿主确认，实时事件便会流向适配器。
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
    // ashpd 0.13.13 未公开 Session 对象路径的访问器。
    // 此变通方法依赖于该锁定版本的 `Serialize` 表示形式；
    // 每次升级 ashpd 时都应重新审视它，并优先采用将来公开的 path API。
    serde_json::to_value(session)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .ok_or_else(|| ShortcutError::SessionError {
            reason: "Portal CreateSession did not provide a usable session handle".into(),
        })
}

fn unavailable_error(operation: &str, error: ashpd::Error) -> ShortcutError {
    debug!(
        operation,
        %error,
        error_category = "unavailable",
        "Portal client operation unavailable"
    );
    ShortcutError::Unavailable {
        reason: format!("Portal GlobalShortcuts is unavailable during {operation}: {error}"),
    }
}

fn session_error<T>(operation: &str, result: Result<T, ashpd::Error>) -> Result<T, ShortcutError> {
    result.map_err(|error| {
        debug!(
            operation,
            %error,
            error_category = "session",
            "Portal session operation failed"
        );
        ShortcutError::SessionError {
            reason: format!("Portal {operation} failed: {error}"),
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
