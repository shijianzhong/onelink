//! 持久化入口：单写线程 + mpsc 命令，UI/网络层只投递命令，SQLite 永不阻塞它们。

pub mod rows;
pub mod schema;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::thread::JoinHandle;
use std::time::SystemTime;

use rusqlite::Connection;

use crate::chat::{ChatMessage, MessageStatus};
use crate::model::DeviceInfo;

/// 启动时从库里读出的快照（设备先按离线渲染，历史按会话分组）。
#[derive(Debug, Default)]
pub struct Loaded {
    pub devices: Vec<DeviceInfo>,
    pub histories: BTreeMap<String, Vec<ChatMessage>>,
    pub self_nickname: Option<String>,
    /// 本机头像（base64 PNG）；头像坏掉/未设置时为 `None`。
    pub self_avatar: Option<String>,
}

enum Command {
    UpsertDevice(Box<DeviceInfo>),
    SetAlias {
        node_id: String,
        alias: Option<String>,
    },
    InsertMessage {
        peer: String,
        msg: Box<ChatMessage>,
    },
    SetMessageStatus {
        peer: String,
        msg_id: u64,
        status: MessageStatus,
    },
    MarkRead(String),
    SetLastSeen {
        node_id: String,
        last_seen: SystemTime,
    },
    SetSelfNickname(String),
    /// 保存本机头像（base64 PNG）；`None` 表示清除。
    SetSelfAvatar(Option<String>),
    /// 删除设备，连同它的会话与消息（不可撤销）。
    DeleteDevice(String),
}

/// 持久化句柄。写入异步投递到后台线程；`Drop` 时关闭通道并 join，保证已投递命令落盘。
pub struct Store {
    tx: Option<Sender<Command>>,
    join: Option<JoinHandle<()>>,
}

impl Store {
    /// 打开（或新建）数据库。打不开/损坏时备份为 `.bak` 并重建空库；再失败则退回内存库，
    /// 保证 UI 永不因为存储问题崩溃。
    pub fn open(path: &Path) -> (Self, Loaded) {
        match Self::try_open(path) {
            Ok(opened) => opened,
            Err(err) => {
                tracing::warn!(error = %err, path = %path.display(), "数据库打开失败，备份后重建");
                backup(path);
                match Self::try_open(path) {
                    Ok(opened) => opened,
                    Err(err) => {
                        tracing::error!(error = %err, "重建数据库仍失败，改用内存库");
                        Self::open_in_memory()
                    }
                }
            }
        }
    }

    /// 内存库（测试/兜底用），同样走单写线程。
    pub fn open_in_memory() -> (Self, Loaded) {
        let conn = Connection::open_in_memory().expect("open in-memory sqlite");
        let loaded = prepare(&conn, false).expect("init in-memory schema");
        spawn(conn, loaded)
    }

    fn try_open(path: &Path) -> anyhow::Result<(Self, Loaded)> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        let loaded = prepare(&conn, true)?;
        Ok(spawn(conn, loaded))
    }

    pub fn upsert_device(&self, info: &DeviceInfo) {
        self.send(Command::UpsertDevice(Box::new(info.clone())));
    }

    /// 只写本地备注；不会碰 `remote_name`。
    pub fn set_alias(&self, node_id: &str, alias: Option<&str>) {
        self.send(Command::SetAlias {
            node_id: node_id.to_string(),
            alias: alias.map(str::to_string),
        });
    }

    pub fn insert_message(&self, peer: &str, msg: &ChatMessage) {
        self.send(Command::InsertMessage {
            peer: peer.to_string(),
            msg: Box::new(msg.clone()),
        });
    }

    pub fn set_message_status(&self, peer: &str, msg_id: u64, status: MessageStatus) {
        self.send(Command::SetMessageStatus {
            peer: peer.to_string(),
            msg_id,
            status,
        });
    }

    pub fn mark_read(&self, peer: &str) {
        self.send(Command::MarkRead(peer.to_string()));
    }

    pub fn set_last_seen(&self, node_id: &str, last_seen: SystemTime) {
        self.send(Command::SetLastSeen {
            node_id: node_id.to_string(),
            last_seen,
        });
    }

    pub fn set_self_nickname(&self, nickname: &str) {
        self.send(Command::SetSelfNickname(nickname.to_string()));
    }

    /// 保存本机头像（base64 PNG）；传 `None` 清除。
    pub fn set_self_avatar(&self, avatar: Option<String>) {
        self.send(Command::SetSelfAvatar(avatar));
    }

    /// 删除设备（离线设备的清理入口）：设备行、会话、历史消息一起删，不可撤销。
    pub fn delete_device(&self, node_id: &str) {
        self.send(Command::DeleteDevice(node_id.to_string()));
    }

    fn send(&self, cmd: Command) {
        if let Some(tx) = &self.tx
            && let Err(err) = tx.send(cmd)
        {
            tracing::warn!(error = %err, "写库命令投递失败（写入线程已退出）");
        }
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        // 关闭通道后后台线程会把已排队的命令执行完再退出：关窗不丢未落盘的数据。
        self.tx.take();
        if let Some(join) = self.join.take()
            && let Err(err) = join.join()
        {
            tracing::warn!(error = ?err, "写库线程异常退出");
        }
    }
}

/// 打开连接后的初始化：WAL、完整性检查、schema 迁移、读取快照。
fn prepare(conn: &Connection, file: bool) -> anyhow::Result<Loaded> {
    if file {
        conn.pragma_update(None, "journal_mode", "WAL")?;
    }
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    let check: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if check != "ok" {
        anyhow::bail!("integrity_check 失败: {check}");
    }
    schema::init(conn)?;
    rows::load(conn)
}

fn spawn(conn: Connection, loaded: Loaded) -> (Store, Loaded) {
    let (tx, rx) = channel::<Command>();
    let join = std::thread::Builder::new()
        .name("onelink-store".to_string())
        .spawn(move || {
            while let Ok(cmd) = rx.recv() {
                if let Err(err) = apply(&conn, cmd) {
                    tracing::warn!(error = %err, "写库失败");
                }
            }
        })
        .expect("spawn store thread");
    (
        Store {
            tx: Some(tx),
            join: Some(join),
        },
        loaded,
    )
}

fn apply(conn: &Connection, cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::UpsertDevice(info) => rows::upsert_device(conn, &info),
        Command::SetAlias { node_id, alias } => rows::set_alias(conn, &node_id, alias.as_deref()),
        Command::InsertMessage { peer, msg } => rows::insert_message(conn, &peer, &msg),
        Command::SetMessageStatus {
            peer,
            msg_id,
            status,
        } => rows::set_message_status(conn, &peer, msg_id, status),
        Command::MarkRead(peer) => rows::mark_read(conn, &peer),
        Command::SetLastSeen { node_id, last_seen } => {
            rows::set_last_seen(conn, &node_id, last_seen)
        }
        Command::SetSelfNickname(nickname) => rows::set_self_nickname(conn, &nickname),
        Command::SetSelfAvatar(avatar) => rows::set_self_avatar(conn, avatar.as_deref()),
        Command::DeleteDevice(node_id) => rows::delete_device(conn, &node_id),
    }
}

/// 把打不开/损坏的库挪到 `*.bak`（同名旧备份先删除），便于用户补救。
///
/// WAL 模式会留下 `-wal` / `-shm` 两个旁路文件，必须一起挪走；否则重建后的空库会把
/// 旧 WAL 当成自己的日志回放，再次打不开。
fn backup(path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let src = PathBuf::from(format!("{}{suffix}", path.display()));
        let dst = PathBuf::from(format!("{}.bak{suffix}", path.display()));
        if !src.exists() {
            continue;
        }
        if dst.exists() {
            let _ = std::fs::remove_file(&dst);
        }
        match std::fs::rename(&src, &dst) {
            Ok(()) => tracing::warn!(path = %dst.display(), "已备份原数据库文件"),
            Err(err) => tracing::warn!(error = %err, "备份原数据库文件失败"),
        }
    }
}
