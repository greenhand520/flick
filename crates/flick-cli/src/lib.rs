//! `flick-cli` 库：可测试的 CLI 逻辑。
//!
//! 将参数解析、IPC 通信和输出格式化
//! 与 `main()` 入口点分离，以便各自独立测试。

use flick_ipc::message::*;
use flick_ipc::unix::UnixSocketClient;
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SocketPathError {
    #[error("XDG_RUNTIME_DIR is required; pass --socket for an explicit path")]
    MissingRuntimeDir,
}
/// 从注入的环境查询中解析 Flick 的默认套接字。
///
/// 这有意与守护进程的策略保持一致：未显式指定
/// `--socket` 时，`XDG_RUNTIME_DIR` 必须存在且非空。
pub fn resolve_default_socket_path<F>(get_env: F) -> Result<PathBuf, SocketPathError>
where
    F: FnOnce(&str) -> Option<String>,
{
    let runtime_dir = get_env("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .ok_or(SocketPathError::MissingRuntimeDir)?;
    Ok(PathBuf::from(runtime_dir).join("flick").join("flick.sock"))
}

/// 未指定时的默认套接字路径。
pub fn default_socket_path() -> Result<PathBuf, SocketPathError> {
    resolve_default_socket_path(|key| std::env::var(key).ok())
}

/// 为给定命令构建 IPC 请求信封。
pub fn build_request(command: IpcCommand) -> IpcRequestEnvelope {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    IpcRequestEnvelope {
        protocol_version: IPC_PROTOCOL_VERSION,
        request_id: format!("cli-{}", COUNTER.fetch_add(1, Ordering::Relaxed)),
        command,
    }
}

/// 格式化 IPC 结果以供输出。
///
/// 返回 `(output_string, is_stderr, exit_code)`。
pub fn format_result(result: &IpcResult, json_output: bool) -> (String, bool, i32) {
    match result {
        IpcResult::Ok(value) => {
            let output = if json_output {
                serde_json::to_string(&json!({ "ok": value })).unwrap()
            } else {
                format!("{value}")
            };
            (output, false, 0)
        }
        IpcResult::Error { code, message } => {
            let output = if json_output {
                serde_json::to_string(&json!({
                    "error": { "code": code.as_str(), "message": message }
                }))
                .unwrap()
            } else {
                format!("Error: {message}")
            };
            let exit_code = match code {
                IpcErrorCode::DaemonUnavailable => 2,
                IpcErrorCode::ProtocolVersionMismatch => 3,
                IpcErrorCode::DesktopUnavailable => 1,
                _ => 1,
            };
            (output, !json_output, exit_code)
        }
    }
}

/// 单次 IPC 连接尝试的结果。
#[derive(Debug)]
pub enum ConnectResult {
    /// 成功收到响应。
    Response(IpcResponseEnvelope),
    /// 连接守护进程套接字失败。
    ConnectError(String),
    /// 已连接但请求/响应期间失败。
    TransportError(String),
}

/// 尝试连接守护进程并发送请求。
pub async fn try_connect_and_send(path: &Path, request: &IpcRequestEnvelope) -> ConnectResult {
    match UnixSocketClient::connect(path).await {
        Ok(mut client) => match client.request(request).await {
            Ok(response) => ConnectResult::Response(response),
            Err(e) => ConnectResult::TransportError(format!("{e}")),
        },
        Err(e) => ConnectResult::ConnectError(format!("{e}")),
    }
}

/// 格式化连接/传输错误以供输出。
pub fn format_connection_error(err_msg: &str, json_output: bool) -> (String, bool) {
    if json_output {
        (
            serde_json::to_string(&json!({
                "error": { "code": "DAEMON_UNAVAILABLE", "message": err_msg }
            }))
            .unwrap(),
            false,
        )
    } else {
        (format!("Cannot connect to daemon: {err_msg}"), true)
    }
}

/// 格式化传输错误以供输出。
pub fn format_transport_error(err_msg: &str, json_output: bool) -> (String, bool) {
    if json_output {
        (
            serde_json::to_string(&json!({
                "error": { "code": "INTERNAL", "message": err_msg }
            }))
            .unwrap(),
            false,
        )
    } else {
        (format!("Error communicating with daemon: {err_msg}"), true)
    }
}
