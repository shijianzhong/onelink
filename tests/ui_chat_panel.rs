mod common;

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::TestAppContext;
use gpui_kit::prelude::*;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    Context, Entity, IntoElement, ParentElement, Render, Styled, Subscription, Window, div,
};
use onelink::chat::ChatMessage;
use onelink::ui::chat_panel::{ChatPanel, ChatPanelEvent};

fn msg(id: u64, text: &str, mine: bool) -> ChatMessage {
    ChatMessage {
        id,
        text: text.into(),
        sent_at_ms: id,
        mine,
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
