//! Database creation, identity checks, structural integrity and durable reopening.

use noending::storage::{Db, DATABASE_APPLICATION_ID, DATABASE_FORMAT_VERSION};
use noending_lib as noending;
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

#[test]
fn fresh_database_has_the_current_identity_and_valid_structure() {
    let path = db_path("current");
    let db = Db::open(path.path()).unwrap();
    assert_eq!(application_id(&db.read()), DATABASE_APPLICATION_ID);
    assert_eq!(user_version(&db.read()), DATABASE_FORMAT_VERSION);
    let integrity: String = db
        .read()
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(integrity, "ok");
    let sources = db.list_ingest_sources().unwrap();
    assert!(!sources.is_empty());
    assert!(sources.iter().all(|source| source.origin == "default"));
}

/// The integrity rules the Logical Session tables promise are actually
/// enforced by the schema itself, not only by the storage code: the role
/// CHECK, the sequence/identity uniqueness, the root-identity uniqueness, and
/// the cascade that keeps a purge atomic.
#[test]
fn logical_session_schema_enforces_its_invariants() {
    let path = db_path("invariants");
    let db = Db::open(path.path()).unwrap();

    let insert_session = |db: &Db, id: &str, root: &str| {
        db.write()
            .execute(
                "INSERT INTO sessions (id, agent, root_agent_session_id, source_kind, source_path)
                 VALUES (?1, 'codex', ?2, 'test', '/tmp/source')",
                rusqlite::params![id, root],
            )
            .unwrap();
    };

    insert_session(&db, "s1", "root-1");

    // CHECK + UNIQUE on the conversation store.
    let insert_message = |db: &Db, id: &str, sequence: i64, role: &str, hash: &str| {
        db.write().execute(
            "INSERT INTO session_messages
               (id, session_id, sequence, source_identity_hash, role, content)
             VALUES (?1, 's1', ?2, ?3, ?4, 'content')",
            rusqlite::params![id, sequence, hash, role],
        )
    };
    insert_message(&db, "msg-1", 1, "user", "hash-1").unwrap();
    let bad_role = insert_message(&db, "msg-role", 2, "system", "hash-role");
    assert!(
        bad_role.is_err(),
        "role CHECK must reject values outside user|assistant"
    );
    let dup_sequence = insert_message(&db, "msg-seq", 1, "assistant", "hash-seq");
    assert!(
        dup_sequence.is_err(),
        "UNIQUE(session_id, sequence) must refuse a reused sequence"
    );
    let dup_identity = insert_message(&db, "msg-hash", 3, "user", "hash-1");
    assert!(
        dup_identity.is_err(),
        "UNIQUE(session_id, source_identity_hash) must refuse a re-ingested message"
    );

    // UNIQUE(agent, root_agent_session_id) — one Logical Session per root
    // Resume identity; re-discovery updates in place instead of duplicating.
    let dup_session = db.write().execute(
        "INSERT INTO sessions (id, agent, root_agent_session_id, source_kind, source_path)
         VALUES ('s-dup', 'codex', 'root-1', 'test', '/tmp/source')",
        [],
    );
    assert!(
        dup_session.is_err(),
        "a (agent, root_agent_session_id) pair must be unique"
    );

    // CASCADE — deleting the session takes everything with it in one step:
    // messages, the projection, and the Context frontier.
    db.write()
        .execute(
            "INSERT INTO session_contexts (session_id, updated_at) VALUES ('s1', 't')",
            [],
        )
        .unwrap();

    db.write()
        .execute(
            "INSERT INTO session_message_projection VALUES ('s1', 1, 'msg-1', 1)",
            [],
        )
        .unwrap();
    db.write()
        .execute("DELETE FROM sessions WHERE id = 's1'", [])
        .unwrap();

    for (table, key_column, what) in [
        ("session_messages", "session_id", "messages"),
        ("session_message_projection", "session_id", "projection"),
        ("session_contexts", "session_id", "context frontier"),
    ] {
        let n: i64 = db
            .read()
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE {key_column} = 's1'"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "cascade delete must remove {what} ({table})");
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
    let newer = db_path("newer-version-journal");
    let db = Db::open(newer.path()).unwrap();
    let objects = object_count(&db.read());
    db.write()
        .pragma_update(None, "user_version", DATABASE_FORMAT_VERSION + 1)
        .unwrap();
    let wal = journal_mode(&db.read());
    drop(db);

    assert!(Db::open(newer.path()).is_err());
    let conn = Connection::open(newer.path()).unwrap();
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
            vec!["DROP TABLE session_contexts"],
            "session_contexts",
        ),
        (
            "missing-index",
            vec!["DROP INDEX idx_projection_message"],
            "idx_projection_message",
        ),
        (
            "missing-fts",
            vec!["DROP TABLE search_index"],
            "search_index",
        ),
        (
            "missing-search-routing-index",
            vec!["DROP INDEX idx_search_documents_parent"],
            "idx_search_documents_parent",
        ),
        (
            "missing-search-view",
            vec!["DROP VIEW search_content"],
            "search_content",
        ),
        (
            "missing-search-trigger",
            vec!["DROP TRIGGER search_documents_delete"],
            "search_documents_delete",
        ),
        // A same-named view is not the table the format declares: the check
        // matches on `sqlite_master.type` too.
        (
            "view-impersonator",
            vec![
                "DROP TABLE session_contexts",
                "CREATE VIEW session_contexts AS SELECT 1 AS id",
            ],
            "session_contexts",
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

/// The search index is the FTS5 virtual table the format declares: a plain
/// table with the same name and columns would satisfy a name+type check and
/// then fail on the first `MATCH`.
#[test]
fn a_plain_table_cannot_stand_in_for_the_fts_index() {
    let path = db_path("plain-index");
    let db = Db::open(path.path()).unwrap();
    db.write()
        .execute_batch(
            "DROP TABLE search_index;
             CREATE TABLE search_index (
               kind TEXT NOT NULL, ref_id TEXT NOT NULL, parent_id TEXT NOT NULL,
               title TEXT NOT NULL, body TEXT NOT NULL
             );",
        )
        .unwrap();
    drop(db);

    let err = open_error(path.path());
    assert!(
        err.to_lowercase().contains("fts5"),
        "refusal must name the FTS5 index, got: {err}"
    );
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

/// Invalid and newer version markers cannot bypass the supported-schema gate.
#[test]
fn unsupported_version_markers_preserve_the_database() {
    for unsupported in [0, DATABASE_FORMAT_VERSION + 1] {
        let path = db_path("unsupported");
        let db = Db::open(path.path()).unwrap();
        assert_eq!(application_id(&db.read()), DATABASE_APPLICATION_ID);
        db.write()
            .pragma_update(None, "user_version", unsupported)
            .unwrap();
        db.set_setting("preserved", "keep").unwrap();
        drop(db);
        let bytes = std::fs::read(path.path()).unwrap();
        let err = open_error(path.path());
        assert_eq!(std::fs::read(path.path()).unwrap(), bytes);
        assert!(
            err.contains(&format!("database_format={unsupported}"))
                && err.contains(&format!("required_format={DATABASE_FORMAT_VERSION}")),
            "refusal must name the found and required formats, got: {err}"
        );
    }
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
            "INSERT INTO ingest_sources (id, agent, path, origin)
             VALUES ('user-src', 'codex', '/mine', 'user')",
            [],
        )
        .unwrap();
    db.write()
        .execute("DELETE FROM ingest_sources WHERE origin = 'default'", [])
        .unwrap();
    drop(db);

    let reopened = Db::open(path.path()).unwrap();
    assert_eq!(
        reopened
            .read()
            .query_row(
                "SELECT COUNT(*) FROM ingest_sources WHERE origin = 'default'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        seeded_sources,
        "default ingest sources must be reconciled on every start"
    );
    let (path, origin): (String, String) = reopened
        .read()
        .query_row(
            "SELECT path, origin FROM ingest_sources WHERE id = 'user-src'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((path.as_str(), origin.as_str()), ("/mine", "user"));
}

fn version_one(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(include_str!("fixtures/schema-v1.sql"))
        .unwrap();
    conn.execute_batch(r#"
      INSERT INTO projects(id,name,created_at,updated_at) VALUES('p','Persistent project','2026-01-01','2026-01-01');
      INSERT INTO workspace_paths(id,canonical_path,project_id,git_state) VALUES('wp','/fixture','p','none');
      INSERT INTO workstreams(id,title,created_at,updated_at) VALUES('t','Persistent task','2026-01-01','2026-01-01');
      INSERT INTO sessions(id,agent,root_agent_session_id,workspace_path_id,source_kind,source_path,latest_message_seq) VALUES('s','codex','native-s','wp','jsonl','/fixture/s.jsonl',1);
      INSERT INTO session_workstreams VALUES('s','t');
      INSERT INTO session_messages(id,session_id,sequence,role,content,source_identity_hash) VALUES('m','s',1,'user','Persistent evidence','hash');
      INSERT INTO session_message_projection VALUES('s',1,'m',1);
      INSERT INTO session_contexts VALUES('s','{"items":[{"title":"Locked title","description":"Retained detail","details":[],"title_locked":true}]}',4,0,1,'2026-01-02');
      INSERT INTO workstream_context_state(workstream_id,context_revision,summary) VALUES('t',2,'{"items":[]}');
      INSERT INTO project_context_state(project_id,context_revision,summary) VALUES('p',2,'{"items":[]}');
      INSERT INTO workstream_session_frontiers VALUES('t','s',4);
      INSERT INTO project_session_frontiers VALUES('p','s',3);
      INSERT INTO settings VALUES('preserved','yes');
    "#).unwrap();
}

fn schema_shape(conn: &Connection) -> Vec<(String, String, Vec<(String, String, i64, i64)>)> {
    let objects: Vec<(String, String)> = conn
        .prepare(
            "SELECT type,name FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    objects
        .into_iter()
        .map(|(kind, name)| {
            let columns = conn
                .prepare("SELECT name,type,\"notnull\",pk FROM pragma_table_info(?1)")
                .unwrap()
                .query_map([&name], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            (kind, name, columns)
        })
        .collect()
}

#[test]
fn baseline_upgrade_preserves_data_and_reports_unknown_freshness_without_inventing_history() {
    let path = db_path("upgrade");
    version_one(path.path());
    let db = Db::open(path.path()).unwrap();
    assert_eq!(user_version(&db.read()), 2);
    assert_eq!(db.get_setting("preserved").unwrap().as_deref(), Some("yes"));
    let session = db.get_session("s").unwrap().unwrap();
    assert_eq!(session.project_id.as_deref(), Some("p"));
    assert_eq!(session.tracked_workstream_ids, vec!["t"]);
    let context = db.get_session_context("s").unwrap().unwrap();
    assert_eq!(context.revision, 4);
    assert!(context.fields.summary.items[0].title_locked);
    assert_eq!(
        context.fields.summary.items[0].description,
        "Retained detail"
    );
    assert_eq!(
        db.read()
            .query_row(
                "SELECT content FROM session_messages WHERE id='m'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "Persistent evidence"
    );
    let view = noending::context::freshness::view(&db, Some("session"), Some("s")).unwrap();
    assert!(view.session_check.unwrap().checked_at.is_none());
    let summary = view.summary.unwrap();
    assert!(summary.refreshed_at.is_none());
    assert_eq!(summary.pending_messages, Some(0));
    let task = noending::context::freshness::view(&db, Some("task"), Some("t"))
        .unwrap()
        .summary
        .unwrap();
    assert_eq!(task.pending_messages, Some(0));
    let project = noending::context::freshness::view(&db, Some("project"), Some("p"))
        .unwrap()
        .summary
        .unwrap();
    assert_eq!(project.pending_messages, None);
    assert_eq!(project.unknown_coverage_sessions, 1);
    let fresh = db_path("equivalence");
    let current = Db::open(fresh.path()).unwrap();
    assert_eq!(schema_shape(&db.read()), schema_shape(&current.read()));
    assert_eq!(
        db.read()
            .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert_eq!(
        db.read()
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .query_map([], |_| Ok(()))
            .unwrap()
            .count(),
        0
    );
    drop(db);
    assert_eq!(
        Db::open(path.path())
            .unwrap()
            .get_session_context("s")
            .unwrap()
            .unwrap()
            .revision,
        4
    );
}

#[test]
fn a_failed_upgrade_rolls_back_all_schema_changes_and_the_version() {
    let path = db_path("rollback");
    version_one(path.path());
    let conn = Connection::open(path.path()).unwrap();
    conn.execute("DROP INDEX idx_projection_message", [])
        .unwrap();
    let before = schema_shape(&conn);
    drop(conn);
    assert!(Db::open(path.path()).is_err());
    let conn = Connection::open(path.path()).unwrap();
    assert_eq!(user_version(&conn), 1);
    assert_eq!(schema_shape(&conn), before);
    assert_eq!(
        conn.query_row(
            "SELECT value FROM settings WHERE key='preserved'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "yes"
    );
}
