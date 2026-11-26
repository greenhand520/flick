//! `flick-runtime` 的契约测试。
//!
//! 每个 `#[test]` 对应一个具名的契约场景。

use flick_domain::{
    ActionContext, ActionDescriptor, ActionError, ActionId, ActionOutcome, ActionRequest, Caller,
    ConcurrencyPolicy, Deadline, InputType,
};
use flick_runtime::{ActionHandler, ActionRuntime};
use std::sync::Arc;
use std::time::Duration;

// ─── 辅助函数 ─────────────────────────────────────────────────────────────────

/// Echo 处理器：将原始输入文本作为 `ActionOutcome::Data` 返回。
struct EchoHandler;

#[async_trait::async_trait]
impl ActionHandler for EchoHandler {
    async fn invoke(
        &self,
        request: ActionRequest,
        _ctx: ActionContext,
    ) -> Result<ActionOutcome, ActionError> {
        // 如果存在 "echo " 前缀则将其去掉，否则返回原始文本。
        let text = request.input.text;
        let stripped = text.strip_prefix("echo ").unwrap_or(&text).to_string();
        Ok(ActionOutcome::Data(serde_json::json!({ "echo": stripped })))
    }
}

/// 一个会休眠并协作式检查取消的慢速处理器。
/// 用于测试截止时间 / 取消路径。
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
    ActionDescriptor::new(
        ActionId::from_static("flick.builtin.echo.v1.echo"),
        "Echo",
        vec!["echo".into(), "repeat".into()],
        ConcurrencyPolicy::Parallel,
    )
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

/// 构建一个预注册了 echo 动作的运行时。
async fn runtime_with_echo() -> ActionRuntime {
    let rt = ActionRuntime::new();
    rt.register(echo_descriptor(), Arc::new(EchoHandler)).await;
    rt
}

// ─── 契约测试 ─────────────────────────────────────────────────────────────────

/// 场景：路由 "echo hello" 会返回 echo 动作，并且从
/// UI 和 CLI 调用方调用它都会得到预期的数据。
#[tokio::test]
async fn runtime_routes_echo_for_plain_text_and_executes_for_ui_and_cli() {
    let rt = runtime_with_echo().await;

    // ── 路由 ──
    let routes = rt.route("echo hello", InputType::PlainText).await;
    assert!(!routes.is_empty(), "echo should be routed for 'echo hello'");
    assert_eq!(
        routes[0].action_id,
        ActionId::from_static("flick.builtin.echo.v1.echo")
    );

    // ── 从 UI 调用方调用 ──
    let ui_req = echo_request("echo hello from ui", Caller::ui("search-panel"));
    let outcome = rt.invoke(ui_req).await;
    match outcome {
        ActionOutcome::Data(val) => {
            assert_eq!(val["echo"], "hello from ui");
        }
        other => panic!("expected Data, got {other:?}"),
    }

    // ── 从 CLI 调用方调用 ──
    let cli_req = echo_request("echo hello from cli", Caller::cli("flick-cli"));
    let outcome = rt.invoke(cli_req).await;
    match outcome {
        ActionOutcome::Data(val) => {
            assert_eq!(val["echo"], "hello from cli");
        }
        other => panic!("expected Data, got {other:?}"),
    }
}

/// 场景："echo hello" 通过 `echo` 关键词命中；运行时把完整 query
/// 交给 Action，Action 自行解析（测试 handler 自行剥离 `echo ` 前缀）。
/// "echohello" 不命中，因为关键词召回不是任意子串匹配。
#[tokio::test]
async fn echo_keyword_routes_and_action_receives_full_query() {
    let rt = runtime_with_echo().await;

    // "echo hello" → 关键词 "echo" 命中
    let routes = rt.route("echo hello", InputType::PlainText).await;
    assert_eq!(
        routes.len(),
        1,
        "expected exactly one route for 'echo hello'"
    );
    assert_eq!(
        routes[0].action_id,
        ActionId::from_static("flick.builtin.echo.v1.echo")
    );

    // 运行时传完整 query；测试 handler 自行剥离前缀
    let req = echo_request("echo hello", Caller::cli("test"));
    let outcome = rt.invoke(req).await;
    match outcome {
        ActionOutcome::Data(val) => {
            assert_eq!(val["echo"], "hello");
        }
        other => panic!("expected Data with 'hello', got {other:?}"),
    }

    // "echohello" → 不命中关键词 "echo"
    let routes = rt.route("echohello", InputType::PlainText).await;
    assert!(
        routes.is_empty(),
        "'echohello' must not match the echo keyword"
    );
}

/// 场景：取消正在执行中的调用会返回 Cancelled
/// 结果（或者处理器提前停止）。
#[tokio::test]
async fn cancelling_an_invocation_returns_cancelled_outcome_or_stops_work() {
    let rt = ActionRuntime::new();

    // 注册一个耗时 10 秒但会检查取消的慢速处理器。
    let slow_id = ActionId::from_static("flick.builtin.slow.v1.run");
    rt.register(
        ActionDescriptor::new(
            slow_id.clone(),
            "Slow",
            vec!["slow".into()],
            ConcurrencyPolicy::Parallel,
        ),
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
        deadline: Deadline::from_secs(60), // 较长的截止时间，以便我们先取消
    };

    // 在后台生成调用任务，短暂延迟后取消。
    let rt_arc = Arc::new(rt);
    let rt_clone = Arc::clone(&rt_arc);
    let invoke_handle = tokio::spawn(async move { rt_clone.invoke(req).await });

    // 给处理器一点启动时间，然后取消。
    tokio::time::sleep(Duration::from_millis(50)).await;
    let cancelled = rt_arc.cancel(invocation_id).await;
    assert!(
        cancelled,
        "cancel() should return true for a running invocation"
    );

    let outcome = invoke_handle.await.expect("invoke task should not panic");
    assert!(
        matches!(outcome, ActionOutcome::Error(ActionError::Cancelled)),
        "expected Cancelled outcome, got {outcome:?}"
    );
}

/// 场景：调用不在注册表中的动作会返回结构化的
/// `UnknownAction` 错误 — 而不会发生 panic。
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
            // 验证它具有稳定的错误码
            let err = ActionError::UnknownAction {
                action_id: action_id.clone(),
            };
            assert_eq!(err.code(), "UNKNOWN_ACTION");
        }
        other => panic!("expected UnknownAction error, got {other:?}"),
    }
}
