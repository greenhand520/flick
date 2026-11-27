//! 集成测试：单次搜索的 action candidates 数量上限（设计文档 §9 约束 8）。

use async_trait::async_trait;
use flick_domain::*;
use flick_runtime::*;
use std::sync::Arc;

/// 不做任何事的处理器。
struct NoopHandler;

#[async_trait]
impl ActionHandler for NoopHandler {
    async fn invoke(
        &self,
        _request: ActionRequest,
        _: PluginContext,
    ) -> Result<ActionOutcome, ActionError> {
        Ok(ActionOutcome::Text("noop".into()))
    }
}

/// 提供 `count` 个命中同一关键词的公开 Action。
struct ManyActionsPlugin {
    /// Action 数量。
    count: usize,
}

impl Plugin for ManyActionsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "test.many".into(),
            name: "Many".into(),
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
        (0..self.count)
            .map(|i| {
                let mut d = ActionDescriptor::new(
                    ActionId::new(format!("test.many.v1.item{i}")).unwrap(),
                    format!("Item {i}"),
                    vec!["demo".into()],
                    ConcurrencyPolicy::Parallel,
                );
                d.search_invocable = false;
                ActionSpec {
                    descriptor: d,
                    handler: Arc::new(NoopHandler),
                }
            })
            .collect()
    }
}

#[tokio::test]
async fn search_input_truncates_action_candidates() {
    let runtime = ActionRuntime::default();
    runtime
        .register_plugin(&ManyActionsPlugin { count: 150 })
        .await
        .unwrap();

    let snapshot = SearchInputSnapshot::text("demo", InputSource::Manual);
    let context = InputContext::from_snapshot(QueryGeneration::INITIAL, &snapshot);
    let items = runtime.route_input(&context).await;

    assert_eq!(items.len(), MAX_ACTION_RESULTS);
}
