//! 设备列表 UI：渲染注册表快照（状态点 + 设备名 + 短 node id + 未读数）。

use std::collections::BTreeMap;

use gpui_kit::base::TestSupportExt;
use gpui_kit::{
    Context, EventEmitter, IntoElement, ParentElement, Render, Styled, Window, div, prelude::*, px,
    rgb,
};

use crate::model::{DeviceInfo, DeviceRegistry, apply_ui_event};
use crate::net::UiEvent;

/// 设备列表向外抛的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceListEvent {
    /// 用户点了某台设备行（payload 是 node id）。
    Selected(String),
}

impl EventEmitter<DeviceListEvent> for DeviceList {}

pub struct DeviceList {
    registry: DeviceRegistry,
    selected: Option<String>,
    /// 各 peer 的未读数（由 `App` 从 `ChatStore` 同步过来，组件自己不算）。
    unread: BTreeMap<String, u32>,
}

impl DeviceList {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            registry: DeviceRegistry::new(),
            selected: None,
            unread: BTreeMap::new(),
        }
    }

    /// 测试/恢复用：直接访问注册表。
    pub fn registry_mut(&mut self) -> &mut DeviceRegistry {
        &mut self.registry
    }

    pub fn snapshot(&self) -> Vec<DeviceInfo> {
        self.registry.snapshot()
    }

    /// 同步某个 peer 的未读数（0 表示清除）。
    pub fn set_unread(&mut self, peer: &str, count: u32, cx: &mut Context<Self>) {
        if self.unread.get(peer).copied().unwrap_or(0) == count {
            return;
        }
        if count == 0 {
            self.unread.remove(peer);
        } else {
            self.unread.insert(peer.to_string(), count);
        }
        cx.notify();
    }

    /// 应用一条网络事件（经注册表合并），有变化即通知重绘。
    pub fn apply(&mut self, event: UiEvent, cx: &mut Context<Self>) {
        let changes = apply_ui_event(&mut self.registry, event);
        if !changes.is_empty() {
            cx.notify();
        }
    }

    fn select(&mut self, node_id: String, cx: &mut Context<Self>) {
        tracing::info!(node_id, "device row clicked");
        self.selected = Some(node_id.clone());
        cx.notify();
        cx.emit(DeviceListEvent::Selected(node_id));
    }
}

impl Render for DeviceList {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let devices = self.registry.snapshot();

        if devices.is_empty() {
            return div()
                .id("device-list")
                .test_support()
                .flex()
                .flex_col()
                .p(px(16.))
                .child(
                    div()
                        .id("device-list-empty")
                        .test_support()
                        .aria_label("等待发现设备…")
                        .text_color(rgb(0x888888))
                        .child("等待发现设备…"),
                );
        }

        div()
            .id("device-list")
            .test_support()
            .flex()
            .flex_col()
            .gap(px(4.))
            .p(px(12.))
            .children(devices.into_iter().map(|d| {
                let device_id = d.node_id.clone();
                let short: String = d.node_id.chars().take(8).collect();
                let dot = if d.online { "●" } else { "○" };
                let dot_color = if d.online {
                    rgb(0x2E7D32)
                } else {
                    rgb(0x9E9E9E)
                };
                let selected = self.selected.as_deref() == Some(d.node_id.as_str());
                let unread = self.unread.get(&d.node_id).copied().unwrap_or(0);

                div()
                    .id(format!("device-{}", d.node_id))
                    .test_support()
                    .aria_label(format!("{dot} {}", d.name))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .when(selected, |s| s.bg(rgb(0xE8F0FE)))
                    .child(div().text_color(dot_color).child(dot))
                    .child(div().child(d.name.clone()))
                    .child(
                        div()
                            .text_color(rgb(0x999999))
                            .text_size(px(11.))
                            .child(short),
                    )
                    .when(unread > 0, |s| {
                        s.child(
                            div()
                                .id(format!("unread-{}", d.node_id))
                                .test_support()
                                .aria_label(format!("未读 {unread}"))
                                .px(px(6.))
                                .rounded(px(8.))
                                .bg(rgb(0xD93025))
                                .text_color(rgb(0xFFFFFF))
                                .text_size(px(11.))
                                .child(unread.to_string()),
                        )
                    })
                    .on_click(cx.listener(move |this, _ev, _window, cx| {
                        this.select(device_id.clone(), cx);
                    }))
            }))
    }
}
