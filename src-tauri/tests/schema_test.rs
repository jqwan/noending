//! Database format contract (方案 §1–§4).
//!
//! NoEnding supports exactly one SQLite format generation at a time, identified
//! by the header pair (`application_id`, `user_version`) that creation stamps.
//! An empty file is created in that format; a database carrying the pair must
//! also still have the full structure; anything else — a foreign SQLite file, an
//! older generation, or an incomplete current-format database — is refused
//! instead of migrated or repaired.

use noending::storage::{Db, DATABASE_APPLICATION_ID, DATABASE_FORMAT_VERSION};
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

fn application_id(conn: &Connection) -> i32 {
    conn.query_row("PRAGMA application_id", [], |row| row.get(0))
        .unwrap()
}

fn object_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = ?1)",
        [name],
        |row| row.get(0),
    )
    .unwrap()
}

/// Every object the file holds, including SQLite's own (`sqlite_master` rows
/// for an FTS5 table include its shadow tables).
fn object_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM sqlite_master", [], |row| row.get(0))
        .unwrap()
}

fn journal_mode(conn: &Connection) -> String {
    conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap()
}

/// `Db` has no `Debug`, and a failure should print the message the user sees.
fn open_error(path: &Path) -> String {
    match Db::open(path) {
        Ok(_) => panic!("{} must be refused", path.display()),
        Err(e) => e.to_string(),
    }
}

/// A brand-new database carries the current format identity, the current
/// generation and the current structure — not an older shape that startup then
/// has to reconcile.
#[test]
fn fresh_database_uses_current_format_generation() {
    let path = db_path("current");
    let db = Db::open(path.path()).unwrap();
    assert_eq!(application_id(&db.read()), DATABASE_APPLICATION_ID);
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
        assert!(
            !object_exists(&db.read(), table),
            "retired table remains: {table}"
        );
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
        assert!(
            object_exists(&conn, "idx_sessions_owner_workstream"),
            "idx_sessions_owner_workstream is missing"
        );
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

/// Reopening a current-format database reads the format marker and checks the
/// structure: the data survives and startup runs no DDL at all — the object set
/// is exactly the one creation left behind.
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
    let objects = object_count(&db.read());
    drop(db);

    let reopened = Db::open(path.path()).unwrap();
    assert_eq!(application_id(&reopened.read()), DATABASE_APPLICATION_ID);
    assert_eq!(user_version(&reopened.read()), DATABASE_FORMAT_VERSION);
    assert_eq!(
        object_count(&reopened.read()),
        objects,
        "startup created or dropped an object on an existing database"
    );
    assert_eq!(
        reopened
            .read()
            .query_row(
                "SELECT value FROM settings WHERE key = 'schema-test'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "persists"
    );
}

/// The format gate runs before any persistent PRAGMA, so a database we refuse
/// keeps the journal mode it had: `journal_mode = WAL` rewrites the file header
/// and must never reach a file that is not ours.
#[test]
fn a_refused_database_is_never_switched_to_wal() {
    let foreign = db_path("foreign-journal");
    let conn = Connection::open(foreign.path()).unwrap();
    conn.execute("CREATE TABLE foo (a TEXT)", []).unwrap();
    let before = journal_mode(&conn);
    drop(conn);

    assert!(Db::open(foreign.path()).is_err());

    let conn = Connection::open(foreign.path()).unwrap();
    assert_ne!(before, "wal", "the fixture must not start in WAL");
    assert_eq!(
        journal_mode(&conn),
        before,
        "a refused file must keep its journal mode"
    );

    // The same holds for our own identity in a generation this build refuses.
    let old = db_path("old-generation-journal");
    let db = Db::open(old.path()).unwrap();
    let objects = object_count(&db.read());
    db.write()
        .pragma_update(None, "user_version", DATABASE_FORMAT_VERSION + 1)
        .unwrap();
    let wal = journal_mode(&db.read());
    drop(db);

    assert!(Db::open(old.path()).is_err());
    let conn = Connection::open(old.path()).unwrap();
    assert_eq!(journal_mode(&conn), wal);
    assert_eq!(
        object_count(&conn),
        objects,
        "a refused generation must not have objects added to it"
    );
}

/// "No repair" must never mean "no validation": a database carrying this
/// build's identity but missing part of its structure is refused rather than
/// quietly completed, and the missing object is not recreated.
#[test]
fn incomplete_current_format_database_is_refused_without_repair() {
    for (tag, damage, missing) in [
        (
            "missing-table",
            vec!["DROP TABLE context_deliveries"],
            "context_deliveries",
        ),
        (
            "missing-index",
            vec!["DROP INDEX idx_sessions_owner_workstream"],
            "idx_sessions_owner_workstream",
        ),
        (
            "missing-fts",
            vec!["DROP TABLE search_index"],
            "search_index",
        ),
        // A same-named view is not the table the format declares: the check
        // matches on `sqlite_master.type` too.
        (
            "view-impersonator",
            vec![
                "DROP TABLE context_deliveries",
                "CREATE VIEW context_deliveries AS SELECT 1 AS id",
            ],
            "context_deliveries",
        ),
    ] {
        let path = db_path(tag);
        let db = Db::open(path.path()).unwrap();
        for sql in damage {
            db.write().execute(sql, []).unwrap();
        }
        drop(db);

        let err = open_error(path.path());
        assert!(
            err.contains(missing),
            "refusal must name the missing object, got: {err}"
        );

        // Startup neither completed the schema nor rewrote what was there.
        let now_a_table: bool = Connection::open(path.path())
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = ?1 AND type = 'table')",
                [missing],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            !now_a_table,
            "startup repaired {missing} instead of refusing the database"
        );
    }
}

/// A foreign SQLite file is not an empty database: it has objects of its own, so
/// it is refused and left exactly as it was found.
#[test]
fn foreign_sqlite_file_is_refused_and_left_untouched() {
    let path = db_path("foreign");
    let conn = Connection::open(path.path()).unwrap();
    conn.execute("CREATE TABLE foo (a TEXT)", []).unwrap();
    drop(conn);

    let err = open_error(path.path());
    assert!(
        err.contains("application_id=0"),
        "refusal must report the identity found, got: {err}"
    );

    let conn = Connection::open(path.path()).unwrap();
    assert!(object_exists(&conn, "foo"), "the foreign file was modified");
    assert!(
        !object_exists(&conn, "projects"),
        "NoEnding schema was created inside a foreign file"
    );
    assert_eq!(application_id(&conn), 0);
    assert_eq!(user_version(&conn), 0);
}

/// A generation marker without the identity that belongs with it is not a
/// NoEnding database either: the version number alone never proves the format.
#[test]
fn version_marker_without_the_database_identity_is_refused() {
    let path = db_path("no-identity");
    let conn = Connection::open(path.path()).unwrap();
    conn.execute("CREATE TABLE projects (id TEXT PRIMARY KEY)", [])
        .unwrap();
    conn.pragma_update(None, "user_version", DATABASE_FORMAT_VERSION)
        .unwrap();
    drop(conn);

    let err = open_error(path.path());
    assert!(
        err.contains(&format!("required_format={DATABASE_FORMAT_VERSION}")),
        "refusal must name the required format, got: {err}"
    );
}

/// Another NoEnding generation is refused, never upgraded. The error names both
/// sides so the log says exactly what was found.
#[test]
fn other_generation_of_this_database_is_refused() {
    let unsupported = DATABASE_FORMAT_VERSION + 1;
    let path = db_path("unsupported");
    let db = Db::open(path.path()).unwrap();
    assert_eq!(application_id(&db.read()), DATABASE_APPLICATION_ID);
    db.write()
        .pragma_update(None, "user_version", unsupported)
        .unwrap();
    drop(db);

    let err = open_error(path.path());
    assert!(
        err.contains(&format!("database_format={unsupported}"))
            && err.contains(&format!("required_format={DATABASE_FORMAT_VERSION}")),
        "refusal must name the found and required formats, got: {err}"
    );
}

/// The environment-dependent defaults are reconciled on EVERY start, not only
/// when the database is created: a relocated `CODEX_HOME` or a newly supported
/// Agent must not need a format change. Reconciliation only inserts what is
/// missing.
#[test]
fn runtime_defaults_are_reconciled_on_every_start() {
    let path = db_path("defaults");
    let db = Db::open(path.path()).unwrap();
    let seeded_sources: i64 = db
        .read()
        .query_row("SELECT COUNT(*) FROM ingest_sources", [], |row| row.get(0))
        .unwrap();
    // A source the user added: reconciliation must never touch it.
    db.write()
        .execute(
            "INSERT INTO ingest_sources (id, agent, path, enabled, origin, created_at)
             VALUES ('user-src', 'codex', '/mine', 1, 'user', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    db.write()
        .execute("DELETE FROM ingest_sources WHERE origin = 'default'", [])
        .unwrap();
    db.delete_setting(noending::settings::CONTEXT_DELIVERY_LEVEL_KEY)
        .unwrap();
    drop(db);

    let reopened = Db::open(path.path()).unwrap();
    assert_eq!(
        reopened
            .read()
            .query_row(
                "SELECT COUNT(*) FROM ingest_sources WHERE origin = 'default' AND enabled = 0",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        seeded_sources,
        "default ingest sources must be reconciled on every start, still disabled"
    );
    let (enabled, origin): (i64, String) = reopened
        .read()
        .query_row(
            "SELECT enabled, origin FROM ingest_sources WHERE id = 'user-src'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((enabled, origin.as_str()), (1, "user"));
    assert_eq!(
        reopened
            .get_setting(noending::settings::CONTEXT_DELIVERY_LEVEL_KEY)
            .unwrap()
            .as_deref(),
        Some("off")
    );
}

/// Identity and structure are written by ONE transaction, so a database that
/// carries NoEnding's identity always has the schema that identity promises.
#[test]
fn creation_stamps_identity_with_the_schema() {
    let path = db_path("stamp");
    let db = Db::open(path.path()).unwrap();
    assert_eq!(application_id(&db.read()), DATABASE_APPLICATION_ID);
    drop(db);

    let conn = Connection::open(path.path()).unwrap();
    assert!(object_exists(&conn, "sessions"));
    assert_eq!(user_version(&conn), DATABASE_FORMAT_VERSION);
}
