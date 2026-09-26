//! Base Experience invariants: ingestion is facts-only.
//!
//! Ingestion is the AUTOMATIC half: Agent sources flow through the typed
//! adapters, resolve against the logical Session graph, and land as messages,
//! stats, cursors and the fact generation. It NEVER calls AI and NEVER writes
//! Context — the Session summary and Workstream state are written only by an
//! explicit user action. Pinned end to end through `ingestion::ingest_session`,
//! the one path reconcilers and launch preparation share.

use noending::domain::{
    Agent, Session, SessionContextFields, SessionMessageRole, SourceCursorUpdate, Workstream,
};
use noending::storage::{new_id, now, Db};
use noending::{ingestion, lifecycle, search};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

mod support;

fn unique_dir(tag: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "noending-base-{}-{}-{}",
        tag,
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn open_db(tag: &str) -> Db {
    Db::open(&unique_dir(tag).join("test.db")).unwrap()
}

/// Deterministic per-content uuid so a re-scan hashes to the same events.
fn claude_line(role: &str, text: &str) -> String {
    format!(
        r#"{{"type":"{role}","uuid":"u-{role}-{text}","timestamp":"2026-09-19T10:00:00Z","message":{{"role":"{role}","content":[{{"type":"text","text":"{text}"}}]}}}}"#
    )
}

const MSGS: [&str; 3] = [
    "决定改用 postgres-primary-store 作为主数据库，SQLite 只保留本地状态",
    "补充约束：api keys 不能提交到仓库，必须走环境变量注入",
    "下一步先把 ingestion 与 sync 的边界拆干净，再处理 launcher 的 prepare 路径",
];

/// Write a transcript containing the first `n` fixture messages.
fn write_transcript(dir: &std::path::Path, n: usize) -> PathBuf {
    let file = dir.join("session.jsonl");
    let body = MSGS[..n]
        .iter()
        .enumerate()
        .map(|(i, m)| claude_line(if i % 2 == 0 { "user" } else { "assistant" }, m))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&file, body).unwrap();
    file
}

/// A Logical Session whose ROOT member's source is the transcript at `path`.
/// Ingestion reads the member's `source_path`, never a session-level raw_path.
fn session_row(db: &Db, path: &std::path::Path) -> Session {
    let root_agent_session_id = format!("as-{}", new_id());
    let (id, _) = db
        .upsert_logical_session_unchecked(
            Agent::ClaudeCode,
            &root_agent_session_id,
            None,
            None,
            None,
            None,
            Some(&now()),
            Some(&now()),
        )
        .unwrap();
    support::ensure_root_member(
        db,
        &id,
        Agent::ClaudeCode,
        &root_agent_session_id,
        &path.to_string_lossy(),
    );
    db.get_session(&id).unwrap().unwrap()
}

fn ws_row(db: &Db, title: &str) -> Workstream {
    let w = Workstream {
        id: new_id(),
        title: title.into(),
        description: String::new(),
        lifecycle: "active".into(),
        visibility: "normal".into(),
        created_at: now(),
        updated_at: now(),
    };
    db.upsert_workstream(&w).unwrap();
    w
}

#[test]
fn an_empty_replacement_generation_invalidates_the_old_session_summary() {
    let db = open_db("empty-generation-context");
    let (session, member_id, _) = support::seed_conversation(
        &db,
        Agent::ClaudeCode,
        "empty-generation-context",
        &[support::parsed_message(
            "old-message",
            SessionMessageRole::User,
            "old transcript",
        )],
    );
    let initial_generation = db.get_session_ingest_state(&session.id).unwrap().generation;
    db.tx(|tx| {
        noending::storage::context_repo::commit_session_context_conn(
            tx,
            &session.id,
            &SessionContextFields {
                summary_current_state: "summary of old transcript".into(),
                ..Default::default()
            },
            0,
            initial_generation,
            1,
        )?;
        Ok(())
    })
    .unwrap();

    db.commit_member_ingest(
        &session.id,
        &member_id,
        &[],
        None,
        &SourceCursorUpdate {
            file_identity: "empty-rewrite".into(),
            generation: 1,
            byte_offset: 0,
            last_seen_size: 0,
            mtime: None,
            start_byte_offset: 0,
            prefix_hash: String::new(),
        },
    )
    .unwrap();

    assert!(
        noending::context::session_context_view(&db, &session.id)
            .unwrap()
            .pending
    );
    let outcome = noending::context::update_session(&db, &session.id).unwrap();
    assert_eq!(
        outcome.status,
        noending::domain::ContextUpdateStatus::Updated
    );

    let view = noending::context::session_context_view(&db, &session.id).unwrap();
    assert!(!view.pending);
    assert_eq!(view.fields.unwrap(), SessionContextFields::default());
    assert_eq!(view.ingest_generation, initial_generation + 1);
}

fn count(db: &Db, table: &str) -> i64 {
    db.read()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

/// (context_items, context_item_revisions, context_conflicts, session_contexts)
/// — every row an AI Context update would write. Ingestion must leave all four
/// at zero.
fn context_footprint(db: &Db) -> (i64, i64, i64, i64) {
    (
        count(db, "context_items"),
        count(db, "context_item_revisions"),
        count(db, "context_conflicts"),
        count(db, "session_contexts"),
    )
}

/// The core invariant: ingestion runs, Context does not — even for a Session
/// that already has an Owner Workstream, which is exactly the row an update
/// WOULD write into.
#[test]
fn ingestion_writes_facts_and_zero_context() {
    let dir = unique_dir("facts-only");
    let db = open_db("facts-only");
    let file = write_transcript(&dir, 3);
    let s = session_row(&db, &file);
    let ws = ws_row(&db, "NoEnding");
    db.set_session_owner(&s.id, Some(&ws.id)).unwrap();

    let ingested = ingestion::ingest_session(&db, &s).unwrap();
    assert_eq!(ingested, 3, "every Agent event is ingested");

    // Facts landed: messages, projection, generation, stats and cursor.
    assert_eq!(db.message_count(&s.id).unwrap(), 3);
    assert_eq!(db.ingested_message_sequence(&s.id).unwrap(), 3);
    let state = db.get_session_ingest_state(&s.id).unwrap();
    assert_eq!(state.latest_message_seq, 3);
    assert_eq!(
        state.generation, 1,
        "the first genesis read establishes generation 1"
    );

    let projection = db.message_projection_ids(&s.id).unwrap();
    assert_eq!(projection.len(), 3);
    let messages = db.get_messages(&s.id, None, 10).unwrap();
    let contents: Vec<&str> = messages.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(
        contents,
        MSGS.to_vec(),
        "the projection is the current conversation in order"
    );

    let member_id = db.members_for_session(&s.id).unwrap()[0].id.clone();
    assert!(
        db.get_member_stats(&member_id).unwrap().is_some(),
        "member stats were snapshotted on a genesis read"
    );
    let cursor = db.get_member_cursor(&member_id).unwrap();
    assert!(cursor.byte_offset > 0, "the read cursor advanced");
    assert!(
        !cursor.source_file_identity.is_empty(),
        "cursor identity set"
    );

    // Search indexing is part of ingestion and stays on.
    assert!(
        search::search(&db, "postgres-primary-store", 10)
            .unwrap()
            .iter()
            .any(|h| h.kind == "message" && h.parent_id == s.id),
        "search indexing stays on"
    );

    // The Context footprint is exactly zero: no AI call, no Context write.
    assert_eq!(context_footprint(&db), (0, 0, 0, 0), "zero Context writes");
    assert_eq!(count(&db, "session_context_revisions"), 0);
    assert!(
        db.get_session_context(&s.id).unwrap().is_none(),
        "no Session summary was ever generated"
    );
    assert!(
        db.items_for_workstream(&ws.id, true).unwrap().is_empty(),
        "the Owner Workstream got no items"
    );
    assert!(
        db.workstream_frontiers(&ws.id).unwrap().is_empty(),
        "no Workstream frontier was recorded"
    );
    assert_eq!(
        db.get_workstream_context_state(&ws.id)
            .unwrap()
            .context_revision,
        0,
        "the Context revision never moved"
    );
}

/// Re-scanning an unchanged source adds nothing: the member cursor already
/// covers the file, so the same content is never stored twice.
#[test]
fn rescanning_an_unchanged_source_adds_nothing() {
    let dir = unique_dir("idempotent");
    let db = open_db("idempotent");
    let file = write_transcript(&dir, 3);
    let s = session_row(&db, &file);

    assert_eq!(ingestion::ingest_session(&db, &s).unwrap(), 3);
    assert_eq!(
        ingestion::ingest_session(&db, &s).unwrap(),
        0,
        "an unchanged source is a no-op"
    );
    assert_eq!(
        ingestion::ingest_session(&db, &s).unwrap(),
        0,
        "and stays a no-op"
    );

    assert_eq!(db.message_count(&s.id).unwrap(), 3);
    assert_eq!(db.message_projection_ids(&s.id).unwrap().len(), 3);
    assert_eq!(context_footprint(&db), (0, 0, 0, 0));
}

/// A source that grew between passes contributes exactly its new delta, and
/// the fact generation still never moves a Context byte.
#[test]
fn appended_source_ingests_only_the_delta() {
    let dir = unique_dir("delta");
    let db = open_db("delta");
    let file = write_transcript(&dir, 2);
    let s = session_row(&db, &file);
    let ws = ws_row(&db, "Delta");
    db.set_session_owner(&s.id, Some(&ws.id)).unwrap();

    assert_eq!(ingestion::ingest_session(&db, &s).unwrap(), 2);
    let generation = db.get_session_ingest_state(&s.id).unwrap().generation;

    // Two offline days: the source grows by one event.
    write_transcript(&dir, 3);
    assert_eq!(
        ingestion::ingest_session(&db, &s).unwrap(),
        1,
        "only the third message is new"
    );

    assert_eq!(db.message_count(&s.id).unwrap(), 3);
    assert_eq!(db.ingested_message_sequence(&s.id).unwrap(), 3);
    assert_eq!(
        db.get_session_ingest_state(&s.id).unwrap().generation,
        generation,
        "an append extends the current generation"
    );
    assert_eq!(context_footprint(&db), (0, 0, 0, 0));
    assert_eq!(
        db.get_workstream_context_state(&ws.id)
            .unwrap()
            .context_revision,
        0
    );
}

/// A trashed Session takes nothing: ingestion refuses it, and the cursor is
/// left untouched so a Restore resumes from exactly where things stopped.
#[test]
fn trashed_session_is_never_ingested() {
    let dir = unique_dir("trash");
    let db = open_db("trash");
    let file = write_transcript(&dir, 2);
    let s = session_row(&db, &file);

    assert_eq!(ingestion::ingest_session(&db, &s).unwrap(), 2);
    let member_id = db.members_for_session(&s.id).unwrap()[0].id.clone();
    let cursor_before = db.get_member_cursor(&member_id).unwrap();

    lifecycle::trash_session(&db, &s.id).unwrap();
    write_transcript(&dir, 3);

    assert_eq!(
        ingestion::ingest_session(&db, &s).unwrap(),
        0,
        "a trashed session is not ingested"
    );
    assert_eq!(db.message_count(&s.id).unwrap(), 2, "facts stay frozen");
    assert_eq!(
        db.get_member_cursor(&member_id).unwrap().byte_offset,
        cursor_before.byte_offset,
        "the member cursor is not advanced for a trashed session"
    );
    assert_eq!(context_footprint(&db), (0, 0, 0, 0));
}

/// `refresh_session` is the per-Session incremental entry point (Resume
/// trigger). It shares `ingest_session`'s facts-only contract.
#[test]
fn refresh_session_ingests_incrementally_and_writes_no_context() {
    let dir = unique_dir("refresh");
    let db = open_db("refresh");
    let file = write_transcript(&dir, 3);
    let s = session_row(&db, &file);

    assert_eq!(ingestion::refresh_session(&db, &s.id).unwrap(), 3);
    assert_eq!(
        ingestion::refresh_session(&db, &s.id).unwrap(),
        0,
        "nothing new on an unchanged source"
    );
    assert_eq!(db.message_count(&s.id).unwrap(), 3);
    assert_eq!(context_footprint(&db), (0, 0, 0, 0));
}
