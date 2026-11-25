//! `ApplicationCoordinator` 的集成测试。
//!
//! 每个测试都用伪造的 Runtime 和 Sink 实现构建协调器，
//! 并端到端验证消息流程。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use flick_application::{ApplicationCoordinator, HostEventSink, RuntimePort};
use flick_domain::*;

// ─── 伪造实现 ───────────────────────────────────────────────────────────────────

/// 具有可配置响应的伪造 `RuntimePort`。
struct FakeRuntime {
    /// 从查询文本到（路由、模拟延迟）的映射。
    route_responses: tokio::sync::Mutex<HashMap<String, (Vec<RouteItem>, Duration)>>,
    /// `invoke` 返回的结果。
    invoke_outcome: Mutex<ActionOutcome>,
    /// 记录每一次 `invoke` 调用以供断言。
    invocations: Arc<Mutex<Vec<ActionRequest>>>,
    /// 记录每一次 `cancel` 调用以供断言。
    cancellations: Arc<Mutex<Vec<InvocationId>>>,
}

impl FakeRuntime {
    fn new() -> Self {
        Self {
            route_responses: tokio::sync::Mutex::new(HashMap::new()),
            invoke_outcome: Mutex::new(ActionOutcome::Notice {
                message: "ok".into(),
            }),
            invocations: Arc::new(Mutex::new(Vec::new())),
            cancellations: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn with_routes(routes: HashMap<String, (Vec<RouteItem>, Duration)>) -> Self {
        let s = Self::new();
        // 将路由存入一个临时变量，随后换入
        // 由于刚刚创建了该结构体，使用 try_lock
        *s.route_responses.try_lock().unwrap() = routes;
        s
    }

    fn with_invoke_outcome(outcome: ActionOutcome) -> Self {
        let s = Self::new();
        *s.invoke_outcome.lock().unwrap() = outcome;
        s
    }
}

#[async_trait]
impl RuntimePort for FakeRuntime {
    async fn route(&self, query: &str, _input_kind: InputType) -> Vec<RouteItem> {
        // 在休眠前将数据从锁中克隆出来。
        let result = {
            let map = self.route_responses.lock().await;
            map.get(query).cloned()
        };
        match result {
            Some((items, delay)) => {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                items
            }
            None => vec![],
        }
    }

    async fn invoke(&self, request: ActionRequest) -> ActionOutcome {
        self.invocations.lock().unwrap().push(request);
        self.invoke_outcome.lock().unwrap().clone()
    }

    async fn cancel(&self, invocation_id: InvocationId) -> bool {
        self.cancellations.lock().unwrap().push(invocation_id);
        true
    }
}

/// 收集所发出事件的伪造 `HostEventSink`。
struct FakeSink {
    events: Arc<Mutex<Vec<HostToUi>>>,
}

impl FakeSink {
    fn new() -> (Self, Arc<Mutex<Vec<HostToUi>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                events: events.clone(),
            },
            events,
        )
    }
}

impl HostEventSink for FakeSink {
    fn send(&self, event: HostToUi) {
        self.events.lock().unwrap().push(event);
    }
}

// ─── 辅助函数 ─────────────────────────────────────────────────────────────────

fn make_route_item(id: &str, title: &str, score: f64) -> RouteItem {
    RouteItem {
        action_id: ActionId::new(id).unwrap(),
        title: title.into(),
        score,
    }
}

// ─── 测试 ───────────────────────────────────────────────────────────────────

/// 两个查询被并发发送。第一个（较慢）查询的结果必须被丢弃，
/// 因为第二个（较快）查询取代了它。
#[tokio::test]
async fn newer_query_generation_wins_when_results_complete_out_of_order() {
    let gen1 = QueryGeneration::INITIAL;
    let gen2 = gen1.next();

    let mut routes = HashMap::new();
    routes.insert(
        "first".into(),
        (
            vec![make_route_item("id_a", "A", 1.0)],
            Duration::from_millis(200),
        ),
    );
    routes.insert(
        "second".into(),
        (vec![make_route_item("id_b", "B", 1.0)], Duration::ZERO),
    );

    let runtime = Arc::new(FakeRuntime::with_routes(routes));
    let (sink, events) = FakeSink::new();
    let coord = Arc::new(ApplicationCoordinator::new(runtime, Arc::new(sink)));

    // 先生成较慢的查询任务。
    let c1 = coord.clone();
    let h1 = tokio::spawn(async move {
        c1.handle_ui(UiToHost::QueryChanged {
            generation: gen1,
            text: "first".into(),
        })
        .await;
    });

    // 给 gen1 时间执行 set_current 并启动其慢速路由。
    tokio::time::sleep(Duration::from_millis(20)).await;

    // 现在生成较快的查询任务。
    let c2 = coord.clone();
    let h2 = tokio::spawn(async move {
        c2.handle_ui(UiToHost::QueryChanged {
            generation: gen2,
            text: "second".into(),
        })
        .await;
    });

    let _ = tokio::join!(h1, h2);

    let emitted = events.lock().unwrap();
    assert_eq!(emitted.len(), 1, "only the newer generation should emit");
    match &emitted[0] {
        HostToUi::Routes { generation, items } => {
            assert_eq!(*generation, gen2);
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].action_id.as_str(), "id_b");
        }
        other => panic!("expected Routes, got {other:?}"),
    }
}

/// 来自 UI 的 `Invoke` 必须使用 `Caller::ui("search-panel")`，并且
/// `InvocationCompleted` 事件必须被转发到接收器。
#[tokio::test]
async fn invoke_from_ui_uses_ui_caller_and_forwards_completion() {
    let inv_id = InvocationId::new();
    let action_id = ActionId::from_static("test.action.v1.run");
    let expected_outcome = ActionOutcome::Notice {
        message: "done".into(),
    };

    let runtime = Arc::new(FakeRuntime::with_invoke_outcome(expected_outcome.clone()));
    let invocations = runtime.invocations.clone();
    let (sink, events) = FakeSink::new();
    let coord = ApplicationCoordinator::new(runtime, Arc::new(sink));

    coord
        .handle_ui(UiToHost::Invoke {
            invocation_id: inv_id,
            action_id: action_id.clone(),
            input: ActionInput::plain_text("hello"),
        })
        .await;

    // 验证运行时收到了正确的请求。
    {
        let invocs = invocations.lock().unwrap();
        assert_eq!(invocs.len(), 1);
        assert_eq!(invocs[0].caller, Caller::ui("search-panel"));
        assert_eq!(invocs[0].invocation_id, inv_id);
        assert_eq!(invocs[0].action_id, action_id);
        assert_eq!(invocs[0].input.text, "hello");
    }

    // 验证完成事件已被转发。
    {
        let emitted = events.lock().unwrap();
        assert_eq!(emitted.len(), 1);
        match &emitted[0] {
            HostToUi::InvocationCompleted {
                invocation_id,
                outcome,
            } => {
                assert_eq!(*invocation_id, inv_id);
                assert_eq!(format!("{outcome:?}"), format!("{expected_outcome:?}"));
            }
            other => panic!("expected InvocationCompleted, got {other:?}"),
        }
    }
}

/// 来自 Portal 的 `ShowSearchPanel` 意图必须发出相应的
/// `HostToUi` 事件，**而不**调用运行时。
#[test]
fn portal_show_search_emits_host_to_ui_without_invoking_runtime() {
    let runtime = Arc::new(FakeRuntime::new());
    let invocations = runtime.invocations.clone();
    let (sink, events) = FakeSink::new();
    let coord = ApplicationCoordinator::new(runtime, Arc::new(sink));

    coord.handle_intent(UiIntent::ShowSearchPanel {
        activation_token: Some("portal-token-42".into()),
    });

    // 事件应按所提供的原样发出。
    {
        let emitted = events.lock().unwrap();
        assert_eq!(emitted.len(), 1);
        match &emitted[0] {
            HostToUi::ShowSearchPanel { activation_token } => {
                assert_eq!(activation_token.as_deref(), Some("portal-token-42"));
            }
            other => panic!("expected ShowSearchPanel, got {other:?}"),
        }
    }

    // 运行时必须未被触碰。
    assert!(invocations.lock().unwrap().is_empty());
}

/// 连续两次切换必须依次发出 Show 和 Hide，且运行时
/// 必须保持未被触碰。
#[test]
fn toggle_emits_show_then_hide_without_stopping_runtime() {
    let runtime = Arc::new(FakeRuntime::new());
    let invocations = runtime.invocations.clone();
    let (sink, events) = FakeSink::new();
    let coord = ApplicationCoordinator::new(runtime, Arc::new(sink));

    // 第一次切换：面板原本隐藏 → 显示。
    coord.toggle_search_panel();
    {
        let emitted = events.lock().unwrap();
        assert_eq!(emitted.len(), 1, "first toggle should emit 1 event");
        match &emitted[0] {
            HostToUi::ShowSearchPanel { activation_token } => {
                assert!(
                    activation_token.is_none(),
                    "toggle should not carry an activation token"
                );
            }
            other => panic!("expected ShowSearchPanel, got {other:?}"),
        }
    }

    // 第二次切换：面板原本显示 → 隐藏。
    coord.toggle_search_panel();
    {
        let emitted = events.lock().unwrap();
        assert_eq!(emitted.len(), 2, "two toggles should emit 2 events");
        match &emitted[1] {
            HostToUi::HideSearchPanel => {}
            other => panic!("expected HideSearchPanel, got {other:?}"),
        }
    }

    // 运行时未被触碰。
    assert!(invocations.lock().unwrap().is_empty());
}
