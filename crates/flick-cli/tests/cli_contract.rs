//! CLI 契约测试。
//!
//! 验证 CLI 是否：
//! - 为每个子命令构建正确的 `IpcCommand` 变体
//! - 正确格式化结果（JSON 和人类可读格式）
//! - 正确处理守护进程不可用的情况
//! - 向真实 IPC 服务器发送预期命令（往返）

use flick_cli::*;
use flick_ipc::message::*;
use flick_ipc::server::{IpcDispatcher, IpcServer};
use flick_ipc::unix;
use std::sync::Arc;
use tempfile::TempDir;

// ─── 测试调度器 ─────────────────────────────────────────────────────────

/// 记录收到的命令并返回可配置
/// 结果的测试调度器。
struct TestDispatcher {
    /// 收到的命令记录。
    received: std::sync::Mutex<Vec<IpcCommand>>,
    /// 预先配置的结果。
    result: IpcResult,
}

impl TestDispatcher {
    fn new(result: IpcResult) -> Self {
        Self {
            received: std::sync::Mutex::new(Vec::new()),
            result,
        }
    }

    fn received_commands(&self) -> Vec<IpcCommand> {
        self.received.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl IpcDispatcher for TestDispatcher {
    async fn dispatch(&self, command: IpcCommand) -> IpcResult {
        self.received.lock().unwrap().push(command);
        self.result.clone()
    }
}

// ─── 辅助函数 ─────────────────────────────────────────────────────────────────

/// 启动测试 IPC 服务器并返回套接字路径 + 调度器句柄。
async fn spawn_test_server(
    tmpdir: &TempDir,
    result: IpcResult,
) -> (std::path::PathBuf, Arc<TestDispatcher>) {
    let path = tmpdir.path().join("flick.sock");
    let dispatcher = Arc::new(TestDispatcher::new(result));
    let server = unix::bind(&path).expect("bind failed");
    let ipc_server = IpcServer::new(server, dispatcher.clone());
    tokio::spawn(async move {
        let _ = ipc_server
            .run(tokio_util::sync::CancellationToken::new())
            .await;
    });
    (path, dispatcher)
}

// ─── 单元测试：套接字解析 ──────────────────────────────────────────

#[test]
fn default_socket_policy_requires_runtime_directory() {
    assert!(matches!(
        resolve_default_socket_path(|_| None),
        Err(SocketPathError::MissingRuntimeDir)
    ));
}

#[test]
fn default_socket_policy_uses_runtime_directory() {
    assert_eq!(
        resolve_default_socket_path(|key| {
            (key == "XDG_RUNTIME_DIR").then(|| "/run/user/1000".into())
        })
        .unwrap(),
        std::path::PathBuf::from("/run/user/1000/flick/flick.sock")
    );
}

// ─── 单元测试：build_request ───────────────────────────────────────────────

#[test]
fn build_request_toggle_has_correct_command() {
    let req = build_request(IpcCommand::Toggle);
    assert_eq!(req.protocol_version, IPC_PROTOCOL_VERSION);
    assert!(!req.request_id.is_empty());
    assert!(matches!(req.command, IpcCommand::Toggle));
}

#[test]
fn build_request_trigger_has_action_and_text() {
    let req = build_request(IpcCommand::Trigger {
        action_id: "flick.builtin.echo.v1.echo".into(),
        text: "hello".into(),
    });
    match &req.command {
        IpcCommand::Trigger { action_id, text } => {
            assert_eq!(action_id, "flick.builtin.echo.v1.echo");
            assert_eq!(text, "hello");
        }
        other => panic!("expected Trigger, got {other:?}"),
    }
}

#[test]
fn build_request_status_has_correct_command() {
    let req = build_request(IpcCommand::Status);
    assert!(matches!(req.command, IpcCommand::Status));
}

#[test]
fn build_request_doctor_has_correct_command() {
    let req = build_request(IpcCommand::Doctor);
    assert!(matches!(req.command, IpcCommand::Doctor));
}

#[test]
fn build_request_shortcuts_configure() {
    let req = build_request(IpcCommand::ShortcutsConfigure {
        preferred_trigger: "Super+Space".into(),
    });

    match req.command {
        IpcCommand::ShortcutsConfigure { preferred_trigger } => {
            assert_eq!(preferred_trigger, "Super+Space");
        }
        other => panic!("expected ShortcutsConfigure, got {other:?}"),
    }
}

#[test]
fn build_request_shortcuts_reconnect() {
    let req = build_request(IpcCommand::ShortcutsReconnect);
    assert!(matches!(req.command, IpcCommand::ShortcutsReconnect));
}

#[test]
fn build_request_shutdown() {
    let req = build_request(IpcCommand::Shutdown);
    assert!(matches!(req.command, IpcCommand::Shutdown));
}

#[test]
fn build_request_ids_are_unique() {
    let a = build_request(IpcCommand::Status);
    let b = build_request(IpcCommand::Status);
    assert_ne!(a.request_id, b.request_id);
}

// ─── 单元测试：format_result ───────────────────────────────────────────────

#[test]
fn format_result_ok_human_readable() {
    let result = IpcResult::Ok(serde_json::json!({"status": "running"}));
    let (output, is_stderr, exit_code) = format_result(&result, false);
    assert!(!is_stderr);
    assert_eq!(exit_code, 0);
    assert!(output.contains("running"));
}

#[test]
fn format_result_ok_json() {
    let result = IpcResult::Ok(serde_json::json!({"echo": "hello"}));
    let (output, is_stderr, exit_code) = format_result(&result, true);
    assert!(!is_stderr);
    assert_eq!(exit_code, 0);
    // 应为有效 JSON
    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["ok"]["echo"], "hello");
}

#[test]
fn format_result_error_human_readable() {
    let result = IpcResult::Error {
        code: IpcErrorCode::ActionNotFound,
        message: "Unknown action: test".into(),
    };
    let (output, is_stderr, exit_code) = format_result(&result, false);
    assert!(is_stderr, "human-readable errors go to stderr");
    assert_eq!(exit_code, 1);
    assert!(output.contains("Unknown action: test"));
}

#[test]
fn format_result_error_json() {
    let result = IpcResult::Error {
        code: IpcErrorCode::ActionNotFound,
        message: "Unknown action: test".into(),
    };
    let (output, is_stderr, exit_code) = format_result(&result, true);
    assert!(!is_stderr, "JSON errors go to stdout");
    assert_eq!(exit_code, 1);
    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["error"]["code"], "action_not_found");
    assert_eq!(parsed["error"]["message"], "Unknown action: test");
}

#[test]
fn format_result_daemon_unavailable_exit_code_2() {
    let result = IpcResult::Error {
        code: IpcErrorCode::DaemonUnavailable,
        message: "not running".into(),
    };
    let (_, _, exit_code) = format_result(&result, false);
    assert_eq!(exit_code, 2);
}

#[test]
fn format_result_protocol_mismatch_exit_code_3() {
    let result = IpcResult::Error {
        code: IpcErrorCode::ProtocolVersionMismatch,
        message: "version mismatch".into(),
    };
    let (_, _, exit_code) = format_result(&result, false);
    assert_eq!(exit_code, 3);
}

#[test]
fn format_result_desktop_unavailable_is_a_nonzero_error() {
    let result = IpcResult::Error {
        code: IpcErrorCode::DesktopUnavailable,
        message: "Portal session is unavailable".into(),
    };
    let (output, is_stderr, exit_code) = format_result(&result, true);
    assert!(!is_stderr);
    assert_eq!(exit_code, 1);
    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["error"]["code"], "desktop_unavailable");
}

// ─── 单元测试：连接错误格式化 ─────────────────────────────────

#[test]
fn format_connection_error_human_readable() {
    let (output, is_stderr) = format_connection_error("connection refused", false);
    assert!(is_stderr);
    assert!(output.contains("Cannot connect to daemon"));
    assert!(output.contains("connection refused"));
}

#[test]
fn format_connection_error_json() {
    let (output, is_stderr) = format_connection_error("connection refused", true);
    assert!(!is_stderr);
    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["error"]["code"], "DAEMON_UNAVAILABLE");
}

#[test]
fn format_transport_error_human_readable() {
    let (output, is_stderr) = format_transport_error("broken pipe", false);
    assert!(is_stderr);
    assert!(output.contains("Error communicating with daemon"));
}

#[test]
fn format_transport_error_json() {
    let (output, is_stderr) = format_transport_error("broken pipe", true);
    assert!(!is_stderr);
    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["error"]["code"], "INTERNAL");
}

// ─── 集成测试：与测试 IPC 服务器的往返 ──────────────────────

#[tokio::test]
async fn toggle_sends_correct_command_to_server() {
    let tmpdir = TempDir::new().unwrap();
    let (path, dispatcher) =
        spawn_test_server(&tmpdir, IpcResult::Ok(serde_json::json!({"toggled": true}))).await;

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let request = build_request(IpcCommand::Toggle);
    let result = try_connect_and_send(&path, &request).await;

    match result {
        ConnectResult::Response(resp) => {
            assert_eq!(resp.request_id, request.request_id);
            match &resp.result {
                IpcResult::Ok(val) => assert_eq!(val["toggled"], true),
                other => panic!("expected Ok, got {other:?}"),
            }
        }
        other => panic!("expected Response, got {other:?}"),
    }

    // 验证服务器收到了正确的命令
    let cmds = dispatcher.received_commands();
    assert_eq!(cmds.len(), 1);
    assert!(matches!(cmds[0], IpcCommand::Toggle));
}

#[tokio::test]
async fn trigger_echo_sends_correct_command_to_server() {
    let tmpdir = TempDir::new().unwrap();
    let (path, dispatcher) =
        spawn_test_server(&tmpdir, IpcResult::Ok(serde_json::json!({"echo": "hello"}))).await;

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let request = build_request(IpcCommand::Trigger {
        action_id: "flick.builtin.echo.v1.echo".into(),
        text: "hello".into(),
    });
    let result = try_connect_and_send(&path, &request).await;

    match result {
        ConnectResult::Response(resp) => match &resp.result {
            IpcResult::Ok(val) => assert_eq!(val["echo"], "hello"),
            other => panic!("expected Ok, got {other:?}"),
        },
        other => panic!("expected Response, got {other:?}"),
    }

    let cmds = dispatcher.received_commands();
    assert_eq!(cmds.len(), 1);
    match &cmds[0] {
        IpcCommand::Trigger { action_id, text } => {
            assert_eq!(action_id, "flick.builtin.echo.v1.echo");
            assert_eq!(text, "hello");
        }
        other => panic!("expected Trigger, got {other:?}"),
    }
}

#[tokio::test]
async fn unknown_action_returns_structured_error() {
    let tmpdir = TempDir::new().unwrap();
    let (path, _) = spawn_test_server(
        &tmpdir,
        IpcResult::Error {
            code: IpcErrorCode::ActionNotFound,
            message: "Unknown action: nonexistent".into(),
        },
    )
    .await;

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let request = build_request(IpcCommand::Trigger {
        action_id: "nonexistent".into(),
        text: "".into(),
    });
    let result = try_connect_and_send(&path, &request).await;

    match result {
        ConnectResult::Response(resp) => match &resp.result {
            IpcResult::Error { code, message } => {
                assert_eq!(*code, IpcErrorCode::ActionNotFound);
                assert!(message.contains("nonexistent"));
            }
            other => panic!("expected Error, got {other:?}"),
        },
        other => panic!("expected Response, got {other:?}"),
    }
}

#[tokio::test]
async fn status_returns_daemon_info() {
    let tmpdir = TempDir::new().unwrap();
    let (path, _) = spawn_test_server(
        &tmpdir,
        IpcResult::Ok(serde_json::json!({"daemon_status": "running"})),
    )
    .await;

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let request = build_request(IpcCommand::Status);
    let result = try_connect_and_send(&path, &request).await;

    match result {
        ConnectResult::Response(resp) => match &resp.result {
            IpcResult::Ok(val) => {
                assert_eq!(val["daemon_status"], "running");
            }
            other => panic!("expected Ok, got {other:?}"),
        },
        other => panic!("expected Response, got {other:?}"),
    }
}

#[tokio::test]
async fn daemon_unreachable_returns_connect_error() {
    let tmpdir = TempDir::new().unwrap();
    let nonexistent_path = tmpdir.path().join("nonexistent").join("flick.sock");

    let request = build_request(IpcCommand::Status);
    let result = try_connect_and_send(&nonexistent_path, &request).await;

    match result {
        ConnectResult::ConnectError(e) => {
            assert!(!e.is_empty());
        }
        other => panic!("expected ConnectError, got {other:?}"),
    }
}

#[tokio::test]
async fn request_id_preserved_through_round_trip() {
    let tmpdir = TempDir::new().unwrap();
    let (path, _) =
        spawn_test_server(&tmpdir, IpcResult::Ok(serde_json::json!({"ok": true}))).await;

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let request = build_request(IpcCommand::Doctor);
    let saved_id = request.request_id.clone();
    let result = try_connect_and_send(&path, &request).await;

    match result {
        ConnectResult::Response(resp) => {
            assert_eq!(resp.request_id, saved_id);
        }
        other => panic!("expected Response, got {other:?}"),
    }
}

#[tokio::test]
async fn shortcuts_configure_sends_correct_command() {
    let tmpdir = TempDir::new().unwrap();
    let (path, dispatcher) = spawn_test_server(
        &tmpdir,
        IpcResult::Ok(serde_json::json!({"configure": "requested"})),
    )
    .await;

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let request = build_request(IpcCommand::ShortcutsConfigure {
        preferred_trigger: "Super+Space".into(),
    });
    let result = try_connect_and_send(&path, &request).await;

    match result {
        ConnectResult::Response(_) => {}
        other => panic!("expected Response, got {other:?}"),
    }

    let cmds = dispatcher.received_commands();
    match &cmds[0] {
        IpcCommand::ShortcutsConfigure { preferred_trigger } => {
            assert_eq!(preferred_trigger, "Super+Space");
        }
        other => panic!("expected ShortcutsConfigure, got {other:?}"),
    }
}

#[tokio::test]
async fn shutdown_sends_correct_command() {
    let tmpdir = TempDir::new().unwrap();
    let (path, dispatcher) = spawn_test_server(
        &tmpdir,
        IpcResult::Ok(serde_json::json!({"shutdown": true})),
    )
    .await;

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let request = build_request(IpcCommand::Shutdown);
    let result = try_connect_and_send(&path, &request).await;

    match result {
        ConnectResult::Response(_) => {}
        other => panic!("expected Response, got {other:?}"),
    }

    let cmds = dispatcher.received_commands();
    assert!(matches!(cmds[0], IpcCommand::Shutdown));
}
