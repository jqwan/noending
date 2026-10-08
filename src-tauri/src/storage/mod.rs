//! SQLite storage: current schema and repository helpers. Owns domain data, the
//! Logical Session graph (one row per session: identity, source, read cursor
//! and fact frontier), context items and FTS search.
//!
//! History integrity: `session_messages` is the ONLY conversation store,
//! append-only and idempotent by `(session_id, source_identity_hash)` — rows
//! are never replaced. The session row carries the *read* cursor, while
//! `session_contexts.processed_through_seq` is the separate *processed*
//! position of Context consumption. One ingest writes messages + projection +
//! cursor + fact frontier in ONE transaction (`commit_ingest`).

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};

use crate::domain::*;
use crate::error::{other, Result};

// The WorkspacePath registry, ordered WorkstreamPath list and Session→path
// attach each live in their own impl file rather than growing this monolith.
pub mod context_repo;
pub mod schema;
pub mod session_lifecycle;
pub mod session_paths;
pub mod workspace;
pub mod workstream_paths;

pub use context_repo::{
    apply_projection_conn, bump_context_revision_conn, bump_input_revision_conn,
    commit_session_context_conn, consume_input_revision_conn, delete_workstream_frontier_conn,
    frontiers_for_workstream_conn, get_ingest_state_conn, get_session_context_conn,
    get_workstream_context_state_conn, message_ids_for_hashes_conn, projection_ids_conn,
    set_workstream_frontier_conn, ProjectionOutcome,
};

pub use schema::{DATABASE_APPLICATION_ID, DATABASE_FORMAT_VERSION};
pub use session_lifecycle::PermanentDeletionCounts;

/// Two connections to one SQLite file so UI reads never queue behind writes:
/// WAL allows one writer plus concurrent readers, every mutation goes through
/// `writer` (serialized by its mutex), queries through `reader`. Callers never
/// see a database lock — there is no `Mutex<Db>` to hold across a file scan.
pub struct Db {
    writer: Mutex<Connection>,
    reader: Mutex<Connection>,
}

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn later_timestamp(current: Option<&str>, observed: Option<&str>) -> Option<String> {
    match (current, observed) {
        (None, None) => None,
        (Some(v), None) => Some(v.to_string()),
        (None, Some(v)) => Some(v.to_string()),
        (Some(a), Some(b)) => {
            let ordering = match (
                chrono::DateTime::parse_from_rfc3339(a),
                chrono::DateTime::parse_from_rfc3339(b),
            ) {
                (Ok(a), Ok(b)) => a.cmp(&b),
                _ => a.cmp(b),
            };
            Some(if ordering.is_lt() { b } else { a }.to_string())
        }
    }
}

fn mtime_timestamp(mtime: Option<f64>) -> Option<String> {
    let mtime = mtime?;
    let seconds = mtime.floor() as i64;
    let nanos = ((mtime - seconds as f64) * 1_000_000_000.0).clamp(0.0, 999_999_999.0) as u32;
    chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, nanos).map(|t| t.to_rfc3339())
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Identity of the first message in a chain (no known predecessor).
pub const IDENTITY_GENESIS: &str = "genesis";

/// Stable identity of a ROOT conversation message, used to make re-scans of the
/// same Agent source idempotent.
///
/// - WITH a native Agent message id: content hash of
///   `(native_id | role | ts | content)`, so identity is position-independent.
/// - WITHOUT one: chained hash `H(prev_hash | role | ts | content)`. The chain
///   encodes *adjacency*: an unchanged prefix reproduces the chain (dedup),
///   while two identical turns under different predecessors stay distinct. A
///   mid-file rewrite diverges the chain exactly where content changed.
pub fn message_identity_hash(
    prev_hash: &str,
    source_message_id: Option<&str>,
    role: &str,
    ts: Option<&str>,
    content: &str,
) -> String {
    use sha2::Digest;
    use std::fmt::Write;
    let normalized = content.trim();
    let mut h = sha2::Sha256::new();
    match source_message_id {
        Some(id) => h.update(id.as_bytes()),
        None => h.update(prev_hash.as_bytes()),
    }
    h.update([0x1f]);
    h.update(role);
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

/// Title update policy, applied in the conflict branch below: the Agent app's
/// own (native) title may keep renaming the session, so it writes through;
/// the text-derived fallback is fixed at the conversation's beginning, so it
/// only ever fills an EMPTY slot — it never overwrites anything.
#[allow(clippy::too_many_arguments)]
fn upsert_logical_session_conn(
    conn: &Connection,
    agent: Agent,
    root_agent_session_id: &str,
    native_title: Option<&str>,
    title: Option<&str>,
    cwd: Option<&str>,
    workspace_path_id: Option<&str>,
    forked_from_session_id: Option<&str>,
    started_at: Option<&str>,
    last_activity_at: Option<&str>,
    source_kind: &str,
    source_path: &str,
    metadata: &serde_json::Value,
) -> Result<(String, bool)> {
    let existing: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT id, last_activity_at FROM sessions
             WHERE agent = ?1 AND root_agent_session_id = ?2",
            params![agent.as_str(), root_agent_session_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((id, previous_activity)) = existing {
        let activity = later_timestamp(previous_activity.as_deref(), last_activity_at);
        conn.execute(
            "UPDATE sessions SET
               title = COALESCE(?2, title, ?3),
               cwd = COALESCE(?4, cwd),
               workspace_path_id = COALESCE(?5, workspace_path_id),
               project_id = COALESCE(
                 (SELECT wp.project_id FROM workspace_paths wp
                   WHERE wp.id = COALESCE(?5, workspace_path_id)),
                 (SELECT wp.project_id FROM workspace_paths wp
                   WHERE wp.id = workspace_path_id)),
               forked_from_session_id = COALESCE(forked_from_session_id, ?6),
               started_at = COALESCE(started_at, ?7),
               last_activity_at = ?8,
               source_kind = ?9, source_path = ?10, metadata = ?11
             WHERE id = ?1",
            params![
                id,
                native_title,
                title,
                cwd,
                workspace_path_id,
                forked_from_session_id,
                started_at,
                activity,
                source_kind,
                source_path,
                metadata.to_string()
            ],
        )?;
        index_session_conn(conn, &id)?;
        return Ok((id, false));
    }
    let id = new_id();
    conn.execute(
        "INSERT INTO sessions
           (id, agent, root_agent_session_id, title, cwd, workspace_path_id, project_id,
            forked_from_session_id, started_at, last_activity_at,
            source_kind, source_path, metadata)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6,
                 (SELECT wp.project_id FROM workspace_paths wp WHERE wp.id = ?6),
                 ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            id,
            agent.as_str(),
            root_agent_session_id,
            native_title.or(title),
            cwd,
            workspace_path_id,
            forked_from_session_id,
            started_at,
            last_activity_at,
            source_kind,
            source_path,
            metadata.to_string()
        ],
    )?;
    index_session_conn(conn, &id)?;
    Ok((id, true))
}

impl Db {
    pub fn open(path: &std::path::Path) -> Result<Db> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let writer = Connection::open(path)?;
        // Connection-local settings first, then the format gate: nothing that
        // outlives the connection may touch the file until we know it is ours.
        // `journal_mode = WAL` is a header change, so it comes after the gate —
        // a refused database is left exactly as it was found.
        writer.pragma_update(None, "foreign_keys", "ON")?;
        writer.pragma_update(None, "synchronous", "NORMAL")?;
        writer.busy_timeout(std::time::Duration::from_secs(5))?;
        Self::initialize_schema(&writer)?;
        writer.pragma_update(None, "journal_mode", "WAL")?;

        // Opened only once the file is known to be ours.
        let reader = Connection::open(path)?;
        reader.pragma_update(None, "foreign_keys", "ON")?;
        reader.pragma_update(None, "synchronous", "NORMAL")?;
        // A generous busy timeout: the reader never competes with the writer
        // under WAL, but an external process touching the file should cause a
        // wait, not an error.
        reader.busy_timeout(std::time::Duration::from_secs(5))?;

        Ok(Db {
            writer: Mutex::new(writer),
            reader: Mutex::new(reader),
        })
    }

    /// A WAL snapshot for query-only work. UI commands run here: a long write
    /// transaction on the writer half (sync commit, ingestion batch) blocks
    /// other writers, never this reader.
    pub fn read(&self) -> MutexGuard<'_, Connection> {
        self.reader.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The single writer. All mutations and every `tx` go through here.
    pub fn write(&self) -> MutexGuard<'_, Connection> {
        self.writer.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Run `f` inside a single SQLite transaction: all writes commit together
    /// or not at all. This is the only sanctioned way to persist multi-step
    /// domain changes (sync mutations, ingest batches, …). The closure must
    /// use the `&Transaction` (or the `*_conn` helpers) — calling further `Db`
    /// methods inside would deadlock on the writer mutex.
    pub fn tx<T>(&self, f: impl FnOnce(&Transaction) -> Result<T>) -> Result<T> {
        let conn = self.write();
        let tx = conn.unchecked_transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    /// Open (or create) the database in the ONE format this build understands,
    /// then reconcile the defaults that depend on the current environment —
    /// see [`schema`] for the recognition rule, the refusal cases and why
    /// reconciliation is not a migration. Neither step ever repairs structure,
    /// and neither runs a persistent PRAGMA, so calling this before
    /// `journal_mode = WAL` keeps a refused file untouched.
    fn initialize_schema(conn: &Connection) -> Result<()> {
        schema::open_or_create(conn)?;
        schema::reconcile_runtime_defaults(conn)
    }

    // Projects

    /// Every Project currently derived from at least one WorkspacePath.
    pub fn list_projects(&self) -> Result<Vec<Project>> {
        let conn = self.read();
        let mut st = conn.prepare("SELECT * FROM projects ORDER BY updated_at DESC")?;
        let rows = st
            .query_map([], row_project)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_project(&self, id: &str) -> Result<Option<Project>> {
        let conn = self.read();
        Ok(conn
            .query_row(
                "SELECT * FROM projects WHERE id = ?1",
                params![id],
                row_project,
            )
            .optional()?)
    }

    pub fn upsert_project(&self, p: &Project) -> Result<()> {
        {
            let conn = self.write();
            upsert_project_conn(&conn, p)?;
        }
        self.index_project(p)
    }

    // Workstreams

    /// Membership is derived from the path chain:
    /// `workstream_paths → workspace_paths.project_id`. Any position counts as
    /// membership; `workspace::project` decides the primary/related distinction
    /// from `position = 0` when it renders a Project page.
    pub fn list_workstreams(&self, project_id: Option<&str>) -> Result<Vec<Workstream>> {
        let conn = self.read();
        let (sql, has_filter): (&str, bool) = if project_id.is_some() {
            (
                "SELECT w.* FROM workstreams w
              WHERE EXISTS (
                SELECT 1 FROM workstream_paths wsp
                JOIN workspace_paths wp ON wp.id = wsp.workspace_path_id
                WHERE wsp.workstream_id = w.id AND wp.project_id = ?1)
              ORDER BY w.updated_at DESC",
                true,
            )
        } else {
            ("SELECT * FROM workstreams ORDER BY updated_at DESC", false)
        };
        let mut st = conn.prepare(sql)?;
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
    /// (id, agent), that session's activity timestamp). Only Sessions that OWN
    /// this Workstream count, so a Session is never counted twice. Archived
    /// sessions are included.
    pub fn workstream_session_stats(
        &self,
        workstream_id: &str,
    ) -> Result<(i64, Option<(String, String)>, Option<String>)> {
        let conn = self.read();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sessions
             WHERE owner_workstream_id = ?1",
            params![workstream_id],
            |r| r.get(0),
        )?;
        let latest = conn
            .query_row(
                "SELECT id, agent, COALESCE(last_activity_at, started_at) AS act
                 FROM sessions
                 WHERE owner_workstream_id = ?1
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
        let conn = self.read();
        let row = conn
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
        let conn = self.read();
        Ok(conn
            .query_row(
                "SELECT MAX(updated_at) FROM context_items WHERE workstream_id = ?1",
                params![workstream_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    }

    pub fn get_workstream(&self, id: &str) -> Result<Option<Workstream>> {
        let conn = self.read();
        Ok(conn
            .query_row(
                "SELECT * FROM workstreams WHERE id = ?1",
                params![id],
                row_workstream,
            )
            .optional()?)
    }

    pub fn upsert_workstream(&self, w: &Workstream) -> Result<()> {
        {
            let conn = self.write();
            upsert_workstream_conn(&conn, w)?;
        }
        self.index_workstream(w)
    }

    /// Is any LaunchIntent still waiting for a Session? Cheap gate for the
    /// discovery path, which would otherwise run the matcher once per
    /// ownerless Session on every pass.
    pub fn has_pending_launch_intents(&self) -> Result<bool> {
        let conn = self.read();
        Ok(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM launch_intents WHERE status IN (?1, ?2))",
            params![launch_status::PENDING, launch_status::AMBIGUOUS],
            |r| r.get(0),
        )?)
    }

    /// Delete a Workstream and everything it owns — in full FK order, which a
    /// bare `DELETE FROM workstreams` violates under
    /// `PRAGMA foreign_keys = ON` (it fails the moment the Workstream has ever
    /// had a Context item or a path).
    ///
    /// This is the mechanical half only. The *product* door is
    /// `workspace::workstream::delete_workstream_permanently`, which additionally
    /// requires `visibility = archived` and preserves Sessions.
    pub fn delete_workstream(&self, id: &str) -> Result<()> {
        self.tx(|tx| workstream_paths::purge_workstream_data_conn(tx, id))?;
        self.unindex("workstream", id);
        Ok(())
    }

    // Logical Sessions

    /// Session-only fixture helper. Production discovery uses
    /// `upsert_logical_session` to create or refresh the whole row atomically.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_logical_session_unchecked(
        &self,
        agent: Agent,
        root_agent_session_id: &str,
        title: Option<&str>,
        cwd: Option<&str>,
        workspace_path_id: Option<&str>,
        forked_from_session_id: Option<&str>,
        started_at: Option<&str>,
        last_activity_at: Option<&str>,
    ) -> Result<(String, bool)> {
        let conn = self.write();
        upsert_logical_session_conn(
            &conn,
            agent,
            root_agent_session_id,
            None,
            title,
            cwd,
            workspace_path_id,
            forked_from_session_id,
            started_at,
            last_activity_at,
            "test",
            "/tmp/source",
            &serde_json::json!({}),
        )
    }

    /// Atomically create or refresh a Logical Session — the session row now
    /// carries its root source (`source_kind` / `source_path` / `metadata`)
    /// directly; discovery is the authority and replaces them wholesale.
    ///
    /// `native_title` is the Agent app's own name for the conversation and
    /// writes through on every ingest; `title` is the text-derived fallback
    /// and only fills an empty slot (see the policy on
    /// [`upsert_logical_session_conn`]).
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_logical_session(
        &self,
        agent: Agent,
        root_agent_session_id: &str,
        native_title: Option<&str>,
        title: Option<&str>,
        cwd: Option<&str>,
        workspace_path_id: Option<&str>,
        forked_from_session_id: Option<&str>,
        started_at: Option<&str>,
        last_activity_at: Option<&str>,
        source_kind: &str,
        source_path: &str,
        metadata: &serde_json::Value,
    ) -> Result<(String, bool)> {
        let conn = self.write();
        upsert_logical_session_conn(
            &conn,
            agent,
            root_agent_session_id,
            native_title,
            title,
            cwd,
            workspace_path_id,
            forked_from_session_id,
            started_at,
            last_activity_at,
            source_kind,
            source_path,
            metadata,
        )
    }

    /// Sessions that may still owe a LaunchIntent match: ownerless Sessions,
    /// trashed ones included. An owned Session has already matched.
    pub fn reconcile_retry_sessions(&self, agent: Option<Agent>) -> Result<Vec<Session>> {
        let conn = self.read();
        let mut st = conn.prepare(
            "SELECT s.* FROM sessions s
             WHERE s.owner_workstream_id IS NULL
               AND (?1 IS NULL OR s.agent = ?1)
             ORDER BY COALESCE(s.last_conversation_at, s.last_activity_at, s.started_at) DESC",
        )?;
        let rows = st
            .query_map(params![agent.map(|a| a.as_str())], row_session)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_session(&self, id: &str) -> Result<Option<Session>> {
        let conn = self.read();
        Ok(conn
            .query_row(
                "SELECT * FROM sessions WHERE id = ?1",
                params![id],
                row_session,
            )
            .optional()?)
    }

    /// The Logical Session for a root Resume identity. This is THE
    /// session lookup for LaunchIntent matching and Resume.
    pub fn find_session_by_root_agent_id(
        &self,
        agent: Agent,
        root_agent_session_id: &str,
    ) -> Result<Option<Session>> {
        let conn = self.read();
        Ok(conn
            .query_row(
                "SELECT * FROM sessions WHERE agent = ?1 AND root_agent_session_id = ?2",
                params![agent.as_str(), root_agent_session_id],
                row_session,
            )
            .optional()?)
    }

    /// A `project_id` filter reads the **authoritative chain**
    /// (`sessions.workspace_path_id → workspace_paths.project_id`), never the
    /// `sessions.project_id` cache.
    ///
    /// The cache is derived for every row the app writes, but rows without a
    /// resolvable path are not members of a Project. Listing follows the chain
    /// so every view uses the same authority.
    pub fn list_sessions(&self, filter: SessionFilter) -> Result<Vec<Session>> {
        let conn = self.read();
        // Dynamic SQL: placeholders are appended together with the bind
        // values, so the numbering can never drift out of sync.
        let mut sql = "SELECT * FROM sessions WHERE 1=1".to_string();
        let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(p) = &filter.project_id {
            values.push(Box::new(p.clone()));
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM workspace_paths wp \
                                WHERE wp.id = sessions.workspace_path_id \
                                  AND wp.project_id = ?{})",
                values.len()
            ));
        }
        if let Some(a) = &filter.agent {
            values.push(Box::new(a.as_str().to_string()));
            sql.push_str(&format!(" AND agent = ?{}", values.len()));
        }
        match filter.scope {
            SessionListScope::Unarchived => sql.push_str(" AND archived_at IS NULL"),
            SessionListScope::Archived => sql.push_str(" AND archived_at IS NOT NULL"),
            SessionListScope::All => {}
        }
        sql.push_str(
            " ORDER BY COALESCE(last_conversation_at, last_activity_at, started_at) DESC LIMIT 500",
        );
        let mut st = conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::types::ToSql> = values.iter().map(|v| v.as_ref()).collect();
        let rows = st
            .query_map(refs.as_slice(), row_session)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // Session source cursor

    /// Rewind the session's source cursor so the next ingest re-scans from
    /// the start. MESSAGES ARE NOT TOUCHED: their ids and every provenance ref
    /// stay valid, because unchanged content dedups by identity on the re-scan.
    /// The Context frontier is preserved.
    pub fn rewind_source_cursor(&self, session_id: &str) -> Result<()> {
        let conn = self.write();
        conn.execute(
            "UPDATE sessions
             SET source_byte_offset = 0, source_last_seen_size = 0,
                 source_prefix_hash = '', source_tail_hash = ''
             WHERE id = ?1",
            params![session_id],
        )?;
        Ok(())
    }

    // Conversation Messages

    /// The atomic member-ingest commit. Messages, the member cursor and the
    /// activity stamps commit or not at all.
    ///
    /// Guards, in order, inside the transaction: the Logical Session still
    /// exists; the member still belongs to it; and messages require
    /// `member.relation == root` (an adapter handing child text to the
    /// conversation is a bug, never silently stored). A failed guard stores
    /// NOTHING, so a purge racing a parse cannot produce a half commit.
    ///
    /// The identity chain starts from genesis on a full re-scan (start offset 0)
    /// and otherwise continues from the cursor's `identity_tail_hash` — the tail
    /// of the CURRENT source chain, never "last message in the store".
    pub fn commit_ingest(
        &self,
        session_id: &str,
        messages: &[ParsedSessionMessage],
        source: &SourceCursorUpdate,
    ) -> Result<Vec<SessionMessage>> {
        self.commit_ingest_snapshot(session_id, messages, source, true)
    }

    /// The atomic ingest commit. Messages, the session's source cursor and
    /// the activity stamps commit or not at all.
    ///
    /// Guards, inside the transaction: the Logical Session still exists (a
    /// purge racing a parse stores NOTHING).
    ///
    /// The identity chain starts from genesis on a full re-scan (start offset 0)
    /// and otherwise continues from the session's `source_tail_hash` — the tail
    /// of the CURRENT source chain, never "last message in the store".
    ///
    /// Same transaction, for readers that can prove whether EVERY frame of
    /// the read was whole (`complete_snapshot`): an incomplete full re-scan
    /// keeps the projection, fact generation and cursor untouched for a retry.
    pub fn commit_ingest_snapshot(
        &self,
        session_id: &str,
        messages: &[ParsedSessionMessage],
        source: &SourceCursorUpdate,
        complete_snapshot: bool,
    ) -> Result<Vec<SessionMessage>> {
        self.tx(|tx| {
            // A session purged mid-parse takes NOTHING.
            if !session_lifecycle::session_exists_conn(tx, session_id)? {
                return Ok(Vec::new());
            }

            let old_cursor: Option<(String, i64, i64, i64, Option<f64>)> = tx
                .query_row(
                    "SELECT source_file_identity, source_generation, source_byte_offset,
                            source_last_seen_size, source_mtime
                     FROM sessions WHERE id = ?1",
                    params![session_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .optional()?;
            let source_changed = old_cursor
                .as_ref()
                .map(|(identity, generation, offset, size, mtime)| {
                    identity != &source.file_identity
                        || *generation != source.generation
                        || *offset != source.byte_offset as i64
                        || *size != source.last_seen_size as i64
                        || match (mtime, source.mtime) {
                            (Some(a), Some(b)) => (a - b).abs() > 1e-6,
                            (None, None) => false,
                            _ => true,
                        }
                })
                .unwrap_or(true);

            let stored_tail: Option<String> = tx
                .query_row(
                    "SELECT source_tail_hash FROM sessions WHERE id = ?1",
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
                    // No chain tail is available for this cursor: fall back to
                    // the last stored message identity, and to genesis when
                    // the session has no message at all.
                    None => tx
                        .query_row(
                            "SELECT source_identity_hash FROM session_messages
                             WHERE session_id = ?1 ORDER BY sequence DESC LIMIT 1",
                            params![session_id],
                            |r| r.get::<_, String>(0),
                        )
                        .optional()?
                        .unwrap_or_else(|| IDENTITY_GENESIS.to_string()),
                }
            };

            let mut next_seq: i64 = tx.query_row(
                "SELECT COALESCE(MAX(sequence), 0) FROM session_messages WHERE session_id = ?1",
                params![session_id],
                |r| r.get::<_, i64>(0),
            )?;
            next_seq += 1;

            let mut stored = Vec::with_capacity(messages.len());
            let mut current_ids: Vec<String> = Vec::with_capacity(messages.len());
            let raw_path: String = tx.query_row(
                "SELECT source_path FROM sessions WHERE id = ?1",
                params![session_id],
                |r| r.get(0),
            )?;
            {
                let mut ins = tx.prepare(
                    "INSERT INTO session_messages
                     (id, session_id, sequence, source_message_id, source_generation,
                      source_position, source_identity_hash, ts, role, content, raw_ref)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                     ON CONFLICT(session_id, source_identity_hash) DO NOTHING",
                )?;
                for m in messages {
                    // On conflict the computed hash IS the stored one (the
                    // unique index matches on it), so the chain stays
                    // continuous whether or not the row is new.
                    let hash = message_identity_hash(
                        &prev_hash,
                        m.source_message_id.as_deref(),
                        m.role.as_str(),
                        m.ts.as_deref(),
                        &m.content,
                    );
                    prev_hash = hash.clone();
                    let id = new_id();
                    let raw_ref = format!("{}#{}", raw_path, m.source_position);
                    let n = ins.execute(params![
                        id,
                        session_id,
                        next_seq,
                        m.source_message_id,
                        source.generation,
                        m.source_position,
                        hash,
                        m.ts,
                        m.role.as_str(),
                        m.content,
                        raw_ref,
                    ])?;
                    if n > 0 {
                        current_ids.push(id.clone());
                        stored.push(SessionMessage {
                            id: id.clone(),
                            session_id: session_id.to_string(),
                            sequence: next_seq,
                            role: m.role,
                            content: m.content.clone(),
                            ts: m.ts.clone(),
                            // Derived from the projection after it moves.
                            turn_final: false,
                            source_message_id: m.source_message_id.clone(),
                            source_generation: source.generation,
                            source_position: m.source_position.clone(),
                            source_identity_hash: hash,
                            raw_ref,
                        });
                        next_seq += 1;
                    } else {
                        // Dedup hit: same message identity. The message is
                        // part of THIS read's conversation even though it was
                        // already stored: a full re-scan's projection must
                        // contain it.
                        if let Some(existing) = tx
                            .query_row(
                                "SELECT id FROM session_messages
                                 WHERE session_id = ?1 AND source_identity_hash = ?2",
                                params![session_id, hash],
                                |r| r.get::<_, String>(0),
                            )
                            .optional()?
                        {
                            current_ids.push(existing);
                        }
                    }
                }
            }

            // The current-message projection: this is what makes a rewritten
            // source's retired messages stop being the conversation, while their
            // `session_messages` rows survive for provenance.
            let projection = apply_projection_conn(
                tx,
                session_id,
                &current_ids,
                source.start_byte_offset == 0,
                complete_snapshot,
            )?;
            if projection == ProjectionOutcome::Incomplete {
                // An unfinished frame means we cannot prove the whole
                // conversation: keep the projection, the fact generation AND
                // the cursor — the retry re-reads the same bytes.
                eprintln!("[ingest] 完整重扫不完整，保留原投影与游标，等待重试");
                return Ok(stored);
            }

            sync_projected_message_search_conn(tx, session_id)?;

            let new_tail = if messages.is_empty() {
                stored_tail.unwrap_or_default()
            } else {
                prev_hash
            };
            tx.execute(
                "UPDATE sessions
                 SET source_file_identity = ?2, source_generation = ?3,
                     source_byte_offset = ?4, source_last_seen_size = ?5,
                     source_mtime = ?6, source_prefix_hash = ?7, source_tail_hash = ?8
                 WHERE id = ?1",
                params![
                    session_id,
                    source.file_identity,
                    source.generation,
                    source.byte_offset as i64,
                    source.last_seen_size as i64,
                    source.mtime,
                    source.prefix_hash,
                    new_tail
                ],
            )?;

            // Source activity advances only when its observed cursor changes;
            // reading an unchanged file is not Agent activity.
            if source_changed {
                let observed = mtime_timestamp(source.mtime);
                if let Some(observed) = observed {
                    let current: Option<String> = tx.query_row(
                        "SELECT last_activity_at FROM sessions WHERE id = ?1",
                        params![session_id],
                        |r| r.get(0),
                    )?;
                    let activity = later_timestamp(current.as_deref(), Some(&observed));
                    if let Some(activity) = activity {
                        tx.execute(
                            "UPDATE sessions SET last_activity_at = ?2 WHERE id = ?1",
                            params![session_id, activity],
                        )?;
                    }
                }
            }
            let latest = stored
                .iter()
                .filter_map(|message| message.ts.as_deref())
                .fold(None, |latest, ts| {
                    later_timestamp(latest.as_deref(), Some(ts))
                });
            let source_mtime = stored
                .iter()
                .any(|message| message.ts.is_none())
                .then(|| mtime_timestamp(source.mtime))
                .flatten();
            let latest = if stored.is_empty() {
                None
            } else {
                later_timestamp(latest.as_deref(), source_mtime.as_deref())
            };
            if let Some(latest) = latest {
                let previous: Option<String> = tx.query_row(
                    "SELECT last_conversation_at FROM sessions WHERE id = ?1",
                    params![session_id],
                    |r| r.get(0),
                )?;
                let latest = later_timestamp(previous.as_deref(), Some(&latest));
                tx.execute(
                    "UPDATE sessions SET last_conversation_at = ?2 WHERE id = ?1",
                    params![session_id, latest],
                )?;
            }
            Ok(stored)
        })
    }

    /// The CURRENT effective conversation, read through the message
    /// projection: `after_ordinal` selects ordinals strictly greater than it,
    /// in projection order. A re-scan that changed the conversation replaced
    /// the projection, so an expired message can never reappear here.
    pub fn get_messages(
        &self,
        session_id: &str,
        after_ordinal: Option<i64>,
        limit: i64,
    ) -> Result<Vec<SessionMessage>> {
        let conn = self.read();
        get_messages_conn(&conn, session_id, after_ordinal, limit)
    }

    /// The Context-input read: current-projection messages after the
    /// processed ordinal, in order.
    pub fn get_messages_after(
        &self,
        session_id: &str,
        processed_ordinal: i64,
        limit: i64,
    ) -> Result<Vec<SessionMessage>> {
        self.get_messages(session_id, Some(processed_ordinal), limit)
    }

    /// The newest `limit` messages of the CURRENT conversation, oldest first.
    /// The newest `limit` messages of the CURRENT conversation, oldest first.
    pub fn recent_messages(&self, session_id: &str, limit: i64) -> Result<Vec<SessionMessage>> {
        let conn = self.read();
        Ok(window_messages_conn(&conn, session_id, None, limit)?
            .into_iter()
            .map(|(_, message)| message)
            .collect())
    }

    /// Like [`Self::recent_messages`], but keeping only user turns and each
    /// turn's FINAL assistant reply — the conversation's structure, without
    /// the assistant's intermediate outputs.
    pub fn recent_turn_messages(
        &self,
        session_id: &str,
        limit: i64,
    ) -> Result<Vec<SessionMessage>> {
        let conn = self.read();
        Ok(
            window_messages_filtered_conn(&conn, session_id, None, limit, true)?
                .into_iter()
                .map(|(_, message)| message)
                .collect(),
        )
    }

    /// One page of the CURRENT conversation read BACKWARD from the tail:
    /// `before_ordinal` is the exclusive upper bound (`None` = the newest
    /// messages). The page comes back oldest first and carries its own cursor,
    /// so callers never handle projection ordinals themselves.
    ///
    /// A page is the conversation's SKELETON — user messages and each turn's
    /// final reply; the assistant's intermediate outputs ride in per turn via
    /// [`WindowedMessage::turn`] and [`Self::turn_intermediates`].
    ///
    /// `generation` travels with the page because an ingest that rewrites the
    /// conversation replaces the projection and raises it: a caller paging
    /// upward that sees it change knows its older pages no longer belong to
    /// this conversation.
    pub fn message_window(
        &self,
        session_id: &str,
        before_ordinal: Option<i64>,
        limit: i64,
    ) -> Result<MessageWindow> {
        let conn = self.read();
        let state = get_ingest_state_conn(&conn, session_id)?;
        let rows = window_messages_filtered_conn(&conn, session_id, before_ordinal, limit, true)?;
        Ok(message_window_from(state, &conn, session_id, rows)?)
    }

    /// One page read FORWARD from `after_ordinal` (exclusive): the next newer
    /// messages, oldest first. This is what lets a reader that jumped into the
    /// middle of a conversation keep reading toward the tail.
    pub fn newer_window(
        &self,
        session_id: &str,
        after_ordinal: i64,
        limit: i64,
    ) -> Result<MessageWindow> {
        let conn = self.read();
        let state = get_ingest_state_conn(&conn, session_id)?;
        let rows = forward_messages_filtered_conn(&conn, session_id, after_ordinal, limit, true)?;
        Ok(message_window_from(state, &conn, session_id, rows)?)
    }

    /// One turn's intermediate messages — everything strictly between the
    /// turn's user message (`after_ordinal`) and its final reply
    /// (`before_ordinal`), oldest first. `truncated` says the cap cut the
    /// range short. This is the expand action behind a turn's collapsed
    /// intermediate block.
    pub fn turn_intermediates(
        &self,
        session_id: &str,
        after_ordinal: i64,
        before_ordinal: i64,
        limit: i64,
    ) -> Result<(Vec<SessionMessage>, bool)> {
        let conn = self.read();
        let mut st = conn.prepare(
            "SELECT m.*
             FROM session_message_projection p
             JOIN session_messages m ON m.id = p.session_message_id
             WHERE p.session_id = ?1 AND p.ordinal > ?2 AND p.ordinal < ?3
             ORDER BY p.ordinal ASC LIMIT ?4",
        )?;
        let mut rows = st
            .query_map(
                params![session_id, after_ordinal, before_ordinal, limit + 1],
                row_message,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let truncated = rows.len() as i64 > limit;
        rows.truncate(limit as usize);
        Ok((rows, truncated))
    }

    /// Where the USER messages sit in the CURRENT conversation, in order: the
    /// navigation rail's marks. Each carries its projection ordinal (for
    /// position and jumping) and the first line of its text (for the tooltip).
    pub fn user_message_marks(&self, session_id: &str) -> Result<Vec<MessageMark>> {
        let conn = self.read();
        let mut st = conn.prepare(
            "SELECT p.ordinal, m.content
             FROM session_message_projection p
             JOIN session_messages m ON m.id = p.session_message_id
             WHERE p.session_id = ?1 AND m.role = 'user'
             ORDER BY p.ordinal",
        )?;
        let rows = st
            .query_map(params![session_id], |r| {
                let ordinal: i64 = r.get(0)?;
                let content: String = r.get(1)?;
                Ok(MessageMark {
                    ordinal,
                    preview: crate::adapters::truncate_text(
                        content.lines().next().unwrap_or("").trim(),
                        MARK_PREVIEW_CHARS,
                    ),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// How many messages the CURRENT conversation holds — a projection
    /// ordinal bound, never `session_messages.sequence`.
    pub fn ingested_message_sequence(&self, session_id: &str) -> Result<i64> {
        let conn = self.read();
        Ok(conn
            .query_row(
                "SELECT latest_message_seq FROM sessions WHERE id = ?1",
                params![session_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0))
    }

    pub fn message_count(&self, session_id: &str) -> Result<i64> {
        self.ingested_message_sequence(session_id)
    }

    /// Resolve a stable provenance reference (`session-message:<id>`) to its
    /// message.
    pub fn get_message_by_ref(&self, source_ref: &str) -> Result<Option<SessionMessage>> {
        if let Some(id) = source_ref.strip_prefix("session-message:") {
            let conn = self.read();
            return Ok(conn
                .query_row(
                    "SELECT * FROM session_messages WHERE id = ?1",
                    params![id],
                    row_message,
                )
                .optional()?);
        }
        Ok(None)
    }

    // Session Owner

    /// Set (or clear) the single Owner Workstream of a Session.
    /// This touches exactly one column: `sessions.owner_workstream_id`.
    /// `None` clears ownership. No implicit WorkstreamPath / cwd / Project
    /// mutation happens here.
    pub fn set_session_owner(&self, session_id: &str, workstream_id: Option<&str>) -> Result<()> {
        let conn = self.write();
        set_session_owner_conn(&conn, session_id, workstream_id)
    }

    /// Sessions owned by `workstream_id`, including archived sessions.
    /// A Session appears in at most one Workstream's list.
    pub fn sessions_for_workstream(&self, workstream_id: &str) -> Result<Vec<Session>> {
        let conn = self.read();
        let mut st = conn.prepare(
            "SELECT * FROM sessions
             WHERE owner_workstream_id = ?1
             ORDER BY COALESCE(last_activity_at, started_at) DESC",
        )?;
        let rows = st
            .query_map(params![workstream_id], row_session)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // Context Items

    pub fn insert_item(&self, item: &ContextItem, revision: &ContextItemRevision) -> Result<()> {
        {
            let conn = self.write();
            insert_item_conn(&conn, item, revision)?;
        }
        self.index_item(item, revision)
    }

    pub fn insert_revision(&self, r: &ContextItemRevision) -> Result<()> {
        let conn = self.write();
        insert_revision_conn(&conn, r)
    }

    pub fn set_item_head(
        &self,
        item_id: &str,
        revision_id: &str,
        status: Option<&str>,
    ) -> Result<()> {
        let conn = self.write();
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

    pub fn set_item_status(&self, item_id: &str, status: &str) -> Result<()> {
        let conn = self.write();
        conn.execute(
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
        source_refs: &[String],
    ) -> Result<()> {
        self.tx(|tx| {
            apply_status_change_conn(tx, item_id, new_status, actor, reason, source_refs)?;
            // A manual status change is Context content movement: the
            // Workstream's context revision must move with it so a concurrent
            // AI commit cannot silently overwrite it.
            let ws: Option<String> = tx
                .query_row(
                    "SELECT workstream_id FROM context_items WHERE id = ?1",
                    params![item_id],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(ws) = ws {
                bump_context_revision_conn(tx, &ws)?;
            }
            Ok(())
        })
    }

    pub fn set_item_authority(&self, item_id: &str, authority: &str) -> Result<()> {
        let conn = self.write();
        set_item_authority_conn(&conn, item_id, authority)
    }

    pub fn get_item(&self, id: &str) -> Result<Option<ContextItem>> {
        let conn = self.read();
        get_item_conn(&conn, id)
    }

    pub fn get_revision(&self, id: &str) -> Result<Option<ContextItemRevision>> {
        let conn = self.read();
        get_revision_conn(&conn, id)
    }

    pub fn items_for_workstream(
        &self,
        workstream_id: &str,
        include_inactive: bool,
    ) -> Result<Vec<(ContextItem, ContextItemRevision)>> {
        let conn = self.read();
        items_for_workstream_conn(&conn, workstream_id, include_inactive)
    }

    pub fn item_history(&self, item_id: &str) -> Result<Vec<ContextItemRevision>> {
        let conn = self.read();
        let mut st = conn.prepare(
            "SELECT id, item_id, title, content, metadata, source_type, source_ref, created_at
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
        let conn = self.read();
        let mut st = conn.prepare(
            "SELECT i.id, i.workstream_id, i.kind, i.status, i.authority, i.created_by, i.current_revision_id, i.supersedes_item_id, i.created_at, i.updated_at,
                    r.id, r.item_id, r.title, r.content, r.metadata, r.source_type, r.source_ref, r.created_at
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

    pub fn item_relations_for_workstream(
        &self,
        workstream_id: &str,
    ) -> Result<Vec<ContextItemRelation>> {
        let conn = self.read();
        item_relations_for_workstream_conn(&conn, workstream_id)
    }

    pub fn get_context_revision_source(
        &self,
        revision_id: &str,
    ) -> Result<Option<ContextSourceDetail>> {
        let rev = self.get_revision(revision_id)?;
        let Some(rev) = rev else {
            return Ok(None);
        };
        // Read authority strictly from the immutable revision metadata and frozen
        // lineage: later item edits must never pollute it.
        let authority = resolve_revision_authority(&rev);

        // A redacted revision's session is GONE — not a tombstone. Return only
        // the fact that the source no longer exists: no session id, title or path
        // may resolve, and nothing may redirect to a rediscovered session.
        if rev.source_type.as_deref() == Some("deleted_session") {
            return Ok(Some(ContextSourceDetail {
                revision_id: rev.id,
                authority,
                source_type: rev.source_type,
                source_ref: None,
                session_id: None,
                session_title: None,
                agent: None,
                message_sequence: None,
                message_ts: None,
                evidence: None,
            }));
        }

        let mut session_id = None;
        let mut session_title = None;
        let mut agent = None;
        let mut message_sequence = None;
        let mut message_ts = None;
        let mut evidence = None;

        if let Some(sref) = &rev.source_ref {
            if let Some(msg) = self.get_message_by_ref(sref)? {
                message_sequence = Some(msg.sequence);
                message_ts = msg.ts;
                evidence = Some(msg.content);
                let sid = msg.session_id;
                if let Some(s) = self.get_session(&sid)? {
                    session_title = s.title.or(Some(s.root_agent_session_id));
                    agent = Some(s.agent);
                }
                session_id = Some(sid);
            }
        }

        Ok(Some(ContextSourceDetail {
            revision_id: rev.id,
            authority,
            source_type: rev.source_type,
            source_ref: rev.source_ref,
            session_id,
            session_title,
            agent,
            message_sequence,
            message_ts,
            evidence,
        }))
    }

    pub fn list_workstream_context_changes(
        &self,
        workstream_id: &str,
        limit: usize,
    ) -> Result<Vec<ContextChange>> {
        let conn = self.read();
        list_workstream_context_changes_conn(&conn, workstream_id, limit)
    }

    pub fn get_workstream_review_state(
        &self,
        workstream_id: &str,
    ) -> Result<Option<WorkstreamReviewState>> {
        let conn = self.read();
        get_workstream_review_state_conn(&conn, workstream_id)
    }

    pub fn get_workstream_review_window(
        &self,
        workstream_id: &str,
    ) -> Result<WorkstreamReviewWindow> {
        let conn = self.read();
        get_workstream_review_window_conn(&conn, workstream_id)
    }

    pub fn mark_workstream_reviewed(
        &self,
        workstream_id: &str,
        frontier: &ReviewFrontier,
    ) -> Result<WorkstreamReviewState> {
        let conn = self.write();
        mark_workstream_reviewed_conn(&conn, workstream_id, frontier)
    }

    pub fn get_workstream_review_summary(
        &self,
        workstream_id: &str,
    ) -> Result<WorkstreamReviewSummary> {
        let conn = self.read();
        get_workstream_review_summary_conn(&conn, workstream_id)
    }

    pub fn list_workstream_review_summaries(&self) -> Result<Vec<WorkstreamReviewSummary>> {
        let conn = self.read();
        list_workstream_review_summaries_conn(&conn)
    }

    // Conflicts

    pub fn insert_conflict(&self, c: &ContextConflict) -> Result<()> {
        let conn = self.write();
        insert_conflict_conn(&conn, c)
    }

    pub fn conflicts_for_workstream(
        &self,
        workstream_id: &str,
        include_closed: bool,
    ) -> Result<Vec<ContextConflict>> {
        let conn = self.read();
        conflicts_for_workstream_conn(&conn, workstream_id, include_closed)
    }

    pub fn open_conflicts_for_item(&self, item_id: &str) -> Result<Vec<ContextConflict>> {
        let conn = self.read();
        let mut st = conn.prepare(
            "SELECT id, workstream_id, left_item_id, right_item_id, conflict_type, status, resolution, created_at, updated_at,
                    left_revision_id, right_revision_id, candidate_snapshot_json
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
        self.resolve_conflict_with_edit(conflict_id, status, resolution, None, "user")
    }

    pub fn resolve_conflict_audited(
        &self,
        conflict_id: &str,
        status: &str,
        resolution: Option<&str>,
        actor: &str,
    ) -> Result<()> {
        self.resolve_conflict_with_edit(conflict_id, status, resolution, None, actor)
    }

    pub fn resolve_conflict_with_edit(
        &self,
        conflict_id: &str,
        status: &str,
        resolution: Option<&str>,
        edit: Option<&ContextItemEditPayload>,
        actor: &str,
    ) -> Result<()> {
        self.tx(|tx| {
            resolve_conflict_with_edit_conn(tx, conflict_id, status, resolution, edit, actor)
        })
    }

    pub fn conflict_history(&self, conflict_id: &str) -> Result<Vec<ContextConflictEvent>> {
        let conn = self.read();
        let mut st = conn.prepare(
            "SELECT id, conflict_id, previous_status, new_status, resolution, actor, created_at, snapshot_json
             FROM context_conflict_events WHERE conflict_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = st
            .query_map(params![conflict_id], row_conflict_event)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_conflict(&self, conflict_id: &str) -> Result<Option<ContextConflict>> {
        let conn = self.read();
        get_conflict_conn(&conn, conflict_id)
    }

    pub fn get_conflict_review_case(
        &self,
        conflict_id: &str,
    ) -> Result<Option<ConflictReviewCase>> {
        let conn = self.read();
        get_conflict_review_case_conn(&conn, conflict_id)
    }

    pub fn list_conflict_review_cases(
        &self,
        workstream_id: &str,
        include_closed: bool,
    ) -> Result<Vec<ConflictReviewCase>> {
        let conn = self.read();
        list_conflict_review_cases_conn(&conn, workstream_id, include_closed)
    }

    // Launch intents

    pub fn insert_launch_intent(&self, i: &LaunchIntent) -> Result<()> {
        let conn = self.write();
        conn.execute(
            "INSERT INTO launch_intents
             (id, launch_type, agent, owner_workstream_id, cwd, process_id, launched_at, matched_session_id, status, note, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                i.id, i.launch_type, i.agent.as_str(),
                i.owner_workstream_id,
                i.cwd, i.process_id.map(|p| p as i64),
                i.launched_at, i.matched_session_id, i.status, i.note, i.created_at, i.updated_at
            ],
        )?;
        Ok(())
    }

    pub fn get_launch_intent(&self, id: &str) -> Result<Option<LaunchIntent>> {
        let conn = self.read();
        crate::storage::get_launch_intent_conn(&conn, id)
    }

    pub fn list_launch_intents(&self, statuses: &[&str], limit: i64) -> Result<Vec<LaunchIntent>> {
        let conn = self.read();
        let filter = if statuses.is_empty() {
            String::new()
        } else {
            let placeholders = statuses.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
            format!(" WHERE status IN ({})", placeholders)
        };
        let sql = format!(
            "SELECT id, launch_type, agent, owner_workstream_id, cwd, process_id, launched_at, matched_session_id, status, note, created_at, updated_at
             FROM launch_intents{} ORDER BY created_at DESC LIMIT {}",
            filter, limit
        );
        let mut st = conn.prepare(&sql)?;
        let statuses: Vec<String> = statuses.iter().map(|s| s.to_string()).collect();
        let rows = st
            .query_map(
                rusqlite::params_from_iter(statuses.iter()),
                row_launch_intent,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // Ingest sources

    pub fn list_ingest_sources(&self) -> Result<Vec<IngestSource>> {
        let conn = self.read();
        let mut st = conn.prepare(
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
        let conn = self.write();
        let src = IngestSource {
            id: new_id(),
            agent,
            path: path.to_string(),
            enabled,
            origin: "user".into(),
            created_at: now(),
        };
        conn.execute(
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
        let conn = self.read();
        Ok(conn
            .query_row(
                "SELECT id, agent, path, enabled, origin, created_at FROM ingest_sources WHERE id = ?1",
                params![id],
                row_ingest_source,
            )
            .optional()?)
    }

    pub fn set_ingest_source_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let conn = self.write();
        conn.execute(
            "UPDATE ingest_sources SET enabled = ?2 WHERE id = ?1",
            params![id, enabled as i64],
        )?;
        Ok(())
    }

    /// Enable / disable EVERY source at once — the 全选 toggle behind the
    /// sources page's checkbox. One statement, one write lock; the returned
    /// count is how many rows changed, so the UI can skip its reload when the
    /// state already matched.
    pub fn set_all_ingest_sources_enabled(&self, enabled: bool) -> Result<usize> {
        let conn = self.write();
        let changed = conn.execute(
            "UPDATE ingest_sources SET enabled = ?1 WHERE enabled != ?1",
            params![enabled as i64],
        )?;
        Ok(changed)
    }

    /// Only user-added sources may be removed; defaults are toggled instead.
    pub fn remove_ingest_source(&self, id: &str) -> Result<()> {
        let conn = self.write();
        conn.execute(
            "DELETE FROM ingest_sources WHERE id = ?1 AND origin = 'user'",
            params![id],
        )?;
        Ok(())
    }

    /// The directories reconcile is allowed to scan for this agent.
    pub fn enabled_roots(&self, agent: Agent) -> Result<Vec<String>> {
        let conn = self.read();
        let mut st = conn.prepare(
            "SELECT path FROM ingest_sources WHERE agent = ?1 AND enabled = 1 ORDER BY created_at",
        )?;
        let rows = st
            .query_map(params![agent.as_str()], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // Settings

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let conn = self.read();
        Ok(conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.write();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = ?2",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn delete_setting(&self, key: &str) -> Result<()> {
        let conn = self.write();
        conn.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
        Ok(())
    }

    // Assistant

    pub fn ensure_assistant_session(&self, session_id: Option<&str>) -> Result<String> {
        let conn = self.write();
        if let Some(id) = session_id {
            let exists = conn
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
        conn.execute(
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
        let conn = self.write();
        let id = new_id();
        let ts = now();
        conn.execute(
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
        let conn = self.read();
        let mut st = conn.prepare(
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

    // Agent installations cache

    pub fn save_installation(
        &self,
        i: &crate::platform::exec_resolver::AgentInstallation,
    ) -> Result<()> {
        let conn = self.write();
        conn.execute(
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
        let conn = self.read();
        Ok(conn
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

    /// Drop a cached installation row (startup snapshot: a failed resolve
    /// means the CLI is gone; keeping the row would keep reporting it as
    /// detected and let it be chosen as default agent).
    pub fn delete_installation(&self, agent: Agent) -> Result<()> {
        let conn = self.write();
        conn.execute(
            "DELETE FROM agent_installations WHERE agent = ?1",
            params![agent.as_str()],
        )?;
        Ok(())
    }

    // FTS

    pub fn unindex(&self, kind: &str, ref_id: &str) {
        let conn = self.write();
        unindex_conn(&conn, kind, ref_id);
    }

    pub fn index_project(&self, p: &Project) -> Result<()> {
        let conn = self.write();
        unindex_conn(&conn, "project", &p.id);
        conn.execute(
            "INSERT INTO search_index (kind, ref_id, parent_id, title, body) VALUES ('project', ?1, '', ?2, ?3)",
            params![p.id, p.name, p.description],
        )?;
        Ok(())
    }

    /// Search's `parent_id` for a Workstream is its **primary-path Project**
    /// (`workstream_paths` at position 0 → `workspace_paths.project_id`), read
    /// at index time. Any path-list mutation therefore has to re-index this row.
    pub fn index_workstream(&self, w: &Workstream) -> Result<()> {
        let conn = self.write();
        unindex_conn(&conn, "workstream", &w.id);
        conn.execute(
            "INSERT INTO search_index (kind, ref_id, parent_id, title, body)
             VALUES ('workstream', ?1,
                     (SELECT wp.project_id FROM workstream_paths wsp
                        JOIN workspace_paths wp ON wp.id = wsp.workspace_path_id
                       WHERE wsp.workstream_id = ?1 AND wsp.position = 0),
                     ?2, ?3)",
            params![w.id, w.title, w.description],
        )?;
        Ok(())
    }

    pub fn index_item(&self, item: &ContextItem, rev: &ContextItemRevision) -> Result<()> {
        let conn = self.write();
        index_item_conn(&conn, item, rev)
    }
}

pub fn unindex_conn(conn: &Connection, kind: &str, ref_id: &str) {
    let _ = conn.execute(
        "DELETE FROM search_index WHERE kind = ?1 AND ref_id = ?2",
        params![kind, ref_id],
    );
}

/// Rebuild a Session's message search rows from the authoritative current
/// conversation projection. Called in the same transaction as projection
/// replacement so retired messages can never remain searchable after commit.
fn sync_projected_message_search_conn(conn: &Connection, session_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM search_index WHERE kind = 'message' AND parent_id = ?1",
        params![session_id],
    )?;
    conn.execute(
        "INSERT INTO search_index (kind, ref_id, parent_id, title, body)
         SELECT 'message', m.id, m.session_id, '', m.content
           FROM session_message_projection p
           JOIN session_messages m ON m.id = p.session_message_id
          WHERE p.session_id = ?1",
        params![session_id],
    )?;
    Ok(())
}

pub fn index_item_conn(
    conn: &Connection,
    item: &ContextItem,
    rev: &ContextItemRevision,
) -> Result<()> {
    unindex_conn(conn, "item", &item.id);
    conn.execute(
        "INSERT INTO search_index (kind, ref_id, parent_id, title, body) VALUES ('item', ?1, ?2, ?3, ?4)",
        params![item.id, item.workstream_id, rev.title, rev.content],
    )?;
    Ok(())
}

impl Db {
    /// Index newly stored conversation messages. Insert-only per message:
    /// identity dedup happens at the message layer, so incremental batches never
    /// need to wipe the session's earlier index rows. Every SessionMessage is
    /// indexed — no length pre-filter stands between a short constraint and
    /// findability.
    pub fn index_new_messages(&self, messages: &[SessionMessage]) -> Result<()> {
        let conn = self.write();
        for m in messages {
            unindex_conn(&conn, "message", &m.id);
        }
        let mut st = conn.prepare(
            "INSERT INTO search_index (kind, ref_id, parent_id, title, body)
             SELECT 'message', ?1, ?2, '', ?3
             WHERE EXISTS (
               SELECT 1 FROM session_message_projection p
               JOIN sessions s ON s.id = p.session_id
               WHERE p.session_id = ?2 AND p.session_message_id = ?1
             )",
        )?;
        for m in messages {
            st.execute(params![m.id, m.session_id, m.content])?;
        }
        Ok(())
    }

    /// Fill in the index rows no incremental write produced: messages whose
    /// ingestion-time indexing was skipped, and Session documents no write has
    /// touched yet. Idempotent.
    ///
    /// Archived and unarchived sessions are indexed identically.
    pub fn backfill_search_index(&self) -> Result<()> {
        let conn = self.write();
        conn.execute(
            "DELETE FROM search_index
             WHERE kind = 'message'
               AND NOT EXISTS (
                 SELECT 1 FROM session_message_projection p
                 JOIN sessions s ON s.id = p.session_id
                 WHERE p.session_id = search_index.parent_id
                   AND p.session_message_id = search_index.ref_id
               )",
            [],
        )?;
        conn.execute(
            "INSERT INTO search_index (kind, ref_id, parent_id, title, body)
             SELECT 'message', m.id, m.session_id, '', m.content
             FROM session_message_projection p
             JOIN session_messages m ON m.id = p.session_message_id
             WHERE p.session_id IN (SELECT id FROM sessions)
               AND m.id NOT IN (
                   SELECT ref_id FROM search_index WHERE kind = 'message')",
            [],
        )?;
        // A document whose body can change (title, Project, Owner Workstream)
        // is refreshed at the write that changed it, so the backfill only covers
        // rows no write ever touched.
        let ids: Vec<String> = {
            let mut st = conn.prepare(
                "SELECT id FROM sessions
                  WHERE id NOT IN (SELECT ref_id FROM search_index WHERE kind = 'session')",
            )?;
            let mapped = st.query_map([], |r| r.get::<_, String>(0))?;
            mapped.collect::<std::result::Result<Vec<_>, _>>()?
        };
        for id in ids {
            index_session_conn(&conn, &id)?;
        }
        Ok(())
    }

    // helpers

    pub fn touch_project(&self, project_id: &str) -> Result<()> {
        let conn = self.write();
        conn.execute(
            "UPDATE projects SET updated_at = ?2 WHERE id = ?1",
            params![project_id, now()],
        )?;
        Ok(())
    }

    /// Sources whose facts are already fully stored: source_path →
    /// (source_file_identity, last_seen_size, mtime) for members whose owning
    /// session has a title. Discovery uses this to skip re-parsing sources that
    /// cannot have changed, so a reconcile pass costs O(changed sources). Only
    /// titled sessions are listed: an untitled row may just predate a title
    /// source, and re-parsing its unchanged file is what heals it.
    pub fn session_source_skipset(
        &self,
    ) -> Result<std::collections::HashMap<String, (String, i64, Option<f64>)>> {
        let conn = self.read();
        let mut st = conn.prepare(
            "SELECT source_path, source_file_identity, source_last_seen_size, source_mtime
             FROM sessions
             WHERE title IS NOT NULL AND source_file_identity != ''",
        )?;
        let rows = st
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    (
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, Option<f64>>(3)?,
                    ),
                ))
            })?
            .collect::<std::result::Result<std::collections::HashMap<_, _>, _>>()?;
        Ok(rows)
    }
}

// Connection-level helpers. Free functions over `&Connection` so `Db` methods
// and `Db::tx` closures share exactly the same SQL paths.

pub fn upsert_project_conn(conn: &Connection, p: &Project) -> Result<()> {
    conn.execute(
        "INSERT INTO projects (id, name, description, git_id, name_customized, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
           -- A customized name is user intent: no automatic rename (worktree
           -- discovery, Git upgrade, Project merge) may overwrite it.
           name = CASE WHEN name_customized = 1 THEN name ELSE ?2 END,
           description = ?3,
           -- Git identity is never cleared *or re-targeted* by a whole-object
           -- write; the one legitimate writer is
           -- `workspace::project::adopt_git_identity_conn` (first-set-only).
           git_id = COALESCE(git_id, ?4),
           name_customized = MAX(name_customized, ?5),
            updated_at = ?7",
        params![
            p.id,
            p.name,
            p.description,
            p.git_id,
            p.name_customized as i64,
            p.created_at,
            p.updated_at
        ],
    )?;
    Ok(())
}

pub fn upsert_workstream_conn(conn: &Connection, w: &Workstream) -> Result<()> {
    // A Session's search document carries its Owner's title, so a rename has
    // to reach every Session that owns this Workstream. The previous title is
    // read first: an update that changed nothing else must not rewrite N docs.
    let previous: Option<(String, String)> = conn
        .query_row(
            "SELECT title, description FROM workstreams WHERE id = ?1",
            params![w.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    conn.execute(
        "INSERT INTO workstreams (id, title, description, visibility, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(id) DO UPDATE SET
           title = ?2, description = ?3, visibility = ?4, updated_at = ?6",
        params![
            w.id,
            w.title,
            w.description,
            w.visibility,
            w.created_at,
            w.updated_at
        ],
    )?;
    // The two-level revision state: a Workstream starts as (0, 1, 0) so its
    // first explicit generation is allowed, and any title / description
    // movement is an INPUT change the next synthesis must consume.
    let text_changed = match &previous {
        Some((t, d)) => t != &w.title || d != &w.description,
        None => false,
    };
    crate::storage::context_repo::ensure_workstream_state_conn(conn, &w.id)?;
    if text_changed {
        crate::storage::bump_input_revision_conn(conn, &w.id)?;
    }
    conn.execute(
        "INSERT OR IGNORE INTO workstream_review_state (workstream_id, reviewed_through_at, reviewed_boundary_change_ids, reviewed_at)
         VALUES (?1, ?2, '[]', ?2)",
        params![w.id, w.created_at],
    )?;
    if text_changed {
        reindex_owned_sessions_conn(conn, &w.id)?;
    }
    Ok(())
}

/// Set (or clear) a Session's single Owner Workstream through a caller-held
/// connection, so a launch match can write ownership together with everything
/// that must agree with it in ONE transaction.
///
/// The target Workstream must exist — a Session pointing at a missing row
/// would be a silently broken owner — and the search document is refreshed in
/// the same connection, or the Session stays searchable under its old label.
pub fn set_session_owner_conn(
    conn: &Connection,
    session_id: &str,
    workstream_id: Option<&str>,
) -> Result<()> {
    if let Some(id) = workstream_id {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM workstreams WHERE id = ?1)",
            params![id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(other(format!("未知 Workstream: {id}")));
        }
    }
    let previous_owner: Option<String> = conn
        .query_row(
            "SELECT COALESCE(owner_workstream_id, '') FROM sessions WHERE id = ?1",
            params![session_id],
            |r| r.get(0),
        )
        .optional()?
        .filter(|s: &String| !s.is_empty());
    let changed = conn.execute(
        "UPDATE sessions SET owner_workstream_id = ?2 WHERE id = ?1",
        params![session_id, workstream_id],
    )?;
    if changed == 0 {
        return Err(other(format!("未知 Session: {session_id}")));
    }
    // An Owner-set change alters BOTH Workstreams' input: the one gaining the
    // Session and the one losing it. Either way the next synthesis must run.
    if let Some(ws) = workstream_id {
        crate::storage::bump_input_revision_conn(conn, ws)?;
    }
    if let Some(old) = previous_owner {
        if Some(old.as_str()) != workstream_id {
            crate::storage::bump_input_revision_conn(conn, &old)?;
        }
    }
    index_session_conn(conn, session_id)
}

/// Read one LaunchIntent through a caller-held connection — inside a matching
/// transaction the *current* row decides, never the copy read before it.
pub fn get_launch_intent_conn(conn: &Connection, id: &str) -> Result<Option<LaunchIntent>> {
    Ok(conn
        .query_row(
            "SELECT id, launch_type, agent, owner_workstream_id, cwd, process_id, launched_at, matched_session_id, status, note, created_at, updated_at
             FROM launch_intents WHERE id = ?1",
            params![id],
            row_launch_intent,
        )
        .optional()?)
}

/// Consume a LaunchIntent's match, exactly once: the row must still be waiting
/// (PENDING or AMBIGUOUS) and unclaimed. Returns false when someone else got
/// there first — the caller must abandon its own match rather than overwrite
/// theirs: a plain `UPDATE … WHERE id` would let both succeed.
pub fn mark_launch_intent_matched_conn(
    conn: &Connection,
    id: &str,
    session_id: &str,
    note: &str,
) -> Result<bool> {
    let updated = conn.execute(
        "UPDATE launch_intents
            SET status = ?2, matched_session_id = ?3, note = ?4, updated_at = ?5
          WHERE id = ?1
            AND status IN (?6, ?7)
            AND matched_session_id IS NULL",
        params![
            id,
            launch_status::MATCHED,
            session_id,
            note,
            now(),
            launch_status::PENDING,
            launch_status::AMBIGUOUS
        ],
    )?;
    Ok(updated == 1)
}

/// Move an intent that is still WAITING (PENDING/AMBIGUOUS, unclaimed) to
/// another status, and report whether it still was. Every transition that is not
/// the match itself goes through here (parking a tie as AMBIGUOUS, expiring a
/// stale intent, recording the pid note after spawning). None of them may
/// overwrite a match: "still waiting" written over an already-consumed intent
/// would make it consumable a second time.
pub fn update_waiting_launch_intent_conn(
    conn: &Connection,
    id: &str,
    status: &str,
    note: &str,
) -> Result<bool> {
    let updated = conn.execute(
        "UPDATE launch_intents
            SET status = ?2, note = ?3, updated_at = ?4
          WHERE id = ?1
            AND status IN (?5, ?6)
            AND matched_session_id IS NULL",
        params![
            id,
            status,
            note,
            now(),
            launch_status::PENDING,
            launch_status::AMBIGUOUS
        ],
    )?;
    Ok(updated == 1)
}

/// Rebuild the search documents of the Sessions that OWN `workstream_id`: a
/// Session's document embeds its Owner Workstream's title, so a rename or an
/// ownership change (a Workstream deletion nulling it) would otherwise leave
/// them describing a fact that is no longer true. Runs in the caller's
/// transaction.
pub fn reindex_owned_sessions_conn(conn: &Connection, workstream_id: &str) -> Result<()> {
    let mut ids: Vec<String> = Vec::new();
    {
        let mut st = conn.prepare("SELECT id FROM sessions WHERE owner_workstream_id = ?1")?;
        for row in st.query_map(params![workstream_id], |r| r.get(0))? {
            ids.push(row?);
        }
    }
    for id in ids {
        index_session_conn(conn, &id)?;
    }
    Ok(())
}

/// Rebuild the search documents of the Sessions projecting onto `project_id` —
/// the same rule as [`reindex_owned_sessions_conn`] for the Project name.
pub fn reindex_sessions_for_project_conn(conn: &Connection, project_id: &str) -> Result<()> {
    let mut ids: Vec<String> = Vec::new();
    {
        let mut st = conn.prepare("SELECT id FROM sessions WHERE project_id = ?1")?;
        for row in st.query_map(params![project_id], |r| r.get(0))? {
            ids.push(row?);
        }
    }
    for id in ids {
        index_session_conn(conn, &id)?;
    }
    Ok(())
}

/// Write (or refresh) one Session's search document: title is the Session title
/// (falling back to the Agent name), body is the Project name plus the ONE Owner
/// Workstream title (a Session has a single Owner) and the cwd.
pub fn index_session_conn(conn: &rusqlite::Connection, session_id: &str) -> Result<()> {
    let row: Option<(String, String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT COALESCE(s.title, s.agent), COALESCE(p.name, ''), w.title, s.cwd
               FROM sessions s
               LEFT JOIN projects p ON p.id = s.project_id
               LEFT JOIN workstreams w ON w.id = s.owner_workstream_id
              WHERE s.id = ?1",
            params![session_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((title, project_name, owner_title, cwd)) = row else {
        return Ok(());
    };
    unindex_conn(conn, "session", session_id);
    let body = format!(
        "{}\n{}\n{}",
        project_name,
        owner_title.unwrap_or_default(),
        cwd.unwrap_or_default()
    );
    conn.execute(
        "INSERT INTO search_index (kind, ref_id, parent_id, title, body) VALUES ('session', ?1, '', ?2, ?3)",
        params![session_id, title, body],
    )?;
    Ok(())
}

/// Insert or refresh one execution member through a caller-held connection, so a
/// graph-resolution pass can create members in the transaction that creates the
/// Session they attach to.
#[allow(clippy::too_many_arguments)]
/// The CURRENT conversation, joined through the projection. `after_ordinal`
/// is a projection ordinal; the rows come back in projection order.
/// One page of the CURRENT conversation: its messages plus the cursor, total
/// and fact generation they were read under.
pub struct MessageWindow {
    pub messages: Vec<WindowedMessage>,
    pub generation: i64,
    /// How many messages the conversation holds in total — every projected
    /// message, intermediates included. The reader's header counts with this;
    /// paging itself runs over the skeleton.
    pub total: i64,
    /// The max projection ordinal of the conversation: "am I holding the tail"
    /// compares against this, never against `total`.
    pub tail_ordinal: i64,
    /// How many skeleton rows sit above the loaded range (the 「加载更早」
    /// label): exact, jumps included.
    pub remaining: i64,
    /// Exclusive upper bound for the next, older page. `None` means the
    /// conversation's beginning was reached.
    pub next_before_ordinal: Option<i64>,
}

/// A message of a window together with the projection ordinal that orders it:
/// the conversation's order is the ordinal, and a reader that merges pages
/// orders by it instead of trusting the order the pages arrived in.
pub struct WindowedMessage {
    pub ordinal: i64,
    pub message: SessionMessage,
    /// Final replies only: the turn's collapsible intermediate block —
    /// everything between this reply and the user message that started the
    /// turn. `None` when there is nothing to collapse.
    pub turn: Option<TurnSummary>,
}

/// One turn's collapsible intermediates.
#[derive(Clone)]
pub struct TurnSummary {
    /// The turn's user message — the exclusive lower bound of the range. `0`
    /// means the turn opens at the conversation's very beginning.
    pub boundary_ordinal: i64,
    /// That user message's ts; the block header derives a duration from it.
    pub boundary_ts: Option<String>,
    /// How many messages sit in the range. Never zero: zero-count turns get
    /// no block at all.
    pub count: i64,
}

/// Where one USER message sits in the conversation — a mark on the reader's
/// navigation rail.
pub struct MessageMark {
    pub ordinal: i64,
    /// The first line of the message, for the mark's tooltip.
    pub preview: String,
}

const MARK_PREVIEW_CHARS: usize = 80;

/// The projection window ending at `before_ordinal` (inclusive), newest first
/// in SQL and oldest first on return, paired with each row's ordinal.
fn window_messages_conn(
    conn: &Connection,
    session_id: &str,
    before_ordinal: Option<i64>,
    limit: i64,
) -> Result<Vec<(i64, SessionMessage)>> {
    window_messages_filtered_conn(conn, session_id, before_ordinal, limit, false)
}

/// `turns_only` keeps just the messages that structure a conversation — user
/// turns and each turn's FINAL assistant reply — hiding the assistant's
/// intermediate (non-final) outputs.
fn window_messages_filtered_conn(
    conn: &Connection,
    session_id: &str,
    before_ordinal: Option<i64>,
    limit: i64,
    turns_only: bool,
) -> Result<Vec<(i64, SessionMessage)>> {
    let turn_filter = if turns_only {
        " AND (m.role = 'user' OR m.turn_final = 1)"
    } else {
        ""
    };
    let mut st = conn.prepare(&format!(
        "SELECT p.ordinal AS ordinal, m.*
         FROM session_message_projection p
         JOIN session_messages m ON m.id = p.session_message_id
         WHERE p.session_id = ?1 AND p.ordinal <= ?2{turn_filter}
         ORDER BY p.ordinal DESC LIMIT ?3",
    ))?;
    let upper = before_ordinal.unwrap_or(i64::MAX);
    let mut rows = st
        .query_map(params![session_id, upper, limit], |r| {
            Ok((r.get::<_, i64>("ordinal")?, row_message(r)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    rows.reverse();
    Ok(rows)
}

/// The projection window starting after `after_ordinal`, oldest first — the
/// forward half of paging.
fn forward_messages_conn(
    conn: &Connection,
    session_id: &str,
    after_ordinal: i64,
    limit: i64,
) -> Result<Vec<(i64, SessionMessage)>> {
    forward_messages_filtered_conn(conn, session_id, after_ordinal, limit, false)
}

fn forward_messages_filtered_conn(
    conn: &Connection,
    session_id: &str,
    after_ordinal: i64,
    limit: i64,
    turns_only: bool,
) -> Result<Vec<(i64, SessionMessage)>> {
    let turn_filter = if turns_only {
        " AND (m.role = 'user' OR m.turn_final = 1)"
    } else {
        ""
    };
    let mut st = conn.prepare(&format!(
        "SELECT p.ordinal AS ordinal, m.*
         FROM session_message_projection p
         JOIN session_messages m ON m.id = p.session_message_id
         WHERE p.session_id = ?1 AND p.ordinal > ?2{turn_filter}
         ORDER BY p.ordinal ASC LIMIT ?3",
    ))?;
    let rows = st
        .query_map(params![session_id, after_ordinal, limit], |r| {
            Ok((r.get::<_, i64>("ordinal")?, row_message(r)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Assemble a page from rows read in either direction. Both directions carry
/// the same backward cursor — one step before the page they came with — so a
/// reader paging up never handles projection ordinals itself.
///
/// The page is the conversation's SKELETON; the assistant's intermediate
/// outputs ride along as each final reply's [`TurnSummary`].
fn message_window_from(
    state: (i64, i64),
    conn: &Connection,
    session_id: &str,
    rows: Vec<(i64, SessionMessage)>,
) -> Result<MessageWindow> {
    let next_before_ordinal = rows
        .first()
        .map(|(ordinal, _)| ordinal - 1)
        .filter(|cursor| *cursor > 0);
    let remaining = match next_before_ordinal {
        None => 0,
        Some(before) => {
            let mut st = conn.prepare(
                "SELECT COUNT(*) FROM session_message_projection p
                 JOIN session_messages m ON m.id = p.session_message_id
                 WHERE p.session_id = ?1 AND p.ordinal < ?2
                   AND (m.role = 'user' OR m.turn_final = 1)",
            )?;
            st.query_row(params![session_id, before], |r| r.get::<_, i64>(0))?
        }
    };
    let turns = turn_summaries_conn(conn, session_id, &rows)?;
    Ok(MessageWindow {
        messages: rows
            .into_iter()
            .map(|(ordinal, message)| {
                let turn = if message.turn_final {
                    turns.get(&ordinal).cloned()
                } else {
                    None
                };
                WindowedMessage {
                    ordinal,
                    message,
                    turn,
                }
            })
            .collect(),
        generation: state.0,
        total: state.1,
        tail_ordinal: state.1,
        remaining,
        next_before_ordinal,
    })
}

/// Per final reply in the page: the turn it closes. The turn's user message is
/// the last user row below the reply — a final's predecessor skeleton row is
/// always a user, because a final is derived as an assistant whose next
/// projected row is not an assistant, so two finals never touch.
fn turn_summaries_conn(
    conn: &Connection,
    session_id: &str,
    rows: &[(i64, SessionMessage)],
) -> Result<HashMap<i64, TurnSummary>> {
    let mut boundary_st = conn.prepare(
        "SELECT p.ordinal, m.ts
         FROM session_message_projection p
         JOIN session_messages m ON m.id = p.session_message_id
         WHERE p.session_id = ?1 AND p.ordinal < ?2 AND m.role = 'user'
         ORDER BY p.ordinal DESC LIMIT 1",
    )?;
    let mut count_st = conn.prepare(
        "SELECT COUNT(*) FROM session_message_projection p
         WHERE p.session_id = ?1 AND p.ordinal > ?2 AND p.ordinal < ?3",
    )?;
    let mut turns = HashMap::new();
    for &(ordinal, ref message) in rows {
        if !message.turn_final {
            continue;
        }
        let (boundary_ordinal, boundary_ts) = boundary_st
            .query_row(params![session_id, ordinal], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))
            })
            .optional()?
            .unwrap_or((0, None));
        let count: i64 =
            count_st.query_row(params![session_id, boundary_ordinal, ordinal], |r| r.get(0))?;
        if count > 0 {
            turns.insert(
                ordinal,
                TurnSummary {
                    boundary_ordinal,
                    boundary_ts,
                    count,
                },
            );
        }
    }
    Ok(turns)
}

pub fn get_messages_conn(
    conn: &Connection,
    session_id: &str,
    after_ordinal: Option<i64>,
    limit: i64,
) -> Result<Vec<SessionMessage>> {
    Ok(
        forward_messages_conn(conn, session_id, after_ordinal.unwrap_or(0), limit)?
            .into_iter()
            .map(|(_, message)| message)
            .collect(),
    )
}

/// The stable diagnostic identity of an unattachable member: one row
/// per (agent, kind, source member), so repeat sightings update instead of
/// duplicating.

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
        "INSERT INTO context_item_revisions (id, item_id, title, content, metadata, source_type, source_ref, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![r.id, r.item_id, r.title, r.content, r.metadata.to_string(), r.source_type, r.source_ref, r.created_at],
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
            },
            "provenance": {
                "authority": if actor == "user" { "user_edit" } else { "system_observed" },
                "actor": actor,
                "source_type": "status_change",
                "source_ref": source_refs.first(),
            }
        }),
        source_type: Some("status_change".into()),
        source_ref: source_refs.first().cloned(),
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
            "SELECT id, item_id, title, content, metadata, source_type, source_ref, created_at
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
                r.id, r.item_id, r.title, r.content, r.metadata, r.source_type, r.source_ref, r.created_at
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

pub fn set_item_authority_conn(conn: &Connection, item_id: &str, authority: &str) -> Result<()> {
    conn.execute(
        "UPDATE context_items SET authority = ?2, updated_at = ?3 WHERE id = ?1",
        params![item_id, authority, now()],
    )?;
    Ok(())
}

pub fn insert_conflict_conn(conn: &Connection, c: &ContextConflict) -> Result<()> {
    conn.execute(
        "INSERT INTO context_conflicts (id, workstream_id, left_item_id, right_item_id, conflict_type, status, resolution, created_at, updated_at,
                                        left_revision_id, right_revision_id, candidate_snapshot_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            c.id,
            c.workstream_id,
            c.left_item_id,
            c.right_item_id,
            c.conflict_type,
            c.status,
            c.resolution,
            c.created_at,
            c.updated_at,
            c.left_revision_id,
            c.right_revision_id,
            c.candidate_snapshot_json,
        ],
    )?;
    Ok(())
}

pub fn conflicts_for_workstream_conn(
    conn: &Connection,
    workstream_id: &str,
    include_closed: bool,
) -> Result<Vec<ContextConflict>> {
    let filter = if include_closed {
        ""
    } else {
        " AND status = 'open'"
    };
    let sql = format!(
        "SELECT id, workstream_id, left_item_id, right_item_id, conflict_type, status, resolution, created_at, updated_at,
                left_revision_id, right_revision_id, candidate_snapshot_json
         FROM context_conflicts WHERE workstream_id = ?1{} ORDER BY created_at DESC",
        filter
    );
    let mut st = conn.prepare(&sql)?;
    let rows = st
        .query_map(params![workstream_id], row_conflict)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn get_conflict_conn(conn: &Connection, conflict_id: &str) -> Result<Option<ContextConflict>> {
    Ok(conn
        .query_row(
            "SELECT id, workstream_id, left_item_id, right_item_id, conflict_type, status, resolution, created_at, updated_at,
                    left_revision_id, right_revision_id, candidate_snapshot_json
             FROM context_conflicts WHERE id = ?1",
            params![conflict_id],
            row_conflict,
        )
        .optional()?)
}

/// Authoritative resolver for revision authority: provenance authority stored in
/// the revision metadata, else audit actor / source_type inference, else
/// [`authority::UNKNOWN`]. NEVER falls back to the mutable `item.authority`.
pub fn resolve_revision_authority(rev: &ContextItemRevision) -> String {
    if let Some(auth) = rev
        .metadata
        .get("provenance")
        .and_then(|p| p.get("authority"))
        .and_then(|a| a.as_str())
    {
        return auth.to_string();
    }
    if let Some(audit) = rev.metadata.get("audit") {
        if let Some(actor) = audit.get("actor").and_then(|a| a.as_str()) {
            if actor == "user" {
                return crate::domain::authority::USER_EDIT.into();
            }
        }
    }
    if rev.source_type.as_deref() == Some("user_edit") {
        crate::domain::authority::USER_EDIT.into()
    } else if rev.source_type.as_deref() == Some("session_message") {
        crate::domain::authority::AGENT_STATEMENT.into()
    } else {
        crate::domain::authority::UNKNOWN.into()
    }
}

pub fn revision_snapshot_from_rev(rev: &ContextItemRevision) -> RevisionSnapshot {
    RevisionSnapshot {
        id: rev.id.clone(),
        item_id: rev.item_id.clone(),
        title: rev.title.clone(),
        content: rev.content.clone(),
        authority: resolve_revision_authority(rev),
        created_at: rev.created_at.clone(),
        source_ref: rev.source_ref.clone(),
        source_type: rev.source_type.clone(),
    }
}

pub fn build_conflict_review_case_conn(
    conn: &Connection,
    conflict: ContextConflict,
) -> Result<ConflictReviewCase> {
    let current_left_item = get_item_conn(conn, &conflict.left_item_id)?;
    let current_left = current_left_item
        .as_ref()
        .and_then(|i| i.current_revision_id.as_deref())
        .and_then(|rid| get_revision_conn(conn, rid).ok().flatten())
        .map(|rev| revision_snapshot_from_rev(&rev));

    let left_at_conflict = conflict
        .left_revision_id
        .as_deref()
        .and_then(|rid| get_revision_conn(conn, rid).ok().flatten())
        .map(|rev| revision_snapshot_from_rev(&rev))
        .or_else(|| current_left.clone());

    let left_changed_since_conflict = match (&left_at_conflict, &current_left) {
        (Some(frozen), Some(curr)) => frozen.id != curr.id,
        _ => false,
    };

    let current_right_item = conflict
        .right_item_id
        .as_deref()
        .and_then(|rid| get_item_conn(conn, rid).ok().flatten());
    let current_right = current_right_item
        .as_ref()
        .and_then(|i| i.current_revision_id.as_deref())
        .and_then(|rid| get_revision_conn(conn, rid).ok().flatten())
        .map(|rev| revision_snapshot_from_rev(&rev));

    let right_at_conflict = conflict
        .right_revision_id
        .as_deref()
        .and_then(|rid| get_revision_conn(conn, rid).ok().flatten())
        .map(|rev| revision_snapshot_from_rev(&rev))
        .or_else(|| current_right.clone());

    let right_changed_since_conflict = match (&right_at_conflict, &current_right) {
        (Some(frozen), Some(curr)) => frozen.id != curr.id,
        _ => false,
    };

    let candidate_at_conflict = conflict
        .candidate_snapshot_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<CandidateSnapshot>(s).ok());

    Ok(ConflictReviewCase {
        conflict,
        left_at_conflict,
        right_at_conflict,
        candidate_at_conflict,
        current_left,
        current_right,
        left_changed_since_conflict,
        right_changed_since_conflict,
    })
}

pub fn get_conflict_review_case_conn(
    conn: &Connection,
    conflict_id: &str,
) -> Result<Option<ConflictReviewCase>> {
    let Some(conflict) = get_conflict_conn(conn, conflict_id)? else {
        return Ok(None);
    };
    Ok(Some(build_conflict_review_case_conn(conn, conflict)?))
}

pub fn list_conflict_review_cases_conn(
    conn: &Connection,
    workstream_id: &str,
    include_closed: bool,
) -> Result<Vec<ConflictReviewCase>> {
    let conflicts = conflicts_for_workstream_conn(conn, workstream_id, include_closed)?;
    let mut cases = Vec::with_capacity(conflicts.len());
    for c in conflicts {
        cases.push(build_conflict_review_case_conn(conn, c)?);
    }
    Ok(cases)
}

// Row mappers. Name-based (`r.get("col")`) wherever a struct is wide or
// growing: a positional mapper silently re-types every field after it when
// someone adds a column to the SELECT list.

fn row_project(r: &Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: r.get("id")?,
        name: r.get("name")?,
        description: r.get("description")?,
        git_id: r.get("git_id")?,
        name_customized: r.get::<_, i64>("name_customized")? != 0,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
    })
}

fn row_workstream(r: &Row) -> rusqlite::Result<Workstream> {
    Ok(Workstream {
        id: r.get("id")?,
        title: r.get("title")?,
        description: r.get("description")?,
        visibility: r.get("visibility")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
    })
}

fn row_session(r: &Row) -> rusqlite::Result<Session> {
    Ok(Session {
        id: r.get("id")?,
        agent: Agent::parse(&r.get::<_, String>("agent")?).unwrap_or(Agent::Codex),
        root_agent_session_id: r.get("root_agent_session_id")?,
        title: r.get("title")?,
        cwd: r.get("cwd")?,
        workspace_path_id: r.get("workspace_path_id")?,
        project_id: r.get("project_id")?,
        owner_workstream_id: r.get("owner_workstream_id")?,
        forked_from_session_id: r.get("forked_from_session_id")?,
        started_at: r.get("started_at")?,
        last_activity_at: r.get("last_activity_at")?,
        last_conversation_at: r.get("last_conversation_at")?,
        archived_at: r.get("archived_at")?,
        source_kind: r.get("source_kind")?,
        source_path: r.get("source_path")?,
        metadata: serde_json::from_str(&r.get::<_, String>("metadata")?).unwrap_or_default(),
        source_file_identity: r.get("source_file_identity")?,
        source_generation: r.get("source_generation")?,
        source_byte_offset: r.get::<_, i64>("source_byte_offset")?.max(0) as u64,
        source_last_seen_size: r.get::<_, i64>("source_last_seen_size")?.max(0) as u64,
        source_mtime: r.get("source_mtime")?,
        source_prefix_hash: r.get("source_prefix_hash")?,
        source_tail_hash: r.get("source_tail_hash")?,
        fact_generation: r.get("fact_generation")?,
        latest_message_seq: r.get("latest_message_seq")?,
    })
}

fn row_message(r: &Row) -> rusqlite::Result<SessionMessage> {
    Ok(SessionMessage {
        id: r.get("id")?,
        session_id: r.get("session_id")?,
        sequence: r.get("sequence")?,
        source_message_id: r.get("source_message_id")?,
        source_generation: r.get("source_generation")?,
        source_position: r.get("source_position")?,
        source_identity_hash: r.get("source_identity_hash")?,
        ts: r.get("ts")?,
        // The CHECK constraint only lets 'user' | 'assistant' through.
        role: if r.get::<_, String>("role")? == "assistant" {
            SessionMessageRole::Assistant
        } else {
            SessionMessageRole::User
        },
        content: r.get("content")?,
        turn_final: r.get::<_, i64>("turn_final")? != 0,
        raw_ref: r.get("raw_ref")?,
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
        created_at: r.get(base + 7)?,
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
        left_revision_id: r.get(9)?,
        right_revision_id: r.get(10)?,
        candidate_snapshot_json: r.get(11)?,
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
        owner_workstream_id: r.get(3)?,
        cwd: r.get(4)?,
        process_id: r.get::<_, Option<i64>>(5)?.map(|p| p as u32),
        launched_at: r.get(6)?,
        matched_session_id: r.get(7)?,
        status: r.get(8)?,
        note: r.get(9)?,
        created_at: r.get(10)?,
        updated_at: r.get(10)?,
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
    /// Board scope defaults to unarchived; association queries include both.
    pub scope: SessionListScope,
}

pub fn ensure_not_empty(name: &str, v: &str) -> Result<()> {
    if v.trim().is_empty() {
        Err(other(format!("{} 不能为空", name)))
    } else {
        Ok(())
    }
}

pub fn item_relations_for_workstream_conn(
    conn: &Connection,
    workstream_id: &str,
) -> Result<Vec<ContextItemRelation>> {
    let items = items_for_workstream_conn(conn, workstream_id, true)?;
    let mut ref_map: std::collections::HashMap<String, ContextItemRef> =
        std::collections::HashMap::new();
    for (item, rev) in &items {
        ref_map.insert(
            item.id.clone(),
            ContextItemRef {
                id: item.id.clone(),
                kind: item.kind.clone(),
                title: rev.title.clone(),
                status: item.status.clone(),
            },
        );
    }

    let mut relations = Vec::with_capacity(items.len());
    for (item, _) in &items {
        let supersedes = item
            .supersedes_item_id
            .as_ref()
            .and_then(|sid| ref_map.get(sid).cloned());
        let superseded_by: Vec<ContextItemRef> = items
            .iter()
            .filter(|(other, _)| other.supersedes_item_id.as_deref() == Some(&item.id))
            .filter_map(|(other, _)| ref_map.get(&other.id).cloned())
            .collect();

        relations.push(ContextItemRelation {
            item_id: item.id.clone(),
            supersedes,
            superseded_by,
        });
    }

    Ok(relations)
}

pub fn normalize_change_actor(actor: &str) -> String {
    let lower = actor.to_lowercase();
    match lower.as_str() {
        "user" => "User".into(),
        "agent" => "Agent".into(),
        "system" => "System".into(),
        a if a.starts_with("sync:") => "Agent".into(),
        _ => actor.to_string(),
    }
}

fn parse_revision_context_change(
    id: String,
    item_id: String,
    title: String,
    metadata_str: String,
    source_type: Option<String>,
    created_at: String,
    created_by: String,
    first_created_at: Option<String>,
) -> ContextChange {
    let is_first = first_created_at.as_deref() == Some(&created_at);
    let metadata_json: serde_json::Value =
        serde_json::from_str(&metadata_str).unwrap_or(serde_json::Value::Null);

    let (kind, actor) = if source_type.as_deref() == Some("status_change") {
        let audit = metadata_json.get("audit");
        let new_status = audit
            .and_then(|a| a.get("new_status"))
            .and_then(|s| s.as_str())
            .unwrap_or("");
        let actor_str = audit
            .and_then(|a| a.get("actor"))
            .and_then(|s| s.as_str())
            .unwrap_or("user");
        let kind = match new_status {
            "resolved" => "resolved",
            "superseded" => "superseded",
            "deleted" | "obsolete" => "deleted",
            _ => "edited",
        };
        (kind.to_string(), normalize_change_actor(actor_str))
    } else {
        let prov_actor = metadata_json
            .get("provenance")
            .and_then(|p| p.get("actor"))
            .and_then(|a| a.as_str());
        let actor = if let Some(a) = prov_actor {
            normalize_change_actor(a)
        } else if source_type.as_deref() == Some("user_edit") || created_by == "user" {
            "User".to_string()
        } else if created_by.starts_with("sync:")
            || created_by == "agent"
            || source_type.as_deref() == Some("session_message")
        {
            "Agent".to_string()
        } else {
            "System".to_string()
        };
        let kind = if is_first { "added" } else { "edited" };
        (kind.to_string(), actor)
    };

    ContextChange {
        id,
        item_id: Some(item_id),
        conflict_id: None,
        kind,
        title,
        actor,
        source_type,
        created_at,
    }
}

pub fn is_review_relevant(change: &ContextChange) -> bool {
    if change.kind == "conflict_created" {
        return true;
    }
    if change.source_type.as_deref() == Some("conflict") {
        return false;
    }
    let actor_lower = change.actor.to_lowercase();
    if actor_lower == "user" {
        return false;
    }
    actor_lower == "agent" || actor_lower == "system"
}

pub fn list_workstream_context_changes_conn(
    conn: &Connection,
    workstream_id: &str,
    limit: usize,
) -> Result<Vec<ContextChange>> {
    let limit = if limit == 0 { 20 } else { limit };
    let mut changes = Vec::new();

    // Item revisions
    let sql_revs = "SELECT r.id, r.item_id, r.title, r.metadata, r.source_type, r.created_at,
                           i.authority, i.created_by,
                           (SELECT MIN(r2.created_at) FROM context_item_revisions r2 WHERE r2.item_id = r.item_id) AS first_created_at
                    FROM context_item_revisions r
                    JOIN context_items i ON i.id = r.item_id
                    WHERE i.workstream_id = ?1
                    ORDER BY r.created_at DESC
                    LIMIT ?2";
    let mut st = conn.prepare(sql_revs)?;
    let rev_rows = st.query_map(params![workstream_id, limit as i64], |r| {
        let id: String = r.get(0)?;
        let item_id: String = r.get(1)?;
        let title: String = r.get(2)?;
        let metadata_str: String = r.get(3)?;
        let source_type: Option<String> = r.get(4)?;
        let created_at: String = r.get(5)?;
        let authority: String = r.get(6)?;
        let created_by: String = r.get(7)?;
        let first_created_at: Option<String> = r.get(8)?;
        Ok((
            id,
            item_id,
            title,
            metadata_str,
            source_type,
            created_at,
            authority,
            created_by,
            first_created_at,
        ))
    })?;

    for row in rev_rows {
        let (
            id,
            item_id,
            title,
            metadata_str,
            source_type,
            created_at,
            _authority,
            created_by,
            first_created_at,
        ) = row?;

        changes.push(parse_revision_context_change(
            id,
            item_id,
            title,
            metadata_str,
            source_type,
            created_at,
            created_by,
            first_created_at,
        ));
    }

    // Conflicts
    let sql_conflicts = "SELECT id, left_item_id, right_item_id, status, created_at, updated_at
                         FROM context_conflicts
                         WHERE workstream_id = ?1
                         ORDER BY created_at DESC
                         LIMIT ?2";
    let mut st = conn.prepare(sql_conflicts)?;
    let conflict_rows = st.query_map(params![workstream_id, limit as i64], |r| {
        let id: String = r.get(0)?;
        let left_item_id: String = r.get(1)?;
        let right_item_id: Option<String> = r.get(2)?;
        let status: String = r.get(3)?;
        let created_at: String = r.get(4)?;
        let updated_at: String = r.get(5)?;
        Ok((
            id,
            left_item_id,
            right_item_id,
            status,
            created_at,
            updated_at,
        ))
    })?;

    for row in conflict_rows {
        let (id, left_item_id, _right_item_id, _status, created_at, _updated_at) = row?;
        changes.push(ContextChange {
            id: format!("conflict-created-{}", id),
            item_id: Some(left_item_id),
            conflict_id: Some(id),
            kind: "conflict_created".into(),
            title: "发现潜在 Context 冲突".into(),
            actor: "Agent".into(),
            source_type: Some("conflict".into()),
            created_at,
        });
    }

    // Conflict resolution events
    let sql_conflict_events =
        "SELECT e.id, e.conflict_id, c.left_item_id, e.new_status, e.actor, e.created_at
                               FROM context_conflict_events e
                               JOIN context_conflicts c ON c.id = e.conflict_id
                               WHERE c.workstream_id = ?1
                               ORDER BY e.created_at DESC
                               LIMIT ?2";
    if let Ok(mut st) = conn.prepare(sql_conflict_events) {
        let event_rows = st.query_map(params![workstream_id, limit as i64], |r| {
            let id: String = r.get(0)?;
            let conflict_id: String = r.get(1)?;
            let left_item_id: String = r.get(2)?;
            let new_status: String = r.get(3)?;
            let actor: String = r.get(4)?;
            let created_at: String = r.get(5)?;
            Ok((id, conflict_id, left_item_id, new_status, actor, created_at))
        })?;

        for row in event_rows {
            let (id, conflict_id, left_item_id, new_status, actor, created_at) = row?;
            let title = if new_status == "resolved" {
                "冲突已解决".to_string()
            } else if new_status == "dismissed" {
                "冲突已忽略".to_string()
            } else {
                format!("冲突状态更新为 {}", new_status)
            };
            changes.push(ContextChange {
                id: format!("conflict-event-{}", id),
                item_id: Some(left_item_id),
                conflict_id: Some(conflict_id),
                kind: "conflict_resolved".into(),
                title,
                actor: normalize_change_actor(&actor),
                source_type: Some("conflict_resolution".into()),
                created_at,
            });
        }
    }

    changes.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    changes.truncate(limit);

    Ok(changes)
}

pub fn list_workstream_review_changes_conn(
    conn: &Connection,
    workstream_id: &str,
    frontier: &ReviewFrontier,
) -> Result<Vec<ContextChange>> {
    let mut unseen_changes = Vec::new();

    // Item revisions
    let sql_revs = "SELECT r.id, r.item_id, r.title, r.metadata, r.source_type, r.created_at,
                           i.authority, i.created_by,
                           (SELECT MIN(r2.created_at) FROM context_item_revisions r2 WHERE r2.item_id = r.item_id) AS first_created_at
                    FROM context_item_revisions r
                    JOIN context_items i ON i.id = r.item_id
                    WHERE i.workstream_id = ?1 AND r.created_at >= ?2
                    ORDER BY r.created_at ASC";
    let mut st = conn.prepare(sql_revs)?;
    let rev_rows = st.query_map(params![workstream_id, frontier.through_at], |r| {
        let id: String = r.get(0)?;
        let item_id: String = r.get(1)?;
        let title: String = r.get(2)?;
        let metadata_str: String = r.get(3)?;
        let source_type: Option<String> = r.get(4)?;
        let created_at: String = r.get(5)?;
        let authority: String = r.get(6)?;
        let created_by: String = r.get(7)?;
        let first_created_at: Option<String> = r.get(8)?;
        Ok((
            id,
            item_id,
            title,
            metadata_str,
            source_type,
            created_at,
            authority,
            created_by,
            first_created_at,
        ))
    })?;

    for row in rev_rows {
        let (
            id,
            item_id,
            title,
            metadata_str,
            source_type,
            created_at,
            _authority,
            created_by,
            first_created_at,
        ) = row?;

        let change = parse_revision_context_change(
            id,
            item_id,
            title,
            metadata_str,
            source_type,
            created_at,
            created_by,
            first_created_at,
        );

        let is_reviewed = if change.created_at < frontier.through_at {
            true
        } else if change.created_at == frontier.through_at {
            frontier.boundary_change_ids.contains(&change.id)
        } else {
            false
        };

        if !is_reviewed && is_review_relevant(&change) {
            unseen_changes.push(change);
        }
    }

    // Conflicts
    let sql_conflicts = "SELECT id, left_item_id, right_item_id, status, created_at, updated_at
                         FROM context_conflicts
                         WHERE workstream_id = ?1 AND created_at >= ?2
                         ORDER BY created_at ASC";
    let mut st = conn.prepare(sql_conflicts)?;
    let conflict_rows = st.query_map(params![workstream_id, frontier.through_at], |r| {
        let id: String = r.get(0)?;
        let left_item_id: String = r.get(1)?;
        let right_item_id: Option<String> = r.get(2)?;
        let status: String = r.get(3)?;
        let created_at: String = r.get(4)?;
        let updated_at: String = r.get(5)?;
        Ok((
            id,
            left_item_id,
            right_item_id,
            status,
            created_at,
            updated_at,
        ))
    })?;

    for row in conflict_rows {
        let (id, left_item_id, _right_item_id, _status, created_at, _updated_at) = row?;
        let change = ContextChange {
            id: format!("conflict-created-{}", id),
            item_id: Some(left_item_id),
            conflict_id: Some(id),
            kind: "conflict_created".into(),
            title: "发现潜在 Context 冲突".into(),
            actor: "Agent".into(),
            source_type: Some("conflict".into()),
            created_at,
        };

        let is_reviewed = if change.created_at < frontier.through_at {
            true
        } else if change.created_at == frontier.through_at {
            frontier.boundary_change_ids.contains(&change.id)
        } else {
            false
        };

        if !is_reviewed && is_review_relevant(&change) {
            unseen_changes.push(change);
        }
    }

    // Conflict resolution events
    let sql_conflict_events =
        "SELECT e.id, e.conflict_id, c.left_item_id, e.new_status, e.actor, e.created_at
         FROM context_conflict_events e
         JOIN context_conflicts c ON c.id = e.conflict_id
         WHERE c.workstream_id = ?1 AND e.created_at >= ?2
         ORDER BY e.created_at ASC";
    if let Ok(mut st) = conn.prepare(sql_conflict_events) {
        let event_rows = st.query_map(params![workstream_id, frontier.through_at], |r| {
            let id: String = r.get(0)?;
            let conflict_id: String = r.get(1)?;
            let left_item_id: String = r.get(2)?;
            let new_status: String = r.get(3)?;
            let actor: String = r.get(4)?;
            let created_at: String = r.get(5)?;
            Ok((id, conflict_id, left_item_id, new_status, actor, created_at))
        })?;

        for row in event_rows {
            let (id, conflict_id, left_item_id, new_status, actor, created_at) = row?;
            let title = if new_status == "resolved" {
                "冲突已解决".to_string()
            } else if new_status == "dismissed" {
                "冲突已忽略".to_string()
            } else {
                format!("冲突状态更新为 {}", new_status)
            };
            let change = ContextChange {
                id: format!("conflict-event-{}", id),
                item_id: Some(left_item_id),
                conflict_id: Some(conflict_id),
                kind: "conflict_resolved".into(),
                title,
                actor: normalize_change_actor(&actor),
                source_type: Some("conflict_resolution".into()),
                created_at,
            };

            let is_reviewed = if change.created_at < frontier.through_at {
                true
            } else if change.created_at == frontier.through_at {
                frontier.boundary_change_ids.contains(&change.id)
            } else {
                false
            };

            if !is_reviewed && is_review_relevant(&change) {
                unseen_changes.push(change);
            }
        }
    }

    unseen_changes.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });

    Ok(unseen_changes)
}

pub fn get_workstream_review_state_conn(
    conn: &Connection,
    workstream_id: &str,
) -> Result<Option<WorkstreamReviewState>> {
    let mut st = conn.prepare(
        "SELECT workstream_id, reviewed_through_at, reviewed_boundary_change_ids, reviewed_at
         FROM workstream_review_state
         WHERE workstream_id = ?1",
    )?;
    let mut rows = st.query(params![workstream_id])?;
    if let Some(r) = rows.next()? {
        let workstream_id: String = r.get(0)?;
        let through_at: String = r.get(1)?;
        let ids_str: String = r.get(2)?;
        let reviewed_at: String = r.get(3)?;
        let boundary_change_ids: Vec<Id> = serde_json::from_str(&ids_str).unwrap_or_default();
        Ok(Some(WorkstreamReviewState {
            workstream_id,
            frontier: ReviewFrontier {
                through_at,
                boundary_change_ids,
            },
            reviewed_at,
        }))
    } else {
        // Baseline a Workstream that has no review state row yet.
        let ws_created_at: Option<String> = conn
            .query_row(
                "SELECT created_at FROM workstreams WHERE id = ?1",
                params![workstream_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(created_at) = ws_created_at {
            conn.execute(
                "INSERT OR IGNORE INTO workstream_review_state (workstream_id, reviewed_through_at, reviewed_boundary_change_ids, reviewed_at)
                 VALUES (?1, ?2, '[]', ?2)",
                params![workstream_id, created_at],
            )?;
            Ok(Some(WorkstreamReviewState {
                workstream_id: workstream_id.to_string(),
                frontier: ReviewFrontier {
                    through_at: created_at.clone(),
                    boundary_change_ids: Vec::new(),
                },
                reviewed_at: created_at,
            }))
        } else {
            Ok(None)
        }
    }
}

pub fn get_workstream_review_window_conn(
    conn: &Connection,
    workstream_id: &str,
) -> Result<WorkstreamReviewWindow> {
    let state = match get_workstream_review_state_conn(conn, workstream_id)? {
        Some(s) => s,
        None => return Err(other(format!("Workstream {} not found", workstream_id))),
    };

    let unseen_changes = list_workstream_review_changes_conn(conn, workstream_id, &state.frontier)?;

    let mark_through = if unseen_changes.is_empty() {
        state.frontier.clone()
    } else {
        let newest_ts = unseen_changes
            .iter()
            .map(|c| &c.created_at)
            .max()
            .unwrap()
            .clone();

        let mut boundary_change_ids: Vec<Id> = unseen_changes
            .iter()
            .filter(|c| c.created_at == newest_ts)
            .map(|c| c.id.clone())
            .collect();

        if newest_ts == state.frontier.through_at {
            for old_id in &state.frontier.boundary_change_ids {
                if !boundary_change_ids.contains(old_id) {
                    boundary_change_ids.push(old_id.clone());
                }
            }
        }
        boundary_change_ids.sort();
        boundary_change_ids.dedup();

        ReviewFrontier {
            through_at: newest_ts,
            boundary_change_ids,
        }
    };

    Ok(WorkstreamReviewWindow {
        state,
        unseen_changes,
        mark_through,
    })
}

pub fn mark_workstream_reviewed_conn(
    conn: &Connection,
    workstream_id: &str,
    new_frontier: &ReviewFrontier,
) -> Result<WorkstreamReviewState> {
    let exists: bool = conn
        .query_row(
            "SELECT 1 FROM workstreams WHERE id = ?1",
            params![workstream_id],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);

    if !exists {
        return Err(other(format!("Workstream {} not found", workstream_id)));
    }

    let current = get_workstream_review_state_conn(conn, workstream_id)?;
    let now_ts = now();

    match current {
        Some(old_state) => {
            if new_frontier.through_at > old_state.frontier.through_at {
                let mut sorted_ids = new_frontier.boundary_change_ids.clone();
                sorted_ids.sort();
                sorted_ids.dedup();
                let ids_json =
                    serde_json::to_string(&sorted_ids).unwrap_or_else(|_| "[]".to_string());
                conn.execute(
                    "UPDATE workstream_review_state
                     SET reviewed_through_at = ?2, reviewed_boundary_change_ids = ?3, reviewed_at = ?4
                     WHERE workstream_id = ?1",
                    params![workstream_id, new_frontier.through_at, ids_json, now_ts],
                )?;
                Ok(WorkstreamReviewState {
                    workstream_id: workstream_id.to_string(),
                    frontier: ReviewFrontier {
                        through_at: new_frontier.through_at.clone(),
                        boundary_change_ids: sorted_ids,
                    },
                    reviewed_at: now_ts,
                })
            } else if new_frontier.through_at == old_state.frontier.through_at {
                let mut merged_ids = old_state.frontier.boundary_change_ids.clone();
                for id in &new_frontier.boundary_change_ids {
                    if !merged_ids.contains(id) {
                        merged_ids.push(id.clone());
                    }
                }
                merged_ids.sort();
                merged_ids.dedup();
                let ids_json =
                    serde_json::to_string(&merged_ids).unwrap_or_else(|_| "[]".to_string());
                conn.execute(
                    "UPDATE workstream_review_state
                     SET reviewed_boundary_change_ids = ?2, reviewed_at = ?3
                     WHERE workstream_id = ?1",
                    params![workstream_id, ids_json, now_ts],
                )?;
                Ok(WorkstreamReviewState {
                    workstream_id: workstream_id.to_string(),
                    frontier: ReviewFrontier {
                        through_at: old_state.frontier.through_at,
                        boundary_change_ids: merged_ids,
                    },
                    reviewed_at: now_ts,
                })
            } else {
                // Stale token; monotonic invariant prevents rewinding.
                Ok(old_state)
            }
        }
        None => {
            let mut sorted_ids = new_frontier.boundary_change_ids.clone();
            sorted_ids.sort();
            sorted_ids.dedup();
            let ids_json = serde_json::to_string(&sorted_ids).unwrap_or_else(|_| "[]".to_string());
            conn.execute(
                "INSERT INTO workstream_review_state (workstream_id, reviewed_through_at, reviewed_boundary_change_ids, reviewed_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![workstream_id, new_frontier.through_at, ids_json, now_ts],
            )?;
            Ok(WorkstreamReviewState {
                workstream_id: workstream_id.to_string(),
                frontier: ReviewFrontier {
                    through_at: new_frontier.through_at.clone(),
                    boundary_change_ids: sorted_ids,
                },
                reviewed_at: now_ts,
            })
        }
    }
}

pub fn get_workstream_review_summary_conn(
    conn: &Connection,
    workstream_id: &str,
) -> Result<WorkstreamReviewSummary> {
    let window = get_workstream_review_window_conn(conn, workstream_id)?;

    let mut new_facts = 0;
    let mut updated_facts = 0;
    let mut resolved_items = 0;
    let mut superseded_items = 0;

    for c in &window.unseen_changes {
        match c.kind.as_str() {
            "added" => new_facts += 1,
            "edited" => updated_facts += 1,
            "resolved" | "deleted" => resolved_items += 1,
            "superseded" => superseded_items += 1,
            _ => {}
        }
    }

    let open_conflict_count: usize = conn.query_row(
        "SELECT COUNT(*) FROM context_conflicts WHERE workstream_id = ?1 AND status = 'open'",
        params![workstream_id],
        |r| r.get(0),
    )?;

    let unseen_change_count = window.unseen_changes.len();
    let last_unseen_change_at = window
        .unseen_changes
        .iter()
        .map(|c| &c.created_at)
        .max()
        .cloned();

    Ok(WorkstreamReviewSummary {
        workstream_id: workstream_id.to_string(),
        unseen_change_count,
        open_conflict_count,
        new_facts,
        updated_facts,
        resolved_items,
        superseded_items,
        last_unseen_change_at,
        reviewed_at: window.state.reviewed_at,
        has_updates: unseen_change_count > 0,
        needs_attention: open_conflict_count > 0,
    })
}

pub fn list_workstream_review_summaries_conn(
    conn: &Connection,
) -> Result<Vec<WorkstreamReviewSummary>> {
    let mut st = conn.prepare("SELECT id FROM workstreams ORDER BY updated_at DESC")?;
    let ids: Vec<String> = st
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut summaries = Vec::with_capacity(ids.len());
    for id in ids {
        summaries.push(get_workstream_review_summary_conn(conn, &id)?);
    }
    Ok(summaries)
}

pub fn resolve_conflict_audited_conn(
    conn: &Connection,
    conflict_id: &str,
    new_status: &str,
    resolution: Option<&str>,
    actor: &str,
) -> Result<()> {
    resolve_conflict_with_edit_conn(conn, conflict_id, new_status, resolution, None, actor)
}

pub fn resolve_conflict_with_edit_conn(
    conn: &Connection,
    conflict_id: &str,
    new_status: &str,
    resolution: Option<&str>,
    edit: Option<&ContextItemEditPayload>,
    actor: &str,
) -> Result<()> {
    if new_status != "resolved" && new_status != "dismissed" {
        return Err(other(format!(
            "无效的冲突状态: {} (仅支持 resolved 或 dismissed)",
            new_status
        )));
    }

    let conflict = get_conflict_conn(conn, conflict_id)?
        .ok_or_else(|| other(format!("冲突不存在: {}", conflict_id)))?;

    let mut resolved_revision_id = None;
    if let Some(e) = edit {
        let title = e.title.trim();
        if title.is_empty() {
            return Err(other("标题不能为空"));
        }
        let mut item = get_item_conn(conn, &conflict.left_item_id)?
            .ok_or_else(|| other(format!("条目不存在: {}", conflict.left_item_id)))?;

        let new_rev = ContextItemRevision {
            id: new_id(),
            item_id: item.id.clone(),
            title: title.to_string(),
            content: e.content.clone(),
            metadata: serde_json::json!({
                "provenance": {
                    "authority": "user_edit",
                    "actor": "user",
                    "source_type": "user_edit",
                    "source_ref": serde_json::Value::Null,
                }
            }),
            source_type: Some("user_edit".into()),
            source_ref: None,
            created_at: now(),
        };
        insert_revision_conn(conn, &new_rev)?;
        set_item_head_conn(conn, &item.id, &new_rev.id, None)?;
        set_item_authority_conn(conn, &item.id, "user_edit")?;
        item.updated_at = now();
        index_item_conn(conn, &item, &new_rev)?;

        resolved_revision_id = Some(new_rev.id);
    }

    let candidate_snapshot = conflict
        .candidate_snapshot_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());

    let snapshot = serde_json::json!({
        "conflict_id": conflict.id,
        "left_item_id": conflict.left_item_id,
        "right_item_id": conflict.right_item_id,
        "left_revision_id": conflict.left_revision_id,
        "right_revision_id": conflict.right_revision_id,
        "resolved_revision_id": resolved_revision_id.as_ref().or(conflict.left_revision_id.as_ref()),
        "candidate_snapshot": candidate_snapshot,
    });

    let event_id = new_id();
    let ts = now();

    conn.execute(
        "INSERT INTO context_conflict_events (id, conflict_id, previous_status, new_status, resolution, actor, created_at, snapshot_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            event_id,
            conflict_id,
            conflict.status,
            new_status,
            resolution,
            actor,
            ts,
            snapshot.to_string(),
        ],
    )?;

    conn.execute(
        "UPDATE context_conflicts SET status = ?2, resolution = ?3, updated_at = ?4 WHERE id = ?1",
        params![conflict_id, new_status, resolution, ts],
    )?;

    Ok(())
}

fn row_conflict_event(r: &Row) -> rusqlite::Result<ContextConflictEvent> {
    Ok(ContextConflictEvent {
        id: r.get(0)?,
        conflict_id: r.get(1)?,
        previous_status: r.get(2)?,
        new_status: r.get(3)?,
        resolution: r.get(4)?,
        actor: r.get(5)?,
        created_at: r.get(6)?,
        snapshot_json: r.get(7)?,
    })
}
