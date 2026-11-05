//! 设备列表 UI：渲染注册表快照（状态点 + 设备名 + 短 node id）。

use gpui_kit::base::TestSupportExt;
use gpui_kit::{
    Context, IntoElement, ParentElement, Render, Styled, Window, div, prelude::*, px, rgb,
};

use crate::model::{DeviceInfo, DeviceRegistry, apply_ui_event};
use crate::net::UiEvent;

pub struct DeviceList {
    registry: DeviceRegistry,
    selected: Option<String>,
}

impl DeviceList {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            registry: DeviceRegistry::new(),
            selected: None,
        }
    }

    /// 测试/恢复用：直接访问注册表。
    pub fn registry_mut(&mut self) -> &mut DeviceRegistry {
        &mut self.registry
    }

    pub fn snapshot(&self) -> Vec<DeviceInfo> {
        self.registry.snapshot()
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
        self.selected = Some(node_id);
        cx.notify();
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
                    .on_click(cx.listener(move |this, _ev, _window, cx| {
                        this.select(device_id.clone(), cx);
                    }))
            }))
    }
}
