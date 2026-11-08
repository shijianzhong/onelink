//! 持久化层测试：真实 SQLite 文件读写、重启 hydrate、去重、备注保护、未读派生、id 续号、损坏恢复。

use std::time::SystemTime;

use onelink::chat::{ChatMessage, ChatStore, MessageStatus};
use onelink::model::DeviceInfo;
use onelink::store::Store;

fn device(node_id: &str, name: &str) -> DeviceInfo {
    DeviceInfo {
        node_id: node_id.into(),
        name: name.into(),
        alias: None,
        addr: None,
        online: true,
        last_seen: SystemTime::now(),
    }
}

fn message(id: u64, text: &str, mine: bool, status: MessageStatus) -> ChatMessage {
    ChatMessage {
        id,
        text: text.into(),
        sent_at_ms: 1_000 + id,
        mine,
        status,
        read: false,
    }
}

#[test]
fn nickname_persists_and_blank_means_default_on_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("onelink.db");

    {
        let (store, _) = Store::open(&path);
        store.set_self_nickname("阿钟");
    }
    let (store, loaded) = Store::open(&path);
    assert_eq!(loaded.self_nickname.as_deref(), Some("阿钟"));

    // 清空昵称 = 使用默认名；重启后应归一成 None。
    store.set_self_nickname("   ");
    drop(store);
    let (_, loaded) = Store::open(&path);
    assert!(loaded.self_nickname.is_none());
}

#[test]
fn alias_change_and_clear_survive_reopen_and_fall_back_to_remote_name() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("onelink.db");

    {
        let (store, _) = Store::open(&path);
        store.upsert_device(&device("n1", "书房Mac"));
        store.set_alias("n1", Some("客厅机器"));
    }

    // 重启：备注仍在，显示名用备注；握手名没被覆盖。
    let (store, loaded) = Store::open(&path);
    assert_eq!(loaded.devices.len(), 1);
    let saved = &loaded.devices[0];
    assert_eq!(saved.name, "书房Mac");
    assert_eq!(saved.alias.as_deref(), Some("客厅机器"));
    assert_eq!(saved.display_name(), "客厅机器");

    // 清空备注：回落到握手名，且重启后仍是"无备注"。
    store.set_alias("n1", None);
    drop(store);
    let (_, loaded) = Store::open(&path);
    let saved = &loaded.devices[0];
    assert!(saved.alias.is_none());
    assert_eq!(saved.display_name(), "书房Mac");
}

#[test]
fn messages_persist_and_hydrate_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("onelink.db");

    {
        let (store, loaded) = Store::open(&path);
        assert!(loaded.devices.is_empty());
        store.upsert_device(&device("n1", "书房Mac"));
        // 按时间先后写入：入站在前、出站在后。
        store.insert_message("n1", &message(1, "你好", false, MessageStatus::Pending));
        store.insert_message("n1", &message(2, "在的", true, MessageStatus::Delivered));
    } // Drop：关闭通道并 join，确保命令全部落盘

    let (_store, loaded) = Store::open(&path);
    assert_eq!(loaded.devices.len(), 1);
    let saved = &loaded.devices[0];
    assert_eq!(saved.name, "书房Mac");
    // 启动时一律先按离线渲染。
    assert!(!saved.online);

    let mut chat = ChatStore::new();
    chat.hydrate(loaded.histories);
    let messages = chat.messages("n1");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].text, "你好");
    assert!(!messages[0].mine);
    assert_eq!(messages[1].id, 2);
    assert!(messages[1].mine);
    assert_eq!(messages[1].status, MessageStatus::Delivered);
}

#[test]
fn duplicate_message_is_ignored_on_write_and_append() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("onelink.db");

    {
        let (store, _loaded) = Store::open(&path);
        store.upsert_device(&device("n1", "书房Mac"));
        let dup = message(5, "只算一条", false, MessageStatus::Pending);
        store.insert_message("n1", &dup);
        store.insert_message("n1", &dup);
        // 不同方向允许同号（去重键含 direction）。
        store.insert_message("n1", &message(5, "自己发的", true, MessageStatus::Pending));
    }

    let (_store, loaded) = Store::open(&path);
    let mut chat = ChatStore::new();
    chat.hydrate(loaded.histories);
    assert_eq!(chat.messages("n1").len(), 2);
    // 内存模型同样去重。
    assert!(!chat.append("n1", message(5, "只算一条", false, MessageStatus::Pending)));
}

#[test]
fn alias_and_remote_name_never_overwrite_each_other() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("onelink.db");

    {
        let (store, _loaded) = Store::open(&path);
        store.upsert_device(&device("n1", "书房Mac"));
        store.set_alias("n1", Some("书房"));
        // 网络侧改名：只能更新 remote_name，不能清掉备注。
        store.upsert_device(&device("n1", "书房的 MacBook Pro"));
    }

    let (_store, loaded) = Store::open(&path);
    let saved = &loaded.devices[0];
    assert_eq!(saved.name, "书房的 MacBook Pro");
    assert_eq!(saved.alias.as_deref(), Some("书房"));
    assert_eq!(saved.display_name(), "书房");
}

#[test]
fn unread_is_derived_from_read_flags() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("onelink.db");

    {
        let (store, _loaded) = Store::open(&path);
        store.upsert_device(&device("n1", "书房Mac"));
        store.insert_message("n1", &message(1, "未读一", false, MessageStatus::Pending));
        store.insert_message("n1", &message(2, "未读二", false, MessageStatus::Pending));
        store.insert_message("n1", &message(3, "自己发的", true, MessageStatus::Pending));
    }

    let (store, loaded) = Store::open(&path);
    let mut chat = ChatStore::new();
    chat.hydrate(loaded.histories);
    assert_eq!(chat.unread("n1"), 2);

    chat.select("n1");
    assert_eq!(chat.unread("n1"), 0);
    store.mark_read("n1");
    drop(store);

    let (_store, loaded) = Store::open(&path);
    let mut chat = ChatStore::new();
    chat.hydrate(loaded.histories);
    assert_eq!(chat.unread("n1"), 0);
    assert!(
        chat.messages("n1")
            .iter()
            .filter(|m| !m.mine)
            .all(|m| m.read)
    );
}

#[test]
fn out_msg_id_continues_from_persisted_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("onelink.db");

    {
        let (store, _loaded) = Store::open(&path);
        store.upsert_device(&device("n1", "书房Mac"));
        store.insert_message("n1", &message(1, "第一条", true, MessageStatus::Pending));
        store.insert_message("n1", &message(7, "第二条", true, MessageStatus::Pending));
        // 入站 id 不参与续号。
        store.insert_message("n1", &message(9, "对端消息", false, MessageStatus::Pending));
    }

    let (_store, loaded) = Store::open(&path);
    let mut chat = ChatStore::new();
    chat.hydrate(loaded.histories);
    assert_eq!(chat.next_out_msg_id("n1"), 8);
    assert_eq!(chat.next_out_msg_id("unknown"), 1);
}

#[test]
fn corrupt_database_is_backed_up_and_rebuilt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("onelink.db");
    std::fs::write(&path, b"this is definitely not a sqlite database").unwrap();

    let (store, loaded) = Store::open(&path);
    assert!(loaded.devices.is_empty());
    assert!(loaded.histories.is_empty());
    assert!(path.exists());
    assert!(
        std::path::PathBuf::from(format!("{}.bak", path.display())).exists(),
        "损坏的库应备份为 .bak"
    );

    // 重建后的库可正常读写。
    store.upsert_device(&device("n1", "书房Mac"));
    drop(store);

    let (_store, loaded) = Store::open(&path);
    assert_eq!(loaded.devices.len(), 1);
}
