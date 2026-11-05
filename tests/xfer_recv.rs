use std::ffi::OsString;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use iroh::endpoint::presets;
use onelink::net::{
    ALPN, FILE_ALPN, IrohServiceHandle, UiEvent, shutdown_iroh_service, spawn_iroh_service,
};
use onelink::proto::{Frame, MAX_FRAME_BYTES, decode_frame, encode_frame};
use onelink::xfer::{
    BLOCK_SIZE, EOF_BLOCK, MAX_FRAMED_BYTES, decode_framed, encode_block, encode_framed,
};
use tokio::io::AsyncWriteExt;

/// 接收目录来自 `HOME`，测试之间必须串行改环境变量。
static HOME_LOCK: Mutex<()> = Mutex::new(());

struct HomeGuard {
    previous: Option<OsString>,
    _guard: MutexGuard<'static, ()>,
}

impl HomeGuard {
    fn set(dir: &Path) -> Self {
        let guard = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::var_os("HOME");
        // SAFETY: 本测试二进制内所有改 HOME 的用例都持有 HOME_LOCK，
        // 且只在本测试线程内读写。
        unsafe { std::env::set_var("HOME", dir) };
        Self {
            previous,
            _guard: guard,
        }
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        // SAFETY: 同 set()，仍在 HOME_LOCK 保护内。
        match &self.previous {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }
}

async fn wait_event<F>(queue: &onelink::net::EventQueue, pred: F, secs: u64) -> Option<UiEvent>
where
    F: Fn(&UiEvent) -> bool,
{
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        while let Some(ev) = queue.try_recv() {
            if pred(&ev) {
                return Some(ev);
            }
        }
        // 必须用 tokio 的 sleep：手写发送端的 iroh 任务跑在本测试 runtime 上，
        // 阻塞线程会让它无法推进传输。
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    None
}

/// 手写发送端也要声明控制 ALPN：接收端的 FileAccept 会连回来。
fn builder() -> iroh::endpoint::Builder {
    iroh::Endpoint::builder(presets::Minimal).alpns(vec![ALPN.to_vec()])
}

/// 手写发送端第一段：连控制 ALPN 发 FileOffer 并 shutdown，连接保留到调用方 drop。
async fn send_offer(
    sender: &iroh::Endpoint,
    recv_addr: iroh::EndpointAddr,
    xfer_id: u64,
    name: &str,
    size: u64,
) -> (
    iroh::endpoint::Connection,
    iroh::endpoint::SendStream,
    iroh::endpoint::RecvStream,
) {
    let conn = sender.connect(recv_addr, ALPN).await.unwrap();
    let (mut send, recv) = conn.open_bi().await.unwrap();
    let offer = Frame::FileOffer {
        xfer_id,
        name: name.to_string(),
        size,
    };
    send.write_all(&encode_frame(&offer).unwrap())
        .await
        .unwrap();
    send.shutdown().await.unwrap();
    // 流句柄必须活到对端读完，否则 drop 会中断尚未传输的帧。
    (conn, send, recv)
}

/// 手写发送端第二段：等接收端连回来，读出 FileAccept。
async fn wait_file_accept(sender: &iroh::Endpoint, xfer_id: u64) {
    let incoming = tokio::time::timeout(Duration::from_secs(10), sender.accept())
        .await
        .expect("等待 FileAccept 连接超时")
        .expect("accept 失败");
    let conn = incoming.await.expect("accept 连接失败");
    let (_send, mut recv) = conn.accept_bi().await.unwrap();
    let buf = recv.read_to_end(MAX_FRAME_BYTES).await.unwrap();
    assert!(
        matches!(
            decode_frame(&buf, MAX_FRAME_BYTES).unwrap(),
            Frame::FileAccept { xfer_id: id } if id == xfer_id
        ),
        "期望 FileAccept({xfer_id})"
    );
}

/// 手写发送端第三段：连 FILE_ALPN 发块 + FileDone（hash 可注入错误值），返回读 ACK 的结果。
async fn send_file_bytes(
    sender: &iroh::Endpoint,
    recv_addr: iroh::EndpointAddr,
    xfer_id: u64,
    data: &[u8],
    hash: &str,
) -> anyhow::Result<Vec<u8>> {
    let conn = sender.connect(recv_addr, FILE_ALPN).await.unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    send.write_all(&encode_framed(&Frame::FileData { xfer_id }).unwrap())
        .await?;
    for chunk in data.chunks(BLOCK_SIZE) {
        send.write_all(&encode_block(chunk).unwrap()).await?;
    }
    send.write_all(&EOF_BLOCK).await?;
    send.write_all(
        &encode_framed(&Frame::FileDone {
            xfer_id,
            hash: hash.to_string(),
        })
        .unwrap(),
    )
    .await?;
    send.shutdown().await?;
    let ack = recv.read_to_end(MAX_FRAMED_BYTES).await?;
    // 直接返回、让 conn 在这里 drop：接收端在回完 ACK 后等对端关闭，
    // 若本端也等 closed() 会互相卡死。
    Ok(ack)
}

fn assert_clean_dir(dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries {
            let p = entry.unwrap().path();
            assert!(
                p.extension().and_then(|e| e.to_str()) != Some("part"),
                "残留临时文件: {p:?}"
            );
        }
    }
}

fn stop(handle: IrohServiceHandle) {
    shutdown_iroh_service(&handle);
}

#[tokio::test]
async fn accepted_file_lands_with_matching_hash() {
    let home = tempfile::tempdir().unwrap();
    let _guard = HomeGuard::set(home.path());
    let recv = spawn_iroh_service().expect("接收端起服务");
    let recv_addr = recv.addr.clone();

    let sender = builder().bind().await.unwrap();
    let xfer_id = 3;
    let data: Vec<u8> = (0..1024u32).map(|i| (i % 251) as u8).collect();
    let hash = blake3::hash(&data).to_hex().to_string();

    let (offer_conn, offer_send, offer_recv) = send_offer(
        &sender,
        recv_addr.clone(),
        xfer_id,
        "../../run.sh",
        data.len() as u64,
    )
    .await;

    let offer = wait_event(
        &recv.tx,
        |e| matches!(e, UiEvent::TransferOffer { xfer_id: id, .. } if *id == xfer_id),
        10,
    )
    .await
    .expect("未收到 TransferOffer");
    match offer {
        UiEvent::TransferOffer { name, size, .. } => {
            assert_eq!(name, "../../run.sh");
            assert_eq!(size, 1024);
        }
        _ => unreachable!(),
    }

    recv.accept_file(xfer_id);
    wait_file_accept(&sender, xfer_id).await;
    drop((offer_conn, offer_send, offer_recv));

    let ack = send_file_bytes(&sender, recv_addr, xfer_id, &data, &hash)
        .await
        .expect("应收到 FileAck");
    assert!(
        matches!(
            decode_framed(&ack, MAX_FRAMED_BYTES).unwrap(),
            Frame::FileAck { xfer_id: id } if id == xfer_id
        ),
        "ACK 帧不匹配"
    );

    let done = wait_event(
        &recv.tx,
        |e| matches!(e, UiEvent::TransferCompleted { xfer_id: id, .. } if *id == xfer_id),
        10,
    )
    .await
    .expect("未收到 TransferCompleted");
    let UiEvent::TransferCompleted {
        path: Some(path), ..
    } = done
    else {
        panic!("TransferCompleted 缺少落盘路径")
    };

    let recv_dir = home.path().join("Downloads").join("OneLink");
    assert_eq!(path.parent().unwrap(), recv_dir);
    assert_eq!(path.file_name().unwrap().to_string_lossy(), "run.sh");
    assert_eq!(std::fs::read(&path).unwrap(), data, "落盘字节不一致");
    assert_clean_dir(&recv_dir);

    sender.close().await;
    stop(recv);
}

#[tokio::test]
async fn hash_mismatch_fails_without_leaving_file() {
    let home = tempfile::tempdir().unwrap();
    let _guard = HomeGuard::set(home.path());
    let recv = spawn_iroh_service().expect("接收端起服务");
    let recv_addr = recv.addr.clone();

    let sender = builder().bind().await.unwrap();
    let xfer_id = 4;
    let data = vec![9u8; 1024];
    let wrong_hash = blake3::hash(b"not the file").to_hex().to_string();

    let (offer_conn, offer_send, offer_recv) = send_offer(
        &sender,
        recv_addr.clone(),
        xfer_id,
        "evil.sh",
        data.len() as u64,
    )
    .await;
    wait_event(
        &recv.tx,
        |e| matches!(e, UiEvent::TransferOffer { xfer_id: id, .. } if *id == xfer_id),
        10,
    )
    .await
    .expect("未收到 TransferOffer");
    recv.accept_file(xfer_id);
    wait_file_accept(&sender, xfer_id).await;
    drop((offer_conn, offer_send, offer_recv));

    // 对端校验失败不会回 ACK，这里只关心接收端事件与文件系统。
    let _ = send_file_bytes(&sender, recv_addr, xfer_id, &data, &wrong_hash).await;
    let failed = wait_event(
        &recv.tx,
        |e| matches!(e, UiEvent::TransferFailed { xfer_id: id, .. } if *id == xfer_id),
        10,
    )
    .await
    .expect("未收到 TransferFailed");
    match failed {
        UiEvent::TransferFailed { reason, .. } => {
            assert!(reason.contains("hash mismatch"), "reason = {reason}");
        }
        _ => unreachable!(),
    }

    let recv_dir = home.path().join("Downloads").join("OneLink");
    assert!(!recv_dir.join("evil.sh").exists(), "校验失败不应留下成品");
    assert!(
        !recv_dir.join("evil.part").exists(),
        "校验失败不应留下临时文件"
    );
    assert_clean_dir(&recv_dir);

    sender.close().await;
    stop(recv);
}

#[tokio::test]
async fn rejected_offer_gives_sender_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("a.bin");
    std::fs::write(&src, vec![1u8; 1024]).unwrap();

    let a = spawn_iroh_service().expect("发送端起服务");
    let b = spawn_iroh_service().expect("接收端起服务");
    let xfer_id = a.send_file_to(b.addr.clone(), src);
    assert_ne!(xfer_id, 0);

    let offer = wait_event(
        &b.tx,
        |e| matches!(e, UiEvent::TransferOffer { xfer_id: id, .. } if *id == xfer_id),
        10,
    )
    .await
    .expect("B 未收到 TransferOffer");
    let UiEvent::TransferOffer {
        node_id,
        name,
        size,
        ..
    } = offer
    else {
        panic!("事件类型不符")
    };
    assert_eq!(node_id, a.node_id);
    assert_eq!(name, "a.bin");
    assert_eq!(size, 1024);

    b.reject_file(xfer_id, "user declined");

    let rejected = wait_event(
        &a.tx,
        |e| matches!(e, UiEvent::TransferRejected { xfer_id: id, .. } if *id == xfer_id),
        10,
    )
    .await
    .expect("A 未收到 TransferRejected");
    match rejected {
        UiEvent::TransferRejected { reason, .. } => assert_eq!(reason, "user declined"),
        _ => unreachable!(),
    }

    stop(a);
    stop(b);
}
