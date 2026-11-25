//! `flick-builtins` —— 随 Flick 一起提供的内置动作。
//!
//! Phase 0 仅包含一个动作：**echo**。

pub mod echo;
pub mod host;

pub use echo::{EchoHandler, EchoPlugin, ECHO_ACTION_ID};
use flick_runtime::ActionRuntime;

/// 将所有 Phase 0 内置动作注册到给定的运行时。
///
/// 通过插件注册表注册 [`EchoPlugin`]。`register_plugin` 对重复动作 ID 返回
/// `IdCollision`（不做静默覆盖），因此每个运行时只应调用一次。
pub async fn register_phase0_builtins(runtime: &ActionRuntime) {
    runtime
        .register_plugin(&EchoPlugin)
        .await
        .expect("register echo plugin");
}
