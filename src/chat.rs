//! 会话模型（纯逻辑，无 I/O）：按对端 node id 存消息、算未读、记录当前选中会话。
//!
//! UI 渲染只读这里的数据；网络事件由 `app.rs` 落到 `append`。

use std::collections::BTreeMap;

/// 一条聊天消息（M2 只存内存，M4 再换 SQLite）。
#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessage {
    /// 消息 id：自己发的由本地计数器生成，收到的沿用对端 id（仅用于 ACK 关联与渲染 key）。
    pub id: u64,
    pub text: String,
    pub sent_at_ms: u64,
    /// `true` = 自己发的（右对齐），`false` = 对端发来的（左对齐）。
    pub mine: bool,
}

#[derive(Debug, Default)]
pub struct ChatStore {
    conversations: BTreeMap<String, Vec<ChatMessage>>,
    unread: BTreeMap<String, u32>,
    selected: Option<String>,
}

impl ChatStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加一条消息。非自己发的、且所在会话未选中时累计未读。
    pub fn append(&mut self, peer: &str, msg: ChatMessage) {
        if !msg.mine && self.selected.as_deref() != Some(peer) {
            *self.unread.entry(peer.to_string()).or_insert(0) += 1;
        }
        self.conversations
            .entry(peer.to_string())
            .or_default()
            .push(msg);
    }

    /// 某个会话的全部消息（按追加顺序）；未知 peer 返回空切片。
    pub fn messages(&self, peer: &str) -> &[ChatMessage] {
        self.conversations
            .get(peer)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// 选中一个会话：清掉它的未读。未知 peer 也会建立空会话条目（点设备行即开新会话）。
    pub fn select(&mut self, peer: &str) {
        self.selected = Some(peer.to_string());
        self.unread.remove(peer);
        self.conversations.entry(peer.to_string()).or_default();
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn unread(&self, peer: &str) -> u32 {
        self.unread.get(peer).copied().unwrap_or(0)
    }

    /// 所有会话的 peer id（BTreeMap 保证字典序）。
    pub fn peers(&self) -> Vec<String> {
        self.conversations.keys().cloned().collect()
    }
}
