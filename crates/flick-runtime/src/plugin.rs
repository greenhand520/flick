//! 插件契约层：Plugin / ActionSpec / 生命周期 / 注册与运行错误。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use flick_domain::{ActionDescriptor, PluginManifest};

use crate::handler::ActionHandler;

/// 一个动作 = 描述符 + 执行体。
pub struct ActionSpec {
    pub descriptor: ActionDescriptor,
    pub handler: Arc<dyn ActionHandler>,
}

/// 插件可提供 UI 的工厂（Plan 2 填充；本步为占位 trait，保证签名稳定）。
pub trait PluginUiFactory: Send + Sync {}

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
