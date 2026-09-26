//! SQLite storage: current schema and repository helpers.
//! Owns: domain data, the Logical Session graph (members / messages / cursors
//! / stats / context frontier / diagnostics), context items, FTS search.
//!
//! History integrity rules:
//! - `session_messages` is the ONLY conversation store (root members only)
//!   and is append-only: the row identity is an app-owned `id`, and a stable
//!   `(member_id, source_identity_hash)` unique index makes re-scans
//!   idempotent. Rows are never replaced or overwritten.
//! - Member cursors track the *read* position per member; the session's
//!   `session_context_state.processed_message_sequence` is the separate
//!   *processed* position of Context consumption.
//! - One member ingest = messages + stats + cursor + activity in ONE
//!   transaction (`commit_member_ingest`), guarded by trash / membership
//!   re-checks inside it.

use std::sync::{Mutex, MutexGuard};

use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};

use crate::domain::*;
use crate::error::{other, Result};

// The WorkspacePath registry, ordered WorkstreamPath list and Session→path
// attach each live in their own impl file rather than growing this monolith.
pub mod schema;
pub mod session_lifecycle;
pub mod session_paths;
pub mod workspace;
pub mod workstream_paths;

pub use schema::{DATABASE_APPLICATION_ID, DATABASE_FORMAT_VERSION};
pub use session_lifecycle::PermanentDeletionCounts;

/// Two connections to one SQLite file, so the UI's reads never queue behind a
/// background sync's writes: WAL allows one writer plus concurrent readers,
/// every mutation goes through `writer` (serialized by its mutex), and query
/// work goes through `reader`. Splitting at the storage layer means callers
/// never see a database lock — there is no `Mutex<Db>` to hold across a file
/// scan, and a long write transaction cannot freeze a list query.
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

/// Stable identity of a ROOT conversation message, used to make re-scans of
/// the same Agent source idempotent.
///
/// - Messages WITH a native Agent message id: content hash of
///   `(native_id | role | ts | content)` — the Agent guarantees the id is
///   unique per logical message, so identity is position-independent.
/// - Messages WITHOUT one: chained hash
///   `H(prev_hash | role | ts | content)`. The chain encodes *adjacency*:
///   re-scanning an unchanged prefix reproduces the chain (same logical
///   message → dedup), while two genuinely identical turns link to different
///   predecessors and stay distinct. A mid-file rewrite diverges the chain
///   exactly where content changed — everything after it is new evidence.
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

#[allow(clippy::too_many_arguments)]
fn upsert_logical_session_conn(
    conn: &Connection,
    agent: Agent,
    root_agent_session_id: &str,
    title: Option<&str>,
    cwd: Option<&str>,
    workspace_path_id: Option<&str>,
    forked_from_session_id: Option<&str>,
    started_at: Option<&str>,
    last_activity_at: Option<&str>,
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
               title = COALESCE(title, ?2),
               cwd = COALESCE(?3, cwd),
               workspace_path_id = COALESCE(?4, workspace_path_id),
               project_id = COALESCE(
                 (SELECT wp.project_id FROM workspace_paths wp
                   WHERE wp.id = COALESCE(?4, workspace_path_id)),
                 (SELECT wp.project_id FROM workspace_paths wp
                   WHERE wp.id = workspace_path_id)),
               forked_from_session_id = COALESCE(forked_from_session_id, ?5),
               started_at = COALESCE(started_at, ?6),
               last_activity_at = ?7
             WHERE id = ?1",
            params![
                id,
                title,
                cwd,
                workspace_path_id,
                forked_from_session_id,
                started_at,
                activity
            ],
        )?;
        index_session_conn(conn, &id)?;
        return Ok((id, false));
    }
    let id = new_id();
    conn.execute(
        "INSERT INTO sessions
           (id, agent, root_agent_session_id, title, cwd, workspace_path_id, project_id,
            forked_from_session_id, started_at, last_activity_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6,
                 (SELECT wp.project_id FROM workspace_paths wp WHERE wp.id = ?6),
                 ?7, ?8, ?9)",
        params![
            id,
            agent.as_str(),
            root_agent_session_id,
            title,
            cwd,
            workspace_path_id,
            forked_from_session_id,
            started_at,
            last_activity_at
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

    // ---------------- Projects ----------------

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

    // ---------------- Workstreams ----------------

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
    /// (id, agent), that session's activity timestamp). Only Sessions that own
    /// this Workstream count; a Session can never be counted twice
    /// across Workstreams. Trashed sessions are inactive and count
    /// for nothing here.
    pub fn workstream_session_stats(
        &self,
        workstream_id: &str,
    ) -> Result<(i64, Option<(String, String)>, Option<String>)> {
        let conn = self.read();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sessions
             WHERE owner_workstream_id = ?1 AND trashed_at IS NULL",
            params![workstream_id],
            |r| r.get(0),
        )?;
        let latest = conn
            .query_row(
                "SELECT id, agent, COALESCE(last_activity_at, started_at) AS act
                 FROM sessions
                 WHERE owner_workstream_id = ?1 AND trashed_at IS NULL
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

    // ---------------- Logical Sessions ----------------

    /// Session-only fixture helper. Production discovery uses
    /// `upsert_logical_root` to create the Session and Root member atomically.
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
            title,
            cwd,
            workspace_path_id,
            forked_from_session_id,
            started_at,
            last_activity_at,
        )
    }

    /// Atomically upsert a Logical Session and its required Root member.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_logical_root(
        &self,
        agent: Agent,
        root_agent_session_id: &str,
        title: Option<&str>,
        cwd: Option<&str>,
        workspace_path_id: Option<&str>,
        forked_from_session_id: Option<&str>,
        started_at: Option<&str>,
        last_activity_at: Option<&str>,
        source_kind: &str,
        source_path: &str,
        parent_source_member_id: Option<&str>,
        metadata: &serde_json::Value,
    ) -> Result<(String, bool)> {
        self.tx(|tx| {
            if let Some((id, true)) = tx
                .query_row(
                    "SELECT id, trashed_at IS NOT NULL FROM sessions
                     WHERE agent = ?1 AND root_agent_session_id = ?2",
                    params![agent.as_str(), root_agent_session_id],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)),
                )
                .optional()?
            {
                return Ok((id, false));
            }
            // Topology guard: a stored child/side member must not be promoted
            // to a Logical Root — refuse before the session row is created, so
            // the refusal can never strand a session without its root member.
            let stored_relation: Option<String> = tx
                .query_row(
                    "SELECT relation FROM session_members
                     WHERE agent = ?1 AND source_member_id = ?2",
                    params![agent.as_str(), root_agent_session_id],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(rel) = stored_relation {
                if rel != SessionMemberRelation::Root.as_str() {
                    return Err(other(format!(
                        "member {root_agent_session_id} 已是 {rel} 成员，拒绝提升为 Logical Root"
                    )));
                }
            }
            let (id, is_new) = upsert_logical_session_conn(
                tx,
                agent,
                root_agent_session_id,
                title,
                cwd,
                workspace_path_id,
                forked_from_session_id,
                started_at,
                last_activity_at,
            )?;
            upsert_session_member_conn(
                tx,
                &id,
                agent,
                root_agent_session_id,
                SessionMemberRelation::Root,
                parent_source_member_id,
                source_kind,
                source_path,
                cwd,
                started_at,
                last_activity_at,
                metadata,
            )?;
            Ok((id, is_new))
        })
    }

    /// Return a snapshot of retryable reconcile work. Ownerless sessions are
    /// included only while a LaunchIntent is pending; owned sessions only
    /// when the Context frontier trails stored conversation.
    pub fn reconcile_retry_sessions(
        &self,
        include_ownerless: bool,
        include_pending_context: bool,
        agent: Option<Agent>,
    ) -> Result<Vec<Session>> {
        let conn = self.read();
        let mut st = conn.prepare(
            "SELECT s.* FROM sessions s
             WHERE s.trashed_at IS NULL AND (?3 IS NULL OR s.agent = ?3) AND (
               (?1 AND s.owner_workstream_id IS NULL)
               OR (?2 AND s.owner_workstream_id IS NOT NULL AND EXISTS (
                 SELECT 1 FROM session_messages m
                 WHERE m.session_id = s.id AND m.sequence > COALESCE((
                   SELECT processed_message_sequence FROM session_context_state
                   WHERE session_id = s.id
                 ), 0)
               ))
             )
             ORDER BY COALESCE(s.last_conversation_at, s.last_activity_at, s.started_at) DESC",
        )?;
        let rows = st
            .query_map(
                params![
                    include_ownerless,
                    include_pending_context,
                    agent.map(|a| a.as_str())
                ],
                row_session,
            )?
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

    /// Sessions discovered but not yet seen by us. Used by LaunchIntent
    /// matching: only genuinely new sessions may claim a pending intent.
    /// The SQL has no created_at column, so the since-filter runs in Rust.
    /// Trashed rows can never be "genuinely new" (matching only fires for
    /// fresh discoveries), but the predicate keeps that invariant explicit
    /// and safe against future callers.
    pub fn recently_created_sessions(&self, agent: Agent, since: &str) -> Result<Vec<Session>> {
        let conn = self.read();
        let mut st = conn.prepare(
            "SELECT * FROM sessions WHERE agent = ?1 AND trashed_at IS NULL \
             ORDER BY COALESCE(started_at, last_activity_at) DESC LIMIT 200",
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
            SessionListScope::Active => sql.push_str(" AND trashed_at IS NULL"),
            SessionListScope::Trash => sql.push_str(" AND trashed_at IS NOT NULL"),
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

    // ---------------- Session Members ----------------

    /// Insert or refresh one execution member. The identity is
    /// `(agent, source_member_id)`; a member whose topology resolution moved
    /// to another Logical Session is re-pointed by the same upsert.
    /// Child / side cwds land on the MEMBER row only — they can never reach
    /// `sessions.cwd` / `project_id`.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_session_member(
        &self,
        session_id: &str,
        agent: Agent,
        source_member_id: &str,
        relation: SessionMemberRelation,
        parent_source_member_id: Option<&str>,
        source_kind: &str,
        source_path: &str,
        cwd: Option<&str>,
        started_at: Option<&str>,
        last_activity_at: Option<&str>,
        metadata: &serde_json::Value,
    ) -> Result<String> {
        let conn = self.write();
        upsert_session_member_conn(
            &conn,
            session_id,
            agent,
            source_member_id,
            relation,
            parent_source_member_id,
            source_kind,
            source_path,
            cwd,
            started_at,
            last_activity_at,
            metadata,
        )
    }

    /// Attach an observed non-root member only while its Logical Session is
    /// active. The lifecycle check and topology write share one transaction.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_active_session_member(
        &self,
        session_id: &str,
        agent: Agent,
        source_member_id: &str,
        relation: SessionMemberRelation,
        parent_source_member_id: Option<&str>,
        source_kind: &str,
        source_path: &str,
        cwd: Option<&str>,
        started_at: Option<&str>,
        last_activity_at: Option<&str>,
        metadata: &serde_json::Value,
    ) -> Result<Option<String>> {
        self.tx(|tx| {
            if !session_lifecycle::session_is_writable_conn(tx, session_id)? {
                return Ok(None);
            }
            Ok(Some(upsert_session_member_conn(
                tx,
                session_id,
                agent,
                source_member_id,
                relation,
                parent_source_member_id,
                source_kind,
                source_path,
                cwd,
                started_at,
                last_activity_at,
                metadata,
            )?))
        })
    }

    /// The member with this Adapter identity, in ANY session — the anchor for
    /// topology resolution and diagnostics cleanup.
    pub fn find_member_by_source_id(
        &self,
        agent: Agent,
        source_member_id: &str,
    ) -> Result<Option<SessionMember>> {
        let conn = self.read();
        Ok(conn
            .query_row(
                "SELECT * FROM session_members WHERE agent = ?1 AND source_member_id = ?2",
                params![agent.as_str(), source_member_id],
                row_member,
            )
            .optional()?)
    }

    pub fn get_member(&self, member_id: &str) -> Result<Option<SessionMember>> {
        let conn = self.read();
        Ok(conn
            .query_row(
                "SELECT * FROM session_members WHERE id = ?1",
                params![member_id],
                row_member,
            )
            .optional()?)
    }

    /// Every execution member of a Logical Session, root first.
    pub fn members_for_session(&self, session_id: &str) -> Result<Vec<SessionMember>> {
        let conn = self.read();
        let mut st = conn.prepare(
            "SELECT * FROM session_members WHERE session_id = ?1
             ORDER BY CASE relation WHEN 'root' THEN 0 ELSE 1 END, started_at, id",
        )?;
        let rows = st
            .query_map(params![session_id], row_member)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The one ROOT member of a Logical Session. The partial unique index
    /// guarantees at most one; `None` means the session has no root member
    /// row yet.
    pub fn root_member_for_session(&self, session_id: &str) -> Result<Option<SessionMember>> {
        let conn = self.read();
        Ok(conn
            .query_row(
                "SELECT * FROM session_members WHERE session_id = ?1 AND relation = 'root'",
                params![session_id],
                row_member,
            )
            .optional()?)
    }

    // ---------------- Member Cursors ----------------

    pub fn get_member_cursor(&self, member_id: &str) -> Result<SessionMemberCursor> {
        let conn = self.read();
        Ok(conn
            .query_row(
                "SELECT member_id, source_file_identity, generation, byte_offset, last_seen_size, mtime, prefix_hash, identity_tail_hash, active_provider, active_model
                 FROM session_member_cursors WHERE member_id = ?1",
                params![member_id],
                |r| {
                    Ok(SessionMemberCursor {
                        member_id: r.get(0)?,
                        source_file_identity: r.get(1)?,
                        generation: r.get(2)?,
                        byte_offset: r.get::<_, i64>(3)?.max(0) as u64,
                        last_seen_size: r.get::<_, i64>(4)?.max(0) as u64,
                        mtime: r.get(5)?,
                        prefix_hash: r.get(6)?,
                        identity_tail_hash: r.get(7)?,
                        active_provider: r.get(8)?,
                        active_model: r.get(9)?,
                    })
                },
            )
            .optional()?
            .unwrap_or(SessionMemberCursor {
                member_id: member_id.to_string(),
                ..Default::default()
            }))
    }

    /// Rewind every member cursor of a session so the next ingest re-scans
    /// from the start. MESSAGES ARE NOT TOUCHED:
    /// their app-owned ids and every provenance ref stay valid — unchanged
    /// content dedups by identity on the re-scan. Stats snapshots replace on
    /// the rescan; the Context frontier is preserved. The provenance state
    /// frontier resets with the bytes: a fresh full scan re-derives it.
    pub fn reset_member_cursors(&self, session_id: &str) -> Result<()> {
        let conn = self.write();
        conn.execute(
            "UPDATE session_member_cursors
             SET byte_offset = 0, last_seen_size = 0, prefix_hash = '', identity_tail_hash = '',
                 active_provider = NULL, active_model = NULL
             WHERE member_id IN (SELECT id FROM session_members WHERE session_id = ?1)",
            params![session_id],
        )?;
        Ok(())
    }

    // ---------------- Conversation Messages ----------------

    /// The atomic member-ingest commit. Messages, stats, the
    /// member cursor and the activity stamps commit or not at all.
    ///
    /// Guards, in order, inside the transaction:
    /// 1. the Logical Session exists and is not trashed;
    /// 2. the member still belongs to that session;
    /// 3. messages require `member.relation == root` — an adapter that hands
    ///    child text to the conversation is a bug, never silently stored.
    ///
    /// A failed guard stores NOTHING (no messages, no stats, no cursor move),
    /// so a Trash racing a parse cannot produce a half commit. The identity
    /// chain starts from genesis on a full re-scan (start offset 0) and
    /// otherwise continues from the cursor's `identity_tail_hash` — the tail
    /// of the CURRENT source chain, never "last message in the store".
    pub fn commit_member_ingest(
        &self,
        session_id: &str,
        member_id: &str,
        messages: &[ParsedSessionMessage],
        stats: Option<StatsUpdate>,
        source: &SourceCursorUpdate,
    ) -> Result<Vec<SessionMessage>> {
        self.commit_member_ingest_with_provenance_state(
            session_id, member_id, messages, stats, source, None, None,
        )
    }

    /// Same transaction, plus the stateful provenance frontier: the bytes
    /// frontier and the provenance state frontier are written together, so
    /// they can never drift. Adapters with direct per-message evidence pass
    /// `None`/`None` — their provenance travels on the messages themselves.
    pub fn commit_member_ingest_with_provenance_state(
        &self,
        session_id: &str,
        member_id: &str,
        messages: &[ParsedSessionMessage],
        stats: Option<StatsUpdate>,
        source: &SourceCursorUpdate,
        next_active_provider: Option<String>,
        next_active_model: Option<String>,
    ) -> Result<Vec<SessionMessage>> {
        self.tx(|tx| {
            // 1. commit-time trash guard. A trashed (or
            // vanished) session takes NOTHING — the next Restore resumes from
            // the untouched cursor.
            if !session_lifecycle::session_is_writable_conn(tx, session_id)? {
                return Ok(Vec::new());
            }
            // 2. the member must still belong to THIS session. A
            // topology correction that moved it mid-parse invalidates the
            // whole prepared batch.
            let relation: Option<String> = tx
                .query_row(
                    "SELECT relation FROM session_members WHERE id = ?1 AND session_id = ?2",
                    params![member_id, session_id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(relation) = relation else {
                return Ok(Vec::new());
            };
            // 3. only the ROOT member may write Conversation.
            let is_root = relation == SessionMemberRelation::Root.as_str();
            if !messages.is_empty() && !is_root {
                return Err(other(format!(
                    "member {member_id} (relation={relation}) 不是 root，拒绝写入会话消息"
                )));
            }
            // 4. provenance guard: user messages have no
            // generation model. An adapter that hands one over is a bug, not
            // data — reject the whole batch, the same way a child message is.
            if messages.iter().any(|m| {
                m.role == SessionMessageRole::User && (m.provider.is_some() || m.model.is_some())
            }) {
                return Err(other(
                    "user 消息不允许携带 provider/model provenance，拒绝整批提交",
                ));
            }

            let old_cursor: Option<(String, i64, i64, i64, Option<f64>)> = tx
                .query_row(
                    "SELECT source_file_identity, generation, byte_offset, last_seen_size, mtime
                     FROM session_member_cursors WHERE member_id = ?1",
                    params![member_id],
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
                    "SELECT identity_tail_hash FROM session_member_cursors WHERE member_id = ?1",
                    params![member_id],
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
                    // the member has no message at all.
                    None => tx
                        .query_row(
                            "SELECT source_identity_hash FROM session_messages
                             WHERE member_id = ?1 ORDER BY sequence DESC LIMIT 1",
                            params![member_id],
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
            let raw_path = {
                let p: String = tx.query_row(
                    "SELECT source_path FROM session_members WHERE id = ?1",
                    params![member_id],
                    |r| r.get(0),
                )?;
                p
            };
            {
                let mut ins = tx.prepare(
                    "INSERT INTO session_messages
                     (id, session_id, member_id, sequence, source_message_id, source_generation,
                      source_position, source_identity_hash, ts, role, content, provider, model, raw_ref)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                     ON CONFLICT(member_id, source_identity_hash) DO NOTHING",
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
                        member_id,
                        next_seq,
                        m.source_message_id,
                        source.generation,
                        m.source_position,
                        hash,
                        m.ts,
                        m.role.as_str(),
                        m.content,
                        m.provider,
                        m.model,
                        raw_ref,
                    ])?;
                    if n > 0 {
                        stored.push(SessionMessage {
                            id: id.clone(),
                            session_id: session_id.to_string(),
                            member_id: member_id.to_string(),
                            sequence: next_seq,
                            role: m.role,
                            content: m.content.clone(),
                            ts: m.ts.clone(),
                            provider: m.provider.clone(),
                            model: m.model.clone(),
                            source_message_id: m.source_message_id.clone(),
                            source_generation: source.generation,
                            source_position: m.source_position.clone(),
                            source_identity_hash: hash,
                            raw_ref,
                        });
                        next_seq += 1;
                    } else {
                        // Dedup hit: same message
                        // identity. `NULL → confirmed` enriches in place;
                        // a confirmed-vs-confirmed contradiction keeps the
                        // stored value and logs — it never becomes a second
                        // message and never silently overwrites.
                        enrich_message_provenance_conn(tx, member_id, &hash, m)?;
                    }
                }
            }

            // 5. stats delta / snapshot, then 6. the cursor advance.
            apply_stats_conn(tx, member_id, stats)?;
            let new_tail = if messages.is_empty() {
                stored_tail.unwrap_or_default()
            } else {
                prev_hash
            };
            upsert_member_cursor_conn(
                tx,
                &SessionMemberCursor {
                    member_id: member_id.to_string(),
                    source_file_identity: source.file_identity.clone(),
                    generation: source.generation,
                    byte_offset: source.byte_offset,
                    last_seen_size: source.last_seen_size,
                    mtime: source.mtime,
                    prefix_hash: source.prefix_hash.clone(),
                    identity_tail_hash: new_tail,
                    active_provider: next_active_provider,
                    active_model: next_active_model,
                },
            )?;

            // Source activity advances only when its observed cursor changes;
            // reading an unchanged file is not Agent activity.
            if source_changed {
                let member_activity: Option<String> = tx.query_row(
                    "SELECT last_activity_at FROM session_members WHERE id = ?1",
                    params![member_id],
                    |r| r.get(0),
                )?;
                let observed = later_timestamp(
                    member_activity.as_deref(),
                    mtime_timestamp(source.mtime).as_deref(),
                );
                if let Some(observed) = observed {
                    let current: Option<String> = tx.query_row(
                        "SELECT last_activity_at FROM sessions WHERE id = ?1",
                        params![session_id],
                        |r| r.get(0),
                    )?;
                    let activity = later_timestamp(current.as_deref(), Some(&observed));
                    tx.execute(
                        "UPDATE session_members SET last_activity_at = ?2 WHERE id = ?1",
                        params![member_id, observed],
                    )?;
                    tx.execute(
                        "UPDATE sessions SET last_activity_at = ?2 WHERE id = ?1",
                        params![session_id, activity],
                    )?;
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

    pub fn get_messages(
        &self,
        session_id: &str,
        after: Option<i64>,
        limit: i64,
    ) -> Result<Vec<SessionMessage>> {
        let conn = self.read();
        get_messages_conn(&conn, session_id, after, limit)
    }

    /// The Context frontier read: root conversation messages after the
    /// processed sequence, in order.
    pub fn get_messages_after(
        &self,
        session_id: &str,
        processed_sequence: i64,
        limit: i64,
    ) -> Result<Vec<SessionMessage>> {
        self.get_messages(session_id, Some(processed_sequence), limit)
    }

    /// The session's ingested conversation frontier: the highest message
    /// sequence durably stored. Distinct from
    /// [`Self::get_context_state`], which is how far Sync has consumed.
    pub fn ingested_message_sequence(&self, session_id: &str) -> Result<i64> {
        let conn = self.read();
        Ok(conn.query_row(
            "SELECT COALESCE(MAX(sequence), 0) FROM session_messages WHERE session_id = ?1",
            params![session_id],
            |r| r.get(0),
        )?)
    }

    pub fn message_count(&self, session_id: &str) -> Result<i64> {
        let conn = self.read();
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM session_messages WHERE session_id = ?1",
            params![session_id],
            |r| r.get(0),
        )?)
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

    // ---------------- Context Frontier ----------------

    /// The Logical Session's Context frontier. Zero when nothing has
    /// been processed — including before the first message exists.
    pub fn get_context_state(&self, session_id: &str) -> Result<SessionContextState> {
        let conn = self.read();
        let processed: i64 = conn
            .query_row(
                "SELECT processed_message_sequence FROM session_context_state WHERE session_id = ?1",
                params![session_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        Ok(SessionContextState {
            session_id: session_id.to_string(),
            processed_message_sequence: processed,
        })
    }

    pub fn set_processed_message_sequence(&self, session_id: &str, seq: i64) -> Result<()> {
        let conn = self.write();
        set_processed_message_sequence_conn(&conn, session_id, seq)
    }

    // ---------------- Member Stats ----------------

    pub fn get_member_stats(&self, member_id: &str) -> Result<Option<SessionMemberStats>> {
        let conn = self.read();
        Ok(conn
            .query_row(
                "SELECT member_id, tool_call_count, tool_error_count, compaction_count,
                        side_activity_count, input_tokens, output_tokens, cached_tokens,
                        reasoning_tokens, cost, updated_at, extra
                 FROM session_member_stats WHERE member_id = ?1",
                params![member_id],
                row_member_stats,
            )
            .optional()?)
    }

    /// Query-time aggregate over the whole execution graph: no cache
    /// table — the member count is small and this can never drift.
    pub fn aggregate_session_stats(&self, session_id: &str) -> Result<SessionAggregateStats> {
        let members = self.members_for_session(session_id)?;
        let mut agg = SessionAggregateStats {
            member_count: members.len() as i64,
            ..Default::default()
        };
        let ids: Vec<String> = members.iter().map(|m| m.id.clone()).collect();
        {
            let conn = self.read();
            for id in &ids {
                let rel: &str = members
                    .iter()
                    .find(|m| &m.id == id)
                    .map(|m| m.relation.as_str())
                    .unwrap_or("");
                match rel {
                    "child" => agg.child_count += 1,
                    "side" => agg.side_count += 1,
                    _ => {}
                }
                if let Some(s) = conn
                    .query_row(
                        "SELECT tool_call_count, tool_error_count, compaction_count,
                                    side_activity_count, input_tokens, output_tokens,
                                    cached_tokens, reasoning_tokens, cost
                             FROM session_member_stats WHERE member_id = ?1",
                        params![id],
                        |r| {
                            Ok((
                                r.get::<_, Option<i64>>(0)?,
                                r.get::<_, Option<i64>>(1)?,
                                r.get::<_, Option<i64>>(2)?,
                                r.get::<_, Option<i64>>(3)?,
                                r.get::<_, Option<i64>>(4)?,
                                r.get::<_, Option<i64>>(5)?,
                                r.get::<_, Option<i64>>(6)?,
                                r.get::<_, Option<i64>>(7)?,
                                r.get::<_, Option<f64>>(8)?,
                            ))
                        },
                    )
                    .optional()?
                {
                    agg.tool_call_count += s.0.unwrap_or(0);
                    agg.tool_error_count += s.1.unwrap_or(0);
                    agg.compaction_count += s.2.unwrap_or(0);
                    agg.side_activity_count += s.3.unwrap_or(0);
                    agg.input_tokens = or_add(agg.input_tokens, s.4);
                    agg.output_tokens = or_add(agg.output_tokens, s.5);
                    agg.cached_tokens = or_add(agg.cached_tokens, s.6);
                    agg.reasoning_tokens = or_add(agg.reasoning_tokens, s.7);
                    agg.cost = match (agg.cost, s.8) {
                        (a, Some(b)) => Some(a.unwrap_or(0.0) + b),
                        (a, None) => a,
                    };
                }
            }
        }
        // Depth over the parent chain (root = 0): member counts are tiny, so
        // a plain walk beats a recursive SQL CTE.
        let by_source: std::collections::HashMap<&str, &SessionMember> = members
            .iter()
            .map(|m| (m.source_member_id.as_str(), m))
            .collect();
        for m in &members {
            let mut depth = 0i64;
            let mut cursor: Option<&SessionMember> = Some(m);
            let mut hops = 0usize;
            while let Some(cur) = cursor {
                if cur.relation.as_str() == "root" {
                    break;
                }
                depth += 1;
                hops += 1;
                if hops > members.len() {
                    break; // defensive: never loop on a cyclic source graph
                }
                cursor = cur
                    .parent_source_member_id
                    .as_deref()
                    .and_then(|p| by_source.get(p).copied());
            }
            agg.max_depth = agg.max_depth.max(depth);
        }
        Ok(agg)
    }

    // ---------------- Ingestion Diagnostics ----------------

    /// Record (or re-observe) an unattachable source. The first sight
    /// inserts quietly; every later reconcile bumps `observation_count` so the
    /// Settings page can show only repeat offenders.
    pub fn upsert_ingestion_diagnostic(
        &self,
        agent: Agent,
        kind: &str,
        source_member_id: Option<&str>,
        parent_source_member_id: Option<&str>,
        source_path: Option<&str>,
        reason: &str,
        details: &serde_json::Value,
    ) -> Result<()> {
        let conn = self.write();
        let key = diagnostic_key(agent, kind, source_member_id);
        let ts = now();
        let n = conn.execute(
            "UPDATE ingestion_diagnostics
             SET last_seen_at = ?2, observation_count = observation_count + 1,
                 reason = ?3, parent_source_member_id = COALESCE(?4, parent_source_member_id),
                 source_path = COALESCE(?5, source_path), details = ?6
             WHERE diagnostic_key = ?1",
            params![
                key,
                ts,
                reason,
                parent_source_member_id,
                source_path,
                details.to_string()
            ],
        )?;
        if n == 0 {
            conn.execute(
                "INSERT INTO ingestion_diagnostics
                 (id, diagnostic_key, agent, kind, source_member_id, parent_source_member_id,
                  source_path, reason, first_seen_at, last_seen_at, observation_count, details)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, 1, ?10)",
                params![
                    new_id(),
                    key,
                    agent.as_str(),
                    kind,
                    source_member_id,
                    parent_source_member_id,
                    source_path,
                    reason,
                    ts,
                    details.to_string()
                ],
            )?;
        }
        Ok(())
    }

    /// The member resolved: its diagnostic goes away. Missing key is a
    /// no-op — resolution must never depend on a diagnostic having existed.
    pub fn resolve_ingestion_diagnostic(
        &self,
        agent: Agent,
        kind: &str,
        source_member_id: &str,
    ) -> Result<()> {
        let conn = self.write();
        conn.execute(
            "DELETE FROM ingestion_diagnostics WHERE diagnostic_key = ?1",
            params![diagnostic_key(agent, kind, Some(source_member_id))],
        )?;
        Ok(())
    }

    /// The Settings page list: repeat offenders only by default.
    pub fn list_ingestion_diagnostics(
        &self,
        min_observation_count: i64,
    ) -> Result<Vec<IngestionDiagnostic>> {
        let conn = self.read();
        let mut st = conn.prepare(
            "SELECT * FROM ingestion_diagnostics
             WHERE observation_count >= ?1
             ORDER BY last_seen_at DESC LIMIT 200",
        )?;
        let rows = st
            .query_map(params![min_observation_count], row_diagnostic)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---------------- Session Owner ----------------

    /// Set (or clear) the single Owner Workstream of a Session.
    /// This touches exactly one column: `sessions.owner_workstream_id`.
    /// `None` clears ownership. No implicit WorkstreamPath / cwd / Project
    /// mutation happens here.
    pub fn set_session_owner(&self, session_id: &str, workstream_id: Option<&str>) -> Result<()> {
        let conn = self.write();
        set_session_owner_conn(&conn, session_id, workstream_id)
    }

    /// Active Sessions that own `workstream_id`, most recent activity first.
    /// A Session appears in at most one Workstream's list.
    pub fn sessions_for_workstream(&self, workstream_id: &str) -> Result<Vec<Session>> {
        let conn = self.read();
        let mut st = conn.prepare(
            "SELECT * FROM sessions
             WHERE owner_workstream_id = ?1 AND trashed_at IS NULL
             ORDER BY COALESCE(last_activity_at, started_at) DESC",
        )?;
        let rows = st
            .query_map(params![workstream_id], row_session)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---------------- Context Items ----------------

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
        run_id: Option<&str>,
        source_refs: &[String],
    ) -> Result<()> {
        self.tx(|tx| {
            apply_status_change_conn(tx, item_id, new_status, actor, reason, run_id, source_refs)
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
        let conn = self.read();
        let mut st = conn.prepare(
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
        // Read authority strictly from the immutable revision metadata and frozen lineage!
        // Never let subsequent item edits pollute historical revision authority.
        let authority = resolve_revision_authority(&rev);

        // A redacted revision's session is GONE. This is not a
        // tombstone: return only the fact that the source no longer exists.
        // No session id, agent session id, title or path may resolve, and
        // nothing may redirect to a future rediscovered session.
        if rev.source_type.as_deref() == Some("deleted_session") {
            return Ok(Some(ContextSourceDetail {
                revision_id: rev.id,
                authority,
                source_type: rev.source_type,
                source_ref: None,
                sync_run_id: None,
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
            sync_run_id: rev.sync_run_id,
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

    // ---------------- Conflicts ----------------

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

    // ---------------- Sync runs ----------------

    pub fn insert_sync_run(&self, run: &SyncRun) -> Result<()> {
        let conn = self.write();
        insert_sync_run_conn(&conn, run)
    }

    /// True when an already-committed run processed exactly this delta —
    /// retries after a crash must not re-apply the same mutations.
    pub fn has_completed_run(&self, session_id: &str, delta_fingerprint: &str) -> Result<bool> {
        let conn = self.read();
        Ok(conn
            .query_row(
                "SELECT 1 FROM sync_runs WHERE session_id = ?1 AND delta_fingerprint = ?2 AND status = 'ok' LIMIT 1",
                params![session_id, delta_fingerprint],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn list_sync_runs(&self, limit: i64) -> Result<Vec<SyncRun>> {
        let conn = self.read();
        let mut st = conn.prepare(
            "SELECT id, session_id, from_sequence, to_sequence, status, mutations, summary, error, created_at, runtime, delta_fingerprint
             FROM sync_runs ORDER BY created_at DESC LIMIT ?1",
        )?;
        let rows = st
            .query_map(params![limit], row_sync_run)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---------------- Launch intents ----------------

    pub fn insert_launch_intent(&self, i: &LaunchIntent) -> Result<()> {
        let conn = self.write();
        conn.execute(
            "INSERT INTO launch_intents
             (id, launch_type, agent, owner_workstream_id, cwd, context_bundle_markdown, context_bundle_revisions, process_id, launched_at, matched_session_id, status, note, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                i.id, i.launch_type, i.agent.as_str(),
                i.owner_workstream_id,
                i.cwd, i.context_bundle_markdown, i.context_bundle_revisions, i.process_id.map(|p| p as i64),
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
            "SELECT id, launch_type, agent, owner_workstream_id, cwd, context_bundle_markdown, process_id, launched_at, matched_session_id, status, note, created_at, updated_at, context_bundle_revisions
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

    // ---------------- Context deliveries ----------------

    pub fn record_delivery(&self, d: &ContextDelivery) -> Result<()> {
        let conn = self.write();
        record_delivery_conn(&conn, d)
    }

    /// Latest successful delivery per workstream for a session.
    pub fn latest_deliveries(&self, session_id: &str) -> Result<Vec<ContextDelivery>> {
        let conn = self.read();
        let mut st = conn.prepare(
            "SELECT id, session_id, workstream_id, bundle_id, delivered_revisions, delivered_conflicts, delivered_at
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

    // ---------------- Settings ----------------

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

    // ---------------- Assistant ----------------

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

    // ---------------- Agent installations cache ----------------

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

    // ---------------- FTS ----------------

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
    /// because message identity dedup happens at the message layer,
    /// incremental batches never need to wipe the session's earlier index
    /// rows. Every SessionMessage is indexed — messages ARE the curated
    /// conversation, so no length pre-filter stands between a short
    /// constraint and findability.
    pub fn index_new_messages(&self, messages: &[SessionMessage]) -> Result<()> {
        let conn = self.write();
        for m in messages {
            unindex_conn(&conn, "message", &m.id);
        }
        let mut st = conn.prepare(
            "INSERT INTO search_index (kind, ref_id, parent_id, title, body) VALUES ('message', ?1, ?2, '', ?3)",
        )?;
        for m in messages {
            st.execute(params![m.id, m.session_id, m.content])?;
        }
        Ok(())
    }

    /// Fill in the index rows no incremental write produced: messages whose
    /// ingestion-time indexing was skipped, and Session documents that no
    /// write has touched yet. Idempotent.
    ///
    /// Only ACTIVE sessions are indexed. This runs at every
    /// startup, so an unguarded run would silently re-index everything a
    /// Trash unindexed — the recycle bin would leak back into search after
    /// every restart. `session_lifecycle::unindex_session_conn` and this WHERE
    /// clause are two halves of one lifecycle invariant.
    pub fn backfill_search_index(&self) -> Result<()> {
        let conn = self.write();
        conn.execute(
            "INSERT INTO search_index (kind, ref_id, parent_id, title, body)
             SELECT 'message', m.id, m.session_id, '', m.content
             FROM session_messages m
             WHERE m.session_id IN (SELECT id FROM sessions WHERE trashed_at IS NULL)
               AND m.id NOT IN (
                   SELECT ref_id FROM search_index WHERE kind = 'message')",
            [],
        )?;
        // Fill in the Session documents that are missing entirely. A
        // document whose body can change (title, Project, Owner Workstream) is
        // refreshed at the write that changed it, so the backfill only has to
        // cover rows no write ever touched.
        let ids: Vec<String> = {
            let mut st = conn.prepare(
                "SELECT id FROM sessions
                  WHERE trashed_at IS NULL
                    AND id NOT IN (SELECT ref_id FROM search_index WHERE kind = 'session')",
            )?;
            let mapped = st.query_map([], |r| r.get::<_, String>(0))?;
            mapped.collect::<std::result::Result<Vec<_>, _>>()?
        };
        for id in ids {
            index_session_conn(&conn, &id)?;
        }
        Ok(())
    }

    // ---------------- helpers ----------------

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
    /// session has a title. Discovery uses this to skip re-parsing sources
    /// that cannot have changed since the last pass, so a reconcile pass costs
    /// O(changed sources), not O(all history). Only titled sessions are listed:
    /// an untitled row may just predate a title source, and re-parsing its
    /// (unchanged) file is exactly what heals it.
    pub fn member_source_skipset(
        &self,
    ) -> Result<std::collections::HashMap<String, (String, i64, Option<f64>)>> {
        let conn = self.read();
        let mut st = conn.prepare(
            "SELECT m.source_path, c.source_file_identity, c.last_seen_size, c.mtime
             FROM session_members m
             JOIN session_member_cursors c ON c.member_id = m.id
             JOIN sessions s ON s.id = m.session_id
             WHERE s.title IS NOT NULL AND c.source_file_identity != ''",
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

// connection-level helpers --------------------------------------------------
// Free functions over &Connection so Db methods and `Db::tx` closures share
// exactly the same SQL paths.

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
           -- write: losing `.git` on one path must not detach a Project,
           -- and pointing an existing Project at a different family is a Policy
           -- decision, not a save side effect. The one legitimate writer is
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
    // A Session's search document carries its Owner's title, so a rename
    // has to reach every Session that owns this Workstream. The previous title
    // is read first: an update that changed nothing else must not rewrite N
    // documents.
    let previous_title: Option<String> = conn
        .query_row(
            "SELECT title FROM workstreams WHERE id = ?1",
            params![w.id],
            |r| r.get(0),
        )
        .optional()?;
    conn.execute(
        "INSERT INTO workstreams (id, title, description, lifecycle, visibility, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
           title = ?2, description = ?3, lifecycle = ?4, visibility = ?5, updated_at = ?7",
        params![w.id, w.title, w.description, w.lifecycle, w.visibility, w.created_at, w.updated_at],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO workstream_review_state (workstream_id, reviewed_through_at, reviewed_boundary_change_ids, reviewed_at)
         VALUES (?1, ?2, '[]', ?2)",
        params![w.id, w.created_at],
    )?;
    if previous_title.as_deref() != Some(w.title.as_str()) {
        reindex_owned_sessions_conn(conn, &w.id)?;
    }
    Ok(())
}

/// Set (or clear) a Session's single Owner Workstream through a caller-held
/// connection, so a launch match can write ownership together with everything
/// that must agree with it in ONE transaction.
///
/// The target Workstream is checked to exist: a Session pointing at a row that
/// is not there would be a silently broken owner. The search document is
/// refreshed inside the same connection — an ownership change that never
/// reached the index would leave the Session searchable under its old label.
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
    let changed = conn.execute(
        "UPDATE sessions SET owner_workstream_id = ?2 WHERE id = ?1",
        params![session_id, workstream_id],
    )?;
    if changed == 0 {
        return Err(other(format!("未知 Session: {session_id}")));
    }
    index_session_conn(conn, session_id)
}

/// Record one Context delivery through a caller-held connection. The matched
/// Session already received this bundle, so this is normally written inside
/// the match transaction, not as a step of its own.
pub fn record_delivery_conn(conn: &Connection, d: &ContextDelivery) -> Result<()> {
    conn.execute(
        "INSERT INTO context_deliveries (id, session_id, workstream_id, bundle_id, delivered_revisions, delivered_conflicts, delivered_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            d.id,
            d.session_id,
            d.workstream_id,
            d.bundle_id,
            serde_json::to_string(&d.delivered_revisions)?,
            serde_json::to_string(&d.delivered_conflicts)?,
            d.delivered_at,
        ],
    )?;
    Ok(())
}

/// Read one LaunchIntent through a caller-held connection — inside a matching
/// transaction the *current* row decides, never the copy read before it.
pub fn get_launch_intent_conn(conn: &Connection, id: &str) -> Result<Option<LaunchIntent>> {
    Ok(conn
        .query_row(
            "SELECT id, launch_type, agent, owner_workstream_id, cwd, context_bundle_markdown, process_id, launched_at, matched_session_id, status, note, created_at, updated_at, context_bundle_revisions
             FROM launch_intents WHERE id = ?1",
            params![id],
            row_launch_intent,
        )
        .optional()?)
}

/// Consume a LaunchIntent's match, exactly once: the row must still be waiting
/// (PENDING or AMBIGUOUS) and unclaimed. Returns false when someone else got
/// there first — the caller must then abandon its own match rather than
/// overwrite theirs. A plain `UPDATE … WHERE id` would let two resolvers both
/// succeed and leave one Session owned by an intent that records the other.
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
/// another status, and report whether it still was.
///
/// Every transition that is not the match itself goes through here: parking a
/// tie as AMBIGUOUS, expiring a stale intent, and recording the pid note after
/// spawning the Agent. None of them may resurrect or overwrite a match —
/// "still waiting" written over an already-consumed intent would make it
/// consumable a second time and hand out an Owner twice.
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

/// Rebuild the search documents of the Sessions that OWN `workstream_id`.
///
/// A Session's document embeds its Owner Workstream's title, so anything
/// that changes that title (a rename) or the ownership itself (a Workstream
/// deletion nulling it) leaves every owned Session's document describing a fact
/// that is no longer true. The caller runs this in the same transaction as the
/// change.
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

/// Rebuild the search documents of the Sessions that project onto
/// `project_id`. Same rule as [`reindex_owned_sessions_conn`] for the other
/// input of a Session document's body — the Project name.
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

/// Write (or refresh) one Session's search document.
///
/// Title is the Session title (falling back to the Agent name); body is the
/// Project name plus the ONE Owner Workstream title. A Session has a single
/// Owner, so there is exactly one Workstream title to store.
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

/// Insert or refresh one execution member through a caller-held connection,
/// so a graph-resolution pass can create members in the same transaction as
/// the session they attach to.
#[allow(clippy::too_many_arguments)]
pub fn upsert_session_member_conn(
    conn: &Connection,
    session_id: &str,
    agent: Agent,
    source_member_id: &str,
    relation: SessionMemberRelation,
    parent_source_member_id: Option<&str>,
    source_kind: &str,
    source_path: &str,
    cwd: Option<&str>,
    started_at: Option<&str>,
    last_activity_at: Option<&str>,
    metadata: &serde_json::Value,
) -> Result<String> {
    let existing: Option<(String, String, Option<String>)> = conn
        .query_row(
            "SELECT id, relation, last_activity_at FROM session_members
                 WHERE agent = ?1 AND source_member_id = ?2",
            params![agent.as_str(), source_member_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((id, existing_relation, previous_activity)) = existing {
        // Topology guard: a member's root-ness never flips. A stored Root
        // re-claimed as child/side (or the reverse) would re-home the member
        // and strand its old Logical Session without a root member; such a
        // claim is a source bug and is refused here — the row stays exactly
        // as stored. child ↔ side changes stay allowed.
        let root_flip = (existing_relation == SessionMemberRelation::Root.as_str())
            != (relation == SessionMemberRelation::Root);
        if root_flip {
            return Ok(id);
        }
        let activity = later_timestamp(previous_activity.as_deref(), last_activity_at);
        conn.execute(
            "UPDATE session_members SET
               session_id = ?2,
               relation = ?3,
               parent_source_member_id = ?4,
               source_kind = ?5,
               source_path = ?6,
               cwd = COALESCE(?7, cwd),
               started_at = COALESCE(started_at, ?8),
               last_activity_at = ?9,
               metadata = ?10
             WHERE id = ?1",
            params![
                id,
                session_id,
                relation.as_str(),
                parent_source_member_id,
                source_kind,
                source_path,
                cwd,
                started_at,
                activity,
                metadata.to_string()
            ],
        )?;
        return Ok(id);
    }
    let id = new_id();
    conn.execute(
        "INSERT INTO session_members
         (id, session_id, agent, source_member_id, relation, parent_source_member_id,
          source_kind, source_path, cwd, started_at, last_activity_at, metadata)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            id,
            session_id,
            agent.as_str(),
            source_member_id,
            relation.as_str(),
            parent_source_member_id,
            source_kind,
            source_path,
            cwd,
            started_at,
            last_activity_at,
            metadata.to_string()
        ],
    )?;
    Ok(id)
}

pub fn upsert_member_cursor_conn(conn: &Connection, c: &SessionMemberCursor) -> Result<()> {
    conn.execute(
        "INSERT INTO session_member_cursors
         (member_id, source_file_identity, generation, byte_offset, last_seen_size, mtime, prefix_hash, identity_tail_hash, active_provider, active_model)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(member_id) DO UPDATE SET
           source_file_identity = ?2, generation = ?3, byte_offset = ?4,
           last_seen_size = ?5, mtime = ?6, prefix_hash = ?7, identity_tail_hash = ?8,
           active_provider = ?9, active_model = ?10",
        params![
            c.member_id,
            c.source_file_identity,
            c.generation,
            c.byte_offset as i64,
            c.last_seen_size as i64,
            c.mtime,
            c.prefix_hash,
            c.identity_tail_hash,
            c.active_provider,
            c.active_model
        ],
    )?;
    Ok(())
}

/// Provenance enrichment on a dedup hit. The message has
/// already been recognised as the same one (identity hash matches), so a
/// newly read provenance that the stored row lacks fills the gap in place; a
/// contradiction with an already-confirmed value keeps the stored one and
/// logs a warning instead of overwriting — the conflict means adapter
/// interpretation or source behavior drifted, and the first version
/// deliberately adds no new diagnostic type.
fn enrich_message_provenance_conn(
    conn: &Connection,
    member_id: &str,
    hash: &str,
    m: &ParsedSessionMessage,
) -> Result<()> {
    let Some((stored_provider, stored_model)): Option<(Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT provider, model FROM session_messages
             WHERE member_id = ?1 AND source_identity_hash = ?2",
            params![member_id, hash],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
    else {
        return Ok(());
    };
    for (field, stored, next) in [
        ("provider", &stored_provider, &m.provider),
        ("model", &stored_model, &m.model),
    ] {
        match (stored, next) {
            (None, Some(v)) => {
                conn.execute(
                    &format!("UPDATE session_messages SET {field} = ?1 WHERE member_id = ?2 AND source_identity_hash = ?3"),
                    params![v, member_id, hash],
                )?;
            }
            (Some(a), Some(b)) if a != b => {
                eprintln!(
                    "[ingest] provenance conflict on message (member {member_id}): stored {field}={a:?}, source now says {b:?}; keeping stored value"
                );
            }
            _ => {}
        }
    }
    Ok(())
}

/// Apply a stats update to one member's 1:1 snapshot row. A DELTA adds
/// its observed counts; a SNAPSHOT replaces the four observed counters. Never
/// called with `None` from callers that had nothing to say — but treated as a
/// no-op here so "no evidence" can never zero a column.
pub fn apply_stats_conn(
    conn: &Connection,
    member_id: &str,
    update: Option<StatsUpdate>,
) -> Result<bool> {
    let Some(update) = update else {
        return Ok(false);
    };
    // Dynamic SET list: only the fields this update speaks for move, so an
    // absent observation leaves the column (and its NULL-vs-0 meaning) alone.
    // A delta adds over COALESCE: a NULL column (never observed) starts from 0.
    let mut sets: Vec<String> = Vec::new();
    match update {
        StatsUpdate::Delta(d) => {
            if let Some(v) = d.tool_call_count {
                sets.push(format!(
                    "tool_call_count = COALESCE(tool_call_count, 0) + {v}"
                ));
            }
            if let Some(v) = d.tool_error_count {
                sets.push(format!(
                    "tool_error_count = COALESCE(tool_error_count, 0) + {v}"
                ));
            }
            if let Some(v) = d.compaction_count {
                sets.push(format!(
                    "compaction_count = COALESCE(compaction_count, 0) + {v}"
                ));
            }
            if let Some(v) = d.side_activity_count {
                sets.push(format!(
                    "side_activity_count = COALESCE(side_activity_count, 0) + {v}"
                ));
            }
        }
        StatsUpdate::Snapshot(s) => {
            if let Some(v) = s.tool_call_count {
                sets.push(format!("tool_call_count = {v}"));
            }
            if let Some(v) = s.tool_error_count {
                sets.push(format!("tool_error_count = {v}"));
            }
            if let Some(v) = s.compaction_count {
                sets.push(format!("compaction_count = {v}"));
            }
            if let Some(v) = s.side_activity_count {
                sets.push(format!("side_activity_count = {v}"));
            }
        }
    }
    if sets.is_empty() {
        return Ok(false);
    }
    conn.execute(
        "INSERT INTO session_member_stats
         (member_id, tool_call_count, tool_error_count, compaction_count, side_activity_count, updated_at)
         VALUES (?1, NULL, NULL, NULL, NULL, ?2)
         ON CONFLICT(member_id) DO NOTHING",
        params![member_id, now()],
    )?;
    let sql = format!(
        "UPDATE session_member_stats SET {}, updated_at = ?1 WHERE member_id = ?2",
        sets.join(", ")
    );
    conn.execute(&sql, params![now(), member_id])?;
    Ok(true)
}

pub fn get_messages_conn(
    conn: &Connection,
    session_id: &str,
    after: Option<i64>,
    limit: i64,
) -> Result<Vec<SessionMessage>> {
    let mut st = conn.prepare(
        "SELECT * FROM session_messages WHERE session_id = ?1 AND sequence > ?2
         ORDER BY sequence LIMIT ?3",
    )?;
    let rows = st
        .query_map(params![session_id, after.unwrap_or(0), limit], row_message)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn set_processed_message_sequence_conn(
    conn: &Connection,
    session_id: &str,
    seq: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO session_context_state (session_id, processed_message_sequence) VALUES (?1, ?2)
         ON CONFLICT(session_id) DO UPDATE SET processed_message_sequence = ?2",
        params![session_id, seq],
    )?;
    Ok(())
}

/// The stable diagnostic identity of an unattachable member: one row
/// per (agent, kind, source member), so repeat sightings update instead of
/// duplicating.
pub fn diagnostic_key(agent: Agent, kind: &str, source_member_id: Option<&str>) -> String {
    format!(
        "{}:{}:{}",
        agent.as_str(),
        kind,
        source_member_id.unwrap_or("-")
    )
}

/// `Some(a) + Some(b)`; `None` never erases a known total.
fn or_add(current: Option<i64>, next: Option<i64>) -> Option<i64> {
    match (current, next) {
        (a, Some(b)) => Some(a.unwrap_or(0) + b),
        (a, None) => a,
    }
}

/// Query-time aggregate over a session's execution graph.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SessionAggregateStats {
    pub member_count: i64,
    pub child_count: i64,
    pub side_count: i64,
    pub max_depth: i64,
    pub tool_call_count: i64,
    pub tool_error_count: i64,
    pub compaction_count: i64,
    pub side_activity_count: i64,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cached_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub cost: Option<f64>,
    // model / provider / effort removed: aggregate
    // "session model" was a guess; per-model counts derive from
    // session_messages WHERE role = 'assistant' when the UI needs them.
}

pub fn insert_sync_run_conn(conn: &Connection, run: &SyncRun) -> Result<()> {
    conn.execute(
        "INSERT INTO sync_runs (id, session_id, from_sequence, to_sequence, status, mutations, summary, error, created_at, runtime, delta_fingerprint)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![run.id, run.session_id, run.from_sequence, run.to_sequence, run.status,
                run.mutations.to_string(), run.summary, run.error, run.created_at, run.runtime,
                run.delta_fingerprint],
    )?;
    Ok(())
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

/// Authoritative resolver for revision authority:
/// 1. provenance authority stored directly in revision metadata
/// 2. audit metadata / source_type / sync_run_id inference if determinable
/// 3. [`authority::UNKNOWN`] — the provenance does not say
///
/// NEVER falls back to mutable item.authority!
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
    } else if rev.source_type.as_deref() == Some("session_message") || rev.sync_run_id.is_some() {
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
    // 1. Current left
    let current_left_item = get_item_conn(conn, &conflict.left_item_id)?;
    let current_left = current_left_item
        .as_ref()
        .and_then(|i| i.current_revision_id.as_deref())
        .and_then(|rid| get_revision_conn(conn, rid).ok().flatten())
        .map(|rev| revision_snapshot_from_rev(&rev));

    // 2. Frozen left at conflict
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

    // 3. Current right
    let current_right_item = conflict
        .right_item_id
        .as_deref()
        .and_then(|rid| get_item_conn(conn, rid).ok().flatten());
    let current_right = current_right_item
        .as_ref()
        .and_then(|i| i.current_revision_id.as_deref())
        .and_then(|rid| get_revision_conn(conn, rid).ok().flatten())
        .map(|rev| revision_snapshot_from_rev(&rev));

    // 4. Frozen right at conflict
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

    // 5. Candidate snapshot at conflict
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

// row mappers -------------------------------------------------------------
//
// Name-based (`r.get("col")`) wherever a struct is wide or growing: a
// positional mapper silently re-types every field after it when someone adds a
// column to the SELECT list.

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
        lifecycle: r.get("lifecycle")?,
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
        trashed_at: r.get("trashed_at")?,
    })
}

fn row_member(r: &Row) -> rusqlite::Result<SessionMember> {
    Ok(SessionMember {
        id: r.get("id")?,
        session_id: r.get("session_id")?,
        agent: Agent::parse(&r.get::<_, String>("agent")?).unwrap_or(Agent::Codex),
        source_member_id: r.get("source_member_id")?,
        relation: SessionMemberRelation::parse(&r.get::<_, String>("relation")?)
            .unwrap_or(SessionMemberRelation::Child),
        parent_source_member_id: r.get("parent_source_member_id")?,
        source_kind: r.get("source_kind")?,
        source_path: r.get("source_path")?,
        cwd: r.get("cwd")?,
        started_at: r.get("started_at")?,
        last_activity_at: r.get("last_activity_at")?,
        metadata: serde_json::from_str(&r.get::<_, String>("metadata")?).unwrap_or_default(),
    })
}

fn row_message(r: &Row) -> rusqlite::Result<SessionMessage> {
    Ok(SessionMessage {
        id: r.get("id")?,
        session_id: r.get("session_id")?,
        member_id: r.get("member_id")?,
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
        provider: r.get("provider")?,
        model: r.get("model")?,
        raw_ref: r.get("raw_ref")?,
    })
}

fn row_member_stats(r: &Row) -> rusqlite::Result<SessionMemberStats> {
    Ok(SessionMemberStats {
        member_id: r.get(0)?,
        tool_call_count: r.get(1)?,
        tool_error_count: r.get(2)?,
        compaction_count: r.get(3)?,
        side_activity_count: r.get(4)?,
        input_tokens: r.get(5)?,
        output_tokens: r.get(6)?,
        cached_tokens: r.get(7)?,
        reasoning_tokens: r.get(8)?,
        cost: r.get(9)?,
        updated_at: r.get(10)?,
        extra: serde_json::from_str(&r.get::<_, String>(11)?).unwrap_or_default(),
    })
}

fn row_diagnostic(r: &Row) -> rusqlite::Result<IngestionDiagnostic> {
    Ok(IngestionDiagnostic {
        id: r.get("id")?,
        diagnostic_key: r.get("diagnostic_key")?,
        agent: Agent::parse(&r.get::<_, String>("agent")?).unwrap_or(Agent::Codex),
        kind: r.get("kind")?,
        source_member_id: r.get("source_member_id")?,
        parent_source_member_id: r.get("parent_source_member_id")?,
        source_path: r.get("source_path")?,
        reason: r.get("reason")?,
        first_seen_at: r.get("first_seen_at")?,
        last_seen_at: r.get("last_seen_at")?,
        observation_count: r.get("observation_count")?,
        details: serde_json::from_str(&r.get::<_, String>("details")?).unwrap_or_default(),
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

fn row_delivery(r: &Row) -> rusqlite::Result<ContextDelivery> {
    Ok(ContextDelivery {
        id: r.get(0)?,
        session_id: r.get(1)?,
        workstream_id: r.get(2)?,
        bundle_id: r.get(3)?,
        delivered_revisions: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default(),
        delivered_conflicts: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or_default(),
        delivered_at: r.get(6)?,
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
    /// Defaults to Active: trashed Sessions are hidden from every default
    /// projection. The recycle bin passes Trash explicitly.
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
    sync_run_id: Option<String>,
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
        } else if sync_run_id.is_some()
            || created_by.starts_with("sync:")
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

    // 1. Revisions for items belonging to this workstream
    let sql_revs = "SELECT r.id, r.item_id, r.title, r.metadata, r.source_type, r.sync_run_id, r.created_at,
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
        let sync_run_id: Option<String> = r.get(5)?;
        let created_at: String = r.get(6)?;
        let authority: String = r.get(7)?;
        let created_by: String = r.get(8)?;
        let first_created_at: Option<String> = r.get(9)?;
        Ok((
            id,
            item_id,
            title,
            metadata_str,
            source_type,
            sync_run_id,
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
            sync_run_id,
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
            sync_run_id,
            created_at,
            created_by,
            first_created_at,
        ));
    }

    // 2. Conflicts for this workstream
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

    // 3. Conflict resolution events from context_conflict_events table
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

    // Sort descending by created_at
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

    // 1. Revisions for items in this workstream with created_at >= frontier.through_at
    let sql_revs = "SELECT r.id, r.item_id, r.title, r.metadata, r.source_type, r.sync_run_id, r.created_at,
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
        let sync_run_id: Option<String> = r.get(5)?;
        let created_at: String = r.get(6)?;
        let authority: String = r.get(7)?;
        let created_by: String = r.get(8)?;
        let first_created_at: Option<String> = r.get(9)?;
        Ok((
            id,
            item_id,
            title,
            metadata_str,
            source_type,
            sync_run_id,
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
            sync_run_id,
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
            sync_run_id,
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

    // 2. Conflicts for this workstream with created_at >= frontier.through_at
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

    // 3. Conflict resolution events for this workstream with created_at >= frontier.through_at
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

    // Sort chronologically and deterministically
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
        // If the workstream exists but has no review state row, initialize baseline
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
            sync_run_id: None,
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
