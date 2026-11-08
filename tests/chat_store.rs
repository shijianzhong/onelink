use onelink::chat::{ChatMessage, ChatStore, MessageStatus};

fn msg(id: u64, text: &str, mine: bool) -> ChatMessage {
    ChatMessage {
        id,
        text: text.into(),
        sent_at_ms: id,
        mine,
        status: MessageStatus::Pending,
        read: false,
    }
}

#[test]
fn keeps_per_peer_order_and_does_not_cross_talk() {
    let mut s = ChatStore::new();
    s.append("b", msg(1, "to-b", true));
    s.append("c", msg(2, "to-c", true));
    s.append("b", msg(3, "to-b-again", false));

    let b_texts: Vec<&str> = s.messages("b").iter().map(|m| m.text.as_str()).collect();
    assert_eq!(b_texts, ["to-b", "to-b-again"]);
    let c_texts: Vec<&str> = s.messages("c").iter().map(|m| m.text.as_str()).collect();
    assert_eq!(c_texts, ["to-c"]);
    assert!(s.messages("unknown").is_empty());
}

#[test]
fn unread_counts_incoming_only_and_clears_on_select() {
    let mut s = ChatStore::new();
    s.append("b", msg(1, "mine", true));
    s.append("b", msg(2, "theirs", false));
    s.append("b", msg(3, "theirs2", false));
    assert_eq!(s.unread("b"), 2, "自己的消息不累计未读");

    s.select("b");
    assert_eq!(s.unread("b"), 0);
    assert_eq!(s.selected(), Some("b"));

    s.append("b", msg(4, "theirs3", false));
    assert_eq!(s.unread("b"), 0, "选中期间收到不该累计");
}

#[test]
fn unread_accumulates_for_unselected_peer() {
    let mut s = ChatStore::new();
    s.select("b");
    s.append("c", msg(1, "from-c", false));
    assert_eq!(s.unread("c"), 1, "非选中会话要累计未读");
    assert_eq!(s.unread("b"), 0);
    assert_eq!(s.peers(), vec!["b".to_string(), "c".to_string()]);
}
