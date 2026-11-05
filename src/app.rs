//! 根组件：左侧设备列表 + 右侧聊天面板，并运行事件泵把网络事件落地到两个子组件。

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    AsyncApp, Context, Entity, IntoElement, ParentElement, Render, Styled, Subscription,
    WeakEntity, Window, div, prelude::*, px, rgb,
};

use crate::chat::{ChatMessage, ChatStore};
use crate::net::{IrohServiceHandle, UiEvent};
use crate::proto::now_ms;
use crate::ui::chat_panel::{ChatPanel, ChatPanelEvent};
use crate::ui::device_list::{DeviceList, DeviceListEvent};

/// 事件泵轮询间隔。
const PUMP_INTERVAL: Duration = Duration::from_millis(300);

pub struct App {
    device_list: Entity<DeviceList>,
    chat_panel: Entity<ChatPanel>,
    chat_store: ChatStore,
    service: Arc<IrohServiceHandle>,
    /// 子组件事件订阅；drop 即取消订阅。
    _subs: Vec<Subscription>,
}

impl App {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        service: Arc<IrohServiceHandle>,
    ) -> Self {
        let device_list = cx.new(DeviceList::new);
        let chat_panel = cx.new(|cx| ChatPanel::new(window, cx));

        let sub_device = cx.subscribe_in(
            &device_list,
            window,
            |this, _list, ev: &DeviceListEvent, _window, cx| {
                let DeviceListEvent::Selected(peer) = ev;
                this.chat_store.select(peer);
                let messages = this.chat_store.messages(peer).to_vec();
                this.chat_panel
                    .update(cx, |panel, cx| panel.show(peer, messages, cx));
                let unread = this.chat_store.unread(peer);
                this.device_list
                    .update(cx, |list, cx| list.set_unread(peer, unread, cx));
                cx.notify();
            },
        );

        let sub_chat = cx.subscribe_in(
            &chat_panel,
            window,
            |this, _panel, ev: &ChatPanelEvent, _window, cx| {
                let ChatPanelEvent::Send(text) = ev;
                let Some(peer) = this.chat_store.selected().map(|s| s.to_string()) else {
                    return;
                };
                let id = this.service.send_chat(&peer, text);
                this.chat_store.append(
                    &peer,
                    ChatMessage {
                        id,
                        text: text.clone(),
                        sent_at_ms: now_ms(),
                        mine: true,
                    },
                );
                let messages = this.chat_store.messages(&peer).to_vec();
                this.chat_panel
                    .update(cx, |panel, cx| panel.show(&peer, messages, cx));
                cx.notify();
            },
        );

        let events = Arc::clone(&service.tx);
        cx.spawn(async move |this: WeakEntity<App>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(PUMP_INTERVAL).await;
                let mut batch = Vec::new();
                while let Some(ev) = events.try_recv() {
                    batch.push(ev);
                }
                if batch.is_empty() {
                    continue;
                }
                if this
                    .update(cx, |app, cx| app.apply_batch(batch, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        Self {
            device_list,
            chat_panel,
            chat_store: ChatStore::new(),
            service,
            _subs: vec![sub_device, sub_chat],
        }
    }

    /// 测试/调试用：左侧设备列表实体。
    pub fn device_list(&self) -> Entity<DeviceList> {
        self.device_list.clone()
    }

    /// 测试/调试用：右侧聊天面板实体。
    pub fn chat_panel(&self) -> Entity<ChatPanel> {
        self.chat_panel.clone()
    }

    /// 测试/调试用：会话模型。
    pub fn chat_store(&self) -> &ChatStore {
        &self.chat_store
    }

    /// 把一批网络事件应用到设备列表/会话模型/聊天面板（事件泵与测试共用）。
    pub fn apply_batch(&mut self, batch: Vec<UiEvent>, cx: &mut Context<Self>) {
        if batch.is_empty() {
            return;
        }
        for ev in batch {
            match ev {
                UiEvent::ChatReceived {
                    node_id,
                    id,
                    text,
                    sent_at_ms,
                } => {
                    self.chat_store.append(
                        &node_id,
                        ChatMessage {
                            id,
                            text,
                            sent_at_ms,
                            mine: false,
                        },
                    );
                    let unread = self.chat_store.unread(&node_id);
                    self.device_list.update(cx, |list, cx| {
                        list.set_unread(&node_id, unread, cx);
                    });
                    if self.chat_store.selected() == Some(node_id.as_str()) {
                        let messages = self.chat_store.messages(&node_id).to_vec();
                        self.chat_panel
                            .update(cx, |panel, cx| panel.show(&node_id, messages, cx));
                    }
                }
                UiEvent::ChatDelivered { node_id, id } => {
                    tracing::debug!(node_id, id, "chat delivered");
                }
                UiEvent::ChatFailed {
                    node_id,
                    id,
                    reason,
                } => {
                    // 面板级失败提示留 M3；M2 只落日志，UI 不卡。
                    tracing::warn!(node_id, id, %reason, "chat send failed");
                }
                other => {
                    self.device_list
                        .update(cx, |list, cx| list.apply(other, cx));
                }
            }
        }
        cx.notify();
    }
}

impl Render for App {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(16.))
                    .py(px(10.))
                    .text_size(px(16.))
                    .child("OneLink"),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.))
                    .child(
                        div()
                            .w(px(280.))
                            .flex()
                            .flex_col()
                            .bg(rgb(0xF7F8F9))
                            .child(self.device_list.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.))
                            .child(self.chat_panel.clone()),
                    ),
            )
    }
}
