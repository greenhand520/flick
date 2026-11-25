//! `flick-builtins` —— 随 Flick 一起提供的内置动作。
//!
//! Phase 0 仅包含一个动作：**echo**。

pub mod echo;

use echo::{ECHO_ACTION_ID, EchoHandler};
use flick_domain::{ActionDescriptor, ActionId, ConcurrencyPolicy, Matcher};
use flick_runtime::ActionRuntime;
use std::sync::Arc;

/// 将所有 Phase 0 内置动作注册到给定的运行时。
///
/// 此函数是幂等的 —— 调用两次只会覆盖先前针对相同动作 ID
/// 注册的处理器。
pub async fn register_phase0_builtins(runtime: &ActionRuntime) {
    let echo_descriptor = ActionDescriptor::new(
        ActionId::from_static(ECHO_ACTION_ID),
        "Echo",
        vec!["echo".into()],
        Matcher::prefix_command("echo"),
        ConcurrencyPolicy::Parallel,
    );

    runtime
        .register(echo_descriptor, Arc::new(EchoHandler))
        .await;
}
