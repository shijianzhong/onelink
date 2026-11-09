//! UI 组件模块。

pub mod chat_panel;
pub mod device_list;
pub mod theme;
pub mod transfer_row;

use gpui_kit::base::{Scrollbar, ScrollbarMode};
use gpui_kit::{AnyElement, IntoElement, ScrollHandle, px, rgb, transparent_black};

use theme::{color, radius, size};

/// 侧栏与消息区共用的细滚动条：常显、低对比、覆盖在留白上（不挤占行宽）。
pub(crate) fn thin_scrollbar(handle: &ScrollHandle) -> AnyElement {
    Scrollbar::vertical(handle)
        .mode(ScrollbarMode::Always)
        .styles(|styles| {
            styles
                .track(|track| {
                    track
                        .width(px(size::SCROLLBAR_TRACK_W))
                        .bg(transparent_black())
                })
                .thumb(|thumb| {
                    thumb
                        .bg(rgb(color::SCROLL_THUMB))
                        .width(px(size::SCROLLBAR_W))
                        .inset(px(size::SCROLLBAR_INSET))
                        .radius(px(radius::SM))
                        .min_length(px(size::SCROLLBAR_MIN_H))
                })
                .thumb_hover(|thumb| thumb.bg(rgb(color::SCROLL_THUMB_HOVER)))
        })
        .into_any_element()
}
