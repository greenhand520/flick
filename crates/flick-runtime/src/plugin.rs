//! 插件契约层：Plugin / ActionSpec / 生命周期 / 注册与运行错误。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use flick_domain::{
    ActionDescriptor, PluginManifest, SurfaceContent, SurfaceEventKind, SurfaceSessionId,
};

use crate::handler::ActionHandler;

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
    fn lifecycle(&self) -> Option<&dyn PluginLifecycle> {
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
    InvalidManifest { reason: String },
    NoActions,
    IdCollision { action_id: String },
    NamespaceViolation { action_id: String, plugin_id: String },
    DuplicateSettingKey { key: String },
    SurfaceIdCollision { surface_id: String },
    DependencyMissing { plugin_id: String },
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
