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

// The WorkspacePath registry, explicit task/project memberships and Session→path
// attach each live in their own impl file rather than growing this monolith.
pub mod context_repo;
pub mod message_query;
pub mod preferences;
pub mod project_context_repo;
pub mod schema;
pub mod session_lifecycle;
pub mod session_paths;
pub mod workspace;
pub mod workstream_projects;

pub use context_repo::{
    apply_projection_conn, bump_input_revision_conn, commit_session_context_conn,
    delete_workstream_frontier_conn, frontiers_for_workstream_conn, get_ingest_state_conn,
    get_session_context_conn, get_workstream_context_state_conn, projection_ids_conn,
    set_workstream_frontier_conn, ProjectionOutcome,
};

pub use schema::{DATABASE_APPLICATION_ID, DATABASE_FORMAT_VERSION};
pub use session_lifecycle::PermanentDeletionCounts;

/// Session API projection: Project membership is read through the path, never stored.
const SESSION_SELECT: &str = "SELECT sessions.*,
    (SELECT wp.project_id FROM workspace_paths wp WHERE wp.id = sessions.workspace_path_id) AS project_id,
    (SELECT json_group_array(workstream_id) FROM (SELECT workstream_id FROM session_workstreams WHERE session_id=sessions.id ORDER BY workstream_id)) AS tracked_workstream_ids
    FROM sessions";

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
           (id, agent, root_agent_session_id, title, cwd, workspace_path_id,
            forked_from_session_id, started_at, last_activity_at,
            source_kind, source_path, metadata)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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

    /// All registered projects, including archived and currently empty projects.
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

    /// Tasks explicitly associated with a project; no path snapshot or roles.
    pub fn list_workstreams(&self, project_id: Option<&str>) -> Result<Vec<Workstream>> {
        let conn = self.read();
        let (sql, has_filter): (&str, bool) = if project_id.is_some() {
            (
                "SELECT w.* FROM workstreams w
              WHERE EXISTS (
                SELECT 1 FROM workstream_projects wsp WHERE wsp.workstream_id=w.id AND wsp.project_id=?1)
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
             WHERE EXISTS (SELECT 1 FROM session_workstreams sw WHERE sw.session_id=sessions.id AND sw.workstream_id=?1)",
            params![workstream_id],
            |r| r.get(0),
        )?;
        let latest = conn
            .query_row(
                "SELECT id, agent, COALESCE(last_activity_at, started_at) AS act
                 FROM sessions
                 WHERE EXISTS (SELECT 1 FROM session_workstreams sw WHERE sw.session_id=sessions.id AND sw.workstream_id=?1)
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
    pub fn get_workstream(&self, id: &str) -> Result<Option<Workstream>> {
        let conn = self.read();
        get_workstream_conn(&conn, id)
    }

    pub fn upsert_workstream(&self, w: &Workstream) -> Result<()> {
        {
            let conn = self.write();
            upsert_workstream_conn(&conn, w)?;
        }
        self.index_workstream(w)
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
        self.tx(|tx| workstream_projects::purge_workstream_data_conn(tx, id))?;
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
        self.upsert_logical_session(
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
        self.tx(|tx| {
            upsert_logical_session_conn(
                tx,
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
        })
    }

    pub fn get_session(&self, id: &str) -> Result<Option<Session>> {
        let conn = self.read();
        Ok(conn
            .query_row(
                &format!("{SESSION_SELECT} WHERE id = ?1"),
                params![id],
                row_session,
            )
            .optional()?)
    }

    /// The Logical Session for a root Resume identity. This is THE
    /// session lookup for terminal binding and Resume.
    pub fn find_session_by_root_agent_id(
        &self,
        agent: Agent,
        root_agent_session_id: &str,
    ) -> Result<Option<Session>> {
        let conn = self.read();
        Ok(conn
            .query_row(
                &format!("{SESSION_SELECT} WHERE agent = ?1 AND root_agent_session_id = ?2"),
                params![agent.as_str(), root_agent_session_id],
                row_session,
            )
            .optional()?)
    }

    /// Project membership and the returned `project_id` both follow WorkspacePath.
    pub fn list_sessions(&self, filter: SessionFilter) -> Result<Vec<Session>> {
        let conn = self.read();
        // Dynamic SQL: placeholders are appended together with the bind
        // values, so the numbering can never drift out of sync.
        let mut sql = format!("{SESSION_SELECT} WHERE 1=1");
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
            {
                let mut ins = tx.prepare(
                    "INSERT INTO session_messages
                     (id, session_id, sequence, source_message_id, source_generation,
                      source_position, source_identity_hash, ts, role, content)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
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

    /// Message counts by role for a Session (user messages vs assistant replies).
    pub fn session_message_stats(&self, session_id: &str) -> Result<SessionMessageStats> {
        let conn = self.read();
        let mut st = conn.prepare(
            "SELECT m.role, COUNT(*)
             FROM session_message_projection p
             JOIN session_messages m ON m.id = p.session_message_id
             WHERE p.session_id = ?1
             GROUP BY m.role",
        )?;
        let mut stats = SessionMessageStats::default();
        let rows = st.query_map(params![session_id], |r| {
            let role: String = r.get(0)?;
            let count: i64 = r.get(1)?;
            Ok((role, count))
        })?;
        for row in rows {
            let (role, count) = row?;
            match role.as_str() {
                "user" => stats.user_messages = count,
                "assistant" => stats.assistant_messages = count,
                _ => {}
            }
        }
        if stats.user_messages == 0 && stats.assistant_messages == 0 {
            let mut st = conn.prepare(
                "SELECT role, COUNT(*)
                 FROM session_messages
                 WHERE session_id = ?1
                 GROUP BY role",
            )?;
            let rows = st.query_map(params![session_id], |r| {
                let role: String = r.get(0)?;
                let count: i64 = r.get(1)?;
                Ok((role, count))
            })?;
            for row in rows {
                let (role, count) = row?;
                match role.as_str() {
                    "user" => stats.user_messages = count,
                    "assistant" => stats.assistant_messages = count,
                    _ => {}
                }
            }
        }
        Ok(stats)
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

    pub fn set_session_tracking(&self, sid: &str, tid: &str, tracked: bool) -> Result<()> {
        self.tx(|tx| workstream_projects::set_tracking_conn(tx, sid, tid, tracked))
    }
    /// A launch selection is applied once to its exact native session. A deleted,
    /// archived or detached task cannot prevent terminal identity binding.
    pub fn inherit_terminal_tracking(&self, session: &Session, task: Option<&str>) -> Result<()> {
        self.tx(|tx| {
            let current=get_session_conn(tx,&session.id)?.ok_or_else(||other("会话不存在"))?;
            if current.agent!=session.agent || current.root_agent_session_id!=session.root_agent_session_id { return Err(other("终端会话身份已变化")); }
            if let Some(tid)=task {
                let eligible:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM workstreams w JOIN workstream_projects p ON p.workstream_id=w.id WHERE w.id=?1 AND w.visibility='normal' AND p.project_id=?2)",params![tid,current.project_id],|r|r.get(0))?;
                if eligible { workstream_projects::set_tracking_conn(tx,&current.id,tid,true)?; }
            }
            Ok(())
        })
    }

    /// Sessions tracking `workstream_id`, including archived sessions.
    /// The same session may appear in multiple tasks.
    pub fn sessions_for_workstream(&self, workstream_id: &str) -> Result<Vec<Session>> {
        let conn = self.read();
        let mut st = conn.prepare(&format!(
            "{SESSION_SELECT} WHERE EXISTS (SELECT 1 FROM session_workstreams sw WHERE sw.session_id=sessions.id AND sw.workstream_id=?1)
             ORDER BY COALESCE(last_activity_at, started_at) DESC"
        ))?;
        let rows = st
            .query_map(params![workstream_id], row_session)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

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
        project_context_repo::reindex(&conn, &p.id)
    }

    /// A task can belong to several projects, so it has no single search parent.
    pub fn index_workstream(&self, w: &Workstream) -> Result<()> {
        let conn = self.write();
        workstream_projects::reindex_workstream_search_conn(&conn, &w.id)
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
        // Tasks are roots in search; their multiple projects are read through paths.
        conn.execute(
            "UPDATE search_index SET parent_id = '' WHERE kind = 'workstream'",
            [],
        )?;
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
        // A document whose body can change (title, Project, tracked tasks)
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
        "INSERT INTO projects (id, name, git_id, name_customized, created_at, updated_at, visibility)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
           -- A customized name is user intent: no automatic rename (worktree
           -- discovery, Git upgrade, Project merge) may overwrite it.
           name = CASE WHEN name_customized = 1 THEN name ELSE ?2 END,
           -- Git identity is never cleared *or re-targeted* by a whole-object
           -- write; the one legitimate writer is
           -- `workspace::project::adopt_git_identity_conn` (first-set-only).
           git_id = COALESCE(git_id, ?3),
           name_customized = MAX(name_customized, ?4),
            updated_at = ?6",
        params![
            p.id,
            p.name,
            p.git_id,
            p.name_customized as i64,
            p.created_at,
            p.updated_at,
            p.visibility
        ],
    )?;
    Ok(())
}

pub fn get_workstream_conn(conn: &Connection, id: &str) -> Result<Option<Workstream>> {
    Ok(conn
        .query_row(
            "SELECT * FROM workstreams WHERE id = ?1",
            params![id],
            row_workstream,
        )
        .optional()?)
}

pub fn upsert_workstream_conn(conn: &Connection, w: &Workstream) -> Result<()> {
    // A Session's search document carries its tracked task titles, so a rename has
    // to reach every Session tracking this task. The previous title is
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
    if text_changed {
        reindex_tracking_sessions_conn(conn, &w.id)?;
    }
    Ok(())
}

/// Rebuild documents for all sessions tracking this task in the caller's transaction.
pub fn reindex_tracking_sessions_conn(conn: &Connection, workstream_id: &str) -> Result<()> {
    let mut ids: Vec<String> = Vec::new();
    {
        let mut st = conn.prepare("SELECT id FROM sessions WHERE EXISTS (SELECT 1 FROM session_workstreams sw WHERE sw.session_id=sessions.id AND sw.workstream_id=?1)")?;
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
/// the same rule as [`reindex_tracking_sessions_conn`] for the Project name.
pub fn reindex_sessions_for_project_conn(conn: &Connection, project_id: &str) -> Result<()> {
    let mut ids: Vec<String> = Vec::new();
    {
        let mut st = conn.prepare("SELECT s.id FROM sessions s JOIN workspace_paths wp ON wp.id = s.workspace_path_id WHERE wp.project_id = ?1")?;
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
/// (falling back to the Agent name), body is the Project name plus all tracked task titles and the cwd.
pub fn index_session_conn(conn: &rusqlite::Connection, session_id: &str) -> Result<()> {
    let row: Option<(String, String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT COALESCE(s.title, s.agent), COALESCE(p.name, ''), (SELECT group_concat(title, char(10)) FROM (SELECT w.title FROM session_workstreams sw JOIN workstreams w ON w.id=sw.workstream_id WHERE sw.session_id=s.id ORDER BY w.id)), s.cwd
               FROM sessions s
               LEFT JOIN workspace_paths wp ON wp.id = s.workspace_path_id
               LEFT JOIN projects p ON p.id = wp.project_id
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

fn row_project(r: &Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: r.get("id")?,
        name: r.get("name")?,
        git_id: r.get("git_id")?,
        name_customized: r.get::<_, i64>("name_customized")? != 0,
        visibility: r.get("visibility")?,
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
        tracked_workstream_ids: serde_json::from_str(
            &r.get::<_, String>("tracked_workstream_ids")?,
        )
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?,
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

pub(crate) fn get_session_conn(conn: &Connection, id: &str) -> Result<Option<Session>> {
    Ok(conn
        .query_row(
            &format!("{SESSION_SELECT} WHERE sessions.id=?1"),
            [id],
            row_session,
        )
        .optional()?)
}

#[cfg(test)]

pub(crate) fn seed_tracking(
    db: &crate::storage::Db,
    sid: &str,
    task: Option<&str>,
) -> crate::error::Result<()> {
    db.tx(|tx| {
        if let Some(tid)=task {
            tx.execute("INSERT OR IGNORE INTO session_workstreams(session_id,workstream_id) VALUES (?1,?2)",rusqlite::params![sid,tid])?;
            crate::storage::bump_input_revision_conn(tx,tid)?;
        } else {
            let mut st=tx.prepare("SELECT workstream_id FROM session_workstreams WHERE session_id=?1")?;
            let ids=st.query_map([sid],|r|r.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;
            drop(st);
            tx.execute("DELETE FROM session_workstreams WHERE session_id=?1",[sid])?;
            for tid in ids { crate::storage::bump_input_revision_conn(tx,&tid)?; }
        }
        crate::storage::index_session_conn(tx,sid)
    })
}
