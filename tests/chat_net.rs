use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::presets;
use onelink::net::{ALPN, EVENT_QUEUE_CAPACITY, EventQueue, UiEvent, XferState, serve_frames};
use onelink::proto::{Frame, MAX_FRAME_BYTES, decode_frame, encode_frame};
use tokio::io::AsyncWriteExt;

fn builder() -> iroh::endpoint::Builder {
    iroh::Endpoint::builder(presets::Minimal).alpns(vec![ALPN.to_vec()])
}

#[tokio::test]
async fn chat_frame_reaches_event_queue_and_gets_ack() {
    let a = builder().bind().await.unwrap();
    let b = builder().bind().await.unwrap();
    let queue = EventQueue::new(EVENT_QUEUE_CAPACITY);

    let q = Arc::clone(&queue);
    let a_serve = a.clone();
    let xfers = Arc::new(XferState::new());
    let x = Arc::clone(&xfers);
    let server = tokio::spawn(async move {
        let _ = serve_frames(&a_serve, "服务端A", &q, &x).await;
    });

    let conn = b.connect(a.addr(), ALPN).await.unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    let f = Frame::Chat {
        id: 42,
        text: "hi".into(),
        sent_at_ms: 1,
    };
    send.write_all(&encode_frame(&f).unwrap()).await.unwrap();
    send.shutdown().await.unwrap();

    let ack = decode_frame(
        &recv.read_to_end(MAX_FRAME_BYTES).await.unwrap(),
        MAX_FRAME_BYTES,
    )
    .unwrap();
    assert_eq!(ack, Frame::ChatAck { id: 42 });

    // 事件是异步入队的，轮询等它出现
    let mut got = None;
    for _ in 0..100 {
        if let Some(UiEvent::ChatReceived {
            node_id,
            id,
            text,
            sent_at_ms,
        }) = queue.try_recv()
        {
            got = Some((node_id, id, text, sent_at_ms));
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (node_id, id, text, sent_at_ms) = got.expect("no ChatReceived event");
    assert_eq!(node_id, b.id().to_string(), "事件要带上发送方 node id");
    assert_eq!((id, text.as_str(), sent_at_ms), (42, "hi", 1));

    b.close().await;
    a.close().await;
    let _ = server.await;
}

#[tokio::test]
async fn hello_still_answered_by_frame_dispatch() {
    // M1 的握手行为不能因为换成 serve_frames 而破
    let a = builder().bind().await.unwrap();
    let b = builder().bind().await.unwrap();
    let queue = EventQueue::new(EVENT_QUEUE_CAPACITY);

    let q = Arc::clone(&queue);
    let a_serve = a.clone();
    let xfers = Arc::new(XferState::new());
    let x = Arc::clone(&xfers);
    let server = tokio::spawn(async move {
        let _ = serve_frames(&a_serve, "服务端A", &q, &x).await;
    });

    let got = onelink::net::handshake_name(&b, a.addr(), "客户端B", None)
        .await
        .unwrap();
    assert_eq!(got.name, "服务端A");

    b.close().await;
    a.close().await;
    let _ = server.await;
}
