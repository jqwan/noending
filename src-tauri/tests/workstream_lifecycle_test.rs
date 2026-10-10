//! Workstream lifecycle and the recycle bin.
//!
//! `visibility = archived` IS the recycle bin. Permanent deletion is the only
//! destructive door, it opens only from the bin, and what it destroys is the
//! Workstream and the rows only it owned — never a Session.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
mod support;

use noending::domain::{workstream_visibility, Agent, Session, SessionMessageRole, Workstream};
use noending::error::Result;
use noending::storage::Db;
use noending::workspace::workstream::{
    add_workstream_path, apply_whole_object_edit, archive_workstream, create_workstream,
    delete_workstream_permanently, restore_workstream,
};
use noending::workspace::{normalize_path, WorkspaceAttaching};
use rusqlite::{params, Connection};

// fixtures

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
        .upsert_logical_session_unchecked(
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

/// archived IS the bin: everything the user built is still there.
#[test]
fn archive_preserves_paths_context_and_ownership() {
    let (_d, db) = temp_db();
    let w = workstream(&db, "w-arch");
    let s = session(&db, "s-arch");
    set_owner(&db, &s.id, &w.id);
    add_workstream_path(&db, &FixedAttacher, &w.id, "/repo/docs").unwrap();
    db.tx(|tx| {
        noending::storage::context_repo::commit_workstream_summary_conn(
            tx,
            &w.id,
            "在已归档列表中也要在",
        )
    })
    .unwrap();
    let before = Snapshot::take(&db, &w.id);

    let archived = archive_workstream(&db, &w.id).unwrap();
    assert_eq!(archived.visibility, workstream_visibility::ARCHIVED);

    let after = Snapshot::take(&db, &w.id);
    assert_eq!(after.paths, before.paths);
    assert_eq!(after.owner_sessions, before.owner_sessions);
    assert_eq!(after.summary, before.summary);
    // the Session is still a Session
    assert_eq!(db.get_session(&s.id).unwrap().unwrap().id, s.id);
}

/// Unarchiving restores the same task and all its associations.
#[test]
fn restore_preserves_task_data() {
    let (_d, db) = temp_db();
    let w = workstream(&db, "w-restore");
    let s = session(&db, "s-restore");
    set_owner(&db, &s.id, &w.id);
    let before = Snapshot::take(&db, &w.id);
    archive_workstream(&db, &w.id).unwrap();
    let restored = restore_workstream(&db, &w.id).unwrap();
    assert_eq!(restored.visibility, workstream_visibility::NORMAL);
    assert_eq!(Snapshot::take(&db, &w.id), before);
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

// permanent deletion

/// / the destructive door only opens from the bin.
#[test]
fn permanent_delete_is_refused_until_archived() {
    let (_d, db) = temp_db();
    let w = workstream(&db, "w-alive");

    let err = delete_workstream_permanently(&db, &w.id).unwrap_err();
    assert!(err.to_string().contains("已归档"), "{err}");
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

/// the Workstream dies; the Sessions that worked on it do not.
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
    let messages = [
        support::parsed_message("m-1", SessionMessageRole::User, "first"),
        support::parsed_message("m-2", SessionMessageRole::Assistant, "second"),
    ];
    db.commit_ingest(&s.id, &messages, &support::seed_source(0))
        .unwrap();
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
        db.get_session_ingest_state(&s.id).unwrap().1,
        2,
        "the fact frontier survives the purge"
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

/// the cleanup list has to be COMPLETE or the first real delete dies
/// on a foreign key. This builds one row of every kind a Workstream can own and
/// deletes it all, then checks each of them by hand.
#[test]
fn permanent_delete_clears_every_row_the_workstream_owns() {
    let (_d, db) = temp_db();
    let w = workstream(&db, "w-every");
    let s = session(&db, "s-every");
    set_owner(&db, &s.id, &w.id);
    add_workstream_path(&db, &FixedAttacher, &w.id, "/repo/docs").unwrap();
    db.tx(|tx| {
        noending::storage::context_repo::commit_workstream_summary_conn(tx, &w.id, "当前任务摘要")?;
        noending::storage::context_repo::set_workstream_frontier_conn(
            tx,
            &noending::domain::WorkstreamSessionFrontier {
                workstream_id: w.id.clone(),
                session_id: s.id.clone(),
                session_context_revision: 0,
            },
        )
    })
    .unwrap();
    archive_workstream(&db, &w.id).unwrap();
    delete_workstream_permanently(&db, &w.id).unwrap();
    for table in [
        "workstream_paths",
        "workstream_context_state",
        "workstream_session_frontiers",
    ] {
        assert_eq!(
            count(
                &db,
                &format!("SELECT COUNT(*) FROM {table} WHERE workstream_id = ?1"),
                &w.id
            ),
            0
        );
    }
    assert!(db.get_workstream(&w.id).unwrap().is_none());
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM search_index WHERE ref_id = ?1",
            &w.id
        ),
        0
    );
    assert!(db
        .get_session(&s.id)
        .unwrap()
        .unwrap()
        .owner_workstream_id
        .is_none());
    assert_eq!(db.list_workspace_paths().unwrap().len(), 2);
    assert!(db.get_project("p1").unwrap().is_some());
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
    db.tx(|tx| {
        noending::storage::context_repo::commit_workstream_summary_conn(tx, &b.id, "别删我")
    })
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
            "SELECT COUNT(*) FROM workstream_context_state WHERE workstream_id = ?1",
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
/// lifecycle in one pass, as the product runs it ('s flow).
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
fn whole_object_write_cannot_change_archive_state() {
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

    // A write of a row that does not exist is refused instead of creating one.
    let ghost = support::workstream("w-ghost".into(), "ghost");
    assert!(apply_whole_object_edit(&db, &ghost).is_err());
}

/// A snapshot of everything that must NOT move when a label or a bin flip does.
/// `updated_at` is deliberately absent: it is the audit timestamp of the write
/// itself, not domain state.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    visibility: String,
    title: String,
    description: String,
    created_at: String,
    paths: Vec<(String, i64)>,
    owner_sessions: Vec<String>,
    summary: String,
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
            visibility: w.visibility,
            paths: db
                .list_workstream_paths(workstream_id)
                .unwrap()
                .into_iter()
                .map(|p| (p.workspace_path_id, p.position))
                .collect(),
            owner_sessions,
            summary: db
                .get_workstream_context_state(workstream_id)
                .unwrap()
                .summary,
        }
    }
}
