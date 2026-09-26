//! Session lifecycle storage: the SQL behind Trash / Restore, permanent local
//! purge, and the FTS unindex / reindex both of them use.
//!
//! There is no deletion job, no crash recovery and no filesystem step: NoEnding
//! never deletes an Agent-owned source, so a purge is one SQLite transaction.
//!
//! Free functions over `&Connection` / `&Transaction`, matching the `_conn`
//! convention, so `Db` methods and `Db::tx` closures share the same paths.

use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::error::Result;

// Trash / Restore

/// The single lifecycle transition Normal → Trash. The `trashed_at IS NULL`
/// guard makes trash idempotent and makes a trash racing a restore commit
/// exactly one transition. `Ok(false)` = session missing or already trashed.
pub fn trash_session_conn(conn: &Connection, session_id: &str, ts: &str) -> Result<bool> {
    let n = conn.execute(
        "UPDATE sessions SET trashed_at = ?2 WHERE id = ?1 AND trashed_at IS NULL",
        params![session_id, ts],
    )?;
    if n > 0 {
        // Trashing removes a Session from its Workstream's live inputs.
        bump_owner_input_revision_conn(conn, session_id)?;
    }
    Ok(n > 0)
}

/// Trash → Normal: same Session id; Owner, members, messages, cursors and the
/// Context frontier were never touched, so the next reconcile simply resumes.
pub fn restore_session_conn(conn: &Connection, session_id: &str) -> Result<bool> {
    let n = conn.execute(
        "UPDATE sessions SET trashed_at = NULL WHERE id = ?1 AND trashed_at IS NOT NULL",
        params![session_id],
    )?;
    if n > 0 {
        bump_owner_input_revision_conn(conn, session_id)?;
    }
    Ok(n > 0)
}

/// Trash / Restore is an input change for the Session's Owner Workstream.
fn bump_owner_input_revision_conn(conn: &Connection, session_id: &str) -> Result<()> {
    let owner: Option<String> = conn
        .query_row(
            "SELECT COALESCE(owner_workstream_id, '') FROM sessions WHERE id = ?1",
            params![session_id],
            |r| r.get(0),
        )
        .optional()?
        .filter(|s: &String| !s.is_empty());
    if let Some(ws) = owner {
        crate::storage::bump_input_revision_conn(conn, &ws)?;
    }
    Ok(())
}

/// Commit-time guard for every Session write path: the session must exist AND
/// be Normal, otherwise work prepared against it (messages, stats, cursors,
/// context mutations) must not be committed.
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

// FTS

/// Drop a trashed / deleted session's message rows from the FTS index. Message
/// rows carry `parent_id = session_id`, so one delete covers the session.
///
/// Errors PROPAGATE: inside the lifecycle transaction a failed index write rolls
/// the lifecycle flip back, so "trashed" and "unindexed" commit atomically.
pub fn unindex_session_conn(conn: &Connection, session_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM search_index WHERE kind = 'message' AND parent_id = ?1",
        params![session_id],
    )?;
    // The Session's own document goes with it (Trash and permanent
    // deletion both route through here; a Restore rebuilds it).
    conn.execute(
        "DELETE FROM search_index WHERE kind = 'session' AND ref_id = ?1",
        params![session_id],
    )?;
    Ok(())
}

/// Rebuild the FTS rows of a restored session from the durable message store,
/// mirroring `backfill_search_index`'s row shape. Errors propagate like
/// [`unindex_session_conn`].
pub fn reindex_session_conn(conn: &Connection, session_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM search_index WHERE kind = 'message' AND parent_id = ?1",
        params![session_id],
    )?;
    crate::storage::index_session_conn(conn, session_id)?;
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

// Preview counts

/// What a permanent LOCAL deletion will remove, for the confirmation preview.
/// Workstreams / WorkstreamPaths / WorkspacePaths / Projects and surviving
/// Context content are deliberately absent — they are KEPT.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PermanentDeletionCounts {
    pub message_count: i64,
    pub member_count: i64,
    /// Committed Session Context rows and their revision history.
    pub session_context_count: i64,
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
            message_count: count("SELECT COUNT(*) FROM session_messages WHERE session_id = ?1")?,
            member_count: count("SELECT COUNT(*) FROM session_members WHERE session_id = ?1")?,
            session_context_count: count(
                "SELECT (SELECT COUNT(*) FROM session_contexts WHERE session_id = ?1)
                      + (SELECT COUNT(*) FROM session_context_revisions WHERE session_id = ?1)",
            )?,
            launch_intent_count: count(
                "SELECT COUNT(*) FROM launch_intents WHERE matched_session_id = ?1",
            )?,
            context_revision_redaction_count: count_redactable_revisions_conn(conn, session_id)?,
        })
    }
}

// Provenance redaction

/// Session-linked provenance keys, collected while the summaries and the
/// message store are still readable. A revision can cite either a raw message
/// (`session-message:<id>`) or a specific Session Context revision
/// (`session-context:<session id>:<revision>`).
struct SessionProvenance {
    refs: Vec<String>,
}

fn collect_session_provenance(conn: &Connection, session_id: &str) -> Result<SessionProvenance> {
    let mut refs = Vec::new();
    {
        let mut st = conn.prepare("SELECT id FROM session_messages WHERE session_id = ?1")?;
        let rows = st.query_map(params![session_id], |r| r.get::<_, String>(0))?;
        for id in rows {
            refs.push(format!("session-message:{}", id?));
        }
    }
    {
        let mut st =
            conn.prepare("SELECT revision FROM session_context_revisions WHERE session_id = ?1")?;
        let rows = st.query_map(params![session_id], |r| r.get::<_, i64>(0))?;
        for revision in rows {
            refs.push(format!("session-context:{}:{}", session_id, revision?));
        }
    }
    Ok(SessionProvenance { refs })
}

/// Every string inside a revision's metadata that claims to reference a
/// session message: `provenance.source_ref`, top-level `source_refs` and
/// `audit.source_refs` (the three shapes writers actually produce).
fn metadata_message_refs(metadata: &serde_json::Value) -> Vec<String> {
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

/// A revision belongs to the dying session when its `source_ref` resolves to
/// one of its messages / summary revisions, or its metadata mentions one of
/// those refs.
fn revision_matches_provenance(
    source_ref: Option<&str>,
    metadata: &serde_json::Value,
    prov: &SessionProvenance,
) -> bool {
    if let Some(r) = source_ref {
        if prov.refs.iter().any(|e| e == r) {
            return true;
        }
    }
    metadata_message_refs(metadata)
        .iter()
        .any(|m| prov.refs.iter().any(|e| e == m))
}

/// The scrubbed metadata: session-derived references removed, authority /
/// actor / status audit preserved (authority resolution reads
/// `provenance.authority` and `audit.actor` — dropping those would degrade
/// every redacted revision to `unknown`).
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

/// How many revisions would be redacted for this session (preview).
pub fn count_redactable_revisions_conn(conn: &Connection, session_id: &str) -> Result<i64> {
    let prov = collect_session_provenance(conn, session_id)?;
    if prov.refs.is_empty() {
        return Ok(0);
    }
    let mut st = conn.prepare("SELECT source_ref, metadata FROM context_item_revisions")?;
    let rows = st.query_map([], |r| {
        Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut n = 0i64;
    for row in rows {
        let (source_ref, metadata) = row?;
        let meta: serde_json::Value = serde_json::from_str(&metadata).unwrap_or_default();
        if revision_matches_provenance(source_ref.as_deref(), &meta, &prov) {
            n += 1;
        }
    }
    Ok(n)
}

/// The redaction half of the permanent LOCAL purge: rewrite every
/// session-linked revision in place to `source_type = 'deleted_session'`,
/// `source_ref = NULL`, metadata scrubbed. Runs INSIDE the purge transaction,
/// while the message and summary refs are still resolvable. The one in-place
/// `UPDATE` the append-only revisions table ever receives — scoped to
/// provenance only (content and title survive).
pub fn redact_session_provenance_conn(tx: &Transaction, session_id: &str) -> Result<usize> {
    let prov = collect_session_provenance(tx, session_id)?;
    if prov.refs.is_empty() {
        return Ok(0);
    }
    let mut st = tx.prepare("SELECT id, source_ref, metadata FROM context_item_revisions")?;
    let mut matches: Vec<(String, String)> = Vec::new();
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (id, source_ref, metadata) = row?;
        let meta: serde_json::Value = serde_json::from_str(&metadata).unwrap_or_default();
        if revision_matches_provenance(source_ref.as_deref(), &meta, &prov) {
            matches.push((id, redacted_metadata(&meta)));
        }
    }
    drop(st);

    let mut n = 0usize;
    for (id, meta_json) in matches {
        n += tx.execute(
            "UPDATE context_item_revisions
             SET source_type = 'deleted_session', source_ref = NULL, metadata = ?2
             WHERE id = ?1",
            params![id, meta_json],
        )?;
    }
    Ok(n)
}

// Permanent local purge

/// The whole NoEnding LOCAL purge in ONE transaction, in the fixed order.
/// Callers have already required Trash + a fresh `Missing` verdict on the ROOT
/// source. Returns the number of redacted revisions. Never touches Workstreams,
/// WorkstreamPaths, WorkspacePaths, Projects, surviving Context content, or
/// anything on the Agent's side — a reappearing source is simply re-ingested as
/// a new Session.
pub fn purge_session_data_conn(tx: &Transaction, session_id: &str) -> Result<usize> {
    // 1+2. Redact Context provenance while the refs still resolve.
    let redacted = redact_session_provenance_conn(tx, session_id)?;

    // 3. Launch history that matched this session.
    tx.execute(
        "DELETE FROM launch_intents WHERE matched_session_id = ?1",
        params![session_id],
    )?;
    // 4. The Session's summaries, their revision history, and its place in
    //    every Workstream's consumption frontier.
    tx.execute(
        "DELETE FROM session_contexts WHERE session_id = ?1",
        params![session_id],
    )?;
    tx.execute(
        "DELETE FROM session_context_revisions WHERE session_id = ?1",
        params![session_id],
    )?;
    tx.execute(
        "DELETE FROM workstream_session_frontiers WHERE session_id = ?1",
        params![session_id],
    )?;
    // 5. Diagnostics that describe exactly this session's members.
    tx.execute(
        "DELETE FROM ingestion_diagnostics
         WHERE agent = (SELECT agent FROM sessions WHERE id = ?1)
           AND source_member_id IN (SELECT source_member_id FROM session_members WHERE session_id = ?1)",
        params![session_id],
    )?;
    // 6. The current-message projection goes with the conversation.
    tx.execute(
        "DELETE FROM session_message_projection WHERE session_id = ?1",
        params![session_id],
    )?;
    tx.execute(
        "DELETE FROM session_ingest_state WHERE session_id = ?1",
        params![session_id],
    )?;
    // 7. The conversation store: THIS session's history ends here, after every
    //    surviving provenance pointer was redacted.
    tx.execute(
        "DELETE FROM session_messages WHERE session_id = ?1",
        params![session_id],
    )?;
    // 8. Member stats and cursors (FK → members).
    tx.execute(
        "DELETE FROM session_member_stats WHERE member_id IN
           (SELECT id FROM session_members WHERE session_id = ?1)",
        params![session_id],
    )?;
    tx.execute(
        "DELETE FROM session_member_cursors WHERE member_id IN
           (SELECT id FROM session_members WHERE session_id = ?1)",
        params![session_id],
    )?;
    // 9. The members themselves.
    tx.execute(
        "DELETE FROM session_members WHERE session_id = ?1",
        params![session_id],
    )?;
    // 10. The session row itself, last, once nothing references it.
    tx.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])?;
    // FTS rows of this session's messages go with the data — a failure here
    // rolls the whole purge back, never a half-deleted session in search.
    unindex_session_conn(tx, session_id)?;

    Ok(redacted)
}
