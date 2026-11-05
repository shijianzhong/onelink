//! M2 端到端验收脚本（不是常规单测）：真实起两个 iroh 服务，走
//! mDNS 发现 → 握手换名 → 收发一条聊天消息的完整链路，不依赖 GPUI 点击。
//!
//! 用法：`cargo run --example e2e_chat`
//! 成功输出 `PASS` 并返回 0；任一步失败输出 `FAIL: ...` 并返回 1。

use std::sync::Arc;
use std::time::{Duration, Instant};

use onelink::net::{UiEvent, shutdown_iroh_service, spawn_iroh_service};

const DISCOVER_TIMEOUT: Duration = Duration::from_secs(40);
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);

fn drain(queue: &Arc<onelink::net::EventQueue>, mut handle: impl FnMut(UiEvent)) {
    while let Some(ev) = queue.try_recv() {
        handle(ev);
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let a = spawn_iroh_service().expect("spawn service A");
    let b = spawn_iroh_service().expect("spawn service B");
    println!("[e2e] two services up");

    // 1) 等 A 发现并命名 B（DeviceNamed 表示握手已完成，拿到的是对端真实名字）。
    let deadline = Instant::now() + DISCOVER_TIMEOUT;
    let mut peer_of_a: Option<String> = None;
    while Instant::now() < deadline && peer_of_a.is_none() {
        drain(&a.tx, |ev| {
            println!("[e2e] A <- {ev:?}");
            if let UiEvent::DeviceNamed { node_id, .. } = &ev
                && peer_of_a.is_none()
            {
                peer_of_a = Some(node_id.clone());
            }
        });
        std::thread::sleep(Duration::from_millis(100));
    }
    let Some(peer) = peer_of_a else {
        println!("FAIL: A did not discover/name B within {DISCOVER_TIMEOUT:?}");
        shutdown_iroh_service(&a);
        shutdown_iroh_service(&b);
        std::process::exit(1);
    };
    println!("[e2e] A sees peer {peer}");

    // 2) A 发一条消息，测 B 收到的时间，并确认 A 拿到 ChatDelivered。
    let sent_at = Instant::now();
    let msg_id = a.send_chat(&peer, "e2e hi");
    println!("[e2e] A sent id={msg_id} at {sent_at:?}");

    let deadline = sent_at + DELIVERY_TIMEOUT;
    let mut received_at: Option<Instant> = None;
    let mut delivered = false;
    let mut failed: Option<String> = None;
    while Instant::now() < deadline && (received_at.is_none() || !delivered) {
        drain(&a.tx, |ev| {
            println!("[e2e] A <- {ev:?}");
            match ev {
                UiEvent::ChatDelivered { id, .. } if id == msg_id => delivered = true,
                UiEvent::ChatFailed { id, reason, .. } if id == msg_id => failed = Some(reason),
                _ => {}
            }
        });
        drain(&b.tx, |ev| {
            println!("[e2e] B <- {ev:?}");
            if let UiEvent::ChatReceived { text, .. } = &ev
                && text == "e2e hi"
                && received_at.is_none()
            {
                received_at = Some(Instant::now());
            }
        });
        std::thread::sleep(Duration::from_millis(20));
    }

    shutdown_iroh_service(&a);
    shutdown_iroh_service(&b);

    match (received_at, delivered, failed) {
        (Some(at), true, _) => {
            let latency = at.duration_since(sent_at);
            println!("PASS: peer received in {latency:?}, sender got ChatDelivered");
        }
        (_, _, Some(reason)) => {
            println!("FAIL: send failed: {reason}");
            std::process::exit(1);
        }
        (None, _, None) => {
            println!("FAIL: peer never received the message within {DELIVERY_TIMEOUT:?}");
            std::process::exit(1);
        }
        (Some(_), false, None) => {
            println!("FAIL: peer received but sender never got ChatDelivered");
            std::process::exit(1);
        }
    }
}
