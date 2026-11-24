//! 长度前缀的 JSON 帧编解码器。
//!
//! 每帧的线格式：
//!
//! ```text
//! ┌────────────────┬──────────────────┐
//! │  length (4 BE) │  JSON payload    │
//! │  u32 bytes     │  <length> bytes  │
//! └────────────────┴──────────────────┘
//! ```
//!
//! 最大帧大小：64 KiB（65 536 字节）。

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

/// 以字节为单位的最大负载大小（64 KiB）。
pub const MAX_FRAME_SIZE: u32 = 64 * 1024;

/// 编解码器操作的错误类型。
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("frame too large: {size} bytes (max 65536)")]
    FrameTooLarge { size: u32 },

    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

// ─── 编码 / 解码（内存中） ────────────────────────────────────────────────────

/// 将一个值序列化为长度前缀的 JSON 帧。
///
/// 如果编码后的 JSON 超过 [`MAX_FRAME_SIZE`]，则返回 `Err`。
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

/// 从原始负载字节中解码 JSON 值（不含长度前缀）。
pub fn decode<T: for<'de> Deserialize<'de>>(data: &[u8]) -> Result<T, CodecError> {
    Ok(serde_json::from_slice(data)?)
}

// ─── 流辅助函数 ───────────────────────────────────────────────────────────────

/// 从读取器中读取恰好 `n` 个字节，读取不足时返回错误。
fn read_exact_bytes<const N: usize>(reader: &mut impl Read) -> io::Result<[u8; N]> {
    let mut buf = [0u8; N];
    reader.read_exact(&mut buf)?;
    Ok(buf)
}

/// 将一个完整的帧（长度前缀 + JSON）写入写入器。
pub fn write_frame<T: Serialize>(writer: &mut impl Write, value: &T) -> Result<(), CodecError> {
    let frame = encode(value)?;
    writer.write_all(&frame)?;
    writer.flush()?;
    Ok(())
}

/// 从读取器读取一个完整的帧并解码 JSON 负载。
///
/// 如果帧超过 [`MAX_FRAME_SIZE`]，则返回 `Err`。
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
