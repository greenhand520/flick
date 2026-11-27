//! 应用层的窄端口 trait。
//!
//! 这些 trait 将协调器与具体的 Runtime 和 UI 实现解耦。
//! 适配器（flick-runtime、flick-shell-gpui、flick-ipc）提供具体实现，
//! 测试则提供伪造实现。

use async_trait::async_trait;
use flick_domain::{
    ActionId, ActionOutcome, ActionRequest, DependencyStatus, ExecutionEventKind, HostToUi,
    InputContext, InputFacet, InputType, InvocationId, RouteItem, SurfaceContent, SurfaceEventKind,
    SurfaceSessionId,
};

/// 对 Flick Runtime 的路由与调用能力的抽象。
///
/// 协调器响应 UI 事件时调用这些方法。
/// 实现位于适配器 crate 中（例如 `flick-runtime`）。
#[async_trait]
pub trait RuntimePort: Send + Sync {
    /// 将用户查询路由到候选动作。
    async fn route(&self, query: &str, input_kind: InputType) -> Vec<RouteItem>;

    /// 按完整输入上下文路由；默认回退到旧的 `route`，便于测试替身逐步迁移。
    async fn route_input(&self, context: &InputContext) -> Vec<RouteItem> {
        let file_count = context
            .facets
            .iter()
            .filter(|facet| matches!(facet, InputFacet::File { .. }))
            .count();
        let input_kind = if context.has_facet(flick_domain::InputFacetKind::ClipboardImage) {
            InputType::ClipboardImage
        } else if file_count > 1 {
            InputType::Files
        } else if file_count == 1 {
            InputType::PathCandidate
        } else {
            InputType::PlainText
        };
        self.route(&context.raw_text, input_kind).await
    }

    /// 搜索阶段执行：调用所有 `search_invocable` Action 并把结果归一化为
    /// `RouteItemKind::Execution` 候选。
    ///
    /// 默认实现返回空列表，便于只实现路由能力的测试替身；生产适配器应转发
    /// Runtime 的 `search_invoke`。
    async fn search_invoke(&self, _query: &str, _input_kind: InputType) -> Vec<RouteItem> {
        Vec::new()
    }

    /// 把 Execution 项的激活事件交给父 Action 处理。
    ///
    /// 返回 `None` 表示父 Action 未实现该事件（no-op）。默认实现即 no-op。
    async fn execution_event(
        &self,
        _parent_action_id: ActionId,
        _item_id: &str,
        _event: ExecutionEventKind,
    ) -> Option<ActionOutcome> {
        None
    }

    /// 执行动作请求并返回结果。
    async fn invoke(&self, request: ActionRequest) -> ActionOutcome;

    /// 尝试取消正在进行的调用。
    ///
    /// 如果找到了该调用并已发出取消信号，则返回 `true`；
    /// 如果它已完成或未知，则返回 `false`。
    async fn cancel(&self, invocation_id: InvocationId) -> bool;

    /// 向一个 Surface 会话投递控件事件，返回更新后的内容。
    ///
    /// 错误以可展示的字符串返回（由实现把 `SurfaceError` 的 `message`
    /// 映射过来）。
    async fn surface_event(
        &self,
        session: SurfaceSessionId,
        kind: SurfaceEventKind,
    ) -> Result<SurfaceContent, String>;

    /// 关闭一个 Surface 会话（幂等）。
    async fn surface_closed(&self, session: SurfaceSessionId);

    /// 返回该插件声明的依赖解析结果。
    ///
    /// 默认返回空，便于测试替身逐步迁移；真实实现由
    /// `RuntimeAdapter` 转发给 `ActionRuntime`。
    async fn dependency_status(&self, _plugin_id: &str) -> Vec<DependencyStatus> {
        Vec::new()
    }

    /// 返回所有已注册插件 id（升序）。
    ///
    /// 默认返回空，供测试替身省略。
    async fn registered_plugin_ids(&self) -> Vec<String> {
        Vec::new()
    }
}

/// 协调器发出的 `HostToUi` 事件的接收器。
///
/// UI 外壳（Iced）或测试框架实现此 trait 以接收
/// 来自应用层的消息。
pub trait HostEventSink: Send + Sync {
    /// 向 UI 发送单个事件。
    fn send(&self, event: HostToUi);
}
