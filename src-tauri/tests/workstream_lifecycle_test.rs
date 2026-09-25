//! Workstream lifecycle and the recycle bin (方案 §1.13, §18-8…§18-13).
//!
//! `lifecycle` (`active | completed`) is a label with no behaviour.
//! `visibility = archived` IS the recycle bin. Permanent deletion is the only
//! destructive door, it opens only from the bin, and what it destroys is the
//! Workstream and the rows only it owned — never a Session.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
mod support;

use noending::domain::{
    workstream_lifecycle, workstream_visibility, Agent, ContextConflict, ContextDelivery, Session,
    SessionMessageRole, Workstream,
};
use noending::error::Result;
use noending::storage::{new_id, now, Db};
use noending::workspace::workstream::{
    add_workstream_path, apply_whole_object_edit, archive_workstream, create_workstream,
    delete_workstream_permanently, restore_workstream, set_workstream_lifecycle,
};
use noending::workspace::{normalize_path, WorkspaceAttaching};
use rusqlite::{params, Connection};

// ------------------------------------------------------------- fixtures

fn temp_db() -> (PathBuf, Db) {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "noending-wslife-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("noending.db")).expect("temp db");
    (dir, db)
}

/// The minimal attacher these tests need: identity comes from the shared lexical
/// normalizer, and the row is written through the caller's connection.
struct FixedAttacher;

impl WorkspaceAttaching for FixedAttacher {
    fn ensure_path(&self, conn: &Connection, raw: &str) -> Result<Option<String>> {
        let canonical = normalize_path(raw).expect("fixture path");
        noending::storage::workspace::insert_workspace_path_conn(conn, &canonical, "p1").map(Some)
    }
}

fn workstream(db: &Db, id: &str) -> Workstream {
    db.upsert_project(&support::project("p1".into(), "P1"))
        .unwrap();
    let mut w = support::workstream(id.into(), id);
    w.id = id.into();
    db.upsert_workstream(&w).unwrap();
    add_workstream_path(db, &FixedAttacher, id, "/repo/main").unwrap();
    w
}

/// The row id is store-assigned (identity is the ROOT member's Agent-side
/// id), so callers use the returned Session instead of a fixture id.
fn session_row_at(
    db: &Db,
    tag: &str,
    cwd: Option<&str>,
    workspace_path_id: Option<&str>,
) -> Session {
    let (row_id, _) = db
        .upsert_logical_session(
            Agent::Codex,
            &format!("src-{tag}"),
            None,
            cwd,
            workspace_path_id,
            None,
            None,
            None,
        )
        .unwrap();
    db.get_session(&row_id).unwrap().expect("session row")
}

fn session(db: &Db, tag: &str) -> Session {
    session_row_at(db, tag, None, None)
}

fn set_owner(db: &Db, session_id: &str, workstream_id: &str) {
    db.set_session_owner(session_id, Some(workstream_id))
        .unwrap();
}

fn count(db: &Db, sql: &str, arg: &str) -> i64 {
    db.read()
        .query_row(sql, params![arg], |r| r.get::<_, i64>(0))
        .unwrap()
}

// ------------------------------------------------------------- lifecycle

/// §18 must-test — the label is a label: switching it changes the label.
#[test]
fn active_to_completed_changes_nothing_else() {
    let (_d, db) = temp_db();
    let w = workstream(&db, "w-label");
    let s = session(&db, "s-label");
    set_owner(&db, &s.id, &w.id);
    let item = noending::sync::create_item(
        &db,
        &w.id,
        "goal",
        "目标",
        "把列表做成唯一权威",
        "user_explicit",
        "manual",
        &[],
        None,
        "user",
    )
    .unwrap();
    let before = Snapshot::take(&db, &w.id);

    let after = set_workstream_lifecycle(&db, &w.id, workstream_lifecycle::COMPLETED).unwrap();
    assert_eq!(after.lifecycle, workstream_lifecycle::COMPLETED);
    // The label is the only field allowed to differ.
    let mut expected = before.clone();
    expected.lifecycle = workstream_lifecycle::COMPLETED.into();
    let completed = Snapshot::take(&db, &w.id);
    assert_eq!(completed, expected, "only the label moved");
    assert_eq!(completed.visibility, workstream_visibility::NORMAL);
    assert_eq!(completed.context_items, 1);
    assert_eq!(completed.revisions, 1);

    // …and back again, with the same quietness.
    set_workstream_lifecycle(&db, &w.id, workstream_lifecycle::ACTIVE).unwrap();
    let mut back = completed.clone();
    back.lifecycle = workstream_lifecycle::ACTIVE.into();
    let now_snapshot = Snapshot::take(&db, &w.id);
    assert_eq!(now_snapshot, back);
    assert_eq!(now_snapshot.created_at, before.created_at);
    assert_eq!(
        db.get_item(&item.id).unwrap().unwrap().authority,
        "user_explicit",
        "a label switch may not demote user authority"
    );
}

/// §1.13 — the vocabulary is `active | completed` and nothing else. `open` and
/// `abandoned` are outside that closed set; a caller still writing one is a bug,
/// and silently accepting it would put a value no reader understands in the
/// column (§42.2-E2/E10).
#[test]
fn lifecycle_rejects_the_retired_vocabulary() {
    let (_d, db) = temp_db();
    let w = workstream(&db, "w-vocab");
    for retired in ["open", "abandoned", "", "ACTIVE"] {
        let err = set_workstream_lifecycle(&db, &w.id, retired).unwrap_err();
        assert!(err.to_string().contains("active"), "{retired:?} → {err}");
    }
    assert_eq!(
        db.get_workstream(&w.id).unwrap().unwrap().lifecycle,
        workstream_lifecycle::ACTIVE
    );
    assert!(
        set_workstream_lifecycle(&db, "no-such-workstream", workstream_lifecycle::COMPLETED)
            .is_err()
    );
}

/// §18-9 — archived IS the bin: everything the user built is still there.
#[test]
fn archive_preserves_lifecycle_paths_and_ownership() {
    let (_d, db) = temp_db();
    let w = workstream(&db, "w-arch");
    set_workstream_lifecycle(&db, &w.id, workstream_lifecycle::COMPLETED).unwrap();
    let s = session(&db, "s-arch");
    set_owner(&db, &s.id, &w.id);
    add_workstream_path(&db, &FixedAttacher, &w.id, "/repo/docs").unwrap();
    let item = noending::sync::create_item(
        &db,
        &w.id,
        "current_state",
        "状态",
        "在回收站里也要在",
        "user_edit",
        "manual",
        &[],
        None,
        "user",
    )
    .unwrap();
    let before = Snapshot::take(&db, &w.id);

    let archived = archive_workstream(&db, &w.id).unwrap();
    assert_eq!(archived.visibility, workstream_visibility::ARCHIVED);
    // lifecycle survives the trip, which is why restore needs no snapshot (§1.13)
    assert_eq!(archived.lifecycle, workstream_lifecycle::COMPLETED);

    let after = Snapshot::take(&db, &w.id);
    assert_eq!(after.paths, before.paths);
    assert_eq!(after.owner_sessions, before.owner_sessions);
    assert_eq!(after.context_items, before.context_items);
    assert_eq!(after.revisions, before.revisions);
    assert_eq!(after.project_projection(), before.project_projection());
    assert!(db.get_workstream_review_state(&w.id).unwrap().is_some());
    assert_eq!(
        db.get_item(&item.id).unwrap().unwrap().current_revision_id,
        item.current_revision_id
    );
    // the Session is still a Session
    assert_eq!(db.get_session(&s.id).unwrap().unwrap().id, s.id);
}

/// §18-10 — restore flips visibility back and nothing else, so the lifecycle the
/// user had set before archiving is still the one they get.
#[test]
fn restore_returns_the_previous_lifecycle() {
    let (_d, db) = temp_db();
    let w = workstream(&db, "w-restore");
    for (i, lifecycle) in [
        workstream_lifecycle::ACTIVE,
        workstream_lifecycle::COMPLETED,
    ]
    .into_iter()
    .enumerate()
    {
        set_workstream_lifecycle(&db, &w.id, lifecycle).unwrap();
        // A distinct root identity per turn: upsert keys on it, so reusing one
        // would update the first row instead of making a second Session.
        let s = session(&db, &format!("s-restore-{i}"));
        set_owner(&db, &s.id, &w.id);
        let before = Snapshot::take(&db, &w.id);

        archive_workstream(&db, &w.id).unwrap();
        let restored = restore_workstream(&db, &w.id).unwrap();

        assert_eq!(restored.visibility, workstream_visibility::NORMAL);
        assert_eq!(restored.lifecycle, lifecycle, "round-tripped untouched");
        assert_eq!(Snapshot::take(&db, &w.id), before);
    }
}

/// Archive and restore are absolute, not toggles: the retired
/// `archive_workstream` flipped visibility, so a retry or a double click
/// silently un-archived what the user had just put in the bin.
#[test]
fn archiving_is_idempotent_and_never_a_toggle() {
    let (_d, db) = temp_db();
    let w = workstream(&db, "w-twice");
    for _ in 0..3 {
        assert_eq!(
            archive_workstream(&db, &w.id).unwrap().visibility,
            workstream_visibility::ARCHIVED
        );
    }
    for _ in 0..3 {
        assert_eq!(
            restore_workstream(&db, &w.id).unwrap().visibility,
            workstream_visibility::NORMAL
        );
    }
    assert!(archive_workstream(&db, "no-such-workstream").is_err());
    assert!(restore_workstream(&db, "no-such-workstream").is_err());
}

// -------------------------------------------------------- permanent deletion

/// §1.13 / §18-11 — the destructive door only opens from the bin.
#[test]
fn permanent_delete_is_refused_until_archived() {
    let (_d, db) = temp_db();
    let w = workstream(&db, "w-alive");

    let err = delete_workstream_permanently(&db, &w.id).unwrap_err();
    assert!(err.to_string().contains("回收站"), "{err}");
    // Refused means NOTHING was deleted: not the row, not its path, not its data.
    assert!(db.get_workstream(&w.id).unwrap().is_some());
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM workstream_paths WHERE workstream_id = ?1",
            &w.id
        ),
        1
    );
    assert_eq!(db.list_workspace_paths().unwrap().len(), 1);

    archive_workstream(&db, &w.id).unwrap();
    delete_workstream_permanently(&db, &w.id).unwrap();
    assert!(db.get_workstream(&w.id).unwrap().is_none());
    // a second delete is "not found", not a re-run of the purge
    assert!(delete_workstream_permanently(&db, &w.id).is_err());
}

/// §18-12 — the Workstream dies; the Sessions that worked on it do not.
#[test]
fn permanent_delete_preserves_sessions_and_their_events() {
    let (_d, db) = temp_db();
    let w = workstream(&db, "w-sessions");
    let path_id = add_workstream_path(&db, &FixedAttacher, &w.id, "/repo/docs")
        .unwrap()
        .workspace_path_id;
    let s = session_row_at(
        &db,
        "s-keep",
        Some(&normalize_path("/repo/docs").unwrap()),
        Some(&path_id),
    );
    set_owner(&db, &s.id, &w.id);
    // A two-message conversation, seeded through the production commit path.
    let member_id =
        support::ensure_root_member(&db, &s.id, Agent::Codex, "src-s-keep", "/raw/s-keep.jsonl");
    let messages = [
        support::parsed_message("m-1", SessionMessageRole::User, "first"),
        support::parsed_message("m-2", SessionMessageRole::Assistant, "second"),
    ];
    db.commit_member_ingest(&s.id, &member_id, &messages, None, &support::seed_source(0))
        .unwrap();
    db.set_processed_message_sequence(&s.id, 2).unwrap();
    // A LaunchIntent naming this Workstream is historical evidence: §42.3-M24
    // forbids a fourth path column on it, and M6 forbids touching it at all.
    let intent = noending::domain::LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent: Agent::Codex,
        owner_workstream_id: Some(w.id.clone()),
        cwd: Some("/repo/docs".into()),
        context_bundle_markdown: None,
        context_bundle_revisions: None,
        process_id: None,
        launched_at: now(),
        matched_session_id: Some(s.id.clone()),
        status: noending::domain::launch_status::MATCHED.into(),
        note: String::new(),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_launch_intent(&intent).unwrap();

    archive_workstream(&db, &w.id).unwrap();
    delete_workstream_permanently(&db, &w.id).unwrap();

    let kept = db
        .get_session(&s.id)
        .unwrap()
        .expect("the Session survives");
    assert_eq!(kept.cwd, Some(normalize_path("/repo/docs").unwrap()));
    // Its own physical facts are untouched: the WorkspacePath it reads through
    // and the derived Project cache both stay (they are not the Workstream's).
    assert_eq!(kept.workspace_path_id, Some(path_id.clone()));
    assert_eq!(
        db.get_workspace_path(&path_id).unwrap().unwrap().project_id,
        "p1"
    );
    assert_eq!(db.message_count(&s.id).unwrap(), 2, "append-only history");
    assert_eq!(
        db.get_context_state(&s.id)
            .unwrap()
            .processed_message_sequence,
        2,
        "context frontier"
    );
    assert_eq!(
        db.get_launch_intent(&intent.id).unwrap().unwrap().status,
        noending::domain::launch_status::MATCHED
    );
    assert_eq!(db.list_workspace_paths().unwrap().len(), 2);
    assert!(
        db.get_project("p1").unwrap().is_some(),
        "a Project is not a Workstream's property"
    );
    // what went is only what the Workstream owned
    assert!(
        kept.owner_workstream_id.is_none(),
        "permanent delete clears the owner via ON DELETE SET NULL"
    );
    assert!(db.sessions_for_workstream(&w.id).unwrap().is_empty());
    assert!(db.list_workstream_paths(&w.id).unwrap().is_empty());
}

/// §42.3-M6 — the cleanup list has to be COMPLETE or the first real delete dies
/// on a foreign key. This builds one row of every kind a Workstream can own and
/// deletes it all, then checks each of them by hand.
#[test]
fn permanent_delete_clears_every_row_the_workstream_owns() {
    let (_d, db) = temp_db();
    let w = workstream(&db, "w-every");
    let s = session(&db, "s-every");
    add_workstream_path(&db, &FixedAttacher, &w.id, "/repo/docs").unwrap();
    let rows = db.list_workstream_paths(&w.id).unwrap();

    // context item + two revisions + the search row create_item writes
    let item = noending::sync::create_item(
        &db,
        &w.id,
        "goal",
        "目标",
        "一版",
        "user_explicit",
        "manual",
        &[],
        None,
        "user",
    )
    .unwrap();
    db.apply_status_change(&item.id, "resolved", "user", "改主意了", None, &[])
        .unwrap();
    // conflict + its audited resolution event
    let second = noending::sync::create_item(
        &db,
        &w.id,
        "goal",
        "另一版",
        "二版",
        "system_observed",
        "agent_statement",
        &[],
        None,
        "agent",
    )
    .unwrap();
    let conflict = ContextConflict {
        id: new_id(),
        workstream_id: w.id.clone(),
        left_item_id: item.id.clone(),
        right_item_id: Some(second.id.clone()),
        conflict_type: "authority".into(),
        status: "open".into(),
        resolution: None,
        created_at: now(),
        updated_at: now(),
        left_revision_id: item.current_revision_id.clone(),
        right_revision_id: second.current_revision_id.clone(),
        candidate_snapshot_json: None,
    };
    db.insert_conflict(&conflict).unwrap();
    db.update_conflict_status(&conflict.id, "resolved", Some("保留用户版"))
        .unwrap();
    // delivery snapshot and review state
    db.record_delivery(&ContextDelivery {
        id: new_id(),
        session_id: s.id.clone(),
        workstream_id: w.id.clone(),
        bundle_id: "b-1".into(),
        delivered_revisions: vec![item.current_revision_id.clone().unwrap()],
        delivered_conflicts: vec![conflict.id.clone()],
        delivered_at: now(),
    })
    .unwrap();
    set_owner(&db, &s.id, &w.id);
    let rejected = session(&db, "s-rejected");
    set_owner(&db, &rejected.id, &w.id);
    assert!(db.get_workstream_review_state(&w.id).unwrap().is_some());

    // Everything is there before the delete…
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM context_items WHERE workstream_id = ?1",
            &w.id
        ),
        2
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM context_conflicts WHERE workstream_id = ?1",
            &w.id
        ),
        1
    );
    assert_eq!(db.conflict_history(&conflict.id).unwrap().len(), 1);
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM workstream_paths WHERE workstream_id = ?1",
            &w.id
        ),
        2
    );

    archive_workstream(&db, &w.id).unwrap();
    delete_workstream_permanently(&db, &w.id).unwrap();

    // …and all of it is gone, in the order §42.3-M6 fixes.
    for (label, sql) in [
        ("conflict events", "SELECT COUNT(*) FROM context_conflict_events WHERE conflict_id IN (SELECT id FROM context_conflicts WHERE workstream_id = ?1)"),
        ("conflicts", "SELECT COUNT(*) FROM context_conflicts WHERE workstream_id = ?1"),
        ("revisions", "SELECT COUNT(*) FROM context_item_revisions WHERE item_id IN (SELECT id FROM context_items WHERE workstream_id = ?1)"),
        ("items", "SELECT COUNT(*) FROM context_items WHERE workstream_id = ?1"),
        ("deliveries", "SELECT COUNT(*) FROM context_deliveries WHERE workstream_id = ?1"),
        ("workstream paths", "SELECT COUNT(*) FROM workstream_paths WHERE workstream_id = ?1"),
        ("review state", "SELECT COUNT(*) FROM workstream_review_state WHERE workstream_id = ?1"),
        ("the workstream", "SELECT COUNT(*) FROM workstreams WHERE id = ?1"),
    ] {
        assert_eq!(count(&db, sql, &w.id), 0, "{label} must be gone");
    }
    // The Workstream's own search row and its items' rows both have to go with
    // them, or search keeps answering with facts nobody owns any more.
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM search_index WHERE ref_id = ?1 OR (kind = 'item' AND parent_id = ?1)",
            &w.id
        ),
        0,
        "search rows must be gone"
    );
    // The Sessions, their events, their cursors and the physical paths all live.
    assert!(db.get_session(&s.id).unwrap().is_some());
    assert!(db.get_session(&rejected.id).unwrap().is_some());
    // …and both lost their Owner (ON DELETE SET NULL), not their identity.
    assert!(db
        .get_session(&s.id)
        .unwrap()
        .unwrap()
        .owner_workstream_id
        .is_none());
    assert!(db
        .get_session(&rejected.id)
        .unwrap()
        .unwrap()
        .owner_workstream_id
        .is_none());
    assert_eq!(db.list_workspace_paths().unwrap().len(), 2);
    assert!(db.get_project("p1").unwrap().is_some());
    // Sanity: the two paths we purged really were this Workstream's.
    assert!(rows.len() == 2);
}

/// Deleting one Workstream must not reach into a sibling that happens to share a
/// Session or a WorkspacePath.
#[test]
fn permanent_delete_leaves_a_sibling_alone() {
    let (_d, db) = temp_db();
    let a = workstream(&db, "w-a");
    let b = workstream(&db, "w-b");
    let shared = session(&db, "s-shared");
    set_owner(&db, &shared.id, &b.id);
    let shared_path = db.list_workstream_paths(&a.id).unwrap()[0]
        .workspace_path_id
        .clone();
    assert_eq!(
        db.list_workstream_paths(&b.id).unwrap()[0].workspace_path_id,
        shared_path,
        "both work on /repo/main, which is one WorkspacePath"
    );
    noending::sync::create_item(
        &db,
        &b.id,
        "goal",
        "留在世上",
        "别删我",
        "user_explicit",
        "manual",
        &[],
        None,
        "user",
    )
    .unwrap();

    archive_workstream(&db, &a.id).unwrap();
    delete_workstream_permanently(&db, &a.id).unwrap();

    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM workstreams WHERE id = ?1", &b.id),
        1
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM context_items WHERE workstream_id = ?1",
            &b.id
        ),
        1
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM workstream_paths WHERE workstream_id = ?1",
            &b.id
        ),
        1
    );
    // b's ownership of the shared Session survived, and the Session still points at b
    let kept = db
        .get_session(&shared.id)
        .unwrap()
        .expect("session survives");
    assert_eq!(kept.owner_workstream_id.as_deref(), Some(b.id.as_str()));
    assert_eq!(db.sessions_for_workstream(&b.id).unwrap().len(), 1);
    // the shared physical path is not a's to delete
    assert!(db.get_workspace_path(&shared_path).unwrap().is_some());
}

/// A `create_workstream` with an initial path, archived and purged: the whole
/// lifecycle in one pass, as the product runs it (§22's flow).
#[test]
fn created_archived_and_purged_leaves_no_trace_of_itself() {
    let (_d, db) = temp_db();
    db.upsert_project(&support::project("p1".into(), "P1"))
        .unwrap();
    let w = create_workstream(&db, &FixedAttacher, "临时", "", &["/repo/tmp".into()])
        .unwrap()
        .workstream;
    assert_eq!(db.list_workstream_paths(&w.id).unwrap().len(), 1);
    archive_workstream(&db, &w.id).unwrap();
    delete_workstream_permanently(&db, &w.id).unwrap();
    assert!(db.get_workstream(&w.id).unwrap().is_none());
    // The WorkspacePath row survives (a physical fact, GC is workspace::project's)
    // and so does the Project that owns it.
    assert_eq!(db.list_workspace_paths().unwrap().len(), 1);
    assert_eq!(db.get_project("p1").unwrap().unwrap().id, "p1");
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM search_index WHERE ref_id = ?1",
            &w.id
        ),
        0
    );
}

/// The whole-object write is what silently reverted an archive before:
/// every screen echoes the object it loaded, so a stale one could restore or
/// retire a Workstream nobody had asked about. Title and description still
/// travel; the two state fields and the two frozen columns do not.
#[test]
fn whole_object_write_cannot_change_lifecycle_or_visibility() {
    let (_d, db) = temp_db();
    let w = workstream(&db, "w-edit");
    let mut stale = db.get_workstream(&w.id).unwrap().unwrap();

    // A title edit through the whole-object door still works.
    stale.title = "改过名".into();
    let edited = apply_whole_object_edit(&db, &stale).unwrap();
    db.upsert_workstream(&edited).unwrap();
    assert_eq!(db.get_workstream(&w.id).unwrap().unwrap().title, "改过名");

    // …but an object loaded before the archive cannot un-archive it.
    archive_workstream(&db, &w.id).unwrap();
    stale.visibility = workstream_visibility::NORMAL.into();
    let err = apply_whole_object_edit(&db, &stale).unwrap_err();
    assert!(err.to_string().contains("restore_workstream"), "{err}");
    assert_eq!(
        db.get_workstream(&w.id).unwrap().unwrap().visibility,
        workstream_visibility::ARCHIVED
    );

    // Nor can it change the lifecycle by hand.
    let mut stale = db.get_workstream(&w.id).unwrap().unwrap();
    stale.lifecycle = workstream_lifecycle::COMPLETED.into();
    assert!(apply_whole_object_edit(&db, &stale)
        .unwrap_err()
        .to_string()
        .contains("set_workstream_lifecycle"));

    // A write of a row that does not exist is refused instead of creating one.
    let ghost = support::workstream("w-ghost".into(), "ghost");
    assert!(apply_whole_object_edit(&db, &ghost).is_err());
}

/// A snapshot of everything that must NOT move when a label or a bin flip does.
/// `updated_at` is deliberately absent: it is the audit timestamp of the write
/// itself, not domain state.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    lifecycle: String,
    visibility: String,
    title: String,
    description: String,
    created_at: String,
    paths: Vec<(String, i64)>,
    owner_sessions: Vec<String>,
    context_items: i64,
    revisions: i64,
    deliveries: i64,
    review_state: i64,
}

impl Snapshot {
    fn take(db: &Db, workstream_id: &str) -> Self {
        let w = db.get_workstream(workstream_id).unwrap().unwrap();
        let owner_sessions: Vec<String> = db
            .sessions_for_workstream(workstream_id)
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        Self {
            created_at: w.created_at,
            title: w.title,
            description: w.description,
            lifecycle: w.lifecycle,
            visibility: w.visibility,
            paths: db
                .list_workstream_paths(workstream_id)
                .unwrap()
                .into_iter()
                .map(|p| (p.workspace_path_id, p.position))
                .collect(),
            owner_sessions,
            context_items: count(db, "SELECT COUNT(*) FROM context_items WHERE workstream_id = ?1", workstream_id),
            revisions: count(db, "SELECT COUNT(*) FROM context_item_revisions WHERE item_id IN (SELECT id FROM context_items WHERE workstream_id = ?1)", workstream_id),
            deliveries: count(db, "SELECT COUNT(*) FROM context_deliveries WHERE workstream_id = ?1", workstream_id),
            review_state: count(db, "SELECT COUNT(*) FROM workstream_review_state WHERE workstream_id = ?1", workstream_id),
        }
    }
    /// The projection every card and detail page publishes (§42.3-M19).
    fn project_projection(&self) -> Option<String> {
        self.paths.first().map(|(id, _)| id.clone())
    }
}
