use noending::storage::{Db, SCHEMA_VERSION};
use rusqlite::Connection;
use std::path::PathBuf;

struct TempDb {
    dir: PathBuf,
    path: PathBuf,
}

impl TempDb {
    fn path(&self) -> &std::path::Path {
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
    let found = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .any(|name| name.map(|value| value == column).unwrap_or(false));
    found
}

#[test]
fn fresh_database_is_already_in_the_current_shape() {
    let path = db_path("current");
    let db = Db::open(path.path()).unwrap();
    let version: i64 = db
        .read()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, SCHEMA_VERSION);

    // 方案 §6.1 — the Session↔Workstream binding tables are gone, not
    // migrated: a Session has at most one Owner Workstream, held on the
    // Session row itself.
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

    db.write()
        .execute(
            "INSERT INTO settings (key, value) VALUES ('schema-test', 'persists')",
            [],
        )
        .unwrap();
    drop(db);
    let reopened = Db::open(path.path()).unwrap();
    assert_eq!(
        reopened
            .read()
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
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
    drop(reopened);
}

#[test]
fn unsupported_schema_versions_are_rejected_without_migration() {
    for (tag, version) in [("older", SCHEMA_VERSION - 1), ("newer", SCHEMA_VERSION + 1)] {
        let path = db_path(tag);
        let conn = Connection::open(path.path()).unwrap();
        conn.pragma_update(None, "user_version", version).unwrap();
        drop(conn);

        assert!(Db::open(path.path()).is_err());
    }
}

#[test]
fn unversioned_old_database_is_rejected() {
    let path = db_path("unversioned");
    let conn = Connection::open(path.path()).unwrap();
    conn.execute("CREATE TABLE projects (id TEXT PRIMARY KEY)", [])
        .unwrap();
    drop(conn);

    let result = Db::open(path.path());
    assert!(result.is_err());
}

#[test]
fn schema_initialization_rolls_back_on_error() {
    let path = db_path("rollback");
    let conn = Connection::open(path.path()).unwrap();
    conn.execute("CREATE TABLE projects (id TEXT PRIMARY KEY)", [])
        .unwrap();
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)
        .unwrap();
    drop(conn);

    assert!(Db::open(path.path()).is_err());

    let conn = Connection::open(path.path()).unwrap();
    let workstreams: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'workstreams'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(workstreams, 0, "failed initialization left partial schema");
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
}
