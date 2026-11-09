mod common;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::SystemTime;

use gpui_kit::TestAppContext;
use gpui_kit::prelude::*;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    Context, Entity, IntoElement, ParentElement, Render, ScrollDelta, Styled, Subscription, Window,
    div, point, px,
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

/// 宿主包装：收集列表事件，便于断言"提交/取消"。
struct AliasHost {
    list: Entity<DeviceList>,
    events: Rc<RefCell<Vec<DeviceListEvent>>>,
    _sub: Subscription,
}

impl Render for AliasHost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.list.clone())
    }
}

fn alias_host(
    cx: &mut TestAppContext,
) -> (
    gpui_kit::WindowHandle<gpui_kit::base::Root>,
    Entity<AliasHost>,
) {
    cx.update(gpui_kit::init);
    let events: Rc<RefCell<Vec<DeviceListEvent>>> = Rc::new(RefCell::new(Vec::new()));
    let sink_sub = Rc::clone(&events);
    let sink_host = Rc::clone(&events);
    common::open_window(cx, None, move |_window, cx| {
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
                sink_sub.borrow_mut().push(ev.clone());
            });
            AliasHost {
                list,
                events: Rc::clone(&sink_host),
                _sub: sub,
            }
        })
    })
}

/// Enter 提交备注：行内立即显示别名，并抛出可落库的事件。
#[gpui_kit::test]
fn alias_edit_enter_commits_and_shows_alias(cx: &mut TestAppContext) {
    let (handle, host) = alias_host(cx);

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        window.click("alias-edit-n1", cx);
    })
    .unwrap();

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        assert!(
            window.try_find("alias-input-n1").is_some(),
            "editor should open"
        );
        window.input("客厅机器", cx);
        window.press("enter", cx);
        window.draw(cx).clear(cx);
    })
    .unwrap();

    let events = cx.update(|cx| host.read(cx).events.borrow().clone());
    assert_eq!(
        events,
        vec![DeviceListEvent::AliasSubmitted {
            node_id: "n1".into(),
            alias: Some("客厅机器".into())
        }]
    );

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        let label = window
            .find("device-n1")
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(label.contains("客厅机器"), "label was {label}");
        assert!(
            window.try_find("alias-input-n1").is_none(),
            "editor should close"
        );
    })
    .unwrap();
}

/// Esc 取消：不发提交事件（因此不会写库），行内容保持原名。
#[gpui_kit::test]
fn alias_edit_escape_cancels_without_submit(cx: &mut TestAppContext) {
    let (handle, host) = alias_host(cx);

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        window.click("alias-edit-n1", cx);
    })
    .unwrap();

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        window.input("临时输入", cx);
        window.press("escape", cx);
        window.draw(cx).clear(cx);
    })
    .unwrap();

    let events = cx.update(|cx| host.read(cx).events.borrow().clone());
    assert_eq!(events, vec![DeviceListEvent::AliasCancelled]);

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        let label = window
            .find("device-n1")
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(label.contains("书房Mac"), "label was {label}");
        assert!(window.try_find("alias-input-n1").is_none());
    })
    .unwrap();
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

/// 条目超过可视高度时：滚轮滚动后，之前够不到的会话能被点中（真实路径），
/// 滚动条不挤压行宽，顶部区域不参与滚动。
#[gpui_kit::test]
fn long_device_list_scrolls_to_reach_early_sessions(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let events: Rc<RefCell<Vec<DeviceListEvent>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&events);

    // 固定小窗口：12 行 × 64px 远高于 240px 可视高度。
    let (handle, _host) = common::open_window(
        cx,
        Some(gpui_kit::size(px(300.), px(240.))),
        move |_window, cx| {
            cx.new(move |cx| {
                let list = cx.new(|cx| {
                    let mut list = DeviceList::new(cx);
                    let now = SystemTime::now();
                    for i in 1..=12 {
                        list.registry_mut().upsert(DeviceInfo {
                            node_id: format!("n{i}"),
                            // 补零保证按名字排序时 n12 真的排在最后。
                            name: format!("设备{i:02}"),
                            alias: None,
                            addr: None,
                            online: false,
                            last_seen: now,
                        });
                    }
                    list
                });
                let sub = cx.subscribe(&list, move |_h, _l, ev: &DeviceListEvent, _cx| {
                    sink.borrow_mut().push(ev.clone());
                });
                RowClickHost { list, _sub: sub }
            })
        },
    );

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        // 首行可见，行宽铺满侧栏扣掉内边距：300 - 8*2 = 284。
        let first = window.find("device-n1").bounds();
        assert_eq!(f32::from(first.size.width), 284.);
        // 视口外的行已注册但不可见——这正是"早期会话选不到"的现象。
        let last = window.try_find("device-n12").expect("row registered");
        assert!(
            !last.visible(),
            "last row must be out of view before scrolling"
        );

        // 滚轮向下（GPUI 的向下 delta 为负）。
        window.scroll(
            "device-list",
            ScrollDelta::Pixels(point(px(0.), px(-800.))),
            cx,
        );
        window.draw(cx).clear(cx);

        let last = window.find("device-n12");
        assert!(
            last.visible(),
            "scrolling down must bring the last row into view"
        );
        assert!(
            !window.find("device-n1").visible(),
            "the top row must scroll out of view"
        );
        // 滚动条覆盖在右侧留白上，滚动前后行宽一致，不会被挤变形。
        assert_eq!(f32::from(last.bounds().size.width), 284.);

        // 真实路径：点中滚动后才露出的会话，事件必须落到它身上。
        window.click("device-n12", cx);
    })
    .unwrap();

    assert_eq!(
        *events.borrow(),
        vec![DeviceListEvent::Selected("n12".into())]
    );
}

/// 少量设备（不满一屏）：不该出现滚动条，滚轮与轨道点击都不能推动行位置。
#[gpui_kit::test]
fn short_device_list_has_no_scrollbar(cx: &mut TestAppContext) {
    let (handle, _list) = common::open_window(
        cx,
        Some(gpui_kit::size(px(300.), px(240.))),
        |_window, cx| {
            cx.new(|cx| {
                let mut list = DeviceList::new(cx);
                let now = SystemTime::now();
                for i in 1..=3 {
                    list.registry_mut().upsert(DeviceInfo {
                        node_id: format!("n{i}"),
                        name: format!("设备{i:02}"),
                        alias: None,
                        addr: None,
                        online: false,
                        last_seen: now,
                    });
                }
                list
            })
        },
    );

    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        // 3 行 + 间距 + 内边距 ≈ 212px，低于 240px 可视高度。
        let list = window.find("device-list").bounds();
        let first = window.find("device-n1").bounds();

        window.scroll(
            "device-list",
            ScrollDelta::Pixels(point(px(0.), px(-800.))),
            cx,
        );
        window.draw(cx).clear(cx);
        assert_eq!(
            first,
            window.find("device-n1").bounds(),
            "short device list must not scroll"
        );

        // 滚动条轨道位于列表右缘内侧：没有溢出时点击不应产生位移。
        window.click_at(
            "device-list",
            point(list.size.width - px(4.), list.size.height / 2.),
            cx,
        );
        window.draw(cx).clear(cx);
        assert_eq!(
            first,
            window.find("device-n1").bounds(),
            "short device list must not have a draggable scrollbar track"
        );
    })
    .unwrap();
}
