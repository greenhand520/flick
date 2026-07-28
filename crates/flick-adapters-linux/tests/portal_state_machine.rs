//! Portal state machine tests using fake adapters.

use flick_adapters_linux::portal::{IntentSink, PortalShortcutAdapter};
use flick_adapters_linux::session::ShortcutActivation;
use flick_adapters_linux::shortcut_state::ShortcutState;
use flick_domain::UiIntent;
use std::sync::{Arc, Mutex};

/// Fake intent sink that records received intents.
struct FakeIntentSink {
    intents: Mutex<Vec<UiIntent>>,
}

impl FakeIntentSink {
    fn new() -> Self {
        Self {
            intents: Mutex::new(Vec::new()),
        }
    }

    fn drain(&self) -> Vec<UiIntent> {
        self.intents.lock().unwrap().drain(..).collect()
    }
}

impl IntentSink for FakeIntentSink {
    fn send_intent(&self, intent: UiIntent) {
        self.intents.lock().unwrap().push(intent);
    }
}

#[tokio::test]
async fn starts_in_unavailable_state() {
    let sink = Arc::new(FakeIntentSink::new());
    let adapter = PortalShortcutAdapter::new(sink);
    assert_eq!(adapter.state().await, ShortcutState::Unavailable);
}

#[tokio::test]
async fn initialize_transitions_to_unconfigured() {
    let sink = Arc::new(FakeIntentSink::new());
    let adapter = PortalShortcutAdapter::new(sink);
    adapter.initialize().await.unwrap();
    assert_eq!(adapter.state().await, ShortcutState::Unconfigured);
}

#[tokio::test]
async fn configure_transitions_to_bound() {
    let sink = Arc::new(FakeIntentSink::new());
    let adapter = PortalShortcutAdapter::new(sink);
    adapter.initialize().await.unwrap();
    let binding = adapter.configure_search_shortcut().await.unwrap();
    assert_eq!(binding.id, "show-search");
    assert_eq!(adapter.state().await, ShortcutState::Bound);
}

#[tokio::test]
async fn user_cancelled_stays_cancelled() {
    let sink = Arc::new(FakeIntentSink::new());
    let adapter = PortalShortcutAdapter::new(sink);
    adapter.initialize().await.unwrap();
    adapter.on_user_cancelled().await;
    assert_eq!(adapter.state().await, ShortcutState::UserCancelled);

    // Verify timer tick doesn't change state
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    assert_eq!(adapter.state().await, ShortcutState::UserCancelled);
}

#[tokio::test]
async fn user_cancelled_prevents_rebind() {
    let sink = Arc::new(FakeIntentSink::new());
    let adapter = PortalShortcutAdapter::new(sink);
    adapter.initialize().await.unwrap();
    adapter.on_user_cancelled().await;
    let result = adapter.configure_search_shortcut().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn session_error_transitions_to_recovering() {
    let sink = Arc::new(FakeIntentSink::new());
    let adapter = PortalShortcutAdapter::new(sink);
    adapter.initialize().await.unwrap();
    adapter.configure_search_shortcut().await.unwrap();

    adapter.on_session_error("transport failure".into()).await;
    assert!(matches!(
        adapter.state().await,
        ShortcutState::Recovering { attempt: 0 }
    ));
}

#[tokio::test]
async fn recovery_does_not_call_bind_shortcuts() {
    let sink = Arc::new(FakeIntentSink::new());
    let adapter = PortalShortcutAdapter::new(sink);
    adapter.initialize().await.unwrap();
    adapter.on_session_error("test".into()).await;
    adapter.attempt_recovery().await.unwrap();

    // After recovery, state should be Unconfigured (not Bound)
    assert_eq!(adapter.state().await, ShortcutState::Unconfigured);
}

#[tokio::test]
async fn activated_forwards_show_search_panel_intent() {
    let sink = Arc::new(FakeIntentSink::new());
    let adapter = PortalShortcutAdapter::new(sink.clone());
    adapter.initialize().await.unwrap();

    adapter
        .on_activated(ShortcutActivation {
            shortcut_id: "show-search".into(),
            activation_token: Some("token123".into()),
        })
        .await;

    let intents = sink.drain();
    assert_eq!(intents.len(), 1);
    match &intents[0] {
        UiIntent::ShowSearchPanel { activation_token } => {
            assert_eq!(activation_token.as_deref(), Some("token123"));
        }
        _ => panic!("Expected ShowSearchPanel intent"),
    }
}

#[tokio::test]
async fn request_ended_is_distinct_from_user_cancelled() {
    let sink = Arc::new(FakeIntentSink::new());
    let adapter = PortalShortcutAdapter::new(sink);
    adapter.initialize().await.unwrap();
    adapter.on_request_ended("timeout".into()).await;
    assert_eq!(adapter.state().await, ShortcutState::RequestEnded);
    assert_ne!(adapter.state().await, ShortcutState::UserCancelled);
}

#[tokio::test]
async fn double_bind_in_same_session_is_rejected() {
    let sink = Arc::new(FakeIntentSink::new());
    let adapter = PortalShortcutAdapter::new(sink);
    adapter.initialize().await.unwrap();
    adapter.configure_search_shortcut().await.unwrap();

    let result = adapter.configure_search_shortcut().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn diagnostics_include_state_and_error() {
    let sink = Arc::new(FakeIntentSink::new());
    let adapter = PortalShortcutAdapter::new(sink);
    adapter.initialize().await.unwrap();
    adapter.on_request_ended("test error".into()).await;

    let diag = adapter.diagnostics().await;
    assert_eq!(diag.state, ShortcutState::RequestEnded);
    assert_eq!(diag.last_error.as_deref(), Some("test error"));
}
