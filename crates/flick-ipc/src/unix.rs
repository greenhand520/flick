//! Unix domain socket transport for Flick IPC.

use crate::codec::{self, CodecError};
use crate::message::{IpcRequestEnvelope, IpcResponseEnvelope};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

/// Maximum payload size in bytes (mirrors [`codec::MAX_FRAME_SIZE`]).
const MAX_FRAME_SIZE: usize = codec::MAX_FRAME_SIZE as usize;

// ─── Errors ───────────────────────────────────────────────────────────────────

/// Errors specific to the Unix socket transport layer.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("socket I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("codec error: {0}")]
    Codec(#[from] CodecError),

    #[error("another instance is already listening on this socket")]
    AlreadyRunning,
}

// ─── Server ───────────────────────────────────────────────────────────────────

/// A Unix domain socket server that accepts a single IPC client at a time.
///
/// Use [`bind`] to create.  Binding performs a single-instance probe:
/// if a connection to the path succeeds, another daemon is already running
/// and [`TransportError::AlreadyRunning`] is returned.  If the connection
/// fails the (stale) socket file is removed before binding.
pub struct UnixSocketServer {
    listener: UnixListener,
}

/// Bind a new Unix socket server at `path`.
///
/// **Single-instance protocol:**
/// 1. Try to connect to `path`.
///    - Success → another instance is already listening → return `AlreadyRunning`.
///    - Failure → the socket is stale (no listener) → proceed.
/// 2. Remove the stale socket file.
/// 3. Bind a fresh listener.
pub fn bind(path: &Path) -> Result<UnixSocketServer, TransportError> {
    // Step 1: probe for existing instance.
    match std::os::unix::net::UnixStream::connect(path) {
        Ok(_stream) => {
            // Another daemon is listening.
            return Err(TransportError::AlreadyRunning);
        }
        Err(_) => {
            // Connection failed — socket is stale or absent.
        }
    }

    // Step 2: remove stale socket file if present.
    let _ = std::fs::remove_file(path);

    // Step 3: create parent directory with user-only permissions.
    if let Some(parent) = path.parent() {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(parent)
            .map_err(TransportError::Io)?;
    }

    // Step 4: bind.
    let listener = UnixListener::bind(path)?;

    // Step 5: set socket permissions to 0600 (user read/write only).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(TransportError::Io)?;
    }

    Ok(UnixSocketServer { listener })
}

impl UnixSocketServer {
    /// Bind a new Unix socket server at `path`.
    ///
    /// Convenience wrapper around the free [`bind`] function.
    pub fn bind(path: &Path) -> Result<UnixSocketServer, TransportError> {
        bind(path)
    }

    /// Accept the next client connection.
    pub async fn accept(&self) -> Result<UnixStream, TransportError> {
        let (stream, _) = self.listener.accept().await?;
        Ok(stream)
    }
}

// ─── Client ───────────────────────────────────────────────────────────────────

/// A Unix domain socket client that sends requests and receives responses.
pub struct UnixSocketClient {
    stream: UnixStream,
}

/// Connect to the IPC server at `path`.
pub async fn connect(path: &Path) -> Result<UnixSocketClient, TransportError> {
    let stream = UnixStream::connect(path).await?;
    Ok(UnixSocketClient { stream })
}

impl UnixSocketClient {
    /// Connect to the IPC server at `path`.
    ///
    /// Convenience wrapper around the free [`connect`] function.
    pub async fn connect(path: &Path) -> Result<UnixSocketClient, TransportError> {
        connect(path).await
    }

    /// Send a request and wait for the response.
    pub async fn request(
        &mut self,
        request: &IpcRequestEnvelope,
    ) -> Result<IpcResponseEnvelope, TransportError> {
        write_envelope(&mut self.stream, request).await?;
        let response: IpcResponseEnvelope = read_envelope(&mut self.stream).await?;
        Ok(response)
    }
}

// ─── Frame helpers (async) ────────────────────────────────────────────────────

/// Write a length-prefixed JSON frame to an async writer.
async fn write_envelope<T: Serialize>(
    writer: &mut (impl AsyncWriteExt + Unpin),
    value: &T,
) -> Result<(), TransportError> {
    let frame = codec::encode(value)?;
    writer.write_all(&frame).await?;
    writer.flush().await?;
    Ok(())
}

/// Read a length-prefixed JSON frame from an async reader.
async fn read_envelope<T: for<'de> Deserialize<'de>>(
    reader: &mut (impl AsyncReadExt + Unpin),
) -> Result<T, TransportError> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_SIZE {
        return Err(TransportError::Codec(CodecError::FrameTooLarge {
            size: len as u32,
        }));
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;
    let value: T = codec::decode(&payload)?;
    Ok(value)
}

/// Read a request envelope from a stream (used by the server loop).
pub async fn read_request(
    reader: &mut (impl AsyncReadExt + Unpin),
) -> Result<IpcRequestEnvelope, TransportError> {
    read_envelope(reader).await
}

/// Write a response envelope to a stream (used by the server loop).
pub async fn write_response(
    writer: &mut (impl AsyncWriteExt + Unpin),
    response: &IpcResponseEnvelope,
) -> Result<(), TransportError> {
    write_envelope(writer, response).await
}
