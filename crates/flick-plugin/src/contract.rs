//! 插件契约层：Plugin / ActionSpec / 生命周期 / 注册与运行错误。
//!
//! 这一层只描述「插件作者需要实现什么」，不含任何运行时状态，
//! 因此不依赖 tokio。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use flick_domain::settings::ConfigSnapshot;
use flick_domain::{
    ActionDescriptor, ActionError, ActionId, ActionOutcome, ActionRequest, ExecutionEventKind,
    PluginContext, PluginManifest, SurfaceContent, SurfaceEventKind, SurfaceSessionId,
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

/// 宿主向运行时提供"插件设置"的抽象。
///
/// 本 crate 是纯契约层、**不依赖 tokio**，因此订阅用"回调注册"而不是
/// watch channel：真正的 `watch::Receiver` 由 `flick-runtime` 在回调之上实现。
///
/// 实现者（`flick-daemon` 的 `SettingsAdapter`）必须：
/// - `snapshot` 返回该插件命名空间的当前值（已合并声明默认值，**不含 secret**）；
/// - `subscribe` 在该命名空间 revision 变化时，用新快照调用 `listener`。
pub trait SettingsProvider: Send + Sync {
    /// 解析某插件的当前设置快照。
    fn snapshot(&self, plugin_id: &str) -> ConfigSnapshot;

    /// 注册一个变更监听器（同一插件可有多个订阅者）。
    fn subscribe(&self, plugin_id: &str, listener: Arc<dyn Fn(ConfigSnapshot) + Send + Sync>);
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
    ///
    /// `ctx` 携带调用入口固定的设置快照（本插件命名空间）。
    async fn on_event(
        &self,
        session: &SurfaceSession,
        event: SurfaceEventKind,
        ctx: PluginContext,
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
    /// 激活插件；`ctx` 携带该插件命名空间在激活时刻的设置快照。
    async fn activate(&self, ctx: PluginContext) -> Result<(), PluginRuntimeError>;
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

#[cfg(test)]
mod settings_provider_tests {
    use super::SettingsProvider;
    use flick_domain::settings::ConfigSnapshot;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FixedProvider {
        listeners: AtomicUsize,
    }

    impl SettingsProvider for FixedProvider {
        fn snapshot(&self, plugin_id: &str) -> ConfigSnapshot {
            ConfigSnapshot {
                namespace: plugin_id.to_string(),
                revision: 3,
                ..Default::default()
            }
        }

        fn subscribe(
            &self,
            _plugin_id: &str,
            _listener: Arc<dyn Fn(ConfigSnapshot) + Send + Sync>,
        ) {
            self.listeners.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn provider_is_object_safe_and_reports_snapshot() {
        let provider: Arc<dyn SettingsProvider> = Arc::new(FixedProvider {
            listeners: AtomicUsize::new(0),
        });
        assert_eq!(provider.snapshot("flick.demo").revision(), 3);
        provider.subscribe("flick.demo", Arc::new(|_| {}));
    }
}
