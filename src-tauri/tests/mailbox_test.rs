//! Persistent mailbox behavior, without desktop, CLI detection or model accounts.
use noending_lib::storage::Db;
use std::path::PathBuf;
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("ne-mailbox-{}", noending_lib::storage::new_id())))
    }
    fn db(&self) -> Db {
        let db = Db::open(&self.0.join("noending.db")).unwrap();
        db.write().execute_batch("INSERT OR IGNORE INTO sessions(id,agent,root_agent_session_id,source_kind,source_path) VALUES('a','codex','native-a','jsonl','/a'),('b','codex','native-b','jsonl','/b')").unwrap();
        db
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn messages_survive_reopen_and_reads_do_not_mark_them_read() {
    let f = Fixture::new();
    let id = {
        let db = f.db();
        db.mailbox_send_with_notification("b", Some("a"), "message\n正文", true)
            .unwrap()
            .id
    };
    let db = f.db();
    let m = db.mailbox_read("b", &id).unwrap();
    assert_eq!(m.body, "message\n正文");
    assert!(m.notify_immediately);
    assert_eq!(m.sender_session_id.as_deref(), Some("a"));
    assert!(m.read_at.is_none());
    assert!(chrono::DateTime::parse_from_rfc3339(&m.created_at).is_ok());
    let marked = db.mailbox_mark("b", &id, m.revision, true).unwrap();
    assert!(marked.read_at.is_some());
    assert_eq!(db.mailbox_list("b", "unread", None, 50).unwrap().unread, 0);
    assert_eq!(
        db.mailbox_list("b", "all", None, 50).unwrap().items[0].body,
        m.body
    );
    assert!(db
        .mailbox_mark("b", &id, marked.revision, false)
        .unwrap()
        .read_at
        .is_none());
}
#[test]
fn editing_preserves_sender_time_and_unread_but_stale_read_cannot_consume_new_content() {
    let f = Fixture::new();
    let db = f.db();
    let m = db
        .mailbox_send_with_notification("b", Some("a"), "old", true)
        .unwrap();
    let edited = db.mailbox_edit("b", &m.id, m.revision, "new").unwrap();
    assert_eq!(edited.sender_session_id, m.sender_session_id);
    assert_eq!(edited.created_at, m.created_at);
    assert!(edited.notify_immediately);
    assert!(edited.read_at.is_none());
    assert!(db.mailbox_mark("b", &m.id, m.revision, true).is_err());
    assert_eq!(db.mailbox_read("b", &m.id).unwrap().body, "new");
    let read = db.mailbox_mark("b", &m.id, edited.revision, true).unwrap();
    assert!(db
        .mailbox_edit("b", &m.id, read.revision, "ignored")
        .is_err());
    assert!(db.mailbox_delete("b", &m.id, read.revision).is_err());
    let unread = db.mailbox_mark("b", &m.id, read.revision, false).unwrap();
    db.mailbox_delete("b", &m.id, unread.revision).unwrap();
    assert!(db.mailbox_read("b", &m.id).is_err());
}
#[test]
fn self_mail_is_rejected_without_persisting_but_user_and_other_session_mail_survive_reopen() {
    let f = Fixture::new();
    {
        let db = f.db();
        for notify in [false, true] {
            let error = db
                .mailbox_send_with_notification("a", Some("a"), "self mail", notify)
                .unwrap_err();
            assert!(error.to_string().contains("不能给自己发送信箱消息"));
        }
        let user = db.mailbox_send("a", None, "user mail").unwrap();
        assert!(user.sender_session_id.is_none());
        db.mailbox_send("a", Some("b"), "other session mail")
            .unwrap();
    }
    let db = f.db();
    let page = db.mailbox_list("a", "all", None, 50).unwrap();
    assert_eq!((page.total, page.unread), (2, 2));
    assert_eq!(page.items[0].body, "user mail");
    assert_eq!(page.items[1].body, "other session mail");
    assert_eq!(page.items[1].sender_session_id.as_deref(), Some("b"));
}

#[test]
fn recipient_deletion_removes_mail_while_sender_deletion_preserves_other_inboxes() {
    let f = Fixture::new();
    let db = f.db();
    let m = db.mailbox_send("b", Some("a"), "keep provenance").unwrap();
    assert!(db.mailbox_send("b", Some("missing"), "reject").is_err());
    assert!(db.mailbox_send("missing", Some("a"), "reject").is_err());
    assert!(db.mailbox_send("b", None, "  ").is_err());
    assert!(!m.notify_immediately);
    assert!(db
        .mailbox_send_with_notification("b", None, "urgent", true)
        .is_err());
    db.write()
        .execute("DELETE FROM sessions WHERE id='a'", [])
        .unwrap();
    assert_eq!(
        db.mailbox_read("b", &m.id)
            .unwrap()
            .sender_session_id
            .as_deref(),
        Some("a")
    );
    db.write()
        .execute("DELETE FROM sessions WHERE id='b'", [])
        .unwrap();
    assert!(db.mailbox_read("b", &m.id).is_err());
}
#[test]
fn pagination_and_new_message_frontiers_do_not_confuse_edits_or_reused_counts_with_new_mail() {
    let f = Fixture::new();
    let db = f.db();
    let empty = db.mailbox_list("b", "all", None, 1).unwrap();
    assert_eq!((empty.total, empty.unread), (0, 0));
    let a = db.mailbox_send("b", None, "one").unwrap();
    let b = db.mailbox_send("b", Some("a"), "two").unwrap();
    db.mailbox_send("a", Some("b"), "another inbox").unwrap();
    let frontier = db.mailbox_frontier().unwrap();
    let page = db.mailbox_list("b", "unread", None, 1).unwrap();
    assert_eq!(page.items[0].id, a.id);
    assert_eq!((page.total, page.unread), (2, 2));
    let cursor = page.next_cursor.unwrap();
    let next = db.mailbox_list("b", "unread", Some(cursor), 1).unwrap();
    assert_eq!(next.items[0].id, b.id);
    assert_eq!((next.total, next.unread), (2, 2));
    db.mailbox_edit("b", &a.id, a.revision, "edit").unwrap();
    db.mailbox_mark("b", &b.id, b.revision, true).unwrap();
    assert!(db.mailbox_arrivals(frontier).unwrap().is_empty());
    let c = db.mailbox_send("b", None, "three").unwrap();
    assert_eq!(db.mailbox_unread("b").unwrap(), 2);
    assert_eq!(db.mailbox_arrivals(frontier).unwrap().len(), 1);
    assert_eq!(
        db.mailbox_list("b", "unread", None, 50).unwrap().items[1].id,
        c.id
    );
    for state in ["all", "unread", "read"] {
        let page = db.mailbox_list("b", state, None, 1).unwrap();
        assert_eq!((page.total, page.unread), (3, 2));
    }
    db.mailbox_delete("b", &c.id, c.revision).unwrap();
    let page = db.mailbox_list("b", "all", None, 1).unwrap();
    assert_eq!((page.total, page.unread), (2, 1));
}

#[test]
fn desktop_mailbox_pages_start_with_latest_arrivals_and_keep_agent_fifo_reads() {
    let f = Fixture::new();
    let db = f.db();
    let oldest = db.mailbox_send("b", None, "oldest").unwrap();
    let middle = db.mailbox_send("b", None, "middle").unwrap();
    let latest = db.mailbox_send("b", None, "latest").unwrap();
    db.mailbox_send("a", Some("b"), "other inbox").unwrap();
    // Same-time arrivals retain a deterministic order across page boundaries.
    db.write().execute("UPDATE mailbox_messages SET created_at='2026-11-08T12:00:00Z' WHERE recipient_session_id='b'", []).unwrap();
    db.mailbox_mark("b", &middle.id, middle.revision, true)
        .unwrap();
    let first = db.mailbox_list_latest("b", "all", None, 1).unwrap();
    assert_eq!(first.items[0].id, latest.id);
    assert_eq!((first.total, first.unread), (3, 2));
    let newest = db.mailbox_send("b", None, "new arrival").unwrap();
    db.mailbox_delete("b", &latest.id, latest.revision).unwrap();
    let second = db
        .mailbox_list_latest("b", "all", first.next_cursor, 1)
        .unwrap();
    assert_eq!(second.items[0].id, middle.id);
    let third = db
        .mailbox_list_latest("b", "all", second.next_cursor, 1)
        .unwrap();
    assert_eq!(third.items[0].id, oldest.id);
    assert!(third.next_cursor.is_none());
    let unread = db.mailbox_list_latest("b", "unread", None, 50).unwrap();
    assert_eq!(
        unread
            .items
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        vec![newest.id.as_str(), oldest.id.as_str()]
    );
    let read = db.mailbox_list_latest("b", "read", None, 50).unwrap();
    assert_eq!(read.items[0].id, middle.id);
    let agent = db.mailbox_list("b", "all", None, 50).unwrap();
    assert_eq!(
        agent
            .items
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        vec![oldest.id.as_str(), middle.id.as_str(), newest.id.as_str()]
    );
}

#[test]
fn desktop_sender_metadata_tracks_session_changes_without_changing_message_provenance() {
    let f = Fixture::new();
    let db = f.db();
    db.write()
        .execute("UPDATE sessions SET title='协作助手' WHERE id='a'", [])
        .unwrap();
    let mail = db.mailbox_send("b", Some("a"), "progress").unwrap();
    let user = db.mailbox_send("b", None, "user message").unwrap();
    let page = db.mailbox_list_latest("b", "all", None, 50).unwrap();
    assert_eq!(page.items[0].id, user.id);
    assert!(page.items[0].sender.is_none());
    let sender = page.items[1].sender.as_ref().unwrap();
    assert_eq!(sender.title.as_deref(), Some("协作助手"));
    assert_eq!(sender.agent, "codex");
    assert!(db.mailbox_list("b", "all", None, 50).unwrap().items[0]
        .sender
        .is_none());
    db.write()
        .execute("UPDATE sessions SET title='新的名称' WHERE id='a'", [])
        .unwrap();
    let page = db.mailbox_list_latest("b", "unread", None, 50).unwrap();
    assert_eq!(
        page.items[1].sender.as_ref().unwrap().title.as_deref(),
        Some("新的名称")
    );
    db.write()
        .execute("DELETE FROM sessions WHERE id='a'", [])
        .unwrap();
    let page = db.mailbox_list_latest("b", "all", None, 50).unwrap();
    let retained = &page.items[1];
    assert_eq!(retained.id, mail.id);
    assert_eq!(retained.sender_session_id.as_deref(), Some("a"));
    assert!(retained.sender.is_none());
    assert_eq!(retained.body, "progress");
}
