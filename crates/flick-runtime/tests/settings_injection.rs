//! S1-A：设置快照注入到插件执行上下文的运行时测试。

use async_trait::async_trait;
use flick_domain::settings::{ConfigSnapshot, SettingSecretReader, SettingValue};
use flick_domain::*;
use flick_plugin::SettingsProvider;
use flick_runtime::{
    ActionHandler, ActionRuntime, ActionSpec, Plugin, PluginLifecycle, PluginRuntimeError,
    SurfaceError, SurfaceHandler, SurfaceSession,
};
use std::sync::{Arc, Mutex};

/// 记录每次调用看到的上下文信息。
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    revision: u64,
    plugin_id: String,
    enabled: Option<bool>,
    secret: Option<String>,
}

fn snapshot() -> ConfigSnapshot {
    let mut values = std::collections::BTreeMap::new();
    values.insert("enabled".to_string(), SettingValue::Bool(true));
    ConfigSnapshot {
        namespace: "demo.settings".to_string(),
        revision: 9,
        values,
    }
}

struct Recorder {
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Recorder {
    fn push(&self, ctx: &PluginContext) {
        self.seen.lock().unwrap().push(Seen {
            revision: ctx.settings().revision(),
            plugin_id: ctx.plugin_id().to_string(),
            enabled: ctx.settings().bool("enabled"),
            secret: ctx.secret("api_token"),
        });
    }
}

#[async_trait]
impl ActionHandler for Recorder {
    async fn invoke(
        &self,
        _r: ActionRequest,
        c: PluginContext,
    ) -> Result<ActionOutcome, ActionError> {
        self.push(&c);
        Ok(ActionOutcome::Text("ok".into()))
    }

    async fn execution_event(
        &self,
        _parent: &ActionId,
        _item: &str,
        _event: ExecutionEventKind,
        c: PluginContext,
    ) -> Result<Option<ActionOutcome>, ActionError> {
        self.push(&c);
        Ok(Some(ActionOutcome::Text("event".into())))
    }
}

struct DemoSurface {
    seen: Arc<Mutex<Vec<Seen>>>,
}

#[async_trait]
impl SurfaceHandler for DemoSurface {
    async fn on_event(
        &self,
        _session: &SurfaceSession,
        _event: SurfaceEventKind,
        c: PluginContext,
    ) -> Result<SurfaceContent, SurfaceError> {
        self.seen.lock().unwrap().push(Seen {
            revision: c.settings().revision(),
            plugin_id: c.plugin_id().to_string(),
            enabled: c.settings().bool("enabled"),
            secret: c.secret("api_token"),
        });
        Ok(SurfaceContent {
            schema_version: SURFACE_SCHEMA_VERSION,
            title: Some("ok".into()),
            body: vec![],
        })
    }
}

struct DemoLifecycle {
    seen: Arc<Mutex<Vec<Seen>>>,
}

#[async_trait]
impl PluginLifecycle for DemoLifecycle {
    async fn activate(&self, c: PluginContext) -> Result<(), PluginRuntimeError> {
        self.seen.lock().unwrap().push(Seen {
            revision: c.settings().revision(),
            plugin_id: c.plugin_id().to_string(),
            enabled: c.settings().bool("enabled"),
            secret: c.secret("api_token"),
        });
        Ok(())
    }

    async fn deactivate(&self) -> Result<(), PluginRuntimeError> {
        Ok(())
    }
}

struct OpenSurfaceHandler {
    seen: Arc<Mutex<Vec<Seen>>>,
}

#[async_trait]
impl ActionHandler for OpenSurfaceHandler {
    async fn invoke(
        &self,
        _r: ActionRequest,
        c: PluginContext,
    ) -> Result<ActionOutcome, ActionError> {
        self.seen.lock().unwrap().push(Seen {
            revision: c.settings().revision(),
            plugin_id: c.plugin_id().to_string(),
            enabled: c.settings().bool("enabled"),
            secret: c.secret("api_token"),
        });
        let content = SurfaceContent {
            schema_version: SURFACE_SCHEMA_VERSION,
            title: Some("demo".into()),
            body: vec![],
        };
        Ok(ActionOutcome::open_surface("info", content))
    }
}

struct DemoPlugin {
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Plugin for DemoPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "demo.settings".into(),
            name: "Settings Demo".into(),
            author: "Flick".into(),
            version: "1.0.0".into(),
            kind: PluginKind::BuiltInRust,
            keywords: vec![],
            icon: None,
            profile: PluginProfile::default(),
            permissions: vec![],
            plugin_dependencies: vec![],
            settings: vec![],
            surfaces: vec![UiSurfaceDecl {
                id: "info".into(),
                title: "Info".into(),
                kind: SurfaceKind::Panel,
                default_placement: SurfacePlacement::BelowSearchBox,
            }],
            supported_os: vec![],
            supported_architectures: vec![],
            needs_search: false,
        }
    }

    fn actions(&self) -> Vec<ActionSpec> {
        vec![
            ActionSpec {
                descriptor: ActionDescriptor::new(
                    ActionId::new("demo.settings.act").unwrap(),
                    "Act",
                    vec![],
                    ConcurrencyPolicy::Parallel,
                ),
                handler: Arc::new(Recorder {
                    seen: Arc::clone(&self.seen),
                }),
            },
            ActionSpec {
                descriptor: ActionDescriptor::new(
                    ActionId::new("demo.settings.open").unwrap(),
                    "Open",
                    vec![],
                    ConcurrencyPolicy::Parallel,
                ),
                handler: Arc::new(OpenSurfaceHandler {
                    seen: Arc::clone(&self.seen),
                }),
            },
        ]
    }

    fn lifecycle(&self) -> Option<Arc<dyn PluginLifecycle>> {
        Some(Arc::new(DemoLifecycle {
            seen: Arc::clone(&self.seen),
        }))
    }

    fn surface_handler(&self) -> Option<Arc<dyn SurfaceHandler>> {
        Some(Arc::new(DemoSurface {
            seen: Arc::clone(&self.seen),
        }))
    }
}

#[derive(Default)]
struct FakeProvider {
    listeners: Mutex<Vec<(String, Arc<dyn Fn(ConfigSnapshot) + Send + Sync>)>>,
}

impl SettingsProvider for FakeProvider {
    fn snapshot(&self, plugin_id: &str) -> ConfigSnapshot {
        if plugin_id == "demo.settings" {
            snapshot()
        } else {
            ConfigSnapshot::empty()
        }
    }

    fn subscribe(&self, plugin_id: &str, listener: Arc<dyn Fn(ConfigSnapshot) + Send + Sync>) {
        self.listeners
            .lock()
            .unwrap()
            .push((plugin_id.to_string(), listener));
    }
}

impl FakeProvider {
    fn push(&self, plugin_id: &str, snapshot: ConfigSnapshot) {
        let listeners = self.listeners.lock().unwrap();
        for (id, listener) in listeners.iter() {
            if id == plugin_id {
                listener(snapshot.clone());
            }
        }
    }
}

struct FakeSecrets;

impl SettingSecretReader for FakeSecrets {
    fn secret(&self, plugin_id: &str, key: &str) -> Option<String> {
        (plugin_id == "demo.settings" && key == "api_token").then(|| "s3cr3t".to_string())
    }
}

fn request(action: &'static str) -> ActionRequest {
    let mut req = ActionRequest::new_echo("x", Caller::cli("test"));
    req.action_id = ActionId::from_static(action);
    req
}

async fn setup(seen: Arc<Mutex<Vec<Seen>>>) -> ActionRuntime {
    let rt = ActionRuntime::default();
    rt.register_plugin(&DemoPlugin { seen }).await.unwrap();
    rt
}

#[tokio::test]
async fn invoke_sees_fixed_snapshot_plugin_id_and_secret() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let rt = setup(Arc::clone(&seen)).await;
    rt.set_settings_provider(Arc::new(FakeProvider::default()));
    rt.set_settings_secrets(Arc::new(FakeSecrets));

    let outcome = rt.invoke(request("demo.settings.act")).await;
    assert!(matches!(outcome, ActionOutcome::Text(_)));
    let seen = seen.lock().unwrap();
    assert_eq!(
        seen[0],
        Seen {
            revision: 9,
            plugin_id: "demo.settings".into(),
            enabled: Some(true),
            secret: Some("s3cr3t".into()),
        }
    );
}

#[tokio::test]
async fn invoke_without_provider_gets_empty_snapshot() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let rt = setup(Arc::clone(&seen)).await;

    rt.invoke(request("demo.settings.act")).await;
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].revision, 0);
    assert_eq!(seen[0].plugin_id.as_str(), "demo.settings");
    assert_eq!(seen[0].enabled, None);
    assert_eq!(seen[0].secret, None);
}

#[tokio::test]
async fn lifecycle_activate_and_surface_event_see_snapshot() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let rt = setup(Arc::clone(&seen)).await;
    rt.set_settings_provider(Arc::new(FakeProvider::default()));
    rt.activate_all_plugins().await;
    assert_eq!(seen.lock().unwrap()[0].revision, 9);

    // 打开 Surface 后转发一次事件。
    let outcome = rt.invoke(request("demo.settings.open")).await;
    let ActionOutcome::OpenSurface { session_id, .. } = outcome else {
        panic!("expected open surface, got {outcome:?}");
    };
    let session_id = session_id.expect("runtime 应回填 session id");
    rt.surface_event(
        session_id,
        SurfaceEventKind::Click {
            node_id: "node".into(),
        },
    )
    .await
    .unwrap();
    let seen = seen.lock().unwrap();
    let last = seen.last().unwrap();
    assert_eq!(last.revision, 9);
    assert_eq!(last.plugin_id.as_str(), "demo.settings");
}

#[tokio::test]
async fn execution_event_sees_snapshot() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let rt = setup(Arc::clone(&seen)).await;
    rt.set_settings_provider(Arc::new(FakeProvider::default()));

    let outcome = rt
        .execution_event(
            ActionId::from_static("demo.settings.act"),
            "item-1",
            ExecutionEventKind::Activate,
        )
        .await;
    assert!(matches!(outcome, Some(ActionOutcome::Text(_))));
    assert_eq!(seen.lock().unwrap()[0].revision, 9);
}

#[tokio::test]
async fn watch_settings_pushes_provider_updates() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let rt = setup(seen).await;
    let provider = Arc::new(FakeProvider::default());
    rt.set_settings_provider(Arc::clone(&provider) as Arc<dyn SettingsProvider>);

    let mut receiver = rt.watch_settings("demo.settings");
    assert_eq!(receiver.borrow_and_update().revision, 9);

    let mut updated = snapshot();
    updated.revision = 10;
    updated
        .values
        .insert("enabled".to_string(), SettingValue::Bool(false));
    provider.push("demo.settings", updated);

    receiver.changed().await.unwrap();
    let current = receiver.borrow_and_update().clone();
    assert_eq!(current.revision, 10);
    assert_eq!(current.bool("enabled"), Some(false));
}

#[tokio::test]
async fn watch_settings_without_provider_stays_empty() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let rt = setup(seen).await;
    let receiver = rt.watch_settings("demo.settings");
    assert_eq!(receiver.borrow().revision, 0);
    assert!(receiver.borrow().values.is_empty());
}
