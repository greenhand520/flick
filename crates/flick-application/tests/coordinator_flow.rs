//! Integration tests for `ApplicationCoordinator`.
//!
//! Each test builds a coordinator with fake Runtime and Sink implementations
//! and verifies the message flow end-to-end.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use flick_application::{ApplicationCoordinator, HostEventSink, RuntimePort};
use flick_domain::*;

// ─── Fakes ───────────────────────────────────────────────────────────────────

/// Fake `RuntimePort` with configurable responses.
struct FakeRuntime {
    /// Map from query text → (routes, simulated latency).
    route_responses: tokio::sync::Mutex<HashMap<String, (Vec<RouteItem>, Duration)>>,
    /// The outcome to return from `invoke`.
    invoke_outcome: Mutex<ActionOutcome>,
    /// Records every `invoke` call for assertion.
    invocations: Arc<Mutex<Vec<ActionRequest>>>,
    /// Records every `cancel` call for assertion.
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
        // Store routes into a temporary that we swap in
        // Use try_lock since we just created the struct
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
    async fn route(&self, query: &str, _input_kind: InputKind) -> Vec<RouteItem> {
        // Clone data out of the lock before sleeping.
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

/// Fake `HostEventSink` that collects emitted events.
struct FakeSink {
    events: Arc<Mutex<Vec<HostToUi>>>,
}

impl FakeSink {
    fn new() -> (Self, Arc<Mutex<Vec<HostToUi>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        (Self { events: events.clone() }, events)
    }
}

impl HostEventSink for FakeSink {
    fn send(&self, event: HostToUi) {
        self.events.lock().unwrap().push(event);
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn make_route_item(id: &str, title: &str, score: f64) -> RouteItem {
    RouteItem {
        action_id: ActionId::new(id).unwrap(),
        title: title.into(),
        score,
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

/// Two queries are sent concurrently. The first (slow) query's results
/// must be discarded because the second (fast) query supersedes it.
#[tokio::test]
async fn newer_query_generation_wins_when_results_complete_out_of_order() {
    let gen1 = QueryGeneration::INITIAL;
    let gen2 = gen1.next();

    let mut routes = HashMap::new();
    routes.insert(
        "first".into(),
        (vec![make_route_item("id_a", "A", 1.0)], Duration::from_millis(200)),
    );
    routes.insert(
        "second".into(),
        (vec![make_route_item("id_b", "B", 1.0)], Duration::ZERO),
    );

    let runtime = Arc::new(FakeRuntime::with_routes(routes));
    let (sink, events) = FakeSink::new();
    let coord = Arc::new(ApplicationCoordinator::new(runtime, Arc::new(sink)));

    // Spawn the slow query first.
    let c1 = coord.clone();
    let h1 = tokio::spawn(async move {
        c1.handle_ui(UiToHost::QueryChanged {
            generation: gen1,
            text: "first".into(),
        })
        .await;
    });

    // Give gen1 time to set_current and start its slow route.
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Now spawn the fast query.
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

/// An `Invoke` from the UI must use `Caller::ui("search-panel")` and
/// the `InvocationCompleted` event must be forwarded to the sink.
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

    // Verify the runtime received the correct request.
    {
        let invocs = invocations.lock().unwrap();
        assert_eq!(invocs.len(), 1);
        assert_eq!(invocs[0].caller, Caller::ui("search-panel"));
        assert_eq!(invocs[0].invocation_id, inv_id);
        assert_eq!(invocs[0].action_id, action_id);
        assert_eq!(invocs[0].input.text, "hello");
    }

    // Verify the completion event was forwarded.
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

/// A `ShowSearchPanel` intent from Portal must emit the corresponding
/// `HostToUi` event **without** invoking the runtime.
#[test]
fn portal_show_search_emits_host_to_ui_without_invoking_runtime() {
    let runtime = Arc::new(FakeRuntime::new());
    let invocations = runtime.invocations.clone();
    let (sink, events) = FakeSink::new();
    let coord = ApplicationCoordinator::new(runtime, Arc::new(sink));

    coord.handle_intent(UiIntent::ShowSearchPanel {
        activation_token: Some("portal-token-42".into()),
    });

    // The event should be emitted exactly as provided.
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

    // The runtime must not have been touched.
    assert!(invocations.lock().unwrap().is_empty());
}

/// Two consecutive toggles must emit Show then Hide, and the runtime
/// must remain untouched.
#[test]
fn toggle_emits_show_then_hide_without_stopping_runtime() {
    let runtime = Arc::new(FakeRuntime::new());
    let invocations = runtime.invocations.clone();
    let (sink, events) = FakeSink::new();
    let coord = ApplicationCoordinator::new(runtime, Arc::new(sink));

    // First toggle: panel was hidden → show.
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

    // Second toggle: panel was shown → hide.
    coord.toggle_search_panel();
    {
        let emitted = events.lock().unwrap();
        assert_eq!(emitted.len(), 2, "two toggles should emit 2 events");
        match &emitted[1] {
            HostToUi::HideSearchPanel => {}
            other => panic!("expected HideSearchPanel, got {other:?}"),
        }
    }

    // Runtime untouched.
    assert!(invocations.lock().unwrap().is_empty());
}
