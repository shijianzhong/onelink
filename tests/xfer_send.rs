//! M3 Task 3：发送端 —— 真起两个 endpoint，测试里手写接收端。
//!
//! 接收端刻意不调用 crate 的接收实现（T4 才落地），这样发送端能独立验收：
//! 块拼接字节、FileDone 里的 blake3、等 ACK、进度事件必须被节流。

use iroh::endpoint::presets;
use onelink::net::{EVENT_QUEUE_CAPACITY, EventQueue, FILE_ALPN, UiEvent, send_file_data};
use onelink::proto::Frame;
use onelink::xfer::{MAX_FRAMED_BYTES, block_len, decode_framed, encode_framed};
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn send_file_streams_blocks_and_waits_for_ack() {
    let a = iroh::Endpoint::builder(presets::Minimal)
        .alpns(vec![FILE_ALPN.to_vec()])
        .bind()
        .await
        .unwrap();
    let b = iroh::Endpoint::builder(presets::Minimal)
        .bind()
        .await
        .unwrap();
    let a_addr = a.addr();

    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.bin");
    // 20MB：20MB / 256KB = 80 块，足够暴露「逐块发进度事件」的问题。
    let data: Vec<u8> = (0..20 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&src, &data).unwrap();
    let expected_hash = blake3::hash(&data).to_hex().to_string();
    let expected_len = data.len();
    let expected = data.clone();

    let receiver = tokio::spawn(async move {
        let incoming = a.accept().await.expect("incoming");
        let conn = incoming.await.unwrap();
        let (mut send, mut recv) = conn.accept_bi().await.unwrap();

        // 首帧：FileData
        let mut hdr = [0u8; 4];
        recv.read_exact(&mut hdr).await.unwrap();
        let len = block_len(&hdr);
        let mut body = vec![0u8; len];
        recv.read_exact(&mut body).await.unwrap();
        let mut buf = hdr.to_vec();
        buf.extend_from_slice(&body);
        assert_eq!(
            decode_framed(&buf, MAX_FRAMED_BYTES).unwrap(),
            Frame::FileData { xfer_id: 9 }
        );

        // 数据块直到 EOF
        let mut got = Vec::new();
        loop {
            recv.read_exact(&mut hdr).await.unwrap();
            let n = block_len(&hdr);
            if n == 0 {
                break;
            }
            let mut block = vec![0u8; n];
            recv.read_exact(&mut block).await.unwrap();
            got.extend_from_slice(&block);
        }

        // 尾帧：FileDone
        recv.read_exact(&mut hdr).await.unwrap();
        let len = block_len(&hdr);
        let mut body = vec![0u8; len];
        recv.read_exact(&mut body).await.unwrap();
        let mut buf = hdr.to_vec();
        buf.extend_from_slice(&body);
        let done = decode_framed(&buf, MAX_FRAMED_BYTES).unwrap();
        let Frame::FileDone { xfer_id, hash } = done else {
            panic!("expected FileDone, got {done:?}");
        };
        assert_eq!(xfer_id, 9);

        // 回 ACK 才算送达
        send.write_all(&encode_framed(&Frame::FileAck { xfer_id: 9 }).unwrap())
            .await
            .unwrap();
        send.shutdown().await.unwrap();
        // 与生产端一致：回完 ACK 必须等对端收尾再关闭连接，否则 ACK 会随拆链丢失
        // （net.rs `dispatch_connection` 已记录该 M1 实机复现问题）。
        let _ = conn.closed().await;

        (got, hash)
    });

    let queue = EventQueue::new(EVENT_QUEUE_CAPACITY);
    send_file_data(&b, a_addr, 9, src.clone(), &queue)
        .await
        .expect("send_file_data");

    let (got, hash) = receiver.await.unwrap();
    assert_eq!(got.len(), expected_len, "接收端字节数应与源文件一致");
    assert_eq!(got, expected, "逐字节一致");
    assert_eq!(hash, expected_hash, "FileDone 的 hash 必须是源文件 blake3");

    let mut progress = 0usize;
    let mut completed = 0usize;
    while let Some(ev) = queue.try_recv() {
        match ev {
            UiEvent::TransferProgress { .. } => progress += 1,
            UiEvent::TransferCompleted { .. } => completed += 1,
            _ => {}
        }
    }
    assert!(progress >= 1, "至少发一条进度");
    assert!(
        progress <= 30,
        "80 块必须被节流到 ≤30 条事件，实为 {progress}"
    );
    assert_eq!(completed, 1, "必须恰好一条完成事件");
}
