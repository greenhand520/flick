//! 可运行 demo：内置真实的 `ActionRuntime`（echo + host + 剪贴板示例插件），不依赖 daemon。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use flick_domain::*;
use flick_runtime::{
    ActionHandler, ActionRuntime, ActionSpec, Plugin, SurfaceError, SurfaceHandler, SurfaceSession,
};
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// 演示插件：demo.builtin.clipboard
//
// 展示 Plan 2c 的交互协议：manifest 声明 needs_search=true（借用宿主搜索框），
// 初始返回含 3 个带 id 的 ListItem + 一个 "Clear" Button 的 SurfaceContent；
// SearchChanged 过滤列表；点击条目返回 Badge("已选择")；Toggled/TextChanged 优雅忽略。
// 会话状态以 Mutex<HashMap<SurfaceSessionId, ClipboardState>> 保存。
// ---------------------------------------------------------------------------

const CLIPBOARD_ACTION_ID: &str = "demo.builtin.clipboard.show";
const CLIPBOARD_SURFACE_ID: &str = "clipboard";

const CLIPBOARD_ITEMS: [(&str, &str); 3] = [
    ("item-1", "第一条剪贴板内容"),
    ("item-2", "第二条剪贴板内容"),
    ("item-3", "第三条剪贴板内容"),
];
const CLIPBOARD_SUBTITLES: [&str; 3] = ["刚刚复制", "5 分钟前", "昨天"];

#[derive(Default)]
struct ClipboardState {
    filter: String,
}

/// 构建剪贴板 SurfaceContent；`filter` 非空时仅保留 title 含该子串的条目。
fn clipboard_surface(filter: &str) -> SurfaceContent {
    let mut items = Vec::new();
    for (i, (id, title)) in CLIPBOARD_ITEMS.iter().enumerate() {
        if !filter.is_empty() && !title.contains(filter) {
            continue;
        }
        items.push(ListItem {
            id: Some((*id).to_string()),
            title: (*title).to_string(),
            subtitle: Some(CLIPBOARD_SUBTITLES[i].to_string()),
            value: Some("复制".to_string()),
            badge: None,
        });
    }
    SurfaceContent {
        schema_version: SURFACE_SCHEMA_VERSION,
        title: Some("剪贴板".to_string()),
        body: vec![
            SurfaceNode::List { id: None, items },
            SurfaceNode::Button {
                id: "clear".to_string(),
                label: "Clear".to_string(),
                tone: ButtonTone::Danger,
            },
        ],
    }
}

fn selected_surface(item_id: &str) -> SurfaceContent {
    let title = CLIPBOARD_ITEMS
        .iter()
        .find(|(id, _)| *id == item_id)
        .map(|(_, title)| (*title).to_string())
        .unwrap_or_else(|| "未知条目".to_string());
    SurfaceContent {
        schema_version: SURFACE_SCHEMA_VERSION,
        title: Some("剪贴板".to_string()),
        body: vec![
            SurfaceNode::Badge {
                id: None,
                text: "已选择".to_string(),
                tone: BadgeTone::Success,
            },
            SurfaceNode::Text {
                id: None,
                text: title,
                muted: false,
            },
        ],
    }
}

struct DemoClipboardPlugin {
    sessions: Arc<Mutex<HashMap<SurfaceSessionId, ClipboardState>>>,
}

impl DemoClipboardPlugin {
    fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn descriptor() -> ActionDescriptor {
        let mut d = ActionDescriptor::new(
            ActionId::from_static(CLIPBOARD_ACTION_ID),
            "剪贴板",
            vec!["clipboard".into(), "剪贴板".into(), "paste".into()],
            ConcurrencyPolicy::Parallel,
        );
        d.output_types = vec![OutputType::Surface];
        d
    }
}

impl Plugin for DemoClipboardPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "demo.builtin.clipboard".into(),
            name: "Demo Clipboard".into(),
            author: "Flick Demo".into(),
            version: "1.0.0".into(),
            kind: PluginKind::BuiltInRust,
            icon: None,
            keywords: vec!["clipboard".into(), "剪贴板".into(), "paste".into()],
            profile: PluginProfile {
                description: "演示可搜索 / 可交互的剪贴板 Surface".into(),
                categories: vec![Category::System],
                tags: vec!["clipboard".into(), "demo".into()],
                languages: vec![],
                execution: ExecutionMode::Local,
                data_handling: None,
            },
            permissions: vec![],
            plugin_dependencies: vec![],
            settings: vec![],
            surfaces: vec![UiSurfaceDecl {
                id: CLIPBOARD_SURFACE_ID.into(),
                title: "剪贴板".into(),
                kind: SurfaceKind::Panel,
                default_placement: SurfacePlacement::BelowSearchBox,
            }],
            supported_os: vec![],
            supported_architectures: vec![],
            needs_search: true,
        }
    }

    fn actions(&self) -> Vec<ActionSpec> {
        vec![ActionSpec {
            descriptor: Self::descriptor(),
            // 动作处理器与 Surface 处理器共享同一份会话表。
            handler: Arc::new(DemoClipboardPlugin {
                sessions: self.sessions.clone(),
            }),
        }]
    }

    fn surface_handler(&self) -> Option<Arc<dyn SurfaceHandler>> {
        Some(Arc::new(DemoClipboardPlugin {
            sessions: self.sessions.clone(),
        }))
    }
}

#[async_trait]
impl ActionHandler for DemoClipboardPlugin {
    async fn invoke(
        &self,
        _request: ActionRequest,
        _ctx: PluginContext,
    ) -> Result<ActionOutcome, ActionError> {
        Ok(ActionOutcome::open_surface(
            CLIPBOARD_SURFACE_ID,
            clipboard_surface(""),
        ))
    }
}

#[async_trait]
impl SurfaceHandler for DemoClipboardPlugin {
    async fn on_open(&self, session: &SurfaceSession) {
        self.sessions
            .lock()
            .unwrap()
            .insert(session.id, ClipboardState::default());
    }

    async fn on_event(
        &self,
        session: &SurfaceSession,
        event: SurfaceEventKind,
        _ctx: PluginContext,
    ) -> Result<SurfaceContent, SurfaceError> {
        let mut guard = self.sessions.lock().unwrap();
        let state = guard.entry(session.id).or_default();
        match event {
            SurfaceEventKind::SearchChanged { text } => {
                state.filter = text;
                Ok(clipboard_surface(&state.filter))
            }
            SurfaceEventKind::Submitted { node_id, text } => {
                if node_id == "__search__" {
                    state.filter = text;
                }
                Ok(clipboard_surface(&state.filter))
            }
            // 输入框文本变更 / 勾选：当前实现不做处理，但优雅返回当前内容。
            SurfaceEventKind::TextChanged { .. } | SurfaceEventKind::Toggled { .. } => {
                Ok(clipboard_surface(&state.filter))
            }
            SurfaceEventKind::Click { node_id } => {
                if node_id == "clear" {
                    Ok(SurfaceContent {
                        schema_version: SURFACE_SCHEMA_VERSION,
                        title: Some("剪贴板".to_string()),
                        body: vec![SurfaceNode::Text {
                            id: None,
                            text: "已清空".to_string(),
                            muted: true,
                        }],
                    })
                } else {
                    Ok(selected_surface(&node_id))
                }
            }
            // v1 不发送右键事件；若收到则原样返回。
            SurfaceEventKind::ContextMenu { .. } => Ok(clipboard_surface(&state.filter)),
        }
    }

    async fn on_close(&self, session: &SurfaceSession) {
        self.sessions.lock().unwrap().remove(&session.id);
    }
}

async fn fake_host(
    host_tx: mpsc::UnboundedSender<HostToUi>,
    mut ui_rx: mpsc::UnboundedReceiver<UiToHost>,
) {
    // 使用与 daemon 相同的运行时与内置插件，让 demo 的路由/调用行为贴近真实。
    let runtime = Arc::new(ActionRuntime::new(
        Arc::new(flick_adapters_linux::host_info::collect_host_info()),
        Arc::new(flick_domain::NoopPaths),
    ));
    flick_builtins::register_builtin(&runtime)
        .await
        .expect("register builtin plugins");
    let _ = runtime.register_plugin(&DemoClipboardPlugin::new()).await;
    runtime.activate_all_plugins().await;

    // 模拟一次全局快捷键唤出面板。
    let _ = host_tx.send(HostToUi::ShowSearchPanel {
        activation_token: None,
    });

    while let Some(message) = ui_rx.recv().await {
        match message {
            UiToHost::QueryChanged { generation, input } => {
                let action_input = input.to_action_input();
                let context = InputContext::from_snapshot(generation, &input);
                if !input.files.is_empty() || input.image.is_some() {
                    eprintln!(
                        "[demo] input kind={:?} source={:?} files={} image={}",
                        input.kind,
                        input.source,
                        input.files.len(),
                        input.image.is_some()
                    );
                }
                // 与协调器同构的两阶段：先出候选，再补执行项。
                let items = runtime.route_input(&context).await;
                let _ = host_tx.send(HostToUi::Routes { generation, items });
                let execution_items = runtime.search_invoke(generation, action_input).await;
                if !execution_items.is_empty() {
                    let _ = host_tx.send(HostToUi::ExecutionResults {
                        generation,
                        items: execution_items,
                    });
                }
            }
            UiToHost::Invoke {
                invocation_id,
                action_id,
                input,
                ..
            } => {
                let request = ActionRequest {
                    invocation_id,
                    action_id,
                    input,
                    caller: Caller::ui("gpui-demo"),
                    source: flick_domain::ActionSource::Ui,
                    deadline: Deadline::from_secs(30),
                };
                let outcome = runtime.invoke(request).await;
                let _ = host_tx.send(HostToUi::InvocationCompleted {
                    invocation_id,
                    outcome,
                });
            }
            UiToHost::ExecutionActivated {
                generation: _,
                parent_action_id,
                invocation_id,
                item_id,
                event,
            } => {
                let item_id = item_id.unwrap_or_default();
                if let Some(outcome) = runtime
                    .execution_event(parent_action_id, &item_id, event)
                    .await
                {
                    let _ = host_tx.send(HostToUi::InvocationCompleted {
                        invocation_id,
                        outcome,
                    });
                }
            }
            UiToHost::Cancel { .. } => {}
            UiToHost::SearchPanelClosed => {
                eprintln!("[demo] search panel closed");
            }
            UiToHost::SurfaceEvent { session_id, kind } => {
                match runtime.surface_event(session_id, kind).await {
                    Ok(content) => {
                        let _ = host_tx.send(HostToUi::SurfaceUpdated {
                            session_id,
                            content,
                        });
                    }
                    Err(err) => {
                        let _ = host_tx.send(HostToUi::SurfaceFailed {
                            session_id,
                            message: err.message,
                        });
                    }
                }
            }
            UiToHost::SurfaceClosed { session_id } => {
                runtime.surface_closed(session_id).await;
            }
            // demo 不持久化主题。
            UiToHost::SetTheme { .. } => {}
        }
    }
}

fn main() {
    let (ui_tx, ui_rx) = mpsc::unbounded_channel::<UiToHost>();
    let (host_tx, host_rx) = mpsc::unbounded_channel::<HostToUi>();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build host runtime");
        rt.block_on(fake_host(host_tx, ui_rx));
    });

    let initial = flick_shell_gpui::detect_initial_theme();
    flick_shell_gpui::run_with_theme(ui_tx, host_rx, initial);
}
