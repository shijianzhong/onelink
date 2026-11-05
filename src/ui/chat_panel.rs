//! 聊天面板：消息列表（自己右对齐、对方左对齐）+ 输入框 + 发送按钮。
//!
//! 发送不在组件内直接做网络 I/O：按钮把文本以 [`ChatPanelEvent::Send`] 抛出，
//! 由 `App` 订阅后投递给 iroh 服务（UI 线程永不阻塞）。

use gpui_kit::base::TestSupportExt;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::{
    App, Context, Entity, EventEmitter, IntoElement, ParentElement, Render, Styled, Window, div,
    prelude::*, px, rgb,
};

use crate::chat::ChatMessage;

/// 面板向外抛的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatPanelEvent {
    /// 用户点了发送，携带已 trim 校验过的文本。
    Send(String),
}

impl EventEmitter<ChatPanelEvent> for ChatPanel {}

pub struct ChatPanel {
    input: Entity<InputState>,
    peer: Option<String>,
    messages: Vec<ChatMessage>,
}

impl ChatPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            input: cx.new(|cx| InputState::new(window, cx)),
            peer: None,
            messages: Vec::new(),
        }
    }

    /// 切换到某个会话并刷新消息列表（不清输入框内容）。
    pub fn show(&mut self, peer: &str, messages: Vec<ChatMessage>, cx: &mut Context<Self>) {
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

    /// 发送：空文本不发；发出后清空输入框。
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
        cx.emit(ChatPanelEvent::Send(text));
    }
}

fn message_row(msg: &ChatMessage) -> impl IntoElement {
    // 双方 id 空间独立，同一数字 id 会在同一会话里出现；element id 必须带方向前缀。
    let side = if msg.mine { "mine" } else { "peer" };
    let bubble = div()
        .id(format!("msg-{side}-{}", msg.id))
        .test_support()
        .aria_label(msg.text.clone())
        .px(px(10.))
        .py(px(6.))
        .rounded(px(8.))
        .max_w(px(520.))
        .child(msg.text.clone());

    if msg.mine {
        div()
            .flex()
            .justify_end()
            .child(bubble.bg(rgb(0x1A73E8)).text_color(rgb(0xFFFFFF)))
    } else {
        div().flex().justify_start().child(bubble.bg(rgb(0xF1F3F4)))
    }
}

impl Render for ChatPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = match &self.peer {
            Some(peer) => {
                let short: String = peer.chars().take(8).collect();
                format!("与 {short} 聊天")
            }
            None => "选择左侧设备开始聊天".to_string(),
        };
        let can_send = self.peer.is_some();

        let mut list = div()
            .id("chat-messages")
            .test_support()
            .flex()
            .flex_col()
            .gap(px(6.))
            .flex_1()
            .p(px(4.));
        for msg in &self.messages {
            list = list.child(message_row(msg));
        }

        div()
            .id("chat-panel")
            .test_support()
            .flex()
            .flex_col()
            .size_full()
            .gap(px(10.))
            .p(px(12.))
            .child(
                div()
                    .id("chat-header")
                    .test_support()
                    .aria_label(header.clone())
                    .text_size(px(13.))
                    .text_color(rgb(0x444444))
                    .child(header),
            )
            .child(list)
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.input).id("chat-input")),
                    )
                    .child(
                        div()
                            .id("chat-send")
                            .test_support()
                            .aria_label("发送")
                            .px(px(12.))
                            .py(px(6.))
                            .rounded(px(6.))
                            .bg(rgb(0x1A73E8))
                            .text_color(rgb(0xFFFFFF))
                            .when(!can_send, |s| s.text_color(rgb(0xAAAAAA)))
                            .child("发送")
                            .on_click(cx.listener(|this, _ev, window, cx| {
                                this.send(window, cx);
                            })),
                    ),
            )
    }
}
