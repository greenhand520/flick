//! ActionRuntime — orchestrates routing, invocation, and lifecycle.

use crate::handler::ActionHandler;
use crate::registry::ActionRegistry;
use flick_domain::{
    ActionContext, ActionDescriptor, ActionError, ActionOutcome, ActionRequest, InputKind,
    InvocationId, RouteItem,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

/// Entry in the active-invocations map.
///
/// Only the [`CancellationToken`] is stored so that external callers
/// can request cancellation.  The [`tokio::task::JoinHandle`] stays
/// local to `invoke()` where it is awaited inside `tokio::select!`.
struct ActiveEntry {
    cancel_token: CancellationToken,
}

/// The Flick runtime.
///
/// Holds the [`ActionRegistry`] behind `Arc<RwLock<_>>` so it can be
/// shared across tasks and permit dynamic re-registration.
///
/// Active invocations are tracked in a `tokio::sync::Mutex<HashMap>`
/// so they can be cancelled from any task.
pub struct ActionRuntime {
    registry: Arc<RwLock<ActionRegistry>>,
    active: Arc<tokio::sync::Mutex<HashMap<InvocationId, ActiveEntry>>>,
}

impl ActionRuntime {
    /// Create a new runtime with an empty registry.
    pub fn new() -> Self {
        Self {
            registry: Arc::new(RwLock::new(ActionRegistry::new())),
            active: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Register a new action (delegates to the inner registry).
    ///
    /// Acquires a write lock on the registry but does **not** hold it
    /// across any `.await`.
    pub async fn register(&self, descriptor: ActionDescriptor, handler: Arc<dyn ActionHandler>) {
        let mut reg = self.registry.write().await;
        reg.register(descriptor, handler);
    }

    /// Return matching [`RouteItem`]s for a search query.
    ///
    /// The registry lock is released before returning.
    pub async fn route(&self, query: &str, input_kind: InputKind) -> Vec<RouteItem> {
        let reg = self.registry.read().await;
        reg.search(query, input_kind)
    }

    /// Invoke an action.
    ///
    /// 1. Looks up the handler in the registry (read lock, released
    ///    immediately).
    /// 2. Spawns the handler with a fresh [`CancellationToken`].
    /// 3. Races handler completion against the request deadline and
    ///    external cancellation.
    /// 4. Cleans up the active entry when done.
    pub async fn invoke(&self, request: ActionRequest) -> ActionOutcome {
        let invocation_id = request.invocation_id;
        let deadline = request.deadline.timeout;

        // ── 1. Look up handler (hold read lock only briefly) ────────
        let handler: Arc<dyn ActionHandler> = {
            let reg = self.registry.read().await;
            match reg.find_by_id(&request.action_id) {
                Some((_desc, h)) => Arc::clone(h),
                None => {
                    return ActionOutcome::Error(ActionError::UnknownAction {
                        action_id: request.action_id.to_string(),
                    });
                }
            }
        };
        // registry read-lock dropped here

        // ── 2. Prepare cancellation token & spawn ───────────────────
        let token = CancellationToken::new();
        let ctx = ActionContext {
            cancellation: token.clone(),
        };

        // Store in active map so `cancel()` can find it.
        {
            let mut active = self.active.lock().await;
            active.insert(
                invocation_id,
                ActiveEntry {
                    cancel_token: token.clone(),
                },
            );
        }

        // Spawn the handler task.
        let req_for_task = request.clone();
        let join_handle = tokio::spawn(async move { handler.invoke(req_for_task, ctx).await });

        // ── 3. Race: handler completion vs deadline vs external cancel
        let outcome = tokio::select! {
            // Branch A — handler finishes naturally
            result = join_handle => {
                match result {
                    Ok(inner) => inner.unwrap_or_else(ActionOutcome::Error),
                    Err(join_err) => {
                        tracing::error!(invocation_id = %invocation_id, "handler panicked: {join_err}");
                        ActionOutcome::Error(ActionError::Internal {
                            message: "handler task panicked".into(),
                        })
                    }
                }
            }
            // Branch B — deadline exceeded
            _ = tokio::time::sleep(deadline) => {
                token.cancel();
                ActionOutcome::Error(ActionError::DeadlineExceeded)
            }
            // Branch C — external cancellation (someone called `cancel()`)
            _ = token.cancelled() => {
                ActionOutcome::Error(ActionError::Cancelled)
            }
        };

        // ── 4. Clean up ────────────────────────────────────────────
        {
            let mut active = self.active.lock().await;
            active.remove(&invocation_id);
        }

        outcome
    }

    /// Cancel an in-flight invocation.
    ///
    /// Returns `true` if the invocation was found and its cancellation
    /// token was newly signalled (i.e. it was not already cancelled).
    pub async fn cancel(&self, invocation_id: InvocationId) -> bool {
        let active = self.active.lock().await;
        match active.get(&invocation_id) {
            Some(entry) => {
                if entry.cancel_token.is_cancelled() {
                    false
                } else {
                    entry.cancel_token.cancel();
                    true
                }
            }
            None => false,
        }
    }
}

impl Default for ActionRuntime {
    fn default() -> Self {
        Self::new()
    }
}
