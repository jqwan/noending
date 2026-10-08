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

/// A Logical Session whose root source is the transcript at `path`.
/// Ingestion reads the session's `source_path`, never a session-level raw_path.
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
    support::ensure_session_source(
        db,
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
    let (session, _) = support::seed_conversation(
        &db,
        Agent::ClaudeCode,
        "empty-generation-context",
        &[support::parsed_message(
            "old-message",
            SessionMessageRole::User,
            "old transcript",
        )],
    );
    let initial_generation = db.get_session_ingest_state(&session.id).unwrap().0;
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

    db.commit_ingest(
        &session.id,
        &[],
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
    let home_dir = unique_dir("context-empty-rewrite-home");
    let home =
        noending::workspace::home::NoEndingHome::new(home_dir.to_str().unwrap(), None).unwrap();
    let outcome = noending::context::update_session(&db, &session.id, Some(&home)).unwrap();
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
    assert_eq!(state.1, 3);
    assert_eq!(
        state.0, 1,
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

    let cursor = db.get_session(&s.id).unwrap().unwrap().source_cursor();
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

/// Two logical sessions whose transcripts share the same message ids — a fork
/// or a resumed copy of a conversation — are independent conversation records:
/// each ingests its own root prose (real-world shape: session ead8405f shared
/// both message ids with the big session it was forked from).
#[test]
fn sessions_sharing_message_ids_ingest_independently() {
    let dir = unique_dir("claim-scope");
    let db = open_db("claim-scope");

    let usage_row = |uuid: &str| {
        format!(
            r#"{{"type":"assistant","uuid":"{uuid}","timestamp":"2026-09-19T10:00:00Z","message":{{"id":"msg_shared","role":"assistant","content":[{{"type":"text","text":"同一个回答"}}]}}}}"#
        )
    };
    let file_a = dir.join("a.jsonl");
    let file_b = dir.join("b.jsonl");
    std::fs::write(&file_a, format!("{}\n", usage_row("u-a"))).unwrap();
    std::fs::write(&file_b, format!("{}\n", usage_row("u-b"))).unwrap();

    let s_a = session_row(&db, &file_a);
    let s_b = session_row(&db, &file_b);

    ingestion::ingest_session(&db, &s_a).unwrap();
    ingestion::ingest_session(&db, &s_b).unwrap();

    for s in [&s_a, &s_b] {
        assert_eq!(
            db.message_count(&s.id).unwrap(),
            1,
            "each record keeps its own conversation"
        );
    }
}

/// Re-scanning an unchanged source adds nothing: the session cursor already
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
    let generation = db.get_session_ingest_state(&s.id).unwrap().0;

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
        db.get_session_ingest_state(&s.id).unwrap().0,
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

/// Trash is a visibility filter, not an ingestion gate: a trashed Session keeps
/// taking its source's facts.
#[test]
fn trashed_session_still_ingests_its_source() {
    let dir = unique_dir("trash");
    let db = open_db("trash");
    let file = write_transcript(&dir, 2);
    let s = session_row(&db, &file);

    assert_eq!(ingestion::ingest_session(&db, &s).unwrap(), 2);
    let cursor_before = db.get_session(&s.id).unwrap().unwrap().source_cursor();

    lifecycle::archive_session(&db, &s.id).unwrap();
    write_transcript(&dir, 3);

    assert_eq!(
        ingestion::ingest_session(&db, &s).unwrap(),
        1,
        "a trashed session keeps ingesting its source"
    );
    assert_eq!(db.message_count(&s.id).unwrap(), 3);
    assert!(
        db.get_session(&s.id)
            .unwrap()
            .unwrap()
            .source_cursor()
            .byte_offset
            > cursor_before.byte_offset,
        "the session cursor advances exactly as it would for an active session"
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

/// The Conversation reader: a preview is the TAIL of the conversation, and the
/// reader pages BACKWARD through the cursor each page returns, without skipping
/// or repeating a message.
#[test]
fn the_conversation_reads_backward_from_the_newest_message() {
    let db = open_db("message-window");
    let (session, _) = support::seed_conversation(
        &db,
        Agent::ClaudeCode,
        "message-window",
        &(1..=5)
            .map(|i| {
                support::parsed_message(
                    format!("m{i}"),
                    if i % 2 == 1 {
                        SessionMessageRole::User
                    } else {
                        SessionMessageRole::Assistant
                    },
                    format!("message {i}"),
                )
            })
            .collect::<Vec<_>>(),
    );

    let recent: Vec<String> = db
        .recent_messages(&session.id, 2)
        .unwrap()
        .into_iter()
        .map(|m| m.content)
        .collect();
    assert_eq!(
        recent,
        vec!["message 4", "message 5"],
        "the detail preview is the newest messages, oldest first"
    );

    let newest = db.message_window(&session.id, None, 2).unwrap();
    assert_eq!(newest.total, 5);
    assert_eq!(newest.messages.len(), 2);
    assert_eq!(newest.messages[1].message.content, "message 5");
    assert_eq!(
        newest
            .messages
            .iter()
            .map(|m| m.ordinal)
            .collect::<Vec<_>>(),
        vec![4, 5],
        "each message carries the ordinal that orders the conversation"
    );
    assert_eq!(
        newest.next_before_ordinal,
        Some(3),
        "the cursor sits just before the page it came with"
    );

    let older = db
        .message_window(&session.id, newest.next_before_ordinal, 2)
        .unwrap();
    assert_eq!(
        older
            .messages
            .iter()
            .map(|m| m.message.content.as_str())
            .collect::<Vec<_>>(),
        vec!["message 2", "message 3"]
    );
    assert_eq!(older.next_before_ordinal, Some(1));
    assert_eq!(older.total, 5, "the total is the whole conversation");
    assert_eq!(
        older.generation, newest.generation,
        "paging inside one conversation never changes the generation"
    );

    let oldest = db
        .message_window(&session.id, older.next_before_ordinal, 2)
        .unwrap();
    assert_eq!(oldest.messages.len(), 1);
    assert_eq!(oldest.messages[0].message.content, "message 1");
    assert_eq!(oldest.messages[0].ordinal, 1);
    assert_eq!(
        oldest.next_before_ordinal, None,
        "the beginning was reached"
    );

    let whole = db.message_window(&session.id, None, 50).unwrap();
    assert_eq!(
        whole.messages.len(),
        5,
        "a page past the tail is the whole conversation"
    );
    assert_eq!(whole.next_before_ordinal, None);
}

/// The reader pages over the skeleton; each final reply carries its turn's
/// collapsible intermediates, and the range fetch returns exactly those.
#[test]
fn a_final_reply_carries_its_turn_and_the_range_fetch_returns_its_intermediates() {
    let db = open_db("turn-blocks");
    let mut first_ask = support::parsed_message("m1", SessionMessageRole::User, "第一问");
    first_ask.ts = Some("2026-10-05T10:00:00+08:00".into());
    let (session, _) = support::seed_conversation(
        &db,
        Agent::ClaudeCode,
        "turn-blocks",
        &[
            first_ask,
            support::parsed_message("m2", SessionMessageRole::Assistant, "中间一"),
            support::parsed_message("m3", SessionMessageRole::Assistant, "中间二"),
            support::parsed_message("m4", SessionMessageRole::Assistant, "最终回答"),
            support::parsed_message("m5", SessionMessageRole::User, "第二问"),
            support::parsed_message("m6", SessionMessageRole::Assistant, "直接的回答"),
        ],
    );

    let window = db.message_window(&session.id, None, 50).unwrap();
    let by_ordinal = |ordinal: i64| {
        window
            .messages
            .iter()
            .find(|m| m.ordinal == ordinal)
            .unwrap_or_else(|| panic!("ordinal {ordinal} not in the page"))
    };
    // 骨架页：两条用户消息 + 两条最终回复；中间回复不占页，total 仍是全部消息。
    assert_eq!(
        window
            .messages
            .iter()
            .map(|m| m.ordinal)
            .collect::<Vec<_>>(),
        vec![1, 4, 5, 6]
    );
    assert_eq!(window.total, 6);
    // ordinal 4 的最终回复带着它那轮：起点是 ordinal 1 的提问，中间两条。
    let turn = by_ordinal(4).turn.as_ref().unwrap();
    assert_eq!(turn.boundary_ordinal, 1);
    assert_eq!(
        turn.boundary_ts.as_deref(),
        Some("2026-10-05T10:00:00+08:00")
    );
    assert_eq!(turn.count, 2);
    // ordinal 6 那轮没有中间回复：没有块。用户消息也不带块。
    assert!(by_ordinal(6).turn.is_none());
    assert!(by_ordinal(1).turn.is_none());

    // 展开取数：正好是起点与最终回复之间的那两条，旧→新；截断如实上报。
    let (intermediates, truncated) = db
        .turn_intermediates(&session.id, turn.boundary_ordinal, 4, 50)
        .unwrap();
    assert_eq!(
        intermediates
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>(),
        vec!["中间一", "中间二"]
    );
    assert!(!truncated);
    let (capped, truncated) = db.turn_intermediates(&session.id, 1, 4, 1).unwrap();
    assert_eq!(
        capped
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>(),
        vec!["中间一"]
    );
    assert!(truncated);
}

/// The navigation rail's marks are the USER messages in conversation order,
/// each with its first line as the tooltip; forward paging continues exactly
/// where a backward page stopped.
#[test]
fn conversation_marks_point_at_user_messages_and_paging_runs_both_ways() {
    let db = open_db("message-marks");
    let long_line = "很长的一行".repeat(40);
    let bodies = [
        long_line,
        "回答一".to_string(),
        "message 3".to_string(),
        "回答二".to_string(),
        "第一行\n第二行".to_string(),
        "回答三".to_string(),
    ];
    let (session, _) = support::seed_conversation(
        &db,
        Agent::ClaudeCode,
        "message-marks",
        &bodies
            .iter()
            .enumerate()
            .map(|(i, body)| {
                support::parsed_message(
                    format!("m{}", i + 1),
                    if i % 2 == 0 {
                        SessionMessageRole::User
                    } else {
                        SessionMessageRole::Assistant
                    },
                    body.clone(),
                )
            })
            .collect::<Vec<_>>(),
    );

    let marks = db.user_message_marks(&session.id).unwrap();
    assert_eq!(
        marks.iter().map(|m| m.ordinal).collect::<Vec<_>>(),
        vec![1, 3, 5],
        "only USER messages, in order"
    );
    assert!(
        marks[0].preview.ends_with('…'),
        "a long first line is truncated for the tooltip"
    );
    assert_eq!(marks[1].preview, "message 3");
    assert_eq!(marks[2].preview, "第一行", "the preview is the first line");

    // 两向分页在同一条缝上对接：向后那页的第一条，正是向前那页的下一条。
    let older = db.message_window(&session.id, Some(4), 2).unwrap();
    assert_eq!(
        older.messages.iter().map(|m| m.ordinal).collect::<Vec<_>>(),
        vec![3, 4]
    );
    let newer = db.newer_window(&session.id, 4, 2).unwrap();
    assert_eq!(
        newer.messages.iter().map(|m| m.ordinal).collect::<Vec<_>>(),
        vec![5, 6],
        "forward paging resumes right after the backward page"
    );
    assert_eq!(newer.total, 6);
    assert_eq!(
        newer.messages.last().unwrap().ordinal,
        newer.total,
        "the forward page reached the tail"
    );
    assert_eq!(
        newer.next_before_ordinal,
        Some(4),
        "a forward page still knows where to page back from"
    );

    let past_the_tail = db.newer_window(&session.id, 6, 2).unwrap();
    assert!(
        past_the_tail.messages.is_empty(),
        "nothing newer than the tail"
    );
}

/// A rewrite replaces the conversation and raises the generation the page
/// carries — how a reader paging upward learns its older pages went stale.
#[test]
fn a_replaced_conversation_reports_a_new_generation_to_paging_readers() {
    let db = open_db("message-window-rewrite");
    let (session, _) = support::seed_conversation(
        &db,
        Agent::ClaudeCode,
        "message-window-rewrite",
        &(1..=3)
            .map(|i| {
                support::parsed_message(
                    format!("m{i}"),
                    SessionMessageRole::User,
                    format!("message {i}"),
                )
            })
            .collect::<Vec<_>>(),
    );
    let before = db.message_window(&session.id, None, 2).unwrap();
    assert_eq!(before.next_before_ordinal, Some(1));

    db.commit_ingest(&session.id, &[], &support::seed_source(1))
        .unwrap();

    let after = db
        .message_window(&session.id, before.next_before_ordinal, 2)
        .unwrap();
    assert_eq!(after.total, 0, "the replacement conversation is empty");
    assert!(after.messages.is_empty());
    assert_eq!(after.next_before_ordinal, None);
    assert_ne!(
        after.generation, before.generation,
        "a replaced conversation raises the generation"
    );
}
