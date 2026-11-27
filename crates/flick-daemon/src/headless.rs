//! 无头运行时：在没有 Iced 或 Portal 的情况下运行 Runtime + IPC。
//!
//! 用于 CI、本地开发和测试。不需要
//! Wayland 会话。

use flick_runtime::ActionRuntime;
use std::sync::Arc;

/// 用于测试的无头运行时句柄。
pub struct HeadlessRuntime {
    pub runtime: Arc<ActionRuntime>,
    /// 解析出的数据目录，供设置系统初始化复用。
    pub dirs: Option<flick_data::DataDirectories>,
}

impl HeadlessRuntime {
    /// 创建一个新的无头运行时，并注册内置动作。
    pub async fn new() -> Self {
        let dirs = flick_data::DataDirectories::discover().ok();
        let paths: Arc<dyn flick_domain::PluginPaths> = match &dirs {
            Some(dirs) => Arc::new(dirs.clone()),
            None => Arc::new(flick_domain::NoopPaths),
        };
        let runtime = Arc::new(ActionRuntime::new(
            Arc::new(flick_adapters_linux::host_info::collect_host_info()),
            paths,
        ));
        flick_builtins::register_builtin(&runtime)
            .await
            .expect("register builtin plugins");
        runtime.activate_all_plugins().await;
        Self { runtime, dirs }
    }
}
