//! Session single-Owner invariant suite: every test pins one rule of
//! `Session ── 0..1 ──> Owner Workstream`.
//!
//! The point is not the happy path alone but that the two directions stay
//! INDEPENDENT: setting an Owner never mutates the Workstream's path list, and
//! mutating the path list never changes an Owner.
//!
//! A Session is keyed by its ROOT member's Resume identity
//! (`root_agent_session_id`); conversation is seeded through the production
//! commit path (`commit_member_ingest`) and read back through `get_messages`.

use noending::domain::{Agent, Session, Workstream};
use noending::launcher::{LaunchWorkspace, SessionLauncher};
use noending::storage::workspace::insert_workspace_path_conn;
use noending::storage::{new_id, now, Db};
use noending::workspace::workstream::{
    add_workstream_path, archive_workstream, create_workstream, delete_workstream_permanently,
};
use noending::workspace::WorkspaceAttaching;
use rusqlite::Connection;
use std::ops::Deref;
mod support;

// harness

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
    let dir = std::env::temp_dir().join(format!("noending-owner-{}-{}", tag, new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).unwrap();
    TestDb { db: Some(db), dir }
}

/// Real directories inside the test's own temp dir, attached to one Project
/// through the production WorkspacePath door. `ensure_path` refuses a path that
/// is not an existing directory, exactly like the real attacher's usability
/// gate — a cwd tier that does not exist must not be launched into.
struct TempPaths {
    root: std::path::PathBuf,
    project_id: String,
}

impl TempPaths {
    /// `project_id` must already exist: `workspace_paths.project_id` is a real
    /// foreign key, and a path attached to a phantom Project is exactly the
    /// kind of fabricated fact the workspace layer refuses to store.
    fn new(root: &std::path::Path, project_id: &str) -> Self {
        Self {
            root: root.to_path_buf(),
            project_id: project_id.to_string(),
        }
    }

    fn dir(&self, name: &str) -> String {
        let p = self.root.join(name);
        std::fs::create_dir_all(&p).unwrap();
        p.to_string_lossy().to_string()
    }
}

impl WorkspaceAttaching for TempPaths {
    fn ensure_path(&self, conn: &Connection, raw: &str) -> noending::error::Result<Option<String>> {
        if !std::path::Path::new(raw).is_dir() {
            return Ok(None);
        }
        Ok(Some(insert_workspace_path_conn(
            conn,
            raw,
            &self.project_id,
        )?))
    }
}

fn seed_project(db: &Db, id: &str) {
    db.upsert_project(&support::project(id.to_string(), "Owner Model Test"))
        .unwrap();
}

fn workstream(db: &Db, title: &str) -> Workstream {
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

/// A Logical Session + its ROOT member. The root source is a REAL file inside
/// the test's temp dir, because resume preparation refuses a Session whose
/// ROOT member source is not present on disk — a resumable fixture
/// must be resumable.
fn session(db: &TestDb, cwd: Option<&str>) -> Session {
    session_of_agent(db, Agent::Codex, cwd)
}

fn session_of_agent(db: &TestDb, agent: Agent, cwd: Option<&str>) -> Session {
    let root_id = format!("root-{}", new_id());
    let raw = db.dir.join(format!("raw-{}.jsonl", new_id()));
    std::fs::write(&raw, "").unwrap();
    let ts = now();
    let (id, _) = db
        .upsert_logical_session_unchecked(
            agent,
            &root_id,
            Some("Owner Model"),
            cwd,
            None,
            None,
            Some(&ts),
            Some(&ts),
        )
        .unwrap();
    support::ensure_session_source(db, agent, &root_id, &raw.to_string_lossy());
    db.get_session(&id).unwrap().unwrap()
}

fn owner_of(db: &Db, session_id: &str) -> Option<String> {
    db.get_session(session_id)
        .unwrap()
        .unwrap()
        .owner_workstream_id
}

// Session Owner basics

#[test]
fn owner_assignment_replaces_clears_and_is_idempotent() {
    let db = open_db("owner-replace");
    let a = workstream(&db, "A");
    let b = workstream(&db, "B");
    let s = session(&db, None);
    assert!(owner_of(&db, &s.id).is_none());

    db.set_session_owner(&s.id, Some(&a.id)).unwrap();
    assert_eq!(owner_of(&db, &s.id).as_deref(), Some(a.id.as_str()));

    // Replacing is a REPLACEMENT, not an addition: A + B is unrepresentable.
    db.set_session_owner(&s.id, Some(&b.id)).unwrap();
    assert_eq!(owner_of(&db, &s.id).as_deref(), Some(b.id.as_str()));
    assert!(db.sessions_for_workstream(&a.id).unwrap().is_empty());
    assert_eq!(db.sessions_for_workstream(&b.id).unwrap().len(), 1);

    db.set_session_owner(&s.id, Some(&b.id)).unwrap();
    assert_eq!(owner_of(&db, &s.id).as_deref(), Some(b.id.as_str()));
    assert_eq!(db.sessions_for_workstream(&b.id).unwrap().len(), 1);

    db.set_session_owner(&s.id, None).unwrap();
    assert!(owner_of(&db, &s.id).is_none());
    assert!(db.sessions_for_workstream(&b.id).unwrap().is_empty());
}

#[test]
fn unknown_workstream_and_unknown_session_are_refused() {
    let db = open_db("owner-refuse");
    let s = session(&db, None);
    assert!(db.set_session_owner(&s.id, Some("no-such-ws")).is_err());
    assert!(db.set_session_owner("no-such-session", None).is_err());
}

// workspace independence

#[test]
fn adding_a_workstream_path_does_not_change_the_owner() {
    let db = open_db("owner-path-add");
    seed_project(&db, "p1");
    let paths = TempPaths::new(&db.dir, "p1");
    let first = paths.dir("repo-a");
    let second = paths.dir("repo-b");
    let w = create_workstream(&db, &paths, "A", "", &[first.clone()])
        .unwrap()
        .workstream;
    let s = session(&db, Some(first.as_str()));
    db.set_session_owner(&s.id, Some(&w.id)).unwrap();

    add_workstream_path(&db, &paths, &w.id, &second).unwrap();

    assert_eq!(db.list_workstream_paths(&w.id).unwrap().len(), 2);
    assert_eq!(owner_of(&db, &s.id).as_deref(), Some(w.id.as_str()));
}

#[test]
fn changing_the_session_cwd_or_project_does_not_change_the_owner() {
    let db = open_db("owner-cwd-drift");
    seed_project(&db, "p1");
    let paths = TempPaths::new(&db.dir, "p1");
    let a_dir = paths.dir("repo-a");
    let b_dir = paths.dir("repo-b");
    let w = workstream(&db, "A");
    let s = session(&db, Some(&a_dir));
    noending::workspace::session::attach_session_conn(&db.read(), &paths, &s.id, Some(&a_dir))
        .unwrap();
    db.set_session_owner(&s.id, Some(&w.id)).unwrap();

    // The Session's physical facts move to another directory (and therefore
    // another WorkspacePath / Project). Ownership is a separate statement.
    db.upsert_logical_session(
        s.agent,
        &s.root_agent_session_id,
        None,
        None,
        Some(&b_dir),
        None,
        None,
        None,
        None,
        &s.source_kind,
        &s.source_path,
        &s.metadata,
    )
    .unwrap();
    seed_project(&db, "p2");
    let b_path = db
        .tx(|tx| TempPaths::new(&db.dir, "p2").ensure_path(tx, &b_dir))
        .unwrap()
        .unwrap();
    db.tx(|tx| noending::workspace::session::move_session_to_path_conn(tx, &s.id, &b_path))
        .unwrap();

    let after = db.get_session(&s.id).unwrap().unwrap();
    assert_eq!(after.workspace_path_id.as_deref(), Some(b_path.as_str()));
    assert_eq!(after.cwd.as_deref(), Some(b_dir.as_str()));
    assert_eq!(after.project_id.as_deref(), Some("p2"));
    assert_eq!(after.owner_workstream_id.as_deref(), Some(w.id.as_str()));
}

// launcher

#[test]
fn new_session_preparation_carries_the_chosen_owner() {
    let db = open_db("owner-launch-new");
    seed_project(&db, "p1");
    let paths = TempPaths::new(&db.dir, "p1");
    let dir = paths.dir("repo-a");
    let w = create_workstream(&db, &paths, "A", "", &[dir.clone()])
        .unwrap()
        .workstream;

    let workspace = LaunchWorkspace {
        default_workspace: None,
    };
    let prepared = SessionLauncher {
        runtime_dir: db.dir.join("runtime"),
    }
    .prepare_new_in(&db, Agent::Codex, Some(&w.id), None, &workspace)
    .unwrap();

    assert_eq!(prepared.owner_workstream_id.as_deref(), Some(w.id.as_str()));
    // Task ownership does not imply a launch directory.
    assert_eq!(prepared.cwd, None);
}

#[test]
fn ownerless_session_with_no_usable_cwd_resolves_to_unresolved() {
    let db = open_db("owner-unresolved");
    let s = session(&db, Some(&db.dir.join("vanished").to_string_lossy()));
    let workspace = LaunchWorkspace {
        default_workspace: None,
    };
    let resolution = noending::launcher::resolve_resume_cwd(&db, &s.id, &workspace).unwrap();
    assert_eq!(resolution.source, noending::launcher::CwdSource::Unresolved);
}

// the owner side-effect rule

/// `set_session_owner` writes `sessions.owner_workstream_id` and
/// NOTHING else. The Workstream's path list is untouched (no path is appended
/// on the Session's behalf), and the Session's cwd, `workspace_path_id` and
/// derived `project_id` stay exactly as they were.
#[test]
fn setting_the_owner_never_adds_a_workstream_path_or_moves_the_session() {
    let db = open_db("owner-no-side-effects");
    seed_project(&db, "p1");
    let paths = TempPaths::new(&db.dir, "p1");
    let dir = paths.dir("repo-a");
    let w = workstream(&db, "A");
    let s = session(&db, Some(&dir));
    noending::workspace::session::attach_session_conn(&db.read(), &paths, &s.id, Some(&dir))
        .unwrap();

    let before = db.get_session(&s.id).unwrap().unwrap();
    assert!(before.workspace_path_id.is_some(), "attached for the test");
    assert_eq!(before.project_id.as_deref(), Some("p1"));

    db.set_session_owner(&s.id, Some(&w.id)).unwrap();

    assert_eq!(owner_of(&db, &s.id).as_deref(), Some(w.id.as_str()));
    assert!(
        db.list_workstream_paths(&w.id).unwrap().is_empty(),
        "the user never chose a directory, so the list must not grow"
    );
    let after = db.get_session(&s.id).unwrap().unwrap();
    assert_eq!(after.cwd, before.cwd);
    assert_eq!(after.workspace_path_id, before.workspace_path_id);
    assert_eq!(after.project_id, before.project_id);
}

/// The clearing direction is equally inert: dropping the Owner leaves the
/// Session's physical facts and the Workstream's path list alone.
#[test]
fn clearing_the_owner_moves_nothing() {
    let db = open_db("owner-clear-inert");
    seed_project(&db, "p1");
    let paths = TempPaths::new(&db.dir, "p1");
    let dir = paths.dir("repo-a");
    let w = workstream(&db, "A");
    let s = session(&db, Some(&dir));
    noending::workspace::session::attach_session_conn(&db.read(), &paths, &s.id, Some(&dir))
        .unwrap();
    db.set_session_owner(&s.id, Some(&w.id)).unwrap();

    let before = db.get_session(&s.id).unwrap().unwrap();
    db.set_session_owner(&s.id, None).unwrap();

    assert!(owner_of(&db, &s.id).is_none());
    assert!(db.list_workstream_paths(&w.id).unwrap().is_empty());
    let after = db.get_session(&s.id).unwrap().unwrap();
    assert_eq!(after.cwd, before.cwd);
    assert_eq!(after.workspace_path_id, before.workspace_path_id);
}

// Workstream statistics

/// a Workstream's stats count exactly the Sessions that OWN it. An
/// unowned Session is counted by no Workstream, and re-homing a Session moves
/// its count instead of duplicating it.
#[test]
fn workstream_session_stats_count_only_owned_sessions() {
    let db = open_db("ws-stats-owner");
    let a = workstream(&db, "A");
    let b = workstream(&db, "B");
    let owned_a = session(&db, None);
    let owned_a2 = session(&db, None);
    let stray = session(&db, None);
    db.set_session_owner(&owned_a.id, Some(&a.id)).unwrap();
    db.set_session_owner(&owned_a2.id, Some(&a.id)).unwrap();

    let (count, latest, _) = db.workstream_session_stats(&a.id).unwrap();
    assert_eq!(count, 2);
    assert!(latest.is_some());
    assert_eq!(db.workstream_session_stats(&b.id).unwrap().0, 0);
    assert!(
        db.sessions_for_workstream(&a.id)
            .unwrap()
            .iter()
            .all(|s| s.id != stray.id),
        "an unowned Session belongs to no Workstream"
    );

    // Re-home one Session: the count MOVES, it is never duplicated.
    db.set_session_owner(&owned_a2.id, Some(&b.id)).unwrap();
    assert_eq!(db.workstream_session_stats(&a.id).unwrap().0, 1);
    assert_eq!(db.workstream_session_stats(&b.id).unwrap().0, 1);
}

// the Owner is the only write target

///  — a run writes its Owner Workstream and nothing else.
///
/// `update` / `supersede` / `resolve` name their target by `item_id`, and that
/// id comes from model output: untrusted text that may echo an identifier
/// found anywhere in the transcript. The boundary therefore lives in the merge
/// engine, on the deterministic side — an id from another Workstream is a
/// skip, not a way in.
#[test]
fn resume_preparation_follows_the_current_owner_after_the_sync_seam() {
    let db = open_db("owner-resume-fresh");
    seed_project(&db, "p1");
    let paths = TempPaths::new(&db.dir, "p1");
    let dir_a = paths.dir("repo-a");
    let dir_b = paths.dir("repo-b");
    let a = create_workstream(&db, &paths, "A", "", &[dir_a.clone()])
        .unwrap()
        .workstream;
    let b = create_workstream(&db, &paths, "B", "", &[dir_b.clone()])
        .unwrap()
        .workstream;

    // The Session cwd is gone; both owners must use the same default workspace.
    // The source file is present, so the resume gate passes.
    let s = session(&db, Some("/gone/workspace"));
    db.set_session_owner(&s.id, Some(&a.id)).unwrap();

    let launcher = SessionLauncher {
        runtime_dir: db.dir.join("runtime"),
    };
    let workspace = LaunchWorkspace {
        default_workspace: Some(
            db.dir
                .join("default-workspace")
                .to_string_lossy()
                .into_owned(),
        ),
    };

    let first = launcher
        .prepare_resume_from_current(&db, &s.id, &workspace)
        .unwrap();
    assert_eq!(first.owner_workstream_id.as_deref(), Some(a.id.as_str()));
    assert_eq!(first.cwd, workspace.default_workspace);

    // Ownership moves during the window in which sync would have been running.
    db.set_session_owner(&s.id, Some(&b.id)).unwrap();

    let second = launcher
        .prepare_resume_from_current(&db, &s.id, &workspace)
        .unwrap();
    assert_eq!(second.owner_workstream_id.as_deref(), Some(b.id.as_str()));
    assert_eq!(
        second.cwd.as_deref(),
        workspace.default_workspace.as_deref(),
        "changing Owner does not change the launch directory"
    );

    // And the end-to-end entry point (which syncs first) agrees.
    let full = launcher.prepare_resume_in(&db, &s.id, &workspace).unwrap();
    assert_eq!(full.owner_workstream_id.as_deref(), Some(b.id.as_str()));
    assert_eq!(full.cwd, workspace.default_workspace);
}

// first discovery, whichever door found the session

// a match is one atomic ownership handover

// prepare-time consistency (Preview = Launch)

/// The consistency predicate a Resume preparation ends with: it may only hand
/// back a plan while the Session still names the Owner it was built for.
///
/// Read against the live row, because that is what the predicate does: the
/// fingerprint cannot stand in for it — it hashes the Owner the bundle was
/// built for AND the Owner currently on the row, so an ownership move while the
/// bundle renders produces the very same hash on both sides.
#[test]
fn preparation_matches_current_owner_tracks_the_row() {
    use noending::launcher::preparation_matches_current_owner;

    let db = open_db("prepare-owner-cas");
    let a = workstream(&db, "A");
    let b = workstream(&db, "B");
    let s = session(&db, None);

    // Unowned: only "no Owner" matches.
    assert!(preparation_matches_current_owner(&db, &s.id, None).unwrap());
    assert!(!preparation_matches_current_owner(&db, &s.id, Some(&a.id)).unwrap());

    db.set_session_owner(&s.id, Some(&a.id)).unwrap();
    assert!(preparation_matches_current_owner(&db, &s.id, Some(&a.id)).unwrap());
    assert!(
        !preparation_matches_current_owner(&db, &s.id, Some(&b.id)).unwrap(),
        "a plan built for A must not be certified against B"
    );

    // Ownership moves mid-flight (what a concurrent edit does).
    db.set_session_owner(&s.id, Some(&b.id)).unwrap();
    assert!(!preparation_matches_current_owner(&db, &s.id, Some(&a.id)).unwrap());
    assert!(preparation_matches_current_owner(&db, &s.id, Some(&b.id)).unwrap());

    // A trashed Session matches nothing, and neither does a missing one.
    noending::lifecycle::archive_session(&db, &s.id).unwrap();
    assert!(!preparation_matches_current_owner(&db, &s.id, Some(&b.id)).unwrap());
    assert!(!preparation_matches_current_owner(&db, "no-such-session", None).unwrap());
}

/// A New Session prepared against a Workstream that is deleted before the
/// preview is built must fail loudly instead of launching against nothing.
#[test]
fn new_preparation_refuses_a_workstream_that_no_longer_exists() {
    let db = open_db("prepare-new-gone-ws");
    let a = workstream(&db, "A");
    archive_workstream(&db, &a.id).unwrap();
    delete_workstream_permanently(&db, &a.id).unwrap();

    let workspace = LaunchWorkspace {
        default_workspace: None,
    };
    let err = SessionLauncher {
        runtime_dir: db.dir.join("runtime"),
    }
    .prepare_new_in(&db, Agent::Codex, Some(&a.id), None, &workspace)
    .unwrap_err();
    assert!(
        err.to_string().contains("任务"),
        "the error must name the missing task, got {err}"
    );
}

// search projections follow the facts they copy

/// A Session's search document embeds its Owner's title and its Project's name
///. Every write that changes either fact must rebuild those documents
/// otherwise a rename leaves the old name searchable, and a deletion keeps a
/// Workstream findable through the Sessions that used to own it.
#[test]
fn session_search_documents_follow_workstream_and_project_renames() {
    use noending::search::search;

    let db = open_db("search-projection");
    db.upsert_project(&support::project("p1".to_string(), "ProjX".to_string()))
        .unwrap();
    let a = workstream(&db, "Alpha");
    // A real directory + WorkspacePath row, so the Project membership comes
    // from the authoritative chain (`workspace_path_id → project_id`,).
    let dir = db.dir.join("repo-search");
    std::fs::create_dir_all(&dir).unwrap();
    let dir_str = dir.to_string_lossy().to_string();
    let path_id = db
        .tx(|tx| insert_workspace_path_conn(tx, &dir_str, "p1"))
        .unwrap();
    let root_id = format!("root-{}", new_id());
    let (s_id, _) = db
        .upsert_logical_session_unchecked(
            Agent::Codex,
            &root_id,
            None,
            Some(&dir_str),
            Some(&path_id),
            None,
            None,
            None,
        )
        .unwrap();
    db.set_session_owner(&s_id, Some(&a.id)).unwrap();

    let hits_session = |query: &str| {
        search(&db, query, 40)
            .unwrap()
            .into_iter()
            .any(|h| h.kind == "session" && h.ref_id == s_id)
    };
    assert!(
        hits_session("Alpha"),
        "the session carries its Owner's title"
    );
    assert!(hits_session("ProjX"), "and its Project's name");

    // Rename the Workstream: the Session's document must follow.
    let mut renamed = db.get_workstream(&a.id).unwrap().unwrap();
    renamed.title = "Beta".into();
    renamed.updated_at = now();
    db.upsert_workstream(&renamed).unwrap();
    assert!(!hits_session("Alpha"), "the old title is gone");
    assert!(hits_session("Beta"), "the new title is there");

    // Rename the Project: same rule, the other input of the body.
    db.tx(|tx| noending::storage::workspace::rename_project_conn(tx, "p1", "ProjY"))
        .unwrap();
    assert!(!hits_session("ProjX"), "the old Project name is gone");
    assert!(hits_session("ProjY"));

    // Deleting the Workstream clears the Owner (FK) and must clear its title
    // from the Session's document with it.
    archive_workstream(&db, &a.id).unwrap();
    delete_workstream_permanently(&db, &a.id).unwrap();
    assert!(
        !hits_session("Beta"),
        "a deleted Workstream's title must not stay findable through its Sessions"
    );
    assert!(hits_session("ProjY"), "the rest of the document survives");
}

#[test]
fn archived_task_refuses_new_preparation_but_unarchive_allows_it() {
    let db = open_db("prepare-new-archived");
    let task = workstream(&db, "archive gate");
    archive_workstream(&db, &task.id).unwrap();
    let launcher = SessionLauncher {
        runtime_dir: db.dir.join("runtime"),
    };
    let error = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            Some(&task.id),
            None,
            &LaunchWorkspace::default(),
        )
        .unwrap_err();
    assert!(error.to_string().contains("已归档"));
    noending::workspace::workstream::restore_workstream(&db, &task.id).unwrap();
    assert!(launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            Some(&task.id),
            None,
            &LaunchWorkspace::default()
        )
        .is_ok());
}
