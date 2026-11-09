//! 根组件：左侧设备列表 + 右侧聊天面板，并运行事件泵把网络事件落地到两个子组件。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::TestSupportExt;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::{
    AsyncApp, Context, Entity, FontWeight, IntoElement, KeyDownEvent, ParentElement,
    PathPromptOptions, Render, Styled, Subscription, WeakEntity, Window, div, prelude::*, px, rgb,
};

use crate::chat::{ChatMessage, ChatStore, MessageStatus};
use crate::net::{IrohServiceHandle, UiEvent};
use crate::proto::now_ms;
use crate::store::{Loaded, Store};
use crate::ui::chat_panel::{ChatPanel, ChatPanelEvent};
use crate::ui::device_list::{DeviceList, DeviceListEvent};
use crate::ui::theme::{color, radius, size, space, text};
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
    /// 本地持久化（异步单写线程）；UI 只投递命令，不阻塞。
    store: Store,
    /// 用户设置的昵称（空串 = 使用默认名）。
    nickname: String,
    /// 主机名兜底，界面上标为"默认名"。
    default_name: String,
    /// 是否正在编辑昵称。
    nickname_editing: bool,
    /// 昵称输入框（常驻，打开编辑时才显示）。
    nickname_input: Entity<TextareaState>,
    /// 子组件事件订阅；drop 即取消订阅。
    _subs: Vec<Subscription>,
}

impl App {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        service: Arc<IrohServiceHandle>,
        store: Store,
        loaded: Loaded,
    ) -> Self {
        let Loaded {
            devices,
            histories,
            self_nickname,
        } = loaded;
        let nickname = self_nickname.unwrap_or_default();
        let default_name = crate::net::local_device_name();
        let mut chat_store = ChatStore::new();
        chat_store.hydrate(histories);

        let nickname_input = cx.new(|cx| TextareaState::new(window, cx).submit_on_enter(true));
        nickname_input.update(cx, |state, cx| {
            state.set_placeholder(format!("留空则使用默认名 {default_name}"), window, cx);
        });
        let sub_nickname = cx.subscribe_in(
            &nickname_input,
            window,
            |this, _input, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift, .. } = ev
                    && !*shift
                {
                    this.save_nickname(window, cx);
                }
            },
        );

        let device_list = cx.new(DeviceList::new);
        let chat_panel = cx.new(|cx| ChatPanel::new(window, cx));
        // 先把持久化的设备摆上列表（全部离线），未读角标按历史派生。
        device_list.update(cx, |list, cx| list.hydrate(devices, cx));
        let unread: Vec<(String, u32)> = chat_store
            .peers()
            .into_iter()
            .map(|peer| {
                let count = chat_store.unread(&peer);
                (peer, count)
            })
            .collect();
        for (peer, count) in unread {
            device_list.update(cx, |list, cx| list.set_unread(&peer, count, cx));
        }

        let sub_device = cx.subscribe_in(
            &device_list,
            window,
            |this, _list, ev: &DeviceListEvent, _window, cx| match ev {
                DeviceListEvent::Selected(peer) => {
                    this.chat_store.select(peer);
                    // 选中即已读：内存标记 + 落库，未读角标清零。
                    this.store.mark_read(peer);
                    let peer_name = this.display_name(peer, cx);
                    let messages = this.chat_store.messages(peer).to_vec();
                    let transfers = this.transfers.get(peer).cloned().unwrap_or_default();
                    this.chat_panel.update(cx, |panel, cx| {
                        panel.show(peer, messages, cx);
                        panel.set_peer_name(peer_name, cx);
                        panel.set_transfers(transfers, cx);
                    });
                    let unread = this.chat_store.unread(peer);
                    this.device_list
                        .update(cx, |list, cx| list.set_unread(peer, unread, cx));
                    cx.notify();
                }
                // 备注只影响本地显示；先落库，再让会话标题跟着改。
                DeviceListEvent::AliasSubmitted { node_id, alias } => {
                    this.store.set_alias(node_id, alias.as_deref());
                    if this.chat_store.selected() == Some(node_id.as_str()) {
                        let name = this.display_name(node_id, cx);
                        this.chat_panel
                            .update(cx, |panel, cx| panel.set_peer_name(name, cx));
                    }
                    cx.notify();
                }
                // 取消编辑不产生任何写入。
                DeviceListEvent::AliasCancelled => {}
                // 删除离线设备：设备行 + 会话 + 历史消息一并删除（UI 已二次确认，不可撤销）。
                DeviceListEvent::DeleteRequested(node_id) => {
                    let was_selected = this.chat_store.selected() == Some(node_id.as_str());
                    this.chat_store.forget(node_id.as_str());
                    this.transfers.remove(node_id);
                    this.store.delete_device(node_id.as_str());
                    this.device_list
                        .update(cx, |list, cx| list.forget(node_id.as_str(), cx));
                    if was_selected {
                        this.chat_panel.update(cx, |panel, cx| panel.clear(cx));
                    }
                    cx.notify();
                }
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
                    // 出站 id 从库里续号（重启后接着历史最大值递增），避免与历史冲突。
                    let id = this.chat_store.next_out_msg_id(&peer);
                    this.service.send_chat_with_id(&peer, text, id);
                    let msg = ChatMessage {
                        id,
                        text: text.clone(),
                        sent_at_ms: now_ms(),
                        mine: true,
                        status: MessageStatus::Pending,
                        read: false,
                    };
                    this.chat_store.append(&peer, msg.clone());
                    this.store.insert_message(&peer, &msg);
                    let messages = this.chat_store.messages(&peer).to_vec();
                    this.chat_panel
                        .update(cx, |panel, cx| panel.show(&peer, messages, cx));
                    cx.notify();
                }
                ChatPanelEvent::PickFile => this.pick_file(cx),
                ChatPanelEvent::DropFiles(paths) => this.send_dropped(paths.clone(), cx),
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
            chat_store,
            transfers: BTreeMap::new(),
            service,
            store,
            nickname,
            default_name,
            nickname_editing: false,
            nickname_input,
            _subs: vec![sub_device, sub_chat, sub_nickname],
        }
    }

    /// 设备显示名（备注优先、其次握手名）。未知设备返回 `None`。
    fn display_name(&self, node_id: &str, cx: &gpui_kit::App) -> Option<String> {
        self.device_list
            .read(cx)
            .device(node_id)
            .map(|d| d.display_name().to_string())
    }

    /// 品牌头那行昵称文案：空设置回退主机名并标注"默认"。
    fn nickname_label(&self) -> String {
        if self.nickname.trim().is_empty() {
            format!("我的昵称：{}（默认）", self.default_name)
        } else {
            format!("我的昵称：{}", self.nickname)
        }
    }

    /// 打开昵称编辑：预填当前设置值并聚焦。
    fn begin_nickname_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.nickname.clone();
        self.nickname_input.update(cx, |state, cx| {
            state.set_value(value, window, cx);
            state.focus(window, cx);
        });
        self.nickname_editing = true;
        cx.notify();
    }

    /// 提交昵称：空白串按"使用默认名"处理；立即写库并换掉运行期的握手名。
    fn save_nickname(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.nickname_input.read(cx).value().to_string();
        let nickname = raw.trim().to_string();
        self.store.set_self_nickname(&nickname);
        self.service.set_own_name(&nickname);
        self.nickname = nickname;
        self.nickname_editing = false;
        cx.notify();
    }

    /// 取消昵称编辑：不写库，仅收起编辑态。
    fn cancel_nickname(&mut self, cx: &mut Context<Self>) {
        self.nickname_editing = false;
        cx.notify();
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
            this.update(cx, |app, cx| app.send_path(peer, path, cx))
                .ok();
        })
        .detach();
    }

    /// 拖入文件：只发给当前选中会话；一次拖入多个时只发第一个（无队列，M4 再做）。
    fn send_dropped(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some(peer) = self.chat_store.selected().map(str::to_string) else {
            tracing::info!("dropped file ignored: no peer selected");
            return;
        };
        let Some(path) = paths.into_iter().next() else {
            return;
        };
        self.send_path(peer, path, cx);
    }

    /// 把路径交给 iroh 服务，并在当前会话加一条 `Waiting` 传输行（📎 与拖拽共用）。
    fn send_path(&mut self, peer: String, path: PathBuf, cx: &mut Context<Self>) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        let xfer_id = self.service.send_file(&peer, path);
        self.transfers.entry(peer).or_default().push(TransferView {
            xfer_id,
            name,
            state: TransferState::Waiting,
        });
        self.push_transfers(cx);
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

    /// 当前网络层使用的对外昵称（测试/回显用）。
    pub fn service_own_name(&self) -> String {
        self.service.own_name()
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
                    let msg = ChatMessage {
                        id,
                        text,
                        sent_at_ms,
                        mine: false,
                        status: MessageStatus::Pending,
                        // 正在看的会话直接算已读，其余留作未读。
                        read: self.chat_store.selected() == Some(node_id.as_str()),
                    };
                    // 重复到达的消息（同一 peer/id）不渲染也不落库。
                    if self.chat_store.append(&node_id, msg.clone()) {
                        self.store.insert_message(&node_id, &msg);
                    }
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
                    self.chat_store
                        .set_status(&node_id, id, MessageStatus::Delivered);
                    self.store
                        .set_message_status(&node_id, id, MessageStatus::Delivered);
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
                    self.chat_store
                        .set_status(&node_id, id, MessageStatus::Failed);
                    self.store
                        .set_message_status(&node_id, id, MessageStatus::Failed);
                    // 只在失败发生在用户正看的会话时提示；后台会话的失败留日志（会话级标记再说）。
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
                    let node_id = match &other {
                        UiEvent::DeviceDiscovered { node_id, .. }
                        | UiEvent::DeviceNamed { node_id, .. }
                        | UiEvent::DeviceLost { node_id } => Some(node_id.clone()),
                        _ => None,
                    };
                    self.device_list
                        .update(cx, |list, cx| list.apply(other, cx));
                    // 设备发现/改名/离线的结果落库（备注列不会被这里覆盖）。
                    if let Some(node_id) = node_id
                        && let Some(info) = self.device_list.read(cx).device(&node_id)
                    {
                        self.store.upsert_device(&info);
                    }
                }
            }
        }
        cx.notify();
    }
}

/// 品牌头的昵称编辑行：Enter 提交（输入框订阅里处理）、Esc 取消。
fn nickname_editor_row(
    input: &Entity<TextareaState>,
    default_name: &str,
    cx: &mut Context<App>,
) -> gpui_kit::AnyElement {
    let action = |id: &str, label: &'static str, accent: bool| {
        div()
            .id(id.to_string())
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
        .id("nickname-editor")
        .test_support()
        .flex()
        .flex_col()
        .gap(px(space::S1))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(space::S1))
                .w_full()
                .child(
                    div()
                        .id("nickname-input")
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
                    action("nickname-save", "保存", true).on_click(
                        cx.listener(|this, _ev, window, cx| this.save_nickname(window, cx)),
                    ),
                )
                .child(
                    action("nickname-cancel", "取消", false)
                        .on_click(cx.listener(|this, _ev, _window, cx| this.cancel_nickname(cx))),
                ),
        )
        .child(
            div()
                .text_size(px(text::CAPTION))
                .text_color(rgb(color::TEXT_MUTED))
                .child(format!("留空则使用默认名：{default_name}")),
        )
        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| {
            if ev.keystroke.key == "escape" {
                this.cancel_nickname(cx);
            }
        }))
        .into_any_element()
}

impl Render for App {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let nickname_label = self.nickname_label();
        let nickname_line = (!self.nickname_editing).then(|| {
            div()
                .id("nickname-line")
                .test_support()
                .aria_label(nickname_label.clone())
                .text_size(px(text::CAPTION))
                .text_color(rgb(color::TEXT_MUTED))
                .child(nickname_label)
        });
        let nickname_editor = self
            .nickname_editing
            .then(|| nickname_editor_row(&self.nickname_input, &self.default_name, cx));
        div()
            .id("app-root")
            .test_support()
            .size_full()
            .flex()
            .flex_row()
            .bg(rgb(color::BG))
            .child(
                // 左侧栏：品牌头 + 设备列表；与聊天区用 1px 分隔线区分，不再靠边框卡片。
                div()
                    .id("app-sidebar")
                    .test_support()
                    .w(px(size::SIDEBAR_W))
                    .flex()
                    .flex_col()
                    .bg(rgb(color::SIDEBAR))
                    .border_r_1()
                    .border_color(rgb(color::DIVIDER))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(space::S2))
                            .px(px(space::S3))
                            .py(px(space::S3))
                            .border_b_1()
                            .border_color(rgb(color::DIVIDER))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .w_full()
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_size(px(text::BRAND))
                                            .text_color(rgb(color::TEXT))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("OneLink"),
                                    )
                                    .child(
                                        div()
                                            .id("nickname-edit")
                                            .test_support()
                                            .aria_label("设置昵称")
                                            .px(px(space::S1))
                                            .rounded(px(radius::SM))
                                            .text_size(px(text::CAPTION))
                                            .text_color(rgb(color::TEXT_MUTED))
                                            .hover(|el| el.text_color(rgb(color::TEXT_SECONDARY)))
                                            .child("昵称")
                                            .on_click(cx.listener(|this, _ev, window, cx| {
                                                this.begin_nickname_edit(window, cx);
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(text::CAPTION))
                                    .text_color(rgb(color::TEXT_MUTED))
                                    .child("局域网直连 · 无需服务器"),
                            )
                            .when_some(nickname_line, |col, line| col.child(line))
                            .when_some(nickname_editor, |col, editor| col.child(editor)),
                    )
                    .child(
                        // 列方向 flex：子项按容器宽度拉伸，避免设备列表收缩到内容宽度。
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .w_full()
                            .min_h(px(0.))
                            .child(self.device_list.clone()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.))
                    .child(self.chat_panel.clone()),
            )
    }
}
