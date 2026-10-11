//! Session lifecycle storage: the SQL behind Archive / Restore, permanent local
//! purge, and the search cleanup used only by permanent deletion.
//!
//! There is no deletion job, no crash recovery and no filesystem step: NoEnding
//! never deletes an Agent-owned source, so a purge is one SQLite transaction.
//!
//! Free functions over `&Connection` / `&Transaction`, matching the `_conn`
//! convention, so `Db` methods and `Db::tx` closures share the same paths.

use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::error::Result;

// Archive / Restore

/// The single lifecycle transition Normal → Archive. The `archived_at IS NULL`
/// guard makes archive idempotent and makes an archive racing an unarchive commit
/// exactly one transition. `Ok(false)` = session missing or already archived.
pub fn archive_session_conn(conn: &Connection, session_id: &str, ts: &str) -> Result<bool> {
    let n = conn.execute(
        "UPDATE sessions SET archived_at = ?2 WHERE id = ?1 AND archived_at IS NULL",
        params![session_id, ts],
    )?;
    Ok(n > 0)
}

/// Archive → Normal: same Session id; tracking, members, messages, cursors and the
/// Context frontier were never touched, so the next reconcile simply resumes.
pub fn restore_session_conn(conn: &Connection, session_id: &str) -> Result<bool> {
    let n = conn.execute(
        "UPDATE sessions SET archived_at = NULL WHERE id = ?1 AND archived_at IS NOT NULL",
        params![session_id],
    )?;
    Ok(n > 0)
}

/// Commit-time guard: the session must still exist, or the rows a caller
/// prepared have nowhere to land. Archive is NOT part of it (see
/// `sessions.archived_at`); only a permanent purge removes the row.
pub fn session_exists_conn(conn: &Connection, session_id: &str) -> Result<bool> {
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sessions WHERE id = ?1",
            params![session_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(exists.is_some())
}

// FTS

/// Drop a permanently deleted session's message rows from the FTS index. Message
/// rows carry `parent_id = session_id`, so one delete covers the session.
///
/// Errors propagate so permanent deletion and search cleanup commit atomically.
pub fn unindex_session_conn(conn: &Connection, session_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM search_index WHERE kind = 'message' AND parent_id = ?1",
        params![session_id],
    )?;
    // Permanent deletion removes the Session document too.
    conn.execute(
        "DELETE FROM search_index WHERE kind = 'session' AND ref_id = ?1",
        params![session_id],
    )?;
    Ok(())
}

// Preview counts

/// What a permanent LOCAL deletion will remove, for the confirmation preview.
/// Workstreams / WorkspacePaths / Projects and surviving
/// Context content are deliberately absent — they are KEPT.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PermanentDeletionCounts {
    pub message_count: i64,
    /// Current Session Context row.
    pub session_context_count: i64,
}

impl PermanentDeletionCounts {
    pub fn collect(conn: &Connection, session_id: &str) -> Result<Self> {
        let count = |sql: &str| -> Result<i64> {
            Ok(conn.query_row(sql, params![session_id], |r| r.get(0))?)
        };
        Ok(Self {
            message_count: count("SELECT COUNT(*) FROM session_messages WHERE session_id = ?1")?,
            session_context_count: count(
                "SELECT COUNT(*) FROM session_contexts WHERE session_id = ?1",
            )?,
        })
    }
}

// Permanent local purge

/// The whole NoEnding LOCAL purge in ONE transaction, in the fixed order.
/// Callers have already required Archive. Other sessions, physical paths,
/// task summaries and Agent-owned sources survive; all tracked tasks become pending.
pub fn purge_session_data_conn(tx: &Transaction, session_id: &str) -> Result<()> {
    let mut st = tx.prepare("SELECT workstream_id FROM session_workstreams WHERE session_id=?1")?;
    let tasks = st
        .query_map([session_id], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(st);
    for tid in tasks {
        crate::storage::bump_input_revision_conn(tx, &tid)?;
    }

    // 4. The Session's current summary and its place in
    //    every Workstream's consumption frontier.
    tx.execute(
        "DELETE FROM session_contexts WHERE session_id = ?1",
        params![session_id],
    )?;
    tx.execute(
        "DELETE FROM workstream_session_frontiers WHERE session_id = ?1",
        params![session_id],
    )?;
    // 5. The current-message projection goes with the conversation.
    tx.execute(
        "DELETE FROM session_message_projection WHERE session_id = ?1",
        params![session_id],
    )?;
    // The conversation store goes with the session.
    tx.execute(
        "DELETE FROM session_messages WHERE session_id = ?1",
        params![session_id],
    )?;
    // 7. The session row itself, last, once nothing references it.
    tx.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])?;
    // FTS rows of this session's messages go with the data — a failure here
    // rolls the whole purge back, never a half-deleted session in search.
    unindex_session_conn(tx, session_id)?;

    Ok(())
}
