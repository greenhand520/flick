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

/// 构造文本查询消息，减少协议字段变化对测试的重复修改。
fn text_query(generation: QueryGeneration, text: &str) -> UiToHost {
    UiToHost::QueryChanged {
        generation,
        input: SearchInputSnapshot::text(text, InputSource::Manual),
    }
}

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
    /// `surface_event` 的返回值。
    surface_event_result: Mutex<Result<SurfaceContent, String>>,
    /// 记录每一次 `surface_event` 调用。
    surface_events: Arc<Mutex<Vec<(SurfaceSessionId, SurfaceEventKind)>>>,
    /// 记录每一次 `surface_closed` 调用。
    surface_closed: Arc<Mutex<Vec<SurfaceSessionId>>>,
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
            surface_event_result: Mutex::new(Ok(SurfaceContent {
                schema_version: 1,
                title: None,
                body: Vec::new(),
            })),
            surface_events: Arc::new(Mutex::new(Vec::new())),
            surface_closed: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn with_routes(routes: HashMap<String, (Vec<RouteItem>, Duration)>) -> Self {
        let s = Self::new();
        // 将路由存入一个临时变量，随后换入
        // 由于刚刚创建了该结构体，使用 try_lock
        *s.route_responses.try_lock().unwrap() = routes;
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

    async fn surface_event(
        &self,
        session: SurfaceSessionId,
        kind: SurfaceEventKind,
    ) -> Result<SurfaceContent, String> {
        self.surface_events.lock().unwrap().push((session, kind));
        self.surface_event_result.lock().unwrap().clone()
    }

    async fn surface_closed(&self, session: SurfaceSessionId) {
        self.surface_closed.lock().unwrap().push(session);
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
    RouteItem::action(ActionId::new(id).unwrap(), title, score)
}

/// 轮询等待事件数量达到 `n`（最多 2s），否则 panic。
async fn wait_events(events: &Arc<Mutex<Vec<HostToUi>>>, n: usize) {
    for _ in 0..200 {
        if events.lock().unwrap().len() >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "timeout waiting for {n} events, got {}",
        events.lock().unwrap().len()
    );
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
        c1.handle_ui(text_query(gen1, "first")).await;
    });

    // 给 gen1 时间执行 set_current 并启动其慢速路由。
    tokio::time::sleep(Duration::from_millis(20)).await;

    // 现在生成较快的查询任务。
    let c2 = coord.clone();
    let h2 = tokio::spawn(async move {
        c2.handle_ui(text_query(gen2, "second")).await;
    });

    let _ = tokio::join!(h1, h2);

    let emitted = events.lock().unwrap();
    // 两阶段后 `Routes` 仍然在 `route_input` 返回时立刻发出，因此
    // gen1 的慢路由不会产出任何事件，只有 gen2 的 `Routes` 会出现。
    let routes: Vec<&HostToUi> = emitted
        .iter()
        .filter(|event| matches!(event, HostToUi::Routes { .. }))
        .collect();
    assert_eq!(
        routes.len(),
        1,
        "only the newer generation should emit routes"
    );
    match routes[0] {
        HostToUi::Routes { generation, items } => {
            assert_eq!(*generation, gen2);
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].action_id.as_str(), "id_b");
        }
        other => panic!("expected Routes, got {other:?}"),
    }
    // gen1 不得留下任何执行结果。
    assert!(
        !emitted.iter().any(|event| matches!(
            event,
            HostToUi::ExecutionResults { generation, .. } if *generation == gen1
        )),
        "stale generation must not emit ExecutionResults"
    );
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

    // 候选身份校验要求先有当前代次的结果集。
    let mut routes = HashMap::new();
    routes.insert(
        "hello".to_string(),
        (
            vec![RouteItem::action(action_id.clone(), "Run", 1.0)],
            Duration::ZERO,
        ),
    );
    let runtime = Arc::new(FakeRuntime::with_routes(routes));
    *runtime.invoke_outcome.lock().unwrap() = expected_outcome.clone();
    let invocations = runtime.invocations.clone();
    let (sink, events) = FakeSink::new();
    let coord = ApplicationCoordinator::new(runtime, Arc::new(sink));

    coord
        .handle_ui(text_query(QueryGeneration::INITIAL, "hello"))
        .await;

    coord
        .handle_ui(UiToHost::Invoke {
            generation: QueryGeneration::INITIAL,
            item_id: format!("action:{action_id}"),
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

    // 验证完成事件已被转发（第一条是 Routes）。
    {
        let emitted = events.lock().unwrap();
        assert_eq!(emitted.len(), 2, "Routes then InvocationCompleted");
        match &emitted[1] {
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

/// 成功调用后应记录一次使用（frecency）；错误结果不应记录。
#[tokio::test]
async fn successful_invoke_records_usage_and_error_does_not() {
    let action_id = ActionId::from_static("test.action.v1.run");
    let mut routes = HashMap::new();
    routes.insert(
        "hello".to_string(),
        (
            vec![RouteItem::action(action_id.clone(), "Run", 1.0)],
            Duration::ZERO,
        ),
    );

    // 情况一：成功（Notice）。
    let runtime = Arc::new(FakeRuntime::with_routes(routes.clone()));
    let (sink, _events) = FakeSink::new();
    let coord = ApplicationCoordinator::new(runtime, Arc::new(sink));
    let usage = coord.usage_store();
    coord
        .handle_ui(text_query(QueryGeneration::INITIAL, "hello"))
        .await;
    coord
        .handle_ui(UiToHost::Invoke {
            generation: QueryGeneration::INITIAL,
            item_id: format!("action:{action_id}"),
            invocation_id: InvocationId::new(),
            action_id: action_id.clone(),
            input: ActionInput::plain_text("hello"),
        })
        .await;
    assert!(
        usage.frecency_boost(&action_id) > 0.0,
        "成功调用应记录一次使用"
    );

    // 情况二：失败（Error）不应记录。
    let runtime = Arc::new(FakeRuntime::with_routes(routes));
    *runtime.invoke_outcome.lock().unwrap() = ActionOutcome::Error(ActionError::DeadlineExceeded);
    let (sink, _events) = FakeSink::new();
    let coord = ApplicationCoordinator::new(runtime, Arc::new(sink));
    let usage = coord.usage_store();
    coord
        .handle_ui(text_query(QueryGeneration::INITIAL, "hello"))
        .await;
    coord
        .handle_ui(UiToHost::Invoke {
            generation: QueryGeneration::INITIAL,
            item_id: format!("action:{action_id}"),
            invocation_id: InvocationId::new(),
            action_id: action_id.clone(),
            input: ActionInput::plain_text("hello"),
        })
        .await;
    assert_eq!(
        usage.frecency_boost(&action_id),
        0.0,
        "错误结果不应记录使用"
    );
}

/// 没有当前路由快照时（例如 UI 直接构造 Invoke），宿主必须拒绝。
#[tokio::test]
async fn invoke_without_current_routes_is_rejected() {
    let inv_id = InvocationId::new();
    let runtime = Arc::new(FakeRuntime::new());
    let invocations = runtime.invocations.clone();
    let (sink, events) = FakeSink::new();
    let coord = ApplicationCoordinator::new(runtime, Arc::new(sink));

    coord
        .handle_ui(UiToHost::Invoke {
            generation: QueryGeneration::INITIAL,
            item_id: "action:test.action.v1.run".into(),
            invocation_id: inv_id,
            action_id: ActionId::from_static("test.action.v1.run"),
            input: ActionInput::plain_text("hello"),
        })
        .await;

    assert!(
        invocations.lock().unwrap().is_empty(),
        "runtime must not be invoked"
    );
    let emitted = events.lock().unwrap();
    match emitted.as_slice() {
        [
            HostToUi::InvocationCompleted {
                invocation_id,
                outcome: ActionOutcome::Error(ActionError::UnknownAction { .. }),
            },
        ] => assert_eq!(*invocation_id, inv_id),
        other => panic!("expected rejection completion, got {other:?}"),
    }
}

/// 宿主从未发出过的「未来代次」必须被拒绝（UI 不可能持有）。
#[tokio::test]
async fn invoke_with_future_generation_is_rejected() {
    let action_id = ActionId::from_static("test.action.v1.run");
    let mut routes = HashMap::new();
    routes.insert(
        "hello".to_string(),
        (
            vec![RouteItem::action(action_id.clone(), "Run", 1.0)],
            Duration::ZERO,
        ),
    );
    let runtime = Arc::new(FakeRuntime::with_routes(routes));
    let invocations = runtime.invocations.clone();
    let (sink, events) = FakeSink::new();
    let coord = ApplicationCoordinator::new(runtime, Arc::new(sink));

    coord
        .handle_ui(text_query(QueryGeneration::INITIAL, "hello"))
        .await;
    let newer = QueryGeneration::INITIAL.next();
    coord.handle_ui(text_query(newer, "hello")).await;

    // 宿主只见过 INITIAL 与 INITIAL.next()，INITIAL.next().next() 是未来代次。
    let future = QueryGeneration::INITIAL.next().next();
    let inv_id = InvocationId::new();
    coord
        .handle_ui(UiToHost::Invoke {
            generation: future,
            item_id: format!("action:{action_id}"),
            invocation_id: inv_id,
            action_id,
            input: ActionInput::plain_text("hello"),
        })
        .await;

    assert!(invocations.lock().unwrap().is_empty());
    let emitted = events.lock().unwrap();
    assert!(matches!(
        emitted.last(),
        Some(HostToUi::InvocationCompleted {
            outcome: ActionOutcome::Error(ActionError::UnknownAction { .. }),
            ..
        })
    ));
}

/// 手动累积多代候选后，提交「上一代遗留、不在最新批」的候选仍应被接受。
#[tokio::test]
async fn invoke_with_accumulated_older_candidate_is_accepted() {
    let action_a = ActionId::from_static("test.action.v1.first");
    let action_b = ActionId::from_static("test.action.v1.second");
    let mut routes = HashMap::new();
    routes.insert(
        "first".to_string(),
        (
            vec![RouteItem::action(action_a.clone(), "First", 1.0)],
            Duration::ZERO,
        ),
    );
    // 第二批不含 A，只有 B。
    routes.insert(
        "second".to_string(),
        (
            vec![RouteItem::action(action_b.clone(), "Second", 1.0)],
            Duration::ZERO,
        ),
    );
    let runtime = Arc::new(FakeRuntime::with_routes(routes));
    let invocations = runtime.invocations.clone();
    let (sink, events) = FakeSink::new();
    let coord = ApplicationCoordinator::new(runtime, Arc::new(sink));

    let gen_a = QueryGeneration::INITIAL;
    let gen_b = gen_a.next();
    coord.handle_ui(text_query(gen_a, "first")).await;
    coord.handle_ui(text_query(gen_b, "second")).await;

    // A 属于旧代次、已不在最新批，但仍在会话并集里，应可提交。
    let inv_id = InvocationId::new();
    coord
        .handle_ui(UiToHost::Invoke {
            generation: gen_b,
            item_id: format!("action:{action_a}"),
            invocation_id: inv_id,
            action_id: action_a.clone(),
            input: ActionInput::plain_text("first"),
        })
        .await;

    // 被接受并实际调用运行时。
    {
        let invocs = invocations.lock().unwrap();
        assert_eq!(
            invocs.len(),
            1,
            "accumulated older candidate must be accepted"
        );
        assert_eq!(invocs[0].action_id, action_a);
    }
    // 最后事件不是 UnknownAction。
    let emitted = events.lock().unwrap();
    assert!(
        !matches!(
            emitted.last(),
            Some(HostToUi::InvocationCompleted {
                outcome: ActionOutcome::Error(ActionError::UnknownAction { .. }),
                ..
            })
        ),
        "accepted invocation must not report UnknownAction, got {:?}",
        emitted.last()
    );
}

/// 代次正确但候选不在当前结果集里的也必须被拒绝。
#[tokio::test]
async fn invoke_with_unknown_item_id_is_rejected() {
    let action_id = ActionId::from_static("test.action.v1.run");
    let mut routes = HashMap::new();
    routes.insert(
        "hello".to_string(),
        (
            vec![RouteItem::action(action_id.clone(), "Run", 1.0)],
            Duration::ZERO,
        ),
    );
    let runtime = Arc::new(FakeRuntime::with_routes(routes));
    let invocations = runtime.invocations.clone();
    let (sink, events) = FakeSink::new();
    let coord = ApplicationCoordinator::new(runtime, Arc::new(sink));

    coord
        .handle_ui(text_query(QueryGeneration::INITIAL, "hello"))
        .await;

    coord
        .handle_ui(UiToHost::Invoke {
            generation: QueryGeneration::INITIAL,
            item_id: "action:not.in.routes".into(),
            invocation_id: InvocationId::new(),
            action_id,
            input: ActionInput::plain_text("hello"),
        })
        .await;

    assert!(invocations.lock().unwrap().is_empty());
    let emitted = events.lock().unwrap();
    assert!(matches!(
        emitted.last(),
        Some(HostToUi::InvocationCompleted {
            outcome: ActionOutcome::Error(ActionError::UnknownAction { .. }),
            ..
        })
    ));
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

// ─── Surface 转发测试 ───────────────────────────────────────────────────────

#[tokio::test]
async fn surface_event_forwards_to_runtime_and_emits_updated() {
    let runtime = Arc::new(FakeRuntime::new());
    let session = SurfaceSessionId::new();
    let content = SurfaceContent {
        schema_version: 1,
        title: Some("标题".into()),
        body: Vec::new(),
    };
    *runtime.surface_event_result.lock().unwrap() = Ok(content.clone());
    let recorded = runtime.surface_events.clone();
    let (sink, events) = FakeSink::new();
    let coord = ApplicationCoordinator::new(runtime, Arc::new(sink));

    coord
        .handle_ui(UiToHost::SurfaceEvent {
            session_id: session,
            kind: SurfaceEventKind::Click {
                node_id: "btn".into(),
            },
        })
        .await;

    {
        let rec = recorded.lock().unwrap();
        assert_eq!(rec.len(), 1);
        assert_eq!(rec[0].0, session);
        assert!(matches!(&rec[0].1, SurfaceEventKind::Click { node_id } if node_id == "btn"));
    }

    let emitted = events.lock().unwrap();
    assert_eq!(emitted.len(), 1);
    match &emitted[0] {
        HostToUi::SurfaceUpdated {
            session_id,
            content: got,
        } => {
            assert_eq!(*session_id, session);
            assert_eq!(*got, content);
        }
        other => panic!("expected SurfaceUpdated, got {other:?}"),
    }
}

#[tokio::test]
async fn surface_event_error_emits_surface_failed() {
    let runtime = Arc::new(FakeRuntime::new());
    let session = SurfaceSessionId::new();
    *runtime.surface_event_result.lock().unwrap() = Err("boom".into());
    let (sink, events) = FakeSink::new();
    let coord = ApplicationCoordinator::new(runtime, Arc::new(sink));

    coord
        .handle_ui(UiToHost::SurfaceEvent {
            session_id: session,
            kind: SurfaceEventKind::SearchChanged { text: "x".into() },
        })
        .await;

    let emitted = events.lock().unwrap();
    assert_eq!(emitted.len(), 1);
    match &emitted[0] {
        HostToUi::SurfaceFailed {
            session_id,
            message,
        } => {
            assert_eq!(*session_id, session);
            assert_eq!(message, "boom");
        }
        other => panic!("expected SurfaceFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn surface_closed_forwards_to_runtime_without_emitting() {
    let runtime = Arc::new(FakeRuntime::new());
    let session = SurfaceSessionId::new();
    let closed = runtime.surface_closed.clone();
    let (sink, events) = FakeSink::new();
    let coord = ApplicationCoordinator::new(runtime, Arc::new(sink));

    coord
        .handle_ui(UiToHost::SurfaceClosed {
            session_id: session,
        })
        .await;

    assert_eq!(*closed.lock().unwrap(), vec![session]);
    assert!(events.lock().unwrap().is_empty());
}

// ─── 搜索阶段 Execution 项 ─────────────────────────────────────────────

/// 同时提供普通路由与搜索阶段执行结果的伪造 Runtime。
struct ExecutionRuntime {
    parent: ActionId,
    execution: RouteItem,
    calls: Arc<Mutex<Vec<(ActionId, String, ExecutionEventKind)>>>,
}

impl ExecutionRuntime {
    fn new() -> Self {
        let parent = ActionId::from_static("flick.builtin.apps.search");
        let item = ExecutionItem {
            item_id: "app:firefox".into(),
            parent_action_id: parent.clone(),
            invocation_id: InvocationId::new(),
            title: "Firefox".into(),
            subtitle: Some("firefox".into()),
            icon: None,
            badge: None,
            output: Default::default(),
        };
        Self {
            parent: parent.clone(),
            execution: RouteItem::execution(item, 0.0),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl RuntimePort for ExecutionRuntime {
    async fn route(&self, _query: &str, _input_kind: InputType) -> Vec<RouteItem> {
        vec![RouteItem::action(self.parent.clone(), "应用搜索", 1.0)]
    }

    async fn search_invoke(
        &self,
        _generation: QueryGeneration,
        _input: ActionInput,
    ) -> Vec<RouteItem> {
        vec![self.execution.clone()]
    }

    async fn execution_event(
        &self,
        parent_action_id: ActionId,
        item_id: &str,
        event: ExecutionEventKind,
    ) -> Option<ActionOutcome> {
        self.calls
            .lock()
            .unwrap()
            .push((parent_action_id, item_id.to_string(), event));
        Some(ActionOutcome::Notice {
            message: "已启动 Firefox".into(),
        })
    }

    async fn invoke(&self, _request: ActionRequest) -> ActionOutcome {
        ActionOutcome::Notice {
            message: "unused".into(),
        }
    }

    async fn cancel(&self, _invocation_id: InvocationId) -> bool {
        false
    }

    async fn surface_event(
        &self,
        _session: SurfaceSessionId,
        _kind: SurfaceEventKind,
    ) -> Result<SurfaceContent, String> {
        Err("unused".into())
    }

    async fn surface_closed(&self, _session: SurfaceSessionId) {}
}

/// 同一代次内必须先发 `Routes`（只含 action candidates），再补发
/// `ExecutionResults`（执行项），两者代次相同。
#[tokio::test]
async fn coordinator_emits_routes_then_execution_results() {
    let runtime = Arc::new(ExecutionRuntime::new());
    let (sink, events) = FakeSink::new();
    let coord = ApplicationCoordinator::new(runtime, Arc::new(sink));

    coord
        .handle_ui(text_query(QueryGeneration::INITIAL, "fire"))
        .await;

    // ExecutionResults 现在由后台任务异步发出，需轮询等待其到达。
    wait_events(&events, 2).await;

    let emitted = events.lock().unwrap();
    assert_eq!(emitted.len(), 2, "Routes then ExecutionResults");
    match &emitted[0] {
        HostToUi::Routes { generation, items } => {
            assert_eq!(*generation, QueryGeneration::INITIAL);
            assert_eq!(items.len(), 1, "Routes must only carry action candidates");
            assert_eq!(items[0].kind, RouteItemKind::Action);
        }
        other => panic!("expected Routes, got {other:?}"),
    }
    match &emitted[1] {
        HostToUi::ExecutionResults { generation, items } => {
            assert_eq!(*generation, QueryGeneration::INITIAL);
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].title, "Firefox");
            assert_eq!(items[0].kind, RouteItemKind::Execution);
        }
        other => panic!("expected ExecutionResults, got {other:?}"),
    }
}

#[tokio::test]
async fn execution_activated_forwards_to_parent_and_emits_completion() {
    let runtime = Arc::new(ExecutionRuntime::new());
    let calls = runtime.calls.clone();
    let parent = runtime.parent.clone();
    let (sink, events) = FakeSink::new();
    let coord = ApplicationCoordinator::new(runtime, Arc::new(sink));
    let invocation_id = InvocationId::new();

    // ExecutionActivated 只对当前查询代次有效，先建立一次查询代次。
    coord
        .handle_ui(text_query(QueryGeneration::INITIAL, "fire"))
        .await;

    coord
        .handle_ui(UiToHost::ExecutionActivated {
            generation: QueryGeneration::INITIAL,
            parent_action_id: parent.clone(),
            invocation_id,
            item_id: Some("app:firefox".into()),
            event: ExecutionEventKind::Activate,
        })
        .await;

    assert_eq!(
        *calls.lock().unwrap(),
        vec![(parent, "app:firefox".into(), ExecutionEventKind::Activate)]
    );
    let emitted = events.lock().unwrap();
    match emitted.last().expect("completion event") {
        HostToUi::InvocationCompleted {
            invocation_id: got,
            outcome: ActionOutcome::Notice { message },
        } => {
            assert_eq!(*got, invocation_id);
            assert!(message.contains("Firefox"));
        }
        other => panic!("expected InvocationCompleted, got {other:?}"),
    }
}

// ─── 按代次取消在途搜索 ───────────────────────────────────────────────

/// `search_invoke` 会长时间阻塞的伪造 Runtime，用于验证 `handle_ui`
/// 不再被搜索阶段串行阻塞（修复前会阻塞在 `search_invoke` 的 `.await` 上）。
struct SlowSearchRuntime;

#[async_trait]
impl RuntimePort for SlowSearchRuntime {
    async fn route(&self, _query: &str, _input_kind: InputType) -> Vec<RouteItem> {
        vec![make_route_item("id_slow", "Slow", 1.0)]
    }

    async fn search_invoke(
        &self,
        _generation: QueryGeneration,
        _input: ActionInput,
    ) -> Vec<RouteItem> {
        // 模拟最长耗时远超 UI 交互预算的在途搜索。
        tokio::time::sleep(Duration::from_secs(5)).await;
        Vec::new()
    }

    async fn invoke(&self, _request: ActionRequest) -> ActionOutcome {
        ActionOutcome::Notice {
            message: "unused".into(),
        }
    }

    async fn cancel(&self, _invocation_id: InvocationId) -> bool {
        false
    }

    async fn surface_event(
        &self,
        _session: SurfaceSessionId,
        _kind: SurfaceEventKind,
    ) -> Result<SurfaceContent, String> {
        Err("unused".into())
    }

    async fn surface_closed(&self, _session: SurfaceSessionId) {}
}

/// 关键回归：`handle_ui(QueryChanged)` 不得被 `search_invoke` 阻塞。
///
/// 修复前，阶段二 `search_invoke` 在 `handle_ui` 内串行 `.await`，这里 50ms
/// 超时必然失败（搜索要睡 5s）；修复后阶段二 spawn 到后台，`handle_ui` 立即
/// 返回，从而新代次能立刻进入并取消在途搜索。
#[tokio::test]
async fn newer_generation_cancels_in_flight_search() {
    let gen1 = QueryGeneration::INITIAL;
    let gen2 = gen1.next();
    let runtime = Arc::new(SlowSearchRuntime);
    let (sink, events) = FakeSink::new();
    let coord = Arc::new(ApplicationCoordinator::new(runtime, Arc::new(sink)));

    // 第一次查询：修复后应立即返回，不被 5s 的 search_invoke 阻塞。
    tokio::time::timeout(
        Duration::from_millis(50),
        coord.handle_ui(text_query(gen1, "a")),
    )
    .await
    .expect("handle_ui must not block on search_invoke (gen1)");

    // 新代次立刻进入：同样应立即返回。
    tokio::time::timeout(
        Duration::from_millis(50),
        coord.handle_ui(text_query(gen2, "b")),
    )
    .await
    .expect("handle_ui must not block on search_invoke (gen2)");

    // 给后台任务一点时间，确认过期代次不会补发 ExecutionResults。
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !events.lock().unwrap().iter().any(|event| matches!(
            event,
            HostToUi::ExecutionResults { generation, .. } if *generation == gen1
        )),
        "stale gen1 must not emit ExecutionResults"
    );
}
