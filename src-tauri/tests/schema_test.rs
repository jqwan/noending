//! Database format contract (Logical Session refactor).
//!
//! Exactly one SQLite format generation is supported at a time, identified by the
//! header pair (`application_id`, `user_version`) that creation stamps. An empty
//! file is created in that format; a database carrying the pair must also have
//! the full structure; anything else — a foreign SQLite file, an older
//! generation, or an incomplete current-format database — is refused instead of
//! migrated or repaired.
//!
//! The current format keys `sessions` by the root source's Resume identity and
//! carries the root source, its cursor and the fact frontier as SESSION columns
//! (the former session_members / session_member_cursors /
//! session_ingest_state / ingestion_diagnostics rows). The RAW conversation is
//! `session_messages` (append-only for provenance), the CURRENT conversation is
//! `session_message_projection`, the Session summary frontier is
//! `session_contexts` / `session_context_revisions`, and the Workstream
//! revision pair is `workstream_context_state` / `workstream_session_frontiers`.
//! The retired objects must stay absent.
//!

use noending::domain::{Agent, SessionMessageRole};
use noending::domain::{ParsedSessionMessage, SourceCursorUpdate};
use noending::storage::{new_id, Db, DATABASE_APPLICATION_ID, DATABASE_FORMAT_VERSION};
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

    let conn = db.read();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('workstreams') WHERE name = 'lifecycle'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "tasks have only archive state");
    drop(conn);

    // the old-generation tables are gone, not carried
    // forward: one conversation store, one session-owned cursor, and no
    // deletion job machinery (NoEnding never deletes an Agent-owned source).
    for table in [
        "session_events",
        "session_cursors",
        "session_deletion_jobs",
        "project_resources",
        "project_affinity_evidence",
        "session_workstream_bindings",
        "session_binding_removals",
        // 统计功能退役（2026-09-29）：连用量快照与事件账本一起退场。
        "session_member_stats",
        "usage_events",
        "ingest_usage_claims",
        "session_member_usage",
        // 会话扁平化（2026-10-05）：成员图四表并入 sessions 的列。
        "session_members",
        "session_member_cursors",
        "session_ingest_state",
        "ingestion_diagnostics",
    ] {
        assert!(
            !object_exists(&db.read(), table),
            "retired table remains: {table}"
        );
    }
    for (table, column) in [
        ("sessions", "agent_session_id"),
        ("sessions", "raw_path"),
        ("sessions", "project_id"),
        ("projects", "description"),
        ("session_messages", "raw_ref"),
        ("launch_intents", "launch_type"),
        ("launch_intents", "process_id"),
        ("sessions", "parent_agent_session_id"),
        ("projects", "archived"),
        ("workstreams", "project_id"),
        ("workstreams", "default_cwd"),
        ("workstream_paths", "source"),
        ("launch_intents", "selected_workstream_ids"),
        ("launch_intents", "context_bundle_markdown"),
        ("launch_intents", "context_bundle_revisions"),
    ] {
        assert!(
            !has_column(&db.read(), table, column),
            "retired column remains: {table}.{column}"
        );
    }

    for index in [
        "idx_session_messages_session",
        "idx_workspace_paths_canonical",
        "idx_workstream_paths_ws",
        "idx_sessions_project",
    ] {
        assert!(
            !object_exists(&db.read(), index),
            "redundant/cache index remains: {index}"
        );
    }

    // The Logical Session model is the replacement: every table of the new
    // shape exists, and the session row carries the flattened source, cursor
    // and fact-frontier columns.
    for object in [
        "session_messages",
        "session_message_projection",
        "session_contexts",
        "session_context_revisions",
        "workstream_context_state",
        "workstream_session_frontiers",
        "idx_sessions_workspace_path",
        "idx_item_revisions_item",
        "idx_conflicts_workstream",
        "idx_projection_message",
    ] {
        assert!(
            object_exists(&db.read(), object),
            "Logical Session object is missing: {object}"
        );
    }
    for column in [
        "source_kind",
        "source_path",
        "metadata",
        "source_file_identity",
        "source_generation",
        "source_byte_offset",
        "source_last_seen_size",
        "source_mtime",
        "source_prefix_hash",
        "source_tail_hash",
        "fact_generation",
        "latest_message_seq",
    ] {
        assert!(
            has_column(&db.read(), "sessions", column),
            "flattened session column is missing: sessions.{column}"
        );
    }

    // A Session is keyed by (agent, root_agent_session_id) — the root source's
    // real Resume identity — and carries fork provenance only as a self-FK.
    {
        let conn = db.read();
        let on_delete: String = conn
            .query_row(
                "SELECT \"table\" || '|' || on_delete FROM pragma_foreign_key_list('sessions')
                  WHERE \"from\" = 'forked_from_session_id'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert_eq!(
            on_delete, "sessions|SET NULL",
            "fork provenance must be sessions(id) ON DELETE SET NULL"
        );
        assert!(
            has_column(&conn, "sessions", "archived_at"),
            "sessions.archived_at is the single lifecycle authority"
        );
        assert!(
            has_column(&conn, "sessions", "owner_workstream_id"),
            "sessions.owner_workstream_id is missing"
        );
        // A pending LaunchIntent's chosen Owner follows the same rule:
        // deleting the Workstream nulls it instead of dangling.
        let intent_owner_fk: String = conn
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

/// The storage-layer round trip over the same invariants: a session ingest
/// commits messages with NoEnding's own sequence and dedups re-scans by
/// identity.
#[test]
fn storage_round_trip_matches_the_schema_promises() {
    let path = db_path("round-trip");
    let db = Db::open(path.path()).unwrap();

    let (s_id, _) = db
        .upsert_logical_session_unchecked(
            Agent::Codex,
            "root-1",
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
    let messages = [
        ParsedSessionMessage {
            source_message_id: Some("m1".into()),
            source_position: "0".into(),
            ts: None,
            role: SessionMessageRole::User,
            content: "first".into(),
        },
        ParsedSessionMessage {
            source_message_id: Some("m2".into()),
            source_position: "1".into(),
            ts: None,
            role: SessionMessageRole::Assistant,
            content: "second".into(),
        },
    ];
    let source = SourceCursorUpdate {
        file_identity: "identity".into(),
        generation: 1,
        byte_offset: 100,
        last_seen_size: 100,
        mtime: None,
        start_byte_offset: 0,
        prefix_hash: String::new(),
    };
    let stored = db.commit_ingest(&s_id, &messages, &source).unwrap();
    assert_eq!(stored.len(), 2);
    assert_eq!(
        stored.iter().map(|m| m.sequence).collect::<Vec<_>>(),
        vec![1, 2],
        "sequence is NoEnding's own per-session counter starting at 1"
    );

    // A full re-scan of the same source dedups by identity.
    let replay = db.commit_ingest(&s_id, &messages, &source).unwrap();
    assert!(replay.is_empty(), "a re-scan must not duplicate messages");
    assert_eq!(db.message_count(&s_id).unwrap(), 2);
    assert_eq!(db.ingested_message_sequence(&s_id).unwrap(), 2);
    let _ = new_id();
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
}
