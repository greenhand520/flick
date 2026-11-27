//! `flick-plugin` — 插件契约层与依赖解析。
//!
//! 本 crate 只包含「插件作者需要实现的 trait」与纯函数形式的
//! 静态校验 / 依赖解析；不依赖 tokio，也不持有任何注册表状态。

mod contract;
mod dependency;
mod manifest;

pub use contract::{
    ActionHandler, ActionSpec, Plugin, PluginLifecycle, PluginRegistrationError,
    PluginRuntimeError, PluginUiFactory, SettingsProvider, SurfaceError, SurfaceHandler,
    SurfaceSession,
};
pub use dependency::{DependencyResolution, resolve_dependencies};
pub use manifest::validate_manifest;

// 插件作者只需依赖 `flick-plugin`，即可拿到编写插件所需的全部领域类型。
// 这些类型仍定义在 `flick-domain`（跨层共享的数据词汇），此处仅做转出，
// 避免插件作者同时 import 两个 crate。
pub use flick_domain::{
    ActionDescriptor, ActionError, ActionId, ActionOutcome, ActionRequest, ActionVisibility,
    Architecture, Category, DependencyRequirement, ExecutionEventKind, ExecutionMode, InputType,
    OperatingSystem, PluginContext, PluginDependency, PluginKind, PluginManifest, PluginPaths,
    SettingDecl, SettingKind, SurfaceContent, SurfaceEventKind, SurfaceKind, SurfacePlacement,
    SurfaceSessionId, UiSurfaceDecl,
};
