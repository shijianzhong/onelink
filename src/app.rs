//! 根组件：左侧设备列表 + 右侧聊天面板，并运行事件泵把网络事件落地到两个子组件。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::TestSupportExt;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::{
    AsyncApp, Context, Entity, FontWeight, Image, ImageFormat, ImageSource, IntoElement,
    KeyDownEvent, ParentElement, PathPromptOptions, Render, Styled, Subscription, WeakEntity,
    Window, div, img, prelude::*, px, rgb,
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

/// 设置页里自己的头像预览边长（比列表头像大一档，方便看清）。
const AVATAR_PREVIEW: f32 = 64.;

/// 根组件当前展示的页面。
///
/// gpui-kit 0.7 没有路由/多页面容器，这里用根组件持有一个页面状态、整窗切换内容：
/// 比另开一个窗口稳（不涉及第二窗口的生命周期/焦点/关闭顺序），也比做成侧栏弹层
/// 更符合"独立设置页 + 能返回聊天"的要求——返回只是把状态切回来，聊天区状态不丢。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Chat,
    Settings,
}

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
    /// 本机头像（base64 PNG）；`None` 表示用首字头像。
    avatar: Option<String>,
    /// 本机头像已校验的 PNG 字节（渲染用，避免每帧重新解码 base64）。
    avatar_png: Option<Arc<[u8]>>,
    /// 头像设置失败提示（下次成功或取消时清空）。
    avatar_error: Option<String>,
    /// 主机名兜底：昵称留空时对外使用它。
    default_name: String,
    /// 当前页面（聊天主界面 / 设置页）。
    page: Page,
    /// 昵称输入框（设置页内常驻；单行 `Input`，不是多行 `Textarea`）。
    nickname_input: Entity<InputState>,
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
            self_avatar,
        } = loaded;
        let nickname = self_nickname.unwrap_or_default();
        let avatar_png = self_avatar
            .as_deref()
            .and_then(crate::avatar::sanitize)
            .map(Arc::<[u8]>::from);
        let default_name = crate::net::local_device_name();
        let mut chat_store = ChatStore::new();
        chat_store.hydrate(histories);

        let nickname_input = cx.new(|cx| InputState::new(window, cx).submit_on_enter(true));
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
            avatar: self_avatar,
            avatar_png,
            avatar_error: None,
            default_name,
            page: Page::Chat,
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

    /// 当前对外展示的本机名：设置过用昵称，留空回退主机名（与网络层同一条规则）。
    fn own_display_name(&self) -> String {
        let nickname = self.nickname.trim();
        if nickname.is_empty() {
            self.default_name.clone()
        } else {
            nickname.to_string()
        }
    }

    /// 本机头像的首字兜底：取生效名字的第一个字符。
    fn own_initial(&self) -> String {
        self.own_display_name()
            .chars()
            .next()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "?".to_string())
    }

    /// 打开设置页：昵称输入框预填已保存值并聚焦，旧的错误提示清空。
    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.nickname.clone();
        self.nickname_input.update(cx, |state, cx| {
            state.set_value(value, window, cx);
            state.focus(window, cx);
        });
        self.page = Page::Settings;
        self.avatar_error = None;
        cx.notify();
    }

    /// 返回聊天界面。未保存的昵称输入直接丢弃（保存是显式动作，语义与旧行内编辑一致）。
    fn back_to_chat(&mut self, cx: &mut Context<Self>) {
        self.page = Page::Chat;
        cx.notify();
    }

    /// 提交昵称：空白串按"使用默认名"处理；立即写库并换掉运行期的握手名。
    fn save_nickname(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.nickname_input.read(cx).value().to_string();
        let nickname = raw.trim().to_string();
        self.store.set_self_nickname(&nickname);
        self.service.set_own_name(&nickname);
        self.nickname = nickname;
        cx.notify();
    }

    /// 取消昵称编辑：输入框还原为已保存的昵称，不写库、不离开设置页。
    fn cancel_nickname(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let saved = self.nickname.clone();
        self.nickname_input.update(cx, |state, cx| {
            state.set_value(saved, window, cx);
        });
        cx.notify();
    }

    /// 选头像：系统文件框 → 后台缩图编码 → 落库并立即用于下一次握手。
    fn pick_avatar(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("选择头像图片（PNG/JPEG）".into()),
        });
        cx.spawn(async move |this: WeakEntity<App>, cx: &mut AsyncApp| {
            let paths = match rx.await {
                Ok(Ok(Some(paths))) => paths,
                _ => return,
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            // 解码 + 缩放放后台执行器：大图不会卡住 UI 线程。
            let encoded = cx
                .background_spawn(async move { crate::avatar::encode_path(&path) })
                .await;
            this.update(cx, |app, cx| app.apply_avatar(encoded, cx))
                .ok();
        })
        .detach();
    }

    /// 应用新头像：失败只提示、保留原头像；成功后落库并换掉运行期头像。
    fn apply_avatar(&mut self, result: Result<String, String>, cx: &mut Context<Self>) {
        match result {
            Ok(b64) => {
                let png = crate::avatar::sanitize(&b64).map(Arc::<[u8]>::from);
                self.store.set_self_avatar(Some(b64.clone()));
                self.service.set_own_avatar(Some(b64.clone()));
                self.avatar = Some(b64);
                self.avatar_png = png;
                self.avatar_error = None;
            }
            Err(err) => self.avatar_error = Some(err),
        }
        cx.notify();
    }

    /// 清除头像，回到首字头像。
    fn clear_avatar(&mut self, cx: &mut Context<Self>) {
        self.store.set_self_avatar(None);
        self.service.set_own_avatar(None);
        self.avatar = None;
        self.avatar_png = None;
        self.avatar_error = None;
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
                UiEvent::DeviceAvatar { node_id, png } => {
                    // 头像只做运行期缓存（每次握手刷新），不落库：devices 表没有该列，
                    // 重启后离线设备显示首字头像，重新握手即恢复。
                    self.device_list
                        .update(cx, |list, cx| list.set_avatar(&node_id, png, cx));
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

impl App {
    /// 聊天主界面：侧栏（本机身份 + 设备列表）+ 聊天面板。
    fn chat_page(&mut self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let sidebar = self.sidebar(cx);
        div()
            .id("chat-page")
            .test_support()
            .flex()
            .flex_row()
            .size_full()
            .child(sidebar)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.))
                    .child(self.chat_panel.clone()),
            )
            .into_any_element()
    }

    /// 侧栏：顶部本机身份（点头像/名字进设置页）+ 设备列表。
    fn sidebar(&mut self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let identity = self.identity_block(cx);
        div()
            .id("app-sidebar")
            .test_support()
            .w(px(size::SIDEBAR_W))
            .flex()
            .flex_col()
            .bg(rgb(color::SIDEBAR))
            .border_r_1()
            .border_color(rgb(color::DIVIDER))
            .child(identity)
            .child(
                // 列方向 flex：子项按容器宽度拉伸，避免设备列表收缩到内容宽度。
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.))
                    .child(self.device_list.clone()),
            )
            .into_any_element()
    }

    /// 侧栏顶部的本机身份：头像 + 生效名字，整块可点，进设置页。
    fn identity_block(&mut self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let name = self.own_display_name();
        let side = px(size::AVATAR);
        // 头像槽：设置过就渲染图片，否则回落到名字首字（与设备行同一套兜底）。
        let slot = div()
            .id(if self.avatar_png.is_some() {
                "self-avatar-img"
            } else {
                "self-avatar-initial"
            })
            .test_support()
            .flex()
            .items_center()
            .justify_center()
            .w(side)
            .h(side)
            .flex_shrink_0()
            .rounded(px(radius::SM))
            .overflow_hidden()
            .bg(rgb(color::AVATAR))
            .text_size(px(text::MESSAGE))
            .text_color(rgb(color::TEXT_SECONDARY));
        let slot = match self.avatar_png.clone() {
            Some(png) => slot.child(
                img(ImageSource::Image(Arc::new(Image::from_bytes(
                    ImageFormat::Png,
                    png.to_vec(),
                ))))
                .w(side)
                .h(side),
            ),
            None => slot.child(self.own_initial()),
        };
        div()
            .id("identity-open-settings")
            .test_support()
            .aria_label(format!("本机身份 {name}，点击打开设置"))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(space::S2))
            .w_full()
            .px(px(space::S3))
            .py(px(space::S3))
            .border_b_1()
            .border_color(rgb(color::DIVIDER))
            .bg(rgb(color::SIDEBAR))
            .cursor_pointer()
            .hover(|el| el.bg(rgb(color::HOVER)))
            .on_click(cx.listener(|this, _ev, window, cx| this.open_settings(window, cx)))
            .child(slot)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(2.))
                    .child(
                        div()
                            .id("self-nickname")
                            .test_support()
                            .aria_label(name.clone())
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(text::TITLE))
                            .text_color(rgb(color::TEXT))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(name),
                    )
                    .child(
                        div()
                            .text_size(px(text::CAPTION))
                            .text_color(rgb(color::TEXT_MUTED))
                            .child("本机 · 点击设置"),
                    ),
            )
            .child(
                div()
                    .id("identity-settings")
                    .test_support()
                    .aria_label("设置")
                    .flex_shrink_0()
                    .px(px(space::S1))
                    .rounded(px(radius::SM))
                    .text_size(px(text::CAPTION))
                    .text_color(rgb(color::TEXT_MUTED))
                    .child("设置"),
            )
            .into_any_element()
    }

    /// 设置页：本机头像 + 昵称统一在这里编辑，可随时返回聊天。
    fn settings_page(&mut self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        // 设置页里的动作按钮（保存/取消/选择图片）：统一圆角与内边距，强调项用主题主色。
        let pill = |id: &str, label: &'static str, accent: bool| {
            div()
                .id(id.to_string())
                .test_support()
                .aria_label(label)
                .flex_shrink_0()
                .px(px(space::S2))
                .rounded(px(radius::SM))
                .text_size(px(text::CAPTION))
                .text_color(rgb(if accent {
                    color::ACCENT
                } else {
                    color::TEXT_MUTED
                }))
                .cursor_pointer()
                .child(label)
        };
        let name = self.own_display_name();
        let side = px(AVATAR_PREVIEW);
        let preview = div()
            .id(if self.avatar_png.is_some() {
                "avatar-preview-img"
            } else {
                "avatar-preview-initial"
            })
            .test_support()
            .flex()
            .items_center()
            .justify_center()
            .w(side)
            .h(side)
            .flex_shrink_0()
            .rounded(px(radius::MD))
            .overflow_hidden()
            .bg(rgb(color::AVATAR))
            .text_size(px(text::TITLE))
            .text_color(rgb(color::TEXT_SECONDARY));
        let preview = match self.avatar_png.clone() {
            Some(png) => preview.child(
                img(ImageSource::Image(Arc::new(Image::from_bytes(
                    ImageFormat::Png,
                    png.to_vec(),
                ))))
                .w(side)
                .h(side),
            ),
            None => preview.child(self.own_initial()),
        };

        let avatar_clear = self.avatar.is_some().then(|| {
            pill("avatar-clear", "移除头像", false)
                .on_click(cx.listener(|this, _ev, _window, cx| this.clear_avatar(cx)))
        });
        let avatar_error = self.avatar_error.clone();

        let avatar_section = div()
            .flex()
            .flex_col()
            .gap(px(space::S2))
            .child(
                div()
                    .text_size(px(text::TITLE))
                    .text_color(rgb(color::TEXT))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("头像"),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(space::S3))
                    .child(preview)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.))
                            .gap(px(space::S2))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(space::S2))
                                    .child(
                                        pill(
                                            "avatar-set",
                                            if self.avatar.is_some() {
                                                "更换图片"
                                            } else {
                                                "选择图片"
                                            },
                                            true,
                                        )
                                        .on_click(
                                            cx.listener(|this, _ev, _window, cx| {
                                                this.pick_avatar(cx)
                                            }),
                                        ),
                                    )
                                    .when_some(avatar_clear, |row, el| row.child(el)),
                            )
                            .child(
                                div()
                                    .text_size(px(text::CAPTION))
                                    .text_color(rgb(color::TEXT_MUTED))
                                    .child(
                                        "支持 PNG / JPEG，自动缩到 96px；未设置时显示名字首字。",
                                    ),
                            ),
                    ),
            )
            .when_some(avatar_error, |col, err| {
                col.child(
                    div()
                        .id("avatar-error")
                        .test_support()
                        .text_size(px(text::CAPTION))
                        .text_color(rgb(color::DANGER))
                        .child(err),
                )
            });

        let nickname_section = div()
            .flex()
            .flex_col()
            .gap(px(space::S2))
            .child(
                div()
                    .text_size(px(text::TITLE))
                    .text_color(rgb(color::TEXT))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("昵称"),
            )
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
                            .rounded(px(radius::SM))
                            .bg(rgb(color::SURFACE))
                            .border_1()
                            // 静态分隔色：聚焦不加任何描边强调，只靠光标。
                            .border_color(rgb(color::DIVIDER))
                            .overflow_hidden()
                            // 单行 Input 自带左右 padding 且根行是 items_center，
                            // 文字在 30px 内垂直居中；外层不再加 padding 去补偿。
                            .child(
                                Input::new(&self.nickname_input)
                                    .appearance(false)
                                    .h(px(30.))
                                    .text_size(px(text::CAPTION)),
                            ),
                    )
                    .child(pill("nickname-save", "保存", true).on_click(
                        cx.listener(|this, _ev, window, cx| this.save_nickname(window, cx)),
                    ))
                    .child(pill("nickname-cancel", "取消", false).on_click(
                        cx.listener(|this, _ev, window, cx| this.cancel_nickname(window, cx)),
                    )),
            )
            .child(
                div()
                    .text_size(px(text::CAPTION))
                    .text_color(rgb(color::TEXT_MUTED))
                    .child(format!("留空则使用默认名：{}", self.default_name)),
            )
            .child(
                div()
                    .id("self-effective-name")
                    .test_support()
                    .aria_label(format!("当前生效：{name}"))
                    .text_size(px(text::CAPTION))
                    .text_color(rgb(color::TEXT_SECONDARY))
                    .child(format!("当前生效：{name}")),
            );

        div()
            .id("settings-page")
            .test_support()
            .flex()
            .flex_col()
            .items_center()
            .size_full()
            .gap(px(space::S6))
            .px(px(space::S6))
            .py(px(space::S6))
            .bg(rgb(color::BG))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.key == "escape" {
                    this.cancel_nickname(window, cx);
                }
            }))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(space::S3))
                    .w_full()
                    .max_w(px(560.))
                    .child(
                        pill("settings-back", "← 返回聊天", false)
                            .on_click(cx.listener(|this, _ev, _window, cx| this.back_to_chat(cx))),
                    )
                    .child(
                        div()
                            .text_size(px(text::TITLE))
                            .text_color(rgb(color::TEXT))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("设置"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S6))
                    .w_full()
                    .max_w(px(560.))
                    .p(px(space::S6))
                    .rounded(px(radius::LG))
                    .bg(rgb(color::SURFACE))
                    .border_1()
                    .border_color(rgb(color::DIVIDER))
                    .child(avatar_section)
                    .child(div().w_full().h(px(1.)).bg(rgb(color::DIVIDER)))
                    .child(nickname_section),
            )
            .into_any_element()
    }
}

impl Render for App {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body: gpui_kit::AnyElement = match self.page {
            Page::Chat => self.chat_page(cx),
            Page::Settings => self.settings_page(cx),
        };
        div()
            .id("app-root")
            .test_support()
            .size_full()
            .bg(rgb(color::BG))
            .child(body)
    }
}
