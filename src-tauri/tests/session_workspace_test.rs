//! Session workspace + binding semantics (方案 §19).
//!
//! Every test here pins one edge of the fact chain
//!
//! ```text
//! Session.cwd → workspace_path_id → workspace_paths.project_id → Project
//! ```
//!
//! plus what a Workstream binding means now that a Workstream owns an ordered
//! path list. Two things are deliberately scripted rather than real:
//!
//! * the WorkspacePath creator is a [`Scripted`] stand-in for
//!   `workspace::project`'s implementation of [`WorkspaceAttaching`], so Session
//!   rules are testable without Git detection;
//! * every path is a plain `/repo/...` string — no temp-dir prefix is ever
//!   asserted, because `std::env::temp_dir()` is a symlink on macOS (§42.3-M8).
//!
//! Only temp databases are opened; no user Home, no real transcripts.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use noending::adapters::{adapter_for, DiscoveredSession};
use noending::domain::{
    binding_source, workstream_path_source, Agent, Project, Session, SessionWorkstreamBinding,
    Workstream,
};
use noending::error::Result;
use noending::ingestion::{ensure_session_row_with, ingest_session, reconcile_with_engine};
use noending::launcher::LaunchWorkspace;
use noending::storage::session_paths::{
    attach_session_workspace_path_conn, reconcile_binding_paths_conn,
    refresh_sessions_project_for_path_conn,
};
use noending::storage::workspace::{
    insert_workspace_path_conn, reassign_workspace_path_project_conn,
};
use noending::storage::workstream_paths::append_workstream_path_conn;
use noending::storage::workstream_paths::remove_workstream_path_conn;
use noending::storage::Db;
use noending::sync::SyncEngine;
use noending::workspace::session::{
    attach_session_conn, record_user_binding, register_workspace_attacher,
    replace_session_bindings, resolve_binding_path_conn, DesiredBinding, UnattachedWorkspacePaths,
};
use noending::workspace::{normalize_path, path_identity_of, WorkspaceAttaching};

// ------------------------------------------------------------- fixtures

fn unique_dir(tag: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "noending-session-ws-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn temp_db(tag: &str) -> (PathBuf, Db) {
    let dir = unique_dir(tag);
    let db = Db::open(&dir.join("noending.db")).expect("temp db");
    (dir, db)
}

/// The seam, scripted: every resolvable spelling becomes its deterministic
/// WorkspacePath under one fixed Project, and every call is counted so "ordinary
/// ingestion does no Project work" is a measurement instead of an intention.
struct Scripted {
    project_id: String,
    calls: AtomicUsize,
}

impl Scripted {
    fn new(project_id: &str) -> Self {
        Self {
            project_id: project_id.into(),
            calls: AtomicUsize::new(0),
        }
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl WorkspaceAttaching for Scripted {
    fn ensure_path(&self, conn: &rusqlite::Connection, raw: &str) -> Result<Option<String>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let Some(canonical) = normalize_path(raw) else {
            return Ok(None);
        };
        Ok(Some(insert_workspace_path_conn(
            conn,
            &canonical,
            &self.project_id,
        )?))
    }
}

fn project(db: &Db, id: &str, name: &str) {
    db.upsert_project(&Project::new(id.into(), name)).unwrap();
}

fn workstream(db: &Db, id: &str) -> String {
    db.upsert_workstream(&Workstream::new(id.into(), id))
        .unwrap();
    id.to_string()
}

fn discovered(agent_session_id: &str, cwd: Option<&str>, raw_path: &Path) -> DiscoveredSession {
    DiscoveredSession {
        agent: Agent::Codex,
        agent_session_id: agent_session_id.into(),
        path: raw_path.to_path_buf(),
        cwd: cwd.map(Into::into),
        started_at: Some("2026-09-13T10:00:00Z".into()),
        last_activity_at: Some("2026-09-13T10:05:00Z".into()),
        first_user_text: Some("帮我看下这个模块".into()),
        parent_agent_session_id: None,
    }
}

/// One discovery pass over a fake transcript location.
fn discover(
    db: &Db,
    attacher: &dyn WorkspaceAttaching,
    agent_session_id: &str,
    cwd: Option<&str>,
) -> (Session, bool) {
    ensure_session_row_with(
        db,
        &discovered(
            agent_session_id,
            cwd,
            &PathBuf::from(format!("/raw/{agent_session_id}.jsonl")),
        ),
        attacher,
    )
    .unwrap()
}

fn bind(db: &Db, session_id: &str, workstream_id: &str, role: &str) {
    record_user_binding(
        db,
        session_id,
        workstream_id,
        role,
        binding_source::USER_ASSIGNED,
        1.0,
    )
    .unwrap();
}

fn stored(db: &Db, id: &str) -> Session {
    db.get_session(id).unwrap().expect("session row")
}

fn claim(db: &Db, session_id: &str, workstream_id: &str) -> Option<String> {
    db.bindings_for_session(session_id)
        .unwrap()
        .into_iter()
        .find(|b| b.workstream_id == workstream_id)
        .and_then(|b| b.workstream_path_id)
}

fn binding(db: &Db, session_id: &str, workstream_id: &str) -> SessionWorkstreamBinding {
    db.bindings_for_session(session_id)
        .unwrap()
        .into_iter()
        .find(|b| b.workstream_id == workstream_id)
        .expect("binding row")
}

fn path_list(db: &Db, workstream_id: &str) -> Vec<(String, i64, String)> {
    db.list_workstream_paths(workstream_id)
        .unwrap()
        .into_iter()
        .map(|p| (p.workspace_path_id, p.position, p.source))
        .collect()
}

/// The WorkstreamPath **row** id at a position — what a binding claims. It is
/// deliberately not the WorkspacePath id: a claim points at the list entry, not
/// at the path itself (§42.3-M1).
fn row_id_at(db: &Db, workstream_id: &str, position: i64) -> String {
    db.list_workstream_paths(workstream_id)
        .unwrap()
        .into_iter()
        .find(|p| p.position == position)
        .unwrap_or_else(|| panic!("no WorkstreamPath at position {position}"))
        .id
}

/// Codex rollout envelope: discovery fingerprints on `{ordinal, payload, type}`
/// per line, so a fixture without `ordinal` is simply not a Codex file.
fn codex_user_line(text: &str) -> String {
    format!(
        r#"{{"ordinal":1,"type":"response_item","timestamp":"2026-09-13T10:01:00Z","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"{text}"}}]}}}}"#
    )
}

fn codex_meta_line(session_id: &str, cwd: &str) -> String {
    format!(
        r#"{{"ordinal":0,"type":"session_meta","timestamp":"2026-09-13T10:00:00Z","payload":{{"id":"{session_id}","cwd":"{cwd}"}}}}"#
    )
}

// ------------------------------------------- 1. discovery → WorkspacePath

#[test]
fn discovered_session_gets_a_workspace_path() {
    let (_d, db) = temp_db("discover");
    project(&db, "p1", "repo");
    let attacher = Scripted::new("p1");

    let (s, is_new) = discover(&db, &attacher, "s1", Some("/repo/app"));
    assert!(is_new);
    let stored = stored(&db, &s.id);
    assert_eq!(
        stored.workspace_path_id,
        path_identity_of("/repo/app"),
        "the observed cwd became its WorkspacePath, by deterministic identity"
    );
    assert_eq!(db.list_workspace_paths().unwrap().len(), 1);

    // Re-discovery of the same transcript learns nothing new, so no second
    // attach happens — the seam is free to write a row and must not be called.
    discover(&db, &attacher, "s1", Some("/repo/app"));
    assert_eq!(attacher.calls(), 1, "a steady-state scan does no path work");
    assert_eq!(db.list_workspace_paths().unwrap().len(), 1);
}

#[test]
fn a_different_spelling_of_the_same_cwd_does_not_move_the_session() {
    let (_d, db) = temp_db("spell");
    project(&db, "p1", "repo");
    let attacher = Scripted::new("p1");

    let (s, _) = discover(&db, &attacher, "s1", Some("/repo/app"));
    let before = stored(&db, &s.id);
    // A trailing separator is the same directory, so the same identity: no
    // second WorkspacePath and no "drift".
    let (_, is_new) = discover(&db, &attacher, "s1", Some("/repo/app/"));
    assert!(!is_new);
    let after = stored(&db, &s.id);
    assert_eq!(before.workspace_path_id, after.workspace_path_id);
    assert_eq!(before.project_id, after.project_id);
    assert_eq!(db.list_workspace_paths().unwrap().len(), 1);
}

#[test]
fn an_unwired_workspace_layer_leaves_the_session_unattached() {
    let (_d, db) = temp_db("unwired");
    let (s, _) = ensure_session_row_with(
        &db,
        &discovered("s1", Some("/repo/app"), &PathBuf::from("/raw/s1.jsonl")),
        &UnattachedWorkspacePaths,
    )
    .unwrap();
    let stored = stored(&db, &s.id);
    assert_eq!(stored.workspace_path_id, None);
    assert_eq!(stored.project_id, None);
    assert!(db.list_workspace_paths().unwrap().is_empty());
}

// ------------------------------------------- 2/3. the derived Project

#[test]
fn session_gets_a_derived_project() {
    let (_d, db) = temp_db("derived");
    project(&db, "p1", "repo");
    let attacher = Scripted::new("p1");

    let (s, _) = discover(&db, &attacher, "s1", Some("/repo/app"));
    let stored = stored(&db, &s.id);
    assert_eq!(stored.project_id.as_deref(), Some("p1"));
    // The cache agrees with its source, which is the whole invariant.
    let wp = db
        .get_workspace_path(stored.workspace_path_id.as_ref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(wp.project_id.as_str(), "p1");
    // And what discovery hands back is the stored row, not a caller's guess:
    // nothing in this flow ever supplied a Project at all.
    assert_eq!(s.project_id, stored.project_id);
    assert_eq!(s.workspace_path_id, stored.workspace_path_id);
}

#[test]
fn a_standalone_session_still_gets_a_project() {
    // §24 — no Workstream is involved: membership comes from the Session's own
    // path, straight down the chain.
    let (_d, db) = temp_db("standalone");
    project(&db, "p1", "repo");
    let attacher = Scripted::new("p1");

    let (s, _) = discover(&db, &attacher, "s1", Some("/repo/app"));
    assert!(db.bindings_for_session(&s.id).unwrap().is_empty());
    assert_eq!(stored(&db, &s.id).project_id.as_deref(), Some("p1"));
}

#[test]
fn an_explicit_attach_recomputes_the_cached_project_from_the_path() {
    // The explicit re-attach door: a Session moves and the cache follows in the
    // same statement. Passing a Project to it is not an option, by design.
    let (_d, db) = temp_db("reattach");
    project(&db, "p1", "one");
    project(&db, "p2", "two");
    let attacher = Scripted::new("p2");
    let (s, _) = discover(&db, &attacher, "s1", Some("/repo/two"));
    assert_eq!(stored(&db, &s.id).project_id.as_deref(), Some("p2"));

    let first = db
        .tx(|tx| insert_workspace_path_conn(tx, &normalize_path("/repo/one").unwrap(), "p1"))
        .unwrap();
    db.tx(|tx| attach_session_workspace_path_conn(tx, &s.id, Some(&first)))
        .unwrap();
    let after = stored(&db, &s.id);
    assert_eq!(after.workspace_path_id.as_deref(), Some(first.as_str()));
    assert_eq!(after.project_id.as_deref(), Some("p1"));
}

#[test]
fn project_id_writers_are_confined_to_the_derived_doors() {
    // §42.5-T2 as an executable check: `sessions.project_id` is a cache, and a
    // cache with a fourth writer stops being a cache.
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let offenders = rust_files(&manifest.join("src"))
        .into_iter()
        .filter(|f| {
            let rel = f
                .strip_prefix(manifest)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            // The two sanctioned files: the derived doors in session_paths.rs,
            // and storage/mod.rs (upsert_session's in-statement COALESCE plus
            // the v12 migration and the referential cleanup on Project delete).
            let allowed = rel == "src/storage/session_paths.rs" || rel == "src/storage/mod.rs";
            !allowed && writes_sessions_project_id(&std::fs::read_to_string(f).unwrap_or_default())
        })
        .collect::<Vec<_>>();
    assert!(
        offenders.is_empty(),
        "sessions.project_id may only be written by upsert_session's in-statement \
         derivation, the batch refresh and the migration: {offenders:?}"
    );
}

/// `UPDATE sessions … SET project_id`, tolerant of a statement wrapped across
/// lines, and blind to prose: the doc comments that *explain* the prohibition
/// must not be reported as violating it. A near-miss like
/// `UPDATE workspace_paths SET project_id` must not trip it either — that write
/// is legitimate Project policy, not a cache violation.
fn writes_sessions_project_id(text: &str) -> bool {
    let code = text
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join(" ");
    let flat = code.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut from = 0usize;
    while let Some(i) = flat[from..].find("UPDATE sessions") {
        let window = &flat[from + i..flat.len().min(from + i + 160)];
        if window.contains("SET project_id") {
            return true;
        }
        from += i + "UPDATE sessions".len();
    }
    false
}

fn rust_files(path: &Path) -> Vec<PathBuf> {
    if path.is_dir() {
        std::fs::read_dir(path)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .flat_map(|e| rust_files(&e.path()))
                    .collect()
            })
            .unwrap_or_default()
    } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
        vec![path.to_path_buf()]
    } else {
        vec![]
    }
}

// ------------------------------------------------ 4. batch Project refresh

#[test]
fn changing_a_workspace_paths_project_refreshes_all_its_sessions() {
    let (_d, db) = temp_db("refresh");
    project(&db, "p1", "one");
    project(&db, "p2", "two");
    let attacher = Scripted::new("p1");

    let a = discover(&db, &attacher, "a", Some("/repo/app")).0;
    let b = discover(&db, &attacher, "b", Some("/repo/app")).0;
    let c = discover(&db, &attacher, "c", Some("/repo/other")).0;
    for s in [&a, &b, &c] {
        assert_eq!(stored(&db, &s.id).project_id.as_deref(), Some("p1"));
    }

    let path = path_identity_of("/repo/app").unwrap();
    db.tx(|tx| reassign_workspace_path_project_conn(tx, &path, "p2"))
        .unwrap();

    assert_eq!(stored(&db, &a.id).project_id.as_deref(), Some("p2"));
    assert_eq!(stored(&db, &b.id).project_id.as_deref(), Some("p2"));
    // Only the Sessions behind that path moved; the third still reads p1 through
    // its own path. A refresh is a projection, not a rename-everything.
    assert_eq!(stored(&db, &c.id).project_id.as_deref(), Some("p1"));
}

#[test]
fn the_batch_refresh_is_alone_sufficient_and_idempotent() {
    let (_d, db) = temp_db("refresh2");
    project(&db, "p1", "one");
    project(&db, "p2", "two");
    let attacher = Scripted::new("p1");
    let a = discover(&db, &attacher, "a", Some("/repo/app")).0;
    let path = path_identity_of("/repo/app").unwrap();
    db.tx(|tx| {
        tx.execute(
            "UPDATE workspace_paths SET project_id = 'p2' WHERE id = ?1",
            rusqlite::params![path],
        )?;
        Ok(())
    })
    .unwrap();

    // The helper alone is enough: it re-reads the Project through the path
    // instead of being told what the new value is.
    let refreshed = db
        .tx(|tx| refresh_sessions_project_for_path_conn(tx, &path))
        .unwrap();
    assert_eq!(refreshed, 1);
    assert_eq!(stored(&db, &a.id).project_id.as_deref(), Some("p2"));
    // Running it again cannot apply the change twice.
    let again = db
        .tx(|tx| refresh_sessions_project_for_path_conn(tx, &path))
        .unwrap();
    assert_eq!(again, 1);
    assert_eq!(stored(&db, &a.id).project_id.as_deref(), Some("p2"));
}

// ------------------------------- 5. ordinary event ingestion does no Project work

#[test]
fn event_ingestion_alone_does_not_change_a_project() {
    let (_d, db) = temp_db("ingest");
    project(&db, "p1", "repo");
    let dir = unique_dir("ingest");
    let file = dir.join("rollout-2026-09-13-t-1-2-3-4.jsonl");
    std::fs::write(
        &file,
        format!(
            "{}\n{}\n",
            codex_meta_line("ingest-1", "/repo/app"),
            codex_user_line("first message")
        ),
    )
    .unwrap();

    let attacher = Scripted::new("p1");
    let (mut s, _) = ensure_session_row_with(
        &db,
        &discovered("ingest-1", Some("/repo/app"), &file),
        &attacher,
    )
    .unwrap();
    assert_eq!(attacher.calls(), 1);
    let before = stored(&db, &s.id);
    let path_before = db
        .get_workspace_path(before.workspace_path_id.as_ref().unwrap())
        .unwrap()
        .unwrap();

    // Real event ingestion through the real Codex adapter.
    let ingested = ingest_session(&db, adapter_for(Agent::Codex), &mut s).unwrap();
    assert!(ingested > 0, "the fixture really ingested events");

    let after = stored(&db, &s.id);
    assert_eq!(after.project_id, before.project_id);
    assert_eq!(after.workspace_path_id, before.workspace_path_id);
    assert_eq!(
        attacher.calls(),
        1,
        "no Project work per event batch (§1.11)"
    );
    let path_after = db
        .get_workspace_path(after.workspace_path_id.as_ref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(path_after.last_seen_at, path_before.last_seen_at);
    assert_eq!(path_after.project_id, path_before.project_id);
}

// ------------------------------------------------ 6/7/8. binding → path list

#[test]
fn binding_a_session_adds_its_missing_workstream_path() {
    let (_d, db) = temp_db("bind-add");
    project(&db, "p1", "repo");
    let attacher = Scripted::new("p1");
    let s = discover(&db, &attacher, "s1", Some("/repo/app")).0;
    let w = workstream(&db, "w1");
    // The user chose this path themselves; it holds position 0.
    let main = db
        .tx(|tx| insert_workspace_path_conn(tx, &normalize_path("/repo/main").unwrap(), "p1"))
        .unwrap();
    db.tx(|tx| {
        append_workstream_path_conn(tx, "w1", &main, workstream_path_source::USER)?;
        Ok(())
    })
    .unwrap();

    bind(&db, &s.id, &w, "related");

    let list = path_list(&db, &w);
    assert_eq!(
        list.iter().map(|p| p.1).collect::<Vec<_>>(),
        vec![0, 1],
        "an existing path keeps its slot; the Session's path appends last"
    );
    assert_eq!(list[0].0, main);
    assert_eq!(list[0].2, workstream_path_source::USER);
    assert_eq!(
        list[1],
        (
            path_identity_of("/repo/app").unwrap(),
            1,
            workstream_path_source::SESSION.to_string()
        )
    );
    assert_eq!(
        claim(&db, &s.id, &w),
        Some(row_id_at(&db, &w, 1)),
        "the binding records the WorkstreamPath that brought it in"
    );
}

#[test]
fn the_first_bound_session_path_becomes_primary() {
    let (_d, db) = temp_db("bind-primary");
    project(&db, "p1", "repo");
    let attacher = Scripted::new("p1");
    let s = discover(&db, &attacher, "s1", Some("/repo/app")).0;
    let w = workstream(&db, "w1");
    assert!(
        path_list(&db, &w).is_empty(),
        "a fresh Workstream has no paths"
    );

    bind(&db, &s.id, &w, "primary");

    // §19-7: an empty list yields position 0 — "became primary" needs no special
    // case, which is why there is no is_primary column to fall out of agreement.
    assert_eq!(
        path_list(&db, &w),
        vec![(
            path_identity_of("/repo/app").unwrap(),
            0,
            workstream_path_source::SESSION.to_string()
        )]
    );
    assert_eq!(
        db.primary_workspace_path_id(&w).unwrap(),
        path_identity_of("/repo/app")
    );
    assert_eq!(claim(&db, &s.id, &w), Some(row_id_at(&db, &w, 0)));
}

#[test]
fn a_later_bound_session_path_appends_as_secondary() {
    let (_d, db) = temp_db("bind-secondary");
    project(&db, "p1", "repo");
    let attacher = Scripted::new("p1");
    let w = workstream(&db, "w1");
    let first = discover(&db, &attacher, "s1", Some("/repo/app")).0;
    let second = discover(&db, &attacher, "s2", Some("/repo/service")).0;

    bind(&db, &first.id, &w, "primary");
    bind(&db, &second.id, &w, "related");

    assert_eq!(
        path_list(&db, &w)
            .into_iter()
            .map(|p| (p.0, p.1))
            .collect::<Vec<_>>(),
        vec![
            (path_identity_of("/repo/app").unwrap(), 0),
            (path_identity_of("/repo/service").unwrap(), 1),
        ]
    );
    assert_eq!(
        db.primary_workspace_path_id(&w).unwrap(),
        path_identity_of("/repo/app"),
        "a later joiner never takes the primary slot"
    );
    assert_eq!(
        claim(&db, &second.id, &w),
        Some(row_id_at(&db, &w, 1)),
        "the secondary joiner claims the secondary entry, not the primary one"
    );
}

#[test]
fn binding_reuses_a_path_that_is_already_in_the_list() {
    let (_d, db) = temp_db("bind-reuse");
    project(&db, "p1", "repo");
    let attacher = Scripted::new("p1");
    let s = discover(&db, &attacher, "s1", Some("/repo/app")).0;
    let path = path_identity_of("/repo/app").unwrap();
    let w = workstream(&db, "w1");
    let row = db
        .tx(|tx| append_workstream_path_conn(tx, "w1", &path, workstream_path_source::USER))
        .unwrap();

    bind(&db, &s.id, &w, "related");

    let list = path_list(&db, &w);
    assert_eq!(list.len(), 1, "no duplicate row for the same path");
    assert_eq!(
        list[0].2,
        workstream_path_source::USER.to_string(),
        "a Session cannot launder the provenance of a path the user chose"
    );
    assert_eq!(claim(&db, &s.id, &w), Some(row.id));
}

#[test]
fn an_automatic_binding_never_grows_the_path_list() {
    // §42.3-M2 — path lists grow from a user action or an explicit bind only.
    let (_d, db) = temp_db("bind-auto");
    project(&db, "p1", "repo");
    let attacher = Scripted::new("p1");
    let s = discover(&db, &attacher, "s1", Some("/repo/app")).0;
    let w = workstream(&db, "w1");
    let main = db
        .tx(|tx| insert_workspace_path_conn(tx, &normalize_path("/repo/main").unwrap(), "p1"))
        .unwrap();
    db.tx(|tx| {
        append_workstream_path_conn(tx, "w1", &main, workstream_path_source::USER)?;
        Ok(())
    })
    .unwrap();

    record_user_binding(&db, &s.id, &w, "related", binding_source::AUTO, 0.6).unwrap();

    assert_eq!(
        path_list(&db, &w),
        vec![(main, 0, workstream_path_source::USER.to_string())],
        "the classifier's guess did not add a path the user never chose"
    );
    assert_eq!(
        claim(&db, &s.id, &w),
        None,
        "and the binding honestly reports that no path brought it in"
    );
}

// --------------------------------------- 9. unbinding preserves the path list

#[test]
fn unbinding_a_session_preserves_the_workstream_path() {
    let (_d, db) = temp_db("unbind");
    project(&db, "p1", "repo");
    let attacher = Scripted::new("p1");
    let s = discover(&db, &attacher, "s1", Some("/repo/app")).0;
    let w = workstream(&db, "w1");
    bind(&db, &s.id, &w, "primary");
    let path = path_identity_of("/repo/app").unwrap();
    assert_eq!(path_list(&db, &w).len(), 1);

    db.unbind(&s.id, &w).unwrap();

    assert!(db.bindings_for_session(&s.id).unwrap().is_empty());
    assert_eq!(
        path_list(&db, &w),
        vec![(path, 0, workstream_path_source::SESSION.to_string())],
        "§1.9: removing a binding never removes a path"
    );
    assert!(
        db.binding_removal_exists(&s.id, &w).unwrap(),
        "the removal stays a permanent negative decision"
    );
    // The Session and its workspace facts are untouched by a bind/unbind.
    let after = stored(&db, &s.id);
    assert_eq!(after.workspace_path_id, path_identity_of("/repo/app"));
    assert_eq!(after.project_id.as_deref(), Some("p1"));
}

#[test]
fn replace_bindings_removing_a_row_keeps_its_path() {
    let (_d, db) = temp_db("unbind-replace");
    project(&db, "p1", "repo");
    let attacher = Scripted::new("p1");
    let s = discover(&db, &attacher, "s1", Some("/repo/app")).0;
    let w = workstream(&db, "w1");
    bind(&db, &s.id, &w, "related");

    replace_session_bindings(&db, &s.id, &[]).unwrap();

    assert!(db.bindings_for_session(&s.id).unwrap().is_empty());
    assert_eq!(path_list(&db, &w).len(), 1);
    assert_eq!(db.list_workspace_paths().unwrap().len(), 1);
    assert!(db.binding_removal_exists(&s.id, &w).unwrap());
}

// --------------------------------- 10. the claim is exact equality (§42.3-M1)

#[test]
fn the_binding_records_the_exact_workstream_path_id() {
    // /repo and /repo/frontend are two WorkspacePaths and both can be in one
    // list. A Session at /repo/frontend claims the /repo/frontend row — never
    // the prefix, never a "longest match": equality.
    let (_d, db) = temp_db("exact");
    project(&db, "p1", "repo");
    let attacher = Scripted::new("p1");
    let w = workstream(&db, "w1");
    let outer = discover(&db, &attacher, "outer", Some("/repo")).0;
    let inner = discover(&db, &attacher, "inner", Some("/repo/frontend")).0;
    let outer_path = path_identity_of("/repo").unwrap();
    let inner_path = path_identity_of("/repo/frontend").unwrap();
    assert_ne!(outer_path, inner_path);
    let outer_row = db
        .tx(|tx| append_workstream_path_conn(tx, "w1", &outer_path, workstream_path_source::USER))
        .unwrap();

    bind(&db, &inner.id, &w, "related");
    bind(&db, &outer.id, &w, "related");

    let list = path_list(&db, &w);
    assert_eq!(list.len(), 2, "both spellings coexist in the list");
    assert_eq!(list[0].0, outer_path);
    assert_eq!(list[1].0, inner_path);
    let inner_claim = claim(&db, &inner.id, &w).expect("inner claim");
    assert_eq!(inner_claim, db.list_workstream_paths(&w).unwrap()[1].id);
    assert_ne!(inner_claim, outer_row.id, "no prefix matching");
    assert_eq!(claim(&db, &outer.id, &w), Some(outer_row.id));
}

#[test]
fn a_claim_must_be_a_path_of_that_workstream_and_of_that_session() {
    let (_d, db) = temp_db("claim-guard");
    project(&db, "p1", "repo");
    let attacher = Scripted::new("p1");
    let s = discover(&db, &attacher, "s1", Some("/repo/app")).0;
    let other = discover(&db, &attacher, "s2", Some("/repo/other")).0;
    let w = workstream(&db, "w1");
    let own = db
        .tx(|tx| {
            append_workstream_path_conn(
                tx,
                "w1",
                &path_identity_of("/repo/app").unwrap(),
                workstream_path_source::USER,
            )
        })
        .unwrap();
    let others_row = db
        .tx(|tx| {
            append_workstream_path_conn(
                tx,
                "w1",
                &path_identity_of("/repo/other").unwrap(),
                workstream_path_source::USER,
            )
        })
        .unwrap();

    // Naming a WorkstreamPath that is not in this Workstream's list is refused…
    let err =
        resolve_binding_path_conn(db.conn(), &s.id, "w-empty", Some(&own.id), false).unwrap_err();
    assert!(!err.to_string().is_empty());
    // …and so is a row that is in the list but belongs to another Session.
    let err =
        resolve_binding_path_conn(db.conn(), &s.id, &w, Some(&others_row.id), false).unwrap_err();
    assert!(!err.to_string().is_empty());
    // The honest answer for the Session's own path:
    assert_eq!(
        resolve_binding_path_conn(db.conn(), &s.id, &w, Some(&own.id), false).unwrap(),
        Some(own.id.clone())
    );
    // And a path already in the list needs no append to be claimed, even when
    // appending is disallowed.
    assert_eq!(
        resolve_binding_path_conn(db.conn(), &other.id, &w, None, false).unwrap(),
        Some(others_row.id)
    );
}

#[test]
fn a_legacy_null_claim_keeps_working_and_is_repaired_by_a_real_rebind() {
    let (_d, db) = temp_db("legacy-null");
    project(&db, "p1", "repo");
    let attacher = Scripted::new("p1");
    let s = discover(&db, &attacher, "s1", Some("/repo/app")).0;
    let w = workstream(&db, "w1");
    let path = path_identity_of("/repo/app").unwrap();
    let row = db
        .tx(|tx| append_workstream_path_conn(tx, "w1", &path, workstream_path_source::MIGRATION))
        .unwrap();

    // A v11-era binding: no claim at all.
    db.bind(&SessionWorkstreamBinding {
        session_id: s.id.clone(),
        workstream_id: w.clone(),
        role: "primary".into(),
        source: binding_source::USER_ASSIGNED.into(),
        confidence: 1.0,
        workstream_path_id: None,
        last_seen_revision: Some("rev-3".into()),
        last_sync_cursor: 11,
        created_at: "2026-09-01T00:00:00Z".into(),
        last_used_at: "2026-09-02T00:00:00Z".into(),
    })
    .unwrap();
    assert_eq!(binding(&db, &s.id, &w).workstream_path_id, None);

    // (1) It is out of reach of a path deletion: removing a path may not destroy
    // a binding we cannot prove came from it (§42.3-M1/M2).
    let unbound = db
        .tx(|tx| remove_workstream_path_conn(tx, "w1", &row.id))
        .unwrap();
    assert_eq!(unbound, 0);
    assert!(path_list(&db, &w).is_empty());
    let kept = binding(&db, &s.id, &w);
    assert_eq!(kept.role, "primary");
    assert_eq!(kept.last_sync_cursor, 11);

    // (2) Re-saving it grows nothing: a kept binding is not a join (§1.8), so the
    // path the user removed is not silently put back.
    replace_session_bindings(
        &db,
        &s.id,
        &[DesiredBinding {
            workstream_id: w.clone(),
            role: "primary".into(),
            workstream_path_id: None,
        }],
    )
    .unwrap();
    assert!(path_list(&db, &w).is_empty());
    assert_eq!(binding(&db, &s.id, &w).workstream_path_id, None);

    // (3) §5.6 — once the path is in the list again, a save records the claim
    // that was always true. Nothing else about the row moves.
    let back = db
        .tx(|tx| append_workstream_path_conn(tx, "w1", &path, workstream_path_source::USER))
        .unwrap();
    replace_session_bindings(
        &db,
        &s.id,
        &[DesiredBinding {
            workstream_id: w.clone(),
            role: "primary".into(),
            workstream_path_id: None,
        }],
    )
    .unwrap();
    let repaired = binding(&db, &s.id, &w);
    assert_eq!(repaired.workstream_path_id, Some(back.id));
    assert_eq!(repaired.last_sync_cursor, 11, "provenance and cursors kept");
    assert_eq!(repaired.created_at, "2026-09-01T00:00:00Z");
    assert_eq!(
        path_list(&db, &w),
        vec![(path, 0, workstream_path_source::USER.to_string())]
    );

    // (4) Now that the claim is provable, deleting that path does take the
    // binding — which is exactly what §1.6 promises, and what NULL was hiding.
    let after = db
        .tx(|tx| remove_workstream_path_conn(tx, "w1", &row_id_at(&db, "w1", 0)))
        .unwrap();
    assert_eq!(after, 1);
    assert!(db.bindings_for_session(&s.id).unwrap().is_empty());
}

// ------------------------------------------- 12. cwd drift never grows a list

#[test]
fn cwd_drift_nulls_the_claim_and_never_grows_the_users_path_list() {
    let (_d, db) = temp_db("drift");
    project(&db, "p1", "repo");
    let attacher = Scripted::new("p1");
    let s = discover(&db, &attacher, "s1", Some("/repo/a")).0;
    let w = workstream(&db, "w1");
    bind(&db, &s.id, &w, "primary");
    assert_eq!(path_list(&db, &w).len(), 1);
    assert!(claim(&db, &s.id, &w).is_some());

    // Re-discovery reads the transcript again: the Session really did move.
    let (again, is_new) = discover(&db, &attacher, "s1", Some("/repo/b"));
    assert!(!is_new);
    assert_eq!(again.id, s.id);
    let after = stored(&db, &s.id);
    assert_eq!(after.cwd.as_deref(), Some("/repo/b"));
    assert_eq!(after.workspace_path_id, path_identity_of("/repo/b"));
    assert_eq!(after.project_id.as_deref(), Some("p1"), "same Project");
    assert_eq!(
        claim(&db, &s.id, &w),
        None,
        "§42.3-M2: the claim went back to NULL"
    );
    assert_eq!(
        path_list(&db, &w).len(),
        1,
        "…and the user's Workstream did NOT gain /repo/b"
    );
    assert_eq!(
        binding(&db, &s.id, &w).role,
        "primary",
        "the binding itself is user intent and stays"
    );

    // Drifting onto a path that IS in the list re-points the claim — exact match
    // again, and still no growth.
    discover(&db, &attacher, "s1", Some("/repo/a"));
    assert_eq!(path_list(&db, &w).len(), 1);
    assert!(claim(&db, &s.id, &w).is_some());
}

#[test]
fn an_interrupted_drift_converges_without_double_applying() {
    // The attach and the claim repair are one transaction; if a caller moved the
    // path alone, replaying the reconciliation must settle on the same state.
    let (_d, db) = temp_db("drift2");
    project(&db, "p1", "repo");
    let attacher = Scripted::new("p1");
    let s = discover(&db, &attacher, "s1", Some("/repo/a")).0;
    let w = workstream(&db, "w1");
    bind(&db, &s.id, &w, "related");

    db.tx(|tx| {
        attach_session_workspace_path_conn(tx, &s.id, path_identity_of("/repo/b").as_deref())
    })
    .unwrap();
    assert!(
        claim(&db, &s.id, &w).is_some(),
        "stale by construction: the path moved, the claim did not"
    );

    for _ in 0..3 {
        db.tx(|tx| reconcile_binding_paths_conn(tx, &s.id)).unwrap();
    }
    assert_eq!(claim(&db, &s.id, &w), None);
    assert_eq!(path_list(&db, &w).len(), 1);

    // Discovery then agrees, and the seam changes nothing further.
    let moved = db
        .tx(|tx| attach_session_conn(tx, &attacher, &s.id, Some("/repo/b")))
        .unwrap();
    assert!(!moved, "already attached: no write, no churn");
    assert_eq!(path_list(&db, &w).len(), 1);
}

// ------------------------------------------------ 13/14. replace semantics

#[test]
fn replace_bindings_keeps_provenance_and_adds_paths_only_for_new_rows() {
    let (_d, db) = temp_db("replace");
    project(&db, "p1", "repo");
    let attacher = Scripted::new("p1");
    let s = discover(&db, &attacher, "s1", Some("/repo/app")).0;
    let kept = workstream(&db, "w-kept");
    let added = workstream(&db, "w-added");
    let kept_path = db
        .tx(|tx| {
            append_workstream_path_conn(
                tx,
                "w-kept",
                &path_identity_of("/repo/app").unwrap(),
                workstream_path_source::USER,
            )
        })
        .unwrap();
    db.bind(&SessionWorkstreamBinding {
        session_id: s.id.clone(),
        workstream_id: kept.clone(),
        role: "primary".into(),
        source: binding_source::USER_ASSIGNED.into(),
        confidence: 1.0,
        workstream_path_id: Some(kept_path.id.clone()),
        last_seen_revision: Some("rev-9".into()),
        last_sync_cursor: 21,
        created_at: "2026-09-01T00:00:00Z".into(),
        last_used_at: "2026-09-02T00:00:00Z".into(),
    })
    .unwrap();

    replace_session_bindings(
        &db,
        &s.id,
        &[
            DesiredBinding {
                workstream_id: kept.clone(),
                role: "primary".into(),
                workstream_path_id: None,
            },
            DesiredBinding {
                workstream_id: added.clone(),
                role: "related".into(),
                workstream_path_id: None,
            },
        ],
    )
    .unwrap();

    let kept_row = binding(&db, &s.id, &kept);
    assert_eq!(kept_row.created_at, "2026-09-01T00:00:00Z");
    assert_eq!(kept_row.last_seen_revision.as_deref(), Some("rev-9"));
    assert_eq!(kept_row.last_sync_cursor, 21);
    assert_eq!(kept_row.workstream_path_id, Some(kept_path.id));
    let added_row = binding(&db, &s.id, &added);
    assert_eq!(added_row.source, binding_source::USER_ASSIGNED);
    assert_eq!(added_row.confidence, 1.0);
    assert_eq!(
        path_list(&db, &added),
        vec![(
            path_identity_of("/repo/app").unwrap(),
            0,
            workstream_path_source::SESSION.to_string()
        )],
        "the added join created its WorkstreamPath at position 0"
    );
    assert_eq!(
        path_list(&db, &kept).len(),
        1,
        "a kept binding never grew its Workstream's list"
    );
}

#[test]
fn replace_bindings_rejects_an_unknown_role_without_leaving_half_state() {
    let (_d, db) = temp_db("replace-atomic");
    project(&db, "p1", "repo");
    let attacher = Scripted::new("p1");
    let s = discover(&db, &attacher, "s1", Some("/repo/app")).0;
    let kept = workstream(&db, "w-kept");
    let other = workstream(&db, "w-other");
    bind(&db, &s.id, &kept, "primary");

    let err = replace_session_bindings(
        &db,
        &s.id,
        &[
            DesiredBinding {
                workstream_id: other.clone(),
                role: "related".into(),
                workstream_path_id: None,
            },
            DesiredBinding {
                workstream_id: kept.clone(),
                role: "boss".into(),
                workstream_path_id: None,
            },
        ],
    )
    .unwrap_err();
    assert!(!err.to_string().is_empty());
    let rows = db.bindings_for_session(&s.id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].workstream_id, kept);
    assert_eq!(rows[0].role, "primary");
    assert!(
        path_list(&db, &other).is_empty(),
        "the rolled-back WorkstreamPath did not survive either"
    );
}

#[test]
fn repeated_binds_are_idempotent_for_the_path_list() {
    // Retry safety: the same user action applied twice must not double-apply.
    let (_d, db) = temp_db("idempotent");
    project(&db, "p1", "repo");
    let attacher = Scripted::new("p1");
    let s = discover(&db, &attacher, "s1", Some("/repo/app")).0;
    let w = workstream(&db, "w1");
    for _ in 0..3 {
        bind(&db, &s.id, &w, "related");
    }
    assert_eq!(path_list(&db, &w).len(), 1);
    assert_eq!(db.bindings_for_session(&s.id).unwrap().len(), 1);
    assert_eq!(db.list_workspace_paths().unwrap().len(), 1);
}

// ------------------------------------------------ product-level discovery

#[test]
fn reconcile_discovers_and_attaches_sessions_to_workspace_paths() {
    let dir = unique_dir("reconcile");
    let root = dir.join("sessions");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("rollout-2026-09-13-x-1-2-3-4.jsonl"),
        format!(
            "{}\n{}\n",
            codex_meta_line("reconciled-1", "/repo/app"),
            codex_user_line("hello")
        ),
    )
    .unwrap();

    let db = Mutex::new(Db::open(&dir.join("noending.db")).unwrap());
    {
        let guard = db.lock().unwrap();
        project(&guard, "p-repo", "repo");
        guard
            .add_ingest_source(Agent::Codex, &root.to_string_lossy(), true)
            .unwrap();
    }
    let attacher = Arc::new(Scripted::new("p-repo"));
    // The one test that needs the app-wide seam; every other test injects one.
    let _ = register_workspace_attacher(attacher.clone());

    let engine = {
        let guard = db.lock().unwrap();
        SyncEngine::from_settings(&guard)
    };
    let seen = AtomicUsize::new(0);
    reconcile_with_engine(&db, &engine, &LaunchWorkspace::default(), &|_| {
        seen.fetch_add(1, Ordering::SeqCst);
    })
    .unwrap();

    let guard = db.lock().unwrap();
    let s = guard
        .find_session_by_agent_id(Agent::Codex, "reconciled-1")
        .unwrap()
        .expect("discovered through the real adapter");
    assert_eq!(s.workspace_path_id, path_identity_of("/repo/app"));
    assert_eq!(s.project_id.as_deref(), Some("p-repo"));
    assert_eq!(s.cwd.as_deref(), Some("/repo/app"));
    assert!(seen.load(Ordering::SeqCst) > 0);
    assert!(attacher.calls() >= 1);
}

// ───────────────────────────────────────────────────────── 方案 §42.5 T1–T4
//
// The mechanical anti-drift assertions. `project_id_writers_are_confined_to_the_
// derived_doors` above is T2; these are the rest of the same family, and they
// live beside it because a guard nobody can find is a guard nobody keeps.
//
// Every one of them reads *source text*, so each must be proven to fire: see the
// per-test note on what mutation breaks it. A grep guard that matches prose is
// worse than no guard — it reports confidence it has not earned.

/// Strip `//` and `///` lines and flatten the rest, so a doc comment that
/// *explains* a prohibition can never be reported as violating it.
fn code_only(text: &str) -> String {
    text.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The argument list of `tauri::generate_handler![ … ]` as entries, comments
/// removed. Counting *entries* rather than "mentions" is the whole point: the
/// retired commands are named repeatedly in this file's own rationale comments
/// (§43.9-3), and a substring grep would read that as them being registered.
fn registered_commands() -> Vec<String> {
    let src = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("lib.rs"),
    )
    .expect("lib.rs");
    let marker = "generate_handler![";
    let start = src.find(marker).expect("generate_handler! block");
    let body = &src[start + marker.len()..];
    let mut depth = 1usize;
    let mut end = 0usize;
    for (i, c) in body.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    assert!(end > 0, "the generate_handler! macro must be closed");
    body[..end]
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with("//"))
        .flat_map(|l| l.split(','))
        .map(|e| e.trim().trim_end_matches(';').to_string())
        .filter(|e| !e.is_empty())
        .collect()
}

/// §42.5-T1 — the retired Project / Session / Workstream commands are not
/// registered. Breaks if any of them is added back to `generate_handler!`.
#[test]
fn retired_commands_are_not_registered() {
    let retired = [
        "create_project",
        "delete_project",
        "update_project",
        "assign_session_project",
        "suggest_session_project",
        "merge_workstreams",
        "add_project_resource",
        "list_project_resources",
        "remove_project_resource",
    ];
    let live = registered_commands();
    assert!(
        live.len() > 40,
        "the handler list must actually be parsed, not silently emptied: {} entries",
        live.len()
    );
    let leaked: Vec<&String> = live
        .iter()
        .filter(|entry| {
            let last = entry.rsplit("::").next().unwrap_or(entry);
            retired.iter().any(|r| *r == last)
        })
        .collect();
    assert!(
        leaked.is_empty(),
        "§11: these left the product API and must not be re-registered: {leaked:?}"
    );
    // The read side of the same freeze: the UI still gets exactly three Project
    // commands, and `list_sessions` is the Session list.
    for kept in ["list_projects", "get_project_detail", "rename_project"] {
        assert!(
            live.iter().any(|e| e.ends_with(&format!("::{kept}"))),
            "{kept} must stay registered (§11)"
        );
    }
}

/// The substring from `window[at] == '('` through its matching close paren.
fn balanced_paren(window: &str, from: usize) -> Option<&str> {
    let open = window[from..].find('(')? + from;
    let mut depth = 0usize;
    for (i, c) in window[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&window[open..=open + i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// §42.5-T3 — every product write to `workstreams` names `lifecycle`
/// explicitly (§42.2-E2: an upgraded database keeps the historical
/// `DEFAULT 'open'`, so the column default is not a safety net), and nothing
/// writes the retired vocabulary. `WHERE lifecycle = 'open'` in the v12
/// migration is a *read* of the old value and is required, so the write form is
/// matched as `SET lifecycle = …`.
#[test]
fn workstream_writes_name_lifecycle_and_never_write_the_retired_vocabulary() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut offenders: Vec<String> = Vec::new();
    for file in rust_files(&manifest.join("src")) {
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let code = code_only(&text);
        let mut from = 0usize;
        while let Some(i) = code[from..].find("INSERT INTO workstreams") {
            let at = from + i;
            let window = &code[at..code.len().min(at + 400)];
            // The *column list*, matched-parenthesis. Scanning to the last `)`
            // in the window would swallow the `ON CONFLICT DO UPDATE SET …
            // lifecycle = ?5` clause, which mentions lifecycle without the
            // INSERT naming the column — and the guard would pass on a write
            // that violates it.
            let columns = match balanced_paren(window, "INSERT INTO workstreams".len()) {
                Some(c) => c,
                None => "",
            };
            if !columns.contains("lifecycle") {
                let rel = file.strip_prefix(manifest).unwrap().display().to_string();
                offenders.push(format!("{rel}: INSERT INTO workstreams without lifecycle"));
            }
            from = at + "INSERT INTO workstreams".len();
        }
        for bad in [
            "SET lifecycle = 'open'",
            "SET lifecycle = \"open\"",
            "SET lifecycle = 'abandoned'",
            "lifecycle: \"open\"",
            "lifecycle: \"abandoned\"",
        ] {
            if code.contains(bad) {
                let rel = file.strip_prefix(manifest).unwrap().display().to_string();
                offenders.push(format!("{rel}: writes {bad:?}"));
            }
        }
    }
    assert!(offenders.is_empty(), "§42.5-T3 / §5.7: {offenders:?}");
}

/// §42.5-T4 — `git` is reached only through the executable resolver (M10: a
/// Finder-launched macOS app has no shell PATH, and `git` is exactly the binary
/// that lives in Homebrew), and the Git *protocol* strings live in
/// `workspace/` alone. Breaks on `Command::new("git")` anywhere, or on a second
/// `--git-common-dir` implementation outside the resolver.
#[test]
fn git_is_only_reached_through_the_resolver_and_only_from_workspace() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut offenders: Vec<String> = Vec::new();
    for file in rust_files(&manifest.join("src")) {
        let rel = file
            .strip_prefix(manifest)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let code = code_only(&std::fs::read_to_string(&file).unwrap_or_default());
        if code.contains("Command::new(\"git\")") {
            offenders.push(format!("{rel}: Command::new(\"git\")"));
        }
        for token in ["--git-common-dir", "worktree list", "--show-toplevel"] {
            if code.contains(token) && rel != "src/workspace/resolver.rs" {
                offenders.push(format!("{rel}: {token}"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "§42.3-M9/M10/M11 put exactly one place allowed to run git: {offenders:?}"
    );
    // Prove the scan reaches the file that is allowed to do it: if this assert
    // ever fails, the walker is broken and every assertion above is vacuous.
    let resolver = std::fs::read_to_string(manifest.join("src/workspace/resolver.rs")).unwrap();
    assert!(code_only(&resolver).contains("--git-common-dir"));
}

/// §42.3-M31 + §43.9-2 — no launcher entry point may resolve a launch without
/// being told the NoEnding Home. The Home-less spellings were the risk: with
/// `default_workspace: None`, §13's third tier is simply absent, and M30's
/// "do not teach the Workstream the fallback directory" gate in `apply_match`
/// *inverts* to growing the list. Enforced here as well as by the compiler,
/// because `LaunchWorkspace::default()` is how the mistake would come back.
#[test]
fn no_launch_entry_point_can_forget_the_home() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let launcher = std::fs::read_to_string(manifest.join("src/launcher/mod.rs")).expect("launcher");
    let offenders: Vec<&str> = launcher
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("//"))
        .filter(|l| l.contains("LaunchWorkspace::default()"))
        .collect();
    assert!(
        offenders.is_empty(),
        "§13 tier 3 must be injected, never defaulted: {offenders:?}"
    );
    // Every public prepare/launch entry point takes the workspace explicitly.
    for name in [
        "prepare_new_in",
        "prepare_resume_in",
        "launch_prepared_in",
        "launch_prepared_with_in",
    ] {
        let sig = launcher
            .split(&format!("pub fn {name}(")[..])
            .nth(1)
            .unwrap_or_else(|| panic!("{name} must exist"));
        let head = &sig[..sig.find("->").unwrap_or(sig.len())];
        assert!(
            head.contains("workspace: &LaunchWorkspace"),
            "{name} must require the LaunchWorkspace rather than assume one"
        );
    }
    // The Home-less wrappers themselves are gone, not merely unused.
    for gone in [
        "pub fn prepare_new(",
        "pub fn prepare_resume(",
        "pub fn launch_prepared(",
        "pub fn new_session(",
        "pub fn resume_session(",
        "pub fn resolve_new_session_cwd(",
    ] {
        assert!(
            !launcher.contains(gone),
            "{gone} is a Home-less launcher entry point and was removed (§43.8 E-(f))"
        );
    }
}

/// §2/§3, and the §25 row "migration occurs before DB open": the Home pointer
/// decides *which file* `Db::open` is handed, so a relocation that has not been
/// applied yet must not be skipped past. Breaks if `Db::open` is hoisted above
/// `prepare_home`, which is precisely the §41 failure mode §43.4-4 exists to
/// avoid (the user's history looks deleted).
#[test]
fn noending_home_is_resolved_before_the_database_opens() {
    let src = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"))
        .expect("lib.rs");
    let home = src
        .find("prepare_home(")
        .expect("prepare_home call in setup");
    let db = src.find("Db::open(").expect("Db::open call in setup");
    assert!(
        home < db,
        "§3: relocation runs inside prepare_home, so it must precede opening the db ({home} vs {db})"
    );
}

/// §1.10 — a Session is a member of a Project because its own path says so.
/// The derived cache is a cache: a row still holding only a v11 hand-attached
/// label is not in that Project, and `list_sessions` must agree with
/// `get_project_detail` instead of disagreeing with it (§42.3-M29).
#[test]
fn a_session_with_only_the_cached_project_id_is_not_a_project_member() {
    let (_d, db) = temp_db("cache-is-not-membership");
    project(&db, "p-real", "Real");
    let attacher = Scripted::new("p-real");
    let (member, _) = discover(&db, &attacher, "s-member", Some("/cache-authority/repo"));
    let (ghost, _) = discover(&db, &attacher, "s-ghost", Some("/cache-authority/repo"));
    // Make the second row look like what it is in a real upgraded database: a
    // v11 Session whose only Project fact was the label a person attached by
    // hand, and no resolvable path. `attach_session_workspace_path_conn(None)`
    // is the door that clears both, so the label is re-written afterwards the
    // way the v11 data already had it.
    db.tx(|tx| attach_session_workspace_path_conn(tx, &ghost.id, None))
        .unwrap();
    db.0.execute(
        "UPDATE sessions SET project_id = 'p-real' WHERE id = ?1",
        rusqlite::params![ghost.id],
    )
    .unwrap();
    assert_eq!(
        stored(&db, &ghost.id).workspace_path_id,
        None,
        "sanity: the ghost has no path, only the cached label"
    );

    let in_project = db
        .list_sessions(noending::storage::SessionFilter {
            scope: Default::default(),
            project_id: Some("p-real".into()),
            agent: None,
        })
        .unwrap();
    let ids: Vec<&str> = in_project.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![member.id.as_str()],
        "the cached row must not be listed as a member the chain denies"
    );
    assert_eq!(
        db.get_session(&ghost.id)
            .unwrap()
            .unwrap()
            .project_id
            .as_deref(),
        Some("p-real"),
        "the stale cache value is left alone — this is a read-side fix, not a rewrite"
    );
}
