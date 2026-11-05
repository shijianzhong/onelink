//! App 接线测试：点设备行 → 打开会话；发送 → 本地立即入会话模型。

mod common;

use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{App, Entity, TestAppContext};
use onelink::app::App as OneLinkApp;
use onelink::net::{IrohServiceHandle, UiEvent, shutdown_iroh_service, spawn_iroh_service};

/// 打开带真实 iroh 服务的 App 窗口（服务只用其命令/事件通道，不发真实消息）。
fn open_app(
    cx: &mut TestAppContext,
) -> (
    gpui_kit::WindowHandle<gpui_kit::base::Root>,
    Entity<OneLinkApp>,
    Arc<IrohServiceHandle>,
) {
    let service = Arc::new(spawn_iroh_service().expect("spawn iroh service"));
    let (handle, app) = common::open_window(cx, None, {
        let service = Arc::clone(&service);
        move |window, cx| cx.new(|cx| OneLinkApp::new(window, cx, Arc::clone(&service)))
    });
    (handle, app, service)
}

fn seed_device(app: &Entity<OneLinkApp>, cx: &mut App, id: &str) {
    let list = app.read(cx).device_list();
    list.update(cx, |list, cx| {
        list.apply(
            UiEvent::DeviceDiscovered {
                node_id: id.into(),
                addr: None,
            },
            cx,
        );
        list.apply(
            UiEvent::DeviceNamed {
                node_id: id.into(),
                name: "书房Mac".into(),
            },
            cx,
        );
    });
}

#[gpui_kit::test]
fn clicking_device_row_opens_that_conversation(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (handle, app, service) = open_app(cx);

    cx.update_window(handle.into(), |_, window, cx| {
        seed_device(&app, cx, "n1");
        window.draw(cx).clear(cx);
        window.click("device-n1", cx);
    })
    .unwrap();

    cx.update_window(handle.into(), |_, _, cx| {
        assert_eq!(app.read(cx).chat_store().selected(), Some("n1"));
    })
    .unwrap();

    shutdown_iroh_service(&service);
}

#[gpui_kit::test]
fn sending_appends_to_local_conversation(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (handle, app, service) = open_app(cx);

    cx.update_window(handle.into(), |_, window, cx| {
        seed_device(&app, cx, "n1");
        window.draw(cx).clear(cx);
        window.click("device-n1", cx);
    })
    .unwrap();

    cx.update_window(handle.into(), |_, window, cx| {
        let panel = app.read(cx).chat_panel();
        panel.update(cx, |panel, cx| panel.set_input_text("hi", window, cx));
        window.draw(cx).clear(cx);
        window.click("chat-send", cx);
    })
    .unwrap();

    cx.update_window(handle.into(), |_, _, cx| {
        let store = app.read(cx).chat_store();
        let messages = store.messages("n1");
        assert_eq!(messages.len(), 1, "expected one local message");
        assert_eq!(messages[0].text, "hi");
        assert!(messages[0].mine);
    })
    .unwrap();

    shutdown_iroh_service(&service);
}

#[gpui_kit::test]
fn chat_failure_shows_hint_until_delivery(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (handle, app, service) = open_app(cx);

    cx.update_window(handle.into(), |_, window, cx| {
        seed_device(&app, cx, "n1");
        window.draw(cx).clear(cx);
        window.click("device-n1", cx);
    })
    .unwrap();

    // 失败事件到达选中的会话 → 面板出现提示
    cx.update_window(handle.into(), |_, _, cx| {
        app.update(cx, |app, cx| {
            app.apply_batch(
                vec![UiEvent::ChatFailed {
                    node_id: "n1".into(),
                    id: 7,
                    reason: "send timed out after 5s".into(),
                }],
                cx,
            );
        });
    })
    .unwrap();

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        let hint = window
            .find("chat-error")
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(hint.contains("5s"), "hint was {hint:?}");
    })
    .unwrap();

    // 后续送达 → 提示清除
    cx.update_window(handle.into(), |_, _, cx| {
        app.update(cx, |app, cx| {
            app.apply_batch(
                vec![UiEvent::ChatDelivered {
                    node_id: "n1".into(),
                    id: 8,
                }],
                cx,
            );
        });
    })
    .unwrap();

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        assert!(window.try_find("chat-error").is_none(), "hint should clear");
    })
    .unwrap();

    shutdown_iroh_service(&service);
}
