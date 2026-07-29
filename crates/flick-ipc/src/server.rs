//! IPC request dispatcher and server loop.

use crate::message::{
    IPC_PROTOCOL_VERSION, IpcCommand, IpcErrorCode, IpcRequestEnvelope, IpcResponseEnvelope,
    IpcResult,
};
use crate::unix::{self, IPC_REQUEST_TIMEOUT, TransportError, UnixSocketServer};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::net::UnixStream;
use tokio::task::JoinSet;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
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

    /// Run the accept loop until `cancel` is cancelled.
    ///
    /// Every accepted connection is handled by a supervised task. Cancellation
    /// stops accepting new connections, interrupts in-flight reads and dispatch
    /// calls, then waits for every client task before returning.
    pub async fn run(self, cancel: CancellationToken) -> Result<(), TransportError> {
        let mut clients = JoinSet::new();
        let client_cancel = cancel.child_token();
        let mut accept_error = None;

        loop {
            tokio::select! {
                biased;

                _ = cancel.cancelled() => break,
                accepted = self.server.accept() => match accepted {
                    Ok(stream) => {
                        let dispatcher = Arc::clone(&self.dispatcher);
                        let client_cancel = client_cancel.clone();
                        clients.spawn(async move {
                            if let Err(error) = Self::handle_client(stream, dispatcher, client_cancel).await {
                                warn!(%error, "IPC client request failed");
                            }
                        });
                    }
                    Err(error) => {
                        accept_error = Some(error);
                        break;
                    }
                },
                Some(joined) = clients.join_next(), if !clients.is_empty() => {
                    if let Err(error) = joined {
                        warn!(%error, "IPC client task panicked");
                    }
                }
            }
        }

        // Also release client work if accepting fails independently of daemon
        // cancellation, before waiting for every supervised task to finish.
        client_cancel.cancel();
        while let Some(joined) = clients.join_next().await {
            if let Err(error) = joined {
                warn!(%error, "IPC client task panicked during shutdown");
            }
        }

        match accept_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Handle a single client connection: read request → dispatch → write
    /// response. The connection is closed after one round-trip (Phase 0).
    async fn handle_client(
        mut stream: UnixStream,
        dispatcher: Arc<dyn IpcDispatcher>,
        cancel: CancellationToken,
    ) -> Result<(), TransportError> {
        let request = tokio::select! {
            biased;

            _ = cancel.cancelled() => return Ok(()),
            result = timeout(IPC_REQUEST_TIMEOUT, unix::read_request(&mut stream)) => {
                result.map_err(|_| TransportError::TimedOut {
                    operation: "read request",
                })??
            }
        };

        let Some(response) = Self::process_request(request, dispatcher, &cancel).await? else {
            return Ok(());
        };

        tokio::select! {
            biased;

            _ = cancel.cancelled() => Ok(()),
            result = timeout(IPC_REQUEST_TIMEOUT, unix::write_response(&mut stream, &response)) => {
                result.map_err(|_| TransportError::TimedOut {
                    operation: "write response",
                })?
            }
        }
    }

    /// Validate and dispatch a single request.
    ///
    /// `None` means daemon shutdown cancelled the in-flight dispatch.
    async fn process_request(
        request: IpcRequestEnvelope,
        dispatcher: Arc<dyn IpcDispatcher>,
        cancel: &CancellationToken,
    ) -> Result<Option<IpcResponseEnvelope>, TransportError> {
        let request_id = request.request_id.clone();

        // Protocol version check.
        if request.protocol_version != IPC_PROTOCOL_VERSION {
            debug!(
                "IPC: version mismatch: client={}, server={}",
                request.protocol_version, IPC_PROTOCOL_VERSION,
            );
            return Ok(Some(IpcResponseEnvelope {
                protocol_version: IPC_PROTOCOL_VERSION,
                request_id,
                result: IpcResult::error(
                    IpcErrorCode::ProtocolVersionMismatch,
                    format!(
                        "unsupported protocol version {} (server expects {})",
                        request.protocol_version, IPC_PROTOCOL_VERSION,
                    ),
                ),
            }));
        }

        let result = tokio::select! {
            biased;

            _ = cancel.cancelled() => return Ok(None),
            result = timeout(IPC_REQUEST_TIMEOUT, dispatcher.dispatch(request.command)) => {
                result.map_err(|_| TransportError::TimedOut {
                    operation: "dispatch request",
                })?
            }
        };

        Ok(Some(IpcResponseEnvelope {
            protocol_version: IPC_PROTOCOL_VERSION,
            request_id,
            result,
        }))
    }
}
