//! 设备列表 UI：渲染注册表快照（头像 + 设备名 + 短 node id + 未读数）。
//!
//! 行内保留在线状态（绿/灰圆点）与未读角标；hover / 选中态由主题 token 驱动，
//! 选中行整行加深底色，符合 IM 侧栏的信息层级。

use std::collections::BTreeMap;

use gpui_kit::base::TestSupportExt;
use gpui_kit::{
    Context, EventEmitter, FontWeight, IntoElement, ParentElement, Render, Styled, Window, div,
    prelude::*, px, rgb,
};

use crate::model::{DeviceInfo, DeviceRegistry, apply_ui_event};
use crate::net::UiEvent;
use crate::ui::theme::{color, radius, size, space, text};

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

/// 一行设备：头像（名字首字）+ 名字/短 id + 未读角标。
fn device_row(
    device: &DeviceInfo,
    selected: bool,
    unread: u32,
    cx: &mut Context<DeviceList>,
) -> gpui_kit::AnyElement {
    let device_id = device.node_id.clone();
    let initial = device
        .name
        .chars()
        .next()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "?".to_string());
    let short: String = device.node_id.chars().take(8).collect();
    let dot = if device.online { "●" } else { "○" };
    let dot_color = if device.online {
        rgb(color::ACCENT)
    } else {
        rgb(color::TEXT_MUTED)
    };

    div()
        .id(format!("device-{}", device.node_id))
        .test_support()
        // aria 文本保持旧格式（状态点 + 名字），测试与无障碍语义不变。
        .aria_label(format!("{dot} {}", device.name))
        .flex()
        .items_center()
        .gap(px(space::S3))
        .h(px(size::DEVICE_ROW_H))
        .px(px(space::S2))
        .rounded(px(radius::MD))
        .when(selected, |row| row.bg(rgb(color::SELECTED)))
        .hover(|row| row.bg(rgb(color::HOVER)))
        .child(
            // 头像：单字占位，避免引入网络头像依赖。
            div()
                .flex()
                .items_center()
                .justify_center()
                .w(px(size::AVATAR))
                .h(px(size::AVATAR))
                .flex_shrink_0()
                .rounded(px(radius::SM))
                .bg(rgb(color::AVATAR))
                .text_size(px(text::MESSAGE))
                .text_color(rgb(color::TEXT_SECONDARY))
                .child(initial),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.))
                .gap(px(2.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S1))
                        .child(
                            div()
                                .text_size(px(text::MESSAGE))
                                .text_color(rgb(color::TEXT))
                                .child(device.name.clone()),
                        )
                        // 在线状态：绿点在线、灰点离线（保留原语义）。
                        .child(div().text_size(px(9.)).text_color(dot_color).child(dot)),
                )
                .child(
                    div()
                        .text_size(px(text::CAPTION))
                        .text_color(rgb(color::TEXT_MUTED))
                        .child(short),
                ),
        )
        .when(unread > 0, |row| {
            row.child(
                div()
                    .id(format!("unread-{}", device.node_id))
                    .test_support()
                    .aria_label(format!("未读 {unread}"))
                    .flex()
                    .items_center()
                    .justify_center()
                    .min_w(px(18.))
                    .h(px(18.))
                    .px(px(5.))
                    .rounded(px(9.))
                    .bg(rgb(color::UNREAD))
                    .text_color(rgb(color::SURFACE))
                    .text_size(px(text::CAPTION))
                    .font_weight(FontWeight::MEDIUM)
                    .child(unread.to_string()),
            )
        })
        .on_click(cx.listener(move |this, _ev, _window, cx| {
            this.select(device_id.clone(), cx);
        }))
        .into_any_element()
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
                .items_center()
                .gap(px(space::S1))
                .p(px(space::S6))
                .child(
                    div()
                        .text_size(px(text::MESSAGE))
                        .text_color(rgb(color::TEXT_MUTED))
                        .child("尚未发现设备"),
                )
                .child(
                    div()
                        .id("device-list-empty")
                        .test_support()
                        .aria_label("等待发现设备…")
                        .text_size(px(text::CAPTION))
                        .text_color(rgb(color::TEXT_MUTED))
                        .child("等待发现设备…"),
                );
        }

        div()
            .id("device-list")
            .test_support()
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(px(space::S2))
            .children(devices.iter().map(|d| {
                let selected = self.selected.as_deref() == Some(d.node_id.as_str());
                let unread = self.unread.get(&d.node_id).copied().unwrap_or(0);
                device_row(d, selected, unread, cx)
            }))
    }
}
