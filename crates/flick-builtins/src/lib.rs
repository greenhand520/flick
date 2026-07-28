//! `flick-builtins` — Built-in actions shipped with Flick.
//!
//! Phase 0 contains a single action: **echo**.

pub mod echo;

use echo::{ECHO_ACTION_ID, EchoHandler};
use flick_domain::{ActionDescriptor, ActionId, ConcurrencyPolicy, Matcher};
use flick_runtime::ActionRuntime;
use std::sync::Arc;

/// Register all Phase 0 built-in actions into the given runtime.
///
/// This function is idempotent — calling it twice simply overwrites the
/// previously registered handlers for the same action IDs.
pub async fn register_phase0_builtins(runtime: &ActionRuntime) {
    let echo_descriptor = ActionDescriptor {
        id: ActionId::from_static(ECHO_ACTION_ID),
        title: "Echo".into(),
        keywords: vec!["echo".into()],
        matcher: Matcher::prefix_command("echo"),
        concurrency: ConcurrencyPolicy::Parallel,
    };

    runtime
        .register(echo_descriptor, Arc::new(EchoHandler))
        .await;
}
