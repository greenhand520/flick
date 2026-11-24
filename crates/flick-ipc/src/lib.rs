//! `flick-ipc` — Flick 的进程间通信。
//!
//! 提供 IPC 协议类型、长度前缀的 JSON 帧编解码器、
//! Unix 域套接字传输，以及请求分派框架。
//!
//! # 线格式
//!
//! 每条 IPC 消息都是一个单独的 JSON 帧，前面带有一个 4 字节的
//! 大端长度头。最大帧大小：64 KiB。
//!
//! # 协议
//!
//! 1. 客户端连接到守护进程的 Unix 套接字。
//! 2. 客户端发送一个 [`message::IpcRequestEnvelope`]。
//! 3. 服务端回复一个 [`message::IpcResponseEnvelope`]。
//! 4. 连接关闭（Phase 0）。

pub mod codec;
pub mod message;
pub mod server;
pub mod unix;

// ─── 重新导出 ─────────────────────────────────────────────────────────────────

pub use codec::{CodecError, MAX_FRAME_SIZE};
pub use message::{
    IPC_PROTOCOL_VERSION, IpcCommand, IpcErrorCode, IpcRequestEnvelope, IpcResponseEnvelope,
    IpcResult,
};
pub use server::{IpcDispatcher, IpcServer};
pub use unix::{TransportError, UnixSocketClient, UnixSocketServer};
