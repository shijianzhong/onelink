//! 会话模型（纯逻辑，无 I/O）：按对端 node id 存消息、算未读、记录当前选中会话。
//!
//! UI 渲染只读这里的数据；网络事件由 `app.rs` 落到 `append`，持久化由 `store` 负责。

use std::collections::BTreeMap;

/// 出站消息的投递状态（入站消息恒为 `Pending`，其“已读”由 `read` 字段表达）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageStatus {
    Pending,
    Delivered,
    Failed,
}

impl MessageStatus {
    /// 落库用的整数（与 SQLite `status` 列一致）。
    pub fn as_i64(self) -> i64 {
        match self {
            Self::Pending => 0,
            Self::Delivered => 1,
            Self::Failed => 2,
        }
    }

    /// 从库里读回；未知值按 `Pending` 处理，避免旧库导致读取失败。
    pub fn from_i64(value: i64) -> Self {
        match value {
            1 => Self::Delivered,
            2 => Self::Failed,
            _ => Self::Pending,
        }
    }
}

/// 一条聊天消息。
#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessage {
    /// 消息 id：自己发的由本地计数器生成，收到的沿用对端 id（仅用于 ACK 关联与渲染 key）。
    pub id: u64,
    pub text: String,
    pub sent_at_ms: u64,
    /// `true` = 自己发的（右对齐），`false` = 对端发来的（左对齐）。
    pub mine: bool,
    /// 出站投递状态；入站消息不关心。
    pub status: MessageStatus,
    /// 入站消息是否已读（出站恒为 false）。未读数由此派生，不再单独存计数器。
    pub read: bool,
}

#[derive(Debug, Default)]
pub struct ChatStore {
    conversations: BTreeMap<String, Vec<ChatMessage>>,
    selected: Option<String>,
}

impl ChatStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 用持久化的历史填充内存模型（按传入顺序追加）。
    pub fn hydrate(&mut self, histories: BTreeMap<String, Vec<ChatMessage>>) {
        for (peer, messages) in histories {
            let conv = self.conversations.entry(peer).or_default();
            conv.extend(messages);
        }
    }

    /// 追加一条消息；同一个 `(id, mine)` 重复到达时丢弃并返回 `false`（与库里的唯一约束一致）。
    ///
    /// 入站消息在会话选中时直接标记已读，否则留给未读计数。
    pub fn append(&mut self, peer: &str, mut msg: ChatMessage) -> bool {
        let selected = self.selected.as_deref() == Some(peer);
        if !msg.mine {
            msg.read = selected;
        }
        let conv = self.conversations.entry(peer.to_string()).or_default();
        if conv.iter().any(|m| m.id == msg.id && m.mine == msg.mine) {
            return false;
        }
        conv.push(msg);
        true
    }

    /// 某个会话的全部消息（按追加顺序）；未知 peer 返回空切片。
    pub fn messages(&self, peer: &str) -> &[ChatMessage] {
        self.conversations
            .get(peer)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// 选中一个会话：把它的入站消息全部标记已读。未知 peer 也会建立空会话条目。
    pub fn select(&mut self, peer: &str) {
        self.selected = Some(peer.to_string());
        if let Some(conv) = self.conversations.get_mut(peer) {
            for msg in conv.iter_mut().filter(|m| !m.mine) {
                msg.read = true;
            }
        } else {
            self.conversations.entry(peer.to_string()).or_default();
        }
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// 未读数：该会话中未读的入站消息条数。
    pub fn unread(&self, peer: &str) -> u32 {
        self.conversations
            .get(peer)
            .map(|conv| conv.iter().filter(|m| !m.mine && !m.read).count() as u32)
            .unwrap_or(0)
    }

    /// 下一条出站消息 id：当前会话出站消息的最大 id + 1（重启后从库里续号，避免与历史冲突）。
    pub fn next_out_msg_id(&self, peer: &str) -> u64 {
        self.conversations
            .get(peer)
            .and_then(|conv| conv.iter().filter(|m| m.mine).map(|m| m.id).max())
            .unwrap_or(0)
            + 1
    }

    /// 更新某条出站消息的投递状态；找不到返回 `false`。
    pub fn set_status(&mut self, peer: &str, id: u64, status: MessageStatus) -> bool {
        let Some(conv) = self.conversations.get_mut(peer) else {
            return false;
        };
        let Some(msg) = conv.iter_mut().find(|m| m.mine && m.id == id) else {
            return false;
        };
        msg.status = status;
        true
    }

    /// 所有会话的 peer id（BTreeMap 保证字典序）。
    pub fn peers(&self) -> Vec<String> {
        self.conversations.keys().cloned().collect()
    }
}
