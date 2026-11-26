//! `flick-builtins` —— 随 Flick 一起提供的内置动作。
//!
//! Phase 0 仅包含一个动作：**echo**。

pub mod apps;
pub mod echo;
pub mod host;

pub use echo::{EchoHandler, EchoPlugin, ECHO_ACTION_ID};
pub use host::{HostPlugin, HOST_ACTION_ID};
use flick_runtime::ActionRuntime;

/// 将所有内置动作注册到给定的运行时（echo + host）。
///
/// 通过插件注册表注册 [`EchoPlugin`] 与 [`HostPlugin`]。`register_plugin` 对重复
/// 动作 ID 返回 `IdCollision`（不做静默覆盖），因此每个运行时只应调用一次。
/// 注册完成后应调用 [`ActionRuntime::activate_all_plugins`] 统一激活生命周期。
pub async fn register_builtin(runtime: &ActionRuntime) {
    runtime
        .register_plugin(&EchoPlugin)
        .await
        .expect("register echo plugin");
    runtime
        .register_plugin(&HostPlugin)
        .await
        .expect("register host plugin");
}
