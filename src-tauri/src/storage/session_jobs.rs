//! Session lifecycle storage (Session Lifecycle & Deletion v0.1).
//!
//! Owns the SQL behind three lifecycle concerns:
//! - **Trash / Restore** — one guarded `UPDATE` on `sessions.trashed_at`,
//!   plus the FTS unindex / reindex of the session's events.
//! - **Deletion jobs** — the transient `session_deletion_jobs` coordination
//!   rows and their crash recovery.
//! - **Permanent purge** — the single-transaction cleanup of every
//!   Session-owned row and the in-place redaction of Context provenance that
//!   pointed at the dying session (方案 §24–§31).
//!
//! Free functions over `&Connection` / `&Transaction`, matching the `_conn`
//! convention, so `Db` methods and `Db::tx` closures share the same paths.

use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};

use crate::domain::SessionDeletionJob;
use crate::error::{other, Result};

// ---------------- Trash / Restore ----------------

/// The single lifecycle transition Normal → Trash. The `trashed_at IS NULL`
/// guard makes trash idempotent and makes a trash racing a restore commit
/// exactly one transition. `Ok(false)` = session missing or already trashed.
pub fn trash_session_conn(conn: &Connection, session_id: &str, ts: &str) -> Result<bool> {
    let n = conn.execute(
        "UPDATE sessions SET trashed_at = ?2 WHERE id = ?1 AND trashed_at IS NULL",
        params![session_id, ts],
    )?;
    Ok(n > 0)
}

/// Trash → Normal, refusing while any deletion job still exists (方案 §42:
/// restore requires an explicit cancel first — even a failed job, so the
/// user sees and dismisses the failure instead of silently abandoning it).
pub fn restore_session_conn(conn: &Connection, session_id: &str) -> Result<bool> {
    let job = conn
        .query_row(
            "SELECT 1 FROM session_deletion_jobs WHERE session_id = ?1",
            params![session_id],
            |_| Ok(()),
        )
        .optional()?;
    if job.is_some() {
        return Err(other("存在未完成的永久删除任务，请先取消该任务再恢复会话"));
    }
    let n = conn.execute(
        "UPDATE sessions SET trashed_at = NULL WHERE id = ?1 AND trashed_at IS NOT NULL",
        params![session_id],
    )?;
    Ok(n > 0)
}

/// Commit-time guard for every Session write path (方案 §43): the session
/// must exist AND be Normal, otherwise work prepared against it (events,
/// cursors, sync runs, context mutations) must not be committed.
pub fn session_is_writable_conn(conn: &Connection, session_id: &str) -> Result<bool> {
    // The turbofish pins the column reader to `Option<String>` so
    // `.optional()`'s outer Option means ROW PRESENCE: `Some(None)` is an
    // existing, Normal session; `None` is a vanished row.
    let trashed: Option<Option<String>> = conn
        .query_row(
            "SELECT trashed_at FROM sessions WHERE id = ?1",
            params![session_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?;
    Ok(matches!(trashed, Some(None)))
}

// ---------------- FTS ----------------

/// A missing `search_index` table (FTS-less build) is fine; any other SQL
/// failure is a real error and must roll the caller's transaction back.
fn tolerate_missing_fts(result: std::result::Result<usize, rusqlite::Error>) -> Result<()> {
    match result {
        Ok(_) => Ok(()),
        Err(e) if e.to_string().contains("no such table") => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Drop a trashed / deleted session's event rows from the FTS index. Event
/// rows carry `parent_id = session_id`, so one delete covers the session.
///
/// Errors PROPAGATE (review P1-1): inside the lifecycle transaction a failed
/// index write rolls the lifecycle flip back, so "trashed" and "unindexed"
/// really do commit atomically. Only a missing virtual table (FTS-less
/// build) is tolerated.
pub fn unindex_session_conn(conn: &Connection, session_id: &str) -> Result<()> {
    tolerate_missing_fts(conn.execute(
        "DELETE FROM search_index WHERE kind = 'event' AND parent_id = ?1",
        params![session_id],
    ))?;
    // §39 — the Session's own document goes with it (Trash and permanent
    // deletion both route through here; a Restore rebuilds it).
    tolerate_missing_fts(conn.execute(
        "DELETE FROM search_index WHERE kind = 'session' AND ref_id = ?1",
        params![session_id],
    ))
}

/// Rebuild the FTS rows of a restored session from the durable event store,
/// mirroring `backfill_search_index`'s row shape. Errors propagate like
/// [`unindex_session_conn`]; an FTS-less build stays on the LIKE fallback.
pub fn reindex_session_conn(conn: &Connection, session_id: &str) -> Result<()> {
    tolerate_missing_fts(conn.execute(
        "DELETE FROM search_index WHERE kind = 'event' AND parent_id = ?1",
        params![session_id],
    ))?;
    crate::storage::index_session_conn(conn, session_id)?;
    tolerate_missing_fts(conn.execute(
        "INSERT INTO search_index (kind, ref_id, parent_id, title, body)
         SELECT 'event', session_id || ':' || sequence, session_id, '', text
         FROM session_events
         WHERE session_id = ?1 AND length(COALESCE(text, '')) >= 20",
        params![session_id],
    ))
}

// ---------------- Preview counts ----------------

/// What a permanent deletion will remove, for the confirmation preview
/// (方案 §20). Workstreams / WorkstreamPaths / WorkspacePaths / Projects and
/// surviving Context content are deliberately absent — they are KEPT.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PermanentDeletionCounts {
    pub event_count: i64,
    pub sync_run_count: i64,
    pub context_delivery_count: i64,
    pub launch_intent_count: i64,
    /// Revisions whose provenance will be redacted to `deleted_session`
    /// (their content survives — only the source pointers die).
    pub context_revision_redaction_count: i64,
}

impl PermanentDeletionCounts {
    pub fn collect(conn: &Connection, session_id: &str) -> Result<Self> {
        let count = |sql: &str| -> Result<i64> {
            Ok(conn.query_row(sql, params![session_id], |r| r.get(0))?)
        };
        Ok(Self {
            event_count: count("SELECT COUNT(*) FROM session_events WHERE session_id = ?1")?,
            sync_run_count: count("SELECT COUNT(*) FROM sync_runs WHERE session_id = ?1")?,
            context_delivery_count: count(
                "SELECT COUNT(*) FROM context_deliveries WHERE session_id = ?1",
            )?,
            launch_intent_count: count(
                "SELECT COUNT(*) FROM launch_intents WHERE matched_session_id = ?1",
            )?,
            context_revision_redaction_count: count_redactable_revisions_conn(conn, session_id)?,
        })
    }
}

// ---------------- Deletion jobs ----------------

fn row_job(r: &Row) -> rusqlite::Result<SessionDeletionJob> {
    Ok(SessionDeletionJob {
        id: r.get("id")?,
        session_id: r.get("session_id")?,
        state: r.get("state")?,
        plan_json: r.get("plan_json")?,
        last_error: r.get("last_error")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
    })
}

/// Store a freshly prepared job, replacing any previous job for the same
/// session (re-prepare after failed / stale is a sanctioned transition,
/// 方案 §42; the caller refuses to clobber `deleting_source`).
pub fn insert_deletion_job_conn(conn: &Connection, job: &SessionDeletionJob) -> Result<()> {
    conn.execute(
        "DELETE FROM session_deletion_jobs WHERE session_id = ?1",
        params![job.session_id],
    )?;
    conn.execute(
        "INSERT INTO session_deletion_jobs
           (id, session_id, state, plan_json, last_error, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            job.id,
            job.session_id,
            job.state,
            job.plan_json,
            job.last_error,
            job.created_at,
            job.updated_at
        ],
    )?;
    Ok(())
}

pub fn get_deletion_job_conn(
    conn: &Connection,
    job_id: &str,
) -> Result<Option<SessionDeletionJob>> {
    Ok(conn
        .query_row(
            "SELECT * FROM session_deletion_jobs WHERE id = ?1",
            params![job_id],
            row_job,
        )
        .optional()?)
}

pub fn get_job_for_session_conn(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<SessionDeletionJob>> {
    Ok(conn
        .query_row(
            "SELECT * FROM session_deletion_jobs WHERE session_id = ?1",
            params![session_id],
            row_job,
        )
        .optional()?)
}

/// Cancel = remove the coordination row. The session simply stays in Trash.
pub fn delete_deletion_job_conn(conn: &Connection, job_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM session_deletion_jobs WHERE id = ?1",
        params![job_id],
    )?;
    Ok(())
}

pub fn set_job_state_conn(
    conn: &Connection,
    job_id: &str,
    state: &str,
    last_error: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE session_deletion_jobs
         SET state = ?2, last_error = COALESCE(?3, last_error), updated_at = ?4
         WHERE id = ?1",
        params![job_id, state, last_error, crate::storage::now()],
    )?;
    Ok(())
}

/// §23 crash recovery: a `deleting_source` job found at startup was
/// interrupted mid-flight (the source file may or may not already be gone).
/// It is converted to `failed` — never auto-continued — so the user's retry
/// revalidates the source (AlreadyAbsent → the purge completes normally).
pub fn recover_interrupted_deletion_jobs(conn: &Connection) -> Result<usize> {
    let n = conn.execute(
        "UPDATE session_deletion_jobs
         SET state = 'failed',
             last_error = '上一次永久删除在执行中被中断，请重试或取消',
             updated_at = ?1
         WHERE state = 'deleting_source'",
        params![crate::storage::now()],
    )?;
    Ok(n)
}

// ---------------- Provenance redaction ----------------

/// Session-linked provenance keys, collected while the event store and sync
/// history are still readable (方案 §26 step 1).
struct SessionProvenance {
    /// Stable event refs: `session-event:<event-id>`.
    event_refs: Vec<String>,
    sync_run_ids: Vec<String>,
}

fn collect_session_provenance(conn: &Connection, session_id: &str) -> Result<SessionProvenance> {
    let mut event_refs = Vec::new();
    {
        let mut st = conn.prepare("SELECT id FROM session_events WHERE session_id = ?1")?;
        let rows = st.query_map(params![session_id], |r| r.get::<_, String>(0))?;
        for id in rows {
            event_refs.push(format!("session-event:{}", id?));
        }
    }
    let mut sync_run_ids = Vec::new();
    {
        let mut st = conn.prepare("SELECT id FROM sync_runs WHERE session_id = ?1")?;
        let rows = st.query_map(params![session_id], |r| r.get::<_, String>(0))?;
        for id in rows {
            sync_run_ids.push(id?);
        }
    }
    Ok(SessionProvenance {
        event_refs,
        sync_run_ids,
    })
}

/// Every string inside a revision's metadata that claims to reference a
/// session event: `provenance.source_ref`, top-level `source_refs` and
/// `audit.source_refs` (the three shapes writers actually produce).
fn metadata_event_refs(metadata: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(s) = metadata
        .get("provenance")
        .and_then(|p| p.get("source_ref"))
        .and_then(|v| v.as_str())
    {
        out.push(s.to_string());
    }
    for arr in [
        metadata.get("source_refs"),
        metadata.get("audit").and_then(|a| a.get("source_refs")),
    ] {
        if let Some(arr) = arr.and_then(|v| v.as_array()) {
            for v in arr {
                if let Some(s) = v.as_str() {
                    out.push(s.to_string());
                }
            }
        }
    }
    out
}

/// A revision belongs to the dying session when its `sync_run_id` is one of
/// the session's SyncRuns, its `source_ref` resolves to one of its events, or
/// its metadata mentions one of those refs (§27).
fn revision_matches_provenance(
    source_ref: Option<&str>,
    sync_run_id: Option<&str>,
    metadata: &serde_json::Value,
    prov: &SessionProvenance,
) -> bool {
    if let Some(r) = sync_run_id {
        if prov.sync_run_ids.iter().any(|id| id == r) {
            return true;
        }
    }
    if let Some(r) = source_ref {
        if prov.event_refs.iter().any(|e| e == r) {
            return true;
        }
    }
    metadata_event_refs(metadata)
        .iter()
        .any(|m| prov.event_refs.iter().any(|e| e == m))
}

/// The scrubbed metadata: session-derived references removed, authority /
/// actor / status audit preserved (authority resolution reads
/// `provenance.authority` and `audit.actor` — dropping those would degrade
/// every redacted revision to `unknown`, 方案 §27 保留项).
fn redacted_metadata(metadata: &serde_json::Value) -> String {
    let mut m = metadata.clone();
    if let Some(obj) = m.as_object_mut() {
        obj.remove("source_refs");
        if let Some(audit) = obj.get_mut("audit").and_then(|a| a.as_object_mut()) {
            audit.remove("source_refs");
        }
        if let Some(prov) = obj.get_mut("provenance").and_then(|p| p.as_object_mut()) {
            prov.remove("source_ref");
            prov.insert(
                "source_type".to_string(),
                serde_json::json!("deleted_session"),
            );
        }
    }
    m.to_string()
}

/// How many revisions would be redacted for this session (preview, §20).
pub fn count_redactable_revisions_conn(conn: &Connection, session_id: &str) -> Result<i64> {
    let prov = collect_session_provenance(conn, session_id)?;
    if prov.event_refs.is_empty() && prov.sync_run_ids.is_empty() {
        return Ok(0);
    }
    let mut st =
        conn.prepare("SELECT source_ref, sync_run_id, metadata FROM context_item_revisions")?;
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, Option<String>>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    let mut n = 0i64;
    for row in rows {
        let (source_ref, sync_run_id, metadata) = row?;
        let meta: serde_json::Value = serde_json::from_str(&metadata).unwrap_or_default();
        if revision_matches_provenance(source_ref.as_deref(), sync_run_id.as_deref(), &meta, &prov)
        {
            n += 1;
        }
    }
    Ok(n)
}

/// §27 / §29 step 2: rewrite every session-linked revision in place to
/// `source_type = 'deleted_session'`, `source_ref = NULL`, `sync_run_id =
/// NULL`, metadata scrubbed. Runs INSIDE the purge transaction, while the
/// event refs and sync run ids are still resolvable. The one in-place
/// `UPDATE` the append-only revisions table ever receives — mandated by the
/// deletion plan, scoped to provenance only (content and title survive).
pub fn redact_session_provenance_conn(tx: &Transaction, session_id: &str) -> Result<usize> {
    let prov = collect_session_provenance(tx, session_id)?;
    if prov.event_refs.is_empty() && prov.sync_run_ids.is_empty() {
        return Ok(0);
    }
    let mut st =
        tx.prepare("SELECT id, source_ref, sync_run_id, metadata FROM context_item_revisions")?;
    let mut matches: Vec<(String, String)> = Vec::new();
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (id, source_ref, sync_run_id, metadata) = row?;
        let meta: serde_json::Value = serde_json::from_str(&metadata).unwrap_or_default();
        if revision_matches_provenance(source_ref.as_deref(), sync_run_id.as_deref(), &meta, &prov)
        {
            matches.push((id, redacted_metadata(&meta)));
        }
    }
    drop(st);

    let mut n = 0usize;
    for (id, meta_json) in matches {
        n += tx.execute(
            "UPDATE context_item_revisions
             SET source_type = 'deleted_session', source_ref = NULL,
                 sync_run_id = NULL, metadata = ?2
             WHERE id = ?1",
            params![id, meta_json],
        )?;
    }
    Ok(n)
}

// ---------------- Permanent purge ----------------

/// §29: the whole NoEnding purge in ONE transaction, in the fixed order.
/// Callers have already verified the Adapter deleted (or never found) the
/// Agent source. Returns the number of redacted revisions. Never touches
/// Workstreams, WorkstreamPaths, WorkspacePaths, Projects, or surviving
/// Context content (§30) — WorkspacePath GC stays with the existing
/// reconciliation pass (§31).
pub fn purge_session_data_conn(tx: &Transaction, session_id: &str) -> Result<usize> {
    // 1+2. Redact Context provenance while the refs still resolve.
    let redacted = redact_session_provenance_conn(tx, session_id)?;

    // 3. Delivery bookkeeping for this session.
    tx.execute(
        "DELETE FROM context_deliveries WHERE session_id = ?1",
        params![session_id],
    )?;
    // 4. Sync history — after redaction matched revisions by run id.
    tx.execute(
        "DELETE FROM sync_runs WHERE session_id = ?1",
        params![session_id],
    )?;
    // 5. Launch history that matched this session.
    tx.execute(
        "DELETE FROM launch_intents WHERE matched_session_id = ?1",
        params![session_id],
    )?;
    // 6. Read cursors (FK → sessions).
    tx.execute(
        "DELETE FROM session_cursors WHERE session_id = ?1",
        params![session_id],
    )?;
    // 7. The append-only event store: THIS session's history ends here,
    //     after every surviving provenance pointer was redacted.
    tx.execute(
        "DELETE FROM session_events WHERE session_id = ?1",
        params![session_id],
    )?;
    // 8. The coordination row dies with the session — no tombstone (§8).
    tx.execute(
        "DELETE FROM session_deletion_jobs WHERE session_id = ?1",
        params![session_id],
    )?;
    // 9. The session row itself, last, once nothing references it.
    tx.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])?;
    // FTS rows of this session's events go with the data — a failure here
    // rolls the whole purge back, never a half-deleted session in search.
    unindex_session_conn(tx, session_id)?;

    Ok(redacted)
}
