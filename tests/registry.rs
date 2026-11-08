use std::time::SystemTime;

use onelink::model::{DeviceChange, DeviceInfo, DeviceRegistry};

fn dev(node_id: &str, name: &str, online: bool) -> DeviceInfo {
    DeviceInfo {
        node_id: node_id.into(),
        name: name.into(),
        alias: None,
        addr: None,
        online,
        last_seen: SystemTime::now(),
    }
}

#[test]
fn upsert_dedups_and_emits_added_then_updated() {
    let mut reg = DeviceRegistry::new();
    let a = dev("node-a", "书房Mac", true);
    assert!(matches!(
        reg.upsert(a.clone()),
        Some(DeviceChange::Added(_))
    ));
    let a2 = dev("node-a", "书房Mac", true); // 重复发现同一设备
    assert!(matches!(reg.upsert(a2), Some(DeviceChange::Updated(_))));
    assert_eq!(reg.snapshot().len(), 1); // 不出现重复行
}

#[test]
fn mark_offline_toggles_state_and_remove_deletes() {
    let mut reg = DeviceRegistry::new();
    reg.upsert(dev("n1", "云主机", true));
    assert!(matches!(
        reg.mark_offline("n1"),
        Some(DeviceChange::Updated(_))
    ));
    assert!(!reg.snapshot()[0].online);
    assert!(matches!(reg.remove("n1"), Some(DeviceChange::Removed(_))));
    assert!(reg.snapshot().is_empty());
}

#[test]
fn snapshot_sorted_by_name_and_unknown_id_is_not_offlined() {
    let mut reg = DeviceRegistry::new();
    reg.upsert(dev("n2", "B", true));
    reg.upsert(dev("n1", "A", false));
    let snap = reg.snapshot();
    assert_eq!(snap[0].name, "A");
    assert!(reg.mark_offline("ghost").is_none()); // 未知设备不产生事件
}
