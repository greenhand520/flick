use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use flick_domain::*;
use flick_runtime::*;

/// 计数生命周期：记录 activate 被调用的次数。
struct CountingLifecycle(Arc<AtomicUsize>);

#[async_trait]
impl PluginLifecycle for CountingLifecycle {
    async fn activate(&self) -> Result<(), PluginRuntimeError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn deactivate(&self) -> Result<(), PluginRuntimeError> {
        Ok(())
    }
}

/// 带生命周期的测试插件。
struct CountingPlugin {
    lifecycle: Arc<CountingLifecycle>,
}

impl Plugin for CountingPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "test.counting".into(),
            name: "Counting".into(),
            author: "test".into(),
            version: "1.0.0".into(),
            kind: PluginKind::BuiltInRust,
            keywords: vec![],
            icon: None,
            profile: PluginProfile::default(),
            permissions: vec![],
            plugin_dependencies: vec![],
            settings: vec![],
            surfaces: vec![],
            supported_os: vec![],
            supported_architectures: vec![],
            needs_search: false,
        }
    }
    fn actions(&self) -> Vec<ActionSpec> {
        vec![ActionSpec {
            descriptor: ActionDescriptor::new(
                ActionId::from_static("test.counting.noop"),
                "Noop",
                vec![],
                ConcurrencyPolicy::Parallel,
            ),
            handler: Arc::new(NoopHandler),
        }]
    }
    fn lifecycle(&self) -> Option<Arc<dyn PluginLifecycle>> {
        Some(self.lifecycle.clone())
    }
}

/// 空操作处理器。
struct NoopHandler;

#[async_trait]
impl ActionHandler for NoopHandler {
    async fn invoke(
        &self,
        _: ActionRequest,
        _: ActionContext,
    ) -> Result<ActionOutcome, ActionError> {
        Ok(ActionOutcome::Notice {
            message: "ok".into(),
        })
    }
}

#[tokio::test]
async fn activate_all_plugins_runs_registered_lifecycles() {
    let counter = Arc::new(AtomicUsize::new(0));
    let runtime = ActionRuntime::new();
    runtime
        .register_plugin(&CountingPlugin {
            lifecycle: Arc::new(CountingLifecycle(counter.clone())),
        })
        .await
        .unwrap();
    runtime.activate_all_plugins().await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}
