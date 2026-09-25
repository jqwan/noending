//! Database format contract (方案 §1–§4).
//!
//! NoEnding supports exactly one SQLite format generation at a time: an empty
//! file is created in the current format, a current-format database is used
//! untouched (startup never repairs schema), and anything else is refused
//! rather than upgraded.

use noending::storage::{Db, DATABASE_FORMAT_VERSION};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

struct TempDb {
    dir: PathBuf,
    path: PathBuf,
}

impl TempDb {
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn db_path(tag: &str) -> TempDb {
    let dir = std::env::temp_dir().join(format!(
        "noending-schema-{tag}-{}",
        noending::storage::new_id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    TempDb {
        path: dir.join("noending.db"),
        dir,
    }
}

fn has_column(conn: &Connection, table: &str, column: &str) -> bool {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    let names: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .map(|name| name.unwrap())
        .collect();
    names.iter().any(|name| name == column)
}

fn user_version(conn: &Connection) -> i64 {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap()
}

/// A brand-new database carries the current format generation and the current
/// structure — not an older shape that startup then has to reconcile.
#[test]
fn fresh_database_uses_current_format_generation() {
    let path = db_path("current");
    let db = Db::open(path.path()).unwrap();
    assert_eq!(user_version(&db.read()), DATABASE_FORMAT_VERSION);

    // 方案 §6.1 — the Session↔Workstream binding tables are gone, not carried
    // forward: a Session has at most one Owner Workstream, held on the Session
    // row itself.
    for table in [
        "project_resources",
        "project_affinity_evidence",
        "session_workstream_bindings",
        "session_binding_removals",
    ] {
        let exists: i64 = db
            .read()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 0, "retired table remains: {table}");
    }
    for (table, column) in [
        ("projects", "archived"),
        ("workstreams", "project_id"),
        ("workstreams", "default_cwd"),
        ("session_events", "legacy_identity_hash"),
        // §7 — WorkstreamPath.source existed only to record "session / launch
        // created this path", a side effect the single-owner model removed.
        ("workstream_paths", "source"),
        // §8.3 — a launch chooses ONE Owner Workstream, not a JSON array.
        ("launch_intents", "selected_workstream_ids"),
    ] {
        assert!(
            !has_column(&db.read(), table, column),
            "retired column remains: {table}.{column}"
        );
    }

    // 方案 §6.2 — the replacement is a single nullable column with an
    // ON DELETE SET NULL FK, indexed for the Workstream detail query.
    {
        let conn = db.read();
        assert!(
            has_column(&conn, "sessions", "owner_workstream_id"),
            "sessions.owner_workstream_id is missing"
        );
        let on_delete: String = conn
            .query_row("PRAGMA foreign_key_list(sessions)", [], |row| {
                Ok(format!(
                    "{}|{}",
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(6)?
                ))
            })
            .unwrap();
        assert!(
            on_delete.contains("workstreams") && on_delete.ends_with("SET NULL"),
            "owner FK must be workstreams(id) ON DELETE SET NULL, got {on_delete}"
        );
        let indexed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                  WHERE type = 'index' AND name = 'idx_sessions_owner_workstream'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(indexed, 1, "idx_sessions_owner_workstream is missing");
    }

    // 方案 §15.1 — a pending LaunchIntent's chosen Owner follows the same rule
    // as a Session's: deleting the Workstream nulls it. Without the FK the
    // intent would keep a dangling id, the match would fail forever and the
    // discovered Session would be left permanently ownerless.
    {
        let intent_owner_fk: String = db
            .read()
            .query_row(
                "SELECT \"table\" || '|' || on_delete FROM pragma_foreign_key_list('launch_intents')
                  WHERE \"from\" = 'owner_workstream_id'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert_eq!(
            intent_owner_fk, "workstreams|SET NULL",
            "launch_intents.owner_workstream_id must be workstreams(id) ON DELETE SET NULL"
        );
    }
}

/// Reopening a current-format database is a pure read of the format marker:
/// the data survives and startup runs no DDL at all, so nothing that is
/// missing can be silently "repaired" back into existence.
#[test]
fn current_format_database_reopens_without_reinitialization() {
    let path = db_path("reopen");
    let db = Db::open(path.path()).unwrap();
    db.write()
        .execute(
            "INSERT INTO settings (key, value) VALUES ('schema-test', 'persists')",
            [],
        )
        .unwrap();
    // A table startup must NOT recreate: if `initialize_schema` re-ran its DDL
    // batches on an existing database, `CREATE TABLE IF NOT EXISTS` would bring
    // it back and this assertion would catch it.
    db.write()
        .execute("DROP TABLE context_deliveries", [])
        .unwrap();
    drop(db);

    let reopened = Db::open(path.path()).unwrap();
    assert_eq!(user_version(&reopened.read()), DATABASE_FORMAT_VERSION);
    assert_eq!(
        reopened
            .read()
            .query_row(
                "SELECT value FROM settings WHERE key = 'schema-test'",
                [],
                |row| { row.get::<_, String>(0) }
            )
            .unwrap(),
        "persists"
    );
    let recreated: i64 = reopened
        .read()
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
              WHERE type = 'table' AND name = 'context_deliveries'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        recreated, 0,
        "startup re-ran schema creation on an existing database"
    );
}

/// A database built by another NoEnding generation is refused, never upgraded.
/// The error names both sides so the log says exactly which format was found.
#[test]
fn non_current_database_format_is_rejected() {
    let unsupported = DATABASE_FORMAT_VERSION + 1;
    let path = db_path("unsupported");
    let conn = Connection::open(path.path()).unwrap();
    conn.execute("CREATE TABLE projects (id TEXT PRIMARY KEY)", [])
        .unwrap();
    conn.pragma_update(None, "user_version", unsupported)
        .unwrap();
    drop(conn);

    let err = match Db::open(path.path()) {
        Ok(_) => panic!("a database in the format {unsupported} must be refused"),
        Err(e) => e.to_string(),
    };
    assert!(
        err.contains(&format!("database_format={unsupported}"))
            && err.contains(&format!("required_format={DATABASE_FORMAT_VERSION}")),
        "refusal must name the found and required formats, got: {err}"
    );
}

/// A non-empty SQLite file with no format marker is not an empty database: it
/// belongs to something else and is refused instead of overwritten.
#[test]
fn unversioned_nonempty_database_is_rejected() {
    let path = db_path("unversioned");
    let conn = Connection::open(path.path()).unwrap();
    conn.execute("CREATE TABLE projects (id TEXT PRIMARY KEY)", [])
        .unwrap();
    drop(conn);

    assert!(Db::open(path.path()).is_err());
}

/// Creation is all-or-nothing. A file where one of the creation statements
/// cannot run (here: `settings` is already taken by a view) must be left with
/// no version marker and no half-created schema, so the next start refuses the
/// file instead of trusting a partial database.
#[test]
fn failed_database_creation_rolls_back_completely() {
    let path = db_path("rollback");
    let conn = Connection::open(path.path()).unwrap();
    conn.execute("CREATE VIEW settings AS SELECT 1 AS value", [])
        .unwrap();
    drop(conn);

    assert!(Db::open(path.path()).is_err());

    let conn = Connection::open(path.path()).unwrap();
    let projects: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'projects'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(projects, 0, "failed creation left partial schema");
    assert_eq!(
        user_version(&conn),
        0,
        "failed creation left a version marker"
    );
}
