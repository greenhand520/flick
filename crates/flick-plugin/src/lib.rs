//! `flick-plugin` — 插件契约层与依赖解析。
//!
//! 本 crate 只包含「插件作者需要实现的 trait」与纯函数形式的
//! 静态校验 / 依赖解析；不依赖 tokio，也不持有任何注册表状态。

mod contract;
mod dependency;
mod manifest;

pub use contract::{
    ActionHandler, ActionSpec, Plugin, PluginLifecycle, PluginRegistrationError, PluginRuntimeError,
    PluginUiFactory, SurfaceError, SurfaceHandler, SurfaceSession,
};
pub use dependency::{DependencyResolution, resolve_dependencies};
pub use manifest::validate_manifest;
