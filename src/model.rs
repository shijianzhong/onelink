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
}
