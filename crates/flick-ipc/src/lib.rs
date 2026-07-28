//! `flick-ipc` — Inter-process communication for Flick.
//!
//! Provides the IPC protocol types, a length-prefixed JSON frame codec,
//! a Unix domain socket transport, and a request dispatcher framework.
//!
//! # Wire format
//!
//! Each IPC message is a single JSON frame prefixed by a 4-byte
//! big-endian length header.  Maximum frame size: 64 KiB.
//!
//! # Protocol
//!
//! 1. Client connects to the daemon's Unix socket.
//! 2. Client sends one [`message::IpcRequestEnvelope`].
//! 3. Server replies with one [`message::IpcResponseEnvelope`].
//! 4. Connection is closed (Phase 0).

pub mod codec;
pub mod message;
pub mod server;
pub mod unix;

// ─── Re-exports ───────────────────────────────────────────────────────────────

pub use codec::{CodecError, MAX_FRAME_SIZE};
pub use message::{
    IpcCommand, IpcErrorCode, IpcRequestEnvelope, IpcResponseEnvelope, IpcResult,
    IPC_PROTOCOL_VERSION,
};
pub use server::{IpcDispatcher, IpcServer};
pub use unix::{TransportError, UnixSocketClient, UnixSocketServer};
