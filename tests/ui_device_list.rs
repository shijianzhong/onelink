mod common;

use std::time::SystemTime;

use gpui_kit::TestAppContext;
use gpui_kit::prelude::*;
use gpui_kit::test::TestWindowExt;
use onelink::model::DeviceInfo;
use onelink::net::UiEvent;
use onelink::ui::device_list::DeviceList;

#[gpui_kit::test]
fn device_rows_show_status_dot_and_name(cx: &mut TestAppContext) {
    let (handle, _list) = common::open_window(cx, None, |_, cx| {
        cx.new(|cx| {
            let mut list = DeviceList::new(cx);
            let now = SystemTime::now();
            list.registry_mut().upsert(DeviceInfo {
                node_id: "n1".into(),
                name: "书房Mac".into(),
                addr: None,
                online: true,
                last_seen: now,
            });
            list.registry_mut().upsert(DeviceInfo {
                node_id: "n2".into(),
                name: "云主机".into(),
                addr: None,
                online: false,
                last_seen: now,
            });
            list
        })
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        let online = window.find("device-n1");
        let label = online.label().unwrap_or_default().to_string();
        assert!(label.contains("书房Mac"), "label was {label}");
        assert!(label.contains("●"), "label was {label}");

        let offline = window.find("device-n2");
        let label = offline.label().unwrap_or_default().to_string();
        assert!(label.contains("云主机"), "label was {label}");
        assert!(label.contains("○"), "label was {label}");
    })
    .unwrap();
}

#[gpui_kit::test]
fn apply_events_updates_rows(cx: &mut TestAppContext) {
    let (handle, list) = common::open_window(cx, None, |_, cx| {
        cx.new(|cx| {
            let mut list = DeviceList::new(cx);
            list.apply(
                UiEvent::DeviceDiscovered {
                    node_id: "n1".into(),
                    addr: None,
                },
                cx,
            );
            list.apply(
                UiEvent::DeviceNamed {
                    node_id: "n1".into(),
                    name: "书房Mac".into(),
                },
                cx,
            );
            list
        })
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        let row = window.find("device-n1");
        assert!(row.label().unwrap_or_default().contains("书房Mac"));
    })
    .unwrap();

    // 离线事件到达后重新渲染，状态点变灰
    cx.update_window(handle.into(), |_, _, cx| {
        list.update(cx, |list, cx| {
            list.apply(
                UiEvent::DeviceLost {
                    node_id: "n1".into(),
                },
                cx,
            );
        });
    })
    .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        let row = window.find("device-n1");
        assert!(row.label().unwrap_or_default().contains("○"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn empty_registry_shows_placeholder(cx: &mut TestAppContext) {
    let (handle, _list) = common::open_window(cx, None, |_, cx| cx.new(DeviceList::new));

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        let hint = window.find("device-list-empty");
        assert!(
            hint.label().unwrap_or_default().contains("等待发现设备"),
            "hint label missing"
        );
    })
    .unwrap();
}
