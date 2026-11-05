use onelink::proto::{Frame, MAX_FRAME_BYTES, decode_frame, encode_frame};

#[test]
fn roundtrips_all_variants() {
    let frames = [
        Frame::Hello { name: "A".into() },
        Frame::HelloAck { name: "B".into() },
        Frame::Chat { id: 7, text: "你好".into(), sent_at_ms: 1700000000000 },
        Frame::ChatAck { id: 7 },
    ];
    for f in frames {
        let bytes = encode_frame(&f).unwrap();
        assert_eq!(decode_frame(&bytes, MAX_FRAME_BYTES).unwrap(), f);
    }
}

#[test]
fn hello_wire_format_stays_m1_compatible() {
    // M1 的 HelloMsg 就是 {"Hello":{"name":"x"}}，M2 必须沿用，否则旧版本互通会断
    let bytes = encode_frame(&Frame::Hello { name: "x".into() }).unwrap();
    assert_eq!(String::from_utf8(bytes).unwrap(), r#"{"Hello":{"name":"x"}}"#);
}

#[test]
fn rejects_frame_over_limit() {
    // 编码侧：超限帧不允许被发送
    let oversized = Frame::Chat {
        id: 1,
        text: "a".repeat(MAX_FRAME_BYTES),
        sent_at_ms: 1,
    };
    let err = encode_frame(&oversized).unwrap_err();
    assert!(err.to_string().contains("too large"), "{err}");

    // 解码侧：对端发来的超限字节在读入后立刻被拒，不尝试解析
    let big = vec![b'a'; MAX_FRAME_BYTES + 1];
    let err = decode_frame(&big, MAX_FRAME_BYTES).unwrap_err();
    assert!(err.to_string().contains("too large"), "{err}");
}

#[test]
fn rejects_garbage_json() {
    assert!(decode_frame(b"not json", MAX_FRAME_BYTES).is_err());
    assert!(decode_frame(b"", MAX_FRAME_BYTES).is_err());
}
