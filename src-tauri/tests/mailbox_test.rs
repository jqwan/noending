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
    let a = db.mailbox_send("b", None, "one").unwrap();
    let b = db.mailbox_send("b", Some("a"), "two").unwrap();
    let frontier = db.mailbox_frontier().unwrap();
    let page = db.mailbox_list("b", "unread", None, 1).unwrap();
    assert_eq!(page.items[0].id, a.id);
    let cursor = page.next_cursor.unwrap();
    assert_eq!(
        db.mailbox_list("b", "unread", Some(cursor), 1)
            .unwrap()
            .items[0]
            .id,
        b.id
    );
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
}
