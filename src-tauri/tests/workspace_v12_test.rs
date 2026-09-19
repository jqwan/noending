//! Wave 0 contract tests for Workspace Domain v0.2 (方案 §15, §25).
//!
//! These lock the four rules the v12 migration and the new storage layer exist
//! to establish. Later waves may build on them but must not be able to break
//! them silently:
//!
//! 1. WorkspacePath identity is deterministic and filesystem-free (§42.3-M8).
//! 2. `workstream_paths.position` is a dense ordering with one primary.
//! 3. `sessions.project_id` is a derived cache, never an independent fact (§29).
//! 4. The v12 migration folds the old authorities in, is replay-safe, and is
//!    backed up before it touches data (§42.3-M5).

use noending::domain::{
    git_state, workstream_lifecycle, workstream_path_source, Agent, Project,
    ProjectAffinityEvidence, ProjectResource, Session, Workstream,
};
use noending::error::Result;
use noending::storage::workspace::{
    insert_workspace_path_conn, reassign_workspace_path_project_conn,
};
use noending::storage::workstream_paths::{
    append_workstream_path_conn, remove_workstream_path_conn, reorder_workstream_paths_conn,
};
use noending::storage::{new_id, now, Db, SCHEMA_VERSION};
use noending::workspace::{
    normalize_path, normalize_path_with, path_identity, path_identity_of, path_identity_with,
    path_key, NormalizeOpts, PathStyle,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

fn unique_dir(tag: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "noending-ws12-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn temp_db() -> (PathBuf, Db) {
    let dir = unique_dir("db");
    let db = Db::open(&dir.join("noending.db")).expect("temp db");
    (dir, db)
}

/// Ensure a WorkspacePath under Project `project_id`, returning its id.
fn ensure_path(db: &Db, canonical: &str, project_id: &str) -> Result<String> {
    db.tx(|tx| insert_workspace_path_conn(tx, canonical, project_id))
}

fn session(id: &str, cwd: Option<&str>) -> Session {
    let mut s = Session::new(
        id.into(),
        Agent::Codex,
        format!("src-{id}"),
        format!("/raw/{id}.jsonl"),
    );
    s.cwd = cwd.map(Into::into);
    s
}

// ------------------------------------------------------- 1. path identity

#[test]
fn workspace_path_identity_is_deterministic_and_filesystem_free() {
    // 方案 §44.3-C1 — the alias spellings below are Unix-shaped, so the style is
    // pinned rather than inherited: on a Windows host `/Users/dev/projects/app` is
    // not an alias of anything, it is a root-relative path with a key of its own,
    // and the loop would have been comparing three directories instead of one.
    // What the *host* door stores is pinned separately, as data, by
    // `unix_v12_path_identity_vectors_are_stable`.
    let unix = |raw: &str| {
        normalize_path_with(
            raw,
            NormalizeOpts {
                style: Some(PathStyle::Unix),
                base: None,
                home: None,
            },
        )
    };
    let a = path_identity_with("/Users/dev/projects/app", PathStyle::Unix);
    assert_eq!(
        a,
        path_identity_with("/Users/dev/projects/app", PathStyle::Unix)
    );
    assert!(a.starts_with("path-"));
    assert_eq!(a.len(), "path-".len() + 32, "id is 'path-' + 32 hex");

    // Every non-canonical spelling of the same directory is the same identity,
    // and no access() is involved — the path need not exist.
    let again = vec![
        "/Users/dev/projects/app/",
        "/Users/dev//projects/./app",
        "/Users/dev/projects/app/../app",
        "//Users/dev/projects/./app/",
    ];
    for raw in again {
        let canonical = unix(raw).unwrap_or_else(|| panic!("{raw} is absolute"));
        assert_eq!(
            path_identity_with(&canonical, PathStyle::Unix),
            a.as_str(),
            "{raw}"
        );
    }

    // Nested directories are distinct WorkspacePaths (方案 §42.3-M1).
    assert_ne!(path_identity("/repo"), path_identity("/repo/frontend"));
    // Unresolvable spelling yields no identity rather than a fabricated one.
    assert_eq!(path_identity_of("   "), None);
    assert_eq!(normalize_path(""), None);

    // Unix is case-sensitive: `/Repo` and `/repo` are two Workspaces. The
    // platform-specific rules (Windows separators, drive case, UNC, `~`) are
    // covered by the unit tests in workspace/identity.rs, which is the only
    // module allowed to spell them.
    assert_ne!(path_key("/Repo"), path_key("/repo"));
    assert_eq!(path_key("C:\\Users\\dev\\app"), "C:/Users/dev/app");
}

// -------------------------------------------------- 2. position invariant

#[test]
fn workstream_path_positions_are_dense_with_one_primary() {
    let (_d, db) = temp_db();
    db.upsert_project(&Project::new("p1".into(), "app"))
        .unwrap();
    db.upsert_workstream(&Workstream::new("w1".into(), "Demo"))
        .unwrap();

    let add = |raw: &str, source: &str| {
        let canonical = normalize_path(raw).unwrap();
        let path_id = ensure_path(&db, &canonical, "p1").unwrap();
        let row = db
            .tx(|tx| append_workstream_path_conn(tx, "w1", &path_id, source))
            .unwrap();
        (row.id, path_id)
    };
    let (first_row, first) = add("/repo", workstream_path_source::USER);
    let second = {
        let (_row, id) = add("/repo/frontend", workstream_path_source::USER);
        id
    };
    let (_third_row, third) = add("/repo/backend", workstream_path_source::USER);

    let positions = || -> Vec<(String, i64, String)> {
        db.list_workstream_paths("w1")
            .unwrap()
            .into_iter()
            .map(|p| (p.workspace_path_id, p.position, p.source))
            .collect()
    };
    assert_eq!(
        positions().iter().map(|p| p.1).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(db.primary_workspace_path_id("w1").unwrap(), Some(first));

    // Appending the same WorkspacePath again is a no-op: no duplicate row, and
    // the original provenance survives (a later session cannot launder 'user').
    let again = ensure_path(&db, "/repo/frontend", "p1").unwrap();
    db.tx(|tx| append_workstream_path_conn(tx, "w1", &again, workstream_path_source::SESSION))
        .unwrap();
    let after = positions();
    assert_eq!(after.len(), 3);
    assert_eq!(after[1].2, workstream_path_source::USER);

    // Removing the primary recompacts: exactly one position 0, no gaps.
    db.tx(|tx| remove_workstream_path_conn(tx, "w1", &first_row))
        .unwrap();
    let after = positions();
    assert_eq!(
        after.iter().map(|p| (p.0.clone(), p.1)).collect::<Vec<_>>(),
        vec![(second.clone(), 0), (third.clone(), 1)]
    );

    // Reorder takes the full list; a partial list is refused rather than
    // silently leaving half the set where it was.
    db.tx(|tx| reorder_workstream_paths_conn(tx, "w1", &[third.clone(), second.clone()]))
        .unwrap();
    assert_eq!(
        db.list_workstream_paths("w1")
            .unwrap()
            .into_iter()
            .map(|p| (p.workspace_path_id, p.position))
            .collect::<Vec<_>>(),
        vec![(third.clone(), 0), (second.clone(), 1)]
    );
    assert_eq!(db.primary_workspace_path_id("w1").unwrap(), Some(third));
    assert!(db
        .tx(|tx| reorder_workstream_paths_conn(tx, "w1", std::slice::from_ref(&second)))
        .is_err());
}

// ------------------------------------------------- 3. derived Project cache

#[test]
fn session_project_id_is_derived_from_its_workspace_path() {
    let (_d, db) = temp_db();
    db.upsert_project(&Project::new("pA".into(), "alpha"))
        .unwrap();
    db.upsert_project(&Project::new("pB".into(), "beta"))
        .unwrap();
    let path_id = ensure_path(&db, "/work/alpha", "pA").unwrap();

    let mut s = session("s1", Some("/work/alpha"));
    s.workspace_path_id = Some(path_id.clone());
    // The caller lies about the Project; the path-derived value must win.
    s.project_id = Some("pB".into());
    db.upsert_session(&s).unwrap();

    let stored = db.get_session("s1").unwrap().unwrap();
    assert_eq!(stored.project_id.as_deref(), Some("pA"));
    assert_eq!(stored.workspace_path_id.as_deref(), Some(path_id.as_str()));

    // Re-ingest without a path must not wipe the derived cache: the row keeps
    // its WorkspacePath and the Project is re-derived through it.
    s.workspace_path_id = None;
    s.project_id = None;
    db.upsert_session(&s).unwrap();
    assert_eq!(
        db.get_session("s1").unwrap().unwrap().project_id.as_deref(),
        Some("pA")
    );

    // Moving the WorkspacePath to another Project refreshes every Session that
    // reads through it (§1.11) — the cache follows, it does not lead.
    db.tx(|tx| reassign_workspace_path_project_conn(tx, &path_id, "pB"))
        .unwrap();
    assert_eq!(
        db.get_session("s1").unwrap().unwrap().project_id.as_deref(),
        Some("pB")
    );
}

#[test]
fn session_without_cwd_gets_no_workspace_path() {
    let (_d, db) = temp_db();
    db.upsert_project(&Project::new("pInv".into(), "invented"))
        .unwrap();
    let mut s = session("s2", None);
    s.project_id = Some("pInv".into());
    db.upsert_session(&s).unwrap();
    let stored = db.get_session("s2").unwrap().unwrap();
    assert_eq!(stored.workspace_path_id, None);
    // No path ⇒ nothing to derive from, so the caller's value is just a cache
    // write. v0.2 never fabricates a WorkspacePath from a default (§5.5), which
    // is why this Session keeps a Project it can be reconciled from later.
    assert_eq!(stored.project_id.as_deref(), Some("pInv"));
}

// ------------------------------------------------------------- 4. migration

/// A database shaped exactly the way v0.1 left it — the four tables written by
/// hand with their **v11** column sets, not a v12 database with the version
/// dial turned back.
///
/// The distinction is the whole point: `migrate()` runs `CREATE TABLE IF NOT
/// EXISTS`, which cannot extend an existing table, so on a real upgrade the v12
/// columns only appear via the idempotent ALTER list. A fixture built on a fresh
/// v12 file therefore tests the *data* branches and nothing else, and it let a
/// `CREATE UNIQUE INDEX ON projects(git_id)` that ran before its own ALTER ship
/// to a real database. Build the old shape, and ordering bugs have somewhere to
/// show up.
fn legacy_db(dir: &Path) -> PathBuf {
    let path = dir.join("noending.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        let ts = "2026-01-01T00:00:00Z";
        conn.execute_batch(&format!(
            r#"
            PRAGMA foreign_keys = ON;
            PRAGMA user_version = 11;
            CREATE TABLE projects (
              id TEXT PRIMARY KEY, name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '',
              archived INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL, updated_at TEXT NOT NULL
            );
            CREATE TABLE workstreams (
              id TEXT PRIMARY KEY, project_id TEXT REFERENCES projects(id), title TEXT NOT NULL,
              description TEXT NOT NULL DEFAULT '', lifecycle TEXT NOT NULL DEFAULT 'open',
              visibility TEXT NOT NULL DEFAULT 'normal', created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
              default_cwd TEXT
            );
            CREATE TABLE sessions (
              id TEXT PRIMARY KEY, agent TEXT NOT NULL, agent_session_id TEXT NOT NULL,
              title TEXT, cwd TEXT, project_id TEXT REFERENCES projects(id), raw_path TEXT NOT NULL,
              parent_agent_session_id TEXT, started_at TEXT, last_activity_at TEXT,
              UNIQUE(agent, agent_session_id)
            );
            CREATE TABLE session_workstream_bindings (
              session_id TEXT NOT NULL, workstream_id TEXT NOT NULL, role TEXT NOT NULL,
              source TEXT NOT NULL, confidence REAL NOT NULL DEFAULT 0,
              last_seen_revision TEXT, last_sync_cursor INTEGER NOT NULL DEFAULT 0,
              created_at TEXT NOT NULL, last_used_at TEXT NOT NULL,
              PRIMARY KEY (session_id, workstream_id)
            );
            INSERT INTO projects VALUES ('legacy-alpha','Alpha','',0,'{ts}','{ts}');
            INSERT INTO projects VALUES ('ghost','Ghost','',1,'{ts}','{ts}');
            INSERT INTO workstreams VALUES ('w-legacy','legacy-alpha','Old','d','abandoned','archived','{ts}','{ts}','/old/repo');
            INSERT INTO sessions VALUES ('s-legacy','codex','src-1','T','/old/repo','legacy-alpha','/raw/s1.jsonl',NULL,'{ts}','{ts}');
            INSERT INTO session_workstream_bindings VALUES ('s-legacy','w-legacy','primary','user_explicit',1,NULL,0,'{ts}','{ts}');
            "#
        ))
        .unwrap();
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            11
        );
        // The columns v12 adds really are absent — that is what the ALTER list
        // is for, and what an index created too early would trip over.
        assert!(conn
            .query_row("SELECT git_id FROM projects LIMIT 1", [], |_| Ok(()))
            .is_err());
    }
    path
}

#[test]
fn v12_migration_folds_the_old_authorities() {
    let dir = unique_dir("mig");
    let path = legacy_db(&dir);
    let db = Db::open(&path).expect("migrate to v12");
    assert_eq!(
        db.0.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION
    );

    // §7.3 — default_cwd became the position-0 WorkstreamPath, source migration.
    let paths = db.list_workstream_paths("w-legacy").unwrap();
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].position, 0);
    assert_eq!(paths[0].source, workstream_path_source::MIGRATION);

    // §7.2 — the Session's cwd became a Project-backed WorkspacePath, and the
    // legacy Project was preferred over inventing a new one (§7.1).
    let wp = db
        .get_workspace_path(&paths[0].workspace_path_id)
        .unwrap()
        .expect("workspace path");
    // §42.3-M8 rule 7: the migration stored the host's canonical form of that cwd,
    // so the expectation has to go through the same normalizer instead of being a
    // Unix literal — `/old/repo` is `/old\repo` on a Windows runner.
    assert_eq!(wp.canonical_path, normalize_path("/old/repo").unwrap());
    assert_eq!(wp.project_id, "legacy-alpha");
    assert_eq!(wp.git_state, git_state::NONE);

    // §5.6 — the binding records which WorkstreamPath brought it in, exactly.
    let binding = db
        .bindings_for_session("s-legacy")
        .unwrap()
        .into_iter()
        .next()
        .expect("binding");
    assert_eq!(binding.workstream_path_id, Some(paths[0].id.clone()));

    // §1.10 — the derived cache agrees with the path's Project.
    let s = db.get_session("s-legacy").unwrap().unwrap();
    assert_eq!(s.workspace_path_id, Some(wp.id.clone()));
    assert_eq!(s.project_id.as_deref(), Some("legacy-alpha"));

    // §5.7 — abandoned folded into completed and its recycle bin survived;
    // the vocabulary is now active/completed only.
    let old = db.get_workstream("w-legacy").unwrap().unwrap();
    assert_eq!(old.lifecycle, workstream_lifecycle::COMPLETED);
    assert_eq!(old.visibility, "archived");

    // §7.4 — a legacy Project with no WorkspacePath is deleted rather than kept
    // alive by a fabricated path. The Workstream it "owned" survives: a Project
    // was never a lifecycle owner.
    assert!(db.get_project("ghost").unwrap().is_none());
    assert!(db.get_workstream("w-legacy").unwrap().is_some());

    // §42.3-M5 — the destructive branches are preceded by a file backup.
    assert!(dir.join("noending.db.pre-v12.bak").exists());

    // Replay: reopening at v12 changes nothing (migrate() has no transaction,
    // so every statement above must be converge-on-repeat).
    drop(db);
    let db2 = Db::open(&path).unwrap();
    assert_eq!(db2.list_workstream_paths("w-legacy").unwrap().len(), 1);
    assert_eq!(db2.list_workspace_paths().unwrap().len(), 1);
    assert!(db2.get_project("ghost").unwrap().is_none());
    assert_eq!(
        db2.bindings_for_session("s-legacy").unwrap()[0].workstream_path_id,
        Some(paths[0].id.clone())
    );
}

#[test]
fn migration_resolves_a_binding_without_a_matching_path_to_null() {
    let dir = unique_dir("mig-null");
    let path = {
        let p = legacy_db(&dir);
        // A second binding whose Session has no path for that Workstream at all.
        let db = Db::open(&p).unwrap();
        let ts = now();
        db.0.execute(
            "INSERT INTO sessions (id, agent, agent_session_id, cwd, raw_path, started_at, last_activity_at)
             VALUES ('s-loose','codex','src-2','/elsewhere','/raw/s2.jsonl',?1,?1)",
            [&ts],
        )
        .unwrap();
        db.0.execute(
            "INSERT INTO session_workstream_bindings
               (session_id, workstream_id, role, source, confidence, last_seen_revision, last_sync_cursor, created_at, last_used_at)
             VALUES ('s-loose','w-legacy','primary','automatic',0.4,0,0,?1,?1)",
            [&ts],
        )
        .unwrap();
        db.0.execute("PRAGMA user_version = 11", []).unwrap();
        drop(db);
        p
    };
    let db = Db::open(&path).unwrap();
    // M1: the pointer is exact equality. Nothing was appended to the list to
    // make this binding look resolved, and the binding itself is kept.
    let loose = db
        .bindings_for_session("s-loose")
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(loose.workstream_path_id, None);
    assert_eq!(db.list_workstream_paths("w-legacy").unwrap().len(), 1);
    // /elsewhere still became its own WorkspacePath from the Session cwd.
    assert_eq!(db.list_workspace_paths().unwrap().len(), 2);
}

// ------------------------------------------------- 5. single-writer plumbing

#[test]
fn a_project_with_resources_and_evidence_can_be_deleted() {
    // §42.3-M4: both child tables reference projects(id) NOT NULL with no
    // cascade, and `delete_project` historically never touched evidence — so a
    // Project that had ever been suggested was undeletable.
    let (_d, db) = temp_db();
    db.upsert_project(&Project::new("p-del".into(), "del"))
        .unwrap();
    db.add_resource(&ProjectResource {
        id: new_id(),
        project_id: "p-del".into(),
        kind: "repository".into(),
        uri: Some("/x".into()),
        metadata: serde_json::json!({}),
        created_at: now(),
    })
    .unwrap();
    db.insert_evidence(&ProjectAffinityEvidence {
        id: new_id(),
        session_id: None,
        workstream_id: None,
        project_id: "p-del".into(),
        evidence_type: "user_correction".into(),
        source: "test".into(),
        score: 1.0,
        created_at: now(),
    })
    .unwrap();

    db.delete_project("p-del").unwrap();
    assert!(db.get_project("p-del").unwrap().is_none());
    assert!(db.list_resources("p-del").unwrap().is_empty());
}

#[test]
fn editing_a_workstream_cannot_move_it_or_retarget_its_path() {
    // §42.2-E6: `upsert_workstream_conn` used to re-commit project_id and
    // default_cwd on every title edit, which is how the two authorities drifted.
    let (_d, db) = temp_db();
    db.upsert_project(&Project::new("pX".into(), "X")).unwrap();
    db.upsert_workstream(&Workstream::new("w-freeze".into(), "t"))
        .unwrap();

    let mut w = db.get_workstream("w-freeze").unwrap().unwrap();
    w.project_id = Some("pX".into());
    w.default_cwd = Some("/somewhere".into());
    w.lifecycle = workstream_lifecycle::COMPLETED.into();
    w.title = "renamed".into();
    db.upsert_workstream(&w).unwrap();

    let stored = db.get_workstream("w-freeze").unwrap().unwrap();
    assert_eq!(stored.title, "renamed");
    assert_eq!(stored.lifecycle, workstream_lifecycle::COMPLETED);
    assert_eq!(stored.project_id, None);
    assert_eq!(stored.default_cwd, None);
}

#[test]
fn a_whole_object_project_write_can_neither_clear_nor_retarget_git_identity() {
    // §1.3 in its other direction: losing `.git` must not detach a Project, and
    // `update_project`-style whole-object writes must not point a Project at a
    // different Git family either. Adopting a family is a policy decision with
    // its own first-set-only door.
    let (_d, db) = temp_db();
    let mut p = Project::new("p-git".into(), "alpha");
    p.git_id = Some("g-one".into());
    db.upsert_project(&p).unwrap();

    let mut cleared = p.clone();
    cleared.git_id = None;
    db.upsert_project(&cleared).unwrap();
    assert_eq!(
        db.get_project("p-git").unwrap().unwrap().git_id.as_deref(),
        Some("g-one")
    );

    let mut retargeted = p.clone();
    retargeted.git_id = Some("g-two".into());
    db.upsert_project(&retargeted).unwrap();
    assert_eq!(
        db.get_project("p-git").unwrap().unwrap().git_id.as_deref(),
        Some("g-one")
    );
}

#[test]
fn listing_workstreams_by_project_is_a_path_projection() {
    // E4: `list_workstreams(project_id)` keeps its signature but its meaning is
    // now "any Workstream that reaches this Project through any of its paths".
    let (_d, db) = temp_db();
    db.upsert_project(&Project::new("pP".into(), "P")).unwrap();
    db.upsert_project(&Project::new("qQ".into(), "Q")).unwrap();
    for id in ["w-primary", "w-secondary", "w-none"] {
        db.upsert_workstream(&Workstream::new(id.into(), id))
            .unwrap();
    }

    let in_p = ensure_path(&db, "/p/one", "pP").unwrap();
    let in_q = ensure_path(&db, "/q/zero", "qQ").unwrap();
    db.tx(|tx| {
        // w-primary reaches pP through its primary path…
        append_workstream_path_conn(tx, "w-primary", &in_p, workstream_path_source::USER)?;
        // …w-secondary only through its secondary one.
        append_workstream_path_conn(tx, "w-secondary", &in_q, workstream_path_source::USER)?;
        append_workstream_path_conn(tx, "w-secondary", &in_p, workstream_path_source::USER)?;
        Ok(())
    })
    .unwrap();

    let mut ids: Vec<String> = db
        .list_workstreams(Some("pP"))
        .unwrap()
        .into_iter()
        .map(|w| w.id)
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["w-primary", "w-secondary"]);
    assert_eq!(db.list_workstreams(None).unwrap().len(), 3);

    // The position-0 relationship is what the card / detail projections report.
    let roles =
        noending::storage::workstream_paths::project_roles_for_workstream(&db.0, "w-secondary")
            .unwrap();
    assert_eq!(
        roles.iter().cloned().collect::<Vec<_>>(),
        vec![("qQ".to_string(), true), ("pP".to_string(), false)]
    );
}
