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
        _: PluginContext,
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
        _: PluginContext,
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
        _ctx: flick_domain::PluginContext,
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

// ─── 并行、取消、上限与输入透传 ───────────────────────────────────────

// 复用文件中部已有的 `use std::sync::atomic::{AtomicUsize, Ordering};`
// （为 `CountingSurfaceHandler` 服务的那一行），此处只补下面这些 import，
// 否则会触发 E0252 重复导入。
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 记录收到的输入，并在固定延迟后返回一条执行项。
struct SlowSearchHandler {
    /// 处理器被调用次数。
    calls: Arc<AtomicUsize>,
    /// 处理器观察到的输入。
    seen: Arc<Mutex<Vec<ActionInput>>>,
    /// 模拟耗时。
    delay: Duration,
}

#[async_trait]
impl ActionHandler for SlowSearchHandler {
    async fn invoke(
        &self,
        request: ActionRequest,
        _: PluginContext,
    ) -> Result<ActionOutcome, ActionError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().unwrap().push(request.input.clone());
        tokio::time::sleep(self.delay).await;
        Ok(ActionOutcome::Data(serde_json::json!([
            {"id": "slow:item", "title": "Slow"}
        ])))
    }
}

/// 返回固定条数执行项的处理器。
struct BulkSearchHandler {
    /// 返回条数。
    count: usize,
}

#[async_trait]
impl ActionHandler for BulkSearchHandler {
    async fn invoke(
        &self,
        _request: ActionRequest,
        _: PluginContext,
    ) -> Result<ActionOutcome, ActionError> {
        let values: Vec<serde_json::Value> = (0..self.count)
            .map(|i| serde_json::json!({ "id": format!("bulk:{i}"), "title": format!("Bulk {i}") }))
            .collect();
        Ok(ActionOutcome::Data(serde_json::Value::Array(values)))
    }
}

/// 阻塞到取消令牌触发才返回的处理器，并记录是否真正观察到取消。
struct CancellableSearchHandler {
    /// 通知测试「已经进入处理器」。
    started: Arc<tokio::sync::Notify>,
    /// 观察到的取消信号（证明在途调用被级联取消）。
    cancelled_seen: Arc<AtomicBool>,
}

#[async_trait]
impl ActionHandler for CancellableSearchHandler {
    async fn invoke(
        &self,
        _request: ActionRequest,
        context: PluginContext,
    ) -> Result<ActionOutcome, ActionError> {
        self.started.notify_one();
        context.cancellation.cancelled().await;
        self.cancelled_seen.store(true, Ordering::SeqCst);
        Err(ActionError::Cancelled)
    }
}

/// 构造一个声明了 `search_invocable` 的插件。
struct SearchablePlugin {
    /// 插件 id（动作 id 必须以 `{id}.` 开头）。
    plugin_id: String,
    /// 每个动作的 (动作 id, 处理器, 支持的输入类型)。
    actions: Vec<(ActionId, Arc<dyn ActionHandler>, Vec<InputType>)>,
}

impl Plugin for SearchablePlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: self.plugin_id.clone(),
            name: "Searchable".into(),
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
        self.actions
            .iter()
            .map(|(id, handler, inputs)| {
                let mut d = ActionDescriptor::new(
                    id.clone(),
                    id.as_str(),
                    vec![],
                    ConcurrencyPolicy::Parallel,
                );
                d.search_invocable = true;
                d.supported_inputs = inputs.clone();
                ActionSpec {
                    descriptor: d,
                    handler: Arc::clone(handler),
                }
            })
            .collect()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_invoke_runs_actions_in_parallel() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let handler = Arc::new(SlowSearchHandler {
        calls: Arc::clone(&calls),
        seen,
        delay: Duration::from_millis(100),
    });
    let plugin = SearchablePlugin {
        plugin_id: "test.parallel".into(),
        actions: vec![
            (
                ActionId::from_static("test.parallel.v1.a"),
                handler.clone(),
                vec![InputType::PlainText],
            ),
            (
                ActionId::from_static("test.parallel.v1.b"),
                handler.clone(),
                vec![InputType::PlainText],
            ),
            (
                ActionId::from_static("test.parallel.v1.c"),
                handler.clone(),
                vec![InputType::PlainText],
            ),
        ],
    };
    let runtime = ActionRuntime::new();
    runtime.register_plugin(&plugin).await.unwrap();

    let started = Instant::now();
    let items = runtime
        .search_invoke(
            QueryGeneration::INITIAL,
            ActionInput::plain_text("slow"),
        )
        .await;
    let elapsed = started.elapsed();

    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(items.len(), 3);
    assert!(
        elapsed < Duration::from_millis(300),
        "3 x 100ms must run in parallel, took {elapsed:?}"
    );
}

#[tokio::test]
async fn search_invoke_truncates_to_twenty_items_per_action() {
    let plugin = SearchablePlugin {
        plugin_id: "test.bulk".into(),
        actions: vec![(
            ActionId::from_static("test.bulk.v1.search"),
            Arc::new(BulkSearchHandler { count: 50 }),
            vec![InputType::PlainText],
        )],
    };
    let runtime = ActionRuntime::new();
    runtime.register_plugin(&plugin).await.unwrap();

    let items = runtime
        .search_invoke(
            QueryGeneration::INITIAL,
            ActionInput::plain_text("bulk"),
        )
        .await;

    assert_eq!(items.len(), MAX_EXECUTION_ITEMS_PER_ACTION);
    assert_eq!(items[0].item_id, "bulk:0");
    assert_eq!(items[MAX_EXECUTION_ITEMS_PER_ACTION - 1].item_id, "bulk:19");
}

#[tokio::test]
async fn search_invoke_passes_files_and_image_to_handler() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let plugin = SearchablePlugin {
        plugin_id: "test.attach".into(),
        actions: vec![(
            ActionId::from_static("test.attach.v1.search"),
            Arc::new(SlowSearchHandler {
                calls: Arc::new(AtomicUsize::new(0)),
                seen: Arc::clone(&seen),
                delay: Duration::ZERO,
            }),
            vec![InputType::Files],
        )],
    };
    let runtime = ActionRuntime::new();
    runtime.register_plugin(&plugin).await.unwrap();

    let mut input = ActionInput::plain_text("a.txt b.txt");
    input.kind = InputType::Files;
    input.files = vec![FileInput {
        uri: "file:///tmp/a.txt".into(),
        display_name: "a.txt".into(),
        extension: Some("txt".into()),
        is_dir: false,
        mime: None,
    }];

    let items = runtime
        .search_invoke(QueryGeneration::INITIAL, input)
        .await;

    assert_eq!(items.len(), 1);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].kind, InputType::Files);
    assert_eq!(seen[0].files.len(), 1);
    assert_eq!(seen[0].files[0].display_name, "a.txt");
}

#[tokio::test]
async fn new_generation_cancels_previous_search() {
    let started = Arc::new(tokio::sync::Notify::new());
    let cancelled_seen = Arc::new(AtomicBool::new(false));
    let plugin = SearchablePlugin {
        plugin_id: "test.cancel".into(),
        actions: vec![(
            ActionId::from_static("test.cancel.v1.search"),
            Arc::new(CancellableSearchHandler {
                started: Arc::clone(&started),
                cancelled_seen: Arc::clone(&cancelled_seen),
            }),
            vec![InputType::PlainText],
        )],
    };
    let runtime = Arc::new(ActionRuntime::new());
    runtime.register_plugin(&plugin).await.unwrap();

    let gen1 = QueryGeneration::INITIAL;
    let gen2 = gen1.next();

    let runtime_for_task = Arc::clone(&runtime);
    let launched_at = Instant::now();
    let first = tokio::spawn(async move {
        runtime_for_task
            .search_invoke(gen1, ActionInput::plain_text("cancel"))
            .await
    });

    // 等第一个代次真正进入处理器，确保取消到达时它确实在途。
    started.notified().await;

    // 新代次用图片输入：该 Action 只支持 PlainText，因此不会被再次调用，
    // 但 `search_invoke` 仍会先取消旧代次的令牌（级联到在途的子令牌）。
    let second = runtime
        .search_invoke(gen2, ActionInput::clipboard_image("image/png", vec![1, 2, 3]))
        .await;
    assert!(second.is_empty());

    let first_items = tokio::time::timeout(Duration::from_secs(2), first)
        .await
        .expect("cancelled search must return promptly")
        .unwrap();
    let elapsed = launched_at.elapsed();

    assert!(
        first_items.is_empty(),
        "cancelled generation must not produce items"
    );
    assert!(
        cancelled_seen.load(Ordering::SeqCst),
        "handler must observe cancellation"
    );
    // 关键判据（区分「被取消」与「超时」）：gen1 的 deadline 固定为
    // SEARCH_INVOKE_TIMEOUT_MS = 150ms。若代次取消失效，gen1 会一直阻塞到
    // 150ms 的 DeadlineExceeded 才返回；届时 items 同样为空，且截止分支也会
    // 取消子令牌（故 cancelled_seen 亦为 true），前两条断言都无法区分。
    // 真正生效的级联取消能让 gen1 远早于 150ms 返回，故用耗时区分之。
    assert!(
        elapsed < Duration::from_millis(120),
        "generation cancellation must return well before the 150ms deadline, took {elapsed:?}"
    );
}

#[tokio::test]
async fn execution_item_carries_real_invocation_id() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let plugin = SearchablePlugin {
        plugin_id: "test.ids".into(),
        actions: vec![(
            ActionId::from_static("test.ids.v1.search"),
            Arc::new(RecordingInvocationHandler {
                seen: Arc::clone(&seen),
            }),
            vec![InputType::PlainText],
        )],
    };
    let runtime = ActionRuntime::new();
    runtime.register_plugin(&plugin).await.unwrap();

    let items = runtime
        .search_invoke(
            QueryGeneration::INITIAL,
            ActionInput::plain_text("ids"),
        )
        .await;

    assert_eq!(items.len(), 1);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        items[0].invocation_id,
        Some(seen[0]),
        "execution item must carry the real invocation id"
    );
}

/// 记录收到的 `invocation_id` 并返回一条执行项。
struct RecordingInvocationHandler {
    /// 收到的调用 id。
    seen: Arc<Mutex<Vec<InvocationId>>>,
}

#[async_trait]
impl ActionHandler for RecordingInvocationHandler {
    async fn invoke(
        &self,
        request: ActionRequest,
        _: PluginContext,
    ) -> Result<ActionOutcome, ActionError> {
        self.seen.lock().unwrap().push(request.invocation_id);
        Ok(ActionOutcome::Data(serde_json::json!([
            {"id": "ids:item", "title": "Ids"}
        ])))
    }
}
