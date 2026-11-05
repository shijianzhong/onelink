//! 文件传输的线格式。
//!
//! 控制帧（offer/accept/reject）走 `onelink-v1`，一条流一帧、无前缀，与 M1/M2 一致。
//! 文件字节走独立 ALPN `onelink-v1-file`，格式为：
//!
//! ```text
//! [u32 BE len][Frame::FileData]      // 首帧，声明 xfer_id
//! [u32 BE len][bytes]  ...           // 数据块，len <= BLOCK_SIZE
//! [0u32]                             // 结束标记
//! [u32 BE len][Frame::FileDone]      // 尾帧，带 blake3
//! ```
//!
//! 两侧都只用一块 [`BLOCK_SIZE`] 缓冲，内存不随文件大小增长。

use anyhow::{Result, bail};

use crate::proto::{Frame, decode_frame, encode_frame};

/// 单个数据块的字节上限。
pub const BLOCK_SIZE: usize = 256 * 1024;

/// 长度前缀之后 payload 的上限（与 [`crate::proto::MAX_FRAME_BYTES`] 同值）。
pub const MAX_FRAMED_BYTES: usize = 64 * 1024;

/// 结束标记：长度为 0 的块。
pub const EOF_BLOCK: [u8; 4] = [0, 0, 0, 0];

/// 编码一条带长度前缀的帧。
pub fn encode_framed(frame: &Frame) -> Result<Vec<u8>> {
    let payload = encode_frame(frame)?;
    if payload.len() > MAX_FRAMED_BYTES {
        bail!(
            "framed payload too large: {} bytes (limit {})",
            payload.len(),
            MAX_FRAMED_BYTES
        );
    }
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&block_header(payload.len()));
    out.extend_from_slice(&payload);
    Ok(out)
}

/// 解析一条带长度前缀的帧，长度前缀必须与正文实际长度一致。
pub fn decode_framed(bytes: &[u8], limit: usize) -> Result<Frame> {
    if bytes.len() < 4 {
        bail!(
            "framed message shorter than length prefix: {} bytes",
            bytes.len()
        );
    }
    let len = block_len(&[bytes[0], bytes[1], bytes[2], bytes[3]]);
    let body = bytes.len() - 4;
    if len != body {
        bail!("framed length mismatch: header says {len}, body is {body}");
    }
    if len > limit {
        bail!("framed payload too large: {len} bytes (limit {limit})");
    }
    decode_frame(&bytes[4..], limit)
}

/// 编码一个数据块（非空，且不超过 [`BLOCK_SIZE`]）。
pub fn encode_block(data: &[u8]) -> Result<Vec<u8>> {
    if data.is_empty() {
        bail!("empty block is not allowed; use EOF_BLOCK to end the stream");
    }
    if data.len() > BLOCK_SIZE {
        bail!(
            "block too large: {} bytes (limit {})",
            data.len(),
            BLOCK_SIZE
        );
    }
    let mut out = Vec::with_capacity(4 + data.len());
    out.extend_from_slice(&block_header(data.len()));
    out.extend_from_slice(data);
    Ok(out)
}

/// 4 字节大端长度。
pub fn block_header(len: usize) -> [u8; 4] {
    (len as u32).to_be_bytes()
}

/// 从 4 字节头读出长度（0 表示流结束）。
pub fn block_len(header: &[u8; 4]) -> usize {
    u32::from_be_bytes(*header) as usize
}
