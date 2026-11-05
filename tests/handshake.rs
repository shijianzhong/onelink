use onelink::model::{DeviceRegistry, apply_ui_event};
use onelink::net::{HelloMsg, UiEvent, handshake_name, serve_handshake};

#[test]
fn hello_msg_roundtrips_json() {
    let m = HelloMsg::Hello {
        name: "书房Mac".into(),
    };
    let bytes = serde_json::to_vec(&m).unwrap();
    let back: HelloMsg = serde_json::from_slice(&bytes).unwrap();
    assert!(matches!(back, HelloMsg::Hello { name } if name == "书房Mac"));
}

#[test]
fn registry_applies_named_event() {
    let mut reg = DeviceRegistry::new();
    let changes = apply_ui_event(
        &mut reg,
        UiEvent::DeviceNamed {
            node_id: "n1".into(),
            name: "云主机".into(),
        },
    );
    assert_eq!(changes.len(), 1);
    let snap = reg.snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].name, "云主机");
    assert!(snap[0].online);
}

#[test]
fn lost_event_marks_device_offline_not_removed() {
    let mut reg = DeviceRegistry::new();
    apply_ui_event(
        &mut reg,
        UiEvent::DeviceNamed {
            node_id: "n1".into(),
            name: "云主机".into(),
        },
    );
    apply_ui_event(
        &mut reg,
        UiEvent::DeviceLost {
            node_id: "n1".into(),
        },
    );
    let snap = reg.snapshot();
    assert_eq!(snap.len(), 1);
    assert!(!snap[0].online);
}

#[test]
fn discovered_event_creates_placeholder_entry() {
    // 握手完成前先用 node id 前缀占位，UI 立刻有反馈
    let mut reg = DeviceRegistry::new();
    apply_ui_event(
        &mut reg,
        UiEvent::DeviceDiscovered {
            node_id: "120a34ba275d04fd".into(),
            addr: Some("ip:192.168.1.5:9000".into()),
        },
    );
    let snap = reg.snapshot();
    assert_eq!(snap.len(), 1);
    assert!(snap[0].online);
    assert!(snap[0].name.starts_with("120a34"));
    assert_eq!(snap[0].addr.as_deref(), Some("ip:192.168.1.5:9000"));
}

#[test]
fn named_event_keeps_addr_from_discovery() {
    let mut reg = DeviceRegistry::new();
    apply_ui_event(
        &mut reg,
        UiEvent::DeviceDiscovered {
            node_id: "n1".into(),
            addr: Some("ip:192.168.1.5:9000".into()),
        },
    );
    apply_ui_event(
        &mut reg,
        UiEvent::DeviceNamed {
            node_id: "n1".into(),
            name: "书房Mac".into(),
        },
    );
    let snap = reg.snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].name, "书房Mac");
    assert_eq!(snap[0].addr.as_deref(), Some("ip:192.168.1.5:9000"));
}

#[tokio::test]
async fn two_endpoints_exchange_names_over_stream() {
    use iroh::endpoint::presets;
    use onelink::net::ALPN;

    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_test_writer()
        .try_init();

    fn builder() -> iroh::endpoint::Builder {
        iroh::Endpoint::builder(presets::Minimal).alpns(vec![ALPN.to_vec()])
    }

    let a = builder().bind().await.unwrap();
    let b = builder().bind().await.unwrap();
    let b_addr = b.addr();

    let b_name = "云主机".to_string();
    let b_shutdown = b.clone();
    let server = tokio::spawn(async move { serve_handshake(&b, &b_name).await });

    let got = handshake_name(&a, b_addr, "书房Mac").await.unwrap();
    assert_eq!(got, "云主机");

    // 优雅收尾：关掉两个 endpoint，让 serve 循环自然结束
    a.close().await;
    b_shutdown.close().await;
    let _ = server.await;
}
