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
//! * [`CURRENT_SCHEMA`] — the complete current structure (tables, indexes and
//!   the FTS5 search index).
//! * [`open_or_create`] — recognise, validate, or create. Nothing else.
//! * [`reconcile_runtime_defaults`] — environment-dependent defaults, on every
//!   start. Not schema, not a migration.

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{other, Result};

/// Identity written into the SQLite header, so a NoEnding database can be told
/// apart from any other SQLite file. `0x4E6F456E` spells "NoEn".
pub const DATABASE_APPLICATION_ID: i32 = 0x4E6F_456E;

/// Exact SQLite format generation required by this build.
///
/// No database migrations or backwards compatibility are supported. A database
/// whose format generation differs from this value must be discarded and
/// rebuilt from Agent source data.
///
/// v2 — the Logical Session refactor (重构方案 §24): `sessions` is keyed by the
/// root member's Resume identity and owns lifecycle/Owner alone; execution
/// lives in `session_members`, conversation in `session_messages`, reading in
/// `session_member_cursors`, observation in `session_member_stats`, the Sync
/// frontier in `session_context_state`, and unattachable sources in
/// `ingestion_diagnostics`. `session_events`, `session_cursors` and
/// `session_deletion_jobs` are gone: there is one conversation store, cursors
/// belong to members, and NoEnding never deletes an Agent-owned source.
///
/// v3 — message-level model provenance (Provenance 方案 §5/§8/§9):
/// `session_messages` gains `provider` / `model` (source-confirmed, Assistant
/// only — enforced by CHECK), and `session_member_stats` loses its
/// `model` / `provider` / `effort` columns: member-level "current model" was
/// a second, semantically unclear authority over the same question.
pub const DATABASE_FORMAT_VERSION: i64 = 3;

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
/// Every object [`CURRENT_SCHEMA`] declares has to be there, of the right kind;
/// a missing one is reported, never recreated.
fn validate(conn: &Connection) -> Result<()> {
    for object in required_objects() {
        let present: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = ?1 AND type = ?2)",
            params![object.name, object.kind],
            |r| r.get(0),
        )?;
        if !present {
            return Err(structure_mismatch(format!(
                "缺少 {} {}",
                object.kind, object.name
            )));
        }
    }
    validate_search_index(conn)
}

/// `search_index` is the one object whose kind alone is not enough: SQLite
/// records a virtual table as `type = 'table'` too, so a plain table with the
/// same name and columns would pass the object check and only fail later, on
/// the first `MATCH`.
fn validate_search_index(conn: &Connection) -> Result<()> {
    let sql: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'search_index'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let normalized = sql
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if !normalized.contains("using fts5") {
        return Err(structure_mismatch("search_index 不是 FTS5 索引表".into()));
    }
    Ok(())
}

fn structure_mismatch(what: String) -> crate::error::AppError {
    other(format!(
        "数据库结构与当前格式不符（{what}）。当前版本不支持修复，\
         请删除数据库后重新启动，Session 会从已配置的 Agent 数据源重新摄入。"
    ))
}

/// One object the current format is made of, and the `sqlite_master` type it
/// has to be — a virtual table is still a `table`, so a same-named view can
/// never satisfy the check.
struct RequiredObject {
    kind: &'static str,
    name: &'static str,
}

/// Objects the current format is made of, read off [`CURRENT_SCHEMA`] so a new
/// table or index cannot be forgotten here.
fn required_objects() -> Vec<RequiredObject> {
    let mut out = Vec::new();
    for (marker, kind) in [
        ("CREATE TABLE IF NOT EXISTS ", "table"),
        ("CREATE VIRTUAL TABLE IF NOT EXISTS ", "table"),
        ("CREATE UNIQUE INDEX IF NOT EXISTS ", "index"),
        ("CREATE INDEX IF NOT EXISTS ", "index"),
    ] {
        for tail in CURRENT_SCHEMA.split(marker).skip(1) {
            if let Some(name) = tail.split(|c: char| c.is_whitespace() || c == '(').next() {
                out.push(RequiredObject { kind, name });
            }
        }
    }
    out
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
    -- The Logical Session: user-visible conversation + lifecycle + Owner.
    -- Identity is (agent, root_agent_session_id): the ROOT member's real
    -- Resume identity, not any child's external id (重构方案 §10.1).
    CREATE TABLE IF NOT EXISTS sessions (
      id TEXT PRIMARY KEY,
      agent TEXT NOT NULL,
      root_agent_session_id TEXT NOT NULL,
      title TEXT,
      cwd TEXT,
      workspace_path_id TEXT,
      project_id TEXT REFERENCES projects(id),
      -- Semantic ownership: at most one Owner Workstream per Session
      -- (方案 §3.3). Deleting the Workstream clears this, never the row.
      owner_workstream_id TEXT
        REFERENCES workstreams(id) ON DELETE SET NULL,
      -- Fork provenance only (§2.2): lifecycle / Owner / Conversation stay
      -- fully independent of the fork source.
      forked_from_session_id TEXT
        REFERENCES sessions(id) ON DELETE SET NULL,
      started_at TEXT,
      last_activity_at TEXT,
      last_conversation_at TEXT,
      -- lifecycle: NULL = Normal, NOT NULL = Trash (RFC3339).
      trashed_at TEXT,
      UNIQUE(agent, root_agent_session_id)
    );
    -- One internal execution unit of a Logical Session (§5). Only relation
    -- 'root' may produce session_messages; exactly one root per session is
    -- enforced by the partial unique index below.
    CREATE TABLE IF NOT EXISTS session_members (
      id TEXT PRIMARY KEY,
      session_id TEXT NOT NULL
        REFERENCES sessions(id) ON DELETE CASCADE,
      agent TEXT NOT NULL,
      source_member_id TEXT NOT NULL,
      relation TEXT NOT NULL CHECK (relation IN ('root', 'child', 'side')),
      parent_source_member_id TEXT,
      source_kind TEXT NOT NULL,
      source_path TEXT NOT NULL,
      cwd TEXT,
      started_at TEXT,
      last_activity_at TEXT,
      metadata TEXT NOT NULL DEFAULT '{}',
      UNIQUE(agent, source_member_id)
    );
    CREATE UNIQUE INDEX IF NOT EXISTS idx_session_members_one_root
      ON session_members(session_id) WHERE relation = 'root';
    CREATE INDEX IF NOT EXISTS idx_session_members_session
      ON session_members(session_id);
    CREATE INDEX IF NOT EXISTS idx_session_members_parent
      ON session_members(agent, parent_source_member_id);
    -- Where each member stopped reading ITS source (§8.1). Member-owned, never
    -- session-owned; the Context frontier lives in session_context_state.
    CREATE TABLE IF NOT EXISTS session_member_cursors (
      member_id TEXT PRIMARY KEY
        REFERENCES session_members(id) ON DELETE CASCADE,
      source_file_identity TEXT NOT NULL DEFAULT '',
      generation INTEGER NOT NULL DEFAULT 0,
      byte_offset INTEGER NOT NULL DEFAULT 0,
      last_seen_size INTEGER NOT NULL DEFAULT 0,
      mtime REAL,
      prefix_hash TEXT NOT NULL DEFAULT '',
      identity_tail_hash TEXT NOT NULL DEFAULT '',
      -- Stateful provenance frontier (Provenance 方案 §15). Only a stateful
      -- evidence adapter writes these; they are cursor state, never a UI
      -- authority, and live in the same transaction as the messages they
      -- cover so the bytes frontier and the provenance state cannot drift.
      active_provider TEXT,
      active_model TEXT
    );
    -- The ONLY conversation store (§6): user/assistant turns of the ROOT
    -- member. Identity dedup is (member_id, source_identity_hash); sequence is
    -- NoEnding's own per-session counter.
    CREATE TABLE IF NOT EXISTS session_messages (
      id TEXT PRIMARY KEY,
      session_id TEXT NOT NULL
        REFERENCES sessions(id) ON DELETE CASCADE,
      member_id TEXT NOT NULL
        REFERENCES session_members(id) ON DELETE CASCADE,
      sequence INTEGER NOT NULL,
      source_message_id TEXT,
      source_generation INTEGER NOT NULL DEFAULT 0,
      source_position TEXT NOT NULL DEFAULT '',
      source_identity_hash TEXT NOT NULL,
      ts TEXT,
      role TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
      content TEXT NOT NULL,
      -- Message-level generation provenance (Provenance 方案 §6/§7):
      -- source-confirmed only, meaningful for Assistant rows alone. NULL =
      -- the source cannot prove it; never inferred from configuration.
      provider TEXT,
      model TEXT,
      raw_ref TEXT NOT NULL,
      CHECK (
        role = 'assistant'
        OR (provider IS NULL AND model IS NULL)
      ),
      UNIQUE(session_id, sequence),
      UNIQUE(member_id, source_identity_hash)
    );
    CREATE INDEX IF NOT EXISTS idx_session_messages_session
      ON session_messages(session_id, sequence);
    -- The Logical Session's Context frontier (§8.2): how far Sync has consumed
    -- root conversation messages. Completely separate lifecycle from cursors.
    CREATE TABLE IF NOT EXISTS session_context_state (
      session_id TEXT PRIMARY KEY
        REFERENCES sessions(id) ON DELETE CASCADE,
      processed_message_sequence INTEGER NOT NULL DEFAULT 0
    );
    -- 1:1 execution snapshot per member (§7): current observable source state,
    -- not an append-only log. NULL = source does not provide the metric.
    CREATE TABLE IF NOT EXISTS session_member_stats (
      member_id TEXT PRIMARY KEY
        REFERENCES session_members(id) ON DELETE CASCADE,
      tool_call_count INTEGER,
      tool_error_count INTEGER,
      compaction_count INTEGER,
      side_activity_count INTEGER,
      input_tokens INTEGER,
      output_tokens INTEGER,
      cached_tokens INTEGER,
      reasoning_tokens INTEGER,
      cost REAL,
      -- model / provider / effort were removed in v3 (Provenance 方案 §9):
      -- message provenance lives in session_messages, and a member-level
      -- "current model" was a second unclear authority, never an execution fact.
      updated_at TEXT NOT NULL,
      extra TEXT NOT NULL DEFAULT '{}'
    );
    -- Ingestion problems that are deliberately NOT Sessions (§11): unattachable
    -- child/side sources and the like. Never Search / Context / Owner /
    -- lifecycle; deleted when the source resolves.
    CREATE TABLE IF NOT EXISTS ingestion_diagnostics (
      id TEXT PRIMARY KEY,
      diagnostic_key TEXT NOT NULL UNIQUE,
      agent TEXT NOT NULL,
      kind TEXT NOT NULL,
      source_member_id TEXT,
      parent_source_member_id TEXT,
      source_path TEXT,
      reason TEXT NOT NULL,
      first_seen_at TEXT NOT NULL,
      last_seen_at TEXT NOT NULL,
      observation_count INTEGER NOT NULL DEFAULT 1,
      details TEXT NOT NULL DEFAULT '{}'
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
      delta_fingerprint TEXT
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

    CREATE UNIQUE INDEX IF NOT EXISTS idx_projects_git_id
      ON projects(git_id)
      WHERE git_id IS NOT NULL;

    -- The search projection: one row per searchable document, written by the
    -- app itself rather than indexed from another table. FTS5 is part of this
    -- format, not an option — this dependency graph bundles SQLite with
    -- `-DSQLITE_ENABLE_FTS5` (libsqlite3-sys `bundled`), so a build that cannot
    -- create this table is a build whose search would silently return nothing.
    CREATE VIRTUAL TABLE IF NOT EXISTS search_index USING fts5(
      kind, ref_id, parent_id, title, body, tokenize = 'unicode61'
    );
"#;

/// Create the current format. Only ever called for a completely empty file, so
/// every `IF NOT EXISTS` here is a fresh creation.
fn create(conn: &Connection) -> Result<()> {
    conn.execute_batch(CURRENT_SCHEMA)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The validation list is read off the DDL, so this pins the reader itself:
    /// every object of the current format must come out of it, with the kind it
    /// must have in `sqlite_master`.
    #[test]
    fn required_objects_covers_the_whole_ddl() {
        let found: Vec<(&str, &str)> = required_objects()
            .into_iter()
            .map(|o| (o.kind, o.name))
            .collect();
        for (kind, name) in [
            ("table", "projects"),
            ("table", "workstreams"),
            ("table", "sessions"),
            ("table", "session_members"),
            ("table", "session_member_cursors"),
            ("table", "session_messages"),
            ("table", "session_context_state"),
            ("table", "session_member_stats"),
            ("table", "ingestion_diagnostics"),
            ("table", "context_items"),
            ("table", "context_item_revisions"),
            ("table", "sync_runs"),
            ("table", "agent_installations"),
            ("table", "settings"),
            ("table", "launch_intents"),
            ("table", "context_conflicts"),
            ("table", "context_deliveries"),
            ("table", "ingest_sources"),
            ("table", "assistant_sessions"),
            ("table", "assistant_messages"),
            ("table", "context_conflict_events"),
            ("table", "workstream_review_state"),
            ("table", "git_identities"),
            ("table", "workspace_paths"),
            ("table", "workstream_paths"),
            ("table", "search_index"),
            ("index", "idx_sessions_project"),
            ("index", "idx_items_workstream"),
            ("index", "idx_sessions_owner_workstream"),
            ("index", "idx_session_members_one_root"),
            ("index", "idx_session_members_session"),
            ("index", "idx_session_members_parent"),
            ("index", "idx_session_messages_session"),
            ("index", "idx_intents_status"),
            ("index", "idx_sync_runs_fingerprint"),
            ("index", "idx_conflict_events_conflict"),
            ("index", "idx_workspace_paths_project"),
            ("index", "idx_workspace_paths_canonical"),
            ("index", "idx_workstream_paths_ws"),
            ("index", "idx_workstream_paths_path"),
            ("index", "idx_projects_git_id"),
        ] {
            assert!(
                found.contains(&(kind, name)),
                "required_objects() missed {kind} {name}"
            );
        }
        assert_eq!(found.len(), 41, "required_objects() found an object twice");
    }
}
