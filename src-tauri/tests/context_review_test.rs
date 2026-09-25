use noending::domain::{
    Agent, ContextConflict, ContextDelivery, ContextItemRevision, ReviewFrontier, Session,
    Workstream,
};
use noending::storage::{new_id, now, Db};
use rusqlite::params;
use std::ops::Deref;

struct TestDb {
    db: Option<Db>,
    dir: std::path::PathBuf,
}

impl Deref for TestDb {
    type Target = Db;

    fn deref(&self) -> &Self::Target {
        self.db.as_ref().unwrap()
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        self.db.take();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn open_db(tag: &str) -> TestDb {
    let dir = std::env::temp_dir().join(format!("noending-review-{}-{}", tag, new_id()));
    let db = Db::open(&dir.join("test.db")).unwrap();
    TestDb { db: Some(db), dir }
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

fn session_row(db: &TestDb, agent: Agent) -> Session {
    let raw = db.dir.join(format!("raw-{}.jsonl", new_id()));
    let _ = std::fs::write(&raw, "");
    let s = Session {
        id: new_id(),
        agent,
        agent_session_id: format!("as-{}", new_id()),
        title: Some("Session Review Isolation".into()),
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

fn set_item_time(db: &Db, item_id: &str, timestamp: &str) {
    let conn = db.write();
    conn.execute(
        "UPDATE context_items SET created_at = ?2, updated_at = ?2 WHERE id = ?1",
        params![item_id, timestamp],
    )
    .unwrap();
    conn.execute(
        "UPDATE context_item_revisions SET created_at = ?2 WHERE item_id = ?1",
        params![item_id, timestamp],
    )
    .unwrap();
}

fn set_revision_time(db: &Db, item_id: &str, revision_id: &str, timestamp: &str) {
    let conn = db.write();
    conn.execute(
        "UPDATE context_item_revisions SET created_at = ?2 WHERE id = ?1",
        params![revision_id, timestamp],
    )
    .unwrap();
    conn.execute(
        "UPDATE context_items SET updated_at = ?2 WHERE id = ?1",
        params![item_id, timestamp],
    )
    .unwrap();
}

fn set_conflict_time(db: &Db, conflict_id: &str, timestamp: &str) {
    db.write()
        .execute(
            "UPDATE context_conflicts SET created_at = ?2, updated_at = ?2 WHERE id = ?1",
            params![conflict_id, timestamp],
        )
        .unwrap();
}

/// 2. new_agent_change_is_unseen
/// An Agent revision created after baseline appears in unseen_changes.
#[test]
fn new_agent_change_is_unseen() {
    let db = open_db("agent-unseen");
    let ws = ws_row(&db, "Agent Unseen WS");

    let initial_win = db.get_workstream_review_window(&ws.id).unwrap();
    assert!(initial_win.unseen_changes.is_empty());

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
    set_item_time(&db, &item.id, "2100-01-01T00:00:01Z");

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
    set_item_time(&db, &item.id, "2100-01-01T00:00:01Z");

    // Catch up frontier so initial item is reviewed
    let win = db.get_workstream_review_window(&ws.id).unwrap();
    assert_eq!(win.unseen_changes.len(), 1);
    db.mark_workstream_reviewed(&ws.id, &win.mark_through)
        .unwrap();

    let win_clean = db.get_workstream_review_window(&ws.id).unwrap();
    assert!(win_clean.unseen_changes.is_empty());

    // 1. User performs an edit on existing item
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
    set_revision_time(&db, &item.id, &edit_rev.id, "2100-01-01T00:00:02Z");

    // 2. User adds a new item explicitly
    let user_item = noending::sync::create_item(
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
    set_item_time(&db, &user_item.id, "2100-01-01T00:00:03Z");

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
    set_conflict_time(&db, &conflict.id, "2100-01-01T00:00:01Z");

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
    set_item_time(&db, &item_a.id, "2100-01-01T00:00:01Z");

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
    set_item_time(&db, &item_b.id, "2100-01-01T00:00:02Z");

    let window_ab = db.get_workstream_review_window(&ws.id).unwrap();
    assert_eq!(window_ab.unseen_changes.len(), 2);
    let token_ab = window_ab.mark_through.clone();

    // Meanwhile, background sync adds change C
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
    set_item_time(&db, &item_c.id, "2100-01-01T00:00:03Z");

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
        title: "Lossless WS".into(),
        description: String::new(),
        lifecycle: "active".into(),
        visibility: "normal".into(),
        created_at: "2026-09-17T11:00:00Z".into(),
        updated_at: "2026-09-17T11:00:00Z".into(),
    };
    db.upsert_workstream(&ws).unwrap();

    let ts = "2026-09-17T12:00:00Z".to_string();

    let item_a_id = new_id();
    let rev_a_id = new_id();
    let item_b_id = new_id();
    let rev_b_id = new_id();

    // Scoped: the writer guard must drop before any further `Db` method —
    // the writer mutex is not re-entrant (see Db::tx's contract note).
    {
        let conn = db.write();
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
    }

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
/// revisions, conflicts, ContextDelivery snapshots, or Session cursors.
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

    db.set_session_owner(&s.id, Some(&ws.id)).unwrap();

    // Snapshot all domain states before mark reviewed
    let item_before = db.get_item(&item.id).unwrap().unwrap();
    let history_before = db.item_history(&item.id).unwrap();
    let conflict_before = db.get_conflict(&conflict.id).unwrap().unwrap();
    let deliveries_before = db.latest_deliveries(&s.id).unwrap();
    let cursor_before = db.get_source_cursor(&s.id).unwrap();
    let owner_before = db.get_session(&s.id).unwrap().unwrap().owner_workstream_id;

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
    let owner_after = db.get_session(&s.id).unwrap().unwrap().owner_workstream_id;

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

    assert_eq!(owner_before, owner_after, "review must not touch the owner");
}

/// 8. sync_status_change_is_review_relevant
/// Sync automated status changes (e.g. actor = "sync:heuristic") must be normalized
/// to Agent and appear in unseen_changes.
#[test]
fn sync_status_change_is_review_relevant() {
    let db = open_db("sync-status-relevant");
    let ws = ws_row(&db, "Sync Status WS");

    let item = noending::sync::create_item(
        &db,
        &ws.id,
        "todo",
        "Auto Resolvable Todo",
        "Will be resolved by sync",
        "agent_inferred",
        "session_event",
        &[],
        None,
        "agent",
    )
    .unwrap();

    // Catch up frontier
    let win0 = db.get_workstream_review_window(&ws.id).unwrap();
    db.mark_workstream_reviewed(&ws.id, &win0.mark_through)
        .unwrap();

    // Simulate Sync automated resolution: actor = "sync:heuristic"
    noending::storage::apply_status_change_conn(
        &db.write(),
        &item.id,
        "resolved",
        "sync:heuristic",
        "自动推断已完成",
        Some("sync-run-1"),
        &[],
    )
    .unwrap();
    let changed = db.get_item(&item.id).unwrap().unwrap();
    set_revision_time(
        &db,
        &item.id,
        changed.current_revision_id.as_deref().unwrap(),
        "2100-01-01T00:00:01Z",
    );

    let win = db.get_workstream_review_window(&ws.id).unwrap();
    assert_eq!(win.unseen_changes.len(), 1);
    assert_eq!(win.unseen_changes[0].kind, "resolved");
    assert_eq!(win.unseen_changes[0].actor, "Agent");
}

/// 9. conflict_evidence_is_not_double_counted
/// When a conflict occurs in production, the finding item created as evidence
/// (source_type = "conflict") must be suppressed in the review loop, so that only
/// the conflict_created event appears as unseen.
#[test]
fn conflict_evidence_is_not_double_counted() {
    let db = open_db("conflict-dedup");
    let ws = ws_row(&db, "Conflict Evidence WS");

    let existing = noending::sync::create_item(
        &db,
        &ws.id,
        "constraint",
        "Keep DB lock short",
        "Do not hold lock across network",
        "user_explicit",
        "user_edit",
        &[],
        None,
        "user",
    )
    .unwrap();

    // Catch up frontier
    let win0 = db.get_workstream_review_window(&ws.id).unwrap();
    db.mark_workstream_reviewed(&ws.id, &win0.mark_through)
        .unwrap();

    // 1. Production merge engine creates an evidence item with source_type = "conflict"
    let finding = noending::sync::create_item_conn(
        &db.write(),
        &ws.id,
        "finding",
        "Conflicting constraint claim",
        "Agent observed lock can be held",
        "agent_inferred",
        "conflict",
        &[],
        Some("sync-run-1"),
        "sync:claude-3-7-sonnet",
    )
    .unwrap();
    set_item_time(&db, &finding.id, "2100-01-01T00:00:01Z");

    // 2. Production merge engine inserts ContextConflict
    let conflict = ContextConflict {
        id: new_id(),
        workstream_id: ws.id.clone(),
        left_item_id: existing.id.clone(),
        right_item_id: Some(finding.id.clone()),
        conflict_type: "direct_contradiction".into(),
        status: "open".into(),
        resolution: None,
        created_at: now(),
        updated_at: now(),
        left_revision_id: existing.current_revision_id.clone(),
        right_revision_id: finding.current_revision_id.clone(),
        candidate_snapshot_json: None,
    };
    db.insert_conflict(&conflict).unwrap();
    set_conflict_time(&db, &conflict.id, "2100-01-01T00:00:02Z");

    // Review window must only contain the conflict_created event, NOT the finding evidence item!
    let win = db.get_workstream_review_window(&ws.id).unwrap();
    assert_eq!(
        win.unseen_changes.len(),
        1,
        "Finding evidence item must be suppressed from unseen changes"
    );
    assert_eq!(win.unseen_changes[0].kind, "conflict_created");
    assert_eq!(win.unseen_changes[0].conflict_id, Some(conflict.id));
}

/// 10. review_summary_category_breakdown
/// Verifies that WorkstreamReviewSummary correctly counts each category:
/// new_facts, updated_facts, resolved_items, superseded_items, unseen_change_count, open_conflict_count.
#[test]
fn review_summary_category_breakdown() {
    let db = open_db("summary-categories");
    let ws = ws_row(&db, "Category Breakdown WS");

    // Baseline catch up
    let win0 = db.get_workstream_review_window(&ws.id).unwrap();
    db.mark_workstream_reviewed(&ws.id, &win0.mark_through)
        .unwrap();

    // 1. Added item (new_facts + 1)
    let item1 = noending::sync::create_item(
        &db,
        &ws.id,
        "todo",
        "Task 1",
        "First task",
        "agent_inferred",
        "session_event",
        &[],
        None,
        "agent",
    )
    .unwrap();
    set_item_time(&db, &item1.id, "2100-01-01T00:00:01Z");

    // 2. Added item (new_facts + 1), then edited (updated_facts + 1)
    let item2 = noending::sync::create_item(
        &db,
        &ws.id,
        "goal",
        "Goal 2",
        "Initial goal content",
        "agent_inferred",
        "session_event",
        &[],
        None,
        "agent",
    )
    .unwrap();
    set_item_time(&db, &item2.id, "2100-01-01T00:00:02Z");

    let edit_rev = ContextItemRevision {
        id: new_id(),
        item_id: item2.id.clone(),
        title: "Goal 2 Updated".into(),
        content: "Refined goal content".into(),
        metadata: serde_json::json!({
            "provenance": {
                "authority": "agent_inferred",
                "actor": "agent",
                "source_type": "session_event",
            }
        }),
        source_type: Some("session_event".into()),
        source_ref: None,
        sync_run_id: Some("sync-run-2".into()),
        created_at: now(),
    };
    db.insert_revision(&edit_rev).unwrap();
    db.set_item_head(&item2.id, &edit_rev.id, None).unwrap();
    set_revision_time(&db, &item2.id, &edit_rev.id, "2100-01-01T00:00:03Z");

    // 3. Added item (new_facts + 1), then resolved via sync (resolved_items + 1)
    let item3 = noending::sync::create_item(
        &db,
        &ws.id,
        "todo",
        "Task 3",
        "Will resolve",
        "agent_inferred",
        "session_event",
        &[],
        None,
        "agent",
    )
    .unwrap();
    set_item_time(&db, &item3.id, "2100-01-01T00:00:04Z");

    noending::storage::apply_status_change_conn(
        &db.write(),
        &item3.id,
        "resolved",
        "sync:heuristic",
        "Auto completed",
        Some("sync-run-3"),
        &[],
    )
    .unwrap();
    let item3_after = db.get_item(&item3.id).unwrap().unwrap();
    set_revision_time(
        &db,
        &item3.id,
        item3_after.current_revision_id.as_deref().unwrap(),
        "2100-01-01T00:00:05Z",
    );

    // 4. Added item (new_facts + 1), then superseded via sync (superseded_items + 1)
    let item4 = noending::sync::create_item(
        &db,
        &ws.id,
        "decision",
        "Decision 4",
        "Old decision",
        "agent_inferred",
        "session_event",
        &[],
        None,
        "agent",
    )
    .unwrap();
    set_item_time(&db, &item4.id, "2100-01-01T00:00:06Z");

    noending::storage::apply_status_change_conn(
        &db.write(),
        &item4.id,
        "superseded",
        "sync:heuristic",
        "Superseded by sync",
        Some("sync-run-4"),
        &[],
    )
    .unwrap();
    let item4_after = db.get_item(&item4.id).unwrap().unwrap();
    set_revision_time(
        &db,
        &item4.id,
        item4_after.current_revision_id.as_deref().unwrap(),
        "2100-01-01T00:00:07Z",
    );

    // 5. Open conflict (open_conflict_count = 1, conflict_created counts in unseen_change_count only)
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
        left_revision_id: item1.current_revision_id.clone(),
        right_revision_id: item2.current_revision_id.clone(),
        candidate_snapshot_json: None,
    };
    db.insert_conflict(&conflict).unwrap();
    set_conflict_time(&db, &conflict.id, "2100-01-01T00:00:08Z");

    let summary = db.get_workstream_review_summary(&ws.id).unwrap();

    assert_eq!(summary.workstream_id, ws.id);
    assert_eq!(summary.new_facts, 4, "4 added items");
    assert_eq!(summary.updated_facts, 1, "1 edited item");
    assert_eq!(summary.resolved_items, 1, "1 resolved item");
    assert_eq!(summary.superseded_items, 1, "1 superseded item");
    assert_eq!(summary.open_conflict_count, 1, "1 open conflict");
    // unseen_change_count = 4 (added) + 1 (edited) + 1 (resolved) + 1 (superseded) + 1 (conflict_created) = 8
    assert_eq!(summary.unseen_change_count, 8);
    assert!(summary.has_updates);
    assert!(summary.needs_attention);
    assert!(summary.last_unseen_change_at.is_some());
}

/// 11. review_summary_updates_vs_attention_independence
/// Tests the core invariant: has_updates and needs_attention are completely independent.
/// Even after a user marks a window reviewed (has_updates -> false), an unresolved
/// conflict keeps needs_attention -> true until resolved.
#[test]
fn review_summary_updates_vs_attention_independence() {
    let db = open_db("summary-independence");
    let ws = ws_row(&db, "Independence WS");

    let item1 = noending::sync::create_item(
        &db,
        &ws.id,
        "goal",
        "Goal 1",
        "G1",
        "user_explicit",
        "user_edit",
        &[],
        None,
        "user",
    )
    .unwrap();

    // Baseline catch up
    let win0 = db.get_workstream_review_window(&ws.id).unwrap();
    db.mark_workstream_reviewed(&ws.id, &win0.mark_through)
        .unwrap();

    // Create open conflict
    let conflict = ContextConflict {
        id: new_id(),
        workstream_id: ws.id.clone(),
        left_item_id: item1.id.clone(),
        right_item_id: None,
        conflict_type: "external_contradiction".into(),
        status: "open".into(),
        resolution: None,
        created_at: now(),
        updated_at: now(),
        left_revision_id: item1.current_revision_id.clone(),
        right_revision_id: None,
        candidate_snapshot_json: None,
    };
    db.insert_conflict(&conflict).unwrap();
    set_conflict_time(&db, &conflict.id, "2100-01-01T00:00:01Z");

    // Before review: has_updates = true, needs_attention = true
    let s0 = db.get_workstream_review_summary(&ws.id).unwrap();
    assert_eq!(s0.unseen_change_count, 1);
    assert_eq!(s0.open_conflict_count, 1);
    assert!(s0.has_updates);
    assert!(s0.needs_attention);

    // Human reviews the changes: mark reviewed
    let win1 = db.get_workstream_review_window(&ws.id).unwrap();
    db.mark_workstream_reviewed(&ws.id, &win1.mark_through)
        .unwrap();

    // After review but before conflict resolution:
    // has_updates MUST be false (no new changes), but needs_attention MUST remain true!
    let s1 = db.get_workstream_review_summary(&ws.id).unwrap();
    assert_eq!(s1.unseen_change_count, 0);
    assert_eq!(s1.open_conflict_count, 1);
    assert!(
        !s1.has_updates,
        "has_updates must be false when unseen_change_count is 0"
    );
    assert!(
        s1.needs_attention,
        "needs_attention must remain true because conflict is open"
    );
    assert!(s1.last_unseen_change_at.is_none());

    // Now resolve the conflict
    db.resolve_conflict_audited(
        &conflict.id,
        "resolved",
        Some("Accepted user state"),
        "user",
    )
    .unwrap();

    // After resolution: has_updates = false, needs_attention = false
    let s2 = db.get_workstream_review_summary(&ws.id).unwrap();
    assert_eq!(s2.unseen_change_count, 0);
    assert_eq!(s2.open_conflict_count, 0);
    assert!(!s2.has_updates);
    assert!(!s2.needs_attention);
}

/// 12. review_summary_batch_list
/// Verifies that list_workstream_review_summaries returns isomorphic summaries
/// for all workstreams without N+1 mismatches.
#[test]
fn review_summary_batch_list() {
    let db = open_db("summary-batch");
    let ws1 = ws_row(&db, "Batch WS 1");
    let ws2 = ws_row(&db, "Batch WS 2");
    let ws3 = ws_row(&db, "Batch WS 3");

    // Baseline catch up for all
    for ws in [&ws1, &ws2, &ws3] {
        let win = db.get_workstream_review_window(&ws.id).unwrap();
        db.mark_workstream_reviewed(&ws.id, &win.mark_through)
            .unwrap();
    }

    // ws1 has a new agent item
    let ws1_item = noending::sync::create_item(
        &db,
        &ws1.id,
        "todo",
        "WS1 Task",
        "Details",
        "agent_inferred",
        "session_event",
        &[],
        None,
        "agent",
    )
    .unwrap();
    set_item_time(&db, &ws1_item.id, "2100-01-01T00:00:01Z");

    // ws2 has an open conflict
    let item2 = noending::sync::create_item(
        &db,
        &ws2.id,
        "goal",
        "WS2 Goal",
        "G",
        "user_explicit",
        "user_edit",
        &[],
        None,
        "user",
    )
    .unwrap();
    set_item_time(&db, &item2.id, "2100-01-01T00:00:02Z");
    let conflict = ContextConflict {
        id: new_id(),
        workstream_id: ws2.id.clone(),
        left_item_id: item2.id.clone(),
        right_item_id: None,
        conflict_type: "user_disagreement".into(),
        status: "open".into(),
        resolution: None,
        created_at: now(),
        updated_at: now(),
        left_revision_id: item2.current_revision_id.clone(),
        right_revision_id: None,
        candidate_snapshot_json: None,
    };
    db.insert_conflict(&conflict).unwrap();
    set_conflict_time(&db, &conflict.id, "2100-01-01T00:00:03Z");

    // ws3 has no new changes

    let batch = db.list_workstream_review_summaries().unwrap();
    assert_eq!(batch.len(), 3);

    let s1 = batch.iter().find(|s| s.workstream_id == ws1.id).unwrap();
    let s2 = batch.iter().find(|s| s.workstream_id == ws2.id).unwrap();
    let s3 = batch.iter().find(|s| s.workstream_id == ws3.id).unwrap();

    assert_eq!(s1, &db.get_workstream_review_summary(&ws1.id).unwrap());
    assert_eq!(s2, &db.get_workstream_review_summary(&ws2.id).unwrap());
    assert_eq!(s3, &db.get_workstream_review_summary(&ws3.id).unwrap());

    assert!(s1.has_updates && !s1.needs_attention);
    assert!(s2.has_updates && s2.needs_attention);
    assert!(!s3.has_updates && !s3.needs_attention);
}
