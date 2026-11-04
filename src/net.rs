//! iroh 服务层与事件桥接：局域网设备发现 + 有界事件队列。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use iroh::endpoint::presets;
use iroh::Endpoint;
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};

/// 连接/协议 ALPN 常量：`Endpoint::builder().alpns(...)` 声明后，不匹配的
/// 连接会在握手层被自动拒绝（iroh 官方模式）。
pub const ALPN: &[u8] = b"onelink-v1";

/// mDNS 服务名：`MdnsAddressLookupBuilder::service_name()` 覆盖默认的
/// `irohv1`，只有同名服务的端点互相发现（swarm-discovery 按名隔离）。
pub const MDNS_SERVICE: &str = "onelink-v1";

/// 事件队列容量：满时丢弃（`push_event` 不阻塞网络线程）。
pub const EVENT_QUEUE_CAPACITY: usize = 32;

/// 网络层 → UI 侧事件。
#[derive(Debug, Clone)]
pub enum UiEvent {
    DeviceDiscovered { node_id: String, addr: Option<String> },
    DeviceLost { node_id: String },
    NetError(String),
}

/// iroh 服务信息。
pub struct IrohService {
    pub node_id: String,
}

/// 服务句柄：`tx` 是 UI 侧可轮询的有界事件队列（`try_recv`）。
pub struct IrohServiceHandle {
    pub tx: Arc<EventQueue>,
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
            DiscoveryEvent::Discovered {
                endpoint_info, ..
            } => {
                let addr = endpoint_info
                    .addrs()
                    .next()
                    .map(|a| a.to_string());
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
pub fn translate_mdns_event(ev: impl MdnsEventLike) -> Vec<UiEvent> {
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

/// 启动 iroh 服务线程：Endpoint(presets::N0，官方 relay 兜底) + mDNS 发现 +
/// 连接 accept 循环。事件经有界队列暴露在 `handle.tx` 供 UI 轮询。
pub fn spawn_iroh_service() -> anyhow::Result<IrohServiceHandle> {
    let queue = EventQueue::new(EVENT_QUEUE_CAPACITY);
    let service_queue = Arc::clone(&queue);
    let err_queue = Arc::clone(&queue);
    std::thread::spawn(move || {
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
            let node_id = endpoint.id().to_string();
            tracing::info!(node_id, %MDNS_SERVICE, "iroh endpoint bound");

            let mdns = MdnsAddressLookup::builder()
                .service_name(MDNS_SERVICE)
                .build(endpoint.id())
                .map_err(|e| anyhow::anyhow!("mdns build: {e}"))?;
            endpoint
                .address_lookup()
                .map_err(|e| anyhow::anyhow!("address lookup: {e}"))?
                .add(mdns.clone());
            tracing::info!("mDNS service registered");

            // 事件任务：mDNS discovered/expired → UiEvent。
            let mdns_queue = Arc::clone(&service_queue);
            let mdns_task = tokio::spawn(async move {
                let mut stream = mdns.subscribe().await;
                while let Some(ev) = stream.next().await {
                    for ui_ev in translate_mdns_event(ev) {
                        push_event(&mdns_queue, ui_ev);
                    }
                }
                tracing::info!("mdns event stream ended");
            });

            // accept 任务：非 ALPN 连接已在 builder.alpns 层被拒绝。
            let accept_task = tokio::spawn(async move {
                while let Some(incoming) = endpoint.accept().await {
                    tokio::spawn(async move {
                        match incoming.await {
                            Ok(conn) => {
                                let remote = conn.remote_id();
                                tracing::info!("accepted connection: {remote:?}");
                                // M1 仅记录；握手（交换设备名）在 Task 5 接入。
                            }
                            Err(e) => tracing::warn!("connection handshake failed: {e}"),
                        }
                    });
                }
                tracing::info!("accept loop ended");
            });

            // 服务线程随任一任务结束而收尾（正常退出路径由 close 触发）。
            tokio::select! {
                r = mdns_task => tracing::info!("mdns task ended: {r:?}"),
                r = accept_task => tracing::info!("accept task ended: {r:?}"),
            }
            Ok::<(), anyhow::Error>(())
        });
        if let Err(e) = result {
            let _ = err_queue.push(UiEvent::NetError(format!("service error: {e:#}")));
        }
    });

    Ok(IrohServiceHandle { tx: queue })
}
