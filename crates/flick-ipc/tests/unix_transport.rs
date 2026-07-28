//! Integration tests for the Unix socket IPC transport.

use async_trait::async_trait;
use flick_ipc::message::{
    IpcCommand, IpcErrorCode, IpcRequestEnvelope, IpcResult,
    IPC_PROTOCOL_VERSION,
};
use flick_ipc::server::{IpcDispatcher, IpcServer};
use flick_ipc::unix;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// A trivial dispatcher that echoes trigger payloads and returns Ok for
/// everything else.
struct EchoDispatcher;

#[async_trait]
impl IpcDispatcher for EchoDispatcher {
    async fn dispatch(&self, command: IpcCommand) -> IpcResult {
        match command {
            IpcCommand::Trigger { action_id, text } => {
                IpcResult::ok(serde_json::json!({
                    "action_id": action_id,
                    "text": text,
                }))
            }
            _ => IpcResult::ok(serde_json::json!({"status": "ok"})),
        }
    }
}

/// Spawn an IPC server in the background and return the socket path.
fn spawn_server(tmpdir: &TempDir) -> PathBuf {
    let path = tmpdir.path().join("flick.sock");
    let server = unix::bind(&path).expect("bind failed");
    let ipc_server = IpcServer::new(server, Arc::new(EchoDispatcher));
    tokio::spawn(async move {
        // run() loops forever; the test will drop the socket path when done.
        let _ = ipc_server.run().await;
    });
    path
}

fn make_request(request_id: &str, command: IpcCommand, version: u16) -> IpcRequestEnvelope {
    IpcRequestEnvelope {
        protocol_version: version,
        request_id: request_id.to_owned(),
        command,
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn response_keeps_request_id_for_trigger() {
    let tmpdir = TempDir::new().unwrap();
    let path = spawn_server(&tmpdir);

    // Give the server a moment to start listening.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut client = unix::connect(&path).await.expect("connect failed");

    let request = make_request(
        "req-abc-123",
        IpcCommand::Trigger {
            action_id: "test.action".into(),
            text: "hello".into(),
        },
        IPC_PROTOCOL_VERSION,
    );

    let response = client.request(&request).await.expect("request failed");

    assert_eq!(response.request_id, "req-abc-123");
    assert_eq!(response.protocol_version, IPC_PROTOCOL_VERSION);

    match &response.result {
        IpcResult::Ok(val) => {
            assert_eq!(val["action_id"], "test.action");
            assert_eq!(val["text"], "hello");
        }
        other => panic!("expected Ok, got {other:?}"),
    }
}

#[tokio::test]
async fn stale_socket_is_removed_only_after_single_instance_probe_fails() {
    let tmpdir = TempDir::new().unwrap();
    let path = tmpdir.path().join("flick.sock");

    // Create a "stale" socket: bind a listener then drop it.
    // The socket file remains on disk, but nothing is listening.
    {
        let _listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        // listener is dropped here — socket file is now stale.
    }

    // The socket file should exist (stale).
    assert!(path.exists(), "stale socket file should exist");

    // bind() should detect no listener, remove the stale file, and succeed.
    let server = unix::bind(&path).expect("bind should succeed after removing stale socket");
    assert!(path.exists(), "fresh socket file should exist after bind");

    // Verify the server actually works.
    let ipc_server = IpcServer::new(server, Arc::new(EchoDispatcher));
    tokio::spawn(async move {
        let _ = ipc_server.run().await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut client = unix::connect(&path).await.expect("connect failed");
    let request = make_request(
        "req-stale",
        IpcCommand::Status,
        IPC_PROTOCOL_VERSION,
    );
    let response = client.request(&request).await.expect("request failed");
    assert_eq!(response.request_id, "req-stale");
}

#[tokio::test]
async fn server_rejects_unknown_protocol_version() {
    let tmpdir = TempDir::new().unwrap();
    let path = spawn_server(&tmpdir);

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut client = unix::connect(&path).await.expect("connect failed");

    // Send a request with a wrong protocol version.
    let request = make_request(
        "req-bad-ver",
        IpcCommand::Status,
        99, // wrong version
    );

    let response = client.request(&request).await.expect("request failed");

    assert_eq!(response.request_id, "req-bad-ver");
    assert_eq!(response.protocol_version, IPC_PROTOCOL_VERSION);

    match &response.result {
        IpcResult::Error { code, message } => {
            assert_eq!(*code, IpcErrorCode::ProtocolVersionMismatch);
            assert!(
                message.contains("99"),
                "error message should mention the client version; got: {message}"
            );
        }
        other => panic!("expected Error with ProtocolVersionMismatch, got {other:?}"),
    }
}
