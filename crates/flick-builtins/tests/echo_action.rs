//! Echo 内置动作的测试。

use flick_builtins::echo::{ECHO_ACTION_ID, EchoHandler};
use flick_domain::{ActionContext, ActionOutcome, ActionRequest, Caller};
use flick_runtime::ActionHandler;
use tokio_util::sync::CancellationToken;

fn test_ctx() -> ActionContext {
    ActionContext {
        cancellation: CancellationToken::new(),
    }
}

#[test]
fn echo_action_id_constant_has_expected_value() {
    assert_eq!(ECHO_ACTION_ID, "flick.builtin.echo.v1.echo");
}

#[tokio::test]
async fn echo_returns_data_with_text() {
    let handler = EchoHandler;
    let request = ActionRequest::new_echo("hello world", Caller::cli("test"));
    let ctx = test_ctx();
    let outcome = handler
        .invoke(request, ctx)
        .await
        .expect("invoke should succeed");
    match outcome {
        ActionOutcome::Data(val) => {
            assert_eq!(val["echo"], "hello world");
        }
        other => panic!("expected ActionOutcome::Data, got {other:?}"),
    }
}

#[tokio::test]
async fn echo_preserves_empty_string() {
    let handler = EchoHandler;
    let request = ActionRequest::new_echo("", Caller::ui("panel"));
    let ctx = test_ctx();
    let outcome = handler
        .invoke(request, ctx)
        .await
        .expect("invoke should succeed");
    match outcome {
        ActionOutcome::Data(val) => {
            assert_eq!(val["echo"], "");
        }
        other => panic!("expected ActionOutcome::Data, got {other:?}"),
    }
}

#[tokio::test]
async fn echo_does_not_vary_by_caller_kind() {
    let handler = EchoHandler;
    let ctx1 = test_ctx();
    let ctx2 = test_ctx();
    let req_ui = ActionRequest::new_echo("same", Caller::ui("panel"));
    let req_cli = ActionRequest::new_echo("same", Caller::cli("cli"));
    let out_ui = handler.invoke(req_ui, ctx1).await.expect("invoke ok");
    let out_cli = handler.invoke(req_cli, ctx2).await.expect("invoke ok");
    match (&out_ui, &out_cli) {
        (ActionOutcome::Data(a), ActionOutcome::Data(b)) => assert_eq!(a, b),
        _ => panic!("expected Data for both callers"),
    }
}

#[tokio::test]
async fn echo_plugin_registers_and_routes() {
    use flick_runtime::ActionRuntime;
    let rt = ActionRuntime::default();
    rt.register_plugin(&flick_builtins::EchoPlugin).await.unwrap();
    let routes = rt
        .route("echo hello", flick_domain::InputKind::PlainText)
        .await;
    assert_eq!(
        routes[0].action_id.as_str(),
        flick_builtins::ECHO_ACTION_ID
    );
}

#[tokio::test]
async fn echo_registered_in_runtime_is_findable() {
    use flick_builtins::register_phase0_builtins;
    use flick_domain::ActionId;

    let runtime = flick_runtime::ActionRuntime::new();
    register_phase0_builtins(&runtime).await;

    let id = ActionId::from_static(ECHO_ACTION_ID);
    let results = runtime
        .route("echo hello", flick_domain::InputKind::PlainText)
        .await;
    assert!(!results.is_empty(), "echo should be routable");
    assert_eq!(results[0].action_id, id);
}
