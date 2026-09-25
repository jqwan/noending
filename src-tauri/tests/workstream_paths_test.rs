//! The ordered WorkstreamPath list (方案 §1.5–§1.7, §18).
//!
//! Every test here is about ONE thing: that the list is the whole model, and
//! that "position 0 is primary" is a property of the list rather than a second
//! fact that can disagree with it.
//!
//! The attacher is a scripted stand-in, because the real implementation must
//! stay separate and the Workstream side must stay testable without Git
//! (`workspace::WorkspaceAttaching`'s contract: resolve through the caller's
//! connection, `Ok(None)` for "this string is no path").

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
mod support;

use rusqlite::{params, Connection};

use noending::commands::workstream_cards;
use noending::domain::{workstream_lifecycle, workstream_visibility, Agent, Session, Workstream};
use noending::error::{other, Result};
use noending::storage::workspace::insert_workspace_path_conn;
use noending::storage::{new_id, now, Db};
use noending::workspace::workstream::{
    add_workstream_path, create_workstream, list_workstream_path_views, remove_workstream_path,
    reorder_workstream_paths, workstream_launch_paths, WorkstreamLaunchPaths,
};
use noending::workspace::{normalize_path, path_identity, WorkspaceAttaching};

// ------------------------------------------------------------- fixtures

fn temp_db() -> (PathBuf, Db) {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "noending-wspaths-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("noending.db")).expect("temp db");
    (dir, db)
}

/// §42.3-M8 note 7: never compare a stored canonical path against a literal, and
/// never against a raw spelling either — run it through the same normalizer the
/// production door uses, or the assertion is a Unix-only assertion.
fn canonical(raw: &str) -> String {
    normalize_path(raw).expect("fixture path is normalizable")
}

fn path_id_of(raw: &str) -> String {
    path_identity(&canonical(raw))
}

/// Scripted stand-in for `workspace::project`'s attacher.
///
/// It resolves with the shared lexical normalizer and writes a real WorkspacePath
/// row **through the connection it was handed** — which is what makes a policy
/// that forgets to pass its own transaction fail loudly instead of committing a
/// path the rollback cannot take back.
#[derive(Default)]
struct ScriptedAttacher {
    /// raw path → Project the resolved WorkspacePath belongs to.
    projects: HashMap<String, String>,
    /// raw paths answered `Ok(None)`: a reserved app path, a Home-level
    /// repository, a relative directory. The real attacher refuses to guess, and
    /// so must any stand-in that wants to be faithful to it.
    unresolvable: HashSet<String>,
    /// raw paths answered `Err` — the attach failing after the Workstream row was
    /// already written, i.e. the atomicity case.
    failing: HashSet<String>,
    calls: RefCell<Vec<String>>,
}

impl WorkspaceAttaching for ScriptedAttacher {
    fn ensure_path(&self, conn: &Connection, raw: &str) -> Result<Option<String>> {
        self.calls.borrow_mut().push(raw.to_string());
        if self.failing.contains(raw) {
            return Err(other("模拟：登记工作路径失败"));
        }
        if self.unresolvable.contains(raw) {
            return Ok(None);
        }
        let project = self
            .projects
            .get(raw)
            .cloned()
            .ok_or_else(|| other(format!("模拟：未脚本化的路径 {raw}")))?;
        Ok(Some(insert_workspace_path_conn(
            conn,
            &canonical(raw),
            &project,
        )?))
    }
}

impl ScriptedAttacher {
    fn for_projects(pairs: &[(&str, &str)]) -> Self {
        Self {
            projects: pairs
                .iter()
                .map(|(raw, project)| (raw.to_string(), project.to_string()))
                .collect(),
            ..Default::default()
        }
    }
    fn paths_tried(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }
}

/// `p1` + `/repo/main`, `/repo/docs`, `/repo/backend`, in that order.
struct Fixture {
    _dir: PathBuf,
    db: Db,
    attacher: ScriptedAttacher,
    workstream: Workstream,
}

fn fixture() -> Fixture {
    let (_dir, db) = temp_db();
    db.upsert_project(&support::project("p1".into(), "P1"))
        .unwrap();
    let attacher = ScriptedAttacher::for_projects(&[
        ("/repo/main", "p1"),
        ("/repo/docs", "p1"),
        ("/repo/backend", "p1"),
    ]);
    let workstream = create_workstream(&db, &attacher, "Three paths", "", &["/repo/main".into()])
        .unwrap()
        .workstream;
    add_workstream_path(&db, &attacher, &workstream.id, "/repo/docs").unwrap();
    add_workstream_path(&db, &attacher, &workstream.id, "/repo/backend").unwrap();
    Fixture {
        _dir,
        db,
        attacher,
        workstream,
    }
}

/// `(workspace_path_id, position)` in list order.
fn list(db: &Db, workstream_id: &str) -> Vec<(String, i64)> {
    db.list_workstream_paths(workstream_id)
        .unwrap()
        .into_iter()
        .map(|p| (p.workspace_path_id, p.position))
        .collect()
}

fn positions(db: &Db, workstream_id: &str) -> Vec<i64> {
    list(db, workstream_id)
        .into_iter()
        .map(|(_, position)| position)
        .collect()
}

fn ordered_path_ids(db: &Db, workstream_id: &str) -> Vec<String> {
    db.list_workstream_paths(workstream_id)
        .unwrap()
        .into_iter()
        .map(|p| p.workspace_path_id)
        .collect()
}

fn session(db: &Db, tag: &str) -> Session {
    let s = support::session(
        new_id(),
        Agent::Codex,
        format!("src-{tag}"),
        format!("/raw/{tag}.jsonl"),
    );
    db.upsert_session(&s).unwrap();
    s
}

fn search_parent(db: &Db, workstream_id: &str) -> Option<String> {
    db.read()
        .query_row(
            "SELECT parent_id FROM search_index WHERE kind = 'workstream' AND ref_id = ?1",
            params![workstream_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
}

// ---------------------------------------------------------------- creation

/// §18 must-test: an empty list is a legal Workstream, not a half-built one.
#[test]
fn empty_path_list_is_valid() {
    let (_d, db) = temp_db();
    let attacher = ScriptedAttacher::default();
    let w = create_workstream(&db, &attacher, "No path", "d", &[])
        .unwrap()
        .workstream;

    assert_eq!(w.lifecycle, workstream_lifecycle::ACTIVE);
    assert_eq!(w.visibility, workstream_visibility::NORMAL);
    assert!(db.list_workstream_paths(&w.id).unwrap().is_empty());
    assert_eq!(db.primary_workspace_path_id(&w.id).unwrap(), None);
    // Zero paths is not an error for any reader either.
    assert!(list_workstream_path_views(&db, &w.id).unwrap().is_empty());
    assert_eq!(
        workstream_launch_paths(&db, &w.id).unwrap(),
        WorkstreamLaunchPaths::default()
    );
}

/// §11 + §18-7 — `create_workstream(title, description, initial_path?)`: the
/// path arrives as a raw string, is resolved by the attacher, and the one path a
/// new Workstream has is its primary without anyone saying so.
#[test]
fn first_path_becomes_primary_automatically() {
    let (_d, db) = temp_db();
    db.upsert_project(&support::project("p1".into(), "P1"))
        .unwrap();
    let attacher = ScriptedAttacher::for_projects(&[("/repo/main", "p1")]);

    let w = create_workstream(&db, &attacher, "Ship it", "  ", &["/repo/main".into()])
        .unwrap()
        .workstream;

    // The string is forwarded untouched — this layer owns no second normalizer.
    assert_eq!(attacher.paths_tried(), vec!["/repo/main".to_string()]);
    assert_eq!(list(&db, &w.id), vec![(path_id_of("/repo/main"), 0)]);
    assert_eq!(
        db.primary_workspace_path_id(&w.id).unwrap().as_deref(),
        Some(path_id_of("/repo/main").as_str())
    );
    // …and it reached the real WorkspacePath row, not a dangling id.
    assert_eq!(
        workstream_launch_paths(&db, &w.id).unwrap().primary(),
        Some(canonical("/repo/main").as_str())
    );
}

/// A `None` from the attacher means "no path", never "no Workstream" (§18-7).
#[test]
fn unresolvable_initial_path_leaves_a_zero_path_workstream() {
    let (_d, db) = temp_db();
    let mut attacher = ScriptedAttacher::default();
    attacher.unresolvable.insert("~/.noending/workspace".into());

    let w = create_workstream(
        &db,
        &attacher,
        "Home default",
        "",
        &["~/.noending/workspace".into()],
    )
    .unwrap()
    .workstream;
    assert!(db.get_workstream(&w.id).unwrap().is_some());
    assert!(db.list_workstream_paths(&w.id).unwrap().is_empty());

    // A blank is not a path: it never reaches the attacher at all.
    let w2 = create_workstream(&db, &attacher, "No path", "", &["   ".into()])
        .unwrap()
        .workstream;
    assert!(db.list_workstream_paths(&w2.id).unwrap().is_empty());
    assert_eq!(
        attacher.paths_tried(),
        vec!["~/.noending/workspace".to_string()]
    );
}

/// Creation is atomic: a failed attach leaves no Workstream behind.
#[test]
fn create_rolls_back_when_the_attach_fails() {
    let (_d, db) = temp_db();
    let mut attacher = ScriptedAttacher::default();
    attacher.failing.insert("/repo/hard".into());

    let err =
        create_workstream(&db, &attacher, "Half built", "", &["/repo/hard".into()]).unwrap_err();
    assert!(err.to_string().contains("模拟"), "{err}");
    assert!(db.list_workstreams(None).unwrap().is_empty());
    assert!(db.list_workspace_paths().unwrap().is_empty());
    assert_eq!(
        db.read()
            .query_row("SELECT COUNT(*) FROM workstreams", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

/// An empty title is refused before anything is written.
#[test]
fn create_requires_a_title() {
    let (_d, db) = temp_db();
    assert!(create_workstream(&db, &ScriptedAttacher::default(), "  ", "", &[]).is_err());
    assert!(db.list_workstreams(None).unwrap().is_empty());
}

// ------------------------------------------------------------ append / order

/// §1.5 — a second entry lands last and the primary does not move.
#[test]
fn append_is_secondary() {
    let (_d, db) = temp_db();
    db.upsert_project(&support::project("p1".into(), "P1"))
        .unwrap();
    let attacher = ScriptedAttacher::for_projects(&[("/repo/main", "p1"), ("/repo/docs", "p1")]);
    let w = create_workstream(&db, &attacher, "Two paths", "", &["/repo/main".into()])
        .unwrap()
        .workstream;
    let primary_before = ordered_path_ids(&db, &w.id)[0].clone();

    let second = add_workstream_path(&db, &attacher, &w.id, "/repo/docs").unwrap();
    assert_eq!(second.position, 1);
    assert_eq!(positions(&db, &w.id), vec![0, 1]);
    assert_eq!(
        ordered_path_ids(&db, &w.id),
        vec![primary_before.clone(), second.workspace_path_id.clone()],
        "append goes last; it never pushes the primary aside"
    );
    assert_eq!(
        db.primary_workspace_path_id(&w.id).unwrap(),
        Some(primary_before)
    );
}

/// Appending to a Workstream with no paths at all gives position 0 — which is
/// why "become the primary path" needs no branch in the append helper (§1.8).
#[test]
fn appending_to_an_empty_list_makes_it_primary() {
    let (_d, db) = temp_db();
    db.upsert_project(&support::project("p1".into(), "P1"))
        .unwrap();
    let attacher = ScriptedAttacher::for_projects(&[("/repo/late", "p1")]);
    let w = create_workstream(&db, &attacher, "Late path", "", &[])
        .unwrap()
        .workstream;

    let row = add_workstream_path(&db, &attacher, &w.id, "/repo/late").unwrap();
    assert_eq!(row.position, 0);
    assert_eq!(
        db.primary_workspace_path_id(&w.id).unwrap(),
        Some(row.workspace_path_id)
    );
}

/// §1.6 — the promotion is automatic: nobody is asked to choose a new primary.
#[test]
fn removing_the_first_path_promotes_the_second() {
    let f = fixture();
    let w = &f.workstream;
    let before = f.db.list_workstream_paths(&w.id).unwrap();
    assert_eq!(positions(&f.db, &w.id), vec![0, 1, 2]);

    remove_workstream_path(&f.db, &w.id, &before[0].id).unwrap();

    let after = f.db.list_workstream_paths(&w.id).unwrap();
    assert_eq!(after.len(), 2);
    assert_eq!(after[0].workspace_path_id, before[1].workspace_path_id);
    assert_eq!(after[1].workspace_path_id, before[2].workspace_path_id);
    assert_eq!(positions(&f.db, &w.id), vec![0, 1]);
    assert_eq!(
        f.db.primary_workspace_path_id(&w.id).unwrap(),
        Some(before[1].workspace_path_id.clone()),
        "the next entry IS the primary now"
    );
}

/// §1.5 — positions stay dense after any removal, so no append can fall into a
/// hole and no `position = 0` read can miss.
#[test]
fn removing_a_middle_path_recompacts() {
    let f = fixture();
    let w = &f.workstream;
    let before = f.db.list_workstream_paths(&w.id).unwrap();

    remove_workstream_path(&f.db, &w.id, &before[1].id).unwrap();
    let after = ordered_path_ids(&f.db, &w.id);
    assert_eq!(
        after,
        vec![
            before[0].workspace_path_id.clone(),
            before[2].workspace_path_id.clone()
        ],
        "order preserved, gap closed"
    );
    assert_eq!(positions(&f.db, &w.id), vec![0, 1]);

    // Removing the tail last leaves an empty-but-legal list with no primary.
    for row in f.db.list_workstream_paths(&w.id).unwrap() {
        remove_workstream_path(&f.db, &w.id, &row.id).unwrap();
    }
    assert!(f.db.list_workstream_paths(&w.id).unwrap().is_empty());
    assert_eq!(f.db.primary_workspace_path_id(&w.id).unwrap(), None);

    // …and a fresh append after that is primary again, not position 3.
    add_workstream_path(&f.db, &f.attacher, &w.id, "/repo/main").unwrap();
    assert_eq!(positions(&f.db, &w.id), vec![0]);
}

/// §1.5 — the reorder is the only way to move position 0, it is deterministic,
/// and it refuses to finish a partial list.
#[test]
fn reorder_is_deterministic() {
    let f = fixture();
    let w = &f.workstream;
    let before = ordered_path_ids(&f.db, &w.id);

    // rotate [0,1,2] → [2,0,1]
    let rotated = vec![before[2].clone(), before[0].clone(), before[1].clone()];
    let after = reorder_workstream_paths(&f.db, &w.id, &rotated).unwrap();
    assert_eq!(positions(&f.db, &w.id), vec![0, 1, 2]);
    assert_eq!(
        after
            .iter()
            .map(|p| p.workspace_path_id.clone())
            .collect::<Vec<_>>(),
        rotated,
        "positions describe the order that was given, not the old one"
    );

    // Repeating the identical reorder is a no-op: it cannot drift or double-shift.
    let again = reorder_workstream_paths(&f.db, &w.id, &rotated).unwrap();
    assert_eq!(again[0].workspace_path_id, rotated[0]);
    assert_eq!(positions(&f.db, &w.id), vec![0, 1, 2]);

    // "设为主路径" (§22) is a reorder to index 0 with no role column involved.
    let promoted = vec![before[1].clone(), rotated[0].clone(), rotated[1].clone()];
    let result = reorder_workstream_paths(&f.db, &w.id, &promoted).unwrap();
    assert_eq!(result[0].workspace_path_id, before[1]);
    assert_eq!(
        f.db.primary_workspace_path_id(&w.id).unwrap(),
        Some(before[1].clone())
    );

    // An incomplete list is refused rather than silently keeping the old tail
    // (a UI race must not be able to drop a path the user chose).
    let err = reorder_workstream_paths(&f.db, &w.id, &[promoted[0].clone()]).unwrap_err();
    assert!(err.to_string().contains("完整"), "{err}");
    assert_eq!(ordered_path_ids(&f.db, &w.id), promoted);
    // An id from another Workstream is not part of this list either.
    assert!(reorder_workstream_paths(&f.db, &w.id, &["not-a-path".into()]).is_err());
    assert_eq!(ordered_path_ids(&f.db, &w.id), promoted);
}

/// §1.5 + §42.3-M1 — one directory is one entry, however many ways there are to
/// spell it.
#[test]
fn duplicate_path_is_idempotent() {
    let (_d, db) = temp_db();
    db.upsert_project(&support::project("p1".into(), "P1"))
        .unwrap();
    let attacher = ScriptedAttacher::for_projects(&[
        ("/repo/main", "p1"),
        ("/repo/main/", "p1"),
        ("/repo/./main", "p1"),
        ("/repo/a/../main", "p1"),
    ]);
    let w = create_workstream(&db, &attacher, "Dup", "", &["/repo/main".into()])
        .unwrap()
        .workstream;
    let first = db.list_workstream_paths(&w.id).unwrap().remove(0);

    for spelling in [
        "/repo/main",
        "/repo/main/",
        "/repo/./main",
        "/repo/a/../main",
    ] {
        let again = add_workstream_path(&db, &attacher, &w.id, spelling).unwrap();
        assert_eq!(again.id, first.id, "{spelling} added a second row");
        assert_eq!(again.position, 0);
    }
    let rows = list(&db, &w.id);
    assert_eq!(rows.len(), 1, "no second row: {rows:?}");
    assert_eq!(rows[0].0, first.workspace_path_id);

    // The same door from a second append still keeps the single user row: the
    // path was already chosen, so nothing duplicates it.
    db.tx(|tx| {
        let idem = noending::storage::workstream_paths::append_workstream_path_conn(
            tx,
            &w.id,
            &first.workspace_path_id,
        )
        .unwrap();
        assert_eq!(idem.id, first.id);
        Ok(())
    })
    .unwrap();
    assert_eq!(list(&db, &w.id).len(), 1);
}

/// The two UNIQUE keys are what make "secondary without primary" unrepresentable
/// (§18-2): the policy does not have to remember a rule the schema can state.
#[test]
fn the_storage_keys_make_a_secondary_without_a_primary_unrepresentable() {
    let f = fixture();
    let w = &f.workstream;
    let rows = f.db.list_workstream_paths(&w.id).unwrap();
    // A fourth, real WorkspacePath that is not in this Workstream's list yet.
    let spare = insert_workspace_path_conn(&f.db.write(), &canonical("/repo/spare"), "p1").unwrap();

    // Two entries cannot both claim position 0 — that is the whole of §1.5's
    // "no primary + has secondary" impossibility.
    let clash = f.db.write().execute(
        "INSERT INTO workstream_paths (id, workstream_id, workspace_path_id, position, created_at)
         VALUES ('dup-zero', ?1, ?2, 0, ?3)",
        params![w.id, spare, now()],
    );
    assert!(
        clash.unwrap_err().to_string().contains("UNIQUE"),
        "UNIQUE(workstream_id, position) must refuse a second primary"
    );

    // …and one path cannot be listed twice, even at a free position.
    let dupe = f.db.write().execute(
        "INSERT INTO workstream_paths (id, workstream_id, workspace_path_id, position, created_at)
         VALUES ('dupe', ?1, ?2, 9, ?3)",
        params![w.id, rows[0].workspace_path_id, now()],
    );
    assert!(
        dupe.unwrap_err().to_string().contains("UNIQUE"),
        "UNIQUE(workstream_id, workspace_path_id) must refuse the duplicate"
    );
    assert_eq!(positions(&f.db, &w.id), vec![0, 1, 2]);
}

/// §1.7 — adding a path adds a path. Nothing under it is imported.
#[test]
fn adding_a_path_imports_no_sessions() {
    let (_d, db) = temp_db();
    db.upsert_project(&support::project("p1".into(), "P1"))
        .unwrap();
    let attacher = ScriptedAttacher::for_projects(&[("/repo/main", "p1")]);
    let w = create_workstream(&db, &attacher, "No import", "", &[])
        .unwrap()
        .workstream;

    // A Session already working in that directory, owned by no Workstream.
    let mut s = session(&db, "in-dir");
    s.workspace_path_id = Some(path_id_of("/repo/main"));
    s.cwd = Some(canonical("/repo/main"));
    db.upsert_session(&s).unwrap();

    add_workstream_path(&db, &attacher, &w.id, "/repo/main").unwrap();

    assert_eq!(db.workstream_session_stats(&w.id).unwrap().0, 0);
    assert!(db
        .get_session(&s.id)
        .unwrap()
        .unwrap()
        .owner_workstream_id
        .is_none());
    assert_eq!(db.list_workstream_paths(&w.id).unwrap().len(), 1);
}

/// §18-7 — the path must be a real path: an unresolvable string on an explicit
/// add is reported, not swallowed (contrast with `create_workstream`, where the
/// field is optional).
#[test]
fn adding_an_unresolvable_path_is_an_error_not_a_noop() {
    let (_d, db) = temp_db();
    let mut attacher = ScriptedAttacher::default();
    attacher.unresolvable.insert("relative/dir".into());
    let w = create_workstream(&db, &attacher, "Strict", "", &[])
        .unwrap()
        .workstream;

    let err = add_workstream_path(&db, &attacher, &w.id, "relative/dir").unwrap_err();
    assert!(err.to_string().contains("工作路径"), "{err}");
    assert!(db.list_workstream_paths(&w.id).unwrap().is_empty());
    // and a Workstream that does not exist is not created by the side either
    assert!(add_workstream_path(&db, &attacher, "gone", "/repo/main").is_err());
}

// ---------------------------------------------- removal reach (§5.2 / §12)

/// §5.2 / §12 — a path removal changes only the path list. Sessions that were
/// launched through the removed directory keep their Owner Workstream, and the
/// Session rows themselves are untouched.
#[test]
fn removing_a_path_leaves_sessions_and_their_owner_alone() {
    let f = fixture();
    let w = &f.workstream;
    let rows = f.db.list_workstream_paths(&w.id).unwrap();
    let primary = &rows[0];

    let s = session(&f.db, "s-carried");
    f.db.set_session_owner(&s.id, Some(&w.id)).unwrap();

    remove_workstream_path(&f.db, &w.id, &primary.id).unwrap();

    let after = f.db.get_session(&s.id).unwrap().expect("session survives");
    assert_eq!(
        after.owner_workstream_id.as_deref(),
        Some(w.id.as_str()),
        "path removal never touches ownership"
    );
    assert_eq!(f.db.list_workspace_paths().unwrap().len(), 3);
    // Re-adding the path next week still works: removal leaves no tombstone.
    add_workstream_path(&f.db, &f.attacher, &w.id, "/repo/main").unwrap();
    assert_eq!(positions(&f.db, &w.id), vec![0, 1, 2]);
}

/// A `workstream_paths.id` is scoped to its Workstream: another Workstream's
/// entry cannot rewrite this one's list.
#[test]
fn a_path_row_of_another_workstream_is_not_accepted() {
    let f = fixture();
    let b = create_workstream(&f.db, &f.attacher, "Other", "", &["/repo/main".into()])
        .unwrap()
        .workstream;
    let of_b = f.db.list_workstream_paths(&b.id).unwrap().remove(0);

    let err = remove_workstream_path(&f.db, &f.workstream.id, &of_b.id).unwrap_err();
    assert!(err.to_string().contains("不属于"), "{err}");
    assert_eq!(positions(&f.db, &f.workstream.id), vec![0, 1, 2]);
    assert_eq!(positions(&f.db, &b.id), vec![0]);
    assert_eq!(of_b.workstream_id, b.id);
}

/// §18-15 + §42.3-M18 — the search row and the card's Project columns follow the
/// position-0 path, never the frozen `workstreams.project_id`.
#[test]
fn the_primary_path_projection_moves_the_search_row_and_the_card() {
    let (_d, db) = temp_db();
    for id in ["p_frozen", "p_real", "p_other"] {
        db.upsert_project(&support::project(id.into(), id.to_uppercase()))
            .unwrap();
    }
    let w = support::workstream("w-proj".into(), "Projection");
    db.upsert_workstream(&w).unwrap();
    let attacher =
        ScriptedAttacher::for_projects(&[("/real/one", "p_real"), ("/other/two", "p_other")]);
    add_workstream_path(&db, &attacher, &w.id, "/real/one").unwrap();
    add_workstream_path(&db, &attacher, &w.id, "/other/two").unwrap();

    let card = work_cards(&db, &w.id);
    assert_eq!(card.project_id.as_deref(), Some("p_real"));
    assert_eq!(card.project_name.as_deref(), Some("P_REAL"));
    assert_eq!(card.path_count, 2);
    assert_eq!(search_parent(&db, &w.id).as_deref(), Some("p_real"));

    // Make the other path primary: the projection moves with the ordered list.
    let rows = db.list_workstream_paths(&w.id).unwrap();
    reorder_workstream_paths(
        &db,
        &w.id,
        &[
            rows[1].workspace_path_id.clone(),
            rows[0].workspace_path_id.clone(),
        ],
    )
    .unwrap();
    assert_eq!(
        work_cards(&db, &w.id).project_name.as_deref(),
        Some("P_OTHER")
    );
    assert_eq!(search_parent(&db, &w.id).as_deref(), Some("p_other"));

    // Removing every path returns the projection to "no Project" instead of
    // falling back to the stale column.
    for row in db.list_workstream_paths(&w.id).unwrap() {
        remove_workstream_path(&db, &w.id, &row.id).unwrap();
    }
    let card = work_cards(&db, &w.id);
    assert_eq!(card.project_id, None);
    assert_eq!(card.project_name, None);
    assert_eq!(card.path_count, 0);
    assert_eq!(search_parent(&db, &w.id), None);
}

fn work_cards(db: &Db, workstream_id: &str) -> noending::commands::workstream::WorkstreamCardView {
    workstream_cards(db)
        .unwrap()
        .into_iter()
        .find(|c| c.workstream.id == workstream_id)
        .expect("card")
}

/// §12 — the list the launcher hashes is ordered, and ordering is what makes a
/// reorder stale an unreconsumed PreparedLaunch.
#[test]
fn launch_path_fingerprint_is_order_sensitive() {
    let f = fixture();
    let w = &f.workstream;

    let forward = workstream_launch_paths(&f.db, &w.id).unwrap();
    assert_eq!(
        forward.ordered_paths,
        vec![
            canonical("/repo/main"),
            canonical("/repo/docs"),
            canonical("/repo/backend")
        ],
        "the list reaches the launcher in position order"
    );
    assert_eq!(
        forward.primary(),
        Some(canonical("/repo/main").as_str()),
        "position 0 IS the launch directory (§13 tier 2)"
    );

    let reversed = WorkstreamLaunchPaths {
        ordered_paths: forward.ordered_paths.iter().rev().cloned().collect(),
    };
    assert_ne!(
        forward.fingerprint_input(),
        reversed.fingerprint_input(),
        "a reorder that moves position 0 must invalidate the prepared launch"
    );

    // Swapping two NON-primary entries leaves position 0 alone…
    let mut tail_swapped = forward.ordered_paths.clone();
    tail_swapped.swap(1, 2);
    let tail_swapped = WorkstreamLaunchPaths {
        ordered_paths: tail_swapped,
    };
    assert_eq!(tail_swapped.primary(), forward.primary());
    assert_ne!(
        tail_swapped.fingerprint_input(),
        forward.fingerprint_input(),
        "the whole ordered list is stale input, not only its head"
    );

    // Every value is labelled and terminated, so a longer list can never collide
    // with a shorter one by shifting a boundary (§42.3-M17).
    assert_ne!(
        WorkstreamLaunchPaths {
            ordered_paths: vec!["/a".into(), "/b".into()]
        }
        .fingerprint_input(),
        WorkstreamLaunchPaths {
            ordered_paths: vec!["/a|/b".into()]
        }
        .fingerprint_input()
    );
    assert_eq!(
        WorkstreamLaunchPaths::default().fingerprint_input(),
        b"ws_paths:primary:|".to_vec()
    );
    assert_eq!(
        WorkstreamLaunchPaths {
            ordered_paths: vec!["/a".into()]
        }
        .fingerprint_input(),
        b"ws_paths:/a|primary:/a|".to_vec()
    );
}

/// The view the detail page reads: the list plus the physical facts behind it.
#[test]
fn path_views_carry_the_facts_the_detail_page_needs() {
    let f = fixture();
    let w = &f.workstream;

    let views = list_workstream_path_views(&f.db, &w.id).unwrap();
    assert_eq!(views.len(), 3);
    assert_eq!(views[0].path.position, 0);
    assert_eq!(views[0].canonical_path, canonical("/repo/main"));
    assert_eq!(views[0].project_id, "p1");
    assert_eq!(views[0].project_name.as_deref(), Some("P1"));
    assert_eq!(views[2].canonical_path, canonical("/repo/backend"));
}

// ------------------------------------------------- multi-path creation (report)

/// §11 — several initial paths land in submission order, and the report names
/// each one's canonical spelling, position and derived Project.
#[test]
fn initial_paths_keep_submission_order_and_report_each_entry() {
    let (_dir, db) = temp_db();
    db.upsert_project(&support::project("p1".into(), "P1"))
        .unwrap();
    let attacher = ScriptedAttacher::for_projects(&[
        ("/repo/main", "p1"),
        ("/repo/docs", "p1"),
        ("/repo/backend", "p1"),
    ]);

    let report = create_workstream(
        &db,
        &attacher,
        "Multi",
        "",
        &[
            "/repo/main".into(),
            "/repo/docs".into(),
            "/repo/backend".into(),
        ],
    )
    .unwrap();

    assert_eq!(report.workstream.title, "Multi");
    assert!(report.paths.iter().all(|p| p.accepted), "all accepted");
    assert_eq!(report.paths[0].project_name.as_deref(), Some("P1"));
    let landed: Vec<(String, i64)> = report
        .paths
        .iter()
        .map(|p| (p.canonical_path.clone().unwrap(), p.position.unwrap()))
        .collect();
    assert_eq!(
        landed,
        vec![
            (canonical("/repo/main"), 0),
            (canonical("/repo/docs"), 1),
            (canonical("/repo/backend"), 2),
        ]
    );
    assert_eq!(list(&db, &report.workstream.id).len(), 3);
}

/// The primary seat is "first ACCEPTED", not "first submitted": a refused
/// string must not leave a hole in the ordered list (§1.5: positions are
/// contiguous by construction).
#[test]
fn first_accepted_path_wins_the_primary_seat_even_after_rejections() {
    let (_dir, db) = temp_db();
    db.upsert_project(&support::project("p1".into(), "P1"))
        .unwrap();
    let mut attacher = ScriptedAttacher::default();
    attacher.unresolvable.insert("/repo/ghost".into());
    attacher.projects.insert("/repo/docs".into(), "p1".into());

    let report = create_workstream(
        &db,
        &attacher,
        "Compact",
        "",
        &["/repo/ghost".into(), "/repo/docs".into()],
    )
    .unwrap();

    let rejected = &report.paths[0];
    assert!(!rejected.accepted);
    assert!(rejected.position.is_none());
    assert!(
        rejected
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("工作路径"),
        "{:?}",
        rejected.reason
    );
    let accepted = &report.paths[1];
    assert!(accepted.accepted);
    assert_eq!(accepted.position, Some(0), "first accepted IS primary");
    assert_eq!(accepted.project_name.as_deref(), Some("P1"));
    let rows = db.list_workstream_paths(&report.workstream.id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].position, 0,
        "the stored list agrees with the report"
    );
}

/// The same raw string twice, or two spellings of one directory, attach once —
/// the second entry is reported with a reason instead of silently collapsing
/// (`append_workstream_path_conn` would happily return the existing row).
#[test]
fn duplicates_inside_one_call_are_reported_not_attached() {
    let (_dir, db) = temp_db();
    db.upsert_project(&support::project("p1".into(), "P1"))
        .unwrap();
    let attacher = ScriptedAttacher::for_projects(&[("/repo/main", "p1"), ("/repo/main/", "p1")]);

    let report = create_workstream(
        &db,
        &attacher,
        "Dup raw",
        "",
        &["/repo/main".into(), "/repo/main".into()],
    )
    .unwrap();
    assert!(report.paths[0].accepted);
    assert!(!report.paths[1].accepted);
    assert!(
        report.paths[1]
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("重复"),
        "{:?}",
        report.paths[1].reason
    );
    assert_eq!(
        db.list_workstream_paths(&report.workstream.id)
            .unwrap()
            .len(),
        1
    );

    let report2 = create_workstream(
        &db,
        &attacher,
        "Dup canonical",
        "",
        &["/repo/main".into(), "/repo/main/".into()],
    )
    .unwrap();
    assert!(report2.paths[0].accepted);
    assert!(!report2.paths[1].accepted);
    assert!(
        report2.paths[1]
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("同一目录"),
        "{:?}",
        report2.paths[1].reason
    );
    assert_eq!(
        db.list_workstream_paths(&report2.workstream.id)
            .unwrap()
            .len(),
        1
    );
}
