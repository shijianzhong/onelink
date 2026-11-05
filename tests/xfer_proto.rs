//! M3 Task 1：长度前缀帧与块编码的纯逻辑测试（不碰网络）。

use onelink::proto::{Frame, encode_frame};
use onelink::xfer::*;

#[test]
fn framed_roundtrip_preserves_frame() {
    let frame = Frame::FileOffer {
        xfer_id: 7,
        name: "a.txt".into(),
        size: 12,
    };
    let bytes = encode_framed(&frame).unwrap();
    assert_eq!(bytes.len(), 4 + encode_frame(&frame).unwrap().len());
    assert_eq!(decode_framed(&bytes, MAX_FRAMED_BYTES).unwrap(), frame);
}

#[test]
fn framed_rejects_length_mismatch() {
    let mut bytes = encode_framed(&Frame::FileAccept { xfer_id: 1 }).unwrap();
    bytes.truncate(bytes.len() - 2); // 前缀声明 N 字节，实际少 2 字节
    let err = decode_framed(&bytes, MAX_FRAMED_BYTES)
        .unwrap_err()
        .to_string();
    assert!(err.contains("length"), "err was {err}");
}

#[test]
fn framed_rejects_over_limit() {
    let big = Frame::FileReject {
        xfer_id: 1,
        reason: "x".repeat(MAX_FRAMED_BYTES),
    };
    assert!(encode_framed(&big).is_err());
}

#[test]
fn framed_control_frames_keep_line_format() {
    // 控制帧仍走无前缀 JSON（M1/M2 兼容），文件帧复用同一枚举。
    let json = encode_frame(&Frame::FileAccept { xfer_id: 3 }).unwrap();
    assert_eq!(json, br#"{"FileAccept":{"xfer_id":3}}"#);
}

#[test]
fn block_roundtrip_and_eof() {
    let data = vec![7u8; 1024];
    let bytes = encode_block(&data).unwrap();
    let mut header = [0u8; 4];
    header.copy_from_slice(&bytes[..4]);
    assert_eq!(block_len(&header), 1024);
    assert_eq!(&bytes[4..], &data[..]);
    assert_eq!(block_len(&EOF_BLOCK), 0);
    assert_eq!(block_header(1024), [0, 0, 4, 0]);
}

#[test]
fn block_rejects_oversize_and_empty() {
    assert!(encode_block(&vec![0u8; BLOCK_SIZE + 1]).is_err());
    assert!(encode_block(&[]).is_err()); // 空块非法，结束用 EOF_BLOCK
}
