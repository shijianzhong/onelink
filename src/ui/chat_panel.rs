//! 聊天面板：消息列表（自己右对齐、对方左对齐）+ 多行输入卡片 + 发送按钮。
//!
//! 发送不在组件内直接做网络 I/O：按钮把文本以 [`ChatPanelEvent::Send`] 抛出，
//! 由 `App` 订阅后投递给 iroh 服务（UI 线程永不阻塞）。

use std::path::PathBuf;

use gpui_kit::base::TestSupportExt;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::{
    App, Context, Entity, EventEmitter, ExternalPaths, FontWeight, IntoElement, ParentElement,
    Render, Styled, Subscription, Window, div, prelude::*, px, rgb,
};

use crate::chat::ChatMessage;
use crate::ui::theme::{color, radius, size, space, text};
use crate::ui::transfer_row::{self, TransferView};

/// 面板向外抛的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatPanelEvent {
    /// 用户点了发送，携带已 trim 校验过的文本。
    Send(String),
    /// 用户点了 📎，请求选择要发送的文件。
    PickFile,
    /// 用户同意接收 `xfer_id` 对应的文件。
    AcceptFile(u64),
    /// 用户拒绝接收 `xfer_id` 对应的文件。
    RejectFile(u64),
    /// 用户把文件拖进聊天面板，携带拖入的路径（至少一个）。
    DropFiles(Vec<PathBuf>),
}

impl EventEmitter<ChatPanelEvent> for ChatPanel {}

pub struct ChatPanel {
    input: Entity<TextareaState>,
    peer: Option<String>,
    /// 会话标题显示的对方名（备注优先，由 `App` 推送；`None` 时回落到短 id）。
    peer_name: Option<String>,
    messages: Vec<ChatMessage>,
    /// 当前会话的传输行（由 `App` 按 peer 维护后推入）。
    transfers: Vec<TransferView>,
    /// 最近一次发送失败的提示（成功发送或送达后清空）。
    error: Option<String>,
    /// 输入框事件订阅（仅 Enter 发送）。
    _input_sub: Subscription,
}

impl ChatPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // 多行输入：回车发送、Shift+Enter 换行（TextareaMode 的多行属性决定）。
        let input = cx.new(|cx| TextareaState::new(window, cx).submit_on_enter(true));
        input.update(cx, |state, cx| {
            state.set_placeholder("输入消息…（Enter 发送，Shift+Enter 换行）", window, cx);
        });
        let input_sub = cx.subscribe_in(
            &input,
            window,
            |this, _input, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift, .. } = ev
                    && !*shift
                {
                    this.send(window, cx);
                }
            },
        );
        Self {
            input,
            peer: None,
            peer_name: None,
            messages: Vec::new(),
            transfers: Vec::new(),
            error: None,
            _input_sub: input_sub,
        }
    }

    /// 刷新当前会话的传输行（无变化不重绘）。
    pub fn set_transfers(&mut self, transfers: Vec<TransferView>, cx: &mut Context<Self>) {
        if self.transfers == transfers {
            return;
        }
        self.transfers = transfers;
        cx.notify();
    }

    /// 设置/清除发送失败提示。
    pub fn set_error(&mut self, error: Option<String>, cx: &mut Context<Self>) {
        if self.error == error {
            return;
        }
        self.error = error;
        cx.notify();
    }

    /// 切换会话标题上显示的对方名（备注改名后由 `App` 立即推送）。
    pub fn set_peer_name(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        if self.peer_name == name {
            return;
        }
        self.peer_name = name;
        cx.notify();
    }

    /// 切换到某个会话并刷新消息列表（不清输入框内容）。
    pub fn show(&mut self, peer: &str, messages: Vec<ChatMessage>, cx: &mut Context<Self>) {
        let peer_changed = self.peer.as_deref() != Some(peer);
        if peer_changed {
            self.peer_name = None;
        }
        self.peer = Some(peer.to_string());
        self.messages = messages;
        cx.notify();
    }

    /// 测试与预填用：直接设置输入框文本。
    pub fn set_input_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |state, cx| {
            state.set_value(text.to_string(), window, cx);
        });
    }

    /// 当前输入框文本。
    pub fn input_text(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    /// 发送：空文本不发；发出后清空输入框与上次的失败提示。
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.peer.is_none() {
            return;
        }
        let text = self.input_text(cx);
        if text.trim().is_empty() {
            return;
        }
        self.input.update(cx, |state, cx| {
            state.set_value(String::new(), window, cx);
        });
        self.error = None;
        cx.emit(ChatPanelEvent::Send(text));
    }
}

/// 把毫秒时间戳格式化成 `HH:MM`。
///
/// 固定东八区：这是面向局域网熟人场景的中文 IM，不为一个时间戳引入时区依赖。
fn clock(sent_at_ms: u64) -> String {
    let seconds = sent_at_ms / 1000 + 8 * 3600;
    let hour = (seconds / 3600) % 24;
    let minute = (seconds / 60) % 60;
    format!("{hour:02}:{minute:02}")
}

fn message_row(msg: &ChatMessage) -> impl IntoElement {
    // 双方 id 空间独立，同一数字 id 会在同一会话里出现；element id 必须带方向前缀。
    let side = if msg.mine { "mine" } else { "peer" };
    let bubble = div()
        .id(format!("msg-{side}-{}", msg.id))
        .test_support()
        .aria_label(msg.text.clone())
        .px(px(space::S3))
        .py(px(space::S2))
        .rounded(px(radius::MD))
        .max_w(px(size::BUBBLE_MAX_W))
        .line_height(px(size::MESSAGE_LINE_H))
        .text_size(px(text::MESSAGE))
        .child(msg.text.clone());

    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .when(msg.mine, |row| row.items_end())
        .when(!msg.mine, |row| row.items_start())
        .child(
            bubble
                .when(msg.mine, |b| {
                    b.bg(rgb(color::BUBBLE_MINE)).text_color(rgb(color::TEXT))
                })
                .when(!msg.mine, |b| {
                    b.bg(rgb(color::SURFACE))
                        .border_1()
                        .border_color(rgb(color::BUBBLE_PEER_BORDER))
                        .text_color(rgb(color::TEXT))
                }),
        )
        // 时间戳与状态弱化：11px 灰字，贴在同一侧气泡下方。
        .child({
            let stamp = clock(msg.sent_at_ms);
            div()
                .id(format!("msg-time-{side}-{}", msg.id))
                .test_support()
                .aria_label(stamp.clone())
                .text_size(px(text::CAPTION))
                .text_color(rgb(color::TEXT_MUTED))
                .child(stamp)
        })
}

impl Render for ChatPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, subtitle) = match &self.peer {
            Some(peer) => {
                let short: String = peer.chars().take(8).collect();
                let name = self
                    .peer_name
                    .clone()
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or(short);
                (
                    format!("与 {name} 聊天"),
                    "局域网直连 · 端到端传输".to_string(),
                )
            }
            None => (
                "选择左侧设备开始聊天".to_string(),
                "发现同一局域网内的设备后即可开始".to_string(),
            ),
        };
        let can_send = self.peer.is_some();

        let mut list = div()
            .id("chat-messages")
            .test_support()
            .flex()
            .flex_col()
            .gap(px(space::S3))
            .flex_1()
            .min_h(px(0.))
            .px(px(space::S6))
            .py(px(space::S3));
        if self.messages.is_empty() {
            list = list.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(text::BODY))
                    .text_color(rgb(color::TEXT_MUTED))
                    .child("还没有消息，发一条试试"),
            );
        }
        for msg in &self.messages {
            list = list.child(message_row(msg));
        }

        let transfers = (!self.transfers.is_empty()).then(|| {
            let mut block = div()
                .id("chat-transfers")
                .test_support()
                .flex()
                .flex_col()
                .gap(px(space::S2))
                .px(px(space::S6));
            for view in &self.transfers {
                block = block.child(transfer_row::render(view, cx));
            }
            block
        });

        let error_row = self.error.clone().map(|reason| {
            div()
                .id("chat-error")
                .test_support()
                .aria_label(reason.clone())
                .mx(px(space::S6))
                .px(px(space::S3))
                .py(px(space::S2))
                .rounded(px(radius::SM))
                .bg(rgb(color::DANGER_BG))
                .text_size(px(text::BODY))
                .text_color(rgb(color::DANGER))
                .child(format!("发送失败：{reason}"))
        });

        let attach = div()
            .id("chat-attach")
            .test_support()
            .aria_label("发送文件")
            .flex()
            .items_center()
            .justify_center()
            .w(px(size::ATTACH_W))
            .h(px(size::ATTACH_H))
            .rounded(px(radius::SM))
            .bg(rgb(color::BG))
            .text_size(px(text::MESSAGE))
            .hover(|button| button.bg(rgb(color::HOVER)))
            .child("📎")
            .on_click(cx.listener(|_this, _ev, _window, cx| {
                cx.emit(ChatPanelEvent::PickFile);
            }));

        let send = div()
            .id("chat-send")
            .test_support()
            .aria_label("发送")
            .flex()
            .items_center()
            .justify_center()
            .w(px(size::SEND_W))
            .h(px(size::SEND_H))
            .rounded(px(radius::SM))
            .bg(rgb(if can_send {
                color::ACCENT
            } else {
                color::DISABLED
            }))
            .text_color(rgb(color::SURFACE))
            .text_size(px(text::BODY))
            .font_weight(FontWeight::MEDIUM)
            .child("发送")
            .on_click(cx.listener(|this, _ev, window, cx| {
                this.send(window, cx);
            }));

        div()
            .id("chat-panel")
            .test_support()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(color::BG))
            .child(
                // 会话头：标题 + 弱化副标题，底部一条分隔线。
                div()
                    .id("chat-header")
                    .test_support()
                    .aria_label(title.clone())
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .px(px(space::S6))
                    .py(px(space::S3))
                    .border_b_1()
                    .border_color(rgb(color::DIVIDER))
                    .child(
                        div()
                            .text_size(px(text::TITLE))
                            .text_color(rgb(color::TEXT))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(text::CAPTION))
                            .text_color(rgb(color::TEXT_MUTED))
                            .child(subtitle),
                    ),
            )
            .child(list)
            .when_some(transfers, |el, block| el.child(block))
            .when_some(error_row, |el, row| el.child(row))
            .child(
                // 输入卡片：白底、圆角；描边为静态分隔色，聚焦不再改变边框，只靠光标提示。
                div()
                    .id("chat-composer")
                    .test_support()
                    .flex()
                    .flex_col()
                    .gap(px(space::S2))
                    .mx(px(space::S6))
                    .mb(px(space::S3))
                    .p(px(space::S3))
                    .rounded(px(radius::LG))
                    .bg(rgb(color::SURFACE))
                    .border_1()
                    .border_color(rgb(color::DIVIDER))
                    .child(
                        // wrapper 承载测试用的 `chat-input` id；点击中心即落在 Textarea 上。
                        div()
                            .id("chat-input")
                            .test_support()
                            .flex()
                            .flex_col()
                            .min_h(px(size::COMPOSER_INPUT_H))
                            .child(
                                // `appearance(false)`：不画组件自带背景/边框，也不触发聚焦 ring；
                                // 输入卡片的静态描边由外层容器统一提供。
                                Textarea::new(&self.input)
                                    .appearance(false)
                                    .h(px(size::COMPOSER_INPUT_H))
                                    .text_size(px(text::MESSAGE)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .child(attach)
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(text::CAPTION))
                                    .text_color(rgb(color::TEXT_MUTED))
                                    .child("拖动文件到窗口即可发送"),
                            )
                            .child(send),
                    ),
            )
            // 平台把外部文件拖入转成内部 drag，这里接收 drop 后只抛事件，不直接发网络请求。
            .on_drop::<ExternalPaths>({
                let this = cx.weak_entity();
                move |paths: &ExternalPaths, _window, cx| {
                    let files: Vec<PathBuf> = paths.paths().to_vec();
                    if files.is_empty() {
                        return;
                    }
                    this.update(cx, move |_this, cx| {
                        cx.emit(ChatPanelEvent::DropFiles(files));
                    })
                    .ok();
                }
            })
    }
}
