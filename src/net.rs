//! iroh 服务层与事件桥接：局域网设备发现 + 握手交换设备名 + 有界事件队列。

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, EndpointId};
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};
use tokio::io::AsyncWriteExt;

use crate::proto::{Frame, MAX_FRAME_BYTES, decode_frame, encode_frame};

/// 连接/协议 ALPN 常量：`Endpoint::builder().alpns(...)` 声明后，不匹配的
/// 连接会在握手层被自动拒绝（iroh 官方模式）。
pub const ALPN: &[u8] = b"onelink-v1";

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
    NetError(String),
}

/// iroh 服务信息。
pub struct IrohService {
    pub node_id: String,
}

/// UI → 服务线程的上行命令。同步 `send` 不阻塞 UI 线程。
pub enum Command {
    SendChat { peer: EndpointAddr, frame: Frame },
}

/// 服务句柄：`tx` 是 UI 侧可轮询的有界事件队列（`try_recv`）。
pub struct IrohServiceHandle {
    pub tx: Arc<EventQueue>,
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
}

impl IrohServiceHandle {
    pub fn is_running(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// 投递一条聊天消息到 `peer_id`；返回本条消息 id，调用方立刻用它渲染本地气泡。
    ///
    /// 同步返回：真正发送在服务线程里异步做，结果经 `ChatDelivered`/`ChatFailed` 回传。
    pub fn send_chat(&self, peer_id: &str, text: &str) -> u64 {
        match peer_id.parse::<EndpointId>() {
            Ok(id) => self.send_chat_to(EndpointAddr::from(id), text),
            Err(e) => {
                let (msg_id, _) = self.prepare_chat(text);
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
        let (msg_id, frame) = self.prepare_chat(text);
        let node_id = peer.id.to_string();
        if text.len() > MAX_FRAME_BYTES {
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

/// 服务线程内执行一条上行命令，把结果回写成 UI 事件。
async fn run_command(endpoint: Endpoint, cmd: Command, queue: &Arc<EventQueue>) {
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
    }
}

/// 停止服务线程并等待其收尾（close endpoint + runtime 退出）。
/// 幂等：第二次调用直接返回。
pub fn shutdown_iroh_service(handle: &IrohServiceHandle) {
    handle.stop.store(true, Ordering::SeqCst);
    let join = handle.join.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(join) = join {
        let _ = join.join();
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

/// 服务端：一条 accept 循环，按 [`Frame`] 分派握手与聊天。非 OneLink 客户端
/// 发来的畸形字节只记日志并关流，不影响进程。
pub async fn serve_frames(
    endpoint: &Endpoint,
    our_name: &str,
    queue: &Arc<EventQueue>,
) -> anyhow::Result<()> {
    while let Some(incoming) = endpoint.accept().await {
        let our_name = our_name.to_string();
        let queue = Arc::clone(queue);
        tokio::spawn(async move {
            let conn = match incoming.await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("incoming connection failed: {e}");
                    return;
                }
            };
            if let Err(e) = dispatch_connection(conn, &our_name, &queue).await {
                tracing::warn!("frame dispatch failed: {e:#}");
            }
        });
    }
    Ok(())
}

/// 读一帧并回应：`Hello` 回 `HelloAck`，`Chat` 入队 `ChatReceived` 并回 `ChatAck`。
pub async fn dispatch_connection(
    conn: iroh::endpoint::Connection,
    our_name: &str,
    queue: &Arc<EventQueue>,
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
                name: our_name.to_string(),
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
        other => tracing::warn!("unexpected frame on server side: {other:?}"),
    }
    Ok(())
}

/// 启动 iroh 服务线程：Endpoint(presets::N0，官方 relay 兜底) + mDNS 发现 +
/// 连接 accept 循环。事件经有界队列暴露在 `handle.tx` 供 UI 轮询。
pub fn spawn_iroh_service() -> anyhow::Result<IrohServiceHandle> {
    let queue = EventQueue::new(EVENT_QUEUE_CAPACITY);
    let service_queue = Arc::clone(&queue);
    let err_queue = Arc::clone(&queue);
    let stop = Arc::new(AtomicBool::new(false));
    let alive = Arc::new(AtomicBool::new(true));
    let seq = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (commands_tx, mut commands_rx) = tokio::sync::mpsc::unbounded_channel::<Command>();
    let thread_stop = Arc::clone(&stop);
    let thread_alive = Arc::clone(&alive);
    let join = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");
        let result = rt.block_on(async move {
            let endpoint = Endpoint::builder(presets::N0)
                .alpns(vec![ALPN.to_vec()])
                .bind()
                .await
                .map_err(|e| anyhow::anyhow!("endpoint bind: {e}"))?;
            let self_id = endpoint.id();
            let node_id = self_id.to_string();
            let our_name = local_device_name();
            tracing::info!(node_id, our_name, %MDNS_SERVICE, "iroh endpoint bound");

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
            let serve_name = our_name.clone();
            let serve_queue = Arc::clone(&service_queue);
            let serve_task = tokio::spawn(async move {
                if let Err(e) = serve_frames(&endpoint_serve, &serve_name, &serve_queue).await {
                    tracing::warn!("serve_frames ended: {e:#}");
                }
            });

            // 发现任务：mDNS discovered/expired → UiEvent；发现即主动握手换设备名。
            let mdns_queue = Arc::clone(&service_queue);
            let endpoint_mdns = endpoint.clone();
            let mdns_name = our_name.clone();
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
                                match handshake_name(&ep, EndpointAddr::from(peer_id), &name).await
                                {
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
                        tokio::spawn(async move { run_command(ep, cmd, &q).await; });
                    }
                    _ = wait_for_stop(Arc::clone(&thread_stop)) => {
                        tracing::info!("stop requested");
                        break;
                    }
                }
            }
            mdns_task.abort();
            serve_task.abort();
            // close 要等 relay/QUIC 收尾，正常网络下 <300ms，但弱网可能久；
            // 退出路径不为此卡住 UI，超时就交给 runtime 限时关闭。
            let closed = tokio::time::timeout(Duration::from_millis(800), endpoint.close()).await;
            tracing::info!(?closed, "endpoint close attempted, service thread exiting");
            Ok::<(), anyhow::Error>(())
        });
        if let Err(e) = result {
            err_queue.push(UiEvent::NetError(format!("service error: {e:#}")));
        }
        rt.shutdown_timeout(Duration::from_millis(500));
        thread_alive.store(false, Ordering::SeqCst);
    });

    Ok(IrohServiceHandle {
        tx: queue,
        commands: commands_tx,
        seq,
        stop,
        alive,
        join: Mutex::new(Some(join)),
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
