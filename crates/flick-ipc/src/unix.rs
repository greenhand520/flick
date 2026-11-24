//! Flick IPC 的 Unix 域套接字传输。

use crate::codec::{self, CodecError};
use crate::message::{IpcRequestEnvelope, IpcResponseEnvelope};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

/// 以字节为单位的最大负载大小（与 [`codec::MAX_FRAME_SIZE`] 一致）。
const MAX_FRAME_SIZE: usize = codec::MAX_FRAME_SIZE as usize;

/// CLI 等待建立本地 IPC 连接的最长时间。
pub const IPC_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// 一次完整 IPC 请求操作所允许的最长时间。
pub const IPC_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

// ─── 错误 ─────────────────────────────────────────────────────────────────────

/// Unix 套接字传输层特有的错误。
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("socket I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("codec error: {0}")]
    Codec(#[from] CodecError),

    #[error("another instance is already listening on this socket")]
    AlreadyRunning,

    #[error("IPC {operation} timed out")]
    TimedOut { operation: &'static str },
}

// ─── 服务端 ───────────────────────────────────────────────────────────────────

/// 一次接受单个 IPC 客户端的 Unix 域套接字服务端。
///
/// 使用 [`bind`] 创建。绑定时会执行单实例探测：
/// 如果到该路径的连接成功，说明另一个守护进程已在运行，
/// 将返回 [`TransportError::AlreadyRunning`]。如果连接失败，
/// 则在绑定前删除（陈旧的）套接字文件。
pub struct UnixSocketServer {
    listener: UnixListener,
}

/// 在 `path` 处绑定一个新的 Unix 套接字服务端。
///
/// **单实例协议：**
/// 1. 尝试连接到 `path`。
///    - 成功 → 另一个实例已在监听 → 返回 `AlreadyRunning`。
///    - 失败 → 套接字已陈旧（无监听者）→ 继续。
/// 2. 删除陈旧的套接字文件。
/// 3. 绑定一个新的监听器。
pub fn bind(path: &Path) -> Result<UnixSocketServer, TransportError> {
    // 第 1 步：探测已有实例。
    match std::os::unix::net::UnixStream::connect(path) {
        Ok(_stream) => {
            // 另一个守护进程正在监听。
            return Err(TransportError::AlreadyRunning);
        }
        Err(_) => {
            // 连接失败——套接字已陈旧或不存在。
        }
    }

    // 第 2 步：如果存在陈旧的套接字文件，则将其删除。
    let _ = std::fs::remove_file(path);

    // 第 3 步：创建仅用户可访问的父目录。
    if let Some(parent) = path.parent() {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(parent)
            .map_err(TransportError::Io)?;
    }

    // 第 4 步：绑定。
    let listener = UnixListener::bind(path)?;

    // 第 5 步：将套接字权限设置为 0600（仅用户可读写）。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(TransportError::Io)?;
    }

    Ok(UnixSocketServer { listener })
}

impl UnixSocketServer {
    /// 在 `path` 处绑定一个新的 Unix 套接字服务端。
    ///
    /// 对自由函数 [`bind`] 的便捷封装。
    pub fn bind(path: &Path) -> Result<UnixSocketServer, TransportError> {
        bind(path)
    }

    /// 接受下一个客户端连接。
    pub async fn accept(&self) -> Result<UnixStream, TransportError> {
        let (stream, _) = self.listener.accept().await?;
        Ok(stream)
    }
}

// ─── 客户端 ───────────────────────────────────────────────────────────────────

/// 发送请求并接收响应的 Unix 域套接字客户端。
pub struct UnixSocketClient {
    stream: UnixStream,
}

/// 连接到 `path` 处的 IPC 服务端。
pub async fn connect(path: &Path) -> Result<UnixSocketClient, TransportError> {
    let stream = tokio::time::timeout(IPC_CONNECT_TIMEOUT, UnixStream::connect(path))
        .await
        .map_err(|_| TransportError::TimedOut {
            operation: "connect",
        })??;
    Ok(UnixSocketClient { stream })
}

impl UnixSocketClient {
    /// 连接到 `path` 处的 IPC 服务端。
    ///
    /// 对自由函数 [`connect`] 的便捷封装。
    pub async fn connect(path: &Path) -> Result<UnixSocketClient, TransportError> {
        connect(path).await
    }

    /// 发送请求并等待响应。
    pub async fn request(
        &mut self,
        request: &IpcRequestEnvelope,
    ) -> Result<IpcResponseEnvelope, TransportError> {
        tokio::time::timeout(IPC_REQUEST_TIMEOUT, async {
            write_envelope(&mut self.stream, request).await?;
            read_envelope(&mut self.stream).await
        })
        .await
        .map_err(|_| TransportError::TimedOut {
            operation: "request",
        })?
    }
}

// ─── 帧辅助函数（异步） ────────────────────────────────────────────────────────

/// 将长度前缀的 JSON 帧写入异步写入器。
async fn write_envelope<T: Serialize>(
    writer: &mut (impl AsyncWriteExt + Unpin),
    value: &T,
) -> Result<(), TransportError> {
    let frame = codec::encode(value)?;
    writer.write_all(&frame).await?;
    writer.flush().await?;
    Ok(())
}

/// 从异步读取器读取长度前缀的 JSON 帧。
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

/// 从流中读取请求信封（由服务端循环使用）。
pub async fn read_request(
    reader: &mut (impl AsyncReadExt + Unpin),
) -> Result<IpcRequestEnvelope, TransportError> {
    read_envelope(reader).await
}

/// 将响应信封写入流（由服务端循环使用）。
pub async fn write_response(
    writer: &mut (impl AsyncWriteExt + Unpin),
    response: &IpcResponseEnvelope,
) -> Result<(), TransportError> {
    write_envelope(writer, response).await
}
