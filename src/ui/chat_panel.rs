//! 聊天面板：消息列表（自己右对齐、对方左对齐）+ 多行输入卡片 + 发送按钮。
//!
//! 发送不在组件内直接做网络 I/O：按钮把文本以 [`ChatPanelEvent::Send`] 抛出，
//! 由 `App` 订阅后投递给 iroh 服务（UI 线程永不阻塞）。

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::base::input::Copy;
use gpui_kit::base::{SelectableText, TestSupportExt, TextSelection};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::{
    App, ClipboardItem, Context, Entity, EventEmitter, ExternalPaths, FocusHandle, FontWeight,
    IntoElement, KeyBinding, MouseButton, MouseDownEvent, ParentElement, Render, ScrollHandle,
    Styled, Subscription, Window, actions, div, point, prelude::*, px, rgb,
};

use crate::chat::ChatMessage;
use crate::ui::theme::{color, radius, size, space, text};
use crate::ui::thin_scrollbar;
use crate::ui::transfer_row::{self, TransferView};

actions!(chat_panel, [SendMessage]);

/// 消息区获得焦点后的键位上下文：选中消息文字后 Cmd/Ctrl+C 复制选区、
/// Enter 继续发送（此时输入框已失焦，靠这里的绑定兜住既有交互）。
const CHAT_CONTEXT: &str = "ChatPanel";

/// 「已复制」反馈的停留时长。
const COPIED_FEEDBACK: Duration = Duration::from_millis(1200);

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
    /// 消息区焦点：拖选/点击消息时把焦点移到这里，让 Cmd+C 走面板的复制绑定。
    focus_handle: FocusHandle,
    /// 消息列表滚动位置（新消息自动贴底）。
    messages_scroll: ScrollHandle,
    /// 是否还需要把最后一条消息对齐到视口底部（等首帧布局量出位置再对齐）。
    pending_scroll_bottom: bool,
    /// 当前 hover 的消息（决定是否显示该条的复制按钮）。
    hovered_message: Option<(bool, u64)>,
    /// 刚通过复制按钮复制成功的消息（短暂显示「已复制」）。
    copied_message: Option<(bool, u64)>,
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
        let focus_handle = cx.focus_handle();
        cx.bind_keys([
            KeyBinding::new("cmd-c", Copy, Some(CHAT_CONTEXT)),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new("ctrl-c", Copy, Some(CHAT_CONTEXT)),
            KeyBinding::new("enter", SendMessage, Some(CHAT_CONTEXT)),
        ]);
        Self {
            input,
            peer: None,
            peer_name: None,
            messages: Vec::new(),
            transfers: Vec::new(),
            error: None,
            _input_sub: input_sub,
            focus_handle,
            messages_scroll: ScrollHandle::new(),
            pending_scroll_bottom: false,
            hovered_message: None,
            copied_message: None,
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
        // 进入会话/来新消息时请求贴底：对齐放到 render 里做，用实测的子项位置，
        // 避免 GPUI 把 padding 也算进滚动范围后留下的残余偏移。
        self.pending_scroll_bottom = true;
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

    /// 面板获得焦点时的复制：复制窗口级文本选区（消息正文）。
    fn copy_selected(&mut self, _: &Copy, window: &mut Window, cx: &mut Context<Self>) {
        let text = TextSelection::selected_text(window, cx).trim().to_string();
        if text.is_empty() {
            cx.propagate();
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    /// 面板获得焦点时的 Enter：维持「Enter 发送」。
    fn send_on_enter(&mut self, _: &SendMessage, window: &mut Window, cx: &mut Context<Self>) {
        self.send(window, cx);
    }

    /// 记录 hover 到哪条消息：只在该行显示复制入口，不改变任何布局。
    fn set_message_hovered(&mut self, key: (bool, u64), hovered: bool, cx: &mut Context<Self>) {
        let next = if hovered {
            Some(key)
        } else if self.hovered_message == Some(key) {
            None
        } else {
            self.hovered_message
        };
        if next != self.hovered_message {
            self.hovered_message = next;
            cx.notify();
        }
    }

    /// hover 复制入口：复制整条消息文本，并短暂显示「已复制」。
    fn copy_message(&mut self, key: (bool, u64), text: String, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied_message = Some(key);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FEEDBACK).await;
            this.update(cx, |this, cx| {
                if this.copied_message == Some(key) {
                    this.copied_message = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
    /// 贴底：用上一帧测到的最后一条消息位置对齐视口底部。
    ///
    /// 不用 `ScrollHandle::scroll_to_bottom`：GPUI 会把容器 padding 计入滚动范围，
    /// 内容不满一屏时也会留下残余负偏移，把首条消息顶出可视区上沿（点不到、hover 失效）。
    fn stick_to_bottom(&mut self, cx: &mut Context<Self>) {
        if !self.pending_scroll_bottom {
            return;
        }
        if self.messages.is_empty() {
            self.pending_scroll_bottom = false;
            return;
        }
        let Some(last) = self
            .messages_scroll
            .bounds_for_item(self.messages.len() - 1)
        else {
            // 首帧还没有布局数据：这一帧画完后会带上子项位置，再对齐一次。
            cx.notify();
            return;
        };
        let list = self.messages_scroll.bounds();
        // 子项 bounds 是未叠加滚动偏移的布局坐标，因此差值可直接累加到当前偏移。
        let delta = list.bottom() - px(space::S3) - last.bottom();
        let max = self.messages_scroll.max_offset().y;
        let y = (self.messages_scroll.offset().y + delta).clamp(-max, px(0.));
        self.messages_scroll.set_offset(point(px(0.), y));
        self.pending_scroll_bottom = false;
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

fn message_row(
    msg: &ChatMessage,
    hovered: bool,
    copied: bool,
    cx: &mut Context<ChatPanel>,
) -> impl IntoElement {
    // 双方 id 空间独立，同一数字 id 会在同一会话里出现；element id 必须带方向前缀。
    let side = if msg.mine { "mine" } else { "peer" };
    let key = (msg.mine, msg.id);
    let copy_text = msg.text.clone();

    // hover 复制入口：绝对定位在气泡外侧，既不占布局、也不压住正文，
    // 因此不会让文字重排或消息跳动；固定宽高避免「复制/已复制」切换时抽动。
    let copy_chip = div()
        .id(format!("msg-copy-{side}-{}", msg.id))
        .test_support()
        .aria_label(if copied { "已复制" } else { "复制" })
        .absolute()
        .top(px(0.))
        // 自己发的消息右对齐 -> 按钮放在气泡左侧；对方消息反之，始终落在空行区。
        .when(msg.mine, |el| el.left(px(-(size::COPY_BTN_W + space::S2))))
        .when(!msg.mine, |el| {
            el.right(px(-(size::COPY_BTN_W + space::S2)))
        })
        .flex()
        .items_center()
        .justify_center()
        .w(px(size::COPY_BTN_W))
        .h(px(size::COPY_BTN_H))
        .rounded(px(radius::SM))
        .bg(rgb(color::SURFACE))
        .border_1()
        .border_color(rgb(color::DIVIDER))
        .text_size(px(text::CAPTION))
        .text_color(rgb(if copied {
            color::ACCENT
        } else {
            color::TEXT_SECONDARY
        }))
        .child(if copied { "已复制" } else { "复制" })
        .on_click(cx.listener(move |this, _ev, _window, cx| {
            this.copy_message(key, copy_text.clone(), cx);
        }));

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
        // 点到消息正文时把焦点从输入框移到消息区：输入框会抢先消费 Cmd+C，
        // 焦点在这里时复制才会落到窗口级选区上（Enter 由面板的同名上下文兜住）。
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.focus_handle.focus(window, cx);
            }),
        )
        // 正文用 `SelectableText`：普通文本元素不支持选择，而它直接参与窗口级
        // `TextSelection`（Root 已渲染选择层并绑定 Cmd/Ctrl+C 复制）。
        .child(SelectableText::new(
            format!("msg-text-{side}-{}", msg.id),
            msg.text.clone(),
        ))
        .when(hovered, |bubble| bubble.child(copy_chip));

    div()
        .id(format!("msg-row-{side}-{}", msg.id))
        .test_support()
        // hover 只切换该行的复制入口显隐，不改动任何尺寸/间距。
        .on_hover(cx.listener(move |this, is_hovered: &bool, _window, cx| {
            this.set_message_hovered(key, *is_hovered, cx);
        }))
        .flex()
        .flex_col()
        // 消息按内容高度排布：多到超出可视高度时滚动，而不是被 flex 压扁。
        .flex_shrink_0()
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
        self.stick_to_bottom(cx);
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
            .py(px(space::S3))
            // 消息多到超出可视高度时滚动；滚动条覆盖在右侧留白上，不占消息宽度。
            .overflow_y_scroll()
            .track_scroll(&self.messages_scroll);
        if self.messages.is_empty() {
            list = list.child(
                div()
                    .id("chat-empty")
                    .test_support()
                    .aria_label("还没有消息，发一条试试")
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
            let key = (msg.mine, msg.id);
            let hovered = self.hovered_message == Some(key);
            let copied = self.copied_message == Some(key);
            list = list.child(message_row(msg, hovered, copied, cx));
        }

        // 滚动条必须是滚动容器的兄弟节点，而不能是它的子节点：`Scrollbar` 自身是
        // 100%×100% 的绝对定位元素，一旦作为子元素参与 `content_size` 的并集计算，
        // 就会凭空把内边距算成可滚动范围，内容不满一屏也会冒出滚动条。
        // 外层只负责定位（relative）和撑满高度，本身不滚动。
        let messages = div()
            .id("chat-messages-viewport")
            .test_support()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.))
            .child(list)
            .child(thin_scrollbar(&self.messages_scroll));

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
            .track_focus(&self.focus_handle)
            .key_context(CHAT_CONTEXT)
            .on_action(cx.listener(Self::copy_selected))
            .on_action(cx.listener(Self::send_on_enter))
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
            .child(messages)
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
