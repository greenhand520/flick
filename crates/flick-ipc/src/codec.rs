//! 长度前缀的 MessagePack 帧编解码器。
//!
//! 每帧的线格式：
//!
//! ```text
//! ┌────────────────┬──────────────────────┐
//! │  length (4 BE) │  MessagePack payload │
//! │  u32 bytes     │  <length> bytes      │
//! └────────────────┴──────────────────────┘
//! ```
//!
//! 最大帧大小：1 MiB（1 048 576 字节）。

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

/// 以字节为单位的最大负载大小（1 MiB）。
pub const MAX_FRAME_SIZE: u32 = 1024 * 1024;

/// 编解码器操作的错误类型。
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("frame too large: {size} bytes (max 1048576)")]
    FrameTooLarge { size: u32 },

    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("MessagePack encode error: {0}")]
    Encode(#[from] rmp_serde::encode::Error),

    #[error("MessagePack decode error: {0}")]
    Decode(#[from] rmp_serde::decode::Error),
}

// ─── 编码 / 解码（内存中） ────────────────────────────────────────────────────

/// 将一个值序列化为长度前缀的 MessagePack 帧。
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, CodecError> {
    let payload = rmp_serde::to_vec_named(value)?;
    // 先按 `usize` 判定上限，避免 `payload.len() as u32` 在 >4 GiB 时截断回绕。
    if payload.len() > MAX_FRAME_SIZE as usize {
        let size = u32::try_from(payload.len()).unwrap_or(u32::MAX);
        return Err(CodecError::FrameTooLarge { size });
    }
    let len = payload.len() as u32;
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// 从原始负载字节中解码 MessagePack 值（不含长度前缀）。
///
/// # 前置约定
///
/// `data` 是**去掉长度前缀后的裸 MessagePack 负载**，调用方应已读取并信任前缀。
/// 作为公开 API 的自我保护，本函数仍会按 [`MAX_FRAME_SIZE`] 校验 `data.len()`；
/// 超过上限直接返回 [`CodecError::FrameTooLarge`]，不再交给解码器，
/// 以免对超大输入做无谓解析。长度前缀的解帧逻辑见 [`read_frame`]。
pub fn decode<T: for<'de> Deserialize<'de>>(data: &[u8]) -> Result<T, CodecError> {
    if data.len() > MAX_FRAME_SIZE as usize {
        let size = u32::try_from(data.len()).unwrap_or(u32::MAX);
        return Err(CodecError::FrameTooLarge { size });
    }
    Ok(rmp_serde::from_slice(data)?)
}

// ─── 流辅助函数 ───────────────────────────────────────────────────────────────

fn read_exact_bytes<const N: usize>(reader: &mut impl Read) -> io::Result<[u8; N]> {
    let mut buf = [0u8; N];
    reader.read_exact(&mut buf)?;
    Ok(buf)
}

/// 将一个完整的帧（长度前缀 + MessagePack）写入写入器。
pub fn write_frame<T: Serialize>(writer: &mut impl Write, value: &T) -> Result<(), CodecError> {
    let frame = encode(value)?;
    writer.write_all(&frame)?;
    writer.flush()?;
    Ok(())
}

/// 从读取器读取一个完整的帧并解码 MessagePack 负载。
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
