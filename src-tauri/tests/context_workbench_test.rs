use noending::domain::{
    Agent, ContextConflict, ContextItem, ContextItemEditPayload, ContextItemRevision, Session,
    SessionEvent,
};
use noending::storage::{new_id, now, Db};
use rusqlite::params;

fn open_db(tag: &str) -> Db {
    let dir = std::env::temp_dir().join(format!("noending-workbench-{}-{}", tag, new_id()));
    Db::open(&dir.join("test.db")).unwrap()
}

fn ws_row(db: &Db, title: &str) -> noending::domain::Workstream {
    let w = noending::domain::Workstream {
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

fn session_row(db: &Db, agent: Agent) -> Session {
    let raw = std::env::temp_dir().join(format!("noending-raw-{}.jsonl", new_id()));
    let _ = std::fs::write(&raw, "");
    let s = Session {
        id: new_id(),
        agent,
        agent_session_id: format!("as-{}", new_id()),
        title: Some("Implement Context Delivery".into()),
        cwd: None,
        workspace_path_id: None,
        project_id: None,
        raw_path: raw.to_string_lossy().to_string(),
        parent_agent_session_id: None,
        started_at: Some(now()),
        last_activity_at: Some(now()),
        trashed_at: None,
        owner_workstream_id: None,
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
        left_revision_id: None,
        right_revision_id: None,
        candidate_snapshot_json: None,
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

#[test]
fn conflict_audited_resolution_and_history() {
    let db = open_db("conflict-audit");
    let ws = ws_row(&db, "Conflict Audit WS");

    let item = noending::sync::create_item(
        &db,
        &ws.id,
        "current_state",
        "Architecture Draft",
        "Monolith vs Microservices",
        "user_explicit",
        "user_edit",
        &[],
        None,
        "user",
    )
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
        left_revision_id: None,
        right_revision_id: None,
        candidate_snapshot_json: None,
    };
    db.insert_conflict(&conflict).unwrap();

    // 1. Resolve with reason
    db.resolve_conflict_audited(
        &conflict.id,
        "resolved",
        Some("Confirmed modular monolith"),
        "user",
    )
    .unwrap();

    let c1 = db.get_conflict(&conflict.id).unwrap().unwrap();
    assert_eq!(c1.status, "resolved");
    assert_eq!(c1.resolution.as_deref(), Some("Confirmed modular monolith"));

    let history = db.conflict_history(&conflict.id).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].previous_status, "open");
    assert_eq!(history[0].new_status, "resolved");
    assert_eq!(history[0].actor, "user");
    assert_eq!(
        history[0].resolution.as_deref(),
        Some("Confirmed modular monolith")
    );

    // 2. Dismiss/re-decide
    db.resolve_conflict_audited(
        &conflict.id,
        "dismissed",
        Some("No longer relevant"),
        "user",
    )
    .unwrap();

    let c2 = db.get_conflict(&conflict.id).unwrap().unwrap();
    assert_eq!(c2.status, "dismissed");
    assert_eq!(c2.resolution.as_deref(), Some("No longer relevant"));

    let history2 = db.conflict_history(&conflict.id).unwrap();
    assert_eq!(history2.len(), 2);
    assert_eq!(history2[1].previous_status, "resolved");
    assert_eq!(history2[1].new_status, "dismissed");
}

#[test]
fn test_historical_provenance_freeze() {
    let db = open_db("provenance-freeze");
    let ws = ws_row(&db, "Provenance Freeze WS");

    // 1. Agent creates an item with agent_statement authority
    let item = noending::sync::create_item(
        &db,
        &ws.id,
        "decision",
        "Architecture Style",
        "Use SQLite for local persistence",
        "agent_statement",
        "session_event",
        &["session-event:100".into()],
        None,
        "agent",
    )
    .unwrap();

    let rev1_id = item.current_revision_id.clone().unwrap();

    // Verify revision 1 source before any edits
    let src1_before = db.get_context_revision_source(&rev1_id).unwrap().unwrap();
    assert_eq!(src1_before.authority, "agent_statement");
    assert_eq!(src1_before.source_type.as_deref(), Some("session_event"));

    // 2. User edits the item, elevating item authority to user_edit
    let rev2 = ContextItemRevision {
        id: new_id(),
        item_id: item.id.clone(),
        title: "Architecture Style Updated".into(),
        content: "Use SQLite with WAL mode".into(),
        metadata: serde_json::json!({
            "provenance": {
                "authority": "user_edit",
                "actor": "user",
                "source_type": "user_edit",
                "source_ref": serde_json::Value::Null,
            }
        }),
        source_type: Some("user_edit".into()),
        source_ref: None,
        sync_run_id: None,
        created_at: now(),
    };
    db.insert_revision(&rev2).unwrap();
    db.set_item_head(&item.id, &rev2.id, None).unwrap();
    db.set_item_authority(&item.id, "user_edit").unwrap();

    // Verify current item authority is user_edit
    let updated_item = db.get_item(&item.id).unwrap().unwrap();
    assert_eq!(updated_item.authority, "user_edit");

    // 3. Invariant check: Inspecting historical rev1 still returns agent_statement!
    let src1_after = db.get_context_revision_source(&rev1_id).unwrap().unwrap();
    assert_eq!(
        src1_after.authority, "agent_statement",
        "Historical revision authority must be frozen and not overwritten by subsequent user edit"
    );

    // Inspecting rev2 returns user_edit
    let src2 = db.get_context_revision_source(&rev2.id).unwrap().unwrap();
    assert_eq!(src2.authority, "user_edit");

    // Activity timeline correctly reports actors
    let changes = db.list_workstream_context_changes(&ws.id, 10).unwrap();
    assert_eq!(changes.len(), 2);
    // Most recent first:
    assert_eq!(changes[0].actor, "User");
    assert_eq!(changes[1].actor, "Agent");
}

#[test]
fn test_conflict_snapshots_freeze() {
    let db = open_db("conflict-snapshot-freeze");
    let ws = ws_row(&db, "Conflict Snapshot Freeze WS");

    let item = noending::sync::create_item(
        &db,
        &ws.id,
        "goal",
        "Deploy to Cloud",
        "Initial proposal",
        "agent_statement",
        "session_event",
        &[],
        None,
        "agent",
    )
    .unwrap();
    let rev1_id = item.current_revision_id.clone().unwrap();

    // Conflict created referencing rev1 and candidate snapshot
    let candidate_val = serde_json::json!({
        "title": "Keep on premise",
        "content": "Privacy requirement",
        "authority": "user_explicit",
    });

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
        left_revision_id: Some(rev1_id.clone()),
        right_revision_id: None,
        candidate_snapshot_json: Some(candidate_val.to_string()),
    };
    db.insert_conflict(&conflict).unwrap();

    // Item evolves to rev2
    let rev2 = ContextItemRevision {
        id: new_id(),
        item_id: item.id.clone(),
        title: "Deploy to Hybrid Cloud".into(),
        content: "Hybrid model".into(),
        metadata: serde_json::json!({
            "provenance": {
                "authority": "agent_statement",
                "actor": "agent",
                "source_type": "session_event",
                "source_ref": serde_json::Value::Null,
            }
        }),
        source_type: Some("session_event".into()),
        source_ref: None,
        sync_run_id: None,
        created_at: now(),
    };
    db.insert_revision(&rev2).unwrap();
    db.set_item_head(&item.id, &rev2.id, None).unwrap();

    // Query conflict: left_revision_id is still rev1!
    let fetched = db.get_conflict(&conflict.id).unwrap().unwrap();
    assert_eq!(fetched.left_revision_id.as_deref(), Some(rev1_id.as_str()));
    assert_eq!(
        fetched.candidate_snapshot_json.as_deref(),
        Some(candidate_val.to_string().as_str())
    );

    // Resolve conflict audited
    db.resolve_conflict_audited(
        &conflict.id,
        "resolved",
        Some("Resolved in favor of privacy"),
        "user",
    )
    .unwrap();

    // Check conflict history audit event snapshot
    let history = db.conflict_history(&conflict.id).unwrap();
    assert_eq!(history.len(), 1);
    assert!(history[0].snapshot_json.is_some());

    let snapshot: serde_json::Value =
        serde_json::from_str(history[0].snapshot_json.as_ref().unwrap()).unwrap();
    assert_eq!(snapshot["left_revision_id"], rev1_id);
    assert_eq!(snapshot["candidate_snapshot"]["title"], "Keep on premise");
}

#[test]
fn test_resolve_conflict_with_edit_atomic_transaction() {
    let db = open_db("conflict-atomic-tx");
    let ws = ws_row(&db, "Atomic Conflict WS");

    // 1. Setup item and conflict
    let item = noending::sync::create_item(
        &db,
        &ws.id,
        "constraint",
        "Max Memory",
        "Max memory is 2GB",
        "agent_statement",
        "session_event",
        &["session-event:1".into()],
        None,
        "agent",
    )
    .unwrap();
    let rev1_id = item.current_revision_id.clone().unwrap();

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
        left_revision_id: Some(rev1_id.clone()),
        right_revision_id: None,
        candidate_snapshot_json: Some(
            serde_json::json!({
                "title": "Max Memory 4GB",
                "content": "Requested 4GB for cache",
            })
            .to_string(),
        ),
    };
    db.insert_conflict(&conflict).unwrap();

    // 2. Resolve conflict with user edit atomically
    let edit_payload = ContextItemEditPayload {
        title: "Max Memory 3GB".into(),
        content: "Compromise limit is 3GB".into(),
    };

    db.resolve_conflict_with_edit(
        &conflict.id,
        "resolved",
        Some("Resolved by compromise at 3GB"),
        Some(&edit_payload),
        "user",
    )
    .unwrap();

    // Verify conflict row
    let c1 = db.get_conflict(&conflict.id).unwrap().unwrap();
    assert_eq!(c1.status, "resolved");
    assert_eq!(
        c1.resolution.as_deref(),
        Some("Resolved by compromise at 3GB")
    );

    // Verify item updated to new revision with user_edit authority
    let item_after = db.get_item(&item.id).unwrap().unwrap();
    assert_eq!(item_after.authority, "user_edit");
    let rev2_id = item_after.current_revision_id.unwrap();
    assert_ne!(rev2_id, rev1_id);

    // Verify revision provenance is user_edit / user
    let src2 = db.get_context_revision_source(&rev2_id).unwrap().unwrap();
    assert_eq!(src2.authority, "user_edit");

    // Verify audit history event captures resolved_revision_id
    let history = db.conflict_history(&conflict.id).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].previous_status, "open");
    assert_eq!(history[0].new_status, "resolved");
    assert_eq!(
        history[0].resolution.as_deref(),
        Some("Resolved by compromise at 3GB")
    );
    let snapshot: serde_json::Value =
        serde_json::from_str(history[0].snapshot_json.as_ref().unwrap()).unwrap();
    assert_eq!(snapshot["left_revision_id"], rev1_id);
    assert_eq!(snapshot["resolved_revision_id"], rev2_id);

    // 3. Verify no stale COALESCE inheritance: re-resolving with resolution = None clears it
    db.resolve_conflict_with_edit(&conflict.id, "dismissed", None, None, "user")
        .unwrap();
    let c2 = db.get_conflict(&conflict.id).unwrap().unwrap();
    assert_eq!(c2.status, "dismissed");
    assert_eq!(
        c2.resolution, None,
        "Resolution must be cleared when None is provided, not retain stale note"
    );

    // 4. Verify transaction rollback on error: empty title fails and rolls back everything
    let conflict2 = ContextConflict {
        id: new_id(),
        workstream_id: ws.id.clone(),
        left_item_id: item.id.clone(),
        right_item_id: None,
        conflict_type: "authority".into(),
        status: "open".into(),
        resolution: None,
        created_at: now(),
        updated_at: now(),
        left_revision_id: Some(rev2_id.clone()),
        right_revision_id: None,
        candidate_snapshot_json: None,
    };
    db.insert_conflict(&conflict2).unwrap();

    let invalid_edit = ContextItemEditPayload {
        title: "   ".into(), // whitespace only
        content: "Something".into(),
    };

    let err = db
        .resolve_conflict_with_edit(
            &conflict2.id,
            "resolved",
            Some("Should fail"),
            Some(&invalid_edit),
            "user",
        )
        .unwrap_err();
    assert!(err.to_string().contains("标题不能为空"));

    // Verify conflict2 remains open and untouched
    let c2_check = db.get_conflict(&conflict2.id).unwrap().unwrap();
    assert_eq!(c2_check.status, "open");
    assert_eq!(c2_check.resolution, None);

    // Verify no conflict event was written
    let h2 = db.conflict_history(&conflict2.id).unwrap();
    assert_eq!(h2.len(), 0);

    // Verify item head was NOT updated
    let item_check = db.get_item(&item.id).unwrap().unwrap();
    assert_eq!(
        item_check.current_revision_id.as_deref(),
        Some(rev2_id.as_str())
    );

    // 5. Verify invalid status rejection
    let err_status = db
        .resolve_conflict_with_edit(&conflict2.id, "unsupported_status", None, None, "user")
        .unwrap_err();
    assert!(err_status.to_string().contains("无效的冲突状态"));
}

#[test]
fn test_conflict_review_case_frozen_vs_current() {
    let db = open_db("conflict-case-frozen-vs-current");
    let ws = ws_row(&db, "Conflict Review Case WS");

    // 1. Create item A with initial revision A1
    let item_a = noending::sync::create_item(
        &db,
        &ws.id,
        "goal",
        "A1 Title",
        "A1 Content",
        "agent_statement",
        "session_event",
        &["session-event:a1".into()],
        None,
        "agent",
    )
    .unwrap();
    let rev_a1_id = item_a.current_revision_id.clone().unwrap();

    // 2. Create item B with initial revision B1
    let item_b = noending::sync::create_item(
        &db,
        &ws.id,
        "goal",
        "B1 Title",
        "B1 Content",
        "agent_statement",
        "session_event",
        &["session-event:b1".into()],
        None,
        "agent",
    )
    .unwrap();
    let rev_b1_id = item_b.current_revision_id.clone().unwrap();

    // 3. Conflict linking A1 and B1
    let conflict = ContextConflict {
        id: new_id(),
        workstream_id: ws.id.clone(),
        left_item_id: item_a.id.clone(),
        right_item_id: Some(item_b.id.clone()),
        conflict_type: "content".into(),
        status: "open".into(),
        resolution: None,
        created_at: now(),
        updated_at: now(),
        left_revision_id: Some(rev_a1_id.clone()),
        right_revision_id: Some(rev_b1_id.clone()),
        candidate_snapshot_json: None,
    };
    db.insert_conflict(&conflict).unwrap();

    // 4. Evolve item A to A2
    let rev_a2 = ContextItemRevision {
        id: new_id(),
        item_id: item_a.id.clone(),
        title: "A2 Title".into(),
        content: "A2 Content".into(),
        metadata: serde_json::json!({
            "provenance": {
                "authority": "user_edit",
                "actor": "user",
                "source_type": "user_edit",
                "source_ref": serde_json::Value::Null,
            }
        }),
        source_type: Some("user_edit".into()),
        source_ref: None,
        sync_run_id: None,
        created_at: now(),
    };
    db.insert_revision(&rev_a2).unwrap();
    db.set_item_head(&item_a.id, &rev_a2.id, None).unwrap();
    db.set_item_authority(&item_a.id, "user_edit").unwrap();

    // 5. Evolve item B to B2
    let rev_b2 = ContextItemRevision {
        id: new_id(),
        item_id: item_b.id.clone(),
        title: "B2 Title".into(),
        content: "B2 Content".into(),
        metadata: serde_json::json!({
            "provenance": {
                "authority": "agent_statement",
                "actor": "agent",
                "source_type": "session_event",
                "source_ref": serde_json::Value::Null,
            }
        }),
        source_type: Some("session_event".into()),
        source_ref: None,
        sync_run_id: None,
        created_at: now(),
    };
    db.insert_revision(&rev_b2).unwrap();
    db.set_item_head(&item_b.id, &rev_b2.id, None).unwrap();

    // 6. Query review case
    let review_case = db
        .get_conflict_review_case(&conflict.id)
        .unwrap()
        .expect("review case must exist");

    // Frozen matches conflict-time evidence A1 / B1
    assert_eq!(
        review_case.left_at_conflict.as_ref().map(|r| &r.id),
        Some(&rev_a1_id)
    );
    assert_eq!(
        review_case
            .left_at_conflict
            .as_ref()
            .map(|r| r.title.as_str()),
        Some("A1 Title")
    );
    assert_eq!(
        review_case.right_at_conflict.as_ref().map(|r| &r.id),
        Some(&rev_b1_id)
    );
    assert_eq!(
        review_case
            .right_at_conflict
            .as_ref()
            .map(|r| r.title.as_str()),
        Some("B1 Title")
    );

    // Current matches latest head revisions A2 / B2
    assert_eq!(
        review_case.current_left.as_ref().map(|r| &r.id),
        Some(&rev_a2.id)
    );
    assert_eq!(
        review_case.current_left.as_ref().map(|r| r.title.as_str()),
        Some("A2 Title")
    );
    assert_eq!(
        review_case.current_right.as_ref().map(|r| &r.id),
        Some(&rev_b2.id)
    );
    assert_eq!(
        review_case.current_right.as_ref().map(|r| r.title.as_str()),
        Some("B2 Title")
    );

    // Evolution flags are accurately detected
    assert!(review_case.left_changed_since_conflict);
    assert!(review_case.right_changed_since_conflict);
}

#[test]
fn test_fts_failure_rolls_back_entire_conflict_transaction() {
    let db = open_db("fts-rollback");
    let ws = ws_row(&db, "FTS Rollback WS");

    let item = noending::sync::create_item(
        &db,
        &ws.id,
        "decision",
        "Initial Decision",
        "Some details",
        "agent_statement",
        "session_event",
        &["session-event:1".into()],
        None,
        "agent",
    )
    .unwrap();
    let rev1_id = item.current_revision_id.clone().unwrap();

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
        left_revision_id: Some(rev1_id.clone()),
        right_revision_id: None,
        candidate_snapshot_json: None,
    };
    db.insert_conflict(&conflict).unwrap();

    // Drop the search_index table to force an FTS indexing error inside the transaction
    db.write()
        .execute_batch("DROP TABLE search_index;")
        .unwrap();

    let edit_payload = ContextItemEditPayload {
        title: "Attempted Fix".into(),
        content: "New content".into(),
    };

    let err = db
        .resolve_conflict_with_edit(
            &conflict.id,
            "resolved",
            Some("Should rollback completely"),
            Some(&edit_payload),
            "user",
        )
        .unwrap_err();

    // Confirm that error occurred from FTS
    assert!(err.to_string().contains("search_index") || err.to_string().contains("no such table"));

    // Verify entire transaction rolled back:
    // 1. Item head unchanged
    let item_after = db.get_item(&item.id).unwrap().unwrap();
    assert_eq!(
        item_after.current_revision_id.as_deref(),
        Some(rev1_id.as_str())
    );
    // 2. Item authority unchanged
    assert_eq!(item_after.authority, "agent_statement");
    // 3. Conflict status still open
    let c_check = db.get_conflict(&conflict.id).unwrap().unwrap();
    assert_eq!(c_check.status, "open");
    assert_eq!(c_check.resolution, None);
    // 4. No conflict event recorded
    let history = db.conflict_history(&conflict.id).unwrap();
    assert_eq!(history.len(), 0);
}

#[test]
fn revision_authority_is_never_polluted_by_item_authority() {
    let db = open_db("rev-authority");
    let ws = ws_row(&db, "Revision WS");

    // 1. Manually insert an item and revision whose metadata carries NO
    //    provenance object
    let item_id = new_id();
    let rev1_id = new_id();
    let ts = now();

    db.write().execute(
        "INSERT INTO context_items (id, workstream_id, kind, status, authority, created_by, current_revision_id, created_at, updated_at)
         VALUES (?1, ?2, 'goal', 'active', 'agent_statement', 'sync:codex', ?3, ?4, ?4)",
        params![item_id, ws.id, rev1_id, ts],
    ).unwrap();

    // Revision metadata is completely empty (no provenance object)
    db.write().execute(
        "INSERT INTO context_item_revisions (id, item_id, title, content, metadata, source_type, sync_run_id, created_at)
         VALUES (?1, ?2, 'Historical Agent Goal', 'Extracted by agent', '{}', 'session_event', 'sync-run-1', ?3)",
        params![rev1_id, item_id, ts],
    ).unwrap();

    // 2. Query revision source before user edit: authority is inferred as agent_statement
    let src_before = db.get_context_revision_source(&rev1_id).unwrap().unwrap();
    assert_eq!(src_before.authority, "agent_statement");

    // 3. Item is subsequently edited by user, changing context_items.authority to user_edit
    db.set_item_authority(&item_id, "user_edit").unwrap();
    let item_check = db.get_item(&item_id).unwrap().unwrap();
    assert_eq!(item_check.authority, "user_edit");

    // 4. Invariant check: Inspecting historical rev1 STILL returns agent_statement, NEVER user_edit!
    let src_after = db.get_context_revision_source(&rev1_id).unwrap().unwrap();
    assert_eq!(
        src_after.authority, "agent_statement",
        "A revision's recorded authority must NEVER fall back to mutable item.authority"
    );
    assert_ne!(src_after.authority, "user_edit");

    // 5. A completely un-inferrable revision resolves to `unknown`, NOT item.authority
    let rev_unknown_id = new_id();
    db.write().execute(
        "INSERT INTO context_item_revisions (id, item_id, title, content, metadata, source_type, sync_run_id, created_at)
         VALUES (?1, ?2, 'Undetermined origin', 'No source', '{}', NULL, NULL, ?3)",
        params![rev_unknown_id, item_id, ts],
    ).unwrap();

    let src_unknown = db
        .get_context_revision_source(&rev_unknown_id)
        .unwrap()
        .unwrap();
    assert_eq!(src_unknown.authority, "unknown");
    assert_ne!(src_unknown.authority, "user_edit");
}
