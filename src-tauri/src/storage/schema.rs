//! Database identity, schema creation and validation.
//!
//! Empty files are initialized atomically. Existing files must carry the
//! NoEnding identity and a supported schema version, and satisfy the declared
//! structure before any runtime defaults or persistent connection settings run.

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{other, Result};

/// Identity written into the SQLite header, so a NoEnding database can be told
/// apart from any other SQLite file. `0x4E453031` spells "NE01".
pub const DATABASE_APPLICATION_ID: i32 = 0x4E45_3031;

/// Published persistence formats increase independently of the app version.
pub const DATABASE_FORMAT_VERSION: i64 = 1;
/// Stable identity of this format lineage; a version alone does not identify it.
pub const DATABASE_BASELINE: &str = "7dc9a8c1-4a08-4430-985b-3f591202614d";
const BASELINE_SETTING: &str = "database.baseline";
const DEFINITION_SETTING: &str = "database.definition";

/// Initialize an empty database, or validate a matching persistence format.
/// Identity and definition checks run before defaults or persistent PRAGMAs.
pub fn open_or_create(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    let application_id: i32 = tx.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let version: i64 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if application_id == 0 && version == 0 && !has_user_objects(&tx)? {
        create(&tx)?;
        tx.execute(
            "INSERT INTO settings(key,value) VALUES(?1,?2),(?3,?4)",
            params![
                BASELINE_SETTING,
                DATABASE_BASELINE,
                DEFINITION_SETTING,
                definition_fingerprint()
            ],
        )?;
        tx.pragma_update(None, "application_id", DATABASE_APPLICATION_ID)?;
        tx.pragma_update(None, "user_version", DATABASE_FORMAT_VERSION)?;
    } else if application_id == DATABASE_APPLICATION_ID && version == DATABASE_FORMAT_VERSION {
        validate_baseline(&tx)?;
        validate(&tx)?;
        let definition: Option<String> = tx
            .query_row(
                "SELECT value FROM settings WHERE key=?1",
                [DEFINITION_SETTING],
                |r| r.get(0),
            )
            .optional()?;
        if definition.as_deref() != Some(definition_fingerprint()) {
            return Err(other("数据库开发结构与当前代码不匹配。请保留数据并先备份，再执行经过验证的开发转换；不能仅修改版本号或结构标识。"));
        }
    } else {
        return Err(format_mismatch(application_id, version));
    }
    tx.commit()?;
    Ok(())
}

fn validate_baseline(conn: &Connection) -> Result<()> {
    let has_settings: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='settings')",
        [],
        |r| r.get(0),
    )?;
    let baseline: Option<String> = if has_settings {
        conn.query_row(
            "SELECT value FROM settings WHERE key=?1",
            [BASELINE_SETTING],
            |r| r.get(0),
        )
        .optional()?
    } else {
        None
    };
    if baseline.as_deref() != Some(DATABASE_BASELINE) {
        return Err(other("数据库基线与此版本 NoEnding 不兼容。请保留数据库，使用匹配的程序版本或经过验证的数据转换工具。"));
    }
    Ok(())
}

/// The stamp detects changes to an unpublished definition without assigning
/// each draft a public version. Comments and whitespace outside SQL literals
/// do not change it; data and environment-dependent defaults are excluded.
fn definition_fingerprint() -> &'static str {
    static FINGERPRINT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    FINGERPRINT.get_or_init(|| {
        // JSON field names/types matter; documentation and input defaults do not.
        use sha2::{Digest, Sha256};
        let mut summary =
            serde_json::to_value(rmcp::schemars::schema_for!(crate::domain::Summary)).unwrap();
        remove_schema_annotations(&mut summary);
        let definition =
            serde_json::json!({"sql": fingerprint(CURRENT_SCHEMA), "summary": summary});
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&definition).unwrap())
        )
    })
}

fn remove_schema_annotations(schema: &mut serde_json::Value) {
    match schema {
        serde_json::Value::Object(object) => {
            for annotation in ["$schema", "title", "description", "default", "examples"] {
                object.remove(annotation);
            }
            for (key, child) in object {
                if matches!(
                    key.as_str(),
                    "properties"
                        | "$defs"
                        | "definitions"
                        | "patternProperties"
                        | "dependentSchemas"
                ) {
                    // These maps use domain field/type names, including "title".
                    if let Some(entries) = child.as_object_mut() {
                        for value in entries.values_mut() {
                            remove_schema_annotations(value);
                        }
                    }
                } else {
                    remove_schema_annotations(child);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                remove_schema_annotations(child);
            }
        }
        _ => {}
    }
}

fn fingerprint(definition: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut tokens = Vec::new();
    let mut chars = definition.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '-' && chars.peek() == Some(&'-') {
            for next in chars.by_ref() {
                if next == '\n' {
                    break;
                }
            }
        } else if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            while let Some(next) = chars.next() {
                if next == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    break;
                }
            }
        } else if matches!(c, '\'' | '"' | '`' | '[') {
            let end = if c == '[' { ']' } else { c };
            let mut token = String::from(c);
            while let Some(next) = chars.next() {
                token.push(next);
                if next == end {
                    if chars.peek() == Some(&end) {
                        token.push(chars.next().unwrap());
                    } else {
                        break;
                    }
                }
            }
            tokens.push(token);
        } else if c.is_ascii_alphanumeric() || c == '_' {
            let mut token = String::from(c);
            while chars
                .peek()
                .is_some_and(|v| v.is_ascii_alphanumeric() || *v == '_')
            {
                token.push(chars.next().unwrap());
            }
            tokens.push(token.to_ascii_lowercase());
        } else if !c.is_whitespace() {
            tokens.push(c.to_string());
        }
    }
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&tokens).expect("SQL token serialization"))
    )
}

/// Fill in the defaults that depend on the *current* environment. Runs on every
/// start, so a newly supported Agent, a relocated `CODEX_HOME`, or a second
/// official surface gaining its own store (Antigravity's `agy` CLI) needs no
/// format change to be picked up.
///
/// Reconciliation, not migration: it inserts missing rows only, creates no
/// schema, and never overwrites registered custom sources.
pub fn reconcile_runtime_defaults(conn: &Connection) -> Result<()> {
    for agent in crate::domain::Agent::all() {
        for root in crate::platform::paths::agent_ingest_roots(*agent) {
            conn.execute(
                "INSERT OR IGNORE INTO ingest_sources (id, agent, path, origin)
                 VALUES (?1, ?2, ?3, 'default')",
                params![
                    crate::storage::new_id(),
                    agent.as_str(),
                    root.to_string_lossy().to_string()
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
    for (table, columns) in [
        ("session_contexts", &["refreshed_at"][..]),
        (
            "mailbox_messages",
            &[
                "seq",
                "id",
                "recipient_session_id",
                "sender_session_id",
                "body",
                "created_at",
                "read_at",
                "revision",
                "notify_immediately",
            ][..],
        ),
        ("workstream_context_state", &["refreshed_at"][..]),
        ("project_context_state", &["refreshed_at"][..]),
        (
            "workstream_session_frontiers",
            &["ingest_generation", "processed_through_seq"][..],
        ),
        (
            "project_session_frontiers",
            &["ingest_generation", "processed_through_seq"][..],
        ),
    ] {
        for column in columns {
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name=?2)",
                params![table, column],
                |r| r.get(0),
            )?;
            if !exists {
                return Err(structure_mismatch(format!("缺少 {table}.{column}")));
            }
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
    if !normalized.contains("using fts5")
        || !normalized.contains("content='search_content'")
        || !normalized.contains("content_rowid='id'")
    {
        return Err(structure_mismatch("search_index 不是 FTS5 索引表".into()));
    }
    Ok(())
}

fn structure_mismatch(what: String) -> crate::error::AppError {
    other(format!(
        "数据库结构校验失败（{what}）。请保留数据库，使用经过验证的备份恢复或联系维护者。"
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
        ("CREATE TRIGGER IF NOT EXISTS ", "trigger"),
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
         required_format={DATABASE_FORMAT_VERSION}）。请保留数据库并使用兼容的程序版本；不能通过修改版本号强行打开。"
    ))
}

/// The complete current structure: every table and index this build reads or
/// writes. Fresh creation and definition validation use this source of truth.
///
/// Domain records, current summaries, synchronization state and derived search
/// structures share the same transactional database.
const CURRENT_SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS projects (
      id TEXT PRIMARY KEY,
      name TEXT NOT NULL,
      git_id TEXT,
      name_customized INTEGER NOT NULL DEFAULT 0,
      visibility TEXT NOT NULL DEFAULT 'normal' CHECK (visibility IN ('normal', 'archived')),
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
    -- The Logical Session: user-visible conversation + lifecycle + tracking.
    -- Identity is (agent, root_agent_session_id): the root source's real
    -- Resume identity. A session IS its root source: exactly one source, one
    -- cursor, one frontier per session.
    CREATE TABLE IF NOT EXISTS sessions (
      id TEXT PRIMARY KEY,
      agent TEXT NOT NULL,
      root_agent_session_id TEXT NOT NULL,
      title TEXT,
      cwd TEXT,
      workspace_path_id TEXT,
      -- Fork provenance only: lifecycle / tracking / Conversation stay
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
    -- NoEnding's own per-session storage order, independent of the current
    -- conversation ordinal. Complete snapshots remove rows no longer
    -- referenced by the projection below.
    CREATE TABLE IF NOT EXISTS session_messages (
      id TEXT PRIMARY KEY,
      session_id TEXT NOT NULL
        REFERENCES sessions(id) ON DELETE CASCADE,
      sequence INTEGER NOT NULL,
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
    -- replaces the whole projection and retires obsolete session_messages.
    -- Conversation, search and Context read HERE.
    CREATE TABLE IF NOT EXISTS session_message_projection (
      session_id TEXT NOT NULL
        REFERENCES sessions(id) ON DELETE CASCADE,
      ordinal INTEGER NOT NULL,
      session_message_id TEXT NOT NULL
        REFERENCES session_messages(id) ON DELETE CASCADE,
      -- Cumulative 2000-character chunks through this ordinal. Maintained
      -- with the projection, so paging never rescans old message bodies.
      part_end INTEGER NOT NULL CHECK (part_end > 0),
      PRIMARY KEY (session_id, ordinal)
    );
    CREATE UNIQUE INDEX IF NOT EXISTS idx_projection_message
      ON session_message_projection(session_message_id);
    CREATE UNIQUE INDEX IF NOT EXISTS idx_projection_parts
      ON session_message_projection(session_id, part_end);
    -- The Logical Session's Context: system-derived, read-only, one current
    -- row per Session. No row means "never generated". Summary is a structured JSON object;
    -- revision is the CAS guard, and
    -- (ingest_generation, processed_through_seq) is the Session frontier in
    -- CURRENT-message-projection ordinals.
    CREATE TABLE IF NOT EXISTS session_contexts (
      session_id TEXT PRIMARY KEY
        REFERENCES sessions(id) ON DELETE CASCADE,
      summary TEXT NOT NULL DEFAULT '{"items":[]}'
        CHECK(json_valid(summary) AND json_type(summary) = 'object' AND json_type(summary, '$.items') IS 'array'),
      revision INTEGER NOT NULL DEFAULT 0,
      ingest_generation INTEGER NOT NULL DEFAULT 0,
      processed_through_seq INTEGER NOT NULL DEFAULT 0,
      updated_at TEXT NOT NULL,
      refreshed_at TEXT
    );
    -- The Workstream side of the two-level revision scheme.
    -- context_revision guards summary writes;
    -- input_revision marks tracked sessions / projects / title /
    -- description changes that leave inputs pending; AI records the
    -- input_revision it consumed so its own success does not leave the
    -- Workstream "pending" again. Created as (0, 1, 0) so the first explicit
    -- generation is allowed.
    CREATE TABLE IF NOT EXISTS workstream_context_state (
      workstream_id TEXT PRIMARY KEY REFERENCES workstreams(id) ON DELETE CASCADE,
      context_revision INTEGER NOT NULL DEFAULT 0,
      input_revision INTEGER NOT NULL DEFAULT 0,
      consumed_input_revision INTEGER NOT NULL DEFAULT 0,
      summary TEXT NOT NULL DEFAULT '{"items":[]}'
        CHECK(json_valid(summary) AND json_type(summary) = 'object' AND json_type(summary, '$.items') IS 'array'),
      updated_at TEXT,
      refreshed_at TEXT
    );
    -- What the Workstream has already consumed from each tracked Session.
    -- Written in the same transaction as a combined update; a Session that
    -- is no longer tracked is cleaned up on the next successful synthesis.
    CREATE TABLE IF NOT EXISTS workstream_session_frontiers (
      workstream_id TEXT NOT NULL REFERENCES workstreams(id) ON DELETE CASCADE,
      session_id TEXT NOT NULL REFERENCES sessions(id),
      session_context_revision INTEGER NOT NULL DEFAULT 0,
      ingest_generation INTEGER,
      processed_through_seq INTEGER,
      PRIMARY KEY (workstream_id, session_id)
    );
    -- One current project summary, an input fingerprint and consumed session
    -- summary revisions. Metadata changes mark pending, never discard a summary.
    CREATE TABLE IF NOT EXISTS project_context_state (
      project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
      context_revision INTEGER NOT NULL DEFAULT 0,
      input_fingerprint TEXT NOT NULL DEFAULT '',
      summary TEXT NOT NULL DEFAULT '{"items":[]}'
        CHECK(json_valid(summary) AND json_type(summary) = 'object' AND json_type(summary, '$.items') IS 'array'),
      updated_at TEXT,
      refreshed_at TEXT
    );
    CREATE TABLE IF NOT EXISTS project_session_frontiers (
      project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
      session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
      session_context_revision INTEGER NOT NULL DEFAULT 0,
      ingest_generation INTEGER,
      processed_through_seq INTEGER,
      PRIMARY KEY (project_id, session_id)
    );
    CREATE TABLE IF NOT EXISTS agent_installations (
      agent TEXT PRIMARY KEY,
      executable_path TEXT,
      version TEXT
    );
    CREATE TABLE IF NOT EXISTS settings (
      key TEXT PRIMARY KEY,
      value TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS ingest_sources (
      id TEXT PRIMARY KEY,
      agent TEXT NOT NULL,
      path TEXT NOT NULL,
      origin TEXT NOT NULL DEFAULT 'user',   -- default | user
      UNIQUE(agent, path)
    );
CREATE TABLE IF NOT EXISTS sync_checks (
  scope TEXT NOT NULL CHECK(scope IN ('all','source','session')),
  target_id TEXT NOT NULL,
  started_at TEXT NOT NULL,
  finished_at TEXT NOT NULL,
  outcome TEXT NOT NULL CHECK(outcome IN ('success','partial','failed','missing','cancelled')),
  checked_at TEXT,
  PRIMARY KEY(scope,target_id),
  CHECK(scope != 'all' OR target_id = '')
);
CREATE TRIGGER IF NOT EXISTS sync_checks_session_delete AFTER DELETE ON sessions BEGIN
  DELETE FROM sync_checks WHERE scope='session' AND target_id=old.id;
END;
CREATE TRIGGER IF NOT EXISTS sync_checks_session_source AFTER UPDATE OF source_path,source_kind ON sessions
WHEN old.source_path!=new.source_path OR old.source_kind!=new.source_kind BEGIN
  DELETE FROM sync_checks WHERE scope='session' AND target_id=old.id;
END;
CREATE TRIGGER IF NOT EXISTS sync_checks_source_delete AFTER DELETE ON ingest_sources BEGIN
  DELETE FROM sync_checks WHERE scope='source' AND target_id=old.id;
  DELETE FROM sync_checks WHERE scope='all';
END;
CREATE TRIGGER IF NOT EXISTS sync_checks_source_update AFTER UPDATE OF agent,path,origin ON ingest_sources BEGIN
  DELETE FROM sync_checks WHERE scope='source' AND target_id=old.id;
  DELETE FROM sync_checks WHERE scope='all';
END;
CREATE TRIGGER IF NOT EXISTS sync_checks_source_insert AFTER INSERT ON ingest_sources BEGIN
  DELETE FROM sync_checks WHERE scope='all';
END;
    CREATE INDEX IF NOT EXISTS idx_sessions_workspace_path ON sessions(workspace_path_id);
    CREATE INDEX IF NOT EXISTS idx_sessions_activity
      ON sessions(COALESCE(last_conversation_at, last_activity_at, started_at) DESC, id);
    CREATE INDEX IF NOT EXISTS idx_sessions_unarchived_activity
      ON sessions(COALESCE(last_conversation_at, last_activity_at, started_at) DESC, id)
      WHERE archived_at IS NULL;
    CREATE INDEX IF NOT EXISTS idx_sessions_fork_parent ON sessions(forked_from_session_id);
    CREATE INDEX IF NOT EXISTS idx_workstream_frontiers_session
      ON workstream_session_frontiers(session_id);
    CREATE INDEX IF NOT EXISTS idx_project_frontiers_session
      ON project_session_frontiers(session_id);

    CREATE TABLE IF NOT EXISTS git_identities (
      id TEXT PRIMARY KEY,
      common_dir TEXT NOT NULL UNIQUE,
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
      exists_on_disk INTEGER NOT NULL DEFAULT 1
    );
    CREATE TABLE IF NOT EXISTS workstream_projects (
      workstream_id TEXT NOT NULL REFERENCES workstreams(id) ON DELETE CASCADE,
      project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
      PRIMARY KEY(workstream_id, project_id)
    );
    CREATE INDEX IF NOT EXISTS idx_workstream_projects_project ON workstream_projects(project_id);
    CREATE TABLE IF NOT EXISTS session_workstreams (
      session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
      workstream_id TEXT NOT NULL REFERENCES workstreams(id) ON DELETE CASCADE,
      PRIMARY KEY(session_id, workstream_id)
    );
    CREATE INDEX IF NOT EXISTS idx_session_workstreams_task ON session_workstreams(workstream_id);
    CREATE INDEX IF NOT EXISTS idx_workspace_paths_project ON workspace_paths(project_id);

    CREATE UNIQUE INDEX IF NOT EXISTS idx_projects_git_id
      ON projects(git_id)
      WHERE git_id IS NOT NULL;

    -- Routing has ordinary indexes. Message documents contain no prose copy;
    -- their immutable content is resolved through search_content. Other kinds
    -- store their compact, assembled search document here.
    CREATE TABLE IF NOT EXISTS search_documents (
      id INTEGER PRIMARY KEY,
      kind TEXT NOT NULL CHECK (kind IN ('session','message')),
      ref_id TEXT NOT NULL,
      parent_id TEXT NOT NULL DEFAULT '',
      title TEXT NOT NULL DEFAULT '',
      body TEXT NOT NULL DEFAULT '',
      UNIQUE(kind, ref_id)
    );
    CREATE INDEX IF NOT EXISTS idx_search_documents_parent
      ON search_documents(kind, parent_id);
    CREATE VIEW IF NOT EXISTS search_content AS
      SELECT d.id, d.kind, d.ref_id, d.parent_id, d.title,
             CASE WHEN d.kind = 'message' THEN COALESCE(m.content, d.body) ELSE d.body END AS body
      FROM search_documents d LEFT JOIN session_messages m
        ON d.kind = 'message' AND m.id = d.ref_id AND m.session_id = d.parent_id;
    -- External content avoids FTS5's second stored copy of every document.
    CREATE VIRTUAL TABLE IF NOT EXISTS search_index USING fts5(
      kind UNINDEXED, ref_id UNINDEXED, parent_id UNINDEXED, title, body,
      content='search_content', content_rowid='id', tokenize = 'unicode61'
    );
    CREATE TRIGGER IF NOT EXISTS search_documents_insert AFTER INSERT ON search_documents BEGIN
      INSERT INTO search_index(rowid, kind, ref_id, parent_id, title, body)
        SELECT id, kind, ref_id, parent_id, title, body FROM search_content WHERE id = new.id;
    END;
    CREATE TRIGGER IF NOT EXISTS search_documents_delete BEFORE DELETE ON search_documents BEGIN
      INSERT INTO search_index(search_index, rowid, kind, ref_id, parent_id, title, body)
        SELECT 'delete', id, kind, ref_id, parent_id, title, body FROM search_content WHERE id = old.id;
    END;
    CREATE TRIGGER IF NOT EXISTS search_documents_before_update BEFORE UPDATE ON search_documents BEGIN
      INSERT INTO search_index(search_index, rowid, kind, ref_id, parent_id, title, body)
        SELECT 'delete', id, kind, ref_id, parent_id, title, body FROM search_content WHERE id = old.id;
    END;
    CREATE TRIGGER IF NOT EXISTS search_documents_update AFTER UPDATE ON search_documents BEGIN
      INSERT INTO search_index(rowid, kind, ref_id, parent_id, title, body)
        SELECT id, kind, ref_id, parent_id, title, body FROM search_content WHERE id = new.id;
    END;
    -- Keep the external content available until its postings are retired,
    -- including a message removed by a session foreign-key cascade.
    CREATE TRIGGER IF NOT EXISTS session_messages_search_delete BEFORE DELETE ON session_messages BEGIN
      DELETE FROM search_documents WHERE kind = 'message' AND ref_id = old.id;
    END;
CREATE TABLE IF NOT EXISTS mailbox_messages (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  id TEXT NOT NULL UNIQUE,
  recipient_session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  sender_session_id TEXT,
  body TEXT NOT NULL CHECK(length(trim(body)) > 0),
  created_at TEXT NOT NULL,
  read_at TEXT,
  revision INTEGER NOT NULL DEFAULT 1 CHECK(revision > 0),
  notify_immediately INTEGER NOT NULL DEFAULT 0 CHECK(notify_immediately IN (0,1))
);
CREATE INDEX IF NOT EXISTS mailbox_recipient ON mailbox_messages(recipient_session_id,seq);
CREATE INDEX IF NOT EXISTS mailbox_unread ON mailbox_messages(recipient_session_id,seq) WHERE read_at IS NULL;

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

    #[test]
    fn json_format_identity_ignores_documentation_but_preserves_domain_field_names_and_types() {
        let mut base = serde_json::json!({"type":"object","properties":{"title":{"type":"string"},"id":{"type":"string"}},"required":["title","id"]});
        let mut documented = base.clone();
        documented["title"] = serde_json::json!("Summary item");
        documented["properties"]["title"]["description"] = serde_json::json!("Editable heading");
        documented["properties"]["id"]["default"] = serde_json::json!("");
        remove_schema_annotations(&mut base);
        remove_schema_annotations(&mut documented);
        assert_eq!(documented, base);
        assert_eq!(base["properties"]["title"]["type"], "string");
        let mut changed = base.clone();
        changed["properties"]["id"]["type"] = serde_json::json!("integer");
        remove_schema_annotations(&mut changed);
        assert_ne!(changed, base);
    }

    #[test]
    fn definition_identity_ignores_formatting_but_preserves_columns_and_literal_content() {
        let base = "CREATE TABLE t (v TEXT DEFAULT 'a b');";
        assert_eq!(
            fingerprint(base),
            fingerprint(
                "-- a comment\ncreate table t(v text /* another comment */ default 'a b');"
            )
        );
        assert_ne!(
            fingerprint(base),
            fingerprint("CREATE TABLE t (v TEXT DEFAULT 'ab');")
        );
        assert_ne!(
            fingerprint(base),
            fingerprint("CREATE TABLE t (v TEXT DEFAULT 'a b', extra TEXT);")
        );
        assert_ne!(
            fingerprint("CREATE TABLE t (v TEXT NOT NULL);"),
            fingerprint("CREATE TABLE t (v TEXTNOTNULL);")
        );
        assert_ne!(
            fingerprint("CREATE TABLE t (v TEXT DEFAULT '--');"),
            fingerprint("CREATE TABLE t (v TEXT DEFAULT '');")
        );
    }

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
             WHERE type IN ('table', 'index', 'view', 'trigger') AND sql IS NOT NULL
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
    /// use the `UNIQUE(agent, path)` guard to keep re-runs
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

        let origin: String = conn.query_row(
            "SELECT origin FROM ingest_sources WHERE agent = 'antigravity' AND path LIKE '%antigravity-cli'", [], |r|r.get(0)
        ).unwrap();
        assert_eq!(origin, "default");
        let enabled_columns: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('ingest_sources') WHERE name='enabled'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(enabled_columns, 0);

        reconcile_runtime_defaults(&conn).unwrap();
        assert_eq!(count(&conn), 2, "re-run must not duplicate");
    }
}
