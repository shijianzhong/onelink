//! App 接线测试：点设备行 → 打开会话；发送 → 本地立即入会话模型。

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{App, Entity, ExternalPaths, FileDropEvent, InputEvent, TestAppContext, px};
use onelink::app::App as OneLinkApp;
use onelink::net::{IrohServiceHandle, UiEvent, shutdown_iroh_service, spawn_iroh_service};
use onelink::store::Store;

/// 打开带真实 iroh 服务的 App 窗口（服务只用其命令/事件通道，不发真实消息）。
fn open_app(
    cx: &mut TestAppContext,
) -> (
    gpui_kit::WindowHandle<gpui_kit::base::Root>,
    Entity<OneLinkApp>,
    Arc<IrohServiceHandle>,
) {
    open_app_sized(cx, None)
}

/// 同上，但可指定初始窗口尺寸（用于默认窗口尺度下的布局回归）。
fn open_app_sized(
    cx: &mut TestAppContext,
    size: Option<gpui_kit::Size<gpui_kit::Pixels>>,
) -> (
    gpui_kit::WindowHandle<gpui_kit::base::Root>,
    Entity<OneLinkApp>,
    Arc<IrohServiceHandle>,
) {
    let service = Arc::new(spawn_iroh_service().expect("spawn iroh service"));
    let (store, loaded) = Store::open_in_memory();
    let (handle, app) = common::open_window(cx, size, {
        let service = Arc::clone(&service);
        move |window, cx| {
            cx.new(|cx| OneLinkApp::new(window, cx, Arc::clone(&service), store, loaded))
        }
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

#[gpui_kit::test]
fn transfer_offer_renders_accept_row_in_selected_chat(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (handle, app, service) = open_app(cx);

    cx.update_window(handle.into(), |_, window, cx| {
        seed_device(&app, cx, "n1");
        window.draw(cx).clear(cx);
        window.click("device-n1", cx);
    })
    .unwrap();

    cx.update_window(handle.into(), |_, _, cx| {
        app.update(cx, |app, cx| {
            app.apply_batch(
                vec![UiEvent::TransferOffer {
                    node_id: "n1".into(),
                    xfer_id: 5,
                    name: "a.bin".into(),
                    size: 2048,
                }],
                cx,
            );
        });
    })
    .unwrap();

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        assert!(
            window.try_find("xfer-row-5").is_some(),
            "offer row should render"
        );
        assert!(
            window.try_find("xfer-accept-5").is_some(),
            "accept button should render"
        );
    })
    .unwrap();

    shutdown_iroh_service(&service);
}

#[gpui_kit::test]
fn dropping_file_starts_transfer_in_selected_chat(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (handle, app, service) = open_app(cx);

    cx.update_window(handle.into(), |_, window, cx| {
        seed_device(&app, cx, "n1");
        window.draw(cx).clear(cx);
        window.click("device-n1", cx);
    })
    .unwrap();

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        let position = window.find("chat-panel").bounds().center();
        let paths = ExternalPaths(
            [PathBuf::from("/tmp/onelink-drop.txt")]
                .into_iter()
                .collect(),
        );
        window.dispatch_event(
            FileDropEvent::Entered { position, paths }.to_platform_input(),
            cx,
        );
        window.dispatch_event(FileDropEvent::Submit { position }.to_platform_input(), cx);
    })
    .unwrap();

    // 无效 node id 时服务返回 xfer_id 0，但传输行必须先出现在当前会话里。
    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        assert!(
            window.try_find("xfer-row-0").is_some(),
            "drop should add a transfer row"
        );
    })
    .unwrap();

    shutdown_iroh_service(&service);
}

#[gpui_kit::test]
fn progress_and_completion_update_row(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (handle, app, service) = open_app(cx);

    cx.update_window(handle.into(), |_, window, cx| {
        seed_device(&app, cx, "n1");
        window.draw(cx).clear(cx);
        window.click("device-n1", cx);
    })
    .unwrap();

    cx.update_window(handle.into(), |_, _, cx| {
        app.update(cx, |app, cx| {
            app.apply_batch(
                vec![
                    UiEvent::TransferOffer {
                        node_id: "n1".into(),
                        xfer_id: 5,
                        name: "a.bin".into(),
                        size: 2048,
                    },
                    UiEvent::TransferProgress {
                        node_id: "n1".into(),
                        xfer_id: 5,
                        transferred: 1024,
                        total: 2048,
                    },
                ],
                cx,
            );
        });
    })
    .unwrap();

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        let text = window
            .find("xfer-progress-5")
            .label()
            .unwrap_or_default()
            .to_string();
        assert_eq!(text, "50%");
    })
    .unwrap();

    cx.update_window(handle.into(), |_, _, cx| {
        app.update(cx, |app, cx| {
            app.apply_batch(
                vec![UiEvent::TransferCompleted {
                    node_id: "n1".into(),
                    xfer_id: 5,
                    path: None,
                }],
                cx,
            );
        });
    })
    .unwrap();

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        let text = window
            .find("xfer-state-5")
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(text.contains("完成"), "state was {text:?}");
    })
    .unwrap();

    shutdown_iroh_service(&service);
}

/// 回归：默认窗口缩小到 960×680 后，侧栏仍 300px、聊天区和输入区不被挤压。
#[gpui_kit::test]
fn compact_default_window_keeps_two_pane_layout(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let window_size = |width, height| gpui_kit::Size {
        width: px(width),
        height: px(height),
    };
    let (handle, app, service) = open_app_sized(
        cx,
        Some(window_size(
            onelink::ui::theme::size::WINDOW_W,
            onelink::ui::theme::size::WINDOW_H,
        )),
    );

    cx.update_window(handle.into(), |_, window, cx| {
        seed_device(&app, cx, "n1");
        window.draw(cx).clear(cx);

        let sidebar = window.find("app-sidebar").bounds();
        assert_eq!(f32::from(sidebar.size.width), 300.);

        // 聊天区吃满剩余宽度：960 - 300 = 660。
        let chat = window.find("chat-panel").bounds();
        assert_eq!(f32::from(chat.size.width), 660.);

        // 输入卡片左右各留 24px，仍有 612px 宽；发送按钮不被压缩。
        let composer = window.find("chat-composer").bounds();
        assert_eq!(f32::from(composer.size.width), 612.);
        let send = window.find("chat-send").bounds();
        assert_eq!(f32::from(send.size.width), 76.);
        let input = window.find("chat-input").bounds();
        assert!(f32::from(input.size.height) >= 112.);
    })
    .unwrap();

    shutdown_iroh_service(&service);
}

/// 布局回归：侧栏固定 300px，聊天区占满剩余宽度（UI 断言，不依赖截图）。
#[gpui_kit::test]
fn sidebar_uses_fixed_im_width(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (handle, _app, service) = open_app(cx);

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        let sidebar = window.find("app-sidebar").bounds();
        assert_eq!(f32::from(sidebar.size.width), 300.);
        let root = window.find("app-root").bounds();
        assert!(f32::from(root.size.width) > 300., "root {root:?}");
    })
    .unwrap();

    shutdown_iroh_service(&service);
}
