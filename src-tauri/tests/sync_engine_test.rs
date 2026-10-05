//! Read-side facts after ingestion: the message projection and the source
//! cursor's zero state. A stored conversation is exactly what `get_messages` /
//! `message_projection_ids` return, and a fresh session cursor is the zero state.

use noending::domain::{Agent, ParsedSessionMessage, SessionMessageRole};
use noending::storage::{new_id, Db};

mod support;

fn open_temp_db() -> Db {
    let dir = std::env::temp_dir().join(format!("noending-test-{}", uuid::Uuid::new_v4()));
    Db::open(&dir.join("test.db")).expect("temp db")
}

fn user_msg(id: &str, text: &str) -> ParsedSessionMessage {
    support::parsed_message(id, SessionMessageRole::User, text)
}

fn assistant_msg(id: &str, text: &str) -> ParsedSessionMessage {
    support::parsed_message(id, SessionMessageRole::Assistant, text)
}

/// The default source cursor for a fresh session is the zero state; the
/// adapter layer treats it as "first ingest". The cursor lives on the SESSION
/// row — and no Context exists before any explicit update.
#[test]
fn session_cursor_defaults_are_empties() {
    let db = open_temp_db();
    let session = support::ensure_session(&db, new_id(), Agent::Pi, "cursor-defaults");
    support::ensure_session_source(&db, Agent::Pi, "cursor-defaults", "/tmp/seed");

    let c = db
        .get_session(&session.id)
        .unwrap()
        .unwrap()
        .source_cursor();
    let d = noending::domain::SourceCursor::default();
    assert_eq!(c.source_file_identity, d.source_file_identity);
    assert_eq!(c.byte_offset, 0);
    assert_eq!(c.generation, 0);
    assert_eq!(c.identity_tail_hash, String::new());

    // the fact frontier starts at zero, and no Session Context exists yet
    let state = db.get_session_ingest_state(&session.id).unwrap();
    assert_eq!(state.0, 0);
    assert_eq!(state.1, 0);
    assert!(db.message_projection_ids(&session.id).unwrap().is_empty());
    assert!(db.get_session_context(&session.id).unwrap().is_none());
}

/// The projection IS the current conversation: `get_messages` reads it in
/// ordinal order, and `after_ordinal` selects the tail.
#[test]
fn projection_is_the_current_conversation() {
    let db = open_temp_db();
    let (session, stored) = support::seed_conversation(
        &db,
        Agent::Codex,
        "projection-fixture",
        &[
            user_msg("p-1", "决定使用 postgres 作为主数据库，SQLite 只做本地状态"),
            assistant_msg("p-2", "已记录，注意约束：不能提交密钥"),
            user_msg("p-3", "下一步拆分 ingestion 与 sync 的边界"),
        ],
    );
    assert_eq!(stored.len(), 3);
    assert_eq!(db.ingested_message_sequence(&session.id).unwrap(), 3);

    let ids = db.message_projection_ids(&session.id).unwrap();
    let ordered: Vec<&str> = stored.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ordered, "projection preserves ingest order");

    let all = db.get_messages(&session.id, None, 10).unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(
        all[0].content,
        "决定使用 postgres 作为主数据库，SQLite 只做本地状态"
    );
    assert_eq!(all[2].content, "下一步拆分 ingestion 与 sync 的边界");

    let tail = db.get_messages_after(&session.id, 1, 10).unwrap();
    assert_eq!(tail.len(), 2, "ordinals strictly greater than 1");
    assert_eq!(tail[0].id, all[1].id);
}
