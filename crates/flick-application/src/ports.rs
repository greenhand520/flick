//! 应用层的窄端口 trait。
//!
//! 这些 trait 将协调器与具体的 Runtime 和 UI 实现解耦。
//! 适配器（flick-runtime、flick-shell-gpui、flick-ipc）提供具体实现，
//! 测试则提供伪造实现。

use async_trait::async_trait;
use flick_domain::{ActionOutcome, ActionRequest, HostToUi, InputType, InvocationId, RouteItem};

/// 对 Flick Runtime 的路由与调用能力的抽象。
///
/// 协调器响应 UI 事件时调用这些方法。
/// 实现位于适配器 crate 中（例如 `flick-runtime`）。
#[async_trait]
pub trait RuntimePort: Send + Sync {
    /// 将用户查询路由到候选动作。
    async fn route(&self, query: &str, input_kind: InputType) -> Vec<RouteItem>;

    /// 执行动作请求并返回结果。
    async fn invoke(&self, request: ActionRequest) -> ActionOutcome;

    /// 尝试取消正在进行的调用。
    ///
    /// 如果找到了该调用并已发出取消信号，则返回 `true`；
    /// 如果它已完成或未知，则返回 `false`。
    async fn cancel(&self, invocation_id: InvocationId) -> bool;
}

/// 协调器发出的 `HostToUi` 事件的接收器。
///
/// UI 外壳（Iced）或测试框架实现此 trait 以接收
/// 来自应用层的消息。
pub trait HostEventSink: Send + Sync {
    /// 向 UI 发送单个事件。
    fn send(&self, event: HostToUi);
}
