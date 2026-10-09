//! The ONE database format this build understands.
//!
//! A database is recognised by *identity*, not by guessing: creation stamps the
//! SQLite header with [`DATABASE_APPLICATION_ID`] +
//! [`DATABASE_FORMAT_VERSION`], and opening demands that pair and then validates
//! that every object [`CURRENT_SCHEMA`] declares is present. An empty file is
//! created; a current-format database is used as it is and never repaired; a
//! foreign file, an older generation or a half-created schema is refused —
//! "no repair" never means "no validation".
//!
//! No migration chain and no `ALTER TABLE`: a breaking change edits
//! [`CURRENT_SCHEMA`] and bumps [`DATABASE_FORMAT_VERSION`], and the local
//! database is rebuilt from the Agent sources. The database is a projection over
//! that data — except the Workstream/Context state NoEnding itself owns, which a
//! rebuild does NOT restore.

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{other, Result};

/// Identity written into the SQLite header, so a NoEnding database can be told
/// apart from any other SQLite file. `0x4E6F456E` spells "NoEn".
pub const DATABASE_APPLICATION_ID: i32 = 0x4E6F_456E;

/// Exact SQLite format generation required by this build.
///
/// No migrations and no backwards compatibility: a database whose generation
/// differs must be discarded and rebuilt from Agent source data.
///
/// v1 — ingestion writes FACTS only (sessions, members, messages, cursors,
/// the current-message projection, ingest generation); Context is written
/// only by an explicit user action. The Session frontier is
/// `(ingest_generation, processed_through_seq)` over the current-message
/// projection; the Workstream side keeps its own `context_revision` /
/// `input_revision` plus per-Session consumption frontier.
///
/// 2026-09-29 统计功能退役: `session_member_stats` / `usage_events` /
/// `session_member_usage` / `ingest_usage_claims`（连同各 adapter 的用量分支与
/// claim 注册表）已删除——产品只保留会话层。对象清单随之收缩，删除时重建。
///
/// 2026-10-05 模型溯源退役: `session_messages.provider/model` 与
/// `session_member_cursors.active_provider/active_model`（连同有状态读取器的
/// provenance 前沿）已删除。
///
/// 2026-10-05 会话扁平化: `session_members` / `session_member_cursors` /
/// `session_ingest_state` / `ingestion_diagnostics` 四表删除——一个会话恰有一个
/// 根源、一份游标、一个事实前沿，全部并入 `sessions` 的列；消息表随之去掉
/// member 维度。拓扑守卫与诊断页失去存在前提（不再存任何非根成员），一并退场。
/// 删除数据库重建。
/// v2 — Workstream has only archive state; Session trash becomes archived_at.
/// v3 — remove unused fields and the Session Project cache; slim FTS metadata.
pub const DATABASE_FORMAT_VERSION: i64 = 3;

/// Open an existing current-format database, or create one.
///
/// The only accepted starting points are a completely empty file and an intact
/// database carrying this build's identity; everything else is refused with a
/// message that says so. A current-format database is returned untouched: the
/// startup path performs no schema repair, because SQLite schema is not
/// self-healing and a half-created database must never look usable.
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
/// start, so a newly supported Agent, a relocated `CODEX_HOME`, or a second
/// official surface gaining its own store (Antigravity's `agy` CLI) needs no
/// format change to be picked up.
///
/// Reconciliation, not migration: it inserts missing rows only, creates no
/// schema, and never overwrites a user's decision (defaults start DISABLED).
pub fn reconcile_runtime_defaults(conn: &Connection) -> Result<()> {
    for agent in crate::domain::Agent::all() {
        for root in crate::platform::paths::agent_ingest_roots(*agent) {
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
        ("CREATE VIEW IF NOT EXISTS ", "view"),
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
/// Agent facts and rebuildable projections coexist with NoEnding-owned state.
/// Tasks, ownership, archive state, Context history and settings must be
/// preserved explicitly when replacing the database.
const CURRENT_SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS projects (
      id TEXT PRIMARY KEY,
      name TEXT NOT NULL,
      git_id TEXT,
      name_customized INTEGER NOT NULL DEFAULT 0,
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS workstreams (
      id TEXT PRIMARY KEY,
      title TEXT NOT NULL,
      description TEXT NOT NULL DEFAULT '',
      visibility TEXT NOT NULL DEFAULT 'normal',
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL
    );
    -- The Logical Session: user-visible conversation + lifecycle + Owner.
    -- Identity is (agent, root_agent_session_id): the root source's real
    -- Resume identity. A session IS its root source: the former
    -- session_members / session_member_cursors / session_ingest_state rows
    -- live here as columns — exactly one source, one cursor, one frontier
    -- per session.
    CREATE TABLE IF NOT EXISTS sessions (
      id TEXT PRIMARY KEY,
      agent TEXT NOT NULL,
      root_agent_session_id TEXT NOT NULL,
      title TEXT,
      cwd TEXT,
      workspace_path_id TEXT,
      -- Semantic ownership: at most one Owner Workstream per Session.
      -- Deleting the Workstream clears this, never the row.
      owner_workstream_id TEXT
        REFERENCES workstreams(id) ON DELETE SET NULL,
      -- Fork provenance only: lifecycle / Owner / Conversation stay
      -- fully independent of the fork source.
      forked_from_session_id TEXT
        REFERENCES sessions(id) ON DELETE SET NULL,
      started_at TEXT,
      last_activity_at TEXT,
      last_conversation_at TEXT,
      -- NULL = unarchived, timestamp = archived. Blocks NoEnding resume and
      -- enables permanent local deletion; ingestion/search/Context stay live.
      archived_at TEXT,
      -- The ROOT source: format descriptor, path (reveal in UI) and the
      -- adapter's non-conversation structural facts (thread_source, …).
      source_kind TEXT NOT NULL,
      source_path TEXT NOT NULL,
      metadata TEXT NOT NULL DEFAULT '{}',
      -- Where reading stopped: file identity, source-shape generation, byte
      -- frontier and the append-proof / chain-tail hashes.
      source_file_identity TEXT NOT NULL DEFAULT '',
      source_generation INTEGER NOT NULL DEFAULT 0,
      source_byte_offset INTEGER NOT NULL DEFAULT 0,
      source_last_seen_size INTEGER NOT NULL DEFAULT 0,
      source_mtime REAL,
      source_prefix_hash TEXT NOT NULL DEFAULT '',
      source_tail_hash TEXT NOT NULL DEFAULT '',
      -- The conversation fact frontier: the generation of the CURRENT
      -- conversation and how many messages the store holds.
      fact_generation INTEGER NOT NULL DEFAULT 0,
      latest_message_seq INTEGER NOT NULL DEFAULT 0,
      UNIQUE(agent, root_agent_session_id)
    );
    -- The ONLY conversation store: user/assistant turns of the root source.
    -- Identity dedup is (session_id, source_identity_hash); sequence is
    -- NoEnding's own per-session append counter. Rows are AUDIT history and
    -- are never deleted by a re-scan — the CURRENT conversation is the
    -- projection below.
    CREATE TABLE IF NOT EXISTS session_messages (
      id TEXT PRIMARY KEY,
      session_id TEXT NOT NULL
        REFERENCES sessions(id) ON DELETE CASCADE,
      sequence INTEGER NOT NULL,
      source_message_id TEXT,
      source_generation INTEGER NOT NULL DEFAULT 0,
      source_position TEXT NOT NULL DEFAULT '',
      source_identity_hash TEXT NOT NULL,
      ts TEXT,
      role TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
      content TEXT NOT NULL,
      -- Assistant rows only: 1 = this is the turn's FINAL reply in the
      -- current conversation (the next projected message is not another
      -- assistant message). Derived from the projection at commit time, never
      -- ingested; an append that continues a turn demotes the old final.
      turn_final INTEGER NOT NULL DEFAULT 0,
      UNIQUE(session_id, sequence),
      UNIQUE(session_id, source_identity_hash)
    );
    -- The CURRENT effective conversation: an ordered view over
    -- session_messages holding exactly one generation. A normal append adds
    -- to the tail; a source rewrite / truncate / reorder that changes the
    -- conversation raises the session fact generation and atomically
    -- replaces the whole projection, while the old session_messages rows stay
    -- for provenance audit. Conversation, search and Context read HERE.
    CREATE TABLE IF NOT EXISTS session_message_projection (
      session_id TEXT NOT NULL
        REFERENCES sessions(id) ON DELETE CASCADE,
      ordinal INTEGER NOT NULL,
      session_message_id TEXT NOT NULL
        REFERENCES session_messages(id) ON DELETE CASCADE,
      PRIMARY KEY (session_id, ordinal)
    );
    CREATE UNIQUE INDEX IF NOT EXISTS idx_projection_message
      ON session_message_projection(session_id, session_message_id);
    -- Workstream Context content authority: items + revisions + conflicts.
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
      created_at TEXT NOT NULL
    );
    -- The Logical Session's Context: system-derived, read-only, one current
    -- row per Session. No row means "never generated". The four JSON columns
    -- are the fixed summary structure; revision is the CAS guard, and
    -- (ingest_generation, processed_through_seq) is the Session frontier in
    -- CURRENT-message-projection ordinals.
    CREATE TABLE IF NOT EXISTS session_contexts (
      session_id TEXT PRIMARY KEY
        REFERENCES sessions(id) ON DELETE CASCADE,
      summary_current_state TEXT NOT NULL DEFAULT '',
      decisions TEXT NOT NULL DEFAULT '[]',
      open_questions TEXT NOT NULL DEFAULT '[]',
      next_steps TEXT NOT NULL DEFAULT '[]',
      revision INTEGER NOT NULL DEFAULT 0,
      ingest_generation INTEGER NOT NULL DEFAULT 0,
      processed_through_seq INTEGER NOT NULL DEFAULT 0,
      updated_at TEXT NOT NULL
    );
    -- Append-only history of every committed Session Context, so a
    -- Workstream update can cite `session-context:<id>:<revision>` and prove
    -- which summary it consumed.
    CREATE TABLE IF NOT EXISTS session_context_revisions (
      session_id TEXT NOT NULL
        REFERENCES sessions(id) ON DELETE CASCADE,
      revision INTEGER NOT NULL,
      summary_current_state TEXT NOT NULL DEFAULT '',
      decisions TEXT NOT NULL DEFAULT '[]',
      open_questions TEXT NOT NULL DEFAULT '[]',
      next_steps TEXT NOT NULL DEFAULT '[]',
      ingest_generation INTEGER NOT NULL DEFAULT 0,
      processed_through_seq INTEGER NOT NULL DEFAULT 0,
      created_at TEXT NOT NULL,
      PRIMARY KEY (session_id, revision)
    );
    -- The Workstream side of the two-level revision scheme.
    -- context_revision guards all ContextItem writes (manual + AI);
    -- input_revision marks manual Context edits / Owner-set / title /
    -- description changes that need re-synthesis; AI records the
    -- input_revision it consumed so its own success does not leave the
    -- Workstream "pending" again. Created as (0, 1, 0) so the first explicit
    -- generation is allowed.
    CREATE TABLE IF NOT EXISTS workstream_context_state (
      workstream_id TEXT PRIMARY KEY REFERENCES workstreams(id),
      context_revision INTEGER NOT NULL DEFAULT 0,
      input_revision INTEGER NOT NULL DEFAULT 0,
      consumed_input_revision INTEGER NOT NULL DEFAULT 0
    );
    -- What the Workstream has already consumed from each Owner Session.
    -- Written in the same transaction as a combined update; a Session that
    -- leaves the Owner set is cleaned up on the next successful synthesis.
    CREATE TABLE IF NOT EXISTS workstream_session_frontiers (
      workstream_id TEXT NOT NULL REFERENCES workstreams(id),
      session_id TEXT NOT NULL REFERENCES sessions(id),
      session_context_revision INTEGER NOT NULL DEFAULT 0,
      ingest_generation INTEGER NOT NULL DEFAULT 0,
      consumed_through_seq INTEGER NOT NULL DEFAULT 0,
      PRIMARY KEY (workstream_id, session_id)
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
      agent TEXT NOT NULL,
      owner_workstream_id TEXT REFERENCES workstreams(id) ON DELETE SET NULL,
      cwd TEXT,
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
    CREATE TABLE IF NOT EXISTS ingest_sources (
      id TEXT PRIMARY KEY,
      agent TEXT NOT NULL,
      path TEXT NOT NULL,
      enabled INTEGER NOT NULL DEFAULT 0,
      origin TEXT NOT NULL DEFAULT 'user',   -- default | user
      created_at TEXT NOT NULL,
      UNIQUE(agent, path)
    );
    CREATE INDEX IF NOT EXISTS idx_sessions_workspace_path ON sessions(workspace_path_id);
    CREATE INDEX IF NOT EXISTS idx_item_revisions_item ON context_item_revisions(item_id, created_at);
    CREATE INDEX IF NOT EXISTS idx_conflicts_workstream ON context_conflicts(workstream_id, created_at);
    CREATE INDEX IF NOT EXISTS idx_items_workstream ON context_items(workstream_id);
    CREATE INDEX IF NOT EXISTS idx_sessions_owner_workstream ON sessions(owner_workstream_id);
    CREATE INDEX IF NOT EXISTS idx_intents_status ON launch_intents(status);
    CREATE INDEX IF NOT EXISTS idx_workstream_frontiers_session
      ON workstream_session_frontiers(session_id);

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
      -- would need quoting in every statement. The domain
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
      kind UNINDEXED, ref_id UNINDEXED, parent_id UNINDEXED, title, body, tokenize = 'unicode61'
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
        let conn = Connection::open_in_memory().unwrap();
        create(&conn).unwrap();
        let actual = conn
            .prepare(
                "SELECT type, name FROM sqlite_master
             WHERE type IN ('table', 'index') AND sql IS NOT NULL
               AND name NOT LIKE 'sqlite_%'
               AND name NOT IN (SELECT name FROM pragma_table_list WHERE type = 'shadow')",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<std::collections::BTreeSet<_>>>()
            .unwrap();
        let parsed = found
            .iter()
            .map(|(kind, name)| (kind.to_string(), name.to_string()))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            parsed, actual,
            "the DDL reader must agree with SQLite's created objects"
        );
        assert_eq!(
            found.len(),
            found
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            "required_objects() found an object twice"
        );
    }

    /// Default ingest sources cover every surface an Agent's conversations
    /// live on. Antigravity has two (IDE store + `agy` CLI store); the rows
    /// start disabled, the `UNIQUE(agent, path)` guard keeps re-runs
    /// idempotent, and a collision of the two would surface here as 1 ≠ 2.
    #[test]
    fn reconcile_seeds_both_antigravity_stores() {
        let conn = Connection::open_in_memory().unwrap();
        create(&conn).unwrap();
        reconcile_runtime_defaults(&conn).unwrap();

        let count = |c: &Connection| -> i64 {
            c.query_row(
                "SELECT COUNT(*) FROM ingest_sources WHERE agent = 'antigravity'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(count(&conn), 2, "IDE and agy CLI stores must both seed");

        let (enabled, origin): (i64, String) = conn
            .query_row(
                "SELECT enabled, origin FROM ingest_sources WHERE agent = 'antigravity'
                 AND path LIKE '%antigravity-cli'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((enabled, origin.as_str()), (0, "default"));

        reconcile_runtime_defaults(&conn).unwrap();
        assert_eq!(count(&conn), 2, "re-run must not duplicate");
    }
}
