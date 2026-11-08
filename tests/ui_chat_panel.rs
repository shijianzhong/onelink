mod common;

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::TestAppContext;
use gpui_kit::prelude::*;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    Context, Entity, IntoElement, ParentElement, Render, Styled, Subscription, Window, div,
};
use onelink::chat::{ChatMessage, MessageStatus};
use onelink::ui::chat_panel::{ChatPanel, ChatPanelEvent};

fn msg(id: u64, text: &str, mine: bool) -> ChatMessage {
    ChatMessage {
        id,
        text: text.into(),
        sent_at_ms: id,
        mine,
        status: MessageStatus::Pending,
        read: false,
    }
}

/// 测试宿主：包一层持有订阅，避免 Subscription 被 drop 后事件收不到。
struct Host {
    panel: Entity<ChatPanel>,
    _sub: Subscription,
}

impl Render for Host {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.panel.clone())
    }
}

#[gpui_kit::test]
fn renders_messages_of_selected_peer(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (handle, _host) = common::open_window(cx, None, |window, cx| {
        cx.new(|cx| {
            let panel = cx.new(|cx| {
                let mut p = ChatPanel::new(window, cx);
                p.show(
                    "peer-b",
                    vec![msg(1, "你好", true), msg(2, "在的", false)],
                    cx,
                );
                p
            });
            let sub = cx.subscribe(&panel, |_h, _p, _ev: &ChatPanelEvent, _cx| {});
            Host { panel, _sub: sub }
        })
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        let first = window
            .find("msg-mine-1")
            .label()
            .unwrap_or_default()
            .to_string();
        assert_eq!(first, "你好");
        let second = window
            .find("msg-peer-2")
            .label()
            .unwrap_or_default()
            .to_string();
        assert_eq!(second, "在的");
        let header = window
            .find("chat-header")
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(
            header.contains("peer-b") || header.contains("与"),
            "header was {header}"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn send_button_emits_text_and_clears_input(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let events: Rc<RefCell<Vec<ChatPanelEvent>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&events);

    let (handle, host) = common::open_window(cx, None, move |window, cx| {
        cx.new(move |cx| {
            let panel = cx.new(|cx| {
                let mut p = ChatPanel::new(window, cx);
                p.show("peer-b", Vec::new(), cx);
                p
            });
            let sub = cx.subscribe(&panel, move |_h, _p, ev: &ChatPanelEvent, _cx| {
                sink.borrow_mut().push(ev.clone());
            });
            Host { panel, _sub: sub }
        })
    });

    cx.update_window(handle.into(), |_, window, cx| {
        host.update(cx, |h, cx| {
            h.panel
                .update(cx, |p, cx| p.set_input_text("hi", window, cx));
        });
        window.draw(cx).clear(cx);
        window.click("chat-send", cx);
    })
    .unwrap();

    assert_eq!(*events.borrow(), vec![ChatPanelEvent::Send("hi".into())]);
    cx.update_window(handle.into(), |_, _, cx| {
        let left = host.read(cx).panel.read(cx).input_text(cx);
        assert!(left.is_empty(), "input should be cleared, got {left:?}");
    })
    .unwrap();
}

#[gpui_kit::test]
fn empty_input_does_not_send(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let events: Rc<RefCell<Vec<ChatPanelEvent>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&events);

    let (handle, _host) = common::open_window(cx, None, move |window, cx| {
        cx.new(move |cx| {
            let panel = cx.new(|cx| {
                let mut p = ChatPanel::new(window, cx);
                p.show("peer-b", Vec::new(), cx);
                p
            });
            let sub = cx.subscribe(&panel, move |_h, _p, ev: &ChatPanelEvent, _cx| {
                sink.borrow_mut().push(ev.clone());
            });
            Host { panel, _sub: sub }
        })
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        window.click("chat-send", cx);
    })
    .unwrap();

    assert!(events.borrow().is_empty(), "empty input must not send");
}

#[gpui_kit::test]
fn same_numeric_id_from_each_side_renders_distinct_bubbles(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (handle, _host) = common::open_window(cx, None, |window, cx| {
        cx.new(|cx| {
            let panel = cx.new(|cx| {
                let mut p = ChatPanel::new(window, cx);
                // 双方计数器独立，同一会话里出现相同数字 id 是正常现象
                p.show(
                    "peer-b",
                    vec![msg(1, "我发的", true), msg(1, "他发的", false)],
                    cx,
                );
                p
            });
            let sub = cx.subscribe(&panel, |_h, _p, _ev: &ChatPanelEvent, _cx| {});
            Host { panel, _sub: sub }
        })
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        let mine = window
            .try_find("msg-mine-1")
            .expect("mine bubble must exist")
            .label()
            .unwrap_or_default()
            .to_string();
        assert_eq!(mine, "我发的");
        let theirs = window
            .try_find("msg-peer-1")
            .expect("peer bubble must exist")
            .label()
            .unwrap_or_default()
            .to_string();
        assert_eq!(theirs, "他发的");
    })
    .unwrap();
}

#[gpui_kit::test]
fn composer_and_bubbles_use_roomy_im_metrics(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (handle, _host) = common::open_window(cx, None, |window, cx| {
        cx.new(|cx| {
            let panel = cx.new(|cx| {
                let mut p = ChatPanel::new(window, cx);
                p.show("peer-b", vec![msg(1, "你好", true)], cx);
                p
            });
            let sub = cx.subscribe(&panel, |_h, _p, _ev: &ChatPanelEvent, _cx| {});
            Host { panel, _sub: sub }
        })
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        // 输入区：多行文本域至少 112px 高，明显大于旧版单行输入。
        let input = window.find("chat-input").bounds();
        assert!(
            f32::from(input.size.height) >= 100.,
            "input height {input:?}"
        );
        // 发送按钮固定 76×32，与附件按钮同一行。
        let send = window.find("chat-send").bounds();
        assert_eq!(f32::from(send.size.width), 76.);
        assert_eq!(f32::from(send.size.height), 32.);
        // 气泡宽度受 460px 上限约束，不会铺满整个聊天区。
        let bubble = window.find("msg-mine-1").bounds();
        assert!(f32::from(bubble.size.width) <= 460., "bubble {bubble:?}");
        // 时间戳弱化展示（msg(1, …) 的 sent_at_ms 是 1，对应东八区 08:00）。
        let time = window
            .find("msg-time-mine-1")
            .label()
            .unwrap_or_default()
            .to_string();
        assert_eq!(time, "08:00");
    })
    .unwrap();
}

#[gpui_kit::test]
fn enter_key_sends_message(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let events: Rc<RefCell<Vec<ChatPanelEvent>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&events);

    let (handle, _host) = common::open_window(cx, None, move |window, cx| {
        cx.new(move |cx| {
            let panel = cx.new(|cx| {
                let mut p = ChatPanel::new(window, cx);
                p.show("peer-b", Vec::new(), cx);
                p
            });
            let sub = cx.subscribe(&panel, move |_h, _p, ev: &ChatPanelEvent, _cx| {
                sink.borrow_mut().push(ev.clone());
            });
            Host { panel, _sub: sub }
        })
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        window.click("chat-input", cx);
        window.draw(cx).clear(cx);
        window.input("回车发送", cx);
        window.press("enter", cx);
    })
    .unwrap();

    assert_eq!(
        *events.borrow(),
        vec![ChatPanelEvent::Send("回车发送".into())]
    );
}

#[gpui_kit::test]
fn error_hint_shows_and_clears(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (handle, host) = common::open_window(cx, None, |window, cx| {
        cx.new(|cx| {
            let panel = cx.new(|cx| {
                let mut p = ChatPanel::new(window, cx);
                p.show("peer-b", Vec::new(), cx);
                p
            });
            let sub = cx.subscribe(&panel, |_h, _p, _ev: &ChatPanelEvent, _cx| {});
            Host { panel, _sub: sub }
        })
    });

    cx.update_window(handle.into(), |_, window, cx| {
        host.update(cx, |h, cx| {
            let panel = h.panel.clone();
            panel.update(cx, |p, cx| {
                p.set_error(Some("send timed out after 5s".into()), cx)
            });
        });
        window.draw(cx).clear(cx);
        let hint = window
            .find("chat-error")
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(hint.contains("send timed out"), "hint was {hint:?}");
    })
    .unwrap();

    cx.update_window(handle.into(), |_, window, cx| {
        host.update(cx, |h, cx| {
            let panel = h.panel.clone();
            panel.update(cx, |p, cx| p.set_error(None, cx));
        });
        window.draw(cx).clear(cx);
        assert!(
            window.try_find("chat-error").is_none(),
            "hint should be gone"
        );
    })
    .unwrap();
}
