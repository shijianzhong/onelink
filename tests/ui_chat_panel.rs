mod common;

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::TestAppContext;
use gpui_kit::base::TextSelection;
use gpui_kit::prelude::*;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    Context, Entity, IntoElement, ParentElement, Render, ScrollDelta, Styled, Subscription, Window,
    div, point, px, size,
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

/// 在气泡内从左到右拖选，覆盖整条正文文字。
fn select_bubble_text(window: &mut Window, id: &'static str, cx: &mut gpui_kit::App) {
    let bubble = window.find(id).bounds();
    let y = bubble.origin.y + bubble.size.height * 0.5;
    let from = point(bubble.origin.x + px(13.), y);
    let to = point(bubble.origin.x + bubble.size.width - px(13.), y);
    window.drag(from, to, cx);
    window.draw(cx).clear(cx);
}

#[gpui_kit::test]
fn message_text_is_selectable_and_copyable(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (handle, _host) = common::open_window(cx, None, |window, cx| {
        cx.new(|cx| {
            let panel = cx.new(|cx| {
                let mut p = ChatPanel::new(window, cx);
                p.show("peer-b", vec![msg(2, "在的", false)], cx);
                p
            });
            let sub = cx.subscribe(&panel, |_h, _p, _ev: &ChatPanelEvent, _cx| {});
            Host { panel, _sub: sub }
        })
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        select_bubble_text(window, "msg-peer-2", cx);
        assert_eq!(
            TextSelection::selected_text(window, cx),
            "在的",
            "selected text should be the whole message"
        );

        // 选中消息会把焦点移到消息区，Cmd+C 走 ChatPanel 的复制绑定写剪贴板。
        window.press("cmd-c", cx);
        let copied = cx
            .read_from_clipboard()
            .and_then(|item| item.text().map(|text| text.to_string()));
        assert_eq!(copied.as_deref(), Some("在的"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn selecting_message_keeps_enter_sending(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let events: Rc<RefCell<Vec<ChatPanelEvent>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&events);

    let (handle, host) = common::open_window(cx, None, move |window, cx| {
        cx.new(move |cx| {
            let panel = cx.new(|cx| {
                let mut p = ChatPanel::new(window, cx);
                p.show("peer-b", vec![msg(2, "在的", false)], cx);
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
        window.input("hi", cx);
        window.draw(cx).clear(cx);

        // 选中消息文字后焦点离开输入框，但 Enter 仍必须能发送草稿。
        select_bubble_text(window, "msg-peer-2", cx);
        window.press("enter", cx);
    })
    .unwrap();

    assert_eq!(
        *events.borrow(),
        vec![ChatPanelEvent::Send("hi".into())],
        "Enter must still send after selecting message text"
    );
    cx.update_window(handle.into(), |_, _, cx| {
        let left = host.read(cx).panel.read(cx).input_text(cx);
        assert!(left.is_empty(), "input should be cleared, got {left:?}");
    })
    .unwrap();
}

/// hover 复制：只在该行出现弱化入口，点击复制整条消息（含换行），
/// 且按钮出现/消失都不改变气泡位置。
#[gpui_kit::test]
fn hover_copy_button_copies_whole_message(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (handle, _host) = common::open_window(cx, None, |window, cx| {
        cx.new(|cx| {
            let panel = cx.new(|cx| {
                let mut p = ChatPanel::new(window, cx);
                p.show(
                    "peer-b",
                    vec![msg(1, "我发的", true), msg(2, "第一行\n第二行", false)],
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
        // 未 hover 时不存在任何复制入口，避免常驻大按钮。
        assert!(
            window.try_find("msg-copy-peer-2").is_none(),
            "copy button must stay hidden before hover"
        );
        let bubble_before = window.find("msg-peer-2").bounds();

        window.hover("msg-row-peer-2", cx);
        window.draw(cx).clear(cx);

        let chip = window
            .try_find("msg-copy-peer-2")
            .expect("copy button appears on hover");
        assert_eq!(chip.label().unwrap_or_default(), "复制");
        // 只针对 hover 的那一行，别的消息不显示入口。
        assert!(
            window.try_find("msg-copy-mine-1").is_none(),
            "only the hovered row shows a copy button"
        );
        // 绝对定位的按钮不参与布局：气泡位置与尺寸前后完全一致。
        assert_eq!(
            bubble_before,
            window.find("msg-peer-2").bounds(),
            "copy button must not shift the bubble"
        );

        window.click("msg-copy-peer-2", cx);
        let copied = cx
            .read_from_clipboard()
            .and_then(|item| item.text().map(|text| text.to_string()));
        assert_eq!(
            copied.as_deref(),
            Some("第一行\n第二行"),
            "hover copy must copy the whole message text, newline included"
        );

        // 极轻反馈：按钮短暂变「已复制」，不弹窗、不通知。
        window.draw(cx).clear(cx);
        assert_eq!(
            window.find("msg-copy-peer-2").label().unwrap_or_default(),
            "已复制"
        );

        // 鼠标离开该行后入口消失，阅读区恢复干净。
        window.hover("chat-header", cx);
        window.draw(cx).clear(cx);
        assert!(
            window.try_find("msg-copy-peer-2").is_none(),
            "copy button disappears when hover leaves the row"
        );
    })
    .unwrap();
}

/// hover 复制与拖选共存：先 hover 复制整条，再拖选 + Cmd+C 复制选区，
/// 两条路径互不干扰。
#[gpui_kit::test]
fn hover_copy_and_drag_selection_coexist(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (handle, _host) = common::open_window(cx, None, |window, cx| {
        cx.new(|cx| {
            let panel = cx.new(|cx| {
                let mut p = ChatPanel::new(window, cx);
                p.show("peer-b", vec![msg(2, "在的", false)], cx);
                p
            });
            let sub = cx.subscribe(&panel, |_h, _p, _ev: &ChatPanelEvent, _cx| {});
            Host { panel, _sub: sub }
        })
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        window.hover("msg-row-peer-2", cx);
        window.draw(cx).clear(cx);
        window.click("msg-copy-peer-2", cx);
        window.draw(cx).clear(cx);
        let whole = cx
            .read_from_clipboard()
            .and_then(|item| item.text().map(|text| text.to_string()));
        assert_eq!(whole.as_deref(), Some("在的"));

        // 拖选仍然能选中正文并 Cmd+C（按钮未拦截指针）。
        select_bubble_text(window, "msg-peer-2", cx);
        assert_eq!(TextSelection::selected_text(window, cx), "在的");
        window.press("cmd-c", cx);
        let selected = cx
            .read_from_clipboard()
            .and_then(|item| item.text().map(|text| text.to_string()));
        assert_eq!(selected.as_deref(), Some("在的"));
    })
    .unwrap();
}

/// 消息多到超出可视高度时必须能滚动：默认贴底（能看到最新一条），
/// 向上滚动后能回到更早的消息。
#[gpui_kit::test]
fn long_message_list_scrolls_to_latest_and_back(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (handle, _host) = common::open_window(cx, Some(size(px(700.), px(400.))), |window, cx| {
        cx.new(|cx| {
            let panel = cx.new(|cx| {
                let mut p = ChatPanel::new(window, cx);
                let messages: Vec<ChatMessage> = (1..=40)
                    .map(|id| msg(id, &format!("消息 {id}"), false))
                    .collect();
                p.show("peer-b", messages, cx);
                p
            });
            let sub = cx.subscribe(&panel, |_h, _p, _ev: &ChatPanelEvent, _cx| {});
            Host { panel, _sub: sub }
        })
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        assert!(
            window.find("msg-peer-40").visible(),
            "newest message should be visible after opening (scrolled to bottom)"
        );
        assert!(
            !window
                .try_find("msg-peer-1")
                .expect("row registered")
                .visible(),
            "earliest message must be out of view before scrolling"
        );

        // 向上滚回顶部，最早的消息重新可见。
        window.scroll(
            "chat-messages",
            ScrollDelta::Pixels(point(px(0.), px(20000.))),
            cx,
        );
        window.draw(cx).clear(cx);
        assert!(
            window.find("msg-peer-1").visible(),
            "scrolling up must bring the earliest message back into view"
        );

        // 有溢出时滚动条是可见且可交互的：点轨道底部回到最新消息。
        let list = window.find("chat-messages").bounds();
        window.click_at(
            "chat-messages",
            point(list.size.width - px(4.), list.size.height - px(8.)),
            cx,
        );
        window.draw(cx).clear(cx);
        assert!(
            window.find("msg-peer-40").visible(),
            "scrollbar track click must jump back to the newest message"
        );
    })
    .unwrap();
}

/// 空会话：没有消息时既不能出现滚动条，也不能有奇怪空白——提示语在可视区垂直居中。
#[gpui_kit::test]
fn empty_conversation_has_no_scrollbar(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (handle, _host) = common::open_window(cx, Some(size(px(600.), px(420.))), |window, cx| {
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
        window.draw(cx).clear(cx);
        window.draw(cx).clear(cx);
        let list = window.find("chat-messages").bounds();
        let hint = window.find("chat-empty").bounds();

        // 提示语垂直居中：内容刚好铺满可视高度，不会撑出滚动范围。
        let list_center = list.origin.y + list.size.height / 2.;
        let hint_center = hint.origin.y + hint.size.height / 2.;
        assert!(
            (f32::from(hint_center - list_center)).abs() <= 1.,
            "empty hint should be vertically centered: hint {hint:?}, list {list:?}"
        );

        // 滚动条轨道位于列表右缘内侧：不可滚动时，滚轮与轨道点击都不该让内容移动。
        window.scroll(
            "chat-messages",
            ScrollDelta::Pixels(point(px(0.), px(-1000.))),
            cx,
        );
        window.draw(cx).clear(cx);
        assert_eq!(
            hint,
            window.find("chat-empty").bounds(),
            "empty conversation must not scroll"
        );

        window.click_at(
            "chat-messages",
            point(list.size.width - px(4.), list.size.height / 2.),
            cx,
        );
        window.draw(cx).clear(cx);
        assert_eq!(
            hint,
            window.find("chat-empty").bounds(),
            "empty conversation must not have a draggable scrollbar track"
        );
    })
    .unwrap();
}

/// 少量消息（不满一屏）：同样不该出现滚动条，消息位置不能被滚轮/轨道点击推动。
#[gpui_kit::test]
fn short_conversation_has_no_scrollbar(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (handle, _host) = common::open_window(cx, Some(size(px(600.), px(420.))), |window, cx| {
        cx.new(|cx| {
            let panel = cx.new(|cx| {
                let mut p = ChatPanel::new(window, cx);
                p.show(
                    "peer-b",
                    vec![msg(1, "第一条", false), msg(2, "第二条", true)],
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
        window.draw(cx).clear(cx);
        assert!(
            window.try_find("chat-empty").is_none(),
            "messages present, no empty hint"
        );
        let list = window.find("chat-messages").bounds();
        let first = window.find("msg-peer-1").bounds();

        window.scroll(
            "chat-messages",
            ScrollDelta::Pixels(point(px(0.), px(-1000.))),
            cx,
        );
        window.draw(cx).clear(cx);
        assert_eq!(
            first,
            window.find("msg-peer-1").bounds(),
            "short conversation must not scroll"
        );

        window.click_at(
            "chat-messages",
            point(list.size.width - px(4.), list.size.height / 2.),
            cx,
        );
        window.draw(cx).clear(cx);
        assert_eq!(
            first,
            window.find("msg-peer-1").bounds(),
            "short conversation must not have a draggable scrollbar track"
        );
    })
    .unwrap();
}
