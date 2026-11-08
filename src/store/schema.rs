//! 数据库结构定义与版本迁移。
//!
//! 版本号存在 `PRAGMA user_version`：0 = 尚未初始化；之后逐版迁移，不降级。

use anyhow::{Result, bail};
use rusqlite::Connection;

/// 当前 schema 版本。
pub const SCHEMA_VERSION: i32 = 1;

/// v1 建表语句（与已评审方案严格一致）。
const V1: &str = "
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS devices (
    node_id       TEXT PRIMARY KEY,
    remote_name   TEXT NOT NULL,
    local_alias   TEXT,
    last_addr     TEXT,
    first_seen_ms INTEGER NOT NULL,
    last_seen_ms  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS conversations (
    peer_node_id      TEXT PRIMARY KEY REFERENCES devices(node_id),
    created_at_ms     INTEGER NOT NULL,
    last_message_at_ms INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS messages (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    peer_node_id TEXT NOT NULL REFERENCES conversations(peer_node_id),
    msg_id       INTEGER NOT NULL,
    direction    INTEGER NOT NULL,
    text         TEXT NOT NULL,
    sent_at_ms   INTEGER NOT NULL,
    status       INTEGER NOT NULL DEFAULT 0,
    read         INTEGER NOT NULL DEFAULT 0,
    created_at_ms INTEGER NOT NULL,
    UNIQUE(peer_node_id, direction, msg_id)
);
CREATE INDEX IF NOT EXISTS idx_msg_conv ON messages(peer_node_id, sent_at_ms, id);
";

/// 初始化/迁移到当前版本。遇到比自己新的库直接报错（不降级、不乱改）。
pub fn init(conn: &Connection) -> Result<()> {
    let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        bail!("数据库版本 {version} 高于当前支持的 {SCHEMA_VERSION}");
    }
    if version == 0 {
        conn.execute_batch(V1)?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    Ok(())
}
