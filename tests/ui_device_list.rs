mod common;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::SystemTime;

use gpui_kit::TestAppContext;
use gpui_kit::prelude::*;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    Context, Entity, IntoElement, ParentElement, Render, Styled, Subscription, Window, div,
};
use onelink::model::DeviceInfo;
use onelink::net::UiEvent;
use onelink::ui::device_list::{DeviceList, DeviceListEvent};

#[gpui_kit::test]
fn device_rows_show_status_dot_and_name(cx: &mut TestAppContext) {
    let (handle, _list) = common::open_window(cx, None, |_, cx| {
        cx.new(|cx| {
            let mut list = DeviceList::new(cx);
            let now = SystemTime::now();
            list.registry_mut().upsert(DeviceInfo {
                node_id: "n1".into(),
                name: "书房Mac".into(),
                alias: None,
                addr: None,
                online: true,
                last_seen: now,
            });
            list.registry_mut().upsert(DeviceInfo {
                node_id: "n2".into(),
                name: "云主机".into(),
                alias: None,
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

/// 宿主包装：持有订阅（Subscription drop 即取消）。
struct RowClickHost {
    list: Entity<DeviceList>,
    _sub: Subscription,
}

impl Render for RowClickHost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.list.clone())
    }
}

#[gpui_kit::test]
fn clicking_device_row_emits_selected(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let events: Rc<RefCell<Vec<DeviceListEvent>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&events);

    let (handle, _host) = common::open_window(cx, None, move |_window, cx| {
        cx.new(move |cx| {
            let list = cx.new(|cx| {
                let mut list = DeviceList::new(cx);
                list.registry_mut().upsert(DeviceInfo {
                    node_id: "n1".into(),
                    name: "书房Mac".into(),
                    alias: None,
                    addr: None,
                    online: true,
                    last_seen: SystemTime::now(),
                });
                list
            });
            let sub = cx.subscribe(&list, move |_h, _l, ev: &DeviceListEvent, _cx| {
                sink.borrow_mut().push(ev.clone());
            });
            RowClickHost { list, _sub: sub }
        })
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        window.click("device-n1", cx);
    })
    .unwrap();

    assert_eq!(
        *events.borrow(),
        vec![DeviceListEvent::Selected("n1".into())]
    );
}

/// 回归：行宽由侧栏决定，与用户名长度无关（短名/长名/带未读角标都一样）。
#[gpui_kit::test]
fn rows_fill_sidebar_regardless_of_name_length(cx: &mut TestAppContext) {
    let (handle, _host) = common::open_window(cx, None, |_, cx| {
        cx.new(|cx| {
            let list = cx.new(|cx| {
                let mut list = DeviceList::new(cx);
                let now = SystemTime::now();
                list.registry_mut().upsert(DeviceInfo {
                    node_id: "short".into(),
                    name: "云主机".into(),
                    alias: None,
                    addr: None,
                    online: true,
                    last_seen: now,
                });
                list.registry_mut().upsert(DeviceInfo {
                    node_id: "long".into(),
                    name: "客厅那台非常非常长的设备名称用来验证省略号截断".into(),
                    alias: None,
                    addr: None,
                    online: false,
                    last_seen: now,
                });
                list.set_unread("long", 12, cx);
                list
            });
            // 复刻生产布局：300px 侧栏内的列方向 flex 容器。
            SidebarHost { list }
        })
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        let short = window.find("device-short").bounds();
        let long = window.find("device-long").bounds();
        assert_eq!(
            f32::from(short.size.width),
            f32::from(long.size.width),
            "短名 {short:?} / 长名 {long:?} 行宽应一致"
        );
        // 300px 侧栏 - 列表左右各 8px 内边距 = 284px。
        assert_eq!(f32::from(short.size.width), 284.);
        assert_eq!(f32::from(long.size.height), 64.);
    })
    .unwrap();
}

/// 复刻生产的侧栏容器：固定 300px + 列方向 flex，子项应横向拉伸。
struct SidebarHost {
    list: Entity<DeviceList>,
}

impl Render for SidebarHost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(gpui_kit::px(300.))
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .w_full()
                    .min_h(gpui_kit::px(0.))
                    .child(self.list.clone()),
            )
    }
}
