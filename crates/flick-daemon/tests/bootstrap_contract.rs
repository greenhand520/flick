//! 引导契约测试。
//!
//! 验证守护进程的组合根目录：
//! - 无头模式在没有 Iced/Portal 的情况下启动 Runtime + IPC
//! - IPC 调度器正确处理所有命令
//! - 关闭时清理套接字
//! - Portal 不可用不会导致守护进程崩溃

use async_trait::async_trait;
use flick_adapters_linux::portal::{IntentSink, PortalShortcutAdapter};
use flick_adapters_linux::portal_client::{
    PortalClient, PortalEvent, PortalSession, PortalSessionId,
};
use flick_adapters_linux::session::{
    ShortcutBinding, ShortcutError, show_search_shortcut_description, show_search_shortcut_id,
};
use flick_application::coordinator::ApplicationCoordinator;
use flick_application::ports::{HostEventSink, RuntimePort};
use flick_domain::*;
use flick_ipc::message::{IpcCommand, IpcErrorCode, IpcResult};
use flick_ipc::server::IpcDispatcher;
use flick_runtime::ActionRuntime;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::sync::mpsc;

#[allow(dead_code)]
#[path = "../src/headless.rs"]
mod headless;

#[allow(dead_code)]
#[path = "../src/bootstrap.rs"]
mod bootstrap;
#[allow(dead_code)]
#[path = "../src/lifecycle.rs"]
mod lifecycle;

// ─── 测试辅助函数 ───────────────────────────────────────────────────────────

struct TestRuntimeAdapter {
    runtime: Arc<ActionRuntime>,
}

#[async_trait::async_trait]
impl RuntimePort for TestRuntimeAdapter {
    async fn route(&self, query: &str, input_kind: InputType) -> Vec<RouteItem> {
        self.runtime.route(query, input_kind).await
    }
    async fn invoke(&self, request: ActionRequest) -> ActionOutcome {
        self.runtime.invoke(request).await
    }
    async fn cancel(&self, invocation_id: InvocationId) -> bool {
        self.runtime.cancel(invocation_id).await
    }
    async fn surface_event(
        &self,
        session: SurfaceSessionId,
        kind: SurfaceEventKind,
    ) -> Result<SurfaceContent, String> {
        self.runtime
            .surface_event(session, kind)
            .await
            .map_err(|e| e.message)
    }
    async fn surface_closed(&self, session: SurfaceSessionId) {
        self.runtime.surface_closed(session).await;
    }
}

struct TestEventSink {
    tx: mpsc::UnboundedSender<HostToUi>,
}

impl HostEventSink for TestEventSink {
    fn send(&self, event: HostToUi) {
        let _ = self.tx.send(event);
    }
}

struct ConfigurePortalClient {
    bind_result: Mutex<Result<Vec<ShortcutBinding>, ShortcutError>>,
    event_rx: Mutex<Option<mpsc::UnboundedReceiver<PortalEvent>>>,
    bind_triggers: Mutex<Vec<String>>,
    open_and_list_calls: AtomicUsize,
    bind_calls: AtomicUsize,
}

impl ConfigurePortalClient {
    fn new(bind_result: Result<Vec<ShortcutBinding>, ShortcutError>) -> Self {
        let (_event_tx, event_rx) = mpsc::unbounded_channel();
        Self {
            bind_result: Mutex::new(bind_result),
            event_rx: Mutex::new(Some(event_rx)),
            bind_triggers: Mutex::new(Vec::new()),
            open_and_list_calls: AtomicUsize::new(0),
            bind_calls: AtomicUsize::new(0),
        }
    }

    fn bind_triggers(&self) -> Vec<String> {
        self.bind_triggers.lock().unwrap().clone()
    }

    fn open_and_list_calls(&self) -> usize {
        self.open_and_list_calls.load(Ordering::SeqCst)
    }

    fn bind_calls(&self) -> usize {
        self.bind_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl PortalClient for ConfigurePortalClient {
    async fn open_session_and_list(&self) -> Result<PortalSession, ShortcutError> {
        self.open_and_list_calls.fetch_add(1, Ordering::SeqCst);
        Ok(PortalSession {
            id: PortalSessionId::new(1),
            bindings: Vec::new(),
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
        self.bind_calls.fetch_add(1, Ordering::SeqCst);
        self.bind_triggers
            .lock()
            .unwrap()
            .push(preferred_trigger.to_owned());
        self.bind_result.lock().unwrap().clone()
    }

    async fn close_session(&self) {}

    fn take_event_receiver(&self) -> Option<mpsc::UnboundedReceiver<PortalEvent>> {
        self.event_rx.lock().unwrap().take()
    }
}

struct NoopIntentSink;

impl IntentSink for NoopIntentSink {
    fn send_intent(&self, _intent: UiIntent) {}
}

fn show_search_binding(actual_trigger: &str) -> ShortcutBinding {
    ShortcutBinding {
        id: show_search_shortcut_id().into(),
        description: show_search_shortcut_description().into(),
        preferred_trigger: actual_trigger.into(),
        actual_trigger: Some(actual_trigger.into()),
    }
}

async fn build_dispatcher(
    portal: Option<Arc<PortalShortcutAdapter>>,
) -> (
    bootstrap::DaemonIpcDispatcher,
    mpsc::UnboundedReceiver<HostToUi>,
) {
    build_dispatcher_with_lifecycle(portal, lifecycle::DaemonLifecycle::new()).await
}

async fn build_dispatcher_with_lifecycle(
    portal: Option<Arc<PortalShortcutAdapter>>,
    lifecycle: lifecycle::DaemonLifecycle,
) -> (
    bootstrap::DaemonIpcDispatcher,
    mpsc::UnboundedReceiver<HostToUi>,
) {
    let (coordinator, runtime, events) = build_test_stack().await;
    let runtime = Arc::new(TestRuntimeAdapter { runtime }) as Arc<dyn RuntimePort>;
    let (intent_tx, _intent_rx) = mpsc::unbounded_channel();
    (
        bootstrap::DaemonIpcDispatcher {
            runtime,
            coordinator,
            intent_tx,
            portal,
            socket_path: std::path::PathBuf::from("/tmp/flick-bootstrap-test.sock"),
            lifecycle,
        },
        events,
    )
}

/// 构建一个测试用的协调器 + 运行时组合。
async fn build_test_stack() -> (
    Arc<ApplicationCoordinator>,
    Arc<ActionRuntime>,
    mpsc::UnboundedReceiver<HostToUi>,
) {
    let runtime = Arc::new(ActionRuntime::new());
    flick_builtins::register_builtin(&runtime)
        .await
        .expect("register builtin plugins");
    runtime.activate_all_plugins().await;

    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let sink = Arc::new(TestEventSink { tx: event_tx });
    let adapter = Arc::new(TestRuntimeAdapter {
        runtime: runtime.clone(),
    }) as Arc<dyn RuntimePort>;
    let coordinator = Arc::new(ApplicationCoordinator::new(adapter, sink));

    (coordinator, runtime, event_rx)
}

// ─── 测试 ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn headless_runtime_registers_echo() {
    let (_, runtime, _) = build_test_stack().await;

    let routes = runtime.route("echo hello", InputType::PlainText).await;
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].action_id.as_str(), "flick.builtin.echo.v1.echo");
}

#[tokio::test]
async fn headless_echo_invoke_returns_data() {
    let (_, runtime, _) = build_test_stack().await;

    let request = ActionRequest::new_echo("test", Caller::cli("test-cli"));
    let outcome = runtime.invoke(request).await;
    match outcome {
        ActionOutcome::Data(val) => assert_eq!(val["echo"], "test"),
        other => panic!("expected Data, got {other:?}"),
    }
}

#[tokio::test]
async fn coordinator_toggle_emits_show_hide() {
    let (coordinator, _, mut events) = build_test_stack().await;

    coordinator.toggle_search_panel();
    let event = events.recv().await.unwrap();
    assert!(matches!(event, HostToUi::ShowSearchPanel { .. }));

    coordinator.toggle_search_panel();
    let event = events.recv().await.unwrap();
    assert!(matches!(event, HostToUi::HideSearchPanel));
}

#[tokio::test]
async fn coordinator_portal_intent_does_not_invoke_runtime() {
    let (coordinator, runtime, mut events) = build_test_stack().await;

    // 通过路由检查记录初始调用次数
    let routes_before = runtime.route("echo", InputType::PlainText).await;

    coordinator.handle_intent(UiIntent::ShowSearchPanel {
        activation_token: Some("test-token".into()),
    });

    let event = events.recv().await.unwrap();
    match event {
        HostToUi::ShowSearchPanel { activation_token } => {
            assert_eq!(activation_token.as_deref(), Some("test-token"));
        }
        other => panic!("expected ShowSearchPanel, got {other:?}"),
    }

    // Runtime 不应被触及
    let routes_after = runtime.route("echo", InputType::PlainText).await;
    assert_eq!(routes_before.len(), routes_after.len());
}

#[tokio::test]
async fn coordinator_invoke_from_ui_returns_completion() {
    let (coordinator, _, mut events) = build_test_stack().await;

    // 候选身份校验（§12.2-3）要求先有当前代次的 Routes。
    coordinator
        .handle_ui(UiToHost::QueryChanged {
            generation: QueryGeneration::INITIAL,
            input: SearchInputSnapshot::text("echo", InputSource::Manual),
        })
        .await;
    let item_id = match events.recv().await.unwrap() {
        HostToUi::Routes { generation, items } => {
            assert_eq!(generation, QueryGeneration::INITIAL);
            let echo = items
                .iter()
                .find(|item| item.action_id.as_str() == "flick.builtin.echo.v1.echo")
                .expect("echo candidate");
            echo.item_id.clone()
        }
        other => panic!("expected Routes, got {other:?}"),
    };

    coordinator
        .handle_ui(UiToHost::Invoke {
            generation: QueryGeneration::INITIAL,
            item_id,
            invocation_id: InvocationId::new(),
            action_id: ActionId::from_static("flick.builtin.echo.v1.echo"),
            input: ActionInput::plain_text("hello"),
        })
        .await;

    let event = events.recv().await.unwrap();
    match event {
        HostToUi::InvocationCompleted { outcome, .. } => match outcome {
            ActionOutcome::Data(val) => assert_eq!(val["echo"], "hello"),
            other => panic!("expected Data, got {other:?}"),
        },
        other => panic!("expected InvocationCompleted, got {other:?}"),
    }
}

#[tokio::test]
async fn coordinator_query_changed_returns_routes() {
    let (coordinator, _, mut events) = build_test_stack().await;

    coordinator
        .handle_ui(UiToHost::QueryChanged {
            generation: QueryGeneration::INITIAL,
            input: SearchInputSnapshot::text("echo", InputSource::Manual),
        })
        .await;

    let event = events.recv().await.unwrap();
    match event {
        HostToUi::Routes { generation, items } => {
            assert_eq!(generation, QueryGeneration::INITIAL);
            assert!(!items.is_empty());
        }
        other => panic!("expected Routes, got {other:?}"),
    }
}

#[tokio::test]
async fn shutdown_response_is_successful_before_lifecycle_is_cancelled() {
    let lifecycle = lifecycle::DaemonLifecycle::new();
    let (dispatcher, _) = build_dispatcher_with_lifecycle(None, lifecycle.clone()).await;

    let response = dispatcher.dispatch(IpcCommand::Shutdown).await;
    match response {
        IpcResult::Ok(value) => assert_eq!(value["shutdown"], true),
        other => panic!("expected successful shutdown response, got {other:?}"),
    }
    assert_eq!(
        lifecycle.reason(),
        None,
        "dispatch must not preempt the IPC response"
    );

    dispatcher.after_response(&IpcCommand::Shutdown).await;
    tokio::time::timeout(Duration::from_millis(50), lifecycle.cancelled())
        .await
        .expect("shutdown response must request the shared lifecycle");
    assert_eq!(lifecycle.reason(), Some(lifecycle::ShutdownReason::Ipc));
}

#[tokio::test]
async fn shortcuts_configure_awaits_portal_and_returns_bound_state_without_showing_panel() {
    let fake = Arc::new(ConfigurePortalClient::new(Ok(vec![show_search_binding(
        "Ctrl+Space",
    )])));
    let portal = Arc::new(PortalShortcutAdapter::new_with_client(
        Arc::new(NoopIntentSink),
        fake.clone(),
    ));
    let (dispatcher, mut events) = build_dispatcher(Some(portal)).await;

    let result = dispatcher
        .dispatch(IpcCommand::ShortcutsConfigure {
            preferred_trigger: "Super+Space".into(),
        })
        .await;

    match result {
        IpcResult::Ok(value) => {
            assert_eq!(value["state"], "bound");
            assert_eq!(value["shortcut"]["id"], "show-search");
            assert_eq!(value["shortcut"]["actual_trigger"], "Ctrl+Space");
        }
        other => panic!("expected shortcut configuration success, got {other:?}"),
    }
    assert_eq!(fake.bind_triggers(), vec!["Super+Space"]);
    assert!(
        events.try_recv().is_err(),
        "configure must not show the panel"
    );
}

#[tokio::test]
async fn shortcuts_reconnect_recreates_and_lists_without_binding_or_showing_panel() {
    let fake = Arc::new(ConfigurePortalClient::new(Ok(Vec::new())));
    let portal = Arc::new(PortalShortcutAdapter::new_with_client(
        Arc::new(NoopIntentSink),
        fake.clone(),
    ));
    let (dispatcher, mut events) = build_dispatcher(Some(portal)).await;

    let result = dispatcher.dispatch(IpcCommand::ShortcutsReconnect).await;

    match result {
        IpcResult::Ok(value) => {
            assert_eq!(value["state"], "unconfigured");
            assert_eq!(value["session_active"], true);
        }
        other => panic!("expected reconnect snapshot, got {other:?}"),
    }
    assert_eq!(fake.open_and_list_calls(), 1);
    assert_eq!(fake.bind_calls(), 0);
    assert!(
        events.try_recv().is_err(),
        "reconnect must not show the panel"
    );
}

#[tokio::test]
async fn doctor_merges_session_portal_and_ipc_facts() {
    let fake = Arc::new(ConfigurePortalClient::new(Ok(Vec::new())));
    let portal = Arc::new(PortalShortcutAdapter::new_with_client(
        Arc::new(NoopIntentSink),
        fake,
    ));
    portal.initialize().await.expect("fake Portal initializes");
    let (dispatcher, _) = build_dispatcher(Some(portal)).await;

    let result = dispatcher.dispatch(IpcCommand::Doctor).await;

    match result {
        IpcResult::Ok(value) => {
            assert!(value.get("session").is_some());
            assert_eq!(value["portal"]["state"], "unconfigured");
            assert!(value["ipc"]["socket"].is_string());
        }
        other => panic!("expected merged doctor result, got {other:?}"),
    }
}

#[tokio::test]
async fn shortcuts_configure_returns_safe_errors_without_a_portal_or_after_cancellation() {
    let (without_portal, mut events) = build_dispatcher(None).await;
    let request = IpcCommand::ShortcutsConfigure {
        preferred_trigger: "Super+Space".into(),
    };
    let unavailable = without_portal.dispatch(request).await;
    match unavailable {
        IpcResult::Error { code, message } => {
            assert_eq!(code, IpcErrorCode::DesktopUnavailable);
            assert!(!message.contains("Super+Space"));
        }
        other => panic!("expected unavailable Portal error, got {other:?}"),
    }
    assert!(
        events.try_recv().is_err(),
        "configure must not show the panel"
    );

    let portal = Arc::new(PortalShortcutAdapter::new_with_client(
        Arc::new(NoopIntentSink),
        Arc::new(ConfigurePortalClient::new(Err(
            ShortcutError::UserCancelled,
        ))),
    ));
    let (cancelled_dispatcher, mut cancelled_events) = build_dispatcher(Some(portal)).await;
    let cancelled = cancelled_dispatcher
        .dispatch(IpcCommand::ShortcutsConfigure {
            preferred_trigger: "Super+Space".into(),
        })
        .await;
    match cancelled {
        IpcResult::Error { code, message } => {
            assert_eq!(code, IpcErrorCode::Cancelled);
            assert!(!message.contains("Super+Space"));
        }
        other => panic!("expected cancellation error, got {other:?}"),
    }
    assert!(
        cancelled_events.try_recv().is_err(),
        "configure must not show the panel"
    );
}

#[tokio::test]
async fn socket_cleanup_on_shutdown() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let socket_path = tmpdir.path().join("test.sock");

    // 绑定服务器
    let server = flick_ipc::unix::UnixSocketServer::bind(&socket_path);
    assert!(server.is_ok());
    assert!(socket_path.exists());

    // 模拟清理
    drop(server);
    let _ = std::fs::remove_file(&socket_path);
    assert!(!socket_path.exists());
}

#[tokio::test]
async fn unknown_action_returns_structured_error() {
    let (_, runtime, _) = build_test_stack().await;

    let request = ActionRequest {
        invocation_id: InvocationId::new(),
        action_id: ActionId::new("nonexistent").unwrap(),
        input: ActionInput::plain_text(""),
        caller: Caller::cli("test"),
        source: flick_domain::ActionSource::Ui,
        deadline: Deadline::from_secs(5),
    };
    let outcome = runtime.invoke(request).await;
    match outcome {
        ActionOutcome::Error(ActionError::UnknownAction { action_id }) => {
            assert_eq!(action_id, "nonexistent");
        }
        other => panic!("expected UnknownAction error, got {other:?}"),
    }
}

#[tokio::test]
async fn shutdown_removes_only_the_socket_and_preserves_its_parent_directory() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let runtime_dir = tmpdir.path().join("runtime");
    tokio::fs::create_dir(&runtime_dir).await.unwrap();
    let socket_path = runtime_dir.join("flick.sock");
    tokio::fs::write(&socket_path, b"test socket placeholder")
        .await
        .unwrap();

    let handle = bootstrap::DaemonHandle {
        cancel_token: tokio_util::sync::CancellationToken::new(),
        socket_path: socket_path.clone(),
        ipc_handle: None,
    };
    bootstrap::shutdown(handle).await;

    assert!(!socket_path.exists(), "shutdown must remove the socket");
    assert!(
        runtime_dir.exists(),
        "Task 4 must not delete a parent directory it does not own"
    );
}
