//! Length-prefixed JSON frame codec.
//!
//! Wire format per frame:
//!
//! ```text
//! ┌────────────────┬──────────────────┐
//! │  length (4 BE) │  JSON payload    │
//! │  u32 bytes     │  <length> bytes  │
//! └────────────────┴──────────────────┘
//! ```
//!
//! Maximum frame size: 64 KiB (65 536 bytes).

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

/// Maximum payload size in bytes (64 KiB).
pub const MAX_FRAME_SIZE: u32 = 64 * 1024;

/// Error type for codec operations.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("frame too large: {size} bytes (max 65536)")]
    FrameTooLarge { size: u32 },

    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

// ─── Encode / Decode (in-memory) ──────────────────────────────────────────────

/// Serialize a value into a length-prefixed JSON frame.
///
/// Returns `Err` if the encoded JSON exceeds [`MAX_FRAME_SIZE`].
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, CodecError> {
    let json_bytes = serde_json::to_vec(value)?;
    let len = json_bytes.len() as u32;
    if len > MAX_FRAME_SIZE {
        return Err(CodecError::FrameTooLarge { size: len });
    }
    let mut frame = Vec::with_capacity(4 + json_bytes.len());
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(&json_bytes);
    Ok(frame)
}

/// Decode a JSON value from raw payload bytes (without the length prefix).
pub fn decode<T: for<'de> Deserialize<'de>>(data: &[u8]) -> Result<T, CodecError> {
    Ok(serde_json::from_slice(data)?)
}

// ─── Stream helpers ───────────────────────────────────────────────────────────

/// Read exactly `n` bytes from a reader, returning an error on short reads.
fn read_exact_bytes<const N: usize>(reader: &mut impl Read) -> io::Result<[u8; N]> {
    let mut buf = [0u8; N];
    reader.read_exact(&mut buf)?;
    Ok(buf)
}

/// Write a complete frame (length prefix + JSON) to a writer.
pub fn write_frame<T: Serialize>(writer: &mut impl Write, value: &T) -> Result<(), CodecError> {
    let frame = encode(value)?;
    writer.write_all(&frame)?;
    writer.flush()?;
    Ok(())
}

/// Read a complete frame from a reader and decode the JSON payload.
///
/// Returns `Err` if the frame exceeds [`MAX_FRAME_SIZE`].
pub fn read_frame<T: for<'de> Deserialize<'de>>(reader: &mut impl Read) -> Result<T, CodecError> {
    let len_bytes = read_exact_bytes::<4>(reader).map_err(CodecError::Io)?;
    let len = u32::from_be_bytes(len_bytes);
    if len > MAX_FRAME_SIZE {
        return Err(CodecError::FrameTooLarge { size: len });
    }
    let mut payload = vec![0u8; len as usize];
    reader.read_exact(&mut payload).map_err(CodecError::Io)?;
    decode(&payload)
}
