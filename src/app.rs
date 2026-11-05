//! 根组件：左侧设备列表 + 右侧聊天面板，并运行事件泵把网络事件落地到两个子组件。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    AsyncApp, Context, Entity, IntoElement, ParentElement, PathPromptOptions, Render, Styled,
    Subscription, WeakEntity, Window, div, prelude::*, px, rgb,
};

use crate::chat::{ChatMessage, ChatStore};
use crate::net::{IrohServiceHandle, UiEvent};
use crate::proto::now_ms;
use crate::ui::chat_panel::{ChatPanel, ChatPanelEvent};
use crate::ui::device_list::{DeviceList, DeviceListEvent};
use crate::ui::transfer_row::{TransferState, TransferView};

/// 事件泵轮询间隔。
const PUMP_INTERVAL: Duration = Duration::from_millis(300);

pub struct App {
    device_list: Entity<DeviceList>,
    chat_panel: Entity<ChatPanel>,
    chat_store: ChatStore,
    /// 按 peer 归属的传输行（会话切换时整份推给聊天面板）。
    transfers: BTreeMap<String, Vec<TransferView>>,
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
                let transfers = this.transfers.get(peer).cloned().unwrap_or_default();
                this.chat_panel.update(cx, |panel, cx| {
                    panel.show(peer, messages, cx);
                    panel.set_transfers(transfers, cx);
                });
                let unread = this.chat_store.unread(peer);
                this.device_list
                    .update(cx, |list, cx| list.set_unread(peer, unread, cx));
                cx.notify();
            },
        );

        let sub_chat = cx.subscribe_in(
            &chat_panel,
            window,
            |this, _panel, ev: &ChatPanelEvent, _window, cx| match ev {
                ChatPanelEvent::Send(text) => {
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
                }
                ChatPanelEvent::PickFile => this.pick_file(cx),
                ChatPanelEvent::AcceptFile(xfer_id) => {
                    this.service.accept_file(*xfer_id);
                    this.update_transfer_of_selected(*xfer_id, TransferState::Active(0.0), cx);
                }
                ChatPanelEvent::RejectFile(xfer_id) => {
                    this.service.reject_file(*xfer_id, "用户拒绝");
                    this.update_transfer_of_selected(
                        *xfer_id,
                        TransferState::Failed("已拒绝".to_string()),
                        cx,
                    );
                }
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
            transfers: BTreeMap::new(),
            service,
            _subs: vec![sub_device, sub_chat],
        }
    }

    /// 弹出系统文件选择框；选中后把路径交给 iroh 服务并发一条 `Waiting` 传输行。
    fn pick_file(&mut self, cx: &mut Context<Self>) {
        let Some(peer) = self.chat_store.selected().map(str::to_string) else {
            return;
        };
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("选择要发送的文件".into()),
        });
        cx.spawn(async move |this: WeakEntity<App>, cx: &mut AsyncApp| {
            let paths = match rx.await {
                Ok(Ok(Some(paths))) => paths,
                _ => return,
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |app, cx| {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.display().to_string());
                let xfer_id = app.service.send_file(&peer, path);
                app.transfers.entry(peer).or_default().push(TransferView {
                    xfer_id,
                    name,
                    state: TransferState::Waiting,
                });
                app.push_transfers(cx);
            })
            .ok();
        })
        .detach();
    }

    /// 更新当前选中会话里某条传输行的状态。
    fn update_transfer_of_selected(
        &mut self,
        xfer_id: u64,
        state: TransferState,
        cx: &mut Context<Self>,
    ) {
        if let Some(peer) = self.chat_store.selected().map(str::to_string) {
            self.update_transfer(&peer, xfer_id, state, cx);
        }
    }

    /// 更新 `peer` 会话里某条传输行的状态（行不存在时忽略）。
    fn update_transfer(
        &mut self,
        peer: &str,
        xfer_id: u64,
        state: TransferState,
        cx: &mut Context<Self>,
    ) {
        if let Some(row) = self
            .transfers
            .get_mut(peer)
            .and_then(|rows| rows.iter_mut().find(|r| r.xfer_id == xfer_id))
        {
            row.state = state;
        }
        self.push_transfers(cx);
    }

    /// 把当前选中会话的传输行推给聊天面板。
    fn push_transfers(&mut self, cx: &mut Context<Self>) {
        let Some(peer) = self.chat_store.selected().map(str::to_string) else {
            return;
        };
        let rows = self.transfers.get(&peer).cloned().unwrap_or_default();
        self.chat_panel
            .update(cx, |panel, cx| panel.set_transfers(rows, cx));
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
                    // 送达即清掉当前会话的失败提示（只影响用户正在看的那个会话）。
                    if self.chat_store.selected() == Some(node_id.as_str()) {
                        self.chat_panel
                            .update(cx, |panel, cx| panel.set_error(None, cx));
                    }
                }
                UiEvent::ChatFailed {
                    node_id,
                    id,
                    reason,
                } => {
                    tracing::warn!(node_id, id, %reason, "chat send failed");
                    // 只在失败发生在用户正看的会话时提示；后台会话的失败留日志（M3 的会话级标记再说）。
                    if self.chat_store.selected() == Some(node_id.as_str()) {
                        let reason = reason.clone();
                        self.chat_panel
                            .update(cx, |panel, cx| panel.set_error(Some(reason), cx));
                    }
                }
                UiEvent::TransferOffer {
                    node_id,
                    xfer_id,
                    name,
                    size,
                } => {
                    let rows = self.transfers.entry(node_id).or_default();
                    match rows.iter_mut().find(|r| r.xfer_id == xfer_id) {
                        Some(row) => {
                            row.name = name;
                            row.state = TransferState::Offer { size };
                        }
                        None => rows.push(TransferView {
                            xfer_id,
                            name,
                            state: TransferState::Offer { size },
                        }),
                    }
                    self.push_transfers(cx);
                }
                UiEvent::TransferProgress {
                    node_id,
                    xfer_id,
                    transferred,
                    total,
                } => {
                    let fraction = if total == 0 {
                        0.0
                    } else {
                        transferred as f32 / total as f32
                    };
                    self.update_transfer(&node_id, xfer_id, TransferState::Active(fraction), cx);
                }
                UiEvent::TransferCompleted {
                    node_id, xfer_id, ..
                } => {
                    self.update_transfer(&node_id, xfer_id, TransferState::Done, cx);
                }
                UiEvent::TransferFailed {
                    node_id,
                    xfer_id,
                    reason,
                }
                | UiEvent::TransferRejected {
                    node_id,
                    xfer_id,
                    reason,
                } => {
                    self.update_transfer(&node_id, xfer_id, TransferState::Failed(reason), cx);
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
