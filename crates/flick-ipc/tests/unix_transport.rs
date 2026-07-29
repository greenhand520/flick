//! Integration tests for the Unix socket IPC transport.

use async_trait::async_trait;
use flick_ipc::message::{
    IPC_PROTOCOL_VERSION, IpcCommand, IpcErrorCode, IpcRequestEnvelope, IpcResult,
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
            IpcCommand::Trigger { action_id, text } => IpcResult::ok(serde_json::json!({
                "action_id": action_id,
                "text": text,
            })),
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
        let _ = ipc_server
            .run(tokio_util::sync::CancellationToken::new())
            .await;
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
        let _ = ipc_server
            .run(tokio_util::sync::CancellationToken::new())
            .await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut client = unix::connect(&path).await.expect("connect failed");
    let request = make_request("req-stale", IpcCommand::Status, IPC_PROTOCOL_VERSION);
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

struct BlockingDispatcher {
    started: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

#[async_trait]
impl IpcDispatcher for BlockingDispatcher {
    async fn dispatch(&self, _command: IpcCommand) -> IpcResult {
        if let Some(started) = self.started.lock().unwrap().take() {
            let _ = started.send(());
        }
        std::future::pending::<IpcResult>().await
    }
}

#[tokio::test]
async fn slow_client_does_not_block_healthy_client_and_is_timed_out() {
    use tokio::io::AsyncReadExt;
    use tokio::net::UnixStream;
    use tokio::time::{Duration, timeout};
    use tokio_util::sync::CancellationToken;

    let tmpdir = TempDir::new().unwrap();
    let path = tmpdir.path().join("flick.sock");
    let server = unix::bind(&path).expect("bind failed");
    let cancel = CancellationToken::new();
    let server_task =
        tokio::spawn(IpcServer::new(server, Arc::new(EchoDispatcher)).run(cancel.clone()));

    let mut slow = UnixStream::connect(&path)
        .await
        .expect("slow connect failed");
    slow.writable().await.expect("slow client not writable");
    assert_eq!(
        slow.try_write(&128_u32.to_be_bytes())
            .expect("slow header write failed"),
        4
    );

    let mut normal_client = unix::UnixSocketClient::connect(&path)
        .await
        .expect("healthy connect failed");
    let response = timeout(
        Duration::from_millis(500),
        normal_client.request(&make_request(
            "healthy-client",
            IpcCommand::Status,
            IPC_PROTOCOL_VERSION,
        )),
    )
    .await
    .expect("healthy client was blocked by slow client")
    .expect("healthy request failed");
    assert_eq!(response.request_id, "healthy-client");

    let mut eof = [0_u8; 1];
    let read = timeout(
        unix::IPC_REQUEST_TIMEOUT + Duration::from_secs(1),
        slow.read(&mut eof),
    )
    .await
    .expect("slow connection was not dropped after the request timeout")
    .expect("slow connection read failed");
    assert_eq!(read, 0, "server should close the timed-out slow connection");

    cancel.cancel();
    let result = timeout(Duration::from_secs(1), server_task)
        .await
        .expect("server did not stop after cancellation")
        .expect("server task panicked");
    assert!(result.is_ok(), "server shutdown failed: {result:?}");
}

#[tokio::test]
async fn cancellation_interrupts_an_in_flight_dispatch_and_awaits_client_tasks() {
    use tokio::time::{Duration, timeout};
    use tokio_util::sync::CancellationToken;

    let tmpdir = TempDir::new().unwrap();
    let path = tmpdir.path().join("flick.sock");
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let dispatcher = Arc::new(BlockingDispatcher {
        started: std::sync::Mutex::new(Some(started_tx)),
    });
    let server = unix::bind(&path).expect("bind failed");
    let cancel = CancellationToken::new();
    let server_task = tokio::spawn(IpcServer::new(server, dispatcher).run(cancel.clone()));

    let client_path = path.clone();
    let client_task = tokio::spawn(async move {
        let mut client = unix::UnixSocketClient::connect(&client_path).await?;
        client
            .request(&make_request(
                "cancel-dispatch",
                IpcCommand::Status,
                IPC_PROTOCOL_VERSION,
            ))
            .await
    });

    timeout(Duration::from_millis(500), started_rx)
        .await
        .expect("dispatcher was never called")
        .expect("dispatcher start signal was dropped");

    cancel.cancel();
    let result = timeout(Duration::from_millis(500), server_task)
        .await
        .expect("server did not await the cancelled client task")
        .expect("server task panicked");
    assert!(result.is_ok(), "server shutdown failed: {result:?}");

    let client_result = timeout(Duration::from_millis(500), client_task)
        .await
        .expect("client did not observe the closed connection")
        .expect("client task panicked");
    assert!(
        client_result.is_err(),
        "cancelled dispatch must not receive a response"
    );
}
