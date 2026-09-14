//! SQLite storage: schema, migrations and repository helpers.
//! Owns: domain data, session index, cursors, context items, FTS search.
//!
//! History integrity rules:
//! - `session_events` is append-only: the row identity is an app-owned `id`,
//!   and a stable `(session_id, source_identity_hash)` unique index makes
//!   re-scans idempotent. Rows are never replaced or overwritten.
//! - Cursors distinguish the *read* position (events durably ingested) from
//!   the *processed* position (events consumed by a committed SyncRun).

use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};

use crate::domain::*;
use crate::error::{other, Result};

pub struct Db(pub Connection);

/// Bump on any schema change. Fresh databases are stamped directly; older
/// (lower-version) databases are brought up by the idempotent ALTERs in
/// migrate(); higher versions refuse to open.
///
/// History: v2 added `prefix_hash` / `context_bundle_revisions`; v3 added
/// `identity_tail_hash` and (unsound for stores that held superseded history
/// from pre-upgrade rewrites) recomputed identity hashes in store order; v4
/// replaced that with a lazy, source-driven migration — legacy events carry
/// a claimable alias and are re-identified from the REAL source on the next
/// full re-scan, event ids preserved.
pub const SCHEMA_VERSION: i64 = 4;

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Identity of the first event in a chain (no known predecessor).
pub const IDENTITY_GENESIS: &str = "genesis";

/// Stable identity of a source event, used to make re-scans of the same
/// Agent transcript idempotent.
///
/// - Events WITH a native Agent event id: content hash of
///   `(native_id | kind | ts | text)` — the Agent guarantees the id is
///   unique per logical event, so identity is position-independent.
/// - Events WITHOUT one: chained hash `H(prev_event_hash | kind | ts | text)`.
///   The chain encodes *adjacency*: re-scanning an unchanged file prefix
///   reproduces the same chain (same logical event → dedup), while two
///   genuinely identical messages ("继续" sent twice) link to different
///   predecessors and stay distinct. A mid-file rewrite diverges the chain
///   exactly where content changed — everything after it is new evidence.
pub fn event_identity_hash(
    prev_hash: &str,
    source_event_id: Option<&str>,
    kind: &str,
    ts: Option<&str>,
    text: Option<&str>,
) -> String {
    use sha2::Digest;
    use std::fmt::Write;
    let normalized = text.unwrap_or("").trim();
    let mut h = sha2::Sha256::new();
    match source_event_id {
        Some(id) => h.update(id.as_bytes()),
        None => h.update(prev_hash.as_bytes()),
    }
    h.update([0x1f]);
    h.update(kind);
    h.update([0x1f]);
    h.update(ts.unwrap_or("-"));
    h.update([0x1f]);
    h.update(normalized);
    let digest = h.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        write!(out, "{:02x}", b).ok();
    }
    out
}

/// First component of the PRE-chain (≤ v2) fallback identity. Exists only
/// for the v4 migration alias: legacy rows are claimed onto the chained
/// identity by reproducing this exact historical hash from their own content.
pub const LEGACY_IDENTITY_FALLBACK: &str = "-";

/// Identity a fallback event (no native Agent id) carried before the chained
/// adjacency identity (≤ v2): content-only, position-independent. Byte-exact
/// historical format — never used for new events.
pub fn legacy_fallback_identity_hash(kind: &str, ts: Option<&str>, text: Option<&str>) -> String {
    event_identity_hash(LEGACY_IDENTITY_FALLBACK, None, kind, ts, text)
}

impl Db {
    pub fn open(path: &std::path::Path) -> Result<Db> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Db(conn);
        db.migrate()?;
        Ok(db)
    }

    pub fn conn(&self) -> &Connection {
        &self.0
    }

    /// Run `f` inside a single SQLite transaction: all writes commit together
    /// or not at all. This is the only sanctioned way to persist multi-step
    /// domain changes (sync mutations, ingest batches, …).
    pub fn tx<T>(&self, f: impl FnOnce(&Transaction) -> Result<T>) -> Result<T> {
        let tx = self.0.unchecked_transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    fn migrate(&self) -> Result<()> {
        // Schema version guard: an unknown NEWER database is refused with a
        // clear message instead of failing later with "no such column".
        // (v0 = pre-versioning dev database: current-shape tables, so the
        // idempotent column additions below bring it up to date.)
        let current_version: i64 = self.0.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if current_version > SCHEMA_VERSION {
            return Err(other(format!(
                "数据库 schema 版本 (v{}) 高于当前应用支持的 v{}，请先备份数据库文件后重建或降级应用。",
                current_version, SCHEMA_VERSION
            )));
        }

        self.0.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS projects (
              id TEXT PRIMARY KEY,
              name TEXT NOT NULL,
              description TEXT NOT NULL DEFAULT '',
              archived INTEGER NOT NULL DEFAULT 0,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );            CREATE TABLE IF NOT EXISTS project_resources (
              id TEXT PRIMARY KEY,
              project_id TEXT NOT NULL REFERENCES projects(id),
              kind TEXT NOT NULL,
              uri TEXT,
              metadata TEXT NOT NULL DEFAULT '{}',
              created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS workstreams (
              id TEXT PRIMARY KEY,
              project_id TEXT REFERENCES projects(id),
              title TEXT NOT NULL,
              description TEXT NOT NULL DEFAULT '',
              lifecycle TEXT NOT NULL DEFAULT 'open',
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
              project_id TEXT REFERENCES projects(id),
              raw_path TEXT NOT NULL,
              parent_agent_session_id TEXT,
              started_at TEXT,
              last_activity_at TEXT,
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
              legacy_identity_hash TEXT,
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
            CREATE TABLE IF NOT EXISTS session_workstream_bindings (
              session_id TEXT NOT NULL REFERENCES sessions(id),
              workstream_id TEXT NOT NULL REFERENCES workstreams(id),
              role TEXT NOT NULL DEFAULT 'related',
              source TEXT NOT NULL DEFAULT 'automatic_classification',
              confidence REAL NOT NULL DEFAULT 0.5,
              last_seen_revision TEXT,
              last_sync_cursor INTEGER NOT NULL DEFAULT 0,
              created_at TEXT NOT NULL,
              last_used_at TEXT NOT NULL,
              PRIMARY KEY (session_id, workstream_id)
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
              selected_workstream_ids TEXT NOT NULL DEFAULT '[]',
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
              updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS project_affinity_evidence (
              id TEXT PRIMARY KEY,
              session_id TEXT REFERENCES sessions(id),
              workstream_id TEXT REFERENCES workstreams(id),
              project_id TEXT NOT NULL REFERENCES projects(id),
              evidence_type TEXT NOT NULL,
              source TEXT NOT NULL DEFAULT '',
              score REAL NOT NULL DEFAULT 0,
              created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS context_deliveries (
              id TEXT PRIMARY KEY,
              session_id TEXT NOT NULL REFERENCES sessions(id),
              workstream_id TEXT NOT NULL REFERENCES workstreams(id),
              bundle_id TEXT NOT NULL,
              delivered_revisions TEXT NOT NULL DEFAULT '[]',
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
            CREATE INDEX IF NOT EXISTS idx_workstreams_project ON workstreams(project_id);
            CREATE INDEX IF NOT EXISTS idx_sessions_project ON sessions(project_id);
            CREATE INDEX IF NOT EXISTS idx_items_workstream ON context_items(workstream_id);
            CREATE INDEX IF NOT EXISTS idx_bindings_ws ON session_workstream_bindings(workstream_id);
            CREATE INDEX IF NOT EXISTS idx_events_session ON session_events(session_id, sequence);
            CREATE INDEX IF NOT EXISTS idx_intents_status ON launch_intents(status);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_sync_runs_fingerprint
              ON sync_runs(session_id, delta_fingerprint)
              WHERE status = 'ok' AND delta_fingerprint IS NOT NULL;
            "#,
        )?;

        self.0.execute_batch(
            r#"
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
            "#,
        )?;

        // FTS5 search index (external-content style: we manage rows manually).
        // If the bundled build lacks FTS5, search falls back to LIKE at query time.
        let fts_ok = self
            .0
            .execute_batch(
                r#"
                CREATE VIRTUAL TABLE IF NOT EXISTS search_index USING fts5(
                  kind, ref_id, parent_id, title, body, tokenize = 'unicode61'
                );
                "#,
            )
            .is_ok();
        if !fts_ok {
            eprintln!("[storage] FTS5 unavailable, search will use LIKE fallback");
        }

        // Seed the per-agent default source roots (~/.codex etc., honoring
        // env overrides). They start DISABLED: whether a source is ingested
        // is always the user's decision.
        self.ensure_default_ingest_sources()?;

        // Columns added after the first v0 dev databases were created:
        // CREATE TABLE IF NOT EXISTS cannot extend an existing table, so
        // add them idempotently (duplicate-column errors are expected and
        // ignored on already-current databases).
        for stmt in [
            "ALTER TABLE session_cursors ADD COLUMN prefix_hash TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE session_cursors ADD COLUMN identity_tail_hash TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE launch_intents ADD COLUMN context_bundle_revisions TEXT",
            "ALTER TABLE session_events ADD COLUMN legacy_identity_hash TEXT",
        ] {
            if let Err(e) = self.0.execute_batch(stmt) {
                if !e.to_string().contains("duplicate column name") {
                    return Err(e.into());
                }
            }
        }

        register_binding_rank_fn(&self.0)?;

        // v2/v3 → v4: pre-chain databases keep legacy event identities. The
        // append-only store is NOT a linear chain of the current source (a
        // rewrite/compact leaves superseded history behind), so hashes cannot
        // be recomputed from the store — they are re-derived LAZILY from the
        // real source: every legacy fallback event receives a claimable
        // alias, read cursors are rewound, and the next full re-scan claims
        // each event onto the new chained identity (event ids — and every
        // SourceReference to them — never change).
        if current_version < 4 {
            self.migrate_legacy_identity_aliases()?;
        }

        self.0.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(())
    }

    /// v4 migration (from v2 and from the shipped-v3 recompute): stamp every
    /// fallback event (no native Agent id) with its pre-chain legacy identity
    /// as a claimable alias, and rewind all read cursors so the next sync
    /// performs a full re-scan. During that re-scan, storage claims each
    /// legacy row onto the freshly computed chained identity — id preserved,
    /// duplicates impossible — and the cursor receives the true source-chain
    /// tail (v3 databases may carry a tail derived from the store, which the
    /// source, not the store, defines). Native-id events need no migration:
    /// their identity is identical under both schemes.
    fn migrate_legacy_identity_aliases(&self) -> Result<()> {
        let rows: Vec<(String, String, Option<String>, Option<String>)> = {
            let mut st = self.0.prepare(
                "SELECT id, kind, ts, text FROM session_events
                 WHERE source_event_id IS NULL AND legacy_identity_hash IS NULL",
            )?;
            let rows = st
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };
        for (id, kind, ts, text) in rows {
            let legacy = legacy_fallback_identity_hash(&kind, ts.as_deref(), text.as_deref());
            self.0.execute(
                "UPDATE session_events SET legacy_identity_hash = ?2 WHERE id = ?1",
                params![id, legacy],
            )?;
        }
        self.0.execute(
            "UPDATE session_cursors
             SET byte_offset = 0, last_seen_size = 0, prefix_hash = '', identity_tail_hash = ''",
            [],
        )?;
        Ok(())
    }

    /// Insert the standard agent data roots as disabled defaults. Idempotent.
    fn ensure_default_ingest_sources(&self) -> Result<()> {
        for agent in crate::domain::Agent::all() {
            if let Some(root) = crate::platform::paths::resolve_agent_data_dir(agent) {
                self.0.execute(
                    "INSERT OR IGNORE INTO ingest_sources (id, agent, path, enabled, origin, created_at)
                     VALUES (?1, ?2, ?3, 0, 'default', ?4)",
                    params![new_id(), agent.as_str(), root.to_string_lossy().to_string(), now()],
                )?;
            }
        }
        Ok(())
    }

    // ---------------- Projects ----------------

    pub fn list_projects(&self) -> Result<Vec<Project>> {
        let mut st = self.0.prepare(
            "SELECT id, name, description, archived, created_at, updated_at
             FROM projects WHERE archived = 0 ORDER BY updated_at DESC",
        )?;
        let rows = st
            .query_map([], row_project)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_project(&self, id: &str) -> Result<Option<Project>> {
        Ok(self
            .0
            .query_row(
                "SELECT id, name, description, archived, created_at, updated_at
                 FROM projects WHERE id = ?1",
                params![id],
                row_project,
            )
            .optional()?)
    }

    pub fn upsert_project(&self, p: &Project) -> Result<()> {
        upsert_project_conn(&self.0, p)?;
        self.index_project(p)?;
        Ok(())
    }

    /// Deleting a Project only detaches: Workstreams and Sessions survive
    /// with `project_id = NULL`. A Project is an optional organization layer,
    /// never the lifecycle owner of a Workstream.
    pub fn delete_project(&self, id: &str) -> Result<()> {
        self.tx(|tx| {
            tx.execute(
                "UPDATE workstreams SET project_id = NULL, updated_at = ?2 WHERE project_id = ?1",
                params![id, now()],
            )?;
            tx.execute(
                "UPDATE sessions SET project_id = NULL WHERE project_id = ?1",
                params![id],
            )?;
            tx.execute(
                "DELETE FROM project_resources WHERE project_id = ?1",
                params![id],
            )?;
            tx.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
            Ok(())
        })?;
        self.unindex("project", id);
        Ok(())
    }

    pub fn add_resource(&self, r: &ProjectResource) -> Result<()> {
        self.0.execute(
            "INSERT INTO project_resources (id, project_id, kind, uri, metadata, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                r.id,
                r.project_id,
                r.kind,
                r.uri,
                r.metadata.to_string(),
                r.created_at
            ],
        )?;
        Ok(())
    }

    pub fn list_resources(&self, project_id: &str) -> Result<Vec<ProjectResource>> {
        let mut st = self.0.prepare(
            "SELECT id, project_id, kind, uri, metadata, created_at
             FROM project_resources WHERE project_id = ?1 ORDER BY created_at",
        )?;
        let rows = st
            .query_map(params![project_id], |r| {
                Ok(ProjectResource {
                    id: r.get(0)?,
                    project_id: r.get(1)?,
                    kind: r.get(2)?,
                    uri: r.get(3)?,
                    metadata: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default(),
                    created_at: r.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn remove_resource(&self, id: &str) -> Result<()> {
        self.0
            .execute("DELETE FROM project_resources WHERE id = ?1", params![id])?;
        Ok(())
    }

    // ---------------- Workstreams ----------------

    pub fn list_workstreams(&self, project_id: Option<&str>) -> Result<Vec<Workstream>> {
        let (sql, has_filter): (&str, bool) = if project_id.is_some() {
            ("SELECT id, project_id, title, description, lifecycle, visibility, created_at, updated_at
              FROM workstreams WHERE project_id = ?1 ORDER BY updated_at DESC", true)
        } else {
            ("SELECT id, project_id, title, description, lifecycle, visibility, created_at, updated_at
              FROM workstreams ORDER BY updated_at DESC", false)
        };
        let mut st = self.0.prepare(sql)?;
        let map = |r: &Row| row_workstream(r);
        let rows = if has_filter {
            st.query_map(params![project_id.unwrap()], map)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            st.query_map([], map)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        Ok(rows)
    }

    /// Card stats for one Workstream: (session_count, latest session as
    /// (id, agent), that session's activity timestamp). "Latest" follows the
    /// transcript, not the binding: most recent activity wins.
    pub fn workstream_session_stats(
        &self,
        workstream_id: &str,
    ) -> Result<(i64, Option<(String, String)>, Option<String>)> {
        let count: i64 = self.0.query_row(
            "SELECT COUNT(*) FROM session_workstream_bindings WHERE workstream_id = ?1",
            params![workstream_id],
            |r| r.get(0),
        )?;
        let latest = self
            .0
            .query_row(
                "SELECT s.id, s.agent, COALESCE(s.last_activity_at, s.started_at) AS act
                 FROM session_workstream_bindings b
                 JOIN sessions s ON s.id = b.session_id
                 WHERE b.workstream_id = ?1
                 ORDER BY act DESC
                 LIMIT 1",
                params![workstream_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?;
        Ok((
            count,
            latest
                .as_ref()
                .map(|(id, agent, _)| (id.clone(), agent.clone())),
            latest.and_then(|(_, _, act)| act),
        ))
    }

    /// Text of the most recently updated active item of `kind`, content
    /// first, falling back to its title. Returns None when absent or empty.
    pub fn workstream_state_text(&self, workstream_id: &str, kind: &str) -> Result<Option<String>> {
        let row = self
            .0
            .query_row(
                "SELECT r.content, r.title
                 FROM context_items i
                 JOIN context_item_revisions r ON r.id = i.current_revision_id
                 WHERE i.workstream_id = ?1 AND i.kind = ?2 AND i.status = 'active'
                 ORDER BY i.updated_at DESC
                 LIMIT 1",
                params![workstream_id, kind],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;
        Ok(row.and_then(|(content, title)| {
            let c = content.trim();
            if !c.is_empty() {
                Some(c.to_string())
            } else {
                let t = title.trim();
                (!t.is_empty()).then(|| t.to_string())
            }
        }))
    }

    /// Latest context edit inside a Workstream (card "last active" signal).
    pub fn workstream_items_last_update(&self, workstream_id: &str) -> Result<Option<String>> {
        Ok(self
            .0
            .query_row(
                "SELECT MAX(updated_at) FROM context_items WHERE workstream_id = ?1",
                params![workstream_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    }

    pub fn get_workstream(&self, id: &str) -> Result<Option<Workstream>> {
        Ok(self
            .0
            .query_row(
                "SELECT id, project_id, title, description, lifecycle, visibility, created_at, updated_at
                 FROM workstreams WHERE id = ?1",
                params![id],
                row_workstream,
            )
            .optional()?)
    }

    pub fn upsert_workstream(&self, w: &Workstream) -> Result<()> {
        upsert_workstream_conn(&self.0, w)?;
        self.index_workstream(w)?;
        Ok(())
    }

    pub fn delete_workstream(&self, id: &str) -> Result<()> {
        self.0
            .execute("DELETE FROM workstreams WHERE id = ?1", params![id])?;
        self.unindex("workstream", id);
        Ok(())
    }

    // ---------------- Sessions ----------------

    pub fn upsert_session(&self, s: &Session) -> Result<bool> {
        let existed = self
            .0
            .query_row(
                "SELECT 1 FROM sessions WHERE id = ?1",
                params![s.id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        self.0.execute(
            "INSERT INTO sessions (id, agent, agent_session_id, title, cwd, project_id, raw_path, parent_agent_session_id, started_at, last_activity_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(agent, agent_session_id) DO UPDATE SET
               title = COALESCE(?4, title),
               cwd = COALESCE(?5, cwd),
               raw_path = ?7,
               last_activity_at = COALESCE(?10, last_activity_at),
               started_at = COALESCE(?9, started_at)",
            params![
                s.id, s.agent.as_str(), s.agent_session_id, s.title, s.cwd,
                s.project_id, s.raw_path, s.parent_agent_session_id, s.started_at, s.last_activity_at
            ],
        )?;
        Ok(existed)
    }

    pub fn get_session(&self, id: &str) -> Result<Option<Session>> {
        Ok(self
            .0
            .query_row(
                "SELECT * FROM sessions WHERE id = ?1",
                params![id],
                row_session,
            )
            .optional()?)
    }

    pub fn find_session_by_agent_id(
        &self,
        agent: Agent,
        agent_session_id: &str,
    ) -> Result<Option<Session>> {
        Ok(self
            .0
            .query_row(
                "SELECT * FROM sessions WHERE agent = ?1 AND agent_session_id = ?2",
                params![agent.as_str(), agent_session_id],
                row_session,
            )
            .optional()?)
    }

    /// Sessions discovered but not yet seen by us. Used by LaunchIntent
    /// matching: only genuinely new sessions may claim a pending intent.
    /// The SQL has no created_at column, so the since-filter runs in Rust.
    pub fn recently_created_sessions(&self, agent: Agent, since: &str) -> Result<Vec<Session>> {
        let mut st = self.0.prepare(
            "SELECT * FROM sessions WHERE agent = ?1 ORDER BY COALESCE(started_at, last_activity_at) DESC LIMIT 200",
        )?;
        let rows = st
            .query_map(params![agent.as_str()], row_session)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .filter(|s| {
                s.started_at
                    .as_deref()
                    .or(s.last_activity_at.as_deref())
                    .map(|t| t >= since)
                    .unwrap_or(false)
            })
            .collect())
    }

    pub fn list_sessions(&self, filter: SessionFilter) -> Result<Vec<Session>> {
        // Dynamic SQL: placeholders are appended together with the bind
        // values, so the numbering can never drift out of sync.
        let mut sql = "SELECT * FROM sessions WHERE 1=1".to_string();
        let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(p) = &filter.project_id {
            values.push(Box::new(p.clone()));
            sql.push_str(&format!(" AND project_id = ?{}", values.len()));
        }
        if let Some(a) = &filter.agent {
            values.push(Box::new(a.as_str().to_string()));
            sql.push_str(&format!(" AND agent = ?{}", values.len()));
        }
        sql.push_str(" ORDER BY COALESCE(last_activity_at, started_at) DESC LIMIT 500");
        let mut st = self.0.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::types::ToSql> = values.iter().map(|v| v.as_ref()).collect();
        let rows = st
            .query_map(refs.as_slice(), row_session)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---------------- Events (append-only, identity-based) ----------------

    /// Persist a batch of parsed source events inside one transaction:
    /// identity dedup (INSERT-or-SKIP, never REPLACE), app-owned monotonic
    /// sequence allocation, and the read-cursor advance commit together.
    /// Returns the events that were actually newly stored.
    ///
    /// The identity chain starts from genesis when the batch is a full
    /// re-scan (start_byte_offset == 0); an append continues from the
    /// cursor's `identity_tail_hash` — the tail of the CURRENT source chain,
    /// never "last event in the store" (after a compact + dedup the store
    /// holds newer history the source no longer has) — see
    /// [`event_identity_hash`]. The tail advances with every batch whether
    /// or not rows were new (dedup reproduces the same hash), so the cursor
    /// always describes the source, not the store.
    ///
    /// During full re-scans, fallback events (no native id) additionally try
    /// to CLAIM a pre-chain legacy row of identical content onto the computed
    /// chain position (v4 migration alias): the row keeps its id and gains
    /// the chained identity, so stores with superseded rewrite history
    /// migrate onto the current source chain without duplication.
    pub fn append_source_events(
        &self,
        session_id: &str,
        parsed: &[ParsedEvent],
        source: &SourceCursorUpdate,
        raw_path: &str,
    ) -> Result<Vec<SessionEvent>> {
        self.tx(|tx| {
            let max_seq: i64 = tx.query_row(
                "SELECT COALESCE(MAX(sequence), 0) FROM session_events WHERE session_id = ?1",
                params![session_id],
                |r| r.get::<_, i64>(0),
            )?;
            let mut next_seq: i64 = max_seq + 1;
            let stored_tail: Option<String> = tx
                .query_row(
                    "SELECT identity_tail_hash FROM session_cursors WHERE session_id = ?1",
                    params![session_id],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                .filter(|h| !h.is_empty());
            let mut prev_hash: String = if source.start_byte_offset == 0 {
                IDENTITY_GENESIS.to_string()
            } else {
                match stored_tail.clone() {
                    Some(tail) => tail,
                    // Legacy cursor without a tail: fall back to the last
                    // stored event; the next full re-scan self-heals anyway.
                    None => tx
                        .query_row(
                            "SELECT source_identity_hash FROM session_events
                             WHERE session_id = ?1 ORDER BY sequence DESC LIMIT 1",
                            params![session_id],
                            |r| r.get::<_, String>(0),
                        )
                        .optional()?
                        .unwrap_or_else(|| IDENTITY_GENESIS.to_string()),
                }
            };
            let mut stored = Vec::with_capacity(parsed.len());
            {
                let mut ins = tx.prepare(
                    "INSERT INTO session_events
                     (id, session_id, sequence, source_event_id, source_generation, source_position, source_identity_hash, ts, kind, text, raw_ref, metadata)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                     ON CONFLICT(session_id, source_identity_hash) DO NOTHING",
                )?;
                for p in parsed {
                    // On conflict the computed hash IS the stored one (the
                    // unique index matches on it), so the chain stays
                    // continuous whether or not the row is new.
                    let hash = event_identity_hash(
                        &prev_hash,
                        p.source_event_id.as_deref(),
                        &p.kind,
                        p.ts.as_deref(),
                        p.text.as_deref(),
                    );
                    prev_hash = hash.clone();
                    let id = new_id();
                    let n = if p.source_event_id.is_some() {
                        // Native Agent id: identity is position-independent,
                        // plain insert-or-dedup.
                        ins.execute(params![
                            id,
                            session_id,
                            next_seq,
                            p.source_event_id,
                            source.generation,
                            p.source_position,
                            hash,
                            p.ts,
                            p.kind,
                            p.text,
                            format!("{}#{}", raw_path, p.source_position),
                            p.metadata.to_string(),
                        ])?
                    } else {
                        // Fallback event on a full re-scan: before inserting,
                        // try to CLAIM a pre-chain (legacy) row of the exact
                        // same content onto this chain position. Legacy rows
                        // carry the v4 migration's alias; claiming upgrades
                        // their identity while PRESERVING the event id — and
                        // therefore every SourceReference to it — so a store
                        // with superseded rewrite history never duplicates.
                        let dedup = tx
                            .query_row(
                                "SELECT 1 FROM session_events
                                 WHERE session_id = ?1 AND source_identity_hash = ?2",
                                params![session_id, hash],
                                |_| Ok(()),
                            )
                            .optional()?
                            .is_some();
                        if dedup {
                            0
                        } else {
                            let claimed = tx.execute(
                                "UPDATE session_events
                                 SET source_identity_hash = ?3, legacy_identity_hash = NULL
                                 WHERE session_id = ?1 AND id = (
                                   SELECT id FROM session_events
                                   WHERE session_id = ?1 AND legacy_identity_hash = ?2
                                   ORDER BY sequence LIMIT 1)",
                                params![
                                    session_id,
                                    legacy_fallback_identity_hash(
                                        &p.kind,
                                        p.ts.as_deref(),
                                        p.text.as_deref()
                                    ),
                                    hash,
                                ],
                            )?;
                            if claimed == 0 {
                                ins.execute(params![
                                    id,
                                    session_id,
                                    next_seq,
                                    p.source_event_id,
                                    source.generation,
                                    p.source_position,
                                    hash,
                                    p.ts,
                                    p.kind,
                                    p.text,
                                    format!("{}#{}", raw_path, p.source_position),
                                    p.metadata.to_string(),
                                ])?
                            } else {
                                0
                            }
                        }
                    };
                    if n > 0 {
                        stored.push(SessionEvent {
                            id: id.clone(),
                            session_id: session_id.to_string(),
                            sequence: next_seq,
                            source_event_id: p.source_event_id.clone(),
                            source_generation: source.generation,
                            source_position: p.source_position.clone(),
                            ts: p.ts.clone(),
                            kind: p.kind.clone(),
                            text: p.text.clone(),
                            raw_ref: format!("{}#{}", raw_path, p.source_position),
                            metadata: p.metadata.clone(),
                        });
                        next_seq += 1;
                    }
                }
            }
            // Tail of the source chain after this batch: the last computed
            // hash when the batch had events (inserted or deduped — identical
            // either way), otherwise whatever the cursor already held.
            let new_tail = if parsed.is_empty() {
                stored_tail.unwrap_or_default()
            } else {
                prev_hash
            };
            upsert_source_cursor_conn(
                tx,
                &SourceCursor {
                    session_id: session_id.to_string(),
                    source_file_identity: source.file_identity.clone(),
                    generation: source.generation,
                    byte_offset: source.byte_offset,
                    last_seen_size: source.last_seen_size,
                    mtime: source.mtime,
                    prefix_hash: source.prefix_hash.clone(),
                    identity_tail_hash: new_tail,
                    last_sequence: if stored.is_empty() {
                        // keep previous max; nothing new was appended
                        tx.query_row(
                            "SELECT COALESCE(MAX(sequence), 0) FROM session_events WHERE session_id = ?1",
                            params![session_id],
                            |r| r.get::<_, i64>(0),
                        )?
                    } else {
                        next_seq - 1
                    },
                },
            )?;
            Ok(stored)
        })
    }

    /// Insert fully-formed events as-is (backfill / test seeding only).
    /// The identity unique index still guards against duplicates.
    pub fn append_events(&self, events: &[SessionEvent]) -> Result<()> {
        self.tx(|tx| {
            insert_events_conn(tx, events)?;
            Ok(())
        })
    }

    pub fn get_events(
        &self,
        session_id: &str,
        after: Option<i64>,
        limit: i64,
    ) -> Result<Vec<SessionEvent>> {
        get_events_conn(&self.0, session_id, after, limit)
    }

    pub fn get_event_by_ref(&self, source_ref: &str) -> Result<Option<SessionEvent>> {
        // accepted forms: "session-event:<event-id>" (stable) and the legacy
        // "session:<session-id>#<sequence>" (display-era reference).
        if let Some(id) = source_ref.strip_prefix("session-event:") {
            return self.query_event(
            "SELECT id, session_id, sequence, source_event_id, source_generation, source_position, ts, kind, text, raw_ref, metadata FROM session_events WHERE id = ?1",
            params![id],
        );
        }
        if let Some(rest) = source_ref.strip_prefix("session:") {
            if let Some((sid, seq)) = rest.rsplit_once('#') {
                if let Ok(seq) = seq.parse::<i64>() {
                    return self.query_event(
                        "SELECT id, session_id, sequence, source_event_id, source_generation, source_position, ts, kind, text, raw_ref, metadata FROM session_events WHERE session_id = ?1 AND sequence = ?2",
                        params![sid, seq],
                    );
                }
            }
        }
        Ok(None)
    }

    fn query_event(&self, sql: &str, p: impl rusqlite::Params) -> Result<Option<SessionEvent>> {
        Ok(self.0.query_row(sql, p, row_event).optional()?)
    }

    pub fn event_count(&self, session_id: &str) -> Result<i64> {
        Ok(self.0.query_row(
            "SELECT COUNT(*) FROM session_events WHERE session_id = ?1",
            params![session_id],
            |r| r.get(0),
        )?)
    }

    // ---------------- Cursors ----------------

    pub fn get_source_cursor(&self, session_id: &str) -> Result<SourceCursor> {
        Ok(self
            .0
            .query_row(
                "SELECT session_id, source_file_identity, generation, byte_offset, last_seen_size, mtime, prefix_hash, identity_tail_hash, last_sequence
                 FROM session_cursors WHERE session_id = ?1",
                params![session_id],
                |r| {
                    Ok(SourceCursor {
                        session_id: r.get(0)?,
                        source_file_identity: r.get(1)?,
                        generation: r.get(2)?,
                        byte_offset: r.get::<_, i64>(3)?.max(0) as u64,
                        last_seen_size: r.get::<_, i64>(4)?.max(0) as u64,
                        mtime: r.get(5)?,
                        prefix_hash: r.get(6)?,
                        identity_tail_hash: r.get(7)?,
                        last_sequence: r.get(8)?,
                    })
                },
            )
            .optional()?
            .unwrap_or_default())
    }

    pub fn set_source_cursor(&self, c: &SourceCursor) -> Result<()> {
        self.tx(|tx| upsert_source_cursor_conn(tx, c))
    }

    /// Legacy read accessor used by the UI: max ingested sequence.
    pub fn get_cursor(&self, session_id: &str) -> Result<i64> {
        Ok(self.get_source_cursor(session_id)?.last_sequence)
    }

    pub fn get_processed_sequence(&self, session_id: &str) -> Result<i64> {
        Ok(self
            .0
            .query_row(
                "SELECT processed_sequence FROM session_cursors WHERE session_id = ?1",
                params![session_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0))
    }

    pub fn set_processed_sequence(&self, session_id: &str, seq: i64) -> Result<()> {
        self.0.execute(
            "INSERT INTO session_cursors (session_id, processed_sequence) VALUES (?1, ?2)
             ON CONFLICT(session_id) DO UPDATE SET processed_sequence = ?2",
            params![session_id, seq],
        )?;
        Ok(())
    }

    // ---------------- Bindings ----------------

    pub fn bind(&self, b: &SessionWorkstreamBinding) -> Result<()> {
        bind_conn(&self.0, b)
    }

    pub fn unbind(&self, session_id: &str, workstream_id: &str) -> Result<()> {
        self.0.execute(
            "DELETE FROM session_workstream_bindings WHERE session_id = ?1 AND workstream_id = ?2",
            params![session_id, workstream_id],
        )?;
        Ok(())
    }

    pub fn bindings_for_session(&self, session_id: &str) -> Result<Vec<SessionWorkstreamBinding>> {
        let mut st = self.0.prepare(
            "SELECT session_id, workstream_id, role, source, confidence, last_seen_revision, last_sync_cursor, created_at, last_used_at
             FROM session_workstream_bindings WHERE session_id = ?1",
        )?;
        let rows = st
            .query_map(params![session_id], row_binding)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn bindings_for_workstream(
        &self,
        workstream_id: &str,
    ) -> Result<Vec<SessionWorkstreamBinding>> {
        let mut st = self.0.prepare(
            "SELECT session_id, workstream_id, role, source, confidence, last_seen_revision, last_sync_cursor, created_at, last_used_at
             FROM session_workstream_bindings WHERE workstream_id = ?1",
        )?;
        let rows = st
            .query_map(params![workstream_id], row_binding)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn update_binding_sync(
        &self,
        session_id: &str,
        workstream_id: &str,
        cursor: i64,
        last_seen_revision: Option<&str>,
    ) -> Result<()> {
        self.0.execute(
            "UPDATE session_workstream_bindings
             SET last_sync_cursor = ?3, last_seen_revision = COALESCE(?4, last_seen_revision)
             WHERE session_id = ?1 AND workstream_id = ?2",
            params![session_id, workstream_id, cursor, last_seen_revision],
        )?;
        Ok(())
    }

    // ---------------- Context Items ----------------

    pub fn insert_item(&self, item: &ContextItem, revision: &ContextItemRevision) -> Result<()> {
        insert_item_conn(&self.0, item, revision)?;
        self.index_item(item, revision)
    }

    pub fn insert_revision(&self, r: &ContextItemRevision) -> Result<()> {
        insert_revision_conn(&self.0, r)
    }

    pub fn set_item_head(
        &self,
        item_id: &str,
        revision_id: &str,
        status: Option<&str>,
    ) -> Result<()> {
        if let Some(s) = status {
            self.0.execute(
                "UPDATE context_items SET current_revision_id = ?2, status = ?3, updated_at = ?4 WHERE id = ?1",
                params![item_id, revision_id, s, now()],
            )?;
        } else {
            self.0.execute(
                "UPDATE context_items SET current_revision_id = ?2, updated_at = ?3 WHERE id = ?1",
                params![item_id, revision_id, now()],
            )?;
        }
        Ok(())
    }

    pub fn set_item_status(&self, item_id: &str, status: &str) -> Result<()> {
        self.0.execute(
            "UPDATE context_items SET status = ?2, updated_at = ?3 WHERE id = ?1",
            params![item_id, status, now()],
        )?;
        Ok(())
    }

    /// Status transitions always leave an audit trail: a status-change
    /// revision records previous/new status, actor and reason, and becomes
    /// the item head. Metadata-only; content is preserved.
    pub fn apply_status_change(
        &self,
        item_id: &str,
        new_status: &str,
        actor: &str,
        reason: &str,
        run_id: Option<&str>,
        source_refs: &[String],
    ) -> Result<()> {
        self.tx(|tx| {
            apply_status_change_conn(tx, item_id, new_status, actor, reason, run_id, source_refs)
        })
    }

    pub fn set_item_authority(&self, item_id: &str, authority: &str) -> Result<()> {
        self.0.execute(
            "UPDATE context_items SET authority = ?2, updated_at = ?3 WHERE id = ?1",
            params![item_id, authority, now()],
        )?;
        Ok(())
    }

    pub fn get_item(&self, id: &str) -> Result<Option<ContextItem>> {
        get_item_conn(&self.0, id)
    }

    pub fn get_revision(&self, id: &str) -> Result<Option<ContextItemRevision>> {
        get_revision_conn(&self.0, id)
    }

    pub fn items_for_workstream(
        &self,
        workstream_id: &str,
        include_inactive: bool,
    ) -> Result<Vec<(ContextItem, ContextItemRevision)>> {
        items_for_workstream_conn(&self.0, workstream_id, include_inactive)
    }

    pub fn item_history(&self, item_id: &str) -> Result<Vec<ContextItemRevision>> {
        let mut st = self.0.prepare(
            "SELECT id, item_id, title, content, metadata, source_type, source_ref, sync_run_id, created_at
             FROM context_item_revisions WHERE item_id = ?1 ORDER BY created_at",
        )?;
        let rows = st
            .query_map(params![item_id], row_revision)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn active_items_since(
        &self,
        workstream_id: &str,
        since: &str,
    ) -> Result<Vec<(ContextItem, ContextItemRevision)>> {
        let mut st = self.0.prepare(
            "SELECT i.id, i.workstream_id, i.kind, i.status, i.authority, i.created_by, i.current_revision_id, i.supersedes_item_id, i.created_at, i.updated_at,
                    r.id, r.item_id, r.title, r.content, r.metadata, r.source_type, r.source_ref, r.sync_run_id, r.created_at
             FROM context_items i JOIN context_item_revisions r ON r.id = i.current_revision_id
             WHERE i.workstream_id = ?1 AND i.status = 'active' AND i.updated_at > ?2
             ORDER BY i.updated_at DESC",
        )?;
        let rows = st
            .query_map(params![workstream_id, since], |r| {
                let item = row_item_at(r, 0)?;
                let rev = row_revision_at(r, 10)?;
                Ok((item, rev))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---------------- Conflicts ----------------

    pub fn insert_conflict(&self, c: &ContextConflict) -> Result<()> {
        insert_conflict_conn(&self.0, c)
    }

    pub fn conflicts_for_workstream(
        &self,
        workstream_id: &str,
        include_closed: bool,
    ) -> Result<Vec<ContextConflict>> {
        let filter = if include_closed {
            ""
        } else {
            " AND status = 'open'"
        };
        let sql = format!(
            "SELECT id, workstream_id, left_item_id, right_item_id, conflict_type, status, resolution, created_at, updated_at
             FROM context_conflicts WHERE workstream_id = ?1{} ORDER BY created_at DESC",
            filter
        );
        let mut st = self.0.prepare(&sql)?;
        let rows = st
            .query_map(params![workstream_id], row_conflict)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn open_conflicts_for_item(&self, item_id: &str) -> Result<Vec<ContextConflict>> {
        let mut st = self.0.prepare(
            "SELECT id, workstream_id, left_item_id, right_item_id, conflict_type, status, resolution, created_at, updated_at
             FROM context_conflicts WHERE (left_item_id = ?1 OR right_item_id = ?1) AND status = 'open'",
        )?;
        let rows = st
            .query_map(params![item_id], row_conflict)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn update_conflict_status(
        &self,
        conflict_id: &str,
        status: &str,
        resolution: Option<&str>,
    ) -> Result<()> {
        self.0.execute(
            "UPDATE context_conflicts SET status = ?2, resolution = COALESCE(?3, resolution), updated_at = ?4 WHERE id = ?1",
            params![conflict_id, status, resolution, now()],
        )?;
        Ok(())
    }

    // ---------------- Sync runs ----------------

    pub fn insert_sync_run(&self, run: &SyncRun) -> Result<()> {
        insert_sync_run_conn(&self.0, run)
    }

    /// True when an already-committed run processed exactly this delta —
    /// retries after a crash must not re-apply the same mutations.
    pub fn has_completed_run(&self, session_id: &str, delta_fingerprint: &str) -> Result<bool> {
        Ok(self
            .0
            .query_row(
                "SELECT 1 FROM sync_runs WHERE session_id = ?1 AND delta_fingerprint = ?2 AND status = 'ok' LIMIT 1",
                params![session_id, delta_fingerprint],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn list_sync_runs(&self, limit: i64) -> Result<Vec<SyncRun>> {
        let mut st = self.0.prepare(
            "SELECT id, session_id, from_sequence, to_sequence, status, mutations, summary, error, created_at, runtime, delta_fingerprint, source_generation
             FROM sync_runs ORDER BY created_at DESC LIMIT ?1",
        )?;
        let rows = st
            .query_map(params![limit], row_sync_run)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---------------- Launch intents ----------------

    pub fn insert_launch_intent(&self, i: &LaunchIntent) -> Result<()> {
        self.0.execute(
            "INSERT INTO launch_intents
             (id, launch_type, agent, selected_workstream_ids, cwd, context_bundle_markdown, context_bundle_revisions, process_id, launched_at, matched_session_id, status, note, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                i.id, i.launch_type, i.agent.as_str(),
                serde_json::to_string(&i.selected_workstream_ids)?,
                i.cwd, i.context_bundle_markdown, i.context_bundle_revisions, i.process_id.map(|p| p as i64),
                i.launched_at, i.matched_session_id, i.status, i.note, i.created_at, i.updated_at
            ],
        )?;
        Ok(())
    }

    pub fn update_launch_intent(
        &self,
        id: &str,
        status: &str,
        matched_session_id: Option<&str>,
        note: &str,
    ) -> Result<()> {
        self.0.execute(
            "UPDATE launch_intents SET status = ?2, matched_session_id = COALESCE(?3, matched_session_id), note = ?4, updated_at = ?5 WHERE id = ?1",
            params![id, status, matched_session_id, note, now()],
        )?;
        Ok(())
    }

    pub fn get_launch_intent(&self, id: &str) -> Result<Option<LaunchIntent>> {
        Ok(self
            .0
            .query_row(
                "SELECT id, launch_type, agent, selected_workstream_ids, cwd, context_bundle_markdown, process_id, launched_at, matched_session_id, status, note, created_at, updated_at, context_bundle_revisions
                 FROM launch_intents WHERE id = ?1",
                params![id],
                row_launch_intent,
            )
            .optional()?)
    }

    pub fn list_launch_intents(&self, statuses: &[&str], limit: i64) -> Result<Vec<LaunchIntent>> {
        let filter = if statuses.is_empty() {
            String::new()
        } else {
            let placeholders = statuses.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
            format!(" WHERE status IN ({})", placeholders)
        };
        let sql = format!(
            "SELECT id, launch_type, agent, selected_workstream_ids, cwd, context_bundle_markdown, process_id, launched_at, matched_session_id, status, note, created_at, updated_at, context_bundle_revisions
             FROM launch_intents{} ORDER BY created_at DESC LIMIT {}",
            filter, limit
        );
        let mut st = self.0.prepare(&sql)?;
        let statuses: Vec<String> = statuses.iter().map(|s| s.to_string()).collect();
        let rows = st
            .query_map(
                rusqlite::params_from_iter(statuses.iter()),
                row_launch_intent,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---------------- Project affinity ----------------

    pub fn insert_evidence(&self, e: &ProjectAffinityEvidence) -> Result<()> {
        self.0.execute(
            "INSERT INTO project_affinity_evidence (id, session_id, workstream_id, project_id, evidence_type, source, score, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![e.id, e.session_id, e.workstream_id, e.project_id, e.evidence_type, e.source, e.score, e.created_at],
        )?;
        Ok(())
    }

    pub fn evidence_for_session(&self, session_id: &str) -> Result<Vec<ProjectAffinityEvidence>> {
        let mut st = self.0.prepare(
            "SELECT id, session_id, workstream_id, project_id, evidence_type, source, score, created_at
             FROM project_affinity_evidence WHERE session_id = ?1 ORDER BY created_at DESC LIMIT 100",
        )?;
        let rows = st
            .query_map(params![session_id], row_evidence)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Score the recorded evidence and return the suggested project, if any
    /// evidence exists. Suggestions never auto-assign anything.
    pub fn resolve_project_affinity(&self, session_id: &str) -> Result<Option<(Id, f32)>> {
        let evidence = self.evidence_for_session(session_id)?;
        if evidence.is_empty() {
            return Ok(None);
        }
        let mut scores: std::collections::HashMap<Id, f32> = Default::default();
        for e in evidence {
            *scores.entry(e.project_id).or_default() += e.score;
        }
        let mut ranked: Vec<(Id, f32)> = scores.into_iter().collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(ranked.into_iter().next())
    }

    // ---------------- Context deliveries ----------------

    pub fn record_delivery(&self, d: &ContextDelivery) -> Result<()> {
        self.0.execute(
            "INSERT INTO context_deliveries (id, session_id, workstream_id, bundle_id, delivered_revisions, delivered_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![d.id, d.session_id, d.workstream_id, d.bundle_id, serde_json::to_string(&d.delivered_revisions)?, d.delivered_at],
        )?;
        Ok(())
    }

    /// Latest successful delivery per workstream for a session.
    pub fn latest_deliveries(&self, session_id: &str) -> Result<Vec<ContextDelivery>> {
        let mut st = self.0.prepare(
            "SELECT id, session_id, workstream_id, bundle_id, delivered_revisions, delivered_at
             FROM context_deliveries WHERE session_id = ?1 ORDER BY delivered_at",
        )?;
        let all = st
            .query_map(params![session_id], row_delivery)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut latest: std::collections::HashMap<Id, ContextDelivery> = Default::default();
        for d in all {
            latest.insert(d.workstream_id.clone(), d);
        }
        Ok(latest.into_values().collect())
    }

    // ---------------- Ingest sources ----------------

    pub fn list_ingest_sources(&self) -> Result<Vec<IngestSource>> {
        let mut st = self.0.prepare(
            "SELECT id, agent, path, enabled, origin, created_at FROM ingest_sources ORDER BY agent, path",
        )?;
        let rows = st
            .query_map([], row_ingest_source)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn add_ingest_source(
        &self,
        agent: Agent,
        path: &str,
        enabled: bool,
    ) -> Result<IngestSource> {
        let src = IngestSource {
            id: new_id(),
            agent,
            path: path.to_string(),
            enabled,
            origin: "user".into(),
            created_at: now(),
        };
        self.0
            .execute(
                "INSERT INTO ingest_sources (id, agent, path, enabled, origin, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    src.id,
                    src.agent.as_str(),
                    src.path,
                    src.enabled as i64,
                    src.origin,
                    src.created_at
                ],
            )
            .map_err(|e| {
                if e.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation) {
                    crate::error::other("该数据源已存在")
                } else {
                    crate::error::AppError::from(e)
                }
            })?;
        Ok(src)
    }

    pub fn get_ingest_source(&self, id: &str) -> Result<Option<IngestSource>> {
        Ok(self
            .0
            .query_row(
                "SELECT id, agent, path, enabled, origin, created_at FROM ingest_sources WHERE id = ?1",
                params![id],
                row_ingest_source,
            )
            .optional()?)
    }

    /// Rewind a session's read cursor so the next ingest re-scans the whole
    /// source from the start. The EVENT STORE IS NOT TOUCHED: event rows,
    /// their app-owned ids and every SourceReference pointing at them stay
    /// intact (append-only history). Unchanged content dedups by identity on
    /// the re-scan; only genuinely new/changed source content appends.
    /// The processed (sync) cursor is preserved for the same reason.
    pub fn reset_session_source_cursor(&self, session_id: &str) -> Result<()> {
        self.0.execute(
            "UPDATE session_cursors
             SET byte_offset = 0, last_seen_size = 0, prefix_hash = '', identity_tail_hash = ''
             WHERE session_id = ?1",
            params![session_id],
        )?;
        Ok(())
    }

    pub fn set_ingest_source_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        self.0.execute(
            "UPDATE ingest_sources SET enabled = ?2 WHERE id = ?1",
            params![id, enabled as i64],
        )?;
        Ok(())
    }

    /// Only user-added sources may be removed; defaults are toggled instead.
    pub fn remove_ingest_source(&self, id: &str) -> Result<()> {
        self.0.execute(
            "DELETE FROM ingest_sources WHERE id = ?1 AND origin = 'user'",
            params![id],
        )?;
        Ok(())
    }

    /// The directories reconcile is allowed to scan for this agent.
    pub fn enabled_roots(&self, agent: Agent) -> Result<Vec<String>> {
        let mut st = self.0.prepare(
            "SELECT path FROM ingest_sources WHERE agent = ?1 AND enabled = 1 ORDER BY created_at",
        )?;
        let rows = st
            .query_map(params![agent.as_str()], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---------------- Settings ----------------

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .0
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.0.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = ?2",
            params![key, value],
        )?;
        Ok(())
    }

    // ---------------- Assistant ----------------

    pub fn ensure_assistant_session(&self, session_id: Option<&str>) -> Result<String> {
        if let Some(id) = session_id {
            let exists = self
                .0
                .query_row(
                    "SELECT 1 FROM assistant_sessions WHERE id = ?1",
                    params![id],
                    |_| Ok(()),
                )
                .optional()?;
            if exists.is_some() {
                return Ok(id.to_string());
            }
        }
        let id = new_id();
        self.0.execute(
            "INSERT INTO assistant_sessions (id, title, created_at) VALUES (?1, ?2, ?3)",
            params![id, "Assistant", now()],
        )?;
        Ok(id)
    }

    #[allow(clippy::type_complexity)]
    pub fn insert_assistant_message(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        action_json: Option<&str>,
        runtime: Option<&str>,
    ) -> Result<(String, String)> {
        let id = new_id();
        let ts = now();
        self.0.execute(
            "INSERT INTO assistant_messages (id, session_id, role, content, action_json, runtime, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, session_id, role, content, action_json, runtime, ts],
        )?;
        Ok((id, ts))
    }

    pub fn list_assistant_messages(
        &self,
        session_id: &str,
        limit: i64,
    ) -> Result<Vec<AssistantMessageRow>> {
        let mut st = self.0.prepare(
            "SELECT id, session_id, role, content, action_json, runtime, created_at
             FROM assistant_messages WHERE session_id = ?1 ORDER BY created_at LIMIT ?2",
        )?;
        let rows = st
            .query_map(params![session_id, limit], |r| {
                Ok(AssistantMessageRow {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    role: r.get(2)?,
                    content: r.get(3)?,
                    action_json: r.get(4)?,
                    runtime: r.get(5)?,
                    created_at: r.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---------------- Agent installations cache ----------------

    pub fn save_installation(
        &self,
        i: &crate::platform::exec_resolver::AgentInstallation,
    ) -> Result<()> {
        self.0.execute(
            "INSERT INTO agent_installations (agent, executable_path, version, source, last_verified_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(agent) DO UPDATE SET executable_path = ?2, version = ?3, source = ?4, last_verified_at = ?5",
            params![i.agent.as_str(), i.executable_path, i.version, i.source, i.last_verified_at],
        )?;
        Ok(())
    }

    pub fn get_installation(
        &self,
        agent: Agent,
    ) -> Result<Option<crate::platform::exec_resolver::AgentInstallation>> {
        Ok(self
            .0
            .query_row(
                "SELECT agent, executable_path, version, source, last_verified_at
                 FROM agent_installations WHERE agent = ?1",
                params![agent.as_str()],
                |r| {
                    Ok(crate::platform::exec_resolver::AgentInstallation {
                        agent: Agent::parse(&r.get::<_, String>(0)?).unwrap_or(Agent::Codex),
                        executable_path: r.get(1)?,
                        version: r.get(2)?,
                        source: r.get(3)?,
                        last_verified_at: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    // ---------------- FTS ----------------

    pub fn fts_available(&self) -> bool {
        self.0
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE name = 'search_index'",
                [],
                |_| Ok(()),
            )
            .optional()
            .unwrap_or(None)
            .is_some()
    }

    pub fn unindex(&self, kind: &str, ref_id: &str) {
        let _ = self.0.execute(
            "DELETE FROM search_index WHERE kind = ?1 AND ref_id = ?2",
            params![kind, ref_id],
        );
    }

    pub fn index_project(&self, p: &Project) -> Result<()> {
        self.unindex("project", &p.id);
        self.0.execute(
            "INSERT INTO search_index (kind, ref_id, parent_id, title, body) VALUES ('project', ?1, '', ?2, ?3)",
            params![p.id, p.name, p.description],
        )?;
        Ok(())
    }

    pub fn index_workstream(&self, w: &Workstream) -> Result<()> {
        self.unindex("workstream", &w.id);
        self.0.execute(
            "INSERT INTO search_index (kind, ref_id, parent_id, title, body) VALUES ('workstream', ?1, ?2, ?3, ?4)",
            params![w.id, w.project_id, w.title, w.description],
        )?;
        Ok(())
    }

    pub fn index_item(&self, item: &ContextItem, rev: &ContextItemRevision) -> Result<()> {
        self.unindex("item", &item.id);
        self.0.execute(
            "INSERT INTO search_index (kind, ref_id, parent_id, title, body) VALUES ('item', ?1, ?2, ?3, ?4)",
            params![item.id, item.workstream_id, rev.title, rev.content],
        )?;
        Ok(())
    }

    /// Index newly stored events. Insert-only per event ref: because event
    /// identity dedup happens at the event layer, incremental batches never
    /// need to wipe the session's earlier index rows.
    pub fn index_new_events(&self, events: &[SessionEvent]) -> Result<()> {
        for e in events {
            self.unindex("event", &format!("{}:{}", e.session_id, e.sequence));
        }
        let mut st = self.0.prepare(
            "INSERT INTO search_index (kind, ref_id, parent_id, title, body) VALUES ('event', ?1, ?2, ?3, ?4)",
        )?;
        for e in events {
            if let Some(t) = &e.text {
                if t.len() < 20 {
                    continue;
                }
                st.execute(params![
                    format!("{}:{}", e.session_id, e.sequence),
                    e.session_id,
                    "",
                    t
                ])?;
            }
        }
        Ok(())
    }

    /// One-time backfill so events ingested before the index existed (or
    /// failed to index) become searchable. Idempotent.
    pub fn backfill_search_index(&self) -> Result<()> {
        if !self.fts_available() {
            return Ok(());
        }
        self.0.execute(
            "INSERT INTO search_index (kind, ref_id, parent_id, title, body)
             SELECT 'event', session_id || ':' || sequence, session_id, '', text
             FROM session_events
             WHERE length(COALESCE(text, '')) >= 20
               AND session_id || ':' || sequence NOT IN (
                   SELECT ref_id FROM search_index WHERE kind = 'event')",
            [],
        )?;
        Ok(())
    }

    // ---------------- helpers ----------------

    pub fn touch_project(&self, project_id: &str) -> Result<()> {
        self.0.execute(
            "UPDATE projects SET updated_at = ?2 WHERE id = ?1",
            params![project_id, now()],
        )?;
        Ok(())
    }

    pub fn stats(&self) -> Result<serde_json::Value> {
        let one = |sql: &str| -> i64 { self.0.query_row(sql, [], |r| r.get(0)).unwrap_or(0) };
        Ok(serde_json::json!({
            "projects": one("SELECT COUNT(*) FROM projects WHERE archived = 0"),
            "workstreams": one("SELECT COUNT(*) FROM workstreams"),
            "sessions": one("SELECT COUNT(*) FROM sessions"),
            "events": one("SELECT COUNT(*) FROM session_events"),
            "context_items": one("SELECT COUNT(*) FROM context_items WHERE status = 'active'"),
            "sync_runs": one("SELECT COUNT(*) FROM sync_runs"),
            "open_conflicts": one("SELECT COUNT(*) FROM context_conflicts WHERE status = 'open'"),
            "pending_launch_intents": one("SELECT COUNT(*) FROM launch_intents WHERE status = 'pending'"),
        }))
    }
}

// connection-level helpers --------------------------------------------------
// Free functions over &Connection so Db methods and `Db::tx` closures share
// exactly the same SQL paths.

pub fn upsert_project_conn(conn: &Connection, p: &Project) -> Result<()> {
    conn.execute(
        "INSERT INTO projects (id, name, description, archived, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(id) DO UPDATE SET name = ?2, description = ?3, archived = ?4, updated_at = ?6",
        params![
            p.id,
            p.name,
            p.description,
            p.archived as i64,
            p.created_at,
            p.updated_at
        ],
    )?;
    Ok(())
}

pub fn upsert_workstream_conn(conn: &Connection, w: &Workstream) -> Result<()> {
    conn.execute(
        "INSERT INTO workstreams (id, project_id, title, description, lifecycle, visibility, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET
           project_id = ?2,
           title = ?3, description = ?4, lifecycle = ?5, visibility = ?6, updated_at = ?8",
        params![w.id, w.project_id, w.title, w.description, w.lifecycle, w.visibility, w.created_at, w.updated_at],
    )?;
    Ok(())
}

pub fn bind_conn(conn: &Connection, b: &SessionWorkstreamBinding) -> Result<()> {
    // Binding sources have an explicit PRECEDENCE, not just confidence:
    // explicit launch selection (3) > user_assigned (2) > automatic (1).
    // A later binding replaces the stored provenance only when it outranks
    // it, or matches it in rank with >= confidence — so a user_assigned
    // resume can never rewrite an explicit_launch_selection into a weaker
    // provenance even at equal confidence, and role changes only when the
    // binding itself is replaced.
    conn.execute(
        "INSERT INTO session_workstream_bindings
         (session_id, workstream_id, role, source, confidence, last_seen_revision, last_sync_cursor, created_at, last_used_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(session_id, workstream_id) DO UPDATE SET
           source = CASE WHEN binding_wins(?4, ?5, session_workstream_bindings.source, session_workstream_bindings.confidence)
                         THEN ?4 ELSE session_workstream_bindings.source END,
           confidence = CASE WHEN binding_wins(?4, ?5, session_workstream_bindings.source, session_workstream_bindings.confidence)
                             THEN ?5 ELSE session_workstream_bindings.confidence END,
           role = CASE WHEN binding_wins(?4, ?5, session_workstream_bindings.source, session_workstream_bindings.confidence)
                       THEN ?3 ELSE session_workstream_bindings.role END,
           last_seen_revision = COALESCE(?6, last_seen_revision),
           last_used_at = ?9",
        params![
            b.session_id, b.workstream_id, b.role, b.source, b.confidence,
            b.last_seen_revision, b.last_sync_cursor, b.created_at, b.last_used_at
        ],
    )?;
    Ok(())
}

/// Register the precedence helper used by bind_conn (deterministic SQL
/// expression of the binding-source ranking).
fn register_binding_rank_fn(conn: &Connection) -> Result<()> {
    conn.create_scalar_function(
        "binding_wins",
        4,
        rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            fn rank(source: &str) -> i64 {
                match source {
                    "explicit_launch_selection" => 3,
                    "user_assigned" => 2,
                    _ => 1,
                }
            }
            let new_source = ctx.get_raw(0).as_str().unwrap_or("").to_string();
            let new_conf: f64 = ctx.get(1)?;
            let old_source = ctx.get_raw(2).as_str().unwrap_or("").to_string();
            let old_conf: f64 = ctx.get(3)?;
            let (rn, ro) = (rank(&new_source), rank(&old_source));
            Ok(rn > ro || (rn == ro && new_conf >= old_conf))
        },
    )?;
    Ok(())
}

pub fn insert_events_conn(conn: &Connection, events: &[SessionEvent]) -> Result<()> {
    let mut ins = conn.prepare(
        "INSERT INTO session_events
         (id, session_id, sequence, source_event_id, source_generation, source_position, source_identity_hash, ts, kind, text, raw_ref, metadata)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(session_id, source_identity_hash) DO NOTHING",
    )?;
    // Test-seeding twin of append_source_events' chain: events are seeded in
    // file order, so the chain restarts from genesis.
    let mut prev_hash = IDENTITY_GENESIS.to_string();
    for e in events {
        let hash = event_identity_hash(
            &prev_hash,
            e.source_event_id.as_deref(),
            &e.kind,
            e.ts.as_deref(),
            e.text.as_deref(),
        );
        prev_hash = hash.clone();
        ins.execute(params![
            e.id,
            e.session_id,
            e.sequence,
            e.source_event_id,
            e.source_generation,
            e.source_position,
            hash,
            e.ts,
            e.kind,
            e.text,
            e.raw_ref,
            e.metadata.to_string()
        ])?;
    }
    Ok(())
}

pub fn upsert_source_cursor_conn(conn: &Connection, c: &SourceCursor) -> Result<()> {
    conn.execute(
        "INSERT INTO session_cursors
         (session_id, last_sequence, last_seen_size, source_file_identity, generation, byte_offset, mtime, prefix_hash, identity_tail_hash, processed_sequence)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                 COALESCE((SELECT processed_sequence FROM session_cursors WHERE session_id = ?1), 0))
         ON CONFLICT(session_id) DO UPDATE SET
           last_sequence = ?2, last_seen_size = ?3, source_file_identity = ?4,
           generation = ?5, byte_offset = ?6, mtime = ?7, prefix_hash = ?8, identity_tail_hash = ?9",
        params![c.session_id, c.last_sequence, c.last_seen_size as i64, c.source_file_identity, c.generation, c.byte_offset as i64, c.mtime, c.prefix_hash, c.identity_tail_hash],
    )?;
    Ok(())
}

pub fn get_events_conn(
    conn: &Connection,
    session_id: &str,
    after: Option<i64>,
    limit: i64,
) -> Result<Vec<SessionEvent>> {
    let mut st = conn.prepare(
        "SELECT id, session_id, sequence, source_event_id, source_generation, source_position, ts, kind, text, raw_ref, metadata
         FROM session_events WHERE session_id = ?1 AND sequence > ?2
         ORDER BY sequence LIMIT ?3",
    )?;
    let rows = st
        .query_map(params![session_id, after.unwrap_or(0), limit], row_event)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn insert_item_conn(
    conn: &Connection,
    item: &ContextItem,
    revision: &ContextItemRevision,
) -> Result<()> {
    conn.execute(
        "INSERT INTO context_items (id, workstream_id, kind, status, authority, created_by, current_revision_id, supersedes_item_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![item.id, item.workstream_id, item.kind, item.status, item.authority, item.created_by,
                item.current_revision_id, item.supersedes_item_id, item.created_at, item.updated_at],
    )?;
    insert_revision_conn(conn, revision)?;
    conn.execute(
        "UPDATE context_items SET current_revision_id = ?2 WHERE id = ?1",
        params![item.id, revision.id],
    )?;
    Ok(())
}

pub fn insert_revision_conn(conn: &Connection, r: &ContextItemRevision) -> Result<()> {
    conn.execute(
        "INSERT INTO context_item_revisions (id, item_id, title, content, metadata, source_type, source_ref, sync_run_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![r.id, r.item_id, r.title, r.content, r.metadata.to_string(), r.source_type, r.source_ref, r.sync_run_id, r.created_at],
    )?;
    Ok(())
}

pub fn insert_sync_run_conn(conn: &Connection, run: &SyncRun) -> Result<()> {
    conn.execute(
        "INSERT INTO sync_runs (id, session_id, from_sequence, to_sequence, status, mutations, summary, error, created_at, runtime, delta_fingerprint, source_generation)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![run.id, run.session_id, run.from_sequence, run.to_sequence, run.status,
                run.mutations.to_string(), run.summary, run.error, run.created_at, run.runtime,
                run.delta_fingerprint, run.source_generation],
    )?;
    Ok(())
}

pub fn set_processed_sequence_conn(conn: &Connection, session_id: &str, seq: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO session_cursors (session_id, processed_sequence) VALUES (?1, ?2)
         ON CONFLICT(session_id) DO UPDATE SET processed_sequence = ?2",
        params![session_id, seq],
    )?;
    Ok(())
}

/// Status change with a full audit revision: previous status, new status,
/// actor, reason and source refs all land in the revision metadata, and the
/// revision becomes the item head (content preserved).
pub fn apply_status_change_conn(
    conn: &Connection,
    item_id: &str,
    new_status: &str,
    actor: &str,
    reason: &str,
    run_id: Option<&str>,
    source_refs: &[String],
) -> Result<()> {
    let (item, rev) = {
        let cur: Option<(String, String)> = conn
            .query_row(
                "SELECT status, COALESCE(current_revision_id, '') FROM context_items WHERE id = ?1",
                params![item_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (status, head) = cur.ok_or_else(|| other(format!("条目不存在: {}", item_id)))?;
        let content: Option<(String, String)> = if head.is_empty() {
            None
        } else {
            conn.query_row(
                "SELECT title, content FROM context_item_revisions WHERE id = ?1",
                params![head],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
        };
        let (title, content) = content.unwrap_or_else(|| ("(无标题)".into(), String::new()));
        (status, (title, content))
    };
    let audit = ContextItemRevision {
        id: new_id(),
        item_id: item_id.to_string(),
        title: rev.0,
        content: rev.1,
        metadata: serde_json::json!({
            "audit": {
                "previous_status": item,
                "new_status": new_status,
                "actor": actor,
                "reason": reason,
                "source_refs": source_refs,
            }
        }),
        source_type: Some("status_change".into()),
        source_ref: source_refs.first().cloned(),
        sync_run_id: run_id.map(|s| s.to_string()),
        created_at: now(),
    };
    insert_revision_conn(conn, &audit)?;
    conn.execute(
        "UPDATE context_items SET status = ?2, current_revision_id = ?3, updated_at = ?4 WHERE id = ?1",
        params![item_id, new_status, audit.id, now()],
    )?;
    Ok(())
}

pub fn get_item_conn(conn: &Connection, id: &str) -> Result<Option<ContextItem>> {
    Ok(conn
        .query_row(
            "SELECT id, workstream_id, kind, status, authority, created_by, current_revision_id, supersedes_item_id, created_at, updated_at
             FROM context_items WHERE id = ?1",
            params![id],
            row_item,
        )
        .optional()?)
}

pub fn get_revision_conn(conn: &Connection, id: &str) -> Result<Option<ContextItemRevision>> {
    Ok(conn
        .query_row(
            "SELECT id, item_id, title, content, metadata, source_type, source_ref, sync_run_id, created_at
             FROM context_item_revisions WHERE id = ?1",
            params![id],
            row_revision,
        )
        .optional()?)
}

pub fn items_for_workstream_conn(
    conn: &Connection,
    workstream_id: &str,
    include_inactive: bool,
) -> Result<Vec<(ContextItem, ContextItemRevision)>> {
    let status_filter = if include_inactive {
        ""
    } else {
        " AND i.status = 'active'"
    };
    let sql = format!(
        "SELECT i.id, i.workstream_id, i.kind, i.status, i.authority, i.created_by, i.current_revision_id, i.supersedes_item_id, i.created_at, i.updated_at,
                r.id, r.item_id, r.title, r.content, r.metadata, r.source_type, r.source_ref, r.sync_run_id, r.created_at
         FROM context_items i LEFT JOIN context_item_revisions r ON r.id = i.current_revision_id
         WHERE i.workstream_id = ?1{} ORDER BY i.updated_at DESC",
        status_filter
    );
    let mut st = conn.prepare(&sql)?;
    let rows = st
        .query_map(params![workstream_id], |r| {
            let item = row_item_at(r, 0)?;
            let rev = row_revision_at(r, 10)?;
            Ok((item, rev))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn set_item_head_conn(
    conn: &Connection,
    item_id: &str,
    revision_id: &str,
    status: Option<&str>,
) -> Result<()> {
    if let Some(s) = status {
        conn.execute(
            "UPDATE context_items SET current_revision_id = ?2, status = ?3, updated_at = ?4 WHERE id = ?1",
            params![item_id, revision_id, s, now()],
        )?;
    } else {
        conn.execute(
            "UPDATE context_items SET current_revision_id = ?2, updated_at = ?3 WHERE id = ?1",
            params![item_id, revision_id, now()],
        )?;
    }
    Ok(())
}

pub fn insert_conflict_conn(conn: &Connection, c: &ContextConflict) -> Result<()> {
    conn.execute(
        "INSERT INTO context_conflicts (id, workstream_id, left_item_id, right_item_id, conflict_type, status, resolution, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![c.id, c.workstream_id, c.left_item_id, c.right_item_id, c.conflict_type, c.status, c.resolution, c.created_at, c.updated_at],
    )?;
    Ok(())
}

// row mappers -------------------------------------------------------------

fn row_project(r: &Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: r.get(0)?,
        name: r.get(1)?,
        description: r.get(2)?,
        archived: r.get::<_, i64>(3)? != 0,
        created_at: r.get(4)?,
        updated_at: r.get(5)?,
    })
}

fn row_workstream(r: &Row) -> rusqlite::Result<Workstream> {
    Ok(Workstream {
        id: r.get(0)?,
        project_id: r.get(1)?,
        title: r.get(2)?,
        description: r.get(3)?,
        lifecycle: r.get(4)?,
        visibility: r.get(5)?,
        created_at: r.get(6)?,
        updated_at: r.get(7)?,
    })
}

fn row_session(r: &Row) -> rusqlite::Result<Session> {
    Ok(Session {
        id: r.get("id")?,
        agent: Agent::parse(&r.get::<_, String>("agent")?).unwrap_or(Agent::Codex),
        agent_session_id: r.get("agent_session_id")?,
        title: r.get("title")?,
        cwd: r.get("cwd")?,
        project_id: r.get("project_id")?,
        raw_path: r.get("raw_path")?,
        parent_agent_session_id: r.get("parent_agent_session_id")?,
        started_at: r.get("started_at")?,
        last_activity_at: r.get("last_activity_at")?,
    })
}

fn row_event(r: &Row) -> rusqlite::Result<SessionEvent> {
    Ok(SessionEvent {
        id: r.get(0)?,
        session_id: r.get(1)?,
        sequence: r.get(2)?,
        source_event_id: r.get(3)?,
        source_generation: r.get(4)?,
        source_position: r.get(5)?,
        ts: r.get(6)?,
        kind: r.get(7)?,
        text: r.get(8)?,
        raw_ref: r.get(9)?,
        metadata: serde_json::from_str(&r.get::<_, String>(10)?).unwrap_or_default(),
    })
}

fn row_binding(r: &Row) -> rusqlite::Result<SessionWorkstreamBinding> {
    Ok(SessionWorkstreamBinding {
        session_id: r.get(0)?,
        workstream_id: r.get(1)?,
        role: r.get(2)?,
        source: r.get(3)?,
        confidence: r.get(4)?,
        last_seen_revision: r.get(5)?,
        last_sync_cursor: r.get(6)?,
        created_at: r.get(7)?,
        last_used_at: r.get(8)?,
    })
}

fn row_item_at(r: &Row, base: usize) -> rusqlite::Result<ContextItem> {
    Ok(ContextItem {
        id: r.get(base + 0)?,
        workstream_id: r.get(base + 1)?,
        kind: r.get(base + 2)?,
        status: r.get(base + 3)?,
        authority: r.get(base + 4)?,
        created_by: r.get(base + 5)?,
        current_revision_id: r.get(base + 6)?,
        supersedes_item_id: r.get(base + 7)?,
        created_at: r.get(base + 8)?,
        updated_at: r.get(base + 9)?,
    })
}

fn row_item(r: &Row) -> rusqlite::Result<ContextItem> {
    row_item_at(r, 0)
}

fn row_revision_at(r: &Row, base: usize) -> rusqlite::Result<ContextItemRevision> {
    Ok(ContextItemRevision {
        id: r.get(base + 0)?,
        item_id: r.get(base + 1)?,
        title: r.get(base + 2).unwrap_or_default(),
        content: r.get(base + 3).unwrap_or_default(),
        metadata: serde_json::from_str(&r.get::<_, String>(base + 4).unwrap_or_default())
            .unwrap_or_default(),
        source_type: r.get(base + 5)?,
        source_ref: r.get(base + 6)?,
        sync_run_id: r.get(base + 7)?,
        created_at: r.get(base + 8)?,
    })
}

fn row_revision(r: &Row) -> rusqlite::Result<ContextItemRevision> {
    row_revision_at(r, 0)
}

fn row_conflict(r: &Row) -> rusqlite::Result<ContextConflict> {
    Ok(ContextConflict {
        id: r.get(0)?,
        workstream_id: r.get(1)?,
        left_item_id: r.get(2)?,
        right_item_id: r.get(3)?,
        conflict_type: r.get(4)?,
        status: r.get(5)?,
        resolution: r.get(6)?,
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
    })
}

fn row_ingest_source(r: &Row) -> rusqlite::Result<IngestSource> {
    Ok(IngestSource {
        id: r.get(0)?,
        agent: Agent::parse(&r.get::<_, String>(1)?).unwrap_or(Agent::Codex),
        path: r.get(2)?,
        enabled: r.get::<_, i64>(3)? != 0,
        origin: r.get(4)?,
        created_at: r.get(5)?,
    })
}

fn row_launch_intent(r: &Row) -> rusqlite::Result<LaunchIntent> {
    Ok(LaunchIntent {
        id: r.get(0)?,
        launch_type: r.get(1)?,
        agent: Agent::parse(&r.get::<_, String>(2)?).unwrap_or(Agent::Codex),
        selected_workstream_ids: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or_default(),
        cwd: r.get(4)?,
        context_bundle_markdown: r.get(5)?,
        process_id: r.get::<_, Option<i64>>(6)?.map(|p| p as u32),
        launched_at: r.get(7)?,
        matched_session_id: r.get(8)?,
        status: r.get(9)?,
        note: r.get(10)?,
        created_at: r.get(11)?,
        updated_at: r.get(12)?,
        context_bundle_revisions: r.get(13)?,
    })
}

fn row_evidence(r: &Row) -> rusqlite::Result<ProjectAffinityEvidence> {
    Ok(ProjectAffinityEvidence {
        id: r.get(0)?,
        session_id: r.get(1)?,
        workstream_id: r.get(2)?,
        project_id: r.get(3)?,
        evidence_type: r.get(4)?,
        source: r.get(5)?,
        score: r.get(6)?,
        created_at: r.get(7)?,
    })
}

fn row_delivery(r: &Row) -> rusqlite::Result<ContextDelivery> {
    Ok(ContextDelivery {
        id: r.get(0)?,
        session_id: r.get(1)?,
        workstream_id: r.get(2)?,
        bundle_id: r.get(3)?,
        delivered_revisions: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default(),
        delivered_at: r.get(5)?,
    })
}

fn row_sync_run(r: &Row) -> rusqlite::Result<SyncRun> {
    Ok(SyncRun {
        id: r.get(0)?,
        session_id: r.get(1)?,
        from_sequence: r.get(2)?,
        to_sequence: r.get(3)?,
        status: r.get(4)?,
        mutations: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or_default(),
        summary: r.get(6)?,
        error: r.get(7)?,
        created_at: r.get(8)?,
        runtime: r
            .get::<_, Option<String>>(9)?
            .unwrap_or_else(|| "heuristic".into()),
        delta_fingerprint: r.get(10)?,
        source_generation: r.get::<_, Option<i64>>(11)?.unwrap_or(0),
    })
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AssistantMessageRow {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub action_json: Option<String>,
    pub runtime: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Default, Clone)]
pub struct SessionFilter {
    pub project_id: Option<String>,
    pub agent: Option<Agent>,
}

pub fn ensure_not_empty(name: &str, v: &str) -> Result<()> {
    if v.trim().is_empty() {
        Err(other(format!("{} 不能为空", name)))
    } else {
        Ok(())
    }
}
