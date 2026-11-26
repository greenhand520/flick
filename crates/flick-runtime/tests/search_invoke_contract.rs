use std::sync::Arc;
use async_trait::async_trait;
use flick_domain::*;
use flick_runtime::*;

struct AppSearchHandler;

#[async_trait]
impl ActionHandler for AppSearchHandler {
    async fn invoke(&self, request: ActionRequest, _: ActionContext) -> Result<ActionOutcome, ActionError> {
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
        let mut d = ActionDescriptor::new(ActionId::from_static("test.apps.search"), "应用", vec!["app".into()], ConcurrencyPolicy::Parallel);
        d.search_invocable = true;
        d.supported_inputs = vec![InputType::PlainText];
        vec![ActionSpec { descriptor: d, handler: Arc::new(AppSearchHandler) }]
    }
}

#[tokio::test]
async fn search_invoke_expands_data_to_execution_route_items() {
    let runtime = ActionRuntime::new();
    runtime.register_plugin(&AppSearchPlugin).await.unwrap();
    let items = runtime.search_invoke("fire", InputType::PlainText).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, RouteItemKind::Execution);
    assert_eq!(items[0].item_id, "app:firefox");
    assert_eq!(items[0].parent_action_id.as_ref().unwrap().as_str(), "test.apps.search");
    assert_eq!(items[0].execution.as_ref().unwrap().output["icon"], "firefox");
}

#[tokio::test]
async fn non_search_invocable_actions_are_not_executed() {
    let runtime = ActionRuntime::new();
    let mut d = ActionDescriptor::new(ActionId::from_static("test.plain.noop"), "Noop", vec![], ConcurrencyPolicy::Parallel);
    d.search_invocable = false;
    runtime.register(d, Arc::new(AppSearchHandler)).await;
    assert!(runtime.search_invoke("fire", InputType::PlainText).await.is_empty());
}
