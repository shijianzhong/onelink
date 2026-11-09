//! SQLite 行 ↔ 领域对象转换，以及所有 SQL 语句。

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::{Connection, params};

use crate::chat::{ChatMessage, MessageStatus};
use crate::model::DeviceInfo;

use super::Loaded;

fn to_ms(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn from_ms(ms: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64)
}

fn now_ms() -> i64 {
    to_ms(SystemTime::now())
}

/// 读取全部设备、历史消息与本地昵称（库为空时返回默认值）。
pub fn load(conn: &Connection) -> Result<Loaded> {
    let mut devices = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT node_id, remote_name, local_alias, last_addr, first_seen_ms, last_seen_ms
             FROM devices",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            devices.push(DeviceInfo {
                node_id: row.get(0)?,
                name: row.get(1)?,
                alias: row.get(2)?,
                addr: row.get(3)?,
                // 启动时一律先按离线渲染，等 mDNS/握手再置在线。
                online: false,
                last_seen: from_ms(row.get(5)?),
            });
        }
    }
    let mut histories: BTreeMap<String, Vec<ChatMessage>> = BTreeMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT peer_node_id, msg_id, direction, text, sent_at_ms, status, read
             FROM messages
             ORDER BY peer_node_id, sent_at_ms, id",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let direction: i64 = row.get(2)?;
            let status: i64 = row.get(5)?;
            let msg = ChatMessage {
                id: row.get::<_, i64>(1)? as u64,
                text: row.get(3)?,
                sent_at_ms: row.get::<_, i64>(4)? as u64,
                mine: direction == 1,
                status: MessageStatus::from_i64(status),
                read: row.get::<_, i64>(6)? != 0,
            };
            histories
                .entry(row.get::<_, String>(0)?)
                .or_default()
                .push(msg);
        }
    }
    let self_nickname: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'self_nickname'",
            [],
            |row| row.get(0),
        )
        .ok()
        // 空串代表"用默认名"：加载时归一成 None，UI/网络层按主机名兜底。
        .filter(|name: &String| !name.trim().is_empty());
    Ok(Loaded {
        devices,
        histories,
        self_nickname,
    })
}

/// 设备信息入库：`remote_name` 可被网络更新，`local_alias` 永不被这里覆盖。
pub fn upsert_device(conn: &Connection, info: &DeviceInfo) -> Result<()> {
    let now = now_ms();
    conn.execute(
        "INSERT INTO devices(node_id, remote_name, local_alias, last_addr, first_seen_ms, last_seen_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(node_id) DO UPDATE SET
             remote_name = excluded.remote_name,
             last_addr   = excluded.last_addr,
             last_seen_ms = excluded.last_seen_ms",
        params![
            info.node_id,
            info.name,
            info.alias,
            info.addr,
            now,
            to_ms(info.last_seen)
        ],
    )?;
    Ok(())
}

/// 只写本地备注（`None` 表示清除）。
pub fn set_alias(conn: &Connection, node_id: &str, alias: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE devices SET local_alias = ?2 WHERE node_id = ?1",
        params![node_id, alias],
    )?;
    Ok(())
}

/// 删除一台设备及其会话与历史消息（同一事务，避免留下孤儿数据）。
pub fn delete_device(conn: &Connection, node_id: &str) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM messages WHERE peer_node_id = ?1",
        params![node_id],
    )?;
    tx.execute(
        "DELETE FROM conversations WHERE peer_node_id = ?1",
        params![node_id],
    )?;
    tx.execute("DELETE FROM devices WHERE node_id = ?1", params![node_id])?;
    tx.commit()?;
    Ok(())
}

/// 消息入库（同一 peer/direction/msg_id 重复时静默丢弃），并推进会话末尾时间。
pub fn insert_message(conn: &Connection, peer: &str, msg: &ChatMessage) -> Result<()> {
    conn.execute(
        "INSERT INTO conversations(peer_node_id, created_at_ms, last_message_at_ms)
         VALUES (?1, ?2, ?2)
         ON CONFLICT(peer_node_id) DO UPDATE SET
             last_message_at_ms = MAX(last_message_at_ms, excluded.last_message_at_ms)",
        params![peer, msg.sent_at_ms as i64],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO messages
            (peer_node_id, msg_id, direction, text, sent_at_ms, status, read, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            peer,
            msg.id as i64,
            i64::from(msg.mine),
            msg.text,
            msg.sent_at_ms as i64,
            msg.status.as_i64(),
            i64::from(msg.read),
            now_ms()
        ],
    )?;
    Ok(())
}

/// 更新出站消息的投递状态。
pub fn set_message_status(
    conn: &Connection,
    peer: &str,
    msg_id: u64,
    status: MessageStatus,
) -> Result<()> {
    conn.execute(
        "UPDATE messages SET status = ?3
         WHERE peer_node_id = ?1 AND msg_id = ?2 AND direction = 1",
        params![peer, msg_id as i64, status.as_i64()],
    )?;
    Ok(())
}

/// 会话内所有入站消息置为已读。
pub fn mark_read(conn: &Connection, peer: &str) -> Result<()> {
    conn.execute(
        "UPDATE messages SET read = 1 WHERE peer_node_id = ?1 AND direction = 0",
        params![peer],
    )?;
    Ok(())
}

/// 更新设备的最后在线时间。
pub fn set_last_seen(conn: &Connection, node_id: &str, last_seen: SystemTime) -> Result<()> {
    conn.execute(
        "UPDATE devices SET last_seen_ms = ?2 WHERE node_id = ?1",
        params![node_id, to_ms(last_seen)],
    )?;
    Ok(())
}

/// 保存自己的昵称（握手时优先使用）。
pub fn set_self_nickname(conn: &Connection, nickname: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings(key, value) VALUES ('self_nickname', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![nickname],
    )?;
    Ok(())
}
