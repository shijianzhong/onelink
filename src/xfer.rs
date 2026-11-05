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

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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

/// 清洗对端发来的文件名：只留最后一段，去掉控制字符，保证非空且不超 200 字符。
///
/// 结果永远只是**一个文件名**，不含任何分隔符，因此拼进接收目录也不会越界。
pub fn sanitize_file_name(raw: &str) -> String {
    let last_segment = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = last_segment.chars().filter(|c| !c.is_control()).collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return "unnamed".to_string();
    }
    trimmed.chars().take(200).collect()
}

/// 在 `dir` 里给 `name` 找一个不覆盖已有文件的路径（`stem (n).ext`）。
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for n in 1..1000u32 {
        let next = dir.join(format!("{stem} ({n}){ext}"));
        if !next.exists() {
            return next;
        }
    }
    dir.join(format!("{stem} (overflow){ext}"))
}

/// 默认接收目录：`~/Downloads/OneLink`（Windows 用 `USERPROFILE`）。
pub fn default_recv_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("Downloads").join("OneLink")
}

/// 流式计算整个文件的 blake3（hex），固定 64KB 缓冲，不把文件读进内存。
pub async fn file_hash(path: &Path) -> Result<String> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("open {}", path.display()))?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// 进度事件节流：跨过 1%、或距上次发出 >=200ms、或传输完成，才允许发一条事件。
///
/// 1GB 文件 = 4096 块，逐块发事件会把容量 32 的事件队列冲爆。
#[derive(Debug, Default)]
pub struct ProgressThrottle {
    last_pct: u8,
    last_at: Option<Instant>,
}

impl ProgressThrottle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn should_emit(&mut self, transferred: u64, total: u64, now: Instant) -> bool {
        let pct = (transferred.min(total) * 100)
            .checked_div(total)
            .map(|v| v as u8)
            .unwrap_or(100);
        let finished = total > 0 && transferred >= total;
        let time_ok = self
            .last_at
            .is_none_or(|last| now.duration_since(last) >= Duration::from_millis(200));
        if finished || self.last_at.is_none() || pct > self.last_pct || time_ok {
            self.last_pct = pct;
            self.last_at = Some(now);
            true
        } else {
            false
        }
    }
}
