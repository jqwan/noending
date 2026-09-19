//! v12 → v13 migration (Session Lifecycle & Deletion v0.1).
//!
//! v13 is purely additive: `sessions.trashed_at` (NULL = Normal for every
//! pre-existing row) and the empty `session_deletion_jobs` coordination
//! table. A v12-shaped database must open, gain both, stamp user_version 13,
//! and reopen idempotently — with no data migration of any kind (方案 §44).

use rusqlite::Connection;
use std::path::Path;

fn v12_db(dir: &std::path::Path) -> String {
    let path = dir.join("legacy.db");
    let conn = Connection::open(&path).unwrap();
    conn.pragma_update(None, "user_version", 12).unwrap();
    // Only the sessions table in its v12 shape (no trashed_at): migrate()'s
    // canonical CREATE IF NOT EXISTS batch creates everything else fresh.
    conn.execute_batch(
        r#"
        CREATE TABLE projects (
          id TEXT PRIMARY KEY,
          name TEXT NOT NULL,
          description TEXT NOT NULL DEFAULT '',
          archived INTEGER NOT NULL DEFAULT 0,
          git_id TEXT,
          name_customized INTEGER NOT NULL DEFAULT 0,
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL
        );
        CREATE TABLE sessions (
          id TEXT PRIMARY KEY,
          agent TEXT NOT NULL,
          agent_session_id TEXT NOT NULL,
          title TEXT,
          cwd TEXT,
          workspace_path_id TEXT,
          project_id TEXT REFERENCES projects(id),
          raw_path TEXT NOT NULL,
          parent_agent_session_id TEXT,
          started_at TEXT,
          last_activity_at TEXT,
          UNIQUE(agent, agent_session_id)
        );
        INSERT INTO sessions (id, agent, agent_session_id, title, raw_path)
          VALUES ('s-legacy', 'codex', 'legacy-1', 'Pre-v13 session', '/tmp/legacy.jsonl');
        "#,
    )
    .unwrap();
    conn.close().unwrap();
    path.to_string_lossy().to_string()
}

#[test]
fn v12_database_gains_trash_column_and_deletion_jobs_table() {
    let dir =
        std::env::temp_dir().join(format!("noending-v13-mig-{}", noending::storage::new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = v12_db(&dir);

    let db = noending::storage::Db::open(Path::new(&path)).unwrap();
    let version: i64 = db
        .conn()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, noending::storage::SCHEMA_VERSION);
    assert_eq!(version, 13);

    // Every pre-existing session is Normal (trashed_at NULL) — nothing is
    // hidden or lost by the upgrade.
    let trashed: Option<String> = db
        .conn()
        .query_row(
            "SELECT trashed_at FROM sessions WHERE id = 's-legacy'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(trashed, None);
    let s = db.get_session("s-legacy").unwrap().unwrap();
    assert!(!s.is_trashed());

    // The coordination table is present and usable.
    let jobs: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM session_deletion_jobs", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(jobs, 0);
    db.tx(|tx| {
        noending::storage::session_jobs::insert_deletion_job_conn(
            tx,
            &noending::domain::SessionDeletionJob {
                id: "job-1".into(),
                session_id: "s-legacy".into(),
                state: "prepared".into(),
                plan_json: "{}".into(),
                last_error: None,
                created_at: "2026-09-20T00:00:00Z".into(),
                updated_at: "2026-09-20T00:00:00Z".into(),
            },
        )
    })
    .unwrap();
}

#[test]
fn v13_reopen_is_idempotent() {
    let dir = std::env::temp_dir().join(format!(
        "noending-v13-reopen-{}",
        noending::storage::new_id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = v12_db(&dir);

    let first = noending::storage::Db::open(Path::new(&path)).unwrap();
    drop(first);
    // Second open replays every idempotent statement without error.
    let second = noending::storage::Db::open(Path::new(&path)).unwrap();
    let version: i64 = second
        .conn()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 13);
    assert!(second.get_session("s-legacy").unwrap().is_some());
}
