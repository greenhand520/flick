//! 确定性的 Portal 快捷键生命周期测试。

use async_trait::async_trait;
use flick_adapters_linux::portal::{IntentSink, PortalShortcutAdapter};
use flick_adapters_linux::portal_client::{
    PortalClient, PortalEvent, PortalSession, PortalSessionId,
};
use flick_adapters_linux::session::{
    ShortcutBinding, ShortcutError, show_search_shortcut_description, show_search_shortcut_id,
};
use flick_adapters_linux::shortcut_state::ShortcutState;
use flick_domain::UiIntent;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Mutex as AsyncMutex, mpsc, oneshot};

type BindResult = Result<Vec<ShortcutBinding>, ShortcutError>;
type BindGate = oneshot::Receiver<BindResult>;
type BindRelease = oneshot::Sender<BindResult>;
type OpenGate = oneshot::Receiver<()>;
type OpenRelease = oneshot::Sender<()>;

/// 记录所接收意图的假意图接收端。
struct FakeIntentSink {
    intents: Mutex<Vec<UiIntent>>,
    next_intent_observer: Mutex<Option<oneshot::Sender<()>>>,
}

impl FakeIntentSink {
    fn new() -> Self {
        Self {
            intents: Mutex::new(Vec::new()),
            next_intent_observer: Mutex::new(None),
        }
    }

    fn intents(&self) -> Vec<UiIntent> {
        self.intents.lock().unwrap().clone()
    }

    fn observe_next_intent(&self) -> oneshot::Receiver<()> {
        let (sender, receiver) = oneshot::channel();
        *self.next_intent_observer.lock().unwrap() = Some(sender);
        receiver
    }
}

impl IntentSink for FakeIntentSink {
    fn send_intent(&self, intent: UiIntent) {
        self.intents.lock().unwrap().push(intent);
        if let Some(observer) = self.next_intent_observer.lock().unwrap().take() {
            let _ = observer.send(());
        }
    }
}

fn sink() -> Arc<FakeIntentSink> {
    Arc::new(FakeIntentSink::new())
}

struct FakePortalClient {
    listed: Mutex<Vec<ShortcutBinding>>,
    next_session_id: Mutex<u64>,
    active_session_id: Mutex<Option<PortalSessionId>>,
    /// 模拟 CreateSession 成功之后、open/list 将其返回给适配器之前，
    /// 客户端所持有的远端会话。
    pending_session_id: Mutex<Option<PortalSessionId>>,
    bind_results: Mutex<VecDeque<BindResult>>,
    open_and_list_calls: Mutex<u32>,
    bind_calls: Mutex<u32>,
    bind_triggers: Mutex<Vec<String>>,
    close_calls: Mutex<u32>,
    bind_gate: AsyncMutex<Option<BindGate>>,
    bind_release: Mutex<Option<BindRelease>>,
    bind_started_tx: mpsc::UnboundedSender<()>,
    bind_started_rx: AsyncMutex<mpsc::UnboundedReceiver<()>>,
    open_gate: AsyncMutex<Option<OpenGate>>,
    open_release: Mutex<Option<OpenRelease>>,
    open_pending_tx: mpsc::UnboundedSender<()>,
    open_pending_rx: AsyncMutex<mpsc::UnboundedReceiver<()>>,
    event_tx: mpsc::UnboundedSender<PortalEvent>,
    event_rx: Mutex<Option<mpsc::UnboundedReceiver<PortalEvent>>>,
}

impl FakePortalClient {
    fn with_listed(listed: Vec<ShortcutBinding>) -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (bind_started_tx, bind_started_rx) = mpsc::unbounded_channel();
        let (open_pending_tx, open_pending_rx) = mpsc::unbounded_channel();
        Self {
            listed: Mutex::new(listed),
            next_session_id: Mutex::new(1),
            active_session_id: Mutex::new(None),
            pending_session_id: Mutex::new(None),
            bind_results: Mutex::new(VecDeque::new()),
            open_and_list_calls: Mutex::new(0),
            bind_calls: Mutex::new(0),
            bind_triggers: Mutex::new(Vec::new()),
            close_calls: Mutex::new(0),
            bind_gate: AsyncMutex::new(None),
            bind_release: Mutex::new(None),
            bind_started_tx,
            bind_started_rx: AsyncMutex::new(bind_started_rx),
            open_gate: AsyncMutex::new(None),
            open_release: Mutex::new(None),
            open_pending_tx,
            open_pending_rx: AsyncMutex::new(open_pending_rx),
            event_tx,
            event_rx: Mutex::new(Some(event_rx)),
        }
    }

    fn with_bind_results(listed: Vec<ShortcutBinding>, bind_results: Vec<BindResult>) -> Self {
        let fake = Self::with_listed(listed);
        *fake.bind_results.lock().unwrap() = bind_results.into();
        fake
    }

    fn open_and_list_calls(&self) -> u32 {
        *self.open_and_list_calls.lock().unwrap()
    }

    fn bind_calls(&self) -> u32 {
        *self.bind_calls.lock().unwrap()
    }

    fn bind_triggers(&self) -> Vec<String> {
        self.bind_triggers.lock().unwrap().clone()
    }

    fn emit(&self, event: PortalEvent) {
        self.event_tx.send(event).unwrap();
    }

    fn active_session_id(&self) -> PortalSessionId {
        (*self.active_session_id.lock().unwrap()).expect("fake session was not opened")
    }

    fn set_listed(&self, listed: Vec<ShortcutBinding>) {
        *self.listed.lock().unwrap() = listed;
    }

    fn close_calls(&self) -> u32 {
        *self.close_calls.lock().unwrap()
    }

    async fn block_next_bind(&self) {
        let (release, gate) = oneshot::channel();
        *self.bind_release.lock().unwrap() = Some(release);
        *self.bind_gate.lock().await = Some(gate);
    }

    async fn wait_for_bind_start(&self) {
        self.bind_started_rx
            .lock()
            .await
            .recv()
            .await
            .expect("blocked bind start notification missing");
    }

    fn release_bind(&self, result: BindResult) {
        let sender = self
            .bind_release
            .lock()
            .unwrap()
            .take()
            .expect("blocked bind release sender missing");
        let _ = sender.send(result);
    }

    async fn block_next_open_after_pending(&self) {
        let (release, gate) = oneshot::channel();
        *self.open_release.lock().unwrap() = Some(release);
        *self.open_gate.lock().await = Some(gate);
    }

    async fn wait_for_pending_open(&self) {
        self.open_pending_rx
            .lock()
            .await
            .recv()
            .await
            .expect("pending open notification missing");
    }

    fn release_open(&self) {
        if let Some(sender) = self.open_release.lock().unwrap().take() {
            let _ = sender.send(());
        }
    }

    fn has_no_registered_session(&self) -> bool {
        self.pending_session_id.lock().unwrap().is_none()
            && self.active_session_id.lock().unwrap().is_none()
    }
}

#[async_trait]
impl PortalClient for FakePortalClient {
    async fn open_session_and_list(&self) -> Result<PortalSession, ShortcutError> {
        *self.open_and_list_calls.lock().unwrap() += 1;
        let session_id = {
            let mut next = self.next_session_id.lock().unwrap();
            let session_id = PortalSessionId::new(*next);
            *next = next.wrapping_add(1);
            session_id
        };
        *self.pending_session_id.lock().unwrap() = Some(session_id);

        if let Some(gate) = self.open_gate.lock().await.take() {
            let _ = self.open_pending_tx.send(());
            let _ = gate.await;
        }

        let pending = self
            .pending_session_id
            .lock()
            .unwrap()
            .take()
            .expect("pending fake session disappeared before open completed");
        assert_eq!(pending, session_id);
        *self.active_session_id.lock().unwrap() = Some(session_id);
        Ok(PortalSession {
            id: session_id,
            bindings: self.listed.lock().unwrap().clone(),
        })
    }

    async fn activate_session_events(
        &self,
        _session_id: PortalSessionId,
    ) -> Result<Vec<PortalEvent>, ShortcutError> {
        Ok(Vec::new())
    }

    async fn bind_show_search(
        &self,
        preferred_trigger: &str,
    ) -> Result<Vec<ShortcutBinding>, ShortcutError> {
        *self.bind_calls.lock().unwrap() += 1;
        self.bind_triggers
            .lock()
            .unwrap()
            .push(preferred_trigger.to_owned());

        if let Some(gate) = self.bind_gate.lock().await.take() {
            let _ = self.bind_started_tx.send(());
            return gate.await.expect("blocked bind release sender dropped");
        }

        if let Some(result) = self.bind_results.lock().unwrap().pop_front() {
            return result;
        }

        Ok(vec![show_search_binding(preferred_trigger)])
    }

    async fn close_session(&self) {
        let pending = self.pending_session_id.lock().unwrap().take();
        let active = self.active_session_id.lock().unwrap().take();
        *self.close_calls.lock().unwrap() +=
            u32::from(pending.is_some()) + u32::from(active.is_some());
    }

    fn take_event_receiver(&self) -> Option<mpsc::UnboundedReceiver<PortalEvent>> {
        self.event_rx.lock().unwrap().take()
    }
}

fn show_search_binding(preferred_trigger: &str) -> ShortcutBinding {
    ShortcutBinding {
        id: show_search_shortcut_id().into(),
        description: show_search_shortcut_description().into(),
        preferred_trigger: preferred_trigger.into(),
        actual_trigger: Some(preferred_trigger.into()),
    }
}

async fn wait_for_recovering(adapter: &PortalShortcutAdapter) {
    for _ in 0..64 {
        if matches!(
            adapter.diagnostics().await.state,
            ShortcutState::Recovering { attempt: 0 }
        ) {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("Portal recovery was not scheduled");
}

async fn yield_runtime() {
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn explicit_configure_opens_a_fresh_session_after_any_prior_bind_attempt() {
    let fake = Arc::new(FakePortalClient::with_listed(Vec::new()));
    let adapter = PortalShortcutAdapter::new_with_client(sink(), fake.clone());

    adapter.initialize().await.unwrap();
    adapter
        .configure_search_shortcut("Super+Space")
        .await
        .unwrap();
    adapter
        .configure_search_shortcut("Ctrl+Space")
        .await
        .unwrap();

    assert_eq!(fake.open_and_list_calls(), 2);
    assert_eq!(fake.bind_calls(), 2);
    assert_eq!(fake.bind_triggers(), vec!["Super+Space", "Ctrl+Space"]);
}

#[tokio::test]
async fn reconnect_opens_and_lists_but_never_binds() {
    let fake = Arc::new(FakePortalClient::with_listed(Vec::new()));
    let adapter = PortalShortcutAdapter::new_with_client(sink(), fake.clone());

    adapter.reconnect().await.unwrap();

    assert_eq!(fake.open_and_list_calls(), 1);
    assert_eq!(fake.bind_calls(), 0);
}

#[tokio::test]
async fn listed_show_search_enters_bound() {
    let fake = Arc::new(FakePortalClient::with_listed(vec![show_search_binding(
        "Super+Space",
    )]));
    let adapter = PortalShortcutAdapter::new_with_client(sink(), fake);

    let snapshot = adapter.initialize().await.unwrap();

    assert_eq!(snapshot.state, ShortcutState::Bound);
    assert!(snapshot.session_active);
    assert_eq!(snapshot.shortcut_count, 1);
}

#[tokio::test]
async fn denied_bind_enters_denied() {
    let fake = Arc::new(FakePortalClient::with_bind_results(
        Vec::new(),
        vec![Err(ShortcutError::UserCancelled)],
    ));
    let adapter = PortalShortcutAdapter::new_with_client(sink(), fake);
    adapter.initialize().await.unwrap();

    assert!(matches!(
        adapter.configure_search_shortcut("Super+Space").await,
        Err(ShortcutError::UserCancelled)
    ));
    assert_eq!(adapter.diagnostics().await.state, ShortcutState::Denied);
}

#[tokio::test]
async fn a_later_explicit_configure_after_denial_uses_a_fresh_session() {
    let fake = Arc::new(FakePortalClient::with_bind_results(
        Vec::new(),
        vec![
            Err(ShortcutError::UserCancelled),
            Ok(vec![show_search_binding("Ctrl+Space")]),
        ],
    ));
    let adapter = PortalShortcutAdapter::new_with_client(sink(), fake.clone());
    adapter.initialize().await.unwrap();

    assert!(matches!(
        adapter.configure_search_shortcut("Super+Space").await,
        Err(ShortcutError::UserCancelled)
    ));
    adapter
        .configure_search_shortcut("Ctrl+Space")
        .await
        .unwrap();

    assert_eq!(fake.open_and_list_calls(), 2);
    assert_eq!(fake.bind_calls(), 2);
    assert_eq!(adapter.diagnostics().await.state, ShortcutState::Bound);
}

#[tokio::test]
async fn bind_response_without_show_search_becomes_a_protocol_error() {
    let fake = Arc::new(FakePortalClient::with_bind_results(
        Vec::new(),
        vec![Ok(Vec::new())],
    ));
    let adapter = PortalShortcutAdapter::new_with_client(sink(), fake);
    adapter.initialize().await.unwrap();

    assert!(matches!(
        adapter.configure_search_shortcut("Super+Space").await,
        Err(ShortcutError::RequestEnded { .. })
    ));
    assert!(matches!(
        adapter.diagnostics().await.state,
        ShortcutState::Error { ref operation, .. } if operation == "BindShortcuts"
    ));
}

#[tokio::test]
async fn activation_of_another_id_is_ignored() {
    let fake = Arc::new(FakePortalClient::with_listed(Vec::new()));
    let intent_sink = sink();
    let adapter = PortalShortcutAdapter::new_with_client(intent_sink.clone(), fake.clone());
    adapter.initialize().await.unwrap();

    fake.emit(PortalEvent::Activated {
        session_id: fake.active_session_id(),
        shortcut_id: "other-shortcut".into(),
        activation_token: Some("unused".into()),
    });
    tokio::time::sleep(Duration::from_millis(30)).await;

    assert!(intent_sink.intents().is_empty());
}

#[tokio::test]
async fn show_search_activation_creates_exactly_one_show_intent() {
    let fake = Arc::new(FakePortalClient::with_listed(Vec::new()));
    let intent_sink = sink();
    let adapter = PortalShortcutAdapter::new_with_client(intent_sink.clone(), fake.clone());
    adapter.initialize().await.unwrap();

    let activation_observed = intent_sink.observe_next_intent();
    fake.emit(PortalEvent::Activated {
        session_id: fake.active_session_id(),
        shortcut_id: show_search_shortcut_id().into(),
        activation_token: Some("activation-token".into()),
    });
    activation_observed
        .await
        .expect("show-search activation should be observed");

    let intents = intent_sink.intents();
    assert_eq!(intents.len(), 1);
    match &intents[0] {
        UiIntent::ToggleSearchPanel => {}
        _ => panic!("expected a toggle-search intent"),
    }
}

#[tokio::test(start_paused = true)]
async fn session_closed_schedules_recovery_without_binding() {
    let fake = Arc::new(FakePortalClient::with_listed(Vec::new()));
    let adapter = PortalShortcutAdapter::new_with_client(sink(), fake.clone());
    adapter.initialize().await.unwrap();

    fake.emit(PortalEvent::SessionClosed {
        session_id: fake.active_session_id(),
    });
    wait_for_recovering(&adapter).await;
    tokio::time::advance(Duration::from_secs(2)).await;
    yield_runtime().await;

    assert_eq!(fake.open_and_list_calls(), 2);
    assert_eq!(fake.bind_calls(), 0);
}

#[tokio::test]
async fn diagnostics_retain_last_error_category_and_message() {
    let fake = Arc::new(FakePortalClient::with_listed(Vec::new()));
    let adapter = PortalShortcutAdapter::new_with_client(sink(), fake.clone());
    adapter.initialize().await.unwrap();

    fake.emit(PortalEvent::TransportError {
        session_id: fake.active_session_id(),
        message: "D-Bus connection reset".into(),
    });
    wait_for_recovering(&adapter).await;

    let snapshot = adapter.diagnostics().await;
    let error = snapshot
        .last_error
        .expect("transport error must be retained");
    assert_eq!(error.operation, "TransportError");
    assert_eq!(error.category, "transport");
    assert_eq!(error.message, "D-Bus connection reset");
}

#[tokio::test]
async fn shutdown_closes_a_pending_opened_session_exactly_once() {
    let fake = Arc::new(FakePortalClient::with_listed(Vec::new()));
    fake.block_next_open_after_pending().await;
    let adapter = PortalShortcutAdapter::new_with_client(sink(), fake.clone());

    let initialize = {
        let adapter = adapter.clone();
        tokio::spawn(async move { adapter.initialize().await })
    };
    fake.wait_for_pending_open().await;

    tokio::time::timeout(Duration::from_secs(1), adapter.shutdown())
        .await
        .expect("shutdown did not clean the pending portal session");
    let initialize = initialize
        .await
        .expect("initialize task panicked while shutdown cancelled open");
    assert!(matches!(
        initialize,
        Err(ShortcutError::SessionError { .. })
    ));

    // 在被取消的 open 已丢弃之后释放假客户端，不会重新注册会话，
    // 也不会产生第二次客户端关闭。
    fake.release_open();
    assert_eq!(fake.close_calls(), 1);
    assert!(fake.has_no_registered_session());
    assert!(!adapter.diagnostics().await.session_active);
}

#[tokio::test(start_paused = true)]
async fn shutdown_after_recovery_scheduling_prevents_later_open_and_is_idempotent() {
    let fake = Arc::new(FakePortalClient::with_listed(Vec::new()));
    let adapter = PortalShortcutAdapter::new_with_client(sink(), fake.clone());
    adapter.initialize().await.unwrap();

    fake.emit(PortalEvent::SessionClosed {
        session_id: fake.active_session_id(),
    });
    wait_for_recovering(&adapter).await;
    adapter.shutdown().await;
    adapter.shutdown().await;

    tokio::time::advance(Duration::from_secs(600)).await;
    yield_runtime().await;

    assert_eq!(fake.open_and_list_calls(), 1);
    assert_eq!(fake.bind_calls(), 0);
    assert_eq!(fake.close_calls(), 1);
    assert!(!adapter.diagnostics().await.session_active);
}

#[tokio::test(start_paused = true)]
async fn manual_reconnect_invalidates_pending_recovery_before_it_can_overwrite_state() {
    let fake = Arc::new(FakePortalClient::with_listed(Vec::new()));
    let adapter = PortalShortcutAdapter::new_with_client(sink(), fake.clone());
    adapter.initialize().await.unwrap();

    fake.emit(PortalEvent::SessionClosed {
        session_id: fake.active_session_id(),
    });
    wait_for_recovering(&adapter).await;
    fake.set_listed(vec![show_search_binding("Ctrl+Space")]);

    let snapshot = adapter.reconnect().await.unwrap();
    assert_eq!(snapshot.state, ShortcutState::Bound);
    assert_eq!(fake.open_and_list_calls(), 2);

    tokio::time::advance(Duration::from_secs(600)).await;
    yield_runtime().await;

    assert_eq!(fake.open_and_list_calls(), 2);
    assert_eq!(adapter.diagnostics().await.state, ShortcutState::Bound);
}

#[tokio::test]
async fn session_close_while_bind_is_in_flight_cannot_publish_bound() {
    let fake = Arc::new(FakePortalClient::with_listed(Vec::new()));
    fake.block_next_bind().await;
    let adapter = Arc::new(PortalShortcutAdapter::new_with_client(sink(), fake.clone()));
    adapter.initialize().await.unwrap();

    let binding_adapter = adapter.clone();
    let binding = tokio::spawn(async move {
        binding_adapter
            .configure_search_shortcut("Super+Space")
            .await
    });
    fake.wait_for_bind_start().await;

    fake.emit(PortalEvent::SessionClosed {
        session_id: fake.active_session_id(),
    });
    wait_for_recovering(&adapter).await;
    fake.release_bind(Ok(vec![show_search_binding("Super+Space")]));

    assert!(matches!(
        binding.await.expect("binding task panicked"),
        Err(ShortcutError::SessionError { .. })
    ));
    assert!(matches!(
        adapter.diagnostics().await.state,
        ShortcutState::Recovering { attempt: 0 }
    ));
}

#[tokio::test]
async fn shutdown_cancels_a_blocked_bind_without_resurrecting_live_state() {
    let fake = Arc::new(FakePortalClient::with_listed(Vec::new()));
    fake.block_next_bind().await;
    let adapter = Arc::new(PortalShortcutAdapter::new_with_client(sink(), fake.clone()));
    adapter.initialize().await.unwrap();

    let configure_adapter = adapter.clone();
    let configure = tokio::spawn(async move {
        configure_adapter
            .configure_search_shortcut("Super+Space")
            .await
    });
    fake.wait_for_bind_start().await;

    tokio::time::timeout(Duration::from_millis(250), adapter.shutdown())
        .await
        .expect("shutdown must cancel a blocked Portal bind");

    assert!(matches!(
        configure.await.expect("configure task panicked"),
        Err(ShortcutError::SessionError { .. })
    ));
    assert_eq!(fake.close_calls(), 1);

    // 被取消的绑定 future 会丢弃其接收端。因此，迟到的 Portal 完成事件
    // 必须是无害的，且不得恢复 active/Bound 状态。
    fake.release_bind(Ok(vec![show_search_binding("Super+Space")]));
    yield_runtime().await;

    let snapshot = adapter.diagnostics().await;
    assert!(!snapshot.session_active);
    assert!(!matches!(snapshot.state, ShortcutState::Bound));
    assert_eq!(fake.close_calls(), 1);
}
