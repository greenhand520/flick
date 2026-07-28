//! Contract tests for `flick-runtime`.
//!
//! Each `#[test]` corresponds to a named contract scenario.

use flick_domain::{
    ActionContext, ActionDescriptor, ActionError, ActionId, ActionOutcome, ActionRequest,
    Caller, ConcurrencyPolicy, Deadline, InputKind, Matcher,
};
use flick_runtime::{ActionHandler, ActionRuntime};
use std::sync::Arc;
use std::time::Duration;

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Echo handler: returns the raw input text as `ActionOutcome::Data`.
struct EchoHandler;

#[async_trait::async_trait]
impl ActionHandler for EchoHandler {
    async fn invoke(
        &self,
        request: ActionRequest,
        _ctx: ActionContext,
    ) -> Result<ActionOutcome, ActionError> {
        // Strip "echo " prefix if present, otherwise return raw text.
        let text = request.input.text;
        let stripped = text
            .strip_prefix("echo ")
            .unwrap_or(&text)
            .to_string();
        Ok(ActionOutcome::Data(serde_json::json!({ "echo": stripped })))
    }
}

/// A slow handler that sleeps and cooperatively checks cancellation.
/// Used to test deadline / cancel paths.
struct SlowHandler {
    delay: Duration,
}

#[async_trait::async_trait]
impl ActionHandler for SlowHandler {
    async fn invoke(
        &self,
        _request: ActionRequest,
        ctx: ActionContext,
    ) -> Result<ActionOutcome, ActionError> {
        tokio::select! {
            _ = tokio::time::sleep(self.delay) => {
                Ok(ActionOutcome::Notice { message: "slow done".into() })
            }
            _ = ctx.cancellation.cancelled() => {
                Err(ActionError::Cancelled)
            }
        }
    }
}

fn echo_descriptor() -> ActionDescriptor {
    ActionDescriptor {
        id: ActionId::from_static("flick.builtin.echo.v1.echo"),
        title: "Echo".into(),
        keywords: vec!["echo".into(), "repeat".into()],
        matcher: Matcher::prefix_command("echo"),
        concurrency: ConcurrencyPolicy::Parallel,
    }
}

fn echo_request(text: &str, caller: Caller) -> ActionRequest {
    ActionRequest {
        invocation_id: flick_domain::InvocationId::new(),
        action_id: ActionId::from_static("flick.builtin.echo.v1.echo"),
        input: flick_domain::ActionInput::plain_text(text),
        caller,
        deadline: Deadline::from_secs(5),
    }
}

/// Build a runtime with the echo action pre-registered.
async fn runtime_with_echo() -> ActionRuntime {
    let rt = ActionRuntime::new();
    rt.register(echo_descriptor(), Arc::new(EchoHandler)).await;
    rt
}

// ─── Contract Tests ──────────────────────────────────────────────────────────

/// Scenario: routing "echo hello" returns the echo action, and invoking
/// it from both UI and CLI callers yields the expected data.
#[tokio::test]
async fn runtime_routes_echo_for_plain_text_and_executes_for_ui_and_cli() {
    let rt = runtime_with_echo().await;

    // ── Routing ──
    let routes = rt.route("echo hello", InputKind::PlainText).await;
    assert!(!routes.is_empty(), "echo should be routed for 'echo hello'");
    assert_eq!(
        routes[0].action_id,
        ActionId::from_static("flick.builtin.echo.v1.echo")
    );

    // ── Invoke from UI caller ──
    let ui_req = echo_request("echo hello from ui", Caller::ui("search-panel"));
    let outcome = rt.invoke(ui_req).await;
    match outcome {
        ActionOutcome::Data(val) => {
            assert_eq!(val["echo"], "hello from ui");
        }
        other => panic!("expected Data, got {other:?}"),
    }

    // ── Invoke from CLI caller ──
    let cli_req = echo_request("echo hello from cli", Caller::cli("flick-cli"));
    let outcome = rt.invoke(cli_req).await;
    match outcome {
        ActionOutcome::Data(val) => {
            assert_eq!(val["echo"], "hello from cli");
        }
        other => panic!("expected Data, got {other:?}"),
    }
}

/// Scenario: "echo hello" matches via prefix-command (strips "echo "
/// prefix → "hello"), while "echohello" does NOT match because there
/// is no word boundary after "echo".
#[tokio::test]
async fn exact_echo_prefix_binds_remaining_text_without_special_ui_path() {
    let rt = runtime_with_echo().await;

    // "echo hello" → matches, remainder is "hello"
    let routes = rt.route("echo hello", InputKind::PlainText).await;
    assert_eq!(routes.len(), 1, "expected exactly one route for 'echo hello'");
    assert_eq!(
        routes[0].action_id,
        ActionId::from_static("flick.builtin.echo.v1.echo")
    );

    // Invoke to confirm the text is correctly stripped
    let req = echo_request("echo hello", Caller::cli("test"));
    let outcome = rt.invoke(req).await;
    match outcome {
        ActionOutcome::Data(val) => {
            assert_eq!(val["echo"], "hello");
        }
        other => panic!("expected Data with 'hello', got {other:?}"),
    }

    // "echohello" → must NOT match (no word boundary)
    let routes = rt.route("echohello", InputKind::PlainText).await;
    assert!(
        routes.is_empty(),
        "'echohello' must not match echo prefix — no word boundary"
    );
}

/// Scenario: cancelling an in-flight invocation returns a Cancelled
/// outcome (or the handler stops early).
#[tokio::test]
async fn cancelling_an_invocation_returns_cancelled_outcome_or_stops_work() {
    let rt = ActionRuntime::new();

    // Register a slow handler that takes 10s but checks cancellation.
    let slow_id = ActionId::from_static("flick.builtin.slow.v1.run");
    rt.register(
        ActionDescriptor {
            id: slow_id.clone(),
            title: "Slow".into(),
            keywords: vec!["slow".into()],
            matcher: Matcher::Keyword,
            concurrency: ConcurrencyPolicy::Parallel,
        },
        Arc::new(SlowHandler {
            delay: Duration::from_secs(10),
        }),
    )
    .await;

    let invocation_id = flick_domain::InvocationId::new();
    let req = ActionRequest {
        invocation_id,
        action_id: slow_id,
        input: flick_domain::ActionInput::plain_text("slow"),
        caller: Caller::cli("test"),
        deadline: Deadline::from_secs(60), // long deadline so we cancel first
    };

    // Spawn invoke in background, cancel after a short delay.
    let rt_arc = Arc::new(rt);
    let rt_clone = Arc::clone(&rt_arc);
    let invoke_handle = tokio::spawn(async move { rt_clone.invoke(req).await });

    // Give the handler a moment to start, then cancel.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let cancelled = rt_arc.cancel(invocation_id).await;
    assert!(cancelled, "cancel() should return true for a running invocation");

    let outcome = invoke_handle.await.expect("invoke task should not panic");
    assert!(
        matches!(outcome, ActionOutcome::Error(ActionError::Cancelled)),
        "expected Cancelled outcome, got {outcome:?}"
    );
}

/// Scenario: invoking an action that is NOT in the registry returns a
/// structured `UnknownAction` error — never a panic.
#[tokio::test]
async fn unknown_action_is_structured_not_a_panic() {
    let rt = ActionRuntime::new();

    let req = ActionRequest {
        invocation_id: flick_domain::InvocationId::new(),
        action_id: ActionId::from_static("does.not.exist"),
        input: flick_domain::ActionInput::plain_text("anything"),
        caller: Caller::cli("test"),
        deadline: Deadline::from_secs(5),
    };

    let outcome = rt.invoke(req).await;
    match outcome {
        ActionOutcome::Error(ActionError::UnknownAction { action_id }) => {
            assert_eq!(action_id, "does.not.exist");
            // Verify it has a stable error code
            let err = ActionError::UnknownAction {
                action_id: action_id.clone(),
            };
            assert_eq!(err.code(), "UNKNOWN_ACTION");
        }
        other => panic!("expected UnknownAction error, got {other:?}"),
    }
}
