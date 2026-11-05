use onelink::net::{
    ALPN, EventQueue, MDNS_SERVICE, MdnsEventLike, UiEvent, push_event, translate_mdns_event,
};

/// 测试用事件：模拟 iroh-mdns-address-lookup 的 DiscoveryEvent。
#[derive(Debug)]
enum TestEvent {
    Discovered(String, Option<String>),
    Expired(String),
}

impl MdnsEventLike for TestEvent {
    fn as_discovered(&self) -> Option<(String, Option<String>)> {
        match self {
            TestEvent::Discovered(id, addr) => Some((id.clone(), addr.clone())),
            _ => None,
        }
    }

    fn as_expired(&self) -> Option<String> {
        match self {
            TestEvent::Expired(id) => Some(id.clone()),
            _ => None,
        }
    }
}

#[test]
fn discovered_event_maps_to_ui_event() {
    let evs = translate_mdns_event(&TestEvent::Discovered(
        "n1".into(),
        Some("192.168.1.5:0".into()),
    ));
    assert!(matches!(
        evs[0],
        UiEvent::DeviceDiscovered { ref node_id, .. } if node_id == "n1"
    ));
}

#[test]
fn expired_event_maps_to_lost_event() {
    let evs = translate_mdns_event(&TestEvent::Expired("n1".into()));
    assert!(matches!(
        evs[0],
        UiEvent::DeviceLost { ref node_id } if node_id == "n1"
    ));
}

#[test]
fn bounded_queue_drops_old_when_full() {
    // 容量 2 的队列，塞 5 个事件 → 只保留最后 2 个（满丢旧），不阻塞
    let q = EventQueue::new(2);
    for i in 0..5 {
        push_event(&q, UiEvent::NetError(format!("e{i}")));
    }
    let got: Vec<_> = std::iter::from_fn(|| q.try_recv()).collect();
    assert_eq!(got.len(), 2);
    assert!(matches!(
        got[0],
        UiEvent::NetError(ref s) if s == "e3"
    ));
    assert!(matches!(
        got[1],
        UiEvent::NetError(ref s) if s == "e4"
    ));
}

#[test]
fn alpn_const_is_onelink_v1() {
    assert_eq!(ALPN, b"onelink-v1"); // 连接过滤：非 OneLink 的 iroh 端点会被拒绝
}

#[test]
fn mdns_service_const_is_onelink_v1() {
    assert_eq!(MDNS_SERVICE, "onelink-v1"); // 服务名不一致的端点互不可见
}
