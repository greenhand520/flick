//! S1-A：插件目录提供者注入到 PluginContext 的运行时测试。

use async_trait::async_trait;
use flick_domain::*;
use flick_runtime::{ActionHandler, ActionRuntime, ActionSpec, Plugin};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq)]
struct Seen {
    plugin_id: String,
    data: Option<PathBuf>,
    cache: Option<PathBuf>,
    tmp: Option<PathBuf>,
}

struct FakePaths;

impl PluginPaths for FakePaths {
    fn data_dir(&self, plugin_id: &str) -> Option<PathBuf> {
        (plugin_id == "demo.paths").then(|| PathBuf::from("/data").join(plugin_id))
    }

    fn cache_dir(&self, plugin_id: &str) -> Option<PathBuf> {
        (plugin_id == "demo.paths").then(|| PathBuf::from("/cache").join(plugin_id))
    }

    fn tmp_dir(&self, plugin_id: &str) -> Option<PathBuf> {
        (plugin_id == "demo.paths").then(|| PathBuf::from("/runtime").join(plugin_id))
    }
}

struct Recorder {
    seen: Arc<Mutex<Vec<Seen>>>,
}

#[async_trait]
impl ActionHandler for Recorder {
    async fn invoke(
        &self,
        _r: ActionRequest,
        c: PluginContext,
    ) -> Result<ActionOutcome, ActionError> {
        self.seen.lock().unwrap().push(Seen {
            plugin_id: c.plugin_id().to_string(),
            data: c.data_dir(),
            cache: c.cache_dir(),
            tmp: c.tmp_dir(),
        });
        Ok(ActionOutcome::Text("ok".into()))
    }
}

struct DemoPlugin {
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Plugin for DemoPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "demo.paths".into(),
            name: "Paths Demo".into(),
            author: "Flick".into(),
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
                ActionId::new("demo.paths.act").unwrap(),
                "Act",
                vec![],
                ConcurrencyPolicy::Parallel,
            ),
            handler: Arc::new(Recorder {
                seen: Arc::clone(&self.seen),
            }),
        }]
    }
}

fn request() -> ActionRequest {
    let mut req = ActionRequest::new_echo("x", Caller::cli("test"));
    req.action_id = ActionId::from_static("demo.paths.act");
    req
}

async fn setup(seen: Arc<Mutex<Vec<Seen>>>, paths: Arc<dyn PluginPaths>) -> ActionRuntime {
    let rt = ActionRuntime::new(Arc::new(HostInfo::default()), paths);
    rt.register_plugin(&DemoPlugin { seen }).await.unwrap();
    rt
}

#[tokio::test]
async fn invoke_sees_paths_scoped_to_current_plugin() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let rt = setup(Arc::clone(&seen), Arc::new(FakePaths)).await;

    let outcome = rt.invoke(request()).await;
    assert!(matches!(outcome, ActionOutcome::Text(_)));

    let seen = seen.lock().unwrap();
    assert_eq!(
        seen[0],
        Seen {
            plugin_id: "demo.paths".into(),
            data: Some(PathBuf::from("/data/demo.paths")),
            cache: Some(PathBuf::from("/cache/demo.paths")),
            tmp: Some(PathBuf::from("/runtime/demo.paths")),
        }
    );
}

#[tokio::test]
async fn invoke_with_noop_paths_gets_no_dirs() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let rt = setup(Arc::clone(&seen), Arc::new(NoopPaths)).await;

    rt.invoke(request()).await;
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].plugin_id, "demo.paths");
    assert_eq!(seen[0].data, None);
    assert_eq!(seen[0].cache, None);
    assert_eq!(seen[0].tmp, None);
}
