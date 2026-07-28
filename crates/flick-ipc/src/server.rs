//! IPC request dispatcher and server loop.

use crate::message::{
    IpcCommand, IpcErrorCode, IpcRequestEnvelope, IpcResponseEnvelope, IpcResult,
    IPC_PROTOCOL_VERSION,
};
use crate::unix::{self, UnixSocketServer};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::net::UnixStream;
use tracing::{debug, warn};

/// Trait that the application layer implements to handle IPC commands.
///
/// Each variant of [`IpcCommand`] is dispatched through this trait.
#[async_trait]
pub trait IpcDispatcher: Send + Sync + 'static {
    /// Dispatch a single IPC command and return the result.
    async fn dispatch(&self, command: IpcCommand) -> IpcResult;
}

/// The IPC server: owns a [`UnixSocketServer`] and a dispatcher, and runs
/// the accept loop.
pub struct IpcServer {
    server: UnixSocketServer,
    dispatcher: Arc<dyn IpcDispatcher>,
}

impl IpcServer {
    /// Create a new IPC server from a bound socket and a dispatcher.
    pub fn new(server: UnixSocketServer, dispatcher: Arc<dyn IpcDispatcher>) -> Self {
        Self { server, dispatcher }
    }

    /// Run the accept loop.  This method does **not** return under normal
    /// operation; it processes one client at a time (Phase 0).
    pub async fn run(self) -> Result<(), std::io::Error> {
        loop {
            let stream = self.server.accept().await.map_err(|e| match e {
                unix::TransportError::Io(io) => io,
                other => std::io::Error::other(other.to_string()),
            })?;

            let dispatcher = Arc::clone(&self.dispatcher);
            // Phase 0: handle one client at a time inline.
            // A later phase will spawn per-connection tasks.
            Self::handle_client(stream, dispatcher).await;
        }
    }

    /// Handle a single client connection: read request → dispatch → write
    /// response.  The connection is closed after one round-trip (Phase 0).
    async fn handle_client(
        mut stream: UnixStream,
        dispatcher: Arc<dyn IpcDispatcher>,
    ) {
        let request = match unix::read_request(&mut stream).await {
            Ok(r) => r,
            Err(e) => {
                warn!("IPC: failed to read request: {e}");
                return;
            }
        };

        let response = Self::process_request(request, dispatcher).await;

        if let Err(e) = unix::write_response(&mut stream, &response).await {
            warn!("IPC: failed to write response: {e}");
        }
    }

    /// Validate and dispatch a single request.
    async fn process_request(
        request: IpcRequestEnvelope,
        dispatcher: Arc<dyn IpcDispatcher>,
    ) -> IpcResponseEnvelope {
        let request_id = request.request_id.clone();

        // Protocol version check.
        if request.protocol_version != IPC_PROTOCOL_VERSION {
            debug!(
                "IPC: version mismatch: client={}, server={}",
                request.protocol_version, IPC_PROTOCOL_VERSION,
            );
            return IpcResponseEnvelope {
                protocol_version: IPC_PROTOCOL_VERSION,
                request_id,
                result: IpcResult::error(
                    IpcErrorCode::ProtocolVersionMismatch,
                    format!(
                        "unsupported protocol version {} (server expects {})",
                        request.protocol_version, IPC_PROTOCOL_VERSION,
                    ),
                ),
            };
        }

        let result = dispatcher.dispatch(request.command).await;

        IpcResponseEnvelope {
            protocol_version: IPC_PROTOCOL_VERSION,
            request_id,
            result,
        }
    }
}
