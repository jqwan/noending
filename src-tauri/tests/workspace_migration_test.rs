//! v11 → v12 migration falsifiability (方案 §7, §25 "v11 → v12", §26).
//!
//! `workspace_v12_test.rs` already proves the big fold: `default_cwd` →
//! WorkstreamPath[0], Session cwd → WorkspacePath, `abandoned` → `completed`,
//! pathless legacy Project deleted, replay-safety, the pre-v12 backup. This file
//! covers the §25 rows that had no assertion behind them — all four are about
//! *ordering* or about what happens **after** the migration, which no
//! fold-the-old-authorities test can see.
//!
//! The fixture is a hand-written real v11 shape (verified against a copy of the
//! user's own database, 方案 §43.6), not "a v12 database with `user_version`
//! dialled back": §43.6 records that the difference hid a P0.
//!
//! Temp databases only; nothing here opens a user Home (§42.3-M13).

use noending::domain::{workstream_lifecycle, Project};
use noending::storage::{Db, SCHEMA_VERSION};
use noending::workspace::project::{
    ensure_workspace_path, registry_is_consistent, UnrestrictedWorkspace,
};
use noending::workspace::{normalize_path, path_identity};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

fn unique_dir(tag: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "noending-mig3-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A v11 database: no `projects.git_id`, no `workspace_paths`, no
/// `workstream_paths`, `sessions.workspace_path_id` absent, and `lifecycle`
/// still carrying the retired `open` / `abandoned` vocabulary.
fn legacy_db(dir: &Path, rows: &str) -> PathBuf {
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
            INSERT INTO projects VALUES ('legacy-alpha','Alpha 项目','手写的名字',0,'{ts}','{ts}');
            INSERT INTO projects VALUES ('legacy-beta','Beta','other',0,'{ts}','{ts}');
            INSERT INTO workstreams VALUES ('w-old','legacy-alpha','Old','d','abandoned','archived','{ts}','{ts}','/legacy/repo/a');
            INSERT INTO sessions VALUES ('s-a','codex','src-a','TA','/legacy/repo/a','legacy-alpha','/raw/a.jsonl',NULL,'{ts}','{ts}');
            INSERT INTO sessions VALUES ('s-b','codex','src-b','TB','/legacy/repo/b','legacy-beta','/raw/b.jsonl',NULL,'{ts}','{ts}');
            INSERT INTO sessions VALUES ('s-free','codex','src-free','TF','/legacy/never/organised',NULL,'/raw/f.jsonl',NULL,'{ts}','{ts}');
            {rows}
            "#
        ))
        .unwrap();
        // Prove the fixture really lacks the v12 columns, or every assertion
        // below would be testing a database that never needed migrating.
        for missing in ["git_id", "workspace_path_id", "name_customized"] {
            assert!(
                conn.query_row(
                    &format!("SELECT {missing} FROM projects LIMIT 1"),
                    [],
                    |_| Ok(())
                )
                .is_err()
                    || conn
                        .query_row(
                            &format!("SELECT {missing} FROM sessions LIMIT 1"),
                            [],
                            |_| Ok(())
                        )
                        .is_err()
            );
        }
    }
    path
}

/// Open it, which is what runs the migration.
fn migrate(dir: &Path, rows: &str) -> Db {
    let db = Db::open(&legacy_db(dir, rows)).expect("v11 → v12");
    assert_eq!(
        db.conn()
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION,
        "the migration must have run"
    );
    db
}

fn name_customized(db: &Db, id: &str) -> Option<bool> {
    db.get_project(id).unwrap().map(|p| p.name_customized)
}

/// §25 "old Project ids/names preserved where paths exist" + "legacy names
/// marked customized".
///
/// §7.1's rule is that a pre-existing Project was named by a person, so
/// §37's automatic naming must never overwrite it again. Nothing else in the
/// suite reads this column after a migration, so dropping the `UPDATE
/// projects SET name_customized = 1` would have passed every existing test — and
/// the first reconcile would rename `Alpha 项目` to `a`.
#[test]
fn legacy_project_names_survive_and_are_marked_customized() {
    let dir = unique_dir("names");
    let db = migrate(&dir, "");

    let alpha = db.get_project("legacy-alpha").unwrap().expect("kept by id");
    assert_eq!(alpha.name, "Alpha 项目", "the name is not re-derived");
    assert!(alpha.name_customized, "§7.1: a person named it");
    let beta = db.get_project("legacy-beta").unwrap().expect("kept by id");
    assert_eq!(beta.name, "Beta");
    assert!(beta.name_customized);
    assert!(
        !beta.archived,
        "§42.2-E3: a Project has no lifecycle in v0.2"
    );

    // And the flag does what it exists for: re-observing the path under automatic
    // naming must not rename it.
    let canonical = normalize_path("/legacy/repo/a").unwrap();
    ensure_workspace_path(
        &db,
        &noending::domain::WorkspaceObservation {
            path_id: path_identity(&canonical),
            canonical_path: canonical.clone(),
            exists: true,
            git: noending::domain::GitDetection::None,
        },
        &UnrestrictedWorkspace,
    )
    .unwrap();
    assert_eq!(
        db.get_project("legacy-alpha").unwrap().unwrap().name,
        "Alpha 项目",
        "automatic naming must not overwrite a customized name"
    );
}

/// §25 "legacy project_id used only as initial migration evidence" — and the
/// ordering §7.1 states in one clause: "Set this BEFORE any new Project is
/// created, or the new ones would be marked too."
///
/// A Session with no legacy Project is the migration's own creation, so it must
/// arrive automatic-named and *not* customized. If the blanket UPDATE ever
/// moves below the path backfill, every migrated Project is frozen and §37 can
/// never rename any of them.
#[test]
fn a_project_the_migration_itself_creates_is_not_marked_customized() {
    let dir = unique_dir("order");
    let db = migrate(&dir, "");

    assert_eq!(
        name_customized(&db, "legacy-alpha"),
        Some(true),
        "pre-existing: §7.1 protects the name"
    );
    let created: Vec<(String, String, i64)> = db
        .conn()
        .prepare("SELECT id, name, name_customized FROM projects WHERE id NOT IN ('legacy-alpha','legacy-beta') ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!(
        created.len(),
        1,
        "exactly one Project is invented, for /legacy/never/organised: {created:?}"
    );
    let (id, name, customized) = &created[0];
    assert_eq!(*customized, 0, "§7.1 ordering: not customized");
    assert_eq!(name, "Organised", "§37: named from the path basename");
    assert_eq!(
        db.conn()
            .query_row(
                "SELECT wp.project_id FROM workspace_paths wp
                   JOIN sessions s ON s.workspace_path_id = wp.id WHERE s.id = 's-free'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        *id,
        "and the Session's cache points at the Project its own path owns (§1.10)"
    );
    registry_is_consistent(&db).expect("consistent after the migration");
}

/// §25 "Git reconcile can subsequently merge Projects".
///
/// The migration can only see lexically: it puts `/legacy/repo/a` and
/// `/legacy/repo/b` in two different legacy Projects. The first reconcile that
/// learns they are one Git family must merge the Projects — and per
/// §42.3-M26 a *path-backed* legacy Project is absorbed whole, while its
/// customized name wins the survivor slot (§37). This is the v12-specific half of
/// that, which `workspace_project_test.rs`'s §8.5 tests do not cover because
/// they start from an empty database.
#[test]
fn reconcile_after_the_migration_merges_the_legacy_projects() {
    let dir = unique_dir("merge");
    let db = migrate(&dir, "");
    let before: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0))
        .unwrap();
    assert_eq!(before, 3, "two inherited + one invented");

    for raw in ["/legacy/repo/a", "/legacy/repo/b"] {
        let canonical = normalize_path(raw).unwrap();
        ensure_workspace_path(
            &db,
            &noending::domain::WorkspaceObservation {
                path_id: path_identity(&canonical),
                canonical_path: canonical.clone(),
                exists: true,
                git: noending::domain::GitDetection::Detected {
                    common_dir: normalize_path("/legacy/repo/a/.git").unwrap(),
                    toplevel: Some(canonical.clone()),
                    kind: noending::domain::GitWorktreeKind::Main,
                    worktrees: vec![canonical],
                },
            },
            &UnrestrictedWorkspace,
        )
        .unwrap();
    }

    let survivors: Vec<Project> = db.list_projects().unwrap();
    assert_eq!(
        survivors.len(),
        2,
        "one family, one Project: the two legacy ones collapse (§8.5). got {:?}",
        survivors
            .iter()
            .map(|p| (&p.id, &p.name))
            .collect::<Vec<_>>()
    );
    let merged = survivors
        .iter()
        .find(|p| p.git_id.is_some())
        .expect("git-backed");
    assert!(
        db.get_workspace_path(&path_identity(&normalize_path("/legacy/repo/b").unwrap()))
            .unwrap()
            .is_some_and(|wp| wp.project_id == merged.id),
        "§42.3-M26: a path-backed legacy Project is absorbed whole, so /b moves"
    );
    assert!(
        merged.name_customized,
        "§37: the inherited name is what survives"
    );
    // Both Sessions' caches followed the path they hang off (§1.11).
    for id in ["s-a", "s-b"] {
        let cached: String = db
            .conn()
            .query_row(
                "SELECT s.project_id FROM sessions s
                   JOIN workspace_paths wp ON wp.id = s.workspace_path_id WHERE s.id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cached, merged.id, "{id} follows its own path");
    }
    registry_is_consistent(&db).expect("no zero-path Project left by the merge");
}

/// §25 "archived Workstream still restores previous lifecycle", end to end from
/// a v11 row — the `abandoned` + `archived` pair v0.2 folds into
/// `completed` + `archived`, and the recycle bin must still remember which one it
/// was when the user restores it (§1.13).
#[test]
fn a_migrated_archived_workstream_restores_the_lifecycle_it_was_migrated_to() {
    let dir = unique_dir("restore");
    let db = migrate(&dir, "");

    let migrated = db.get_workstream("w-old").unwrap().expect("survived");
    assert_eq!(migrated.lifecycle, workstream_lifecycle::COMPLETED);
    assert_eq!(migrated.visibility, "archived");

    noending::workspace::workstream::restore_workstream(&db, "w-old").unwrap();
    let restored = db.get_workstream("w-old").unwrap().unwrap();
    assert_eq!(restored.visibility, "normal");
    assert_eq!(
        restored.lifecycle,
        workstream_lifecycle::COMPLETED,
        "§1.13: restore changes visibility only, never lifecycle"
    );
    assert_eq!(
        restored.default_cwd.as_deref(),
        Some("/legacy/repo/a"),
        "§7.3: the frozen column keeps its value as migration evidence"
    );
    // …but it is not a launch authority any more (launch_cwd_test pins that).
    let paths: Vec<(i64, String)> = db
        .list_workstream_paths("w-old")
        .unwrap()
        .into_iter()
        .map(|p| (p.position, p.workspace_path_id))
        .collect();
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].0, 0);
}
