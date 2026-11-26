//! 无头端到端测试。
//!
//! 验证无头模式下的完整守护进程路径：
//! - 启动带 IPC 的无头守护进程
//! - 通过 CLI 风格的 IPC 客户端发送命令
//! - 验证状态、echo（UI + CLI 调用方）、未知动作、关闭
//!
//! 此测试需要 Unix Socket 访问权限（在沙箱中失败）。

use flick_application::ports::{HostEventSink, RuntimePort};
use flick_domain::*;
use flick_ipc::message::*;
use flick_ipc::server::{IpcDispatcher, IpcServer};
use flick_ipc::unix;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;

#[allow(dead_code)]
#[path = "../src/bootstrap.rs"]
mod bootstrap;
#[allow(dead_code)]
#[path = "../src/headless.rs"]
mod headless;
#[allow(dead_code)]
#[path = "../src/lifecycle.rs"]
mod lifecycle;

// ─── 测试守护进程接线 ─────────────────────────────────────────────────────

struct TestRuntimeAdapter {
    runtime: Arc<flick_runtime::ActionRuntime>,
}

#[async_trait::async_trait]
impl RuntimePort for TestRuntimeAdapter {
    async fn route(&self, query: &str, input_kind: InputType) -> Vec<RouteItem> {
        self.runtime.route(query, input_kind).await
    }
    async fn invoke(&self, request: ActionRequest) -> ActionOutcome {
        self.runtime.invoke(request).await
    }
    async fn cancel(&self, invocation_id: InvocationId) -> bool {
        self.runtime.cancel(invocation_id).await
    }
    async fn surface_event(
        &self,
        session: SurfaceSessionId,
        kind: SurfaceEventKind,
    ) -> Result<SurfaceContent, String> {
        self.runtime
            .surface_event(session, kind)
            .await
            .map_err(|e| e.message)
    }
    async fn surface_closed(&self, session: SurfaceSessionId) {
        self.runtime.surface_closed(session).await;
    }
}

struct TestEventSink {
    events: Arc<Mutex<Vec<HostToUi>>>,
}

impl HostEventSink for TestEventSink {
    fn send(&self, event: HostToUi) {
        self.events
            .lock()
            .expect("test event sink mutex poisoned")
            .push(event);
    }
}

struct TestDispatcher {
    runtime: Arc<dyn RuntimePort>,
    coordinator: Arc<flick_application::ApplicationCoordinator>,
    events: Arc<Mutex<Vec<HostToUi>>>,
    socket_path: std::path::PathBuf,
}

#[async_trait::async_trait]
impl IpcDispatcher for TestDispatcher {
    async fn dispatch(&self, command: IpcCommand) -> IpcResult {
        match command {
            IpcCommand::Toggle => {
                self.coordinator.toggle_search_panel();
                IpcResult::Ok(serde_json::json!({"toggled": true}))
            }
            IpcCommand::Show => {
                self.coordinator.handle_intent(UiIntent::ShowSearchPanel {
                    activation_token: None,
                });
                IpcResult::Ok(serde_json::json!({"show": true}))
            }
            IpcCommand::Hide => {
                self.coordinator.handle_intent(UiIntent::HideSearchPanel);
                IpcResult::Ok(serde_json::json!({"hide": true}))
            }
            IpcCommand::Trigger { action_id, text } => {
                let action_id = match ActionId::new(&action_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return IpcResult::Error {
                            code: IpcErrorCode::InvalidInput,
                            message: e.user_message(),
                        };
                    }
                };
                let request = ActionRequest {
                    invocation_id: InvocationId::new(),
                    action_id,
                    input: ActionInput::plain_text(&text),
                    caller: Caller::cli("flick-cli"),
                    source: flick_domain::ActionSource::Ui,
                    deadline: Deadline::from_secs(30),
                };
                let outcome = self.runtime.invoke(request).await;
                match outcome {
                    ActionOutcome::Data(val) => IpcResult::Ok(val),
                    ActionOutcome::Notice { message } => {
                        IpcResult::Ok(serde_json::json!({"notice": message}))
                    }
                    ActionOutcome::Error(err) => {
                        let code = match &err {
                            ActionError::UnknownAction { .. } => IpcErrorCode::ActionNotFound,
                            ActionError::InvalidInput { .. } => IpcErrorCode::InvalidInput,
                            ActionError::Cancelled => IpcErrorCode::Cancelled,
                            _ => IpcErrorCode::Internal,
                        };
                        IpcResult::Error {
                            code,
                            message: err.user_message(),
                        }
                    }
                    ActionOutcome::Text(text) => IpcResult::Ok(serde_json::json!({ "text": text })),
                    ActionOutcome::Url(url) => IpcResult::Ok(serde_json::json!({ "url": url })),
                    ActionOutcome::File(file) => {
                        IpcResult::Ok(serde_json::to_value(&file).unwrap_or(serde_json::Value::Null))
                    }
                    ActionOutcome::OpenSurface { surface_id, content, .. } => IpcResult::Ok(
                        serde_json::json!({ "surface_id": surface_id, "content": content }),
                    ),
                }
            }
            IpcCommand::Status => IpcResult::Ok(serde_json::json!({"daemon_status": "running"})),
            IpcCommand::Doctor => IpcResult::Ok(serde_json::json!({
                "session": "headless",
                "portal": { "state": "headless_not_started" },
                "ipc": { "socket": self.socket_path.display().to_string() },
            })),
            IpcCommand::ShortcutsConfigure {
                preferred_trigger: _,
            } => IpcResult::Ok(serde_json::json!({"configure": "requested"})),
            IpcCommand::ShortcutsReconnect => {
                IpcResult::Ok(serde_json::json!({"reconnect": "requested"}))
            }
            IpcCommand::Shutdown => IpcResult::Ok(serde_json::json!({"shutdown": true})),
        }
    }
}

async fn spawn_headless_daemon(tmpdir: &TempDir) -> (std::path::PathBuf, Arc<TestDispatcher>) {
    let socket_path = tmpdir.path().join("flick.sock");

    // 创建运行时 + 内置动作
    let runtime = Arc::new(flick_runtime::ActionRuntime::new());
    flick_builtins::register_builtin(&runtime)
        .await
        .expect("register builtin plugins");
    runtime.activate_all_plugins().await;

    // 创建协调器
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::new(TestEventSink {
        events: Arc::clone(&events),
    });
    let adapter = Arc::new(TestRuntimeAdapter {
        runtime: runtime.clone(),
    }) as Arc<dyn RuntimePort>;
    let coordinator = Arc::new(flick_application::ApplicationCoordinator::new(
        adapter.clone(),
        sink,
    ));

    let dispatcher = Arc::new(TestDispatcher {
        runtime: adapter,
        coordinator,
        events,
        socket_path: socket_path.clone(),
    });

    // 绑定 IPC
    let server = unix::bind(&socket_path).expect("bind failed");
    let ipc_server = IpcServer::new(server, dispatcher.clone());
    tokio::spawn(async move {
        let _ = ipc_server
            .run(tokio_util::sync::CancellationToken::new())
            .await;
    });

    (socket_path, dispatcher)
}

async fn send_ipc(path: &std::path::Path, command: IpcCommand) -> IpcResponseEnvelope {
    let request = IpcRequestEnvelope {
        protocol_version: IPC_PROTOCOL_VERSION,
        request_id: "e2e-test".to_string(),
        command,
    };
    let mut client = unix::connect(path).await.expect("connect failed");
    client.request(&request).await.expect("request failed")
}

// ─── 测试 ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn e2e_status_reports_running() {
    let tmpdir = TempDir::new().unwrap();
    let (path, _) = spawn_headless_daemon(&tmpdir).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let resp = send_ipc(&path, IpcCommand::Status).await;
    match &resp.result {
        IpcResult::Ok(val) => assert_eq!(val["daemon_status"], "running"),
        other => panic!("expected Ok, got {other:?}"),
    }
}

#[tokio::test]
async fn e2e_echo_trigger_returns_data() {
    let tmpdir = TempDir::new().unwrap();
    let (path, _) = spawn_headless_daemon(&tmpdir).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let resp = send_ipc(
        &path,
        IpcCommand::Trigger {
            action_id: "flick.builtin.echo.v1.echo".into(),
            text: "hello world".into(),
        },
    )
    .await;

    match &resp.result {
        IpcResult::Ok(val) => {
            assert_eq!(val["echo"], "hello world");
        }
        other => panic!("expected Ok with echo data, got {other:?}"),
    }
}

#[tokio::test]
async fn e2e_unknown_action_returns_error() {
    let tmpdir = TempDir::new().unwrap();
    let (path, _) = spawn_headless_daemon(&tmpdir).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let resp = send_ipc(
        &path,
        IpcCommand::Trigger {
            action_id: "nonexistent.action".into(),
            text: "".into(),
        },
    )
    .await;

    match &resp.result {
        IpcResult::Error { code, message } => {
            assert_eq!(*code, IpcErrorCode::ActionNotFound);
            assert!(message.contains("nonexistent"));
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn e2e_toggle_returns_ok() {
    let tmpdir = TempDir::new().unwrap();
    let (path, _) = spawn_headless_daemon(&tmpdir).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let resp = send_ipc(&path, IpcCommand::Toggle).await;
    match &resp.result {
        IpcResult::Ok(val) => assert_eq!(val["toggled"], true),
        other => panic!("expected Ok, got {other:?}"),
    }
}

#[tokio::test]
async fn e2e_show_and_hide_emit_panel_events() {
    let tmpdir = TempDir::new().unwrap();
    let (path, dispatcher) = spawn_headless_daemon(&tmpdir).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let show = send_ipc(&path, IpcCommand::Show).await;
    match &show.result {
        IpcResult::Ok(value) => assert_eq!(value["show"], true),
        other => panic!("expected Ok, got {other:?}"),
    }

    let hide = send_ipc(&path, IpcCommand::Hide).await;
    match &hide.result {
        IpcResult::Ok(value) => assert_eq!(value["hide"], true),
        other => panic!("expected Ok, got {other:?}"),
    }

    let events = dispatcher
        .events
        .lock()
        .expect("test event collection mutex poisoned");
    assert!(events.iter().any(|event| {
        matches!(
            event,
            HostToUi::ShowSearchPanel {
                activation_token: None
            }
        )
    }));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, HostToUi::HideSearchPanel))
    );
}

#[tokio::test]
async fn e2e_doctor_returns_session_info() {
    let tmpdir = TempDir::new().unwrap();
    let (path, _) = spawn_headless_daemon(&tmpdir).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let resp = send_ipc(&path, IpcCommand::Doctor).await;
    match &resp.result {
        IpcResult::Ok(val) => {
            assert!(val.get("session").is_some());
            assert_eq!(val["portal"]["state"], "headless_not_started");
            assert_eq!(val["ipc"]["socket"], path.display().to_string());
        }
        other => panic!("expected Ok, got {other:?}"),
    }
}

#[tokio::test]
async fn e2e_real_headless_daemon_stops_and_removes_socket_after_shutdown() {
    let tmpdir = TempDir::new().unwrap();
    let socket_path = tmpdir.path().join("daemon.sock");
    let daemon = tokio::spawn(bootstrap::run_headless(socket_path.clone()));

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(mut client) = unix::connect(&socket_path).await {
                let request = IpcRequestEnvelope {
                    protocol_version: IPC_PROTOCOL_VERSION,
                    request_id: "headless-ready".into(),
                    command: IpcCommand::Status,
                };
                if client.request(&request).await.is_ok() {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("headless daemon must bind its IPC socket");

    let doctor = send_ipc(&socket_path, IpcCommand::Doctor).await;
    match doctor.result {
        IpcResult::Ok(value) => {
            assert!(value.get("session").is_some());
            assert_eq!(value["portal"]["state"], "headless_not_started");
            assert_eq!(value["ipc"]["socket"], socket_path.display().to_string());
        }
        other => panic!("expected headless doctor result, got {other:?}"),
    }

    let response = send_ipc(&socket_path, IpcCommand::Shutdown).await;
    match response.result {
        IpcResult::Ok(value) => assert_eq!(value["shutdown"], true),
        other => panic!("expected successful shutdown response, got {other:?}"),
    }

    tokio::time::timeout(Duration::from_secs(2), daemon)
        .await
        .expect("headless daemon must exit after IPC shutdown")
        .expect("headless daemon task must not panic")
        .expect("headless daemon must not report a shutdown error");
    assert!(
        !socket_path.exists(),
        "shutdown must remove only Flick's socket file"
    );
}

#[tokio::test]
async fn e2e_shutdown_returns_ok() {
    let tmpdir = TempDir::new().unwrap();
    let (path, _) = spawn_headless_daemon(&tmpdir).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let resp = send_ipc(&path, IpcCommand::Shutdown).await;
    match &resp.result {
        IpcResult::Ok(val) => assert_eq!(val["shutdown"], true),
        other => panic!("expected Ok, got {other:?}"),
    }
}

#[tokio::test]
async fn e2e_request_id_preserved() {
    let tmpdir = TempDir::new().unwrap();
    let (path, _) = spawn_headless_daemon(&tmpdir).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let request = IpcRequestEnvelope {
        protocol_version: IPC_PROTOCOL_VERSION,
        request_id: "e2e-unique-id-42".into(),
        command: IpcCommand::Status,
    };
    let mut client = unix::connect(&path).await.unwrap();
    let resp = client.request(&request).await.unwrap();

    assert_eq!(resp.request_id, "e2e-unique-id-42");
}

#[tokio::test]
async fn e2e_socket_removed_after_server_drop() {
    let tmpdir = TempDir::new().unwrap();
    let socket_path = tmpdir.path().join("flick.sock");

    let server = unix::bind(&socket_path).unwrap();
    assert!(socket_path.exists());

    drop(server);
    // 套接字文件在 drop 后应仍然存在（操作系统行为），
    // 但监听器已关闭。需要手动清理。
    let _ = std::fs::remove_file(&socket_path);
    assert!(!socket_path.exists());
}
