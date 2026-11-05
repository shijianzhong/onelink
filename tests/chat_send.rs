use std::sync::Arc;
use std::time::{Duration, Instant};

use iroh::endpoint::presets;
use onelink::net::{
    ALPN, EVENT_QUEUE_CAPACITY, EventQueue, UiEvent, XferState, send_chat_frame, serve_frames,
    shutdown_iroh_service, spawn_iroh_service,
};
use onelink::proto::{Frame, now_ms};

fn builder() -> iroh::endpoint::Builder {
    iroh::Endpoint::builder(presets::Minimal).alpns(vec![ALPN.to_vec()])
}

fn pop_until(deadline: Instant, queue: &EventQueue, want: fn(&UiEvent) -> bool) -> Option<UiEvent> {
    while Instant::now() < deadline {
        if let Some(ev) = queue.try_recv()
            && want(&ev)
        {
            return Some(ev);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

#[tokio::test]
async fn send_chat_frame_delivers_and_gets_ack() {
    let a = builder().bind().await.unwrap();
    let b = builder().bind().await.unwrap();
    let queue = EventQueue::new(EVENT_QUEUE_CAPACITY);

    let q = Arc::clone(&queue);
    let b_serve = b.clone();
    let xfers = Arc::new(XferState::new());
    let x = Arc::clone(&xfers);
    let server = tokio::spawn(async move {
        let _ = serve_frames(&b_serve, "对端B", &q, &x).await;
    });

    let f = Frame::Chat {
        id: 7,
        text: "你好".into(),
        sent_at_ms: now_ms(),
    };
    send_chat_frame(&a, b.addr(), &f).await.unwrap();

    // 对端入队的 ChatReceived 是异步的
    let mut got = None;
    for _ in 0..100 {
        if let Some(UiEvent::ChatReceived { id, text, .. }) = queue.try_recv() {
            got = Some((id, text));
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(got, Some((7, "你好".to_string())));

    b.close().await;
    a.close().await;
    let _ = server.await;
}

#[test]
fn handle_send_chat_reports_failure_for_unreachable_peer() {
    let handle = spawn_iroh_service().expect("spawn service");
    // 一个几乎不可能存在的 node id（64 个 0）：connect 会因为没有可用地址而失败
    let dead_peer = "0".repeat(64);
    let id = handle.send_chat(&dead_peer, "anyone there?");

    let got = pop_until(Instant::now() + Duration::from_secs(10), &handle.tx, |ev| {
        matches!(ev, UiEvent::ChatFailed { .. })
    });
    let Some(UiEvent::ChatFailed {
        node_id,
        id: got_id,
        reason,
    }) = got
    else {
        panic!("expected ChatFailed event within 10s");
    };
    assert_eq!(got_id, id, "失败事件要回带同一条消息 id");
    assert_eq!(node_id, dead_peer);
    assert!(!reason.is_empty(), "失败原因不能为空");

    shutdown_iroh_service(&handle);
}

#[test]
fn oversized_text_is_rejected_without_network_attempt() {
    let handle = spawn_iroh_service().expect("spawn service");
    // 不存在的对端：若真去连会失败；这里断言失败原因来自本地长度校验。
    let dead = iroh::EndpointAddr::from("0".repeat(64).parse::<iroh::EndpointId>().unwrap());
    let too_long = "a".repeat(onelink::proto::MAX_FRAME_BYTES + 1);

    let id = handle.send_chat_to(dead, &too_long);

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut failure = None;
    while Instant::now() < deadline && failure.is_none() {
        while let Some(ev) = handle.tx.try_recv() {
            if let UiEvent::ChatFailed {
                id: failed_id,
                reason,
                ..
            } = ev
                && failed_id == id
            {
                failure = Some(reason);
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    shutdown_iroh_service(&handle);

    assert_eq!(failure.as_deref(), Some("message too large"));
}
