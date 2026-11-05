//! 线上帧协议：一条 QUIC 双向流承载一条 `Frame`（JSON）。
//!
//! `Frame` 是 M1 的 `HelloMsg` 的超集：`Hello`/`HelloAck` 的线格式与 M1 逐字节一致
//! （externally tagged 默认序列化，`{"Hello":{"name":"x"}}`），旧版本握手不受影响。

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// 单条 frame 的字节上限，编码与解码两侧都检查。
pub const MAX_FRAME_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Frame {
    /// 客户端问候：报告自己的设备名。
    Hello { name: String },
    /// 服务端应答：报告自己的设备名。
    HelloAck { name: String },
    /// 一条聊天消息。
    Chat {
        id: u64,
        text: String,
        sent_at_ms: u64,
    },
    /// 对 `Chat` 的送达确认（回带 id）。
    ChatAck { id: u64 },
}

/// 当前时间戳（毫秒），用于消息排序与展示。
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 序列化一帧，超过 [`MAX_FRAME_BYTES`] 直接拒绝，不截断。
pub fn encode_frame(frame: &Frame) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(frame)?;
    if bytes.len() > MAX_FRAME_BYTES {
        bail!(
            "frame too large: {} bytes (limit {})",
            bytes.len(),
            MAX_FRAME_BYTES
        );
    }
    Ok(bytes)
}

/// 解析一帧；`limit` 是对端声明/本端约定的字节上限。
pub fn decode_frame(bytes: &[u8], limit: usize) -> Result<Frame> {
    if bytes.len() > limit {
        bail!("frame too large: {} > {limit} bytes", bytes.len());
    }
    Ok(serde_json::from_slice(bytes)?)
}
