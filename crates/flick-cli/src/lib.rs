//! `flick-cli` library: testable CLI logic.
//!
//! Separates argument parsing, IPC communication, and output formatting
//! from the `main()` entry point so each can be tested independently.

use flick_ipc::message::*;
use flick_ipc::unix::UnixSocketClient;
use serde_json::json;
use std::path::{Path, PathBuf};

/// Default socket path if none is specified.
pub fn default_socket_path() -> PathBuf {
    let runtime_dir =
        std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime_dir).join("flick").join("flick.sock")
}

/// Build an IPC request envelope for a given command.
pub fn build_request(command: IpcCommand) -> IpcRequestEnvelope {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    IpcRequestEnvelope {
        protocol_version: IPC_PROTOCOL_VERSION,
        request_id: format!("cli-{}", COUNTER.fetch_add(1, Ordering::Relaxed)),
        command,
    }
}

/// Format an IPC result for output.
///
/// Returns `(output_string, is_stderr, exit_code)`.
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
                _ => 1,
            };
            (output, !json_output, exit_code)
        }
    }
}

/// Result of a single IPC connection attempt.
#[derive(Debug)]
pub enum ConnectResult {
    /// Successfully received a response.
    Response(IpcResponseEnvelope),
    /// Failed to connect to the daemon socket.
    ConnectError(String),
    /// Connected but failed during request/response.
    TransportError(String),
}

/// Attempt to connect to the daemon and send a request.
pub async fn try_connect_and_send(
    path: &Path,
    request: &IpcRequestEnvelope,
) -> ConnectResult {
    match UnixSocketClient::connect(path).await {
        Ok(mut client) => match client.request(request).await {
            Ok(response) => ConnectResult::Response(response),
            Err(e) => ConnectResult::TransportError(format!("{e}")),
        },
        Err(e) => ConnectResult::ConnectError(format!("{e}")),
    }
}

/// Format a connection/transport error for output.
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

/// Format a transport error for output.
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
        (
            format!("Error communicating with daemon: {err_msg}"),
            true,
        )
    }
}
