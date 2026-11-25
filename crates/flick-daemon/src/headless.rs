//! 无头运行时：在没有 Iced 或 Portal 的情况下运行 Runtime + IPC。
//!
//! 用于 CI、本地开发和测试。不需要
//! Wayland 会话。

use flick_runtime::ActionRuntime;
use std::sync::Arc;

/// 用于测试的无头运行时句柄。
pub struct HeadlessRuntime {
    pub runtime: Arc<ActionRuntime>,
}

impl HeadlessRuntime {
    /// 创建一个新的无头运行时，并注册内置动作。
    pub async fn new() -> Self {
        let runtime = Arc::new(ActionRuntime::new());
        flick_builtins::register_phase0_builtins(&runtime).await;
        runtime.set_host_info(Arc::new(flick_adapters_linux::host_info::collect_host_info()));
        Self { runtime }
    }
}
