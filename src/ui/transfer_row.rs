//! 传输行渲染：文件名 · 状态 · 进度条；待接收时给出接受/拒绝按钮。
//!
//! 接受/拒绝不在组件内直接调网络：按钮只把 id 以 [`ChatPanelEvent`] 抛出，
//! 由 `App` 订阅后投递给 iroh 服务（UI 线程永不阻塞）。

use gpui_kit::base::TestSupportExt;
use gpui_kit::{
    Context, FontWeight, IntoElement, ParentElement, Styled, div, prelude::*, px, relative, rgb,
};

use crate::ui::chat_panel::{ChatPanel, ChatPanelEvent};
use crate::ui::theme::{color, radius, space, text};

/// 单条传输在 UI 上的状态。
#[derive(Debug, Clone, PartialEq)]
pub enum TransferState {
    /// 已向对方发出文件请求，等对方确认。
    Waiting,
    /// 传输中，取值 `0.0..=1.0`。
    Active(f32),
    /// 已完成（发送端收到 ACK / 接收端落盘并校验通过）。
    Done,
    /// 失败或拒绝，附带给用户看的原因。
    Failed(String),
    /// 收到对方的文件请求，等用户接受/拒绝。
    Offer { size: u64 },
}

/// 一条传输行的显示数据（由 `App` 按会话维护）。
#[derive(Debug, Clone, PartialEq)]
pub struct TransferView {
    pub xfer_id: u64,
    pub name: String,
    pub state: TransferState,
}

fn percent(fraction: f32) -> String {
    format!("{:.0}%", fraction.clamp(0.0, 1.0) * 100.0)
}

fn format_size(size: u64) -> String {
    if size >= 1024 * 1024 {
        format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
    } else if size >= 1024 {
        format!("{:.1} KB", size as f64 / 1024.0)
    } else {
        format!("{size} B")
    }
}

/// 卡片内的操作按钮：接受用强调绿，拒绝用中性灰。
fn action_button(
    id: String,
    label: &'static str,
    primary: bool,
    cx: &mut Context<ChatPanel>,
    event: ChatPanelEvent,
) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .aria_label(label)
        .flex()
        .items_center()
        .justify_center()
        .px(px(space::S3))
        .py(px(5.))
        .rounded(px(radius::SM))
        .text_size(px(text::BODY))
        .bg(if primary {
            rgb(color::ACCENT)
        } else {
            rgb(color::BG)
        })
        .text_color(if primary {
            rgb(color::SURFACE)
        } else {
            rgb(color::TEXT_SECONDARY)
        })
        .hover(|button| {
            button.bg(if primary {
                rgb(color::ACCENT)
            } else {
                rgb(color::HOVER)
            })
        })
        .child(label)
        .on_click(cx.listener(move |_this, _ev, _window, cx| {
            cx.emit(event.clone());
        }))
}

/// 渲染一条传输行。
pub fn render(view: &TransferView, cx: &mut Context<ChatPanel>) -> impl IntoElement {
    let id = view.xfer_id;
    let (state_text, state_color) = match &view.state {
        TransferState::Waiting => ("等待对方接受…".to_string(), color::TEXT_MUTED),
        TransferState::Active(_) => ("传输中".to_string(), color::TEXT_SECONDARY),
        TransferState::Done => ("完成".to_string(), color::ACCENT),
        TransferState::Failed(reason) => (format!("失败：{reason}"), color::DANGER),
        TransferState::Offer { size } => (
            format!("待接收 · {}", format_size(*size)),
            color::TEXT_SECONDARY,
        ),
    };

    let mut row = div()
        .id(format!("xfer-row-{id}"))
        .test_support()
        .flex()
        .flex_col()
        .gap(px(space::S2))
        .p(px(space::S3))
        .rounded(px(radius::LG))
        .bg(rgb(color::SURFACE))
        .border_1()
        .border_color(rgb(color::DIVIDER))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(space::S2))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_size(px(text::BODY))
                        .text_color(rgb(color::TEXT))
                        .font_weight(FontWeight::MEDIUM)
                        .child(view.name.clone()),
                )
                .child(
                    div()
                        .id(format!("xfer-state-{id}"))
                        .test_support()
                        .aria_label(state_text.clone())
                        .text_size(px(text::CAPTION))
                        .text_color(rgb(state_color))
                        .child(state_text),
                )
                .when(matches!(&view.state, TransferState::Offer { .. }), |el| {
                    el.child(action_button(
                        format!("xfer-accept-{id}"),
                        "接受",
                        true,
                        cx,
                        ChatPanelEvent::AcceptFile(id),
                    ))
                    .child(action_button(
                        format!("xfer-reject-{id}"),
                        "拒绝",
                        false,
                        cx,
                        ChatPanelEvent::RejectFile(id),
                    ))
                }),
        );

    if let TransferState::Active(fraction) = &view.state {
        let text = percent(*fraction);
        row = row.child(
            div()
                .flex()
                .items_center()
                .gap(px(space::S2))
                .child(
                    div()
                        .flex_1()
                        .h(px(6.))
                        .rounded(px(3.))
                        .bg(rgb(color::TRACK))
                        .child(
                            div()
                                .h_full()
                                .w(relative(fraction.clamp(0.0, 1.0)))
                                .rounded(px(3.))
                                .bg(rgb(color::ACCENT)),
                        ),
                )
                .child(
                    div()
                        .id(format!("xfer-progress-{id}"))
                        .test_support()
                        .aria_label(text.clone())
                        .text_size(px(text::CAPTION))
                        .text_color(rgb(color::TEXT_SECONDARY))
                        .child(text),
                ),
        );
    }

    row
}
