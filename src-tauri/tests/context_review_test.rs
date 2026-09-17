use noending::domain::{
    Agent, ContextConflict, ContextDelivery, ContextItemRevision, ReviewFrontier, Session,
    SessionWorkstreamBinding, Workstream,
};
use noending::storage::{new_id, now, Db};
use rusqlite::params;

fn open_db(tag: &str) -> Db {
    let dir = std::env::temp_dir().join(format!("noending-review-{}-{}", tag, new_id()));
    Db::open(&dir.join("test.db")).unwrap()
}

fn ws_row(db: &Db, title: &str) -> Workstream {
    let w = Workstream {
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
        title: Some("Session Review Isolation".into()),
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

/// 1. migration_baselines_existing_workstreams
/// Upgrading a pre-v10 database baselines existing workstreams so historical
/// context changes are treated as already reviewed.
#[test]
fn migration_baselines_existing_workstreams() {
    let dir = std::env::temp_dir().join(format!("noending-test-v10-mig-{}", new_id()));
    let db_path = dir.join("test.db");

    let ws_id = new_id();
    {
        let db = Db::open(&db_path).unwrap();
        let conn = db.conn();
        conn.execute("DROP TABLE IF EXISTS workstream_review_state", [])
            .unwrap();
        conn.pragma_update(None, "user_version", 9).unwrap();

        let past_time = "2026-09-01T10:00:00Z";
        conn.execute(
            "INSERT INTO workstreams (id, title, description, lifecycle, visibility, created_at, updated_at)
             VALUES (?1, 'Legacy Workstream', 'Existed before v10', 'open', 'normal', ?2, ?2)",
            params![ws_id, past_time],
        )
        .unwrap();

        let item_id = new_id();
        let rev_id = new_id();
        conn.execute(
            "INSERT INTO context_items (id, workstream_id, kind, status, authority, created_by, current_revision_id, created_at, updated_at)
             VALUES (?1, ?2, 'goal', 'active', 'agent_inferred', 'agent', ?3, ?4, ?4)",
            params![item_id, ws_id, rev_id, "2026-09-01T10:05:00Z"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO context_item_revisions (id, item_id, title, content, metadata, source_type, created_at)
             VALUES (?1, ?2, 'Legacy Goal', 'Pre-migration historical context', '{}', 'session_event', ?3)",
            params![rev_id, item_id, "2026-09-01T10:05:00Z"],
        )
        .unwrap();
    }

    // Reopen database with v10 migration code
    let db = Db::open(&db_path).unwrap();
    let version: i64 = db
        .conn()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, noending::storage::SCHEMA_VERSION);

    // Verify historical changes are NOT unseen
    let win = db.get_workstream_review_window(&ws_id).unwrap();
    assert!(
        win.unseen_changes.is_empty(),
        "Historical pre-migration changes must be considered already reviewed"
    );

    // New change created after migration becomes unseen
    std::thread::sleep(std::time::Duration::from_millis(50));
    let new_item = noending::sync::create_item(
        &db,
        &ws_id,
        "goal",
        "New Post-Migration Goal",
        "Should appear as unseen",
        "agent_inferred",
        "session_event",
        &[],
        None,
        "agent",
    )
    .unwrap();

    let win_after = db.get_workstream_review_window(&ws_id).unwrap();
    assert_eq!(win_after.unseen_changes.len(), 1);
    assert_eq!(win_after.unseen_changes[0].item_id, Some(new_item.id));
}

/// 2. new_agent_change_is_unseen
/// An Agent revision created after baseline appears in unseen_changes.
#[test]
fn new_agent_change_is_unseen() {
    let db = open_db("agent-unseen");
    let ws = ws_row(&db, "Agent Unseen WS");

    let initial_win = db.get_workstream_review_window(&ws.id).unwrap();
    assert!(initial_win.unseen_changes.is_empty());

    std::thread::sleep(std::time::Duration::from_millis(20));
    let item = noending::sync::create_item(
        &db,
        &ws.id,
        "goal",
        "Agent Inferred Goal",
        "Agent observed something",
        "agent_inferred",
        "session_event",
        &[],
        Some("sync-run-1"),
        "sync:agent",
    )
    .unwrap();

    let win = db.get_workstream_review_window(&ws.id).unwrap();
    assert_eq!(win.unseen_changes.len(), 1);
    assert_eq!(win.unseen_changes[0].title, "Agent Inferred Goal");
    assert_eq!(win.unseen_changes[0].actor, "Agent");
    assert_eq!(win.unseen_changes[0].kind, "added");
    assert_eq!(win.unseen_changes[0].item_id, Some(item.id.clone()));

    assert_eq!(
        win.mark_through.through_at,
        win.unseen_changes[0].created_at
    );
    assert_eq!(
        win.mark_through.boundary_change_ids,
        vec![win.unseen_changes[0].id.clone()]
    );
}

/// 3. user_edit_is_not_review_relevant
/// Explicit user edits and additions are known to the user and do NOT become unseen.
#[test]
fn user_edit_is_not_review_relevant() {
    let db = open_db("user-edit-relevance");
    let ws = ws_row(&db, "User Relevance WS");

    std::thread::sleep(std::time::Duration::from_millis(20));
    let item = noending::sync::create_item(
        &db,
        &ws.id,
        "current_state",
        "Initial State",
        "State from agent",
        "agent_inferred",
        "session_event",
        &[],
        None,
        "agent",
    )
    .unwrap();

    // Catch up frontier so initial item is reviewed
    let win = db.get_workstream_review_window(&ws.id).unwrap();
    assert_eq!(win.unseen_changes.len(), 1);
    db.mark_workstream_reviewed(&ws.id, &win.mark_through)
        .unwrap();

    let win_clean = db.get_workstream_review_window(&ws.id).unwrap();
    assert!(win_clean.unseen_changes.is_empty());

    // 1. User performs an edit on existing item
    std::thread::sleep(std::time::Duration::from_millis(20));
    let edit_rev = ContextItemRevision {
        id: new_id(),
        item_id: item.id.clone(),
        title: "User Edited State".into(),
        content: "User revised content".into(),
        metadata: serde_json::json!({
            "provenance": {
                "authority": "user_edit",
                "actor": "user",
                "source_type": "user_edit",
            }
        }),
        source_type: Some("user_edit".into()),
        source_ref: None,
        sync_run_id: None,
        created_at: now(),
    };
    db.insert_revision(&edit_rev).unwrap();
    db.set_item_head(&item.id, &edit_rev.id, None).unwrap();
    db.set_item_authority(&item.id, "user_edit").unwrap();

    // 2. User adds a new item explicitly
    std::thread::sleep(std::time::Duration::from_millis(20));
    noending::sync::create_item(
        &db,
        &ws.id,
        "goal",
        "User Created Goal",
        "User manual goal",
        "user_explicit",
        "user_edit",
        &[],
        None,
        "user",
    )
    .unwrap();

    // Query window: user actions must NOT appear in unseen_changes
    let win_after_user = db.get_workstream_review_window(&ws.id).unwrap();
    assert!(
        win_after_user.unseen_changes.is_empty(),
        "User's own edits/additions must not be treated as unseen in Review Loop"
    );
}

/// 4. conflict_creation_is_unseen
/// New conflicts are contested state and always enter unseen.
#[test]
fn conflict_creation_is_unseen() {
    let db = open_db("conflict-unseen");
    let ws = ws_row(&db, "Conflict Unseen WS");

    let item1 = noending::sync::create_item(
        &db,
        &ws.id,
        "goal",
        "Goal A",
        "A",
        "user_explicit",
        "user_edit",
        &[],
        None,
        "user",
    )
    .unwrap();

    let item2 = noending::sync::create_item(
        &db,
        &ws.id,
        "goal",
        "Goal B",
        "B",
        "user_explicit",
        "user_edit",
        &[],
        None,
        "user",
    )
    .unwrap();

    let win0 = db.get_workstream_review_window(&ws.id).unwrap();
    db.mark_workstream_reviewed(&ws.id, &win0.mark_through)
        .unwrap();

    std::thread::sleep(std::time::Duration::from_millis(20));
    let conflict = ContextConflict {
        id: new_id(),
        workstream_id: ws.id.clone(),
        left_item_id: item1.id.clone(),
        right_item_id: Some(item2.id.clone()),
        conflict_type: "direct_contradiction".into(),
        status: "open".into(),
        resolution: None,
        created_at: now(),
        updated_at: now(),
        left_revision_id: None,
        right_revision_id: None,
        candidate_snapshot_json: None,
    };
    db.insert_conflict(&conflict).unwrap();

    let win = db.get_workstream_review_window(&ws.id).unwrap();
    assert_eq!(win.unseen_changes.len(), 1);
    assert_eq!(win.unseen_changes[0].kind, "conflict_created");
    assert_eq!(win.unseen_changes[0].conflict_id, Some(conflict.id));
    assert_eq!(win.unseen_changes[0].actor, "Agent");
}

/// 5. mark_reviewed_advances_only_observed_frontier
/// Reading A/B gives mark_through F1. If C is inserted concurrently,
/// marking F1 advances through A and B, leaving C unseen.
#[test]
fn mark_reviewed_advances_only_observed_frontier() {
    let db = open_db("advances-observed");
    let ws = ws_row(&db, "Observed Frontier WS");

    std::thread::sleep(std::time::Duration::from_millis(20));
    let item_a = noending::sync::create_item(
        &db,
        &ws.id,
        "goal",
        "Goal A",
        "A",
        "agent_inferred",
        "session_event",
        &[],
        None,
        "agent",
    )
    .unwrap();

    std::thread::sleep(std::time::Duration::from_millis(20));
    let item_b = noending::sync::create_item(
        &db,
        &ws.id,
        "goal",
        "Goal B",
        "B",
        "agent_inferred",
        "session_event",
        &[],
        None,
        "agent",
    )
    .unwrap();

    let window_ab = db.get_workstream_review_window(&ws.id).unwrap();
    assert_eq!(window_ab.unseen_changes.len(), 2);
    let token_ab = window_ab.mark_through.clone();

    // Meanwhile, background sync adds change C
    std::thread::sleep(std::time::Duration::from_millis(20));
    let item_c = noending::sync::create_item(
        &db,
        &ws.id,
        "goal",
        "Goal C",
        "C",
        "agent_inferred",
        "session_event",
        &[],
        None,
        "agent",
    )
    .unwrap();

    // User marks reviewed using the token from the observed window
    let advanced_state = db.mark_workstream_reviewed(&ws.id, &token_ab).unwrap();
    assert_eq!(advanced_state.frontier, token_ab);

    // Change C MUST remain unseen
    let window_after = db.get_workstream_review_window(&ws.id).unwrap();
    assert_eq!(window_after.unseen_changes.len(), 1);
    assert_eq!(window_after.unseen_changes[0].item_id, Some(item_c.id));
    assert_ne!(window_after.unseen_changes[0].item_id, Some(item_a.id));
    assert_ne!(window_after.unseen_changes[0].item_id, Some(item_b.id));
}

/// 6. same_timestamp_boundary_is_lossless
/// When changes A and B share identical created_at timestamps, reviewing only A
/// leaves B unseen without loss.
#[test]
fn same_timestamp_boundary_is_lossless() {
    let db = open_db("lossless-boundary");

    // Create a workstream with baseline at 11:00:00Z
    let ws_id = new_id();
    let ws = Workstream {
        id: ws_id.clone(),
        project_id: None,
        title: "Lossless WS".into(),
        description: String::new(),
        lifecycle: "open".into(),
        visibility: "normal".into(),
        default_cwd: None,
        created_at: "2026-09-17T11:00:00Z".into(),
        updated_at: "2026-09-17T11:00:00Z".into(),
    };
    db.upsert_workstream(&ws).unwrap();

    let ts = "2026-09-17T12:00:00Z".to_string();

    let item_a_id = new_id();
    let rev_a_id = new_id();
    let item_b_id = new_id();
    let rev_b_id = new_id();

    let conn = db.conn();
    conn.execute(
        "INSERT INTO context_items (id, workstream_id, kind, status, authority, created_by, current_revision_id, created_at, updated_at)
         VALUES (?1, ?2, 'goal', 'active', 'agent_inferred', 'agent', ?3, ?4, ?4)",
        params![item_a_id, ws.id, rev_a_id, ts],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO context_item_revisions (id, item_id, title, content, metadata, source_type, created_at)
         VALUES (?1, ?2, 'Goal A', 'Same timestamp A', '{\"provenance\":{\"actor\":\"agent\"}}', 'session_event', ?3)",
        params![rev_a_id, item_a_id, ts],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO context_items (id, workstream_id, kind, status, authority, created_by, current_revision_id, created_at, updated_at)
         VALUES (?1, ?2, 'goal', 'active', 'agent_inferred', 'agent', ?3, ?4, ?4)",
        params![item_b_id, ws.id, rev_b_id, ts],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO context_item_revisions (id, item_id, title, content, metadata, source_type, created_at)
         VALUES (?1, ?2, 'Goal B', 'Same timestamp B', '{\"provenance\":{\"actor\":\"agent\"}}', 'session_event', ?3)",
        params![rev_b_id, item_b_id, ts],
    )
    .unwrap();

    // Set review frontier to only have reviewed A at timestamp ts
    let frontier = ReviewFrontier {
        through_at: ts.clone(),
        boundary_change_ids: vec![rev_a_id.clone()],
    };
    db.mark_workstream_reviewed(&ws.id, &frontier).unwrap();

    // Query window: B MUST remain unseen
    let win = db.get_workstream_review_window(&ws.id).unwrap();
    assert_eq!(win.unseen_changes.len(), 1);
    assert_eq!(win.unseen_changes[0].id, rev_b_id);

    // mark_through should now combine both A and B
    assert_eq!(win.mark_through.through_at, ts);
    assert!(win.mark_through.boundary_change_ids.contains(&rev_a_id));
    assert!(win.mark_through.boundary_change_ids.contains(&rev_b_id));

    // Mark reviewed with the combined frontier
    db.mark_workstream_reviewed(&ws.id, &win.mark_through)
        .unwrap();

    // Now all changes at ts are reviewed
    let win_final = db.get_workstream_review_window(&ws.id).unwrap();
    assert!(win_final.unseen_changes.is_empty());
}

/// 7. review_state_isolated_from_domain_state
/// Marking reviewed is purely observational and MUST NOT mutate Context items,
/// revisions, conflicts, ContextDelivery snapshots, Session cursors, or bindings.
#[test]
fn review_state_isolated_from_domain_state() {
    let db = open_db("domain-isolation");
    let ws = ws_row(&db, "Isolation WS");
    let s = session_row(&db, Agent::Codex);

    let item = noending::sync::create_item(
        &db,
        &ws.id,
        "goal",
        "Core Goal",
        "Mission",
        "agent_inferred",
        "session_event",
        &[],
        None,
        "agent",
    )
    .unwrap();

    let item2 = noending::sync::create_item(
        &db,
        &ws.id,
        "constraint",
        "Constraint",
        "Rule",
        "agent_inferred",
        "session_event",
        &[],
        None,
        "agent",
    )
    .unwrap();
    let conflict = ContextConflict {
        id: new_id(),
        workstream_id: ws.id.clone(),
        left_item_id: item.id.clone(),
        right_item_id: Some(item2.id.clone()),
        conflict_type: "direct_contradiction".into(),
        status: "open".into(),
        resolution: None,
        created_at: now(),
        updated_at: now(),
        left_revision_id: None,
        right_revision_id: None,
        candidate_snapshot_json: None,
    };
    db.insert_conflict(&conflict).unwrap();

    let delivery = ContextDelivery {
        id: new_id(),
        session_id: s.id.clone(),
        workstream_id: ws.id.clone(),
        bundle_id: "bundle-1".into(),
        delivered_revisions: vec![item.current_revision_id.clone().unwrap()],
        delivered_conflicts: vec![conflict.id.clone()],
        delivered_at: now(),
    };
    db.record_delivery(&delivery).unwrap();

    let mut cursor = db.get_source_cursor(&s.id).unwrap();
    cursor.session_id = s.id.clone();
    cursor.last_sequence = 42;
    cursor.byte_offset = 100;
    cursor.generation = 1;
    db.set_source_cursor(&cursor).unwrap();

    let binding = SessionWorkstreamBinding {
        session_id: s.id.clone(),
        workstream_id: ws.id.clone(),
        role: "primary".into(),
        source: "explicit_launch_selection".into(),
        confidence: 1.0,
        last_seen_revision: None,
        last_sync_cursor: 0,
        created_at: now(),
        last_used_at: now(),
    };
    db.bind(&binding).unwrap();

    // Snapshot all domain states before mark reviewed
    let item_before = db.get_item(&item.id).unwrap().unwrap();
    let history_before = db.item_history(&item.id).unwrap();
    let conflict_before = db.get_conflict(&conflict.id).unwrap().unwrap();
    let deliveries_before = db.latest_deliveries(&s.id).unwrap();
    let cursor_before = db.get_source_cursor(&s.id).unwrap();
    let bindings_before = db.bindings_for_session(&s.id).unwrap();

    // Perform mark_workstream_reviewed
    let win = db.get_workstream_review_window(&ws.id).unwrap();
    let advanced_state = db
        .mark_workstream_reviewed(&ws.id, &win.mark_through)
        .unwrap();
    assert_eq!(advanced_state.frontier, win.mark_through);

    // Snapshot and verify all domain states after mark reviewed
    let item_after = db.get_item(&item.id).unwrap().unwrap();
    let history_after = db.item_history(&item.id).unwrap();
    let conflict_after = db.get_conflict(&conflict.id).unwrap().unwrap();
    let deliveries_after = db.latest_deliveries(&s.id).unwrap();
    let cursor_after = db.get_source_cursor(&s.id).unwrap();
    let bindings_after = db.bindings_for_session(&s.id).unwrap();

    assert_eq!(item_before.id, item_after.id);
    assert_eq!(item_before.kind, item_after.kind);
    assert_eq!(item_before.status, item_after.status);
    assert_eq!(item_before.authority, item_after.authority);
    assert_eq!(
        item_before.current_revision_id,
        item_after.current_revision_id
    );
    assert_eq!(item_before.updated_at, item_after.updated_at);

    assert_eq!(history_before.len(), history_after.len());
    for (b, a) in history_before.iter().zip(history_after.iter()) {
        assert_eq!(b.id, a.id);
        assert_eq!(b.title, a.title);
        assert_eq!(b.content, a.content);
        assert_eq!(b.created_at, a.created_at);
    }

    assert_eq!(conflict_before.id, conflict_after.id);
    assert_eq!(conflict_before.status, conflict_after.status);
    assert_eq!(conflict_before.resolution, conflict_after.resolution);
    assert_eq!(conflict_before.updated_at, conflict_after.updated_at);

    assert_eq!(deliveries_before.len(), deliveries_after.len());
    assert_eq!(deliveries_before[0].id, deliveries_after[0].id);
    assert_eq!(
        deliveries_before[0].delivered_revisions,
        deliveries_after[0].delivered_revisions
    );
    assert_eq!(
        deliveries_before[0].delivered_conflicts,
        deliveries_after[0].delivered_conflicts
    );

    assert_eq!(cursor_before.last_sequence, cursor_after.last_sequence);
    assert_eq!(cursor_before.byte_offset, cursor_after.byte_offset);
    assert_eq!(cursor_before.generation, cursor_after.generation);

    assert_eq!(bindings_before.len(), bindings_after.len());
    assert_eq!(
        bindings_before[0].workstream_id,
        bindings_after[0].workstream_id
    );
    assert_eq!(bindings_before[0].role, bindings_after[0].role);
}
