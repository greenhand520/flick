//! 插件契约层：Plugin / ActionSpec / 生命周期 / 注册与运行错误。
//!
//! 这一层只描述「插件作者需要实现什么」，不含任何运行时状态，
//! 因此不依赖 tokio。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use flick_domain::{
    PluginContext, ActionDescriptor, ActionError, ActionId, ActionOutcome, ActionRequest,
    ExecutionEventKind, PluginManifest, SurfaceContent, SurfaceEventKind, SurfaceSessionId,
};

/// 每个注册的 Action 所实现的 trait。
///
/// 实现者接收一个 [`ActionRequest`] 和一个 [`PluginContext`]
/// （其中携带一个 [`CancellationToken`]），并返回一个
/// [`ActionOutcome`]。
///
/// 运行时使用 `tokio::select!` 让处理器与请求截止时间
/// 和外部取消进行竞速，因此处理器应在长时间运行的工作中
/// 协作式地检查取消令牌。
#[async_trait]
pub trait ActionHandler: Send + Sync {
    /// 执行 `request` 所描述的动作。
    ///
    /// `ctx` 提供一个 [`CancellationToken`]，处理器应在长时间运行
    /// 的工作中轮询它，以便外部取消能够及时
    /// 得到响应。
    async fn invoke(
        &self,
        request: ActionRequest,
        ctx: PluginContext,
    ) -> Result<ActionOutcome, ActionError>;

    /// 用户对 Execution 项的操作；默认 no-op。
    async fn execution_event(
        &self,
        _parent_action_id: &ActionId,
        _item_id: &str,
        _event: ExecutionEventKind,
        _ctx: PluginContext,
    ) -> Result<Option<ActionOutcome>, ActionError> {
        Ok(None)
    }
}

/// 一个动作 = 描述符 + 执行体。
pub struct ActionSpec {
    pub descriptor: ActionDescriptor,
    pub handler: Arc<dyn ActionHandler>,
}

/// 插件可提供 UI 的工厂（Plan 2 填充；本步为占位 trait，保证签名稳定）。
pub trait PluginUiFactory: Send + Sync {}

/// Surface 交互处理器：接收控件事件并返回新的内容。
///
/// 与 [`ActionHandler`] 一样使用 `async_trait` 以保持对象安全。
#[async_trait]
pub trait SurfaceHandler: Send + Sync {
    /// 处理一次 Surface 交互事件，返回更新后的内容。
    async fn on_event(
        &self,
        session: &SurfaceSession,
        event: SurfaceEventKind,
    ) -> Result<SurfaceContent, SurfaceError>;

    /// Surface 会话打开时的可选钩子。
    async fn on_open(&self, _session: &SurfaceSession) {}

    /// Surface 会话关闭时的可选钩子。
    async fn on_close(&self, _session: &SurfaceSession) {}
}

/// 单个 Surface 交互会话的元数据。
#[derive(Debug, Clone)]
pub struct SurfaceSession {
    pub id: SurfaceSessionId,
    pub plugin_id: String,
    pub surface_id: String,
}

/// Surface 交互错误（可携带给 UI 展示的消息）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceError {
    pub message: String,
}

/// 可选的生命周期钩子；未提供时视为 Resident（静态在进程内）。
#[async_trait]
pub trait PluginLifecycle: Send + Sync {
    async fn activate(&self) -> Result<(), PluginRuntimeError>;
    async fn deactivate(&self) -> Result<(), PluginRuntimeError>;
    fn idle_timeout(&self) -> Option<Duration> {
        None
    }
}

/// 插件 = manifest（元数据）+ actions（动作集）。
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> PluginManifest;
    fn actions(&self) -> Vec<ActionSpec>;
    /// 返回插件生命周期句柄，供运行时在启动时统一激活。
    fn lifecycle(&self) -> Option<Arc<dyn PluginLifecycle>> {
        None
    }
    fn ui(&self) -> Option<&dyn PluginUiFactory> {
        None
    }
    /// 该插件的 Surface 交互处理器；默认不提供。
    fn surface_handler(&self) -> Option<Arc<dyn SurfaceHandler>> {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginRegistrationError {
    InvalidManifest {
        reason: String,
    },
    NoActions,
    IdCollision {
        action_id: String,
    },
    NamespaceViolation {
        action_id: String,
        plugin_id: String,
    },
    DuplicateSettingKey {
        key: String,
    },
    SurfaceIdCollision {
        surface_id: String,
    },
    DependencyMissing {
        plugin_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginRuntimeError {
    NotActivated,
    ActivationFailed { reason: String },
    DeactivationFailed { reason: String },
    ResourceUnavailable { resource: String },
    UnsupportedInput { input_type: String },
    DependencyUnavailable { plugin_id: String },
    HandlerPanicked,
    DeadlineExceeded,
    Cancelled,
}
