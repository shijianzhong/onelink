//! iroh 服务层与事件桥接：局域网设备发现 + 握手交换设备名 + 有界事件队列。

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use iroh::endpoint::{Connection, presets};
use iroh::{Endpoint, EndpointAddr, EndpointId};
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::proto::{Frame, MAX_FRAME_BYTES, decode_frame, encode_frame};
use crate::xfer::{
    BLOCK_SIZE, EOF_BLOCK, MAX_FRAMED_BYTES, decode_framed, default_recv_dir, encode_block,
    encode_framed, sanitize_file_name, unique_path,
};

/// 连接/协议 ALPN 常量：`Endpoint::builder().alpns(...)` 声明后，不匹配的
/// 连接会在握手层被自动拒绝（iroh 官方模式）。
pub const ALPN: &[u8] = b"onelink-v1";

/// 文件数据传输专用 ALPN：控制帧（offer/accept/reject）仍走 [`ALPN`]，
/// 文件字节走这条独立连接（`onelink-v1-file`）。
pub const FILE_ALPN: &[u8] = b"onelink-v1-file";

/// 进度事件最小间隔：20MB 按 256KB 分块是 80 块，逐块发会把容量 32 的事件队列冲爆。
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

/// mDNS 服务名：`MdnsAddressLookupBuilder::service_name()` 覆盖默认的
/// `irohv1`，只有同名服务的端点互相发现（swarm-discovery 按名隔离）。
pub const MDNS_SERVICE: &str = "onelink-v1";

/// 事件队列容量：满时丢弃（`push_event` 不阻塞网络线程）。
pub const EVENT_QUEUE_CAPACITY: usize = 32;

/// 握手协议单条消息的大小上限（防御畸形/超大 payload）。
pub const MAX_HANDSHAKE_BYTES: usize = 4096;

/// 握手消息别名：M1 的 `HelloMsg` 现由 [`Frame`] 的 `Hello`/`HelloAck` 变体承载，
/// 线格式逐字节不变（`{"Hello":{"name":"x"}}`）。
pub type HelloMsg = Frame;

/// 网络层 → UI 侧事件。
#[derive(Debug, Clone)]
pub enum UiEvent {
    DeviceDiscovered {
        node_id: String,
        addr: Option<String>,
    },
    /// 握手完成，拿到对方设备名（注册表更新 name）。
    DeviceNamed {
        node_id: String,
        name: String,
    },
    DeviceLost {
        node_id: String,
    },
    /// 收到对端发来的一条聊天消息。
    ChatReceived {
        node_id: String,
        id: u64,
        text: String,
        sent_at_ms: u64,
    },
    /// 对端已回 ChatAck。
    ChatDelivered {
        node_id: String,
        id: u64,
    },
    /// 发送失败（连接失败 / 超时 / ACK 不匹配）。
    ChatFailed {
        node_id: String,
        id: u64,
        reason: String,
    },
    /// 收到对端的文件接收请求，等用户接受/拒绝。
    TransferOffer {
        node_id: String,
        xfer_id: u64,
        name: String,
        size: u64,
    },
    /// 文件发送进度（已发出的字节数与文件总大小）。
    TransferProgress {
        node_id: String,
        xfer_id: u64,
        transferred: u64,
        total: u64,
    },
    /// 文件发送完成：对端已回 `FileAck`。接收方落盘路径由接收事件携带。
    TransferCompleted {
        node_id: String,
        xfer_id: u64,
        path: Option<PathBuf>,
    },
    /// 传输失败（连接中断 / 校验不一致 / IO 错误）。
    TransferFailed {
        node_id: String,
        xfer_id: u64,
        reason: String,
    },
    /// 对端拒绝了接收请求。
    TransferRejected {
        node_id: String,
        xfer_id: u64,
        reason: String,
    },
    NetError(String),
}

/// iroh 服务信息。
pub struct IrohService {
    pub node_id: String,
}

/// UI → 服务线程的上行命令。同步 `send` 不阻塞 UI 线程。
pub enum Command {
    SendChat {
        peer: EndpointAddr,
        frame: Frame,
    },
    /// 发出文件接收请求（offer）；`xfer_id` 由 [`IrohServiceHandle::send_file`] 分配。
    SendFile {
        peer: EndpointAddr,
        xfer_id: u64,
        path: PathBuf,
    },
    /// 同意接收，回 `FileAccept` 给发送端。
    AcceptFile {
        xfer_id: u64,
    },
    /// 拒绝接收，回 `FileReject` 给发送端。
    RejectFile {
        xfer_id: u64,
        reason: String,
    },
}

/// 一次待确认的发送：offer 已发出，等对端 `FileAccept` 后由 `send_file_data` 流式发送。
pub struct Outgoing {
    pub peer: EndpointAddr,
    pub path: PathBuf,
    pub size: u64,
    pub name: String,
}

/// 发送待确认表：`xfer_id` → 待发送条目。
pub type OutgoingMap = Arc<Mutex<HashMap<u64, Outgoing>>>;

/// 收到但尚未接受的文件请求。
pub struct Incoming {
    /// 发送端地址（accept/reject 要连回去）。
    pub peer: EndpointAddr,
    pub node_id: String,
    pub name: String,
    pub size: u64,
}

/// 接收待确认表：`xfer_id` → 待确认条目。
pub type PendingMap = Arc<Mutex<HashMap<u64, Incoming>>>;

/// 服务线程内共享的传输状态。
pub struct XferState {
    pub outgoing: OutgoingMap,
    pub pending: PendingMap,
}

impl XferState {
    pub fn new() -> Self {
        Self {
            outgoing: Arc::new(Mutex::new(HashMap::new())),
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Default for XferState {
    fn default() -> Self {
        Self::new()
    }
}

/// 服务句柄：`tx` 是 UI 侧可轮询的有界事件队列（`try_recv`）。
pub struct IrohServiceHandle {
    pub tx: Arc<EventQueue>,
    /// 本机 node id（与 `addr.id` 相同，方便直接当字符串用）。
    pub node_id: String,
    /// 本机端点地址（含直连地址，供测试或手动直连）。
    pub addr: EndpointAddr,
    /// 上行命令通道（[`IrohServiceHandle::send_chat`] 往里投递）。
    pub commands: tokio::sync::mpsc::UnboundedSender<Command>,
    /// 本地消息 id 计数器（与对端 id 空间独立）。
    seq: Arc<std::sync::atomic::AtomicU64>,
    /// 置位后服务线程在下一个 100ms 检查点退出并 close endpoint。
    stop: Arc<AtomicBool>,
    /// 服务线程是否仍在运行。
    alive: Arc<AtomicBool>,
    /// 服务线程 join 句柄（take 后不可重复 join）。
    join: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// 服务线程退出时发一个空信号，供关闭路径按宽限期等待。
    finished: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    /// 运行期可换的对外昵称（握手应答/主动握手都读它）。
    own_name: OwnName,
}

impl IrohServiceHandle {
    /// 请求停止并最多等 `grace`：超时就放弃优雅关闭（线程自行收尾，不再 join）。
    ///
    /// 返回 true 表示服务线程已在宽限内退出。幂等：重复调用直接按存活状态返回。
    pub fn shutdown(&self, grace: Duration) -> bool {
        self.stop.store(true, Ordering::SeqCst);
        let finished = match self
            .finished
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            Some(rx) => rx.recv_timeout(grace).is_ok(),
            None => !self.alive.load(Ordering::SeqCst),
        };
        let join = self.join.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(join) = join
            && finished
        {
            let _ = join.join();
        }
        finished
    }

    pub fn is_running(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// 运行期更换对外昵称（空串回退主机名）。下一帧握手立即生效，无需重启服务。
    pub fn set_own_name(&self, nickname: &str) {
        let next = normalize_own_name(nickname);
        *self.own_name.lock().unwrap_or_else(|e| e.into_inner()) = next;
    }

    /// 当前对外昵称（UI 回显/测试用）。
    pub fn own_name(&self) -> String {
        current_own_name(&self.own_name)
    }

    /// 投递一条聊天消息到 `peer_id`；返回本条消息 id，调用方立刻用它渲染本地气泡。
    ///
    /// 同步返回：真正发送在服务线程里异步做，结果经 `ChatDelivered`/`ChatFailed` 回传。
    pub fn send_chat(&self, peer_id: &str, text: &str) -> u64 {
        let msg_id = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        self.send_chat_with_id(peer_id, text, msg_id)
    }

    /// 用调用方指定的消息 id 发送（上层据此从持久化历史续号，重启后不与旧消息冲突）。
    pub fn send_chat_with_id(&self, peer_id: &str, text: &str, msg_id: u64) -> u64 {
        match peer_id.parse::<EndpointId>() {
            Ok(id) => {
                let frame = Frame::Chat {
                    id: msg_id,
                    text: text.to_string(),
                    sent_at_ms: crate::proto::now_ms(),
                };
                self.dispatch_chat(EndpointAddr::from(id), frame)
            }
            Err(e) => {
                push_event(
                    &self.tx,
                    UiEvent::ChatFailed {
                        node_id: peer_id.to_string(),
                        id: msg_id,
                        reason: format!("bad node id: {e}"),
                    },
                );
                msg_id
            }
        }
    }

    /// 同 [`Self::send_chat`]，但目标用 `EndpointAddr`（可带直连地址，供测试/手动直连）。
    pub fn send_chat_to(&self, peer: EndpointAddr, text: &str) -> u64 {
        let (_, frame) = self.prepare_chat(text);
        self.dispatch_chat(peer, frame)
    }

    /// 把已构造好的 Chat 帧交给服务线程；超长/通道关闭都在这里统一处理。
    fn dispatch_chat(&self, peer: EndpointAddr, frame: Frame) -> u64 {
        let (msg_id, text_len) = match &frame {
            Frame::Chat { id, text, .. } => (*id, text.len()),
            _ => (0, 0),
        };
        let node_id = peer.id.to_string();
        if text_len > MAX_FRAME_BYTES {
            // 超长文本本地拒绝，绝不发半截（Review Focus 2）
            push_event(
                &self.tx,
                UiEvent::ChatFailed {
                    node_id,
                    id: msg_id,
                    reason: "message too large".to_string(),
                },
            );
            return msg_id;
        }
        if self
            .commands
            .send(Command::SendChat { peer, frame })
            .is_err()
        {
            tracing::warn!("send_chat: service command channel closed");
        }
        msg_id
    }

    /// 向 `peer_id` 发文件接收请求；返回本次传输的 `xfer_id`（解析失败返回 0）。
    ///
    /// 真正发送在服务线程里做：先写 `FileOffer`，等对端 `FileAccept` 后才开始传数据。
    pub fn send_file(&self, peer_id: &str, path: PathBuf) -> u64 {
        match peer_id.parse::<EndpointId>() {
            Ok(id) => self.send_file_to(EndpointAddr::from(id), path),
            Err(e) => {
                push_event(
                    &self.tx,
                    UiEvent::TransferFailed {
                        node_id: peer_id.to_string(),
                        xfer_id: 0,
                        reason: format!("bad node id: {e}"),
                    },
                );
                0
            }
        }
    }

    /// 同 [`Self::send_file`]，但目标用 `EndpointAddr`（可带直连地址，供测试/手动直连）。
    pub fn send_file_to(&self, peer: EndpointAddr, path: PathBuf) -> u64 {
        let xfer_id = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let node_id = peer.id.to_string();
        if let Err(e) = self.commands.send(Command::SendFile {
            peer,
            xfer_id,
            path,
        }) {
            push_event(
                &self.tx,
                UiEvent::TransferFailed {
                    node_id,
                    xfer_id,
                    reason: format!("service command channel closed: {e}"),
                },
            );
            return 0;
        }
        xfer_id
    }

    /// 同意接收 `xfer_id` 对应的文件。
    pub fn accept_file(&self, xfer_id: u64) {
        if self.commands.send(Command::AcceptFile { xfer_id }).is_err() {
            tracing::warn!(xfer_id, "accept_file: service command channel closed");
        }
    }

    /// 拒绝接收 `xfer_id`，并把 `reason` 回给发送端。
    pub fn reject_file(&self, xfer_id: u64, reason: &str) {
        if self
            .commands
            .send(Command::RejectFile {
                xfer_id,
                reason: reason.to_string(),
            })
            .is_err()
        {
            tracing::warn!(xfer_id, "reject_file: service command channel closed");
        }
    }

    fn prepare_chat(&self, text: &str) -> (u64, Frame) {
        let id = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        (
            id,
            Frame::Chat {
                id,
                text: text.to_string(),
                sent_at_ms: crate::proto::now_ms(),
            },
        )
    }
}

/// 把一条 `Chat` 帧发到对端并等 `ChatAck`；ACK 的 id 不匹配视为失败。
pub async fn send_chat_frame(
    endpoint: &Endpoint,
    peer: EndpointAddr,
    frame: &Frame,
) -> anyhow::Result<()> {
    let expected = match frame {
        Frame::Chat { id, .. } => *id,
        _ => anyhow::bail!("send_chat_frame expects a Chat frame"),
    };
    let conn = endpoint
        .connect(peer, ALPN)
        .await
        .map_err(|e| anyhow::anyhow!("connect: {e}"))?;
    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow::anyhow!("open_bi: {e}"))?;
    send.write_all(&encode_frame(frame)?).await?;
    send.shutdown().await?;
    let buf = recv
        .read_to_end(MAX_FRAME_BYTES)
        .await
        .map_err(|e| anyhow::anyhow!("read chat ack: {e}"))?;
    match decode_frame(&buf, MAX_FRAME_BYTES)? {
        Frame::ChatAck { id } if id == expected => Ok(()),
        other => anyhow::bail!("expected ChatAck({expected}), got {other:?}"),
    }
}

/// 流式发送 `path` 指向的文件，等对端回 `FileAck` 后才推 `TransferCompleted`。
///
/// 数据流格式见 [`crate::xfer`]：`FileData{xfer_id}` 头 → 若干 `[len][bytes]` 块 →
/// `[0u32]` 结束 → `FileDone{blake3}`。哈希边读边算，不二次读盘；全程只用一块
/// `BLOCK_SIZE` 缓冲；进度事件按 [`PROGRESS_INTERVAL`] 节流。
pub async fn send_file_data(
    endpoint: &Endpoint,
    peer: EndpointAddr,
    xfer_id: u64,
    path: PathBuf,
    queue: &Arc<EventQueue>,
) -> anyhow::Result<()> {
    let node_id = peer.id.to_string();
    let size = tokio::fs::metadata(&path)
        .await
        .map_err(|e| anyhow::anyhow!("stat {}: {e}", path.display()))?
        .len();

    let conn = endpoint
        .connect(peer, FILE_ALPN)
        .await
        .map_err(|e| anyhow::anyhow!("connect(file): {e}"))?;
    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow::anyhow!("open_bi(file): {e}"))?;

    send.write_all(&encode_framed(&Frame::FileData { xfer_id })?)
        .await?;

    let mut file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| anyhow::anyhow!("open {}: {e}", path.display()))?;
    let mut buf = vec![0u8; BLOCK_SIZE];
    let mut hasher = blake3::Hasher::new();
    let mut transferred = 0u64;
    let mut last_progress: Option<Instant> = None;
    loop {
        let n = file
            .read(&mut buf)
            .await
            .map_err(|e| anyhow::anyhow!("read {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        send.write_all(&encode_block(&buf[..n])?).await?;
        hasher.update(&buf[..n]);
        transferred += n as u64;

        // 首块必发一条，之后每 PROGRESS_INTERVAL 最多一条（不逐块刷事件）。
        let now = Instant::now();
        if last_progress.is_none_or(|t| now.duration_since(t) >= PROGRESS_INTERVAL) {
            last_progress = Some(now);
            push_event(
                queue,
                UiEvent::TransferProgress {
                    node_id: node_id.clone(),
                    xfer_id,
                    transferred,
                    total: size,
                },
            );
        }
    }

    send.write_all(&EOF_BLOCK).await?;
    let hash = hasher.finalize().to_hex().to_string();
    send.write_all(&encode_framed(&Frame::FileDone { xfer_id, hash })?)
        .await?;

    let ack = recv
        .read_to_end(MAX_FRAMED_BYTES)
        .await
        .map_err(|e| anyhow::anyhow!("read file ack: {e}"))?;
    send.shutdown().await?;
    match decode_framed(&ack, MAX_FRAMED_BYTES)? {
        Frame::FileAck { xfer_id: id } if id == xfer_id => {
            push_event(
                queue,
                UiEvent::TransferCompleted {
                    node_id,
                    xfer_id,
                    path: Some(path),
                },
            );
            Ok(())
        }
        other => anyhow::bail!("expected FileAck({xfer_id}), got {other:?}"),
    }
}

/// 发一条文件控制帧（offer/accept/reject）到对端，并等对端读完关闭连接。
///
/// 写完立刻拆链可能丢最后一帧（M1 已复现同类问题），所以等 `closed()`；正常对端
/// 读完后即关闭，最多等 2s，避免 UI 线程外的发送任务被卡死。
async fn send_control(endpoint: &Endpoint, peer: EndpointAddr, frame: Frame) -> anyhow::Result<()> {
    let conn = endpoint
        .connect(peer, ALPN)
        .await
        .map_err(|e| anyhow::anyhow!("connect(control): {e}"))?;
    let (mut send, _recv) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow::anyhow!("open_bi(control): {e}"))?;
    send.write_all(&encode_frame(&frame)?).await?;
    send.shutdown().await?;
    let closed = tokio::time::timeout(Duration::from_secs(2), conn.closed()).await;
    tracing::debug!(?closed, "control frame delivered");
    Ok(())
}

/// 服务线程内执行一条上行命令，把结果回写成 UI 事件。
async fn run_command(
    endpoint: Endpoint,
    cmd: Command,
    queue: &Arc<EventQueue>,
    xfers: &Arc<XferState>,
) {
    match cmd {
        Command::SendChat { peer, frame } => {
            let node_id = peer.id.to_string();
            let id = match &frame {
                Frame::Chat { id, .. } => *id,
                _ => 0,
            };
            let result = tokio::time::timeout(
                Duration::from_secs(5),
                send_chat_frame(&endpoint, peer, &frame),
            )
            .await;
            match result {
                Ok(Ok(())) => push_event(queue, UiEvent::ChatDelivered { node_id, id }),
                Ok(Err(e)) => push_event(
                    queue,
                    UiEvent::ChatFailed {
                        node_id,
                        id,
                        reason: format!("{e:#}"),
                    },
                ),
                Err(_) => push_event(
                    queue,
                    UiEvent::ChatFailed {
                        node_id,
                        id,
                        reason: "send timed out after 5s".to_string(),
                    },
                ),
            }
        }
        Command::SendFile {
            peer,
            xfer_id,
            path,
        } => {
            let node_id = peer.id.to_string();
            let size = match tokio::fs::metadata(&path).await {
                Ok(m) => m.len(),
                Err(e) => {
                    push_event(
                        queue,
                        UiEvent::TransferFailed {
                            node_id,
                            xfer_id,
                            reason: format!("stat {}: {e}", path.display()),
                        },
                    );
                    return;
                }
            };
            let name = sanitize_file_name(
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unnamed"),
            );
            // 先登记再发 offer：accept 可能在任何时刻回包。
            xfers
                .outgoing
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(
                    xfer_id,
                    Outgoing {
                        peer: peer.clone(),
                        path: path.clone(),
                        size,
                        name: name.clone(),
                    },
                );
            let offer = Frame::FileOffer {
                xfer_id,
                name,
                size,
            };
            if let Err(e) = send_control(&endpoint, peer, offer).await {
                xfers
                    .outgoing
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&xfer_id);
                push_event(
                    queue,
                    UiEvent::TransferFailed {
                        node_id,
                        xfer_id,
                        reason: format!("send offer: {e:#}"),
                    },
                );
            }
        }
        Command::AcceptFile { xfer_id } => {
            let entry = xfers
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&xfer_id)
                .map(|i| (i.peer.clone(), i.node_id.clone()));
            match entry {
                Some((peer, node_id)) => {
                    if let Err(e) =
                        send_control(&endpoint, peer, Frame::FileAccept { xfer_id }).await
                    {
                        push_event(
                            queue,
                            UiEvent::TransferFailed {
                                node_id,
                                xfer_id,
                                reason: format!("send accept: {e:#}"),
                            },
                        );
                    }
                }
                None => tracing::warn!(xfer_id, "accept_file: no pending transfer"),
            }
        }
        Command::RejectFile { xfer_id, reason } => {
            let entry = xfers
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&xfer_id);
            match entry {
                Some(incoming) => {
                    let reject = Frame::FileReject { xfer_id, reason };
                    if let Err(e) = send_control(&endpoint, incoming.peer, reject).await {
                        tracing::warn!(xfer_id, "send reject failed: {e:#}");
                    }
                }
                None => tracing::warn!(xfer_id, "reject_file: no pending transfer"),
            }
        }
    }
}

/// 退出时等待服务线程收尾的宽限期：内部 endpoint close 800ms + runtime 500ms 已各自限时，
/// 这里再兜一层，保证关闭路径不会无限挂住。
pub const SHUTDOWN_GRACE: Duration = Duration::from_millis(1500);

/// 停止服务线程并等待其收尾（close endpoint + runtime 退出）；超过宽限期就放弃优雅关闭。
/// 幂等：第二次调用直接返回。
pub fn shutdown_iroh_service(handle: &IrohServiceHandle) {
    let t = Instant::now();
    if handle.shutdown(SHUTDOWN_GRACE) {
        tracing::info!(
            elapsed_ms = t.elapsed().as_millis(),
            "iroh service shut down"
        );
    } else {
        tracing::warn!(
            elapsed_ms = t.elapsed().as_millis(),
            "iroh service did not stop within grace; abandoning graceful close"
        );
    }
}

/// 有界事件队列：满时丢弃最旧事件（始终保留最新状态）。
/// 设备发现场景下 UI 消费慢时，旧通知可丢，注册表快照是状态权威。
pub struct EventQueue {
    inner: Mutex<VecDeque<UiEvent>>,
    capacity: usize,
}

impl EventQueue {
    pub fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(VecDeque::with_capacity(capacity + 1)),
            capacity,
        })
    }

    /// 入队；满则先丢弃最旧事件（不阻塞调用方，网络线程可安全调用）。
    pub fn push(&self, ev: UiEvent) {
        let mut q = self.inner.lock().expect("event queue poisoned");
        if q.len() >= self.capacity {
            q.pop_front();
        }
        q.push_back(ev);
    }

    /// 非阻塞取最早事件；空队列返回 `None`。
    pub fn try_recv(&self) -> Option<UiEvent> {
        self.inner.lock().expect("event queue poisoned").pop_front()
    }
}

/// mDNS 事件统一视图。真实 `DiscoveryEvent` 与测试 `TestEvent` 都实现它，
/// `translate_mdns_event` 只依赖该 trait。
pub trait MdnsEventLike {
    /// 发现事件 → (node_id, 可显示地址)。
    fn as_discovered(&self) -> Option<(String, Option<String>)>;
    /// 过期事件 → node_id。
    fn as_expired(&self) -> Option<String>;
}

impl MdnsEventLike for DiscoveryEvent {
    fn as_discovered(&self) -> Option<(String, Option<String>)> {
        match self {
            DiscoveryEvent::Discovered { endpoint_info, .. } => {
                let addr = endpoint_info.addrs().next().map(|a| a.to_string());
                Some((endpoint_info.endpoint_id.to_string(), addr))
            }
            DiscoveryEvent::Expired { .. } => None,
            _ => None,
        }
    }

    fn as_expired(&self) -> Option<String> {
        match self {
            DiscoveryEvent::Expired { endpoint_id } => Some(endpoint_id.to_string()),
            _ => None,
        }
    }
}

/// 将一条 mDNS 事件翻译为 0..N 条 UI 事件。
pub fn translate_mdns_event(ev: &impl MdnsEventLike) -> Vec<UiEvent> {
    if let Some((node_id, addr)) = ev.as_discovered() {
        vec![UiEvent::DeviceDiscovered { node_id, addr }]
    } else if let Some(node_id) = ev.as_expired() {
        vec![UiEvent::DeviceLost { node_id }]
    } else {
        vec![]
    }
}

/// 有界事件入队：满丢旧、不阻塞调用方（网络线程）。
pub fn push_event(q: &EventQueue, ev: UiEvent) {
    q.push(ev);
}

/// 本机显示名（默认取系统主机名）。
pub fn local_device_name() -> String {
    gethostname::gethostname().to_string_lossy().to_string()
}

/// 服务运行期的对外昵称：握手时读取，支持不重启服务直接改名。
pub type OwnName = Arc<std::sync::Mutex<String>>;

/// 空昵称回退主机名，两侧空白一并去掉。
fn normalize_own_name(nickname: &str) -> String {
    if nickname.trim().is_empty() {
        local_device_name()
    } else {
        nickname.trim().to_string()
    }
}

/// 读出当前昵称（持锁期间只做一次 clone）。
pub fn current_own_name(name: &OwnName) -> String {
    name.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 主动握手：连接对方，打开双向流发 `Hello{our_name}`，读回 `HelloAck{name}`。
pub async fn handshake_name(
    endpoint: &Endpoint,
    peer: EndpointAddr,
    our_name: &str,
) -> anyhow::Result<String> {
    let conn = endpoint
        .connect(peer, ALPN)
        .await
        .map_err(|e| anyhow::anyhow!("connect: {e}"))?;
    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow::anyhow!("open_bi: {e}"))?;
    let hello = serde_json::to_vec(&HelloMsg::Hello {
        name: our_name.to_string(),
    })?;
    send.write_all(&hello).await?;
    send.shutdown().await?;

    let buf = recv
        .read_to_end(MAX_HANDSHAKE_BYTES)
        .await
        .map_err(|e| anyhow::anyhow!("read hello ack: {e}"))?;
    match serde_json::from_slice::<HelloMsg>(&buf)? {
        HelloMsg::HelloAck { name } => Ok(name),
        other => anyhow::bail!("expected HelloAck, got {other:?}"),
    }
}

/// 被动握手：循环接受连接，读 `Hello`、回 `HelloAck{our_name}`。
pub async fn serve_handshake(endpoint: &Endpoint, our_name: &str) -> anyhow::Result<()> {
    while let Some(incoming) = endpoint.accept().await {
        let our_name = our_name.to_string();
        tokio::spawn(async move {
            let conn = match incoming.await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("incoming connection failed: {e}");
                    return;
                }
            };
            if let Err(e) = answer_hello(conn, &our_name).await {
                tracing::warn!("hello exchange failed: {e:#}");
            }
        });
    }
    Ok(())
}

async fn answer_hello(conn: iroh::endpoint::Connection, our_name: &str) -> anyhow::Result<()> {
    let (mut send, mut recv) = conn
        .accept_bi()
        .await
        .map_err(|e| anyhow::anyhow!("accept_bi: {e}"))?;
    let buf = recv
        .read_to_end(MAX_HANDSHAKE_BYTES)
        .await
        .map_err(|e| anyhow::anyhow!("read hello: {e}"))?;
    let peer_name = match serde_json::from_slice::<HelloMsg>(&buf)? {
        HelloMsg::Hello { name } => name,
        other => anyhow::bail!("expected Hello, got {other:?}"),
    };
    tracing::info!(peer_name, "received hello");
    let ack = serde_json::to_vec(&HelloMsg::HelloAck {
        name: our_name.to_string(),
    })?;
    send.write_all(&ack).await?;
    send.shutdown().await?;
    // 保持连接直到对端读完 ACK 并关闭：若此处立即 drop，QUIC 会拆链，
    // 尚未送达的 ACK 数据会被丢弃（客户端读到 connection lost）。
    let reason = conn.closed().await;
    tracing::debug!("handshake connection closed: {reason:?}");
    Ok(())
}

/// 服务端：一条 accept 循环，按 ALPN 分派：`onelink-v1-file` 走文件接收，
/// 其余走 [`Frame`] 分派（握手/聊天/文件控制帧）。畸形字节只记日志并关流。
pub async fn serve_frames(
    endpoint: &Endpoint,
    our_name: &str,
    queue: &Arc<EventQueue>,
    xfers: &Arc<XferState>,
) -> anyhow::Result<()> {
    // 固定昵称入口：内部包成一个不会变化的共享名，供测试/工具直接调用。
    let fixed: OwnName = Arc::new(std::sync::Mutex::new(our_name.to_string()));
    serve_frames_with_name(endpoint, &fixed, queue, xfers).await
}

/// 服务入口：昵称从共享单元现读，`set_own_name` 后下一帧握手即用新名。
pub async fn serve_frames_with_name(
    endpoint: &Endpoint,
    our_name: &OwnName,
    queue: &Arc<EventQueue>,
    xfers: &Arc<XferState>,
) -> anyhow::Result<()> {
    while let Some(incoming) = endpoint.accept().await {
        let our_name = Arc::clone(our_name);
        let queue = Arc::clone(queue);
        let xfers = Arc::clone(xfers);
        let endpoint = endpoint.clone();
        tokio::spawn(async move {
            let conn = match incoming.await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("incoming connection failed: {e}");
                    return;
                }
            };
            if conn.alpn() == FILE_ALPN {
                if let Err(e) = handle_file_conn(conn, &queue, &xfers).await {
                    tracing::warn!("file receive failed: {e:#}");
                }
            } else if let Err(e) =
                dispatch_connection(conn, &our_name, &queue, &xfers, &endpoint).await
            {
                tracing::warn!("frame dispatch failed: {e:#}");
            }
        });
    }
    Ok(())
}

/// 读一条 `[u32 len][payload]` 帧；长度必须与正文一致且不超 `limit`。
async fn read_framed(recv: &mut iroh::endpoint::RecvStream, limit: usize) -> anyhow::Result<Frame> {
    let mut hdr = [0u8; 4];
    recv.read_exact(&mut hdr)
        .await
        .map_err(|e| anyhow::anyhow!("read frame header: {e}"))?;
    let len = crate::xfer::block_len(&hdr);
    if len > limit {
        anyhow::bail!("framed payload too large: {len} bytes (limit {limit})");
    }
    let mut body = vec![0u8; len];
    recv.read_exact(&mut body)
        .await
        .map_err(|e| anyhow::anyhow!("read frame body: {e}"))?;
    let mut buf = hdr.to_vec();
    buf.extend_from_slice(&body);
    decode_framed(&buf, limit)
}

/// 读一帧并回应：`Hello` 回 `HelloAck`，`Chat` 入队并回 `ChatAck`；
/// 文件控制帧只处理不发回执（accept/reject 由本端主动连回发送端）。
pub async fn dispatch_connection(
    conn: Connection,
    our_name: &OwnName,
    queue: &Arc<EventQueue>,
    xfers: &Arc<XferState>,
    endpoint: &Endpoint,
) -> anyhow::Result<()> {
    let (mut send, mut recv) = conn
        .accept_bi()
        .await
        .map_err(|e| anyhow::anyhow!("accept_bi: {e}"))?;
    let buf = recv
        .read_to_end(MAX_FRAME_BYTES)
        .await
        .map_err(|e| anyhow::anyhow!("read frame: {e}"))?;
    let frame = match decode_frame(&buf, MAX_FRAME_BYTES) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("bad frame from {}: {e:#}", conn.remote_id());
            return Ok(());
        }
    };
    match frame {
        Frame::Hello { name } => {
            tracing::info!(peer_name = %name, "received hello");
            let ack = encode_frame(&Frame::HelloAck {
                name: current_own_name(our_name),
            })?;
            send.write_all(&ack).await?;
            send.shutdown().await?;
            // 回完 ACK 必须等对端关闭，否则拆链会让 ACK 丢失（M1 实机复现）。
            let reason = conn.closed().await;
            tracing::debug!("handshake connection closed: {reason:?}");
        }
        Frame::Chat {
            id,
            text,
            sent_at_ms,
        } => {
            tracing::debug!(%id, bytes = text.len(), "received chat frame");
            push_event(
                queue,
                UiEvent::ChatReceived {
                    node_id: conn.remote_id().to_string(),
                    id,
                    text,
                    sent_at_ms,
                },
            );
            let ack = encode_frame(&Frame::ChatAck { id })?;
            send.write_all(&ack).await?;
            send.shutdown().await?;
            let reason = conn.closed().await;
            tracing::debug!("chat connection closed: {reason:?}");
        }
        Frame::FileOffer {
            xfer_id,
            name,
            size,
        } => {
            let node_id = conn.remote_id().to_string();
            // 接受态 Connection 不直接暴露对端地址，用路径快照里记录的对端地址连回去。
            let addrs: Vec<iroh::TransportAddr> = conn
                .paths()
                .iter()
                .map(|p| p.remote_addr().clone())
                .collect();
            let peer = EndpointAddr::from_parts(conn.remote_id(), addrs);
            tracing::info!(%xfer_id, %name, size, "received file offer");
            xfers
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(
                    xfer_id,
                    Incoming {
                        peer,
                        node_id: node_id.clone(),
                        name: name.clone(),
                        size,
                    },
                );
            push_event(
                queue,
                UiEvent::TransferOffer {
                    node_id,
                    xfer_id,
                    name,
                    size,
                },
            );
            // 本端是读取方：直接返回即可让发送端的等待结束。
        }
        Frame::FileAccept { xfer_id } => {
            let outgoing = xfers
                .outgoing
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&xfer_id);
            match outgoing {
                Some(o) => {
                    let ep = endpoint.clone();
                    let q = Arc::clone(queue);
                    tokio::spawn(async move {
                        let node_id = o.peer.id.to_string();
                        if let Err(e) = send_file_data(&ep, o.peer, xfer_id, o.path, &q).await {
                            push_event(
                                &q,
                                UiEvent::TransferFailed {
                                    node_id,
                                    xfer_id,
                                    reason: format!("{e:#}"),
                                },
                            );
                        }
                    });
                }
                None => tracing::warn!(xfer_id, "accept for unknown transfer"),
            }
        }
        Frame::FileReject { xfer_id, reason } => {
            // 对端已拒收：清掉待发送条目，避免 map 里永久留一条死数据。
            xfers
                .outgoing
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&xfer_id);
            tracing::info!(%xfer_id, %reason, "peer rejected transfer");
            push_event(
                queue,
                UiEvent::TransferRejected {
                    node_id: conn.remote_id().to_string(),
                    xfer_id,
                    reason,
                },
            );
        }
        other => tracing::warn!("unexpected frame on server side: {other:?}"),
    }
    Ok(())
}

/// 接收一条文件数据连接：校验落盘到 `~/Downloads/OneLink`，成功才回 `FileAck`。
///
/// 失败（哈希不一致 / IO 错误）删除 `.part` 且不回 ACK，让发送端以失败收场。
async fn handle_file_conn(
    conn: Connection,
    queue: &Arc<EventQueue>,
    xfers: &Arc<XferState>,
) -> anyhow::Result<()> {
    let (mut send, mut recv) = conn
        .accept_bi()
        .await
        .map_err(|e| anyhow::anyhow!("accept_bi(file): {e}"))?;
    let first = read_framed(&mut recv, MAX_FRAMED_BYTES).await?;
    let Frame::FileData { xfer_id } = first else {
        anyhow::bail!("expected FileData, got {first:?}");
    };
    // 取出即删：一个 offer 只允许一次数据传输，防重放。
    let incoming = xfers
        .pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&xfer_id)
        .ok_or_else(|| anyhow::anyhow!("no pending transfer {xfer_id}"))?;
    let node_id = incoming.node_id.clone();

    let dir = default_recv_dir();
    let target = unique_path(&dir, &sanitize_file_name(&incoming.name));
    let part = target.with_extension("part");

    let outcome = receive_into(
        &mut recv,
        &dir,
        &part,
        &node_id,
        xfer_id,
        incoming.size,
        queue,
    )
    .await;
    match outcome {
        Ok(()) => {
            tokio::fs::rename(&part, &target)
                .await
                .map_err(|e| anyhow::anyhow!("rename {}: {e}", target.display()))?;
            push_event(
                queue,
                UiEvent::TransferCompleted {
                    node_id,
                    xfer_id,
                    path: Some(target),
                },
            );
            send.write_all(&encode_framed(&Frame::FileAck { xfer_id })?)
                .await?;
            send.shutdown().await?;
            // 等发送端收尾再拆链，否则 ACK 会丢（同 M1 握手）。
            let _ = conn.closed().await;
            Ok(())
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&part).await;
            push_event(
                queue,
                UiEvent::TransferFailed {
                    node_id,
                    xfer_id,
                    reason: format!("{e:#}"),
                },
            );
            Err(e)
        }
    }
}

/// 流式接收块到 `.part`，比对 `FileDone` 的 blake3；失败由调用方清理临时文件。
async fn receive_into(
    recv: &mut iroh::endpoint::RecvStream,
    dir: &std::path::Path,
    part: &std::path::Path,
    node_id: &str,
    xfer_id: u64,
    total: u64,
    queue: &Arc<EventQueue>,
) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| anyhow::anyhow!("create {}: {e}", dir.display()))?;
    let mut file = tokio::fs::File::create(part)
        .await
        .map_err(|e| anyhow::anyhow!("create {}: {e}", part.display()))?;

    let mut hdr = [0u8; 4];
    let mut buf = vec![0u8; BLOCK_SIZE];
    let mut hasher = blake3::Hasher::new();
    let mut transferred = 0u64;
    let mut last_progress: Option<Instant> = None;
    loop {
        recv.read_exact(&mut hdr)
            .await
            .map_err(|e| anyhow::anyhow!("read block header: {e}"))?;
        let n = crate::xfer::block_len(&hdr);
        if n == 0 {
            break;
        }
        if n > BLOCK_SIZE {
            anyhow::bail!("block too large: {n} bytes (limit {BLOCK_SIZE})");
        }
        recv.read_exact(&mut buf[..n])
            .await
            .map_err(|e| anyhow::anyhow!("read block body: {e}"))?;
        file.write_all(&buf[..n]).await?;
        hasher.update(&buf[..n]);
        transferred += n as u64;

        // 与发送端一致：首块必发、之后每 PROGRESS_INTERVAL 最多一条。
        let now = Instant::now();
        if last_progress.is_none_or(|t| now.duration_since(t) >= PROGRESS_INTERVAL) {
            last_progress = Some(now);
            push_event(
                queue,
                UiEvent::TransferProgress {
                    node_id: node_id.to_string(),
                    xfer_id,
                    transferred,
                    total,
                },
            );
        }
    }
    file.flush().await?;
    drop(file);

    let done = read_framed(recv, MAX_FRAMED_BYTES).await?;
    let Frame::FileDone {
        xfer_id: done_id,
        hash,
    } = done
    else {
        anyhow::bail!("expected FileDone, got {done:?}");
    };
    if done_id != xfer_id {
        anyhow::bail!("FileDone for {done_id}, expected {xfer_id}");
    }
    let actual = hasher.finalize().to_hex().to_string();
    if actual != hash {
        anyhow::bail!("hash mismatch: expected {hash}, got {actual}");
    }
    Ok(())
}

/// 启动 iroh 服务线程：Endpoint(presets::N0，官方 relay 兜底) + mDNS 发现 +
/// 连接 accept 循环。事件经有界队列暴露在 `handle.tx` 供 UI 轮询。
pub fn spawn_iroh_service() -> anyhow::Result<IrohServiceHandle> {
    spawn_iroh_service_with_name(local_device_name())
}

/// 启动服务，握手时用调用方给的昵称（空串回退主机名）。
pub fn spawn_iroh_service_with_name(nickname: String) -> anyhow::Result<IrohServiceHandle> {
    // 昵称优先取本地设置；为空则用主机名兜底。运行期可经 `set_own_name` 替换。
    let own_name: OwnName = Arc::new(std::sync::Mutex::new(normalize_own_name(&nickname)));
    let thread_own_name = Arc::clone(&own_name);
    let queue = EventQueue::new(EVENT_QUEUE_CAPACITY);
    let service_queue = Arc::clone(&queue);
    let err_queue = Arc::clone(&queue);
    let stop = Arc::new(AtomicBool::new(false));
    let alive = Arc::new(AtomicBool::new(true));
    let seq = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let xfers = Arc::new(XferState::new());
    let thread_xfers = Arc::clone(&xfers);
    let (addr_tx, addr_rx) = std::sync::mpsc::sync_channel::<anyhow::Result<EndpointAddr>>(1);
    let (commands_tx, mut commands_rx) = tokio::sync::mpsc::unbounded_channel::<Command>();
    let thread_stop = Arc::clone(&stop);
    let thread_alive = Arc::clone(&alive);
    let (finished_tx, finished_rx) = std::sync::mpsc::channel::<()>();
    let join = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");
        let result = rt.block_on(async move {
            let endpoint = Endpoint::builder(presets::N0)
                .alpns(vec![ALPN.to_vec(), FILE_ALPN.to_vec()])
                .bind()
                .await
                .map_err(|e| anyhow::anyhow!("endpoint bind: {e}"))?;
            let self_id = endpoint.id();
            let node_id = self_id.to_string();
            // `thread_own_name` 是运行期昵称，握手时现读，改名无需重启服务。
            let self_addr = endpoint.addr();
            // 把地址交回构造函数：测试/手动直连不用再等 mDNS 发现。
            let _ = addr_tx.send(Ok(self_addr));
            tracing::info!(
                node_id,
                our_name = %current_own_name(&thread_own_name),
                %MDNS_SERVICE,
                "iroh endpoint bound"
            );

            let mdns = MdnsAddressLookup::builder()
                .service_name(MDNS_SERVICE)
                .build(endpoint.id())
                .map_err(|e| anyhow::anyhow!("mdns build: {e}"))?;
            endpoint
                .address_lookup()
                .map_err(|e| anyhow::anyhow!("address lookup: {e}"))?
                .add(mdns.clone());
            tracing::info!("mDNS service registered");

            // 服务端任务：一条 accept 循环，同时服务握手与聊天帧
            // （非 ALPN 连接已在 alpns 层拒绝）。
            let endpoint_serve = endpoint.clone();
            let serve_name = Arc::clone(&thread_own_name);
            let serve_queue = Arc::clone(&service_queue);
            let serve_xfers = Arc::clone(&thread_xfers);
            let serve_task = tokio::spawn(async move {
                if let Err(e) =
                    serve_frames_with_name(&endpoint_serve, &serve_name, &serve_queue, &serve_xfers)
                        .await
                {
                    tracing::warn!("serve_frames ended: {e:#}");
                }
            });

            // 发现任务：mDNS discovered/expired → UiEvent；发现即主动握手换设备名。
            let mdns_queue = Arc::clone(&service_queue);
            let endpoint_mdns = endpoint.clone();
            let mdns_name = Arc::clone(&thread_own_name);
            let mdns_task = tokio::spawn(async move {
                let mut stream = mdns.subscribe().await;
                let mut handshaked: HashSet<EndpointId> = HashSet::new();
                while let Some(ev) = stream.next().await {
                    for ui_ev in translate_mdns_event(&ev) {
                        push_event(&mdns_queue, ui_ev);
                    }
                    if let DiscoveryEvent::Discovered { endpoint_info, .. } = &ev {
                        let peer_id = endpoint_info.endpoint_id;
                        if peer_id == self_id || !handshaked.insert(peer_id) {
                            continue;
                        }
                        let ep = endpoint_mdns.clone();
                        let name = mdns_name.clone();
                        let q = Arc::clone(&mdns_queue);
                        tokio::spawn(async move {
                            let mut attempt = 0;
                            loop {
                                attempt += 1;
                                let our = current_own_name(&name);
                                match handshake_name(&ep, EndpointAddr::from(peer_id), &our).await {
                                    Ok(peer_name) => {
                                        push_event(
                                            &q,
                                            UiEvent::DeviceNamed {
                                                node_id: peer_id.to_string(),
                                                name: peer_name,
                                            },
                                        );
                                        break;
                                    }
                                    Err(e) if attempt < 3 => {
                                        tracing::debug!(
                                            "handshake attempt {attempt} failed: {e:#}"
                                        );
                                        tokio::time::sleep(Duration::from_secs(2)).await;
                                    }
                                    Err(e) => {
                                        tracing::warn!("handshake with {peer_id} failed: {e:#}");
                                        break;
                                    }
                                }
                            }
                        });
                    }
                }
                tracing::info!("mdns event stream ended");
            });

            // 服务线程随任一任务结束或 stop 信号而收尾：close endpoint 让
            // 对端尽快看到节点下线，再退出 runtime。命令分支不退出，只派发发送任务。
            let mut mdns_task = mdns_task;
            let mut serve_task = serve_task;
            loop {
                tokio::select! {
                    r = &mut mdns_task => {
                        tracing::info!("mdns task ended: {r:?}");
                        break;
                    }
                    r = &mut serve_task => {
                        tracing::info!("serve task ended: {r:?}");
                        break;
                    }
                    Some(cmd) = commands_rx.recv() => {
                        let ep = endpoint.clone();
                        let q = Arc::clone(&service_queue);
                        let x = Arc::clone(&thread_xfers);
                        tokio::spawn(async move { run_command(ep, cmd, &q, &x).await; });
                    }
                    _ = wait_for_stop(Arc::clone(&thread_stop)) => {
                        tracing::info!("stop requested");
                        break;
                    }
                }
            }
            let t = Instant::now();
            mdns_task.abort();
            serve_task.abort();
            // close 要等 relay/QUIC 收尾，正常网络下 <300ms，但弱网可能久；
            // 退出路径不为此卡住 UI，超时就交给 runtime 限时关闭。
            let closed = tokio::time::timeout(Duration::from_millis(800), endpoint.close()).await;
            tracing::info!(
                ?closed,
                elapsed_ms = t.elapsed().as_millis(),
                "endpoint close attempted, service thread exiting"
            );
            Ok::<(), anyhow::Error>(())
        });
        if let Err(e) = result {
            err_queue.push(UiEvent::NetError(format!("service error: {e:#}")));
        }
        rt.shutdown_timeout(Duration::from_millis(500));
        thread_alive.store(false, Ordering::SeqCst);
        // 通知关闭路径：线程已收尾（接收方可能已超时放弃，忽略发送失败）。
        let _ = finished_tx.send(());
    });

    // 等端点绑定后拿到本机地址：服务线程绑定失败时通道关闭，这里直接报错。
    let addr = addr_rx
        .recv()
        .map_err(|_| anyhow::anyhow!("iroh service thread ended before binding"))??;
    let node_id = addr.id.to_string();

    Ok(IrohServiceHandle {
        tx: queue,
        node_id,
        addr,
        commands: commands_tx,
        seq,
        stop,
        alive,
        join: Mutex::new(Some(join)),
        finished: Mutex::new(Some(finished_rx)),
        own_name,
    })
}

/// 每 100ms 检查一次停止标志（nimble 退出，不打断 in-flight 任务）。
async fn wait_for_stop(stop: Arc<AtomicBool>) {
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
