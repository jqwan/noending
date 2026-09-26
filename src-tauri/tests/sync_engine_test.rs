//! End-to-end domain test: ingest a Logical Session's conversation through
//! the production commit path, run the sync engine, and verify context items,
//! dedup and frontier advancement — plus the Context matrix: a short
//! user message reaches the extractor, an ownerless session ingests with a
//! frozen frontier, and assigning an Owner replays the pending messages.

use noending::domain::{Agent, ParsedSessionMessage, Session, SessionMessage, SessionMessageRole};
use noending::storage::{new_id, now, Db};
use noending::{context, sync};

mod support;

fn open_temp_db() -> Db {
    let dir = std::env::temp_dir().join(format!("noending-test-{}", uuid::Uuid::new_v4()));
    Db::open(&dir.join("test.db")).expect("temp db")
}

fn create_project(db: &Db, name: &str) -> noending::domain::Project {
    let p = noending::domain::Project {
        id: new_id(),
        name: name.into(),
        description: String::new(),
        git_id: None,
        name_customized: false,
        created_at: now(),
        updated_at: now(),
    };
    db.upsert_project(&p).unwrap();
    p
}

fn create_workstream(
    db: &Db,
    id: &str,
    title: &str,
    description: &str,
) -> noending::domain::Workstream {
    let w = support::workstream(id.into(), title);
    let mut w = w;
    w.description = description.into();
    db.upsert_workstream(&w).unwrap();
    w
}

/// A Logical Session + ROOT member + conversation, all through the production
/// commit path (`support::seed_conversation`).
fn make_session(
    db: &Db,
    agent: Agent,
    root_agent_session_id: &str,
    messages: &[ParsedSessionMessage],
) -> (Session, String, Vec<SessionMessage>) {
    support::seed_conversation(db, agent, root_agent_session_id, messages)
}

fn user_msg(id: &str, text: &str) -> ParsedSessionMessage {
    support::parsed_message(id, SessionMessageRole::User, text)
}

fn assistant_msg(id: &str, text: &str) -> ParsedSessionMessage {
    support::parsed_message(id, SessionMessageRole::Assistant, text)
}

fn set_owner(db: &Db, session_id: &str, workstream_id: &str) {
    db.set_session_owner(session_id, Some(workstream_id))
        .unwrap();
}

fn processed_of(db: &Db, session_id: &str) -> i64 {
    db.get_context_state(session_id)
        .unwrap()
        .processed_message_sequence
}

#[test]
fn sync_engine_extracts_and_merges() {
    let db = open_temp_db();

    let _project = create_project(&db, "Test Project");
    let ws = create_workstream(&db, "ws-sync", "Context Sync", "同步机制设计");

    let (session, _member_id, messages) = make_session(
        &db,
        Agent::Codex,
        "fake-session-1",
        &[
            user_msg(
                "m-1",
                "我们决定使用 SQLite FTS5 做全文搜索，不再引入向量数据库，决定采用这个方案。",
            ),
            assistant_msg("m-2", "好的。注意约束：不能修改现有 API，保持向后兼容。"),
            user_msg("m-3", "接下来要实现 SessionCursor 增量读取。"),
        ],
    );
    set_owner(&db, &session.id, &ws.id);

    let engine = sync::SyncEngine::default();
    let out = engine
        .run_session_sync(&db, &session, &messages, 0, 3)
        .expect("sync should succeed");

    assert!(out.applied > 0, "expected mutations applied, got {:?}", out);

    let items = db.items_for_workstream(&ws.id, true).unwrap();
    let kinds: Vec<&str> = items.iter().map(|(i, _)| i.kind.as_str()).collect();
    assert!(
        kinds.contains(&"decision"),
        "decision extracted, got {:?}",
        kinds
    );
    assert!(
        kinds.contains(&"constraint"),
        "constraint extracted, got {:?}",
        kinds
    );

    // Context frontier advanced
    assert_eq!(processed_of(&db, &session.id), 3);

    // user words keep user authority even though the extractor wrote the row
    let decision = items
        .iter()
        .find(|(i, _)| i.kind == "decision")
        .expect("decision item");
    assert_eq!(
        decision.0.authority, "user_explicit",
        "user message keeps user authority"
    );
    assert!(
        decision.0.created_by.starts_with("sync:"),
        "created_by records the extractor, got {}",
        decision.0.created_by
    );
    // the provenance trail cites the stable app-owned message ref
    assert!(
        decision
            .1
            .source_ref
            .as_deref()
            .unwrap_or_default()
            .starts_with("session-message:"),
        "source_ref is a session-message ref, got {:?}",
        decision.1.source_ref
    );

    // identical re-sync must not duplicate items (dedup rule)
    let before = items.len();
    let out2 = engine
        .run_session_sync(&db, &session, &messages, 3, 3)
        .expect("second sync");
    let after = db.items_for_workstream(&ws.id, true).unwrap().len();
    assert_eq!(before, after, "dedup: no new items on identical re-sync");
    assert_eq!(out2.applied, 0);

    // a retried delta with the SAME fingerprint is recognized as done
    let out3 = engine
        .run_session_sync(&db, &session, &messages, 0, 3)
        .expect("fingerprint retry");
    assert_eq!(out3.applied, 0, "completed run must be skipped");
    assert!(
        out3.summary.contains("幂等"),
        "retry summary explains the skip"
    );
}

#[test]
fn context_bundle_contains_core_sections() {
    let db = open_temp_db();
    let _project = create_project(&db, "Trip");
    let ws = create_workstream(&db, "ws-trip", "行程设计", "");

    sync::create_item(
        &db,
        &ws.id,
        "goal",
        "规划关西七日行程",
        "覆盖京都大阪奈良",
        "user_explicit",
        "user_edit",
        &[],
        None,
        "user",
    )
    .unwrap();
    sync::create_item(
        &db,
        &ws.id,
        "constraint",
        "预算不超过 3 万",
        "",
        "user_edit",
        "user_edit",
        &[],
        None,
        "user",
    )
    .unwrap();

    let bundle = context::build_bundle(
        &db,
        "new",
        None,
        Some(&ws.id),
        context::ContextDeliveryLevel::Balanced,
    )
    .unwrap();
    assert!(bundle.markdown.contains("Goal"));
    assert!(bundle.markdown.contains("规划关西七日行程"));
    assert!(bundle.markdown.contains("预算不超过 3 万"));
    assert!(bundle.markdown.contains("Constraints"));
}

#[test]
fn search_finds_ingested_messages() {
    let db = open_temp_db();
    let _project = create_project(&db, "Search Project");
    let ws = create_workstream(&db, "ws-search", "检索", "");

    let (session, _member_id, messages) = make_session(
        &db,
        Agent::ClaudeCode,
        "search-fixture",
        &[user_msg(
            "m-search-1",
            "我们需要为搜索功能选择 SQLite FTS5 还是外部向量数据库，这影响索引设计。",
        )],
    );
    set_owner(&db, &session.id, &ws.id);
    db.index_new_messages(&messages).unwrap();

    let hits = noending::search::search(&db, "FTS5", 10).unwrap();
    assert!(!hits.is_empty(), "FTS should find the indexed message");
    assert_eq!(hits[0].kind, "message");
}

/// The default member cursor for a fresh ROOT member is the zero state; the
/// adapter layer treats it as "first ingest". Cursors belong to the MEMBER,
/// never to the session.
#[test]
fn member_cursor_defaults_are_empties() {
    let db = open_temp_db();
    let session = support::ensure_session(&db, new_id(), Agent::Pi, "cursor-defaults");
    let member_id =
        support::ensure_root_member(&db, &session.id, Agent::Pi, "cursor-defaults", "/tmp/seed");
    let c = db.get_member_cursor(&member_id).unwrap();
    let d = noending::domain::SessionMemberCursor::default();
    assert_eq!(c.source_file_identity, d.source_file_identity);
    assert_eq!(c.byte_offset, 0);
    assert_eq!(c.generation, 0);
    assert_eq!(c.identity_tail_hash, String::new());
    // and the Context frontier starts at zero, separately
    assert_eq!(processed_of(&db, &session.id), 0);
}

/// a short user message reaches the extractor untouched: there is no
/// length pre-filter between the store and extraction; a short
/// message can be the constraint that matters.
#[test]
fn short_user_message_reaches_extractor() {
    let db = open_temp_db();
    let ws = create_workstream(&db, "ws-short", "短消息", "");

    // 18 bytes, well under any historical length heuristic — but it IS a
    // decision the user made.
    let (session, member_id, messages) = make_session(
        &db,
        Agent::Codex,
        "short-msg-root",
        &[user_msg("m-short-1", "决定用 Postgres")],
    );
    assert_eq!(messages.len(), 1);
    assert!(
        messages[0].content.len() < 30,
        "fixture must be a SHORT message"
    );
    assert_eq!(db.message_count(&session.id).unwrap(), 1);
    assert_eq!(db.ingested_message_sequence(&session.id).unwrap(), 1);
    let _ = member_id;

    set_owner(&db, &session.id, &ws.id);
    let engine = sync::SyncEngine::default();
    let out = engine
        .run_session_sync(&db, &session, &messages, 0, 1)
        .expect("sync should succeed");
    assert!(out.applied > 0, "the short decision was extracted");

    let items = db.items_for_workstream(&ws.id, true).unwrap();
    assert!(
        items.iter().any(|(i, _)| i.kind == "decision"),
        "the short decision became a decision item, got {:?}",
        items
            .iter()
            .map(|(i, _)| i.kind.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(processed_of(&db, &session.id), 1);
}

/// ownerless semantics: messages keep ingesting, the Context frontier
/// stays frozen, and a later Owner assignment replays everything pending.
#[test]
fn ownerless_frontier_frozen_then_owner_replays() {
    let db = open_temp_db();
    // The unified path reads the member's file delta first, so the ROOT
    // member needs a real (empty) source file behind its cursor.
    let dir = std::env::temp_dir().join(format!("noending-ownerless-{}", new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("source.jsonl");
    std::fs::write(&file, "").unwrap();

    let session = support::ensure_session(&db, new_id(), Agent::Codex, "ownerless-root");
    let member_id = support::ensure_root_member(
        &db,
        &session.id,
        Agent::Codex,
        "ownerless-root",
        &file.to_string_lossy(),
    );

    // two pending user decisions ingested through the production commit path
    let stored = db
        .commit_member_ingest(
            &session.id,
            &member_id,
            &[
                user_msg(
                    "ol-1",
                    "我们决定使用 PostgreSQL 作为主数据库，不再使用 SQLite 存储业务数据",
                ),
                user_msg(
                    "ol-2",
                    "补充约束：不能把密钥提交到代码仓库，必须使用环境变量",
                ),
            ],
            None,
            &support::seed_source(0),
        )
        .unwrap();
    assert_eq!(stored.len(), 2);
    assert!(session.owner_workstream_id.is_none(), "ownerless fixture");

    noending::settings::set_context_intelligence_enabled(&db, true).unwrap();
    let engine = sync::SyncEngine::default();

    // ownerless: prepare refuses, the frontier stays frozen
    let pre = engine.prepare(&db, &session, &stored, 0, 2).unwrap();
    assert!(pre.is_none(), "an ownerless session is never prepared");
    let (ingested, applied) =
        noending::ingestion::ingest_and_sync_session(&db, &engine, &session).unwrap();
    assert_eq!(applied, 0, "nothing was extracted for an ownerless session");
    assert_eq!(processed_of(&db, &session.id), 0, "frontier frozen");
    assert_eq!(db.items_for_workstream("ws-none", true).unwrap().len(), 0);
    // but the messages DID ingest (and stay ingested)
    assert_eq!(db.message_count(&session.id).unwrap(), 2);
    let _ = ingested;

    // assign an Owner: the pending messages replay from the frozen frontier
    let ws = create_workstream(&db, "ws-ownerless", "接管的任务", "");
    set_owner(&db, &session.id, &ws.id);
    let (_ingested, applied) =
        noending::ingestion::ingest_and_sync_session(&db, &engine, &session).unwrap();
    assert!(
        applied > 0,
        "assigning an owner replays the pending messages"
    );
    assert_eq!(processed_of(&db, &session.id), 2, "frontier caught up");
    assert!(
        !db.items_for_workstream(&ws.id, true).unwrap().is_empty(),
        "the replayed delta became context"
    );
}
