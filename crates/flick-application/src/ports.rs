//! Narrow port traits for the application layer.
//!
//! These traits decouple the coordinator from concrete Runtime and UI
//! implementations. Adapters (flick-runtime, flick-shell-iced, flick-ipc)
//! provide concrete implementations; tests provide fakes.

use async_trait::async_trait;
use flick_domain::{
    ActionOutcome, ActionRequest, HostToUi, InputKind, InvocationId, RouteItem,
};

/// Abstraction over the Flick Runtime's routing and invocation capabilities.
///
/// The coordinator calls these methods in response to UI events.
/// Implementations live in adapter crates (e.g. `flick-runtime`).
#[async_trait]
pub trait RuntimePort: Send + Sync {
    /// Route a user query to candidate actions.
    async fn route(&self, query: &str, input_kind: InputKind) -> Vec<RouteItem>;

    /// Execute an action request and return the outcome.
    async fn invoke(&self, request: ActionRequest) -> ActionOutcome;

    /// Attempt to cancel an in-flight invocation.
    ///
    /// Returns `true` if the invocation was found and cancellation was
    /// signalled, `false` if it was already completed or unknown.
    async fn cancel(&self, invocation_id: InvocationId) -> bool;
}

/// Sink for `HostToUi` events that the coordinator emits.
///
/// The UI shell (Iced) or a test harness implements this to receive
/// messages from the application layer.
pub trait HostEventSink: Send + Sync {
    /// Send a single event to the UI.
    fn send(&self, event: HostToUi);
}
