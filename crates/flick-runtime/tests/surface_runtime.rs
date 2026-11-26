//! Wave A 运行时 Surface 会话测试。

use async_trait::async_trait;
use flick_domain::*;
use flick_runtime::{
    ActionHandler, ActionRuntime, ActionSpec, Plugin, SurfaceError, SurfaceHandler, SurfaceSession,
};
use std::sync::Arc;

struct OpenHandler;

#[async_trait]
impl ActionHandler for OpenHandler {
    async fn invoke(
        &self,
        _r: ActionRequest,
        _c: ActionContext,
    ) -> Result<ActionOutcome, ActionError> {
        let content = SurfaceContent {
            schema_version: SURFACE_SCHEMA_VERSION,
            title: Some("S".into()),
            body: vec![],
        };
        Ok(ActionOutcome::open_surface("info", content))
    }
}

struct EchoSurface;

#[async_trait]
impl SurfaceHandler for EchoSurface {
    async fn on_event(
        &self,
        _session: &SurfaceSession,
        event: SurfaceEventKind,
    ) -> Result<SurfaceContent, SurfaceError> {
        Ok(SurfaceContent {
            schema_version: SURFACE_SCHEMA_VERSION,
            title: Some(format!("{event:?}")),
            body: vec![],
        })
    }
}

struct SurfacePlugin {
    m: PluginManifest,
}

impl Plugin for SurfacePlugin {
    fn manifest(&self) -> PluginManifest {
        self.m.clone()
    }
    fn actions(&self) -> Vec<ActionSpec> {
        vec![ActionSpec {
            descriptor: ActionDescriptor::new(
                ActionId::new("demo.surface.open").unwrap(),
                "Open",
                vec![],
                ConcurrencyPolicy::Parallel,
            ),
            handler: Arc::new(OpenHandler),
        }]
    }
    fn surface_handler(&self) -> Option<Arc<dyn SurfaceHandler>> {
        Some(Arc::new(EchoSurface))
    }
}

fn manifest(needs_search: bool) -> PluginManifest {
    PluginManifest {
        id: "demo.surface".into(),
        name: "Surface".into(),
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
        needs_search,
    }
}

async fn setup(needs_search: bool) -> ActionRuntime {
    let rt = ActionRuntime::default();
    rt.register_plugin(&SurfacePlugin {
        m: manifest(needs_search),
    })
    .await
    .unwrap();
    rt
}

async fn invoke_open(rt: &ActionRuntime) -> ActionOutcome {
    let mut req = ActionRequest::new_echo("open", Caller::cli("test"));
    req.action_id = ActionId::from_static("demo.surface.open");
    rt.invoke(req).await
}

#[tokio::test]
async fn invoke_open_surface_creates_session_with_needs_search() {
    let rt = setup(true).await;
    match invoke_open(&rt).await {
        ActionOutcome::OpenSurface {
            session_id,
            needs_search,
            ..
        } => {
            assert!(
                needs_search,
                "needs_search should be backfilled from manifest"
            );
            assert!(session_id.is_some(), "runtime should generate a session id");
        }
        other => panic!("unexpected outcome: {other:?}"),
    }
}

#[tokio::test]
async fn invoke_open_surface_needs_search_false_when_manifest_says_so() {
    let rt = setup(false).await;
    match invoke_open(&rt).await {
        ActionOutcome::OpenSurface { needs_search, .. } => assert!(!needs_search),
        other => panic!("unexpected outcome: {other:?}"),
    }
}

#[tokio::test]
async fn surface_event_routes_to_handler() {
    let rt = setup(true).await;
    let sid = match invoke_open(&rt).await {
        ActionOutcome::OpenSurface {
            session_id: Some(id),
            ..
        } => id,
        other => panic!("unexpected outcome: {other:?}"),
    };
    let content = rt
        .surface_event(
            sid,
            SurfaceEventKind::Click {
                node_id: "n".into(),
            },
        )
        .await
        .expect("event should route");
    assert!(content.title.as_deref().unwrap_or("").contains("Click"));
}

#[tokio::test]
async fn surface_event_unknown_session_errors() {
    let rt = setup(true).await;
    let err = rt
        .surface_event(
            SurfaceSessionId::new(),
            SurfaceEventKind::Click {
                node_id: "n".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.message, "surface session not found");
}

#[tokio::test]
async fn surface_closed_is_idempotent_and_removes_session() {
    let rt = setup(true).await;
    let sid = match invoke_open(&rt).await {
        ActionOutcome::OpenSurface {
            session_id: Some(id),
            ..
        } => id,
        other => panic!("unexpected outcome: {other:?}"),
    };
    rt.surface_closed(sid).await;
    // 第二次关闭不应 panic。
    rt.surface_closed(sid).await;
    // 关闭后会话已移除，事件应返回 not found。
    let err = rt
        .surface_event(
            sid,
            SurfaceEventKind::Click {
                node_id: "n".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.message, "surface session not found");
}
