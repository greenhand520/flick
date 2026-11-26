use flick_runtime::{
    ActionHandler, ActionRegistry, ActionSpec, Plugin, PluginLifecycle, PluginRegistrationError,
    PluginRuntimeError, PluginUiFactory,
};

// 仅用于编译期检查 trait 存在与对象安全。
fn _assert_plugin(_p: &dyn Plugin) {}
fn _assert_lifecycle(_l: &dyn PluginLifecycle) {}
fn _assert_ui(_u: &dyn PluginUiFactory) {}

#[test]
fn plugin_traits_exist() {
    // 一个最小实现能被当作 dyn Plugin（编译即可）
    struct Dummy;
    impl Plugin for Dummy {
        fn manifest(&self) -> flick_domain::PluginManifest { unimplemented!() }
        fn actions(&self) -> Vec<ActionSpec> { Vec::new() }
    }
    let d = Dummy;
    _assert_plugin(&d);
    assert!(matches!(PluginRuntimeError::Cancelled, PluginRuntimeError::Cancelled));
}

use std::sync::Arc;
use async_trait::async_trait;
use flick_domain::*;

struct NoopHandler;
#[async_trait]
impl ActionHandler for NoopHandler {
    async fn invoke(&self, _r: ActionRequest, _c: ActionContext) -> Result<ActionOutcome, ActionError> {
        Ok(ActionOutcome::Data(serde_json::json!({"ok": true})))
    }
}

fn manifest(id: &str) -> PluginManifest {
    PluginManifest {
        id: id.into(), name: "P".into(), author: "Flick".into(), version: "1.0.0".into(),
        kind: PluginKind::BuiltInRust, keywords: vec![], icon: None, profile: PluginProfile::default(),
        permissions: vec![], plugin_dependencies: vec![], settings: vec![], surfaces: vec![],
        supported_os: vec![], supported_architectures: vec![],
        needs_search: false,
    }
}

fn spec(id: &str) -> ActionSpec {
    ActionSpec {
        descriptor: ActionDescriptor::new(
            ActionId::new(id).unwrap(), "T", vec!["x".into()],
            ConcurrencyPolicy::Parallel,
        ),
        handler: Arc::new(NoopHandler),
    }
}

struct P { m: PluginManifest, a: Vec<ActionSpec> }
impl Plugin for P {
    fn manifest(&self) -> PluginManifest { self.m.clone() }
    fn actions(&self) -> Vec<ActionSpec> {
        self.a.iter().map(|s| ActionSpec {
            descriptor: s.descriptor.clone(), handler: s.handler.clone(),
        }).collect()
    }
}

#[test]
fn register_plugin_success() {
    let mut reg = ActionRegistry::default();
    let p = P { m: manifest("demo.plugin"), a: vec![spec("demo.plugin.run")] };
    reg.register_plugin(&p).unwrap();
    assert!(reg.find_by_id(&ActionId::from_static("demo.plugin.run")).is_some());
}

#[test]
fn register_plugin_rejects_empty_author() {
    let mut reg = ActionRegistry::new();
    let mut m = manifest("demo.plugin");
    m.author = "   ".into();
    let p = P { m, a: vec![spec("demo.plugin.run")] };
    match reg.register_plugin(&p).unwrap_err() {
        PluginRegistrationError::InvalidManifest { reason } => assert!(reason.contains("author")),
        e => panic!("expected invalid manifest, got {e:?}"),
    }
}

#[test]
fn register_plugin_rejects_no_actions() {
    let mut reg = ActionRegistry::default();
    let p = P { m: manifest("demo.plugin"), a: vec![] };
    assert_eq!(reg.register_plugin(&p).unwrap_err(), PluginRegistrationError::NoActions);
}

#[test]
fn register_plugin_rejects_namespace_violation() {
    let mut reg = ActionRegistry::default();
    let p = P { m: manifest("demo.plugin"), a: vec![spec("other.thing.run")] };
    match reg.register_plugin(&p).unwrap_err() {
        PluginRegistrationError::NamespaceViolation { .. } => {}
        e => panic!("expected namespace violation, got {e:?}"),
    }
}

#[test]
fn register_plugin_rejects_id_collision() {
    let mut reg = ActionRegistry::default();
    let a = P { m: manifest("demo.plugin"), a: vec![spec("demo.plugin.run")] };
    reg.register_plugin(&a).unwrap();
    let b = P { m: manifest("demo.plugin"), a: vec![spec("demo.plugin.run")] };
    match reg.register_plugin(&b).unwrap_err() {
        PluginRegistrationError::IdCollision { .. } => {}
        e => panic!("expected id collision, got {e:?}"),
    }
}

#[test]
fn register_plugin_rejects_duplicate_setting_key() {
    let mut reg = ActionRegistry::default();
    let mut m = manifest("demo.plugin");
    m.settings = vec![
        SettingDecl { key: "k".into(), label: "".into(), description: "".into(), kind: SettingKind::Bool, default: None, secret: false },
        SettingDecl { key: "k".into(), label: "".into(), description: "".into(), kind: SettingKind::Bool, default: None, secret: false },
    ];
    let p = P { m, a: vec![spec("demo.plugin.run")] };
    match reg.register_plugin(&p).unwrap_err() {
        PluginRegistrationError::DuplicateSettingKey { key } => assert_eq!(key, "k"),
        e => panic!("expected duplicate setting key, got {e:?}"),
    }
}

#[test]
fn register_plugin_rejects_duplicate_surface_id() {
    let mut reg = ActionRegistry::default();
    let mut m = manifest("demo.plugin");
    let surface = UiSurfaceDecl {
        id: "info".into(),
        title: "Info".into(),
        kind: SurfaceKind::Panel,
        default_placement: SurfacePlacement::BelowSearchBox,
    };
    m.surfaces = vec![surface.clone(), surface];
    let p = P { m, a: vec![spec("demo.plugin.run")] };
    match reg.register_plugin(&p).unwrap_err() {
        PluginRegistrationError::SurfaceIdCollision { surface_id } => assert_eq!(surface_id, "info"),
        e => panic!("expected surface id collision, got {e:?}"),
    }
}

#[tokio::test]
async fn runtime_register_plugin_routes_and_invokes() {
    let rt = flick_runtime::ActionRuntime::default();
    let p = P { m: manifest("demo.plugin"), a: vec![spec("demo.plugin.run")] };
    rt.register_plugin(&p).await.unwrap();
    // 该测试插件以关键词 "x" 召回
    let routes = rt.route("x", InputType::PlainText).await;
    assert_eq!(routes.len(), 1);
    // 用真实 id 构造请求（new_echo 返回 Self，非 async）
    let mut req = ActionRequest::new_echo("x", Caller::cli("test"));
    req.action_id = ActionId::from_static("demo.plugin.run");
    let outcome = rt.invoke(req).await;
    assert!(matches!(outcome, ActionOutcome::Data(_)));
}
