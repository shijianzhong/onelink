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

use crate::proto::Frame;

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
    NetError(String),
}

/// iroh 服务信息。
pub struct IrohService {
    pub node_id: String,
}

/// 服务句柄：`tx` 是 UI 侧可轮询的有界事件队列（`try_recv`）。
pub struct IrohServiceHandle {
    pub tx: Arc<EventQueue>,
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

/// 启动 iroh 服务线程：Endpoint(presets::N0，官方 relay 兜底) + mDNS 发现 +
/// 连接 accept 循环。事件经有界队列暴露在 `handle.tx` 供 UI 轮询。
pub fn spawn_iroh_service() -> anyhow::Result<IrohServiceHandle> {
    let queue = EventQueue::new(EVENT_QUEUE_CAPACITY);
    let service_queue = Arc::clone(&queue);
    let err_queue = Arc::clone(&queue);
    let stop = Arc::new(AtomicBool::new(false));
    let alive = Arc::new(AtomicBool::new(true));
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

            // 被动握手任务：accept 循环（非 ALPN 连接已在 alpns 层拒绝）。
            let endpoint_serve = endpoint.clone();
            let serve_name = our_name.clone();
            let serve_task = tokio::spawn(async move {
                if let Err(e) = serve_handshake(&endpoint_serve, &serve_name).await {
                    tracing::warn!("serve_handshake ended: {e:#}");
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
            // 对端尽快看到节点下线，再退出 runtime。
            let mut mdns_task = mdns_task;
            let mut serve_task = serve_task;
            tokio::select! {
                r = &mut mdns_task => tracing::info!("mdns task ended: {r:?}"),
                r = &mut serve_task => tracing::info!("serve task ended: {r:?}"),
                _ = wait_for_stop(Arc::clone(&thread_stop)) => tracing::info!("stop requested"),
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
