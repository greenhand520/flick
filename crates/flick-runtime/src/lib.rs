//! `flick-runtime` — 动作路由、调用和生命周期管理。
//!
//! 此 crate 为 Flick 的动作系统提供运行时层。
//! 它与框架无关：不依赖于 Iced、Portal、
//! D-Bus 或 Unix Socket。
//!
//! # 关键类型
//!
//! * [`ActionHandler`] — 每个注册的 Action 所实现的 trait。
//! * [`ActionRegistry`] — 存储描述符 + 处理器；支持
//!   关键字和前缀命令搜索。
//! * [`InvocationHandle`] — 指向正在运行的调用的句柄，支持
//!   协作式取消。
//! * [`ActionRuntime`] — 顶层编排器：路由查询、
//!   调用动作、管理截止时间和取消。

pub mod invocation;
pub mod registry;
pub mod runtime;
mod search;

// 在 crate 根重新导出主要的公共 API。
pub use invocation::InvocationHandle;
// 插件契约已抽到 `flick-plugin`；此处 re-export 保持下游
// （`flick-builtins` / `flick-testkit`）import 路径不变。
pub use flick_plugin::{
    ActionHandler, ActionSpec, DependencyResolution, Plugin, PluginLifecycle,
    PluginRegistrationError, PluginRuntimeError, PluginUiFactory, SurfaceError, SurfaceHandler,
    SurfaceSession, validate_manifest,
};
pub use registry::ActionRegistry;
pub use runtime::ActionRuntime;
