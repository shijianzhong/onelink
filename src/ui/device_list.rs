//! 设备列表 UI：渲染注册表快照（头像 + 设备名 + 短 node id + 未读数）。
//!
//! 行内保留在线状态（绿/灰圆点）与未读角标；hover / 选中态由主题 token 驱动，
//! 选中行整行加深底色，符合 IM 侧栏的信息层级。

use std::collections::BTreeMap;

use gpui_kit::base::TestSupportExt;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::{
    Context, Entity, EventEmitter, FontWeight, IntoElement, KeyDownEvent, ParentElement, Render,
    ScrollHandle, Styled, Subscription, Window, div, prelude::*, px, rgb,
};

use crate::model::{DeviceInfo, DeviceRegistry, apply_ui_event};
use crate::net::UiEvent;
use crate::ui::theme::{color, radius, size, space, text};
use crate::ui::thin_scrollbar;

/// 设备列表向外抛的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceListEvent {
    /// 用户点了某台设备行（payload 是 node id）。
    Selected(String),
    /// 用户提交了备注；`alias: None` 表示清空备注（回落到握手名）。
    AliasSubmitted {
        node_id: String,
        alias: Option<String>,
    },
    /// 用户取消了备注编辑（不写库）。
    AliasCancelled,
}

impl EventEmitter<DeviceListEvent> for DeviceList {}

pub struct DeviceList {
    registry: DeviceRegistry,
    selected: Option<String>,
    /// 各 peer 的未读数（由 `App` 从 `ChatStore` 同步过来，组件自己不算）。
    unread: BTreeMap<String, u32>,
    /// 正在改备注的 node id（行内编辑态）。
    editing: Option<String>,
    /// 行内编辑框；与 `editing` 同时存在。
    alias_input: Option<Entity<TextareaState>>,
    /// 编辑框事件订阅（Enter 提交）；退出编辑即 drop。
    _alias_sub: Option<Subscription>,
    /// 列表滚动位置（滚动条显式常显，见 render 末尾）。
    scroll: ScrollHandle,
}

impl DeviceList {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            registry: DeviceRegistry::new(),
            selected: None,
            unread: BTreeMap::new(),
            editing: None,
            alias_input: None,
            _alias_sub: None,
            scroll: ScrollHandle::new(),
        }
    }

    /// 测试/恢复用：直接访问注册表。
    pub fn registry_mut(&mut self) -> &mut DeviceRegistry {
        &mut self.registry
    }

    pub fn snapshot(&self) -> Vec<DeviceInfo> {
        self.registry.snapshot()
    }

    /// 按 node id 查设备快照（持久化/调试用）。
    pub fn device(&self, node_id: &str) -> Option<DeviceInfo> {
        self.registry.get(node_id).cloned()
    }

    /// 用持久化的设备列表填充（启动时全部按离线渲染，等 mDNS/握手再置在线）。
    pub fn hydrate(&mut self, devices: Vec<DeviceInfo>, cx: &mut Context<Self>) {
        for mut device in devices {
            device.online = false;
            self.registry.upsert(device);
        }
        cx.notify();
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

    /// 进入某台设备的行内备注编辑（预填当前备注并聚焦）。
    pub fn begin_alias_edit(&mut self, node_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let current = self
            .registry
            .get(node_id)
            .and_then(|d| d.alias.clone())
            .unwrap_or_default();
        let input = cx.new(|cx| TextareaState::new(window, cx).submit_on_enter(true));
        input.update(cx, |state, cx| {
            state.set_value(current, window, cx);
            state.set_placeholder("留空恢复原名", window, cx);
            state.focus(window, cx);
        });
        let sub = cx.subscribe_in(
            &input,
            window,
            |this, _input, ev: &InputEvent, _window, cx| {
                if let InputEvent::PressEnter { shift, .. } = ev
                    && !*shift
                {
                    this.commit_alias_edit(cx);
                }
            },
        );
        self.editing = Some(node_id.to_string());
        self.alias_input = Some(input);
        self._alias_sub = Some(sub);
        cx.notify();
    }

    /// 提交备注：空白串按清空处理；先更新内存显示，再抛事件让 `App` 落库。
    pub fn commit_alias_edit(&mut self, cx: &mut Context<Self>) {
        let Some(node_id) = self.editing.take() else {
            return;
        };
        let Some(input) = self.alias_input.take() else {
            return;
        };
        let text = input.read(cx).value().to_string();
        let alias = Some(text.trim().to_string()).filter(|a| !a.is_empty());
        self._alias_sub = None;
        self.registry.set_alias(&node_id, alias.clone());
        cx.notify();
        cx.emit(DeviceListEvent::AliasSubmitted { node_id, alias });
    }

    /// 取消编辑：丢掉输入，不写库。
    pub fn cancel_alias_edit(&mut self, cx: &mut Context<Self>) {
        if self.editing.take().is_none() {
            return;
        }
        self.alias_input = None;
        self._alias_sub = None;
        cx.notify();
        cx.emit(DeviceListEvent::AliasCancelled);
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
    let edit_id = device.node_id.clone();
    let initial = device
        .display_name()
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
        // aria 文本保持旧格式（状态点 + 显示名），测试与无障碍语义不变。
        .aria_label(format!("{dot} {}", device.display_name()))
        .flex()
        // 行宽由侧栏决定，不随名字长度收缩：显式占满列表可用宽度。
        .w_full()
        // 固定行高不参与 flex 收缩：条目多于可视高度时改为滚动，而不是把行压扁。
        .flex_shrink_0()
        .items_center()
        .gap(px(space::S3))
        .h(px(size::DEVICE_ROW_H))
        .min_h(px(size::DEVICE_ROW_H))
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
                            // 名字吃掉剩余宽度；过长时省略号截断，不撑开布局。
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(text::MESSAGE))
                                .text_color(rgb(color::TEXT))
                                .child(device.display_name().to_string()),
                        )
                        // 在线状态：绿点在线、灰点离线（保留原语义），不参与宽度竞争。
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(9.))
                                .text_color(dot_color)
                                .child(dot),
                        ),
                )
                .child(
                    div()
                        .text_size(px(text::CAPTION))
                        .text_color(rgb(color::TEXT_MUTED))
                        .child(short),
                ),
        )
        // 行内次要操作：改本地备注（点击不冒泡成"选中会话"）。
        .child(
            div()
                .id(format!("alias-edit-{edit_id}"))
                .test_support()
                .aria_label("改备注")
                .flex_shrink_0()
                .px(px(space::S1))
                .rounded(px(radius::SM))
                .text_size(px(text::CAPTION))
                .text_color(rgb(color::TEXT_MUTED))
                .hover(|el| el.text_color(rgb(color::TEXT_SECONDARY)))
                .child("备注")
                .on_click(cx.listener(move |this, _ev, window, cx| {
                    cx.stop_propagation();
                    this.begin_alias_edit(&edit_id, window, cx);
                })),
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
                    .flex_shrink_0()
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

/// 行内备注编辑：焦点描边沿用输入框的淡绿 token；Enter 提交、Esc 取消。
fn alias_editor_row(
    node_id: &str,
    input: &Entity<TextareaState>,
    cx: &mut Context<DeviceList>,
) -> gpui_kit::AnyElement {
    let action = |id: String, label: &'static str, accent: bool| {
        div()
            .id(id)
            .test_support()
            .aria_label(label)
            .flex_shrink_0()
            .px(px(space::S1))
            .rounded(px(radius::SM))
            .text_size(px(text::CAPTION))
            .text_color(rgb(if accent {
                color::ACCENT
            } else {
                color::TEXT_MUTED
            }))
            .child(label)
    };

    div()
        .id(format!("alias-editor-{node_id}"))
        .test_support()
        .flex()
        .w_full()
        .flex_shrink_0()
        .items_center()
        .gap(px(space::S2))
        .h(px(size::DEVICE_ROW_H))
        .px(px(space::S2))
        .rounded(px(radius::MD))
        .bg(rgb(color::HOVER))
        .child(
            div()
                .id(format!("alias-input-{node_id}"))
                .test_support()
                .flex()
                .flex_col()
                .justify_center()
                .flex_1()
                .min_w(px(0.))
                .h(px(30.))
                .px(px(space::S2))
                .rounded(px(radius::SM))
                .bg(rgb(color::SURFACE))
                .border_1()
                // 静态分隔色：聚焦不加任何描边强调，只靠光标。
                .border_color(rgb(color::DIVIDER))
                .child(
                    Textarea::new(input)
                        .appearance(false)
                        .h(px(26.))
                        .text_size(px(text::CAPTION)),
                ),
        )
        .child(
            action(format!("alias-save-{node_id}"), "保存", true).on_click(cx.listener(
                |this, _ev, _window, cx| {
                    cx.stop_propagation();
                    this.commit_alias_edit(cx);
                },
            )),
        )
        .child(
            action(format!("alias-cancel-{node_id}"), "取消", false).on_click(cx.listener(
                |this, _ev, _window, cx| {
                    cx.stop_propagation();
                    this.cancel_alias_edit(cx);
                },
            )),
        )
        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| {
            if ev.keystroke.key == "escape" {
                this.cancel_alias_edit(cx);
            }
        }))
        .into_any_element()
}

impl Render for DeviceList {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let devices = self.registry.snapshot();
        let editing = self.editing.clone();
        let alias_input = self.alias_input.clone();
        let unread_all = self.unread.clone();
        let selected = self.selected.clone();

        if devices.is_empty() {
            return div()
                .id("device-list")
                .test_support()
                .flex()
                .flex_col()
                .w_full()
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
            // 列表铺满侧栏；行内再用 w_full 撑满扣掉内边距后的可用宽度。
            .w_full()
            .h_full()
            .gap(px(2.))
            .p(px(space::S2))
            // 品牌头在 render 外层固定，这里只让设备行滚动；滚动条覆盖在右侧留白上。
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .children(devices.iter().map(|d| {
                if editing.as_deref() == Some(d.node_id.as_str())
                    && let Some(input) = &alias_input
                {
                    return alias_editor_row(&d.node_id, input, cx);
                }
                let is_selected = selected.as_deref() == Some(d.node_id.as_str());
                let unread = unread_all.get(&d.node_id).copied().unwrap_or(0);
                device_row(d, is_selected, unread, cx)
            }))
            .child(thin_scrollbar(&self.scroll))
    }
}
