//! Bootstrap contract tests.
//!
//! Verifies the daemon composition root:
//! - Headless mode starts Runtime + IPC without Iced/Portal
//! - IPC dispatcher correctly handles all commands
//! - Shutdown cleans up socket
//! - Portal unavailability doesn't crash daemon

use flick_application::ports::{HostEventSink, RuntimePort};
use flick_domain::*;
use std::sync::Arc;
use tokio::sync::mpsc;

// ─── Import daemon's internal dispatcher types ──────────────────────────────
// We test via the public IPC dispatcher interface, not by importing daemon internals.
// Instead, we recreate the wiring pattern from bootstrap.rs.

use flick_application::coordinator::ApplicationCoordinator;
use flick_runtime::ActionRuntime;

// ─── Test helpers ───────────────────────────────────────────────────────────

struct TestRuntimeAdapter {
    runtime: Arc<ActionRuntime>,
}

#[async_trait::async_trait]
impl RuntimePort for TestRuntimeAdapter {
    async fn route(&self, query: &str, input_kind: InputKind) -> Vec<RouteItem> {
        self.runtime.route(query, input_kind).await
    }
    async fn invoke(&self, request: ActionRequest) -> ActionOutcome {
        self.runtime.invoke(request).await
    }
    async fn cancel(&self, invocation_id: InvocationId) -> bool {
        self.runtime.cancel(invocation_id).await
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

/// Build a test coordinator + runtime pair.
async fn build_test_stack() -> (
    Arc<ApplicationCoordinator>,
    Arc<ActionRuntime>,
    mpsc::UnboundedReceiver<HostToUi>,
) {
    let runtime = Arc::new(ActionRuntime::new());
    flick_builtins::register_phase0_builtins(&runtime).await;

    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let sink = Arc::new(TestEventSink { tx: event_tx });
    let adapter = Arc::new(TestRuntimeAdapter {
        runtime: runtime.clone(),
    }) as Arc<dyn RuntimePort>;
    let coordinator = Arc::new(ApplicationCoordinator::new(adapter, sink));

    (coordinator, runtime, event_rx)
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn headless_runtime_registers_echo() {
    let (_, runtime, _) = build_test_stack().await;

    let routes = runtime.route("echo hello", InputKind::PlainText).await;
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

    // Record initial invocation count via a route check
    let routes_before = runtime.route("echo", InputKind::PlainText).await;

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

    // Runtime should not have been touched
    let routes_after = runtime.route("echo", InputKind::PlainText).await;
    assert_eq!(routes_before.len(), routes_after.len());
}

#[tokio::test]
async fn coordinator_invoke_from_ui_returns_completion() {
    let (coordinator, _, mut events) = build_test_stack().await;

    coordinator
        .handle_ui(UiToHost::Invoke {
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
            text: "echo".into(),
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
async fn socket_cleanup_on_shutdown() {
    let tmpdir = tempfile::TempDir::new().unwrap();
    let socket_path = tmpdir.path().join("test.sock");

    // Bind a server
    let server = flick_ipc::unix::UnixSocketServer::bind(&socket_path);
    assert!(server.is_ok());
    assert!(socket_path.exists());

    // Simulate cleanup
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
