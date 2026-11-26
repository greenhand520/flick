//! `flick-builtins` —— 随 Flick 一起提供的内置动作。
//!
//! 内置插件：**echo**、**host** 与 **apps**（应用搜索）。

pub mod apps;
pub mod echo;
pub mod host;

use crate::apps::AppsPlugin;
pub use echo::{ECHO_ACTION_ID, EchoHandler, EchoPlugin};
use flick_runtime::{ActionRuntime, PluginRegistrationError};
pub use host::{HOST_ACTION_ID, HostPlugin};

/// 将所有内置动作注册到给定的运行时（echo + host + apps）。
///
/// 通过插件注册表注册 [`EchoPlugin`]、[`HostPlugin`] 与 [`AppsPlugin`]。`register_plugin`
/// 对重复动作 ID 返回 `IdCollision`（不做静默覆盖），因此每个运行时只应调用一次。
/// 注册完成后应调用 [`ActionRuntime::activate_all_plugins`] 统一激活生命周期。
pub async fn register_builtin(runtime: &ActionRuntime) -> Result<(), PluginRegistrationError> {
    runtime.register_plugin(&EchoPlugin).await?;
    runtime.register_plugin(&HostPlugin).await?;
    runtime.register_plugin(&AppsPlugin::default()).await?;
    Ok(())
}
