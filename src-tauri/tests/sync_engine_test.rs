//! End-to-end domain test: ingest a fake session, run the sync engine,
//! and verify context items, dedup and cursor advancement.

use noending::domain::{Agent, Session, SessionEvent};
use noending::storage::{now, new_id, Db};
use noending::{context, sync};

fn open_temp_db() -> Db {
    let dir = std::env::temp_dir().join(format!("noending-test-{}", uuid::Uuid::new_v4()));
    Db::open(&dir.join("test.db")).expect("temp db")
}

fn create_project(db: &Db, name: &str) -> noending::domain::Project {
    let p = noending::domain::Project {
        id: new_id(),
        name: name.into(),
        description: String::new(),
        archived: false,
        created_at: now(),
        updated_at: now(),
    };
    db.upsert_project(&p).unwrap();
    p
}

fn create_workstream(db: &Db, project_id: &str, title: &str, description: &str) -> noending::domain::Workstream {
    let w = noending::domain::Workstream {
        id: new_id(),
        project_id: Some(project_id.into()),
        title: title.into(),
        description: description.into(),
        lifecycle: "open".into(),
        visibility: "normal".into(),
        created_at: now(),
        updated_at: now(),
    };
    db.upsert_workstream(&w).unwrap();
    w
}

fn make_session(db: &Db, agent: Agent, agent_session_id: &str) -> Session {
    let s = Session {
        id: new_id(),
        agent,
        agent_session_id: agent_session_id.into(),
        title: None,
        cwd: None,
        project_id: None,
        raw_path: "/tmp/fake.jsonl".into(),
        parent_agent_session_id: None,
        started_at: Some(now()),
        last_activity_at: Some(now()),
    };
    db.upsert_session(&s).unwrap();
    s
}

fn bind(db: &Db, session_id: &str, workstream_id: &str) {
    db.bind(&noending::domain::SessionWorkstreamBinding {
        session_id: session_id.into(),
        workstream_id: workstream_id.into(),
        role: "primary".into(),
        last_seen_revision: None,
        last_sync_cursor: 0,
        created_at: now(),
        last_used_at: now(),
    })
    .unwrap();
}

fn event(session: &Session, sequence: i64, kind: &str, text: &str) -> SessionEvent {
    SessionEvent {
        session_id: session.id.clone(),
        sequence,
        ts: Some(now()),
        kind: kind.into(),
        text: Some(text.into()),
        raw_ref: format!("test#{}", sequence),
        metadata: serde_json::json!({}),
    }
}

#[test]
fn sync_engine_extracts_and_merges() {
    let db = open_temp_db();

    let project = create_project(&db, "Test Project");
    let ws = create_workstream(&db, &project.id, "Context Sync", "同步机制设计");

    let session = make_session(&db, Agent::Codex, "fake-session-1");
    bind(&db, &session.id, &ws.id);

    let engine = sync::SyncEngine::default();
    let events = vec![
        event(&session, 1, "user_message", "我们决定使用 SQLite FTS5 做全文搜索，不再引入向量数据库，决定采用这个方案。"),
        event(&session, 2, "assistant_message", "好的。注意约束：不能修改现有 API，保持向后兼容。"),
        event(&session, 3, "user_message", "接下来要实现 SessionCursor 增量读取。"),
    ];

    let out = engine
        .run_session_sync(&db, &session, &events, 0, 3)
        .expect("sync should succeed");

    assert!(out.applied > 0, "expected mutations applied, got {:?}", out);

    let items = db.items_for_workstream(&ws.id, true).unwrap();
    let kinds: Vec<&str> = items.iter().map(|(i, _)| i.kind.as_str()).collect();
    assert!(kinds.contains(&"decision"), "decision extracted, got {:?}", kinds);
    assert!(kinds.contains(&"constraint"), "constraint extracted, got {:?}", kinds);

    // cursor advanced
    assert_eq!(db.get_cursor(&session.id).unwrap(), 3);

    // identical re-sync must not duplicate items (dedup rule)
    let before = items.len();
    let out2 = engine
        .run_session_sync(&db, &session, &events, 3, 3)
        .expect("second sync");
    assert_eq!(out2.applied, 0);
    let after = db.items_for_workstream(&ws.id, true).unwrap().len();
    assert_eq!(before, after, "dedup: no new items on identical re-sync");
}

#[test]
fn context_bundle_contains_core_sections() {
    let db = open_temp_db();
    let project = create_project(&db, "Trip");
    let ws = create_workstream(&db, &project.id, "行程设计", "");

    sync::create_item(&db, &ws.id, "goal", "规划关西七日行程", "覆盖京都大阪奈良", "user_explicit", "user_edit", None, None).unwrap();
    sync::create_item(&db, &ws.id, "constraint", "预算不超过 3 万", "", "user_edit", "user_edit", None, None).unwrap();

    let bundle = context::build_bundle(&db, "new", None, &[ws.id.clone()], 4000).unwrap();
    assert!(bundle.markdown.contains("Goal"));
    assert!(bundle.markdown.contains("规划关西七日行程"));
    assert!(bundle.markdown.contains("预算不超过 3 万"));
    assert!(bundle.markdown.contains("Constraints"));
}

#[test]
fn search_finds_ingested_events() {
    let db = open_temp_db();
    let project = create_project(&db, "Search Project");
    let ws = create_workstream(&db, &project.id, "检索", "");

    let session = make_session(&db, Agent::ClaudeCode, "search-fixture");
    bind(&db, &session.id, &ws.id);

    let events = vec![event(
        &session,
        1,
        "user_message",
        "我们需要为搜索功能选择 SQLite FTS5 还是外部向量数据库，这影响索引设计。",
    )];
    db.append_events(&events).unwrap();
    db.index_events(&events).unwrap();

    let hits = noending::search::search(&db, "FTS5", 10).unwrap();
    assert!(!hits.is_empty(), "FTS should find the indexed event");
    assert_eq!(hits[0].kind, "event");
}
