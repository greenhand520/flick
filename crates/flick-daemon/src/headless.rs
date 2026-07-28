//! Headless runtime: runs Runtime + IPC without Iced or Portal.
//!
//! Used for CI, local development, and testing. Does not require
//! a Wayland session.

use flick_runtime::ActionRuntime;
use std::sync::Arc;

/// A handle to the headless runtime for testing.
pub struct HeadlessRuntime {
    pub runtime: Arc<ActionRuntime>,
}

impl HeadlessRuntime {
    /// Create a new headless runtime with builtins registered.
    pub async fn new() -> Self {
        let runtime = Arc::new(ActionRuntime::new());
        flick_builtins::register_phase0_builtins(&runtime).await;
        Self { runtime }
    }
}
