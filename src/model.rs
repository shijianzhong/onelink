//! 设备注册表：纯逻辑模型，不依赖任何网络库类型（node id 用字符串抽象）。

use std::collections::BTreeMap;
use std::time::SystemTime;

/// 一台已发现/已知设备的快照信息。
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub node_id: String,
    pub name: String,
    pub addr: Option<String>,
    pub online: bool,
    pub last_seen: SystemTime,
}

/// 注册表变更事件。`Removed` 携带被移除设备的 `node_id`。
#[derive(Debug, Clone)]
pub enum DeviceChange {
    Added(DeviceInfo),
    Updated(DeviceInfo),
    Removed(String),
}

/// 设备注册表：按 node_id 去重存储，按 name 排序输出快照。
pub struct DeviceRegistry {
    devices: BTreeMap<String, DeviceInfo>,
}

impl Default for DeviceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceRegistry {
    pub fn new() -> Self {
        Self {
            devices: BTreeMap::new(),
        }
    }

    /// 插入或更新设备。首次发现发 `Added`，已存在发 `Updated`（去重）。
    pub fn upsert(&mut self, info: DeviceInfo) -> Option<DeviceChange> {
        let node_id = info.node_id.clone();
        match self.devices.entry(node_id) {
            std::collections::btree_map::Entry::Vacant(e) => {
                e.insert(info.clone());
                Some(DeviceChange::Added(info))
            }
            std::collections::btree_map::Entry::Occupied(mut e) => {
                e.insert(info.clone());
                Some(DeviceChange::Updated(info))
            }
        }
    }

    /// 标记设备离线。未知 node_id 不产生事件。
    pub fn mark_offline(&mut self, node_id: &str) -> Option<DeviceChange> {
        let info = self.devices.get_mut(node_id)?;
        info.online = false;
        info.last_seen = SystemTime::now();
        Some(DeviceChange::Updated(info.clone()))
    }

    /// 移除设备。未知 node_id 不产生事件。
    pub fn remove(&mut self, node_id: &str) -> Option<DeviceChange> {
        if self.devices.remove(node_id).is_some() {
            Some(DeviceChange::Removed(node_id.to_string()))
        } else {
            None
        }
    }

    /// 全部设备快照，按设备名排序。
    pub fn snapshot(&self) -> Vec<DeviceInfo> {
        let mut v: Vec<DeviceInfo> = self.devices.values().cloned().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    pub fn is_known(&self, node_id: &str) -> bool {
        self.devices.contains_key(node_id)
    }

    /// 按 node_id 查设备。
    pub fn get(&self, node_id: &str) -> Option<&DeviceInfo> {
        self.devices.get(node_id)
    }
}

/// 未命名设备的占位名：node id 前 8 位。
fn placeholder_name(node_id: &str) -> String {
    node_id.chars().take(8).collect()
}

/// 把一条网络事件应用到注册表，返回产生的变更列表（UI 侧直接消费）。
pub fn apply_ui_event(
    registry: &mut DeviceRegistry,
    event: crate::net::UiEvent,
) -> Vec<DeviceChange> {
    use crate::net::UiEvent;

    match event {
        UiEvent::DeviceDiscovered { node_id, addr } => {
            // 重复发现（地址变化）保留已有名字，只刷新在线状态与地址
            let existing = registry.get(&node_id).cloned();
            let name = existing
                .as_ref()
                .map(|d| d.name.clone())
                .unwrap_or_else(|| placeholder_name(&node_id));
            let addr = addr.or_else(|| existing.and_then(|d| d.addr));
            registry
                .upsert(DeviceInfo {
                    node_id,
                    name,
                    addr,
                    online: true,
                    last_seen: SystemTime::now(),
                })
                .into_iter()
                .collect()
        }
        UiEvent::DeviceNamed { node_id, name } => {
            // 握手换来真名：更新 name / online，保留发现时拿到的地址
            let addr = registry.get(&node_id).and_then(|d| d.addr.clone());
            registry
                .upsert(DeviceInfo {
                    node_id,
                    name,
                    addr,
                    online: true,
                    last_seen: SystemTime::now(),
                })
                .into_iter()
                .collect()
        }
        UiEvent::DeviceLost { node_id } => registry.mark_offline(&node_id).into_iter().collect(),
        // 聊天消息/送达状态不进设备注册表，由 ChatStore 承接（M2 Task 4）；
        // 传输事件归传输 UI（M3 Task 5），注册表只关心设备本身。
        UiEvent::ChatReceived { .. }
        | UiEvent::ChatDelivered { .. }
        | UiEvent::ChatFailed { .. }
        | UiEvent::TransferOffer { .. }
        | UiEvent::TransferProgress { .. }
        | UiEvent::TransferCompleted { .. }
        | UiEvent::TransferFailed { .. }
        | UiEvent::TransferRejected { .. } => Vec::new(),
        UiEvent::NetError(_) => Vec::new(),
    }
}
