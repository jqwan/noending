//! Opt-in migration rehearsal against a COPY of a real database (§26).
//!
//! CI never has a real corpus, so this test skips unless
//! `NOENDING_V12_DOGFOOD` points at a database *copy*. It exists because the
//! v12 migration is the one part of this phase that rewrites user data, and a
//! synthetic fixture can only prove the branches it was told about — the values
//! people actually accumulate (a `/.claude`, a `/private/tmp` with 400 Sessions,
//! an archived Workstream with a `default_cwd`) are the interesting ones.
//!
//! The target must be under the system temp directory: that keeps an
//! accidentally-live database path from being opened read-write by this test.
//!
//! ```text
//! cp ~/Library/Application\ Support/app.noending.desktop/noending.db{,-wal} /tmp/rehearsal/
//! NOENDING_V12_DOGFOOD=/tmp/rehearsal/noending.db cargo test --test v12_dogfood_test -- --nocapture
//! ```

use noending::storage::Db;
use std::path::{Path, PathBuf};

fn target() -> Option<PathBuf> {
    let raw = std::env::var_os("NOENDING_V12_DOGFOOD").map(PathBuf::from)?;
    // `/tmp` is a symlink to `/private/tmp` on macOS, so it is not
    // `env::temp_dir()`; both are legitimate places to put a copy.
    let temp_roots = [std::env::temp_dir(), PathBuf::from("/tmp")];
    assert!(
        temp_roots.iter().any(|root| raw.starts_with(root)),
        "NOENDING_V12_DOGFOOD must point at a copy under a temp directory \
         ({temp_roots:?}), never a live database"
    );
    Some(raw)
}

fn counts(db: &Db) -> Vec<(String, i64)> {
    let one = |sql: &str| -> i64 { db.0.query_row(sql, [], |r| r.get(0)).unwrap_or(-1) };
    vec![
        ("projects".into(), one("SELECT COUNT(*) FROM projects")),
        (
            "workspace_paths".into(),
            one("SELECT COUNT(*) FROM workspace_paths"),
        ),
        (
            "workstream_paths".into(),
            one("SELECT COUNT(*) FROM workstream_paths"),
        ),
        (
            "sessions_with_path".into(),
            one("SELECT COUNT(*) FROM sessions WHERE workspace_path_id IS NOT NULL"),
        ),
        (
            "sessions_with_project".into(),
            one("SELECT COUNT(*) FROM sessions WHERE project_id IS NOT NULL"),
        ),
        (
            "sessions_without_cwd".into(),
            one("SELECT COUNT(*) FROM sessions WHERE cwd IS NULL OR TRIM(cwd) = ''"),
        ),
        ("events".into(), one("SELECT COUNT(*) FROM session_events")),
        (
            "bindings".into(),
            one("SELECT COUNT(*) FROM session_workstream_bindings"),
        ),
        (
            "bindings_with_claim".into(),
            one("SELECT COUNT(*) FROM session_workstream_bindings WHERE workstream_path_id IS NOT NULL"),
        ),
        (
            "launch_intents".into(),
            one("SELECT COUNT(*) FROM launch_intents"),
        ),
        (
            "context_items".into(),
            one("SELECT COUNT(*) FROM context_items"),
        ),
        (
            "cursors".into(),
            one("SELECT COUNT(*) FROM session_cursors"),
        ),
    ]
}

#[test]
fn v12_migration_rehearsal_on_a_real_copy() {
    let Some(path) = target() else {
        eprintln!("[dogfood] NOENDING_V12_DOGFOOD unset — skipping");
        return;
    };
    let side = PathBuf::from(format!("{}.v11side", path.display()));
    std::fs::copy(&path, &side).expect("snapshot the pre-migration copy");

    let before = {
        let db = Db::open(&path).expect("migrate the copy");
        let version: i64 =
            db.0.query_row("PRAGMA user_version", [], |r| r.get(0))
                .unwrap();
        assert_eq!(version, noending::storage::SCHEMA_VERSION);
        counts(&db)
    };

    for (name, n) in &before {
        eprintln!("[dogfood] {name} = {n}");
    }
    let project_list: Vec<(String, i64, i64)> = {
        let db = Db::open(&path).unwrap();
        let mut st = db
            .0
            .prepare("SELECT p.name, (SELECT COUNT(*) FROM workspace_paths w WHERE w.project_id = p.id), (SELECT COUNT(*) FROM sessions s WHERE s.project_id = p.id) FROM projects p ORDER BY 2 DESC")
            .unwrap();
        let rows = st
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        rows
    };
    eprintln!("[dogfood] projects after migration:");
    for (name, paths, sessions) in &project_list {
        eprintln!("[dogfood]   {sessions:>4} sessions · {paths:>2} paths · {name}");
    }

    let db = Db::open(&path).unwrap();

    // The invariants §1.1/§1.2 claim, checked against real accumulated junk
    // rather than a fixture that was designed to satisfy them.
    let pathless_projects: i64 = db
        .0
        .query_row(
            "SELECT COUNT(*) FROM projects WHERE id NOT IN (SELECT project_id FROM workspace_paths)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        pathless_projects, 0,
        "§1.2: a Project owns at least one path"
    );

    let orphan_paths: i64 = db
        .0
        .query_row(
            "SELECT COUNT(*) FROM workspace_paths WHERE project_id IS NULL OR project_id NOT IN (SELECT id FROM projects)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(orphan_paths, 0, "§1.1: every WorkspacePath has one Project");

    let stale_lifecycle: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM workstreams WHERE lifecycle NOT IN ('active','completed')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stale_lifecycle, 0, "§5.7: only active/completed survive");

    let unprojected_sessions: i64 = db
        .0
        .query_row(
            "SELECT COUNT(*) FROM sessions s
              WHERE s.workspace_path_id IS NOT NULL
                AND s.project_id IS NOT (SELECT project_id FROM workspace_paths w WHERE w.id = s.workspace_path_id)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        unprojected_sessions, 0,
        "§1.10: the cache agrees with the path it was derived from"
    );

    let bad_claims: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM session_workstream_bindings b
              WHERE b.workstream_path_id IS NOT NULL
                AND NOT EXISTS (SELECT 1 FROM workstream_paths p
                                 WHERE p.id = b.workstream_path_id
                                   AND p.workstream_id = b.workstream_id)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        bad_claims, 0,
        "§42.3-M1: a claim points at that Workstream's own path"
    );

    // Nothing the migration is not allowed to touch may have shrunk.
    let original = Db::open(&side).expect("reopen the untouched snapshot");
    for (name, n) in counts(&original) {
        if matches!(
            name.as_str(),
            "events" | "bindings" | "launch_intents" | "context_items" | "cursors"
        ) {
            let after = before
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| *v)
                .unwrap();
            assert_eq!(after, n, "{name} must survive the migration untouched");
        }
    }
    assert!(
        Path::new(&format!("{}.pre-v12.bak", path.display())).exists(),
        "§42.3-M5: the file backup precedes the destructive branches"
    );
}

/// The same corpus, one step further: what the user actually sees after the
/// first Workspace Reconcile has run Git detection.
///
/// The v12 migration is lexical on purpose (§6), so immediately after upgrading
/// every working directory is its own Project. Git detection is what collapses
/// siblings into families (§8.3, §8.5) — and whether that is enough is an
/// empirical question about a real corpus, not a question about a fixture.
/// Read-only against the repositories: `git rev-parse` with optional locks off.
#[test]
#[ignore = "needs a real corpus copy and a real git; run deliberately"]
fn v12_reconcile_rehearsal_on_a_real_copy() {
    let Some(path) = target() else { return };
    let home = noending::workspace::home::NoEndingHome::new(
        &std::env::temp_dir().join("noending-rehearsal-home").display().to_string(),
        std::env::var("HOME").ok().as_deref(),
    )
    .expect("synthetic home");
    std::fs::create_dir_all(&home.root).ok();

    let db = std::sync::Mutex::new(Db::open(&path).expect("open the migrated copy"));
    let layer = noending::workspace::wiring::WorkspaceLayer::new(&home);
    let report = noending::workspace::project::reconcile_workspace_paths(
        &db,
        &layer.projection(),
        usize::MAX,
    );
    match report {
        Ok(r) => eprintln!(
            "[dogfood] reconcile: scanned {} · moved {} · discovered {} · failed {} · deleted {} paths / {} projects",
            r.scanned,
            r.moved_paths,
            r.discovered_paths.len(),
            r.failed.len(),
            r.outcome.deleted_paths.len(),
            r.outcome.deleted_projects.len()
        ),
        Err(e) => eprintln!("[dogfood] reconcile failed: {e}"),
    }

    let db = db.into_inner().unwrap();
    let mut st = db
        .0
        .prepare(
            "SELECT p.name, p.git_id IS NOT NULL,
                    (SELECT COUNT(*) FROM workspace_paths w WHERE w.project_id = p.id),
                    (SELECT COUNT(*) FROM sessions s WHERE s.project_id = p.id)
               FROM projects p ORDER BY 4 DESC",
        )
        .unwrap();
    let rows = st
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)? != 0,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    eprintln!("[dogfood] projects after reconcile:");
    for (name, git, paths, sessions) in &rows {
        eprintln!(
            "[dogfood]   {sessions:>4} sessions · {paths:>2} paths · {}· {name}",
            if *git { "git  " } else { "plain" }
        );
    }
    // No assertion on the shape of the list: worktrees Git discovered legitimately
    // bring Projects nobody has a Session in yet. What must never happen is a
    // Project without a path (§1.2), which the migration test already proves.
    assert!(!rows.is_empty(), "reconcile must not empty the registry");
}
