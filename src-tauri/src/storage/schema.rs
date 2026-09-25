//! The ONE database format this build understands.
//!
//! NoEnding supports exactly one SQLite shape at a time:
//!
//! ```text
//! empty file                     → create the current format
//! current format, intact         → use as it is, never repair it
//! anything else, or incomplete   → refuse to open
//! ```
//!
//! A database is recognised by *identity*, not by guessing: creation stamps the
//! SQLite header with [`DATABASE_APPLICATION_ID`] plus
//! [`DATABASE_FORMAT_VERSION`], and opening demands that pair. A foreign SQLite
//! file, an older NoEnding generation and a file with no identity at all are
//! therefore all refused, and so is a current-format database whose structure
//! is missing a required table or index — "no repair" never means "no
//! validation", because half a schema is exactly what no repair pass could be
//! trusted to complete.
//!
//! There is no migration chain, no `ALTER TABLE` step and no old-row mapping: a
//! breaking change edits [`CURRENT_SCHEMA`] and bumps
//! [`DATABASE_FORMAT_VERSION`], and the local database is rebuilt from the
//! Agent sources plus whatever the user re-creates. The database is a
//! projection over that data, not the only copy of it — except for the
//! Workstream/Context state NoEnding itself owns, which a rebuild does NOT
//! restore.
//!
//! Responsibility split:
//!
//! * [`CURRENT_SCHEMA`] — the complete current structure (tables, indexes).
//! * [`open_or_create`] — recognise, validate, or create. Nothing else.
//! * [`reconcile_runtime_defaults`] — environment-dependent defaults, on every
//!   start. Not schema, not a migration.

use rusqlite::{params, Connection};

use crate::error::{other, Result};

/// Identity written into the SQLite header, so a NoEnding database can be told
/// apart from any other SQLite file. `0x4E6F456E` spells "NoEn".
pub const DATABASE_APPLICATION_ID: i32 = 0x4E6F_456E;

/// Exact SQLite format generation required by this build.
///
/// No database migrations or backwards compatibility are supported. A database
/// whose format generation differs from this value must be discarded and
/// rebuilt from Agent source data.
pub const DATABASE_FORMAT_VERSION: i64 = 1;

/// Open an existing current-format database, or create one.
///
/// The only accepted starting points are a completely empty file and an intact
/// database carrying this build's identity; everything else is refused with a
/// message that says so. A current-format database is returned untouched — the
/// startup path deliberately performs no schema repair, because SQLite schema
/// is not self-healing and a half-created database must never look usable.
pub fn open_or_create(conn: &Connection) -> Result<()> {
    let application_id: i32 = conn.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;

    // A completely empty file: no identity, no marker, no user object. Anything
    // else in this branch (a foreign SQLite file, a file that only carries a
    // version marker) fails the `empty` test and is refused below.
    if application_id == 0 && version == 0 && !has_user_objects(conn)? {
        let tx = conn.unchecked_transaction()?;
        create(&tx)?;
        tx.pragma_update(None, "application_id", DATABASE_APPLICATION_ID)?;
        tx.pragma_update(None, "user_version", DATABASE_FORMAT_VERSION)?;
        tx.commit()?;
        return Ok(());
    }

    // This build's own database, in the generation this build expects.
    if application_id == DATABASE_APPLICATION_ID && version == DATABASE_FORMAT_VERSION {
        return validate(conn);
    }

    Err(format_mismatch(application_id, version))
}

/// Fill in the defaults that depend on the *current* environment. Runs on every
/// start, so a newly supported Agent, or a relocated `CODEX_HOME`, needs no
/// format change to be picked up.
///
/// This is reconciliation, not migration: it inserts missing rows only, creates
/// no schema and never overwrites the user's own decision about a source (a
/// default source always starts DISABLED).
pub fn reconcile_runtime_defaults(conn: &Connection) -> Result<()> {
    for agent in crate::domain::Agent::all() {
        if let Some(root) = crate::platform::paths::resolve_agent_data_dir(*agent) {
            conn.execute(
                "INSERT OR IGNORE INTO ingest_sources (id, agent, path, enabled, origin, created_at)
                 VALUES (?1, ?2, ?3, 0, 'default', ?4)",
                params![
                    crate::storage::new_id(),
                    agent.as_str(),
                    root.to_string_lossy().to_string(),
                    crate::storage::now()
                ],
            )?;
        }
    }

    conn.execute(
        "INSERT OR IGNORE INTO settings (key, value) VALUES (?1, 'off')",
        params![crate::settings::CONTEXT_DELIVERY_LEVEL_KEY],
    )?;
    Ok(())
}

/// Does the file hold any object of its own? `sqlite_%` covers the internal
/// tables SQLite maintains (`sqlite_sequence`, `sqlite_stat1`, …); object names
/// with that prefix cannot be created by users.
fn has_user_objects(conn: &Connection) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name NOT LIKE 'sqlite_%')",
        [],
        |r| r.get(0),
    )?)
}

/// A database with this build's identity must also have this build's structure.
/// Every object [`CURRENT_SCHEMA`] declares has to be there; a missing one is
/// reported, never recreated.
fn validate(conn: &Connection) -> Result<()> {
    for name in required_objects() {
        let present: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = ?1)",
            params![name],
            |r| r.get(0),
        )?;
        if !present {
            return Err(other(format!(
                "数据库结构与当前格式不符（缺少 {name}）。当前版本不支持修复，\
                 请删除数据库后重新启动，Session 会从已配置的 Agent 数据源重新摄入。"
            )));
        }
    }
    Ok(())
}

/// Names of the objects the current format is made of, read off
/// [`CURRENT_SCHEMA`] so a new table or index cannot be forgotten here. The
/// optional FTS table is excluded on purpose: a build without FTS5 creates a
/// usable database without it.
fn required_objects() -> Vec<&'static str> {
    let mut names = Vec::new();
    for marker in [
        "CREATE TABLE IF NOT EXISTS ",
        "CREATE UNIQUE INDEX IF NOT EXISTS ",
        "CREATE INDEX IF NOT EXISTS ",
    ] {
        for tail in CURRENT_SCHEMA.split(marker).skip(1) {
            if let Some(name) = tail.split(|c: char| c.is_whitespace() || c == '(').next() {
                names.push(name);
            }
        }
    }
    names
}

fn format_mismatch(application_id: i32, version: i64) -> crate::error::AppError {
    other(format!(
        "数据库格式与此版本 NoEnding 不兼容（application_id={application_id}，\
         database_format={version}；本版本要求 application_id={DATABASE_APPLICATION_ID}，\
         required_format={DATABASE_FORMAT_VERSION}）。当前版本不支持数据库升级，\
         请删除数据库后重新启动，Session 会从已配置的 Agent 数据源重新摄入。"
    ))
}

/// The complete current structure: every table and index this build reads or
/// writes. Verbatim source of truth — nothing else in the codebase creates
/// schema.
///
/// The physical workspace: paths are first-class domain state, and a path-backed
/// Project is what every Session's cwd chain derives from. `workspace_paths.id`
/// is NOT a random uuid — it is derived from the canonical path by
/// `workspace::path_identity`, which is what makes `ensure_workspace_path`
/// idempotent: observing the same directory twice cannot create two rows.
///
/// `session_deletion_jobs` is transient coordination for prepared permanent
/// deletions. It spans SQLite + filesystem, which no single transaction can
/// cover, so the plan is frozen there first and revalidated at execute time.
/// This is NOT deletion history / tombstone / blacklist: the row is deleted in
/// the same transaction that purges the Session, so a completed permanent
/// deletion leaves nothing behind.
const CURRENT_SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS projects (
      id TEXT PRIMARY KEY,
      name TEXT NOT NULL,
      description TEXT NOT NULL DEFAULT '',
      git_id TEXT,
      name_customized INTEGER NOT NULL DEFAULT 0,
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS workstreams (
      id TEXT PRIMARY KEY,
      title TEXT NOT NULL,
      description TEXT NOT NULL DEFAULT '',
      lifecycle TEXT NOT NULL DEFAULT 'active',
      visibility TEXT NOT NULL DEFAULT 'normal',
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS sessions (
      id TEXT PRIMARY KEY,
      agent TEXT NOT NULL,
      agent_session_id TEXT NOT NULL,
      title TEXT,
      cwd TEXT,
      workspace_path_id TEXT,
      project_id TEXT REFERENCES projects(id),
      -- Semantic ownership: at most one Owner Workstream per Session
      -- (方案 §3.3). Deleting the Workstream clears this, never the row.
      owner_workstream_id TEXT
        REFERENCES workstreams(id) ON DELETE SET NULL,
      raw_path TEXT NOT NULL,
      parent_agent_session_id TEXT,
      started_at TEXT,
      last_activity_at TEXT,
      -- lifecycle: NULL = Normal, NOT NULL = Trash (RFC3339).
      trashed_at TEXT,
      UNIQUE(agent, agent_session_id)
    );
    CREATE TABLE IF NOT EXISTS session_events (
      id TEXT PRIMARY KEY,
      session_id TEXT NOT NULL REFERENCES sessions(id),
      sequence INTEGER NOT NULL,
      source_event_id TEXT,
      source_generation INTEGER NOT NULL DEFAULT 0,
      source_position TEXT NOT NULL DEFAULT '',
      source_identity_hash TEXT NOT NULL,
      ts TEXT,
      kind TEXT NOT NULL,
      text TEXT,
      raw_ref TEXT NOT NULL,
      metadata TEXT NOT NULL DEFAULT '{}',
      UNIQUE (session_id, source_identity_hash)
    );
    CREATE TABLE IF NOT EXISTS session_cursors (
      session_id TEXT PRIMARY KEY REFERENCES sessions(id),
      last_sequence INTEGER NOT NULL DEFAULT 0,
      last_seen_size INTEGER NOT NULL DEFAULT 0,
      source_file_identity TEXT NOT NULL DEFAULT '',
      generation INTEGER NOT NULL DEFAULT 0,
      byte_offset INTEGER NOT NULL DEFAULT 0,
      prefix_hash TEXT NOT NULL DEFAULT '',
      identity_tail_hash TEXT NOT NULL DEFAULT '',
      mtime REAL,
      processed_sequence INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE IF NOT EXISTS context_items (
      id TEXT PRIMARY KEY,
      workstream_id TEXT NOT NULL REFERENCES workstreams(id),
      kind TEXT NOT NULL,
      status TEXT NOT NULL DEFAULT 'active',
      authority TEXT NOT NULL DEFAULT 'system_observed',
      created_by TEXT NOT NULL DEFAULT 'unknown',
      current_revision_id TEXT,
      supersedes_item_id TEXT,
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS context_item_revisions (
      id TEXT PRIMARY KEY,
      item_id TEXT NOT NULL REFERENCES context_items(id),
      title TEXT NOT NULL,
      content TEXT NOT NULL DEFAULT '',
      metadata TEXT NOT NULL DEFAULT '{}',
      source_type TEXT,
      source_ref TEXT,
      sync_run_id TEXT,
      created_at TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS sync_runs (
      id TEXT PRIMARY KEY,
      session_id TEXT NOT NULL,
      from_sequence INTEGER NOT NULL,
      to_sequence INTEGER NOT NULL,
      status TEXT NOT NULL,
      mutations TEXT NOT NULL DEFAULT '[]',
      summary TEXT NOT NULL DEFAULT '',
      error TEXT,
      created_at TEXT NOT NULL,
      runtime TEXT NOT NULL DEFAULT 'heuristic',
      delta_fingerprint TEXT,
      source_generation INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE IF NOT EXISTS agent_installations (
      agent TEXT PRIMARY KEY,
      executable_path TEXT,
      version TEXT,
      source TEXT,
      last_verified_at TEXT
    );
    CREATE TABLE IF NOT EXISTS settings (
      key TEXT PRIMARY KEY,
      value TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS launch_intents (
      id TEXT PRIMARY KEY,
      launch_type TEXT NOT NULL DEFAULT 'new',
      agent TEXT NOT NULL,
      owner_workstream_id TEXT REFERENCES workstreams(id) ON DELETE SET NULL,
      cwd TEXT,
      context_bundle_markdown TEXT,
      context_bundle_revisions TEXT,
      process_id INTEGER,
      launched_at TEXT NOT NULL,
      matched_session_id TEXT,
      status TEXT NOT NULL DEFAULT 'pending',
      note TEXT NOT NULL DEFAULT '',
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS context_conflicts (
      id TEXT PRIMARY KEY,
      workstream_id TEXT NOT NULL REFERENCES workstreams(id),
      left_item_id TEXT NOT NULL REFERENCES context_items(id),
      right_item_id TEXT,
      conflict_type TEXT NOT NULL DEFAULT 'authority',
      status TEXT NOT NULL DEFAULT 'open',
      resolution TEXT,
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL,
      left_revision_id TEXT,
      right_revision_id TEXT,
      candidate_snapshot_json TEXT
    );
    CREATE TABLE IF NOT EXISTS context_deliveries (
      id TEXT PRIMARY KEY,
      session_id TEXT NOT NULL REFERENCES sessions(id),
      workstream_id TEXT NOT NULL REFERENCES workstreams(id),
      bundle_id TEXT NOT NULL,
      delivered_revisions TEXT NOT NULL DEFAULT '[]',
      delivered_conflicts TEXT NOT NULL DEFAULT '[]',
      delivered_at TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS ingest_sources (
      id TEXT PRIMARY KEY,
      agent TEXT NOT NULL,
      path TEXT NOT NULL,
      enabled INTEGER NOT NULL DEFAULT 0,
      origin TEXT NOT NULL DEFAULT 'user',   -- default | user
      created_at TEXT NOT NULL,
      UNIQUE(agent, path)
    );
    CREATE INDEX IF NOT EXISTS idx_sessions_project ON sessions(project_id);
    CREATE INDEX IF NOT EXISTS idx_items_workstream ON context_items(workstream_id);
    CREATE INDEX IF NOT EXISTS idx_sessions_owner_workstream ON sessions(owner_workstream_id);
    CREATE INDEX IF NOT EXISTS idx_events_session ON session_events(session_id, sequence);
    CREATE INDEX IF NOT EXISTS idx_intents_status ON launch_intents(status);
    CREATE UNIQUE INDEX IF NOT EXISTS idx_sync_runs_fingerprint
      ON sync_runs(session_id, delta_fingerprint)
      WHERE status = 'ok' AND delta_fingerprint IS NOT NULL;

    CREATE TABLE IF NOT EXISTS assistant_sessions (
      id TEXT PRIMARY KEY,
      title TEXT NOT NULL DEFAULT 'Assistant',
      created_at TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS assistant_messages (
      id TEXT PRIMARY KEY,
      session_id TEXT NOT NULL REFERENCES assistant_sessions(id),
      role TEXT NOT NULL,              -- user | assistant
      content TEXT NOT NULL DEFAULT '',
      action_json TEXT,                -- proposed action (needs user confirm)
      runtime TEXT,
      created_at TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS context_conflict_events (
      id TEXT PRIMARY KEY,
      conflict_id TEXT NOT NULL REFERENCES context_conflicts(id),
      previous_status TEXT NOT NULL,
      new_status TEXT NOT NULL,
      resolution TEXT,
      actor TEXT NOT NULL,
      created_at TEXT NOT NULL,
      snapshot_json TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_conflict_events_conflict ON context_conflict_events(conflict_id);
    CREATE TABLE IF NOT EXISTS workstream_review_state (
      workstream_id TEXT PRIMARY KEY REFERENCES workstreams(id) ON DELETE CASCADE,
      reviewed_through_at TEXT NOT NULL,
      reviewed_boundary_change_ids TEXT NOT NULL DEFAULT '[]',
      reviewed_at TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS git_identities (
      id TEXT PRIMARY KEY,
      common_dir TEXT NOT NULL UNIQUE,
      first_seen_at TEXT NOT NULL,
      last_seen_at TEXT NOT NULL,
      metadata TEXT NOT NULL DEFAULT '{}'
    );
    CREATE TABLE IF NOT EXISTS workspace_paths (
      id TEXT PRIMARY KEY,
      canonical_path TEXT NOT NULL UNIQUE,
      project_id TEXT NOT NULL REFERENCES projects(id),
      git_state TEXT NOT NULL DEFAULT 'none',
      git_kind TEXT,
      -- Named `exists_on_disk`, not `exists`: EXISTS is a SQLite keyword and
      -- `exists INTEGER NOT NULL` is a syntax error, so the spec's column name
      -- would need quoting in every statement (方案 §42.2-E13). The domain
      -- field stays `exists`.
      exists_on_disk INTEGER NOT NULL DEFAULT 1,
      first_seen_at TEXT NOT NULL,
      last_seen_at TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS workstream_paths (
      id TEXT PRIMARY KEY,
      workstream_id TEXT NOT NULL REFERENCES workstreams(id),
      workspace_path_id TEXT NOT NULL REFERENCES workspace_paths(id),
      position INTEGER NOT NULL,
      created_at TEXT NOT NULL,
      UNIQUE(workstream_id, workspace_path_id),
      UNIQUE(workstream_id, position)
    );
    CREATE INDEX IF NOT EXISTS idx_workspace_paths_project ON workspace_paths(project_id);
    CREATE INDEX IF NOT EXISTS idx_workspace_paths_canonical ON workspace_paths(canonical_path);
    CREATE INDEX IF NOT EXISTS idx_workstream_paths_ws ON workstream_paths(workstream_id, position);
    CREATE INDEX IF NOT EXISTS idx_workstream_paths_path ON workstream_paths(workspace_path_id);

    CREATE TABLE IF NOT EXISTS session_deletion_jobs (
      id TEXT PRIMARY KEY,
      session_id TEXT NOT NULL UNIQUE REFERENCES sessions(id),
      state TEXT NOT NULL,          -- prepared | deleting_source | failed | stale
      plan_json TEXT NOT NULL,
      last_error TEXT,
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL
    );

    CREATE UNIQUE INDEX IF NOT EXISTS idx_projects_git_id
      ON projects(git_id)
      WHERE git_id IS NOT NULL;
"#;

/// The FTS5 search index (external-content style: we manage rows manually).
/// Kept out of [`CURRENT_SCHEMA`] because it is the one optional part: a bundled
/// SQLite without FTS5 still creates a usable database, and search then falls
/// back to LIKE at query time.
const OPTIONAL_FTS_SCHEMA: &str = r#"
    CREATE VIRTUAL TABLE IF NOT EXISTS search_index USING fts5(
      kind, ref_id, parent_id, title, body, tokenize = 'unicode61'
    );
"#;

/// Create the current format. Only ever called for a completely empty file, so
/// every `IF NOT EXISTS` here is a fresh creation.
fn create(conn: &Connection) -> Result<()> {
    conn.execute_batch(CURRENT_SCHEMA)?;
    if conn.execute_batch(OPTIONAL_FTS_SCHEMA).is_err() {
        eprintln!("[storage] FTS5 unavailable, search will use LIKE fallback");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The validation list is read off the DDL, so this pins the reader itself:
    /// every object of the current format must come out of it, and the optional
    /// FTS table must not be demanded.
    #[test]
    fn required_objects_covers_the_whole_ddl() {
        let names: HashSet<&str> = required_objects().into_iter().collect();
        for expected in [
            "projects",
            "workstreams",
            "sessions",
            "session_events",
            "session_cursors",
            "context_items",
            "context_item_revisions",
            "sync_runs",
            "agent_installations",
            "settings",
            "launch_intents",
            "context_conflicts",
            "context_deliveries",
            "ingest_sources",
            "assistant_sessions",
            "assistant_messages",
            "context_conflict_events",
            "workstream_review_state",
            "git_identities",
            "workspace_paths",
            "workstream_paths",
            "session_deletion_jobs",
            "idx_sessions_project",
            "idx_items_workstream",
            "idx_sessions_owner_workstream",
            "idx_events_session",
            "idx_intents_status",
            "idx_sync_runs_fingerprint",
            "idx_conflict_events_conflict",
            "idx_workspace_paths_project",
            "idx_workspace_paths_canonical",
            "idx_workstream_paths_ws",
            "idx_workstream_paths_path",
            "idx_projects_git_id",
        ] {
            assert!(
                names.contains(expected),
                "required_objects() missed {expected}"
            );
        }
        assert_eq!(names.len(), 34, "required_objects() found a name twice");
        assert!(!names.contains("search_index"));
    }
}
