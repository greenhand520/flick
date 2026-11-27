use async_trait::async_trait;
use flick_domain::*;
use flick_runtime::*;
use std::sync::Arc;

struct AppSearchHandler;

#[async_trait]
impl ActionHandler for AppSearchHandler {
    async fn invoke(
        &self,
        request: ActionRequest,
        _: ActionContext,
    ) -> Result<ActionOutcome, ActionError> {
        assert_eq!(request.source, ActionSource::SearchRoute);
        Ok(ActionOutcome::Data(serde_json::json!([
            {"id":"app:firefox","title":"Firefox","subtitle":"浏览器","icon":"firefox"}
        ])))
    }
}

struct AppSearchPlugin;

impl Plugin for AppSearchPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "test.apps".into(),
            name: "Apps".into(),
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
        let mut d = ActionDescriptor::new(
            ActionId::from_static("test.apps.search"),
            "应用",
            vec!["app".into()],
            ConcurrencyPolicy::Parallel,
        );
        d.search_invocable = true;
        d.supported_inputs = vec![InputType::PlainText];
        vec![ActionSpec {
            descriptor: d,
            handler: Arc::new(AppSearchHandler),
        }]
    }
}

#[tokio::test]
async fn search_invoke_expands_data_to_execution_route_items() {
    let runtime = ActionRuntime::new();
    runtime.register_plugin(&AppSearchPlugin).await.unwrap();
    let items = runtime
        .search_invoke(QueryGeneration::INITIAL, ActionInput::plain_text("fire"))
        .await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, RouteItemKind::Execution);
    assert_eq!(items[0].item_id, "app:firefox");
    assert_eq!(
        items[0].parent_action_id.as_ref().unwrap().as_str(),
        "test.apps.search"
    );
    assert_eq!(
        items[0].execution.as_ref().unwrap().output["icon"],
        "firefox"
    );
}

#[tokio::test]
async fn non_search_invocable_actions_are_not_executed() {
    let runtime = ActionRuntime::new();
    let mut d = ActionDescriptor::new(
        ActionId::from_static("test.plain.noop"),
        "Noop",
        vec![],
        ConcurrencyPolicy::Parallel,
    );
    d.search_invocable = false;
    runtime.register(d, Arc::new(AppSearchHandler)).await;
    assert!(
        runtime
            .search_invoke(QueryGeneration::INITIAL, ActionInput::plain_text("fire"))
            .await
            .is_empty()
    );
}

// ─── 搜索阶段副作用约束 ─────────────────────────────────────────────

use std::sync::atomic::{AtomicUsize, Ordering};

/// 搜索阶段返回 OpenSurface 的 handler（必须只读，不应触发会话）。
struct SurfaceSearchHandler;

#[async_trait]
impl ActionHandler for SurfaceSearchHandler {
    async fn invoke(
        &self,
        request: ActionRequest,
        _: ActionContext,
    ) -> Result<ActionOutcome, ActionError> {
        assert_eq!(request.source, ActionSource::SearchRoute);
        Ok(ActionOutcome::open_surface(
            "test.surface.view",
            SurfaceContent {
                schema_version: SURFACE_SCHEMA_VERSION,
                title: None,
                body: vec![],
            },
        ))
    }
}

/// 记录 `on_open` 调用次数的 Surface 处理器。
struct CountingSurfaceHandler {
    opened: Arc<AtomicUsize>,
}

#[async_trait]
impl SurfaceHandler for CountingSurfaceHandler {
    async fn on_event(
        &self,
        _session: &SurfaceSession,
        _event: SurfaceEventKind,
    ) -> Result<SurfaceContent, SurfaceError> {
        Ok(SurfaceContent {
            schema_version: SURFACE_SCHEMA_VERSION,
            title: None,
            body: vec![],
        })
    }

    async fn on_open(&self, _session: &SurfaceSession) {
        self.opened.fetch_add(1, Ordering::SeqCst);
    }
}

struct SurfacePlugin {
    opened: Arc<AtomicUsize>,
}

impl Plugin for SurfacePlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "test.surface".into(),
            name: "Surface".into(),
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
        let mut d = ActionDescriptor::new(
            ActionId::from_static("test.surface.open"),
            "打开",
            vec!["open".into()],
            ConcurrencyPolicy::Parallel,
        );
        d.search_invocable = true;
        vec![ActionSpec {
            descriptor: d,
            handler: Arc::new(SurfaceSearchHandler),
        }]
    }

    fn surface_handler(&self) -> Option<Arc<dyn SurfaceHandler>> {
        Some(Arc::new(CountingSurfaceHandler {
            opened: self.opened.clone(),
        }))
    }
}

#[tokio::test]
async fn search_route_open_surface_has_no_side_effects() {
    let opened = Arc::new(AtomicUsize::new(0));
    let runtime = ActionRuntime::new();
    runtime
        .register_plugin(&SurfacePlugin {
            opened: opened.clone(),
        })
        .await
        .unwrap();

    let items = runtime
        .search_invoke(QueryGeneration::INITIAL, ActionInput::plain_text("open"))
        .await;

    // OpenSurface 在搜索阶段被丢弃，不产生执行项。
    assert!(items.is_empty(), "search-stage OpenSurface must be dropped");
    // 不得触发 on_open（即不得创建 Surface 会话）。
    assert_eq!(
        opened.load(Ordering::SeqCst),
        0,
        "on_open must not fire for search-stage OpenSurface"
    );
}
