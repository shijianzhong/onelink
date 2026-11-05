//! 传输行 UI 测试：进度百分比、接受/拒绝事件、📎 按钮、失败原因。

mod common;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    Context, Entity, ExternalPaths, FileDropEvent, InputEvent, IntoElement, ParentElement, Render,
    Styled, Subscription, TestAppContext, Window, WindowHandle, base::Root, div,
};
use onelink::ui::chat_panel::{ChatPanel, ChatPanelEvent};
use onelink::ui::transfer_row::{TransferState, TransferView};

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

fn view(xfer_id: u64, name: &str, state: TransferState) -> TransferView {
    TransferView {
        xfer_id,
        name: name.into(),
        state,
    }
}

fn open_panel(
    cx: &mut TestAppContext,
    transfers: Vec<TransferView>,
) -> (
    WindowHandle<Root>,
    Entity<Host>,
    Rc<RefCell<Vec<ChatPanelEvent>>>,
) {
    let events: Rc<RefCell<Vec<ChatPanelEvent>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&events);
    let (handle, host) = common::open_window(cx, None, move |window, cx| {
        cx.new(move |cx| {
            let panel = cx.new(|cx| {
                let mut p = ChatPanel::new(window, cx);
                p.show("peer-b", Vec::new(), cx);
                p.set_transfers(transfers.clone(), cx);
                p
            });
            let sub = cx.subscribe(&panel, move |_h, _p, ev: &ChatPanelEvent, _cx| {
                sink.borrow_mut().push(ev.clone());
            });
            Host { panel, _sub: sub }
        })
    });
    (handle, host, events)
}

#[gpui_kit::test]
fn progress_row_shows_percent_text(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (handle, _host, _events) =
        open_panel(cx, vec![view(1, "a.bin", TransferState::Active(0.42))]);

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        let text = window
            .find("xfer-progress-1")
            .label()
            .unwrap_or_default()
            .to_string();
        assert_eq!(text, "42%");
    })
    .unwrap();
}

#[gpui_kit::test]
fn offer_row_accept_and_reject_emit_ids(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (handle, _host, events) = open_panel(
        cx,
        vec![view(1, "a.bin", TransferState::Offer { size: 1024 })],
    );

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        window.click("xfer-accept-1", cx);
    })
    .unwrap();
    assert_eq!(*events.borrow(), vec![ChatPanelEvent::AcceptFile(1)]);

    events.borrow_mut().clear();
    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        window.click("xfer-reject-1", cx);
    })
    .unwrap();
    assert_eq!(*events.borrow(), vec![ChatPanelEvent::RejectFile(1)]);
}

#[gpui_kit::test]
fn attach_button_emits_pick_file(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (handle, _host, events) = open_panel(cx, Vec::new());

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        window.click("chat-attach", cx);
    })
    .unwrap();
    assert_eq!(*events.borrow(), vec![ChatPanelEvent::PickFile]);
}

#[gpui_kit::test]
fn dropping_files_emits_drop_files(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (handle, _host, events) = open_panel(cx, Vec::new());
    let path = PathBuf::from("/tmp/onelink-drop.txt");

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        let position = window.find("chat-panel").bounds().center();
        let paths = ExternalPaths([path.clone()].into_iter().collect());
        window.dispatch_event(
            FileDropEvent::Entered { position, paths }.to_platform_input(),
            cx,
        );
        window.dispatch_event(FileDropEvent::Submit { position }.to_platform_input(), cx);
    })
    .unwrap();

    assert_eq!(
        *events.borrow(),
        vec![ChatPanelEvent::DropFiles(vec![path])]
    );
}

#[gpui_kit::test]
fn failed_row_shows_reason_text(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (handle, _host, _events) = open_panel(
        cx,
        vec![view(
            1,
            "a.bin",
            TransferState::Failed("hash mismatch".into()),
        )],
    );

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        let text = window
            .find("xfer-state-1")
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(text.contains("hash mismatch"), "state was {text:?}");
    })
    .unwrap();
}
