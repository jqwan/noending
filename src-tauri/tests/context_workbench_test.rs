use noending::domain::{
    Agent, ContextConflict, ContextItem, ContextItemRevision, Session, SessionEvent,
};
use noending::storage::{new_id, now, Db};

fn open_db(tag: &str) -> Db {
    let dir = std::env::temp_dir().join(format!("noending-workbench-{}-{}", tag, new_id()));
    Db::open(&dir.join("test.db")).unwrap()
}

fn ws_row(db: &Db, title: &str) -> noending::domain::Workstream {
    let w = noending::domain::Workstream {
        id: new_id(),
        project_id: None,
        title: title.into(),
        description: String::new(),
        lifecycle: "open".into(),
        visibility: "normal".into(),
        default_cwd: None,
        created_at: now(),
        updated_at: now(),
    };
    db.upsert_workstream(&w).unwrap();
    w
}

fn session_row(db: &Db, agent: Agent) -> Session {
    let raw = std::env::temp_dir().join(format!("noending-raw-{}.jsonl", new_id()));
    let _ = std::fs::write(&raw, "");
    let s = Session {
        id: new_id(),
        agent,
        agent_session_id: format!("as-{}", new_id()),
        title: Some("Implement Context Delivery".into()),
        cwd: None,
        project_id: None,
        raw_path: raw.to_string_lossy().to_string(),
        parent_agent_session_id: None,
        started_at: Some(now()),
        last_activity_at: Some(now()),
    };
    db.upsert_session(&s).unwrap();
    s
}

#[test]
fn workstream_context_core_matches_resolve_core_context() {
    let db = open_db("core-match");
    let ws = ws_row(&db, "Core Match WS");

    noending::sync::create_item(
        &db,
        &ws.id,
        "goal",
        "Ship Workbench v0.1",
        "Full control over context facts",
        "user_explicit",
        "user_edit",
        &[],
        None,
        "user",
    )
    .unwrap();

    noending::sync::create_item(
        &db,
        &ws.id,
        "current_state",
        "Backend read model completed",
        "Working on tests",
        "user_explicit",
        "user_edit",
        &[],
        None,
        "user",
    )
    .unwrap();

    noending::sync::create_item(
        &db,
        &ws.id,
        "todo",
        "Write integration tests",
        "Verify read model",
        "user_explicit",
        "user_edit",
        &[],
        None,
        "user",
    )
    .unwrap();

    let core_resolved = noending::context::resolve_core_context(&db, &ws.id).unwrap();
    assert_eq!(core_resolved.len(), 2);
    assert_eq!(core_resolved[0].kind, "goal");
    assert_eq!(core_resolved[1].kind, "current_state");
    assert!(core_resolved[0].revision_id.is_some());
    assert_eq!(
        core_resolved[0].workstream_id.as_deref(),
        Some(ws.id.as_str())
    );
}

#[test]
fn item_relations_supersede_chain() {
    let db = open_db("relations");
    let ws = ws_row(&db, "Relations WS");

    let item1 = noending::sync::create_item(
        &db,
        &ws.id,
        "decision",
        "Use PostgreSQL",
        "Heavy RDBMS",
        "user_explicit",
        "user_edit",
        &[],
        None,
        "user",
    )
    .unwrap();

    let item2_id = new_id();
    let rev2_id = new_id();
    let item2 = ContextItem {
        id: item2_id.clone(),
        workstream_id: ws.id.clone(),
        kind: "decision".into(),
        status: "active".into(),
        authority: "user_explicit".into(),
        created_by: "user".into(),
        current_revision_id: Some(rev2_id.clone()),
        supersedes_item_id: Some(item1.id.clone()),
        created_at: now(),
        updated_at: now(),
    };
    let rev2 = ContextItemRevision {
        id: rev2_id,
        item_id: item2_id.clone(),
        title: "Use SQLite for local-first".into(),
        content: "Embedded DB".into(),
        metadata: serde_json::json!({}),
        source_type: Some("user_edit".into()),
        source_ref: None,
        sync_run_id: None,
        created_at: now(),
    };
    db.insert_item(&item2, &rev2).unwrap();

    db.apply_status_change(
        &item1.id,
        "superseded",
        "user",
        "Superseded by SQLite",
        None,
        &[],
    )
    .unwrap();

    let relations = db.item_relations_for_workstream(&ws.id).unwrap();
    assert_eq!(relations.len(), 2);

    let rel1 = relations.iter().find(|r| r.item_id == item1.id).unwrap();
    assert!(rel1.supersedes.is_none());
    assert_eq!(rel1.superseded_by.len(), 1);
    assert_eq!(rel1.superseded_by[0].id, item2.id);
    assert_eq!(rel1.superseded_by[0].title, "Use SQLite for local-first");

    let rel2 = relations.iter().find(|r| r.item_id == item2.id).unwrap();
    assert!(rel2.supersedes.is_some());
    assert_eq!(rel2.supersedes.as_ref().unwrap().id, item1.id);
    assert_eq!(rel2.supersedes.as_ref().unwrap().title, "Use PostgreSQL");
    assert_eq!(rel2.superseded_by.len(), 0);
}

#[test]
fn get_context_revision_source_resolution() {
    let db = open_db("rev-source");
    let ws = ws_row(&db, "Source WS");
    let session = session_row(&db, Agent::Codex);

    let event_id = new_id();
    let event = SessionEvent {
        id: event_id.clone(),
        session_id: session.id.clone(),
        sequence: 42,
        source_event_id: Some("codex-ev-42".into()),
        source_generation: 0,
        source_position: "42".into(),
        ts: Some("2026-09-17T20:14:00Z".into()),
        kind: "agent_message".into(),
        text: Some("Evidence: Migration implementation completed.".into()),
        raw_ref: "raw".into(),
        metadata: serde_json::json!({}),
    };
    db.append_events(&[event]).unwrap();

    let sref = format!("session-event:{}", event_id);
    let item = noending::sync::create_item(
        &db,
        &ws.id,
        "current_state",
        "API migration done",
        "Windows verification remaining",
        "agent_statement",
        "session_event",
        &[sref.clone()],
        Some("sync-run-123"),
        "sync:heuristic",
    )
    .unwrap();

    let rev_id = item.current_revision_id.unwrap();
    let detail = db
        .get_context_revision_source(&rev_id)
        .unwrap()
        .expect("source detail found");

    assert_eq!(detail.revision_id, rev_id);
    assert_eq!(detail.authority, "agent_statement");
    assert_eq!(detail.source_type.as_deref(), Some("session_event"));
    assert_eq!(detail.source_ref.as_deref(), Some(sref.as_str()));
    assert_eq!(detail.sync_run_id.as_deref(), Some("sync-run-123"));
    assert_eq!(detail.session_id.as_deref(), Some(session.id.as_str()));
    assert_eq!(
        detail.session_title.as_deref(),
        Some("Implement Context Delivery")
    );
    assert_eq!(detail.agent, Some(Agent::Codex));
    assert_eq!(detail.event_sequence, Some(42));
    assert_eq!(detail.event_ts.as_deref(), Some("2026-09-17T20:14:00Z"));
    assert_eq!(
        detail.evidence.as_deref(),
        Some("Evidence: Migration implementation completed.")
    );
}

#[test]
fn list_workstream_context_changes_timeline() {
    let db = open_db("timeline");
    let ws = ws_row(&db, "Timeline WS");

    let item = noending::sync::create_item(
        &db,
        &ws.id,
        "current_state",
        "Initial State",
        "First draft",
        "user_explicit",
        "user_edit",
        &[],
        None,
        "user",
    )
    .unwrap();

    let rev2 = ContextItemRevision {
        id: new_id(),
        item_id: item.id.clone(),
        title: "Initial State".into(),
        content: "Second draft edited by user".into(),
        metadata: serde_json::json!({}),
        source_type: Some("user_edit".into()),
        source_ref: None,
        sync_run_id: None,
        created_at: now(),
    };
    db.insert_revision(&rev2).unwrap();
    db.set_item_head(&item.id, &rev2.id, None).unwrap();

    db.apply_status_change(&item.id, "resolved", "user", "Work done", None, &[])
        .unwrap();

    let conflict = ContextConflict {
        id: new_id(),
        workstream_id: ws.id.clone(),
        left_item_id: item.id.clone(),
        right_item_id: None,
        conflict_type: "authority".into(),
        status: "open".into(),
        resolution: None,
        created_at: now(),
        updated_at: now(),
    };
    db.insert_conflict(&conflict).unwrap();

    db.update_conflict_status(&conflict.id, "resolved", Some("User confirmed left item"))
        .unwrap();

    let changes = db.list_workstream_context_changes(&ws.id, 20).unwrap();
    assert!(changes.len() >= 4);

    let kinds: Vec<&str> = changes.iter().map(|c| c.kind.as_str()).collect();
    assert!(kinds.contains(&"conflict_resolved"));
    assert!(kinds.contains(&"conflict_created"));
    assert!(kinds.contains(&"resolved"));
    assert!(kinds.contains(&"edited"));
    assert!(kinds.contains(&"added"));
}
