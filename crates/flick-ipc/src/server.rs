//! IPC 请求分派器与服务端循环。

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

/// 应用层实现的 trait，用于处理 IPC 命令。
///
/// [`IpcCommand`] 的每个变体都通过这个 trait 进行分派。
#[async_trait]
pub trait IpcDispatcher: Send + Sync + 'static {
    /// 分派单条 IPC 命令并返回结果。
    async fn dispatch(&self, command: IpcCommand) -> IpcResult;

    /// 仅在 `command` 的响应写出之后才执行某个副作用。
    ///
    /// IPC 关闭操作使用这个钩子，以便其确认消息在停止 accept 循环之前
    /// 到达客户端。其他分派器不需要后续操作。
    async fn after_response(&self, _command: &IpcCommand) {}
}

/// IPC 服务端：持有一个 [`UnixSocketServer`] 和一个分派器，并运行
/// accept 循环。
pub struct IpcServer {
    server: UnixSocketServer,
    dispatcher: Arc<dyn IpcDispatcher>,
}

impl IpcServer {
    /// 从已绑定的套接字和分派器创建一个新的 IPC 服务端。
    pub fn new(server: UnixSocketServer, dispatcher: Arc<dyn IpcDispatcher>) -> Self {
        Self { server, dispatcher }
    }

    /// 运行 accept 循环，直到 `cancel` 被取消。
    ///
    /// 每个被接受的连接都由一个受监管的任务处理。取消操作
    /// 会停止接受新连接，中断进行中的读取和分派调用，
    /// 然后等待每个客户端任务完成后才返回。
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

        // 如果接受连接失败（与守护进程取消无关），也要释放客户端工作，
        // 然后再等待每个受监管的任务完成。
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

    /// 处理单个客户端连接：读取请求 → 分派 → 写入响应。
    /// 一次往返之后连接即关闭（Phase 0）。
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

        let Some((response, command)) =
            Self::process_request(request, Arc::clone(&dispatcher), &cancel).await?
        else {
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
        }?;

        if let Some(command) = command {
            dispatcher.after_response(&command).await;
        }
        Ok(())
    }

    /// 校验并分派单个请求。
    ///
    /// `None` 表示守护进程关闭取消了进行中的分派。
    async fn process_request(
        request: IpcRequestEnvelope,
        dispatcher: Arc<dyn IpcDispatcher>,
        cancel: &CancellationToken,
    ) -> Result<Option<(IpcResponseEnvelope, Option<IpcCommand>)>, TransportError> {
        let request_id = request.request_id.clone();

        // 协议版本检查。
        if request.protocol_version != IPC_PROTOCOL_VERSION {
            debug!(
                "IPC: version mismatch: client={}, server={}",
                request.protocol_version, IPC_PROTOCOL_VERSION,
            );
            return Ok(Some((
                IpcResponseEnvelope {
                    protocol_version: IPC_PROTOCOL_VERSION,
                    request_id,
                    result: IpcResult::error(
                        IpcErrorCode::ProtocolVersionMismatch,
                        format!(
                            "unsupported protocol version {} (server expects {})",
                            request.protocol_version, IPC_PROTOCOL_VERSION,
                        ),
                    ),
                },
                None,
            )));
        }

        let command = request.command;
        let result = tokio::select! {
            biased;

            _ = cancel.cancelled() => return Ok(None),
            result = timeout(IPC_REQUEST_TIMEOUT, dispatcher.dispatch(command.clone())) => {
                result.map_err(|_| TransportError::TimedOut {
                    operation: "dispatch request",
                })?
            }
        };

        Ok(Some((
            IpcResponseEnvelope {
                protocol_version: IPC_PROTOCOL_VERSION,
                request_id,
                result,
            },
            Some(command),
        )))
    }
}
