//! Durable observations, separate from source mtime and conversation activity.
use crate::error::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize)]
pub struct SyncCheck {
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub outcome: Option<String>,
    /// Last successful observation, including checks that found no new data.
    pub checked_at: Option<String>,
}

pub fn check(conn: &Connection, scope: &str, id: &str) -> Result<SyncCheck> {
    Ok(conn.query_row("SELECT started_at,finished_at,outcome,checked_at FROM sync_checks WHERE scope=?1 AND target_id=?2", params![scope,id], |r| Ok(SyncCheck { started_at:r.get(0)?,finished_at:r.get(1)?,outcome:r.get(2)?,checked_at:r.get(3)? })).optional()?.unwrap_or_default())
}

pub fn record(
    conn: &Connection,
    scope: &str,
    id: &str,
    started: &str,
    outcome: &str,
) -> Result<()> {
    // A source/session can be deleted while IO is in progress. Never resurrect
    // its status or retain observations for an unregistered source.
    let exists: bool = match scope {
        "session" => conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1)",
            [id],
            |r| r.get(0),
        )?,
        "source" => conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM ingest_sources WHERE id=?1)",
            [id],
            |r| r.get(0),
        )?,
        _ => true,
    };
    if !exists {
        return Ok(());
    }
    let finished = super::now();
    conn.execute("INSERT INTO sync_checks(scope,target_id,started_at,finished_at,outcome,checked_at) VALUES (?1,?2,?3,?4,?5,CASE WHEN ?5='success' THEN ?3 END)
        ON CONFLICT(scope,target_id) DO UPDATE SET
          checked_at=CASE WHEN excluded.checked_at IS NOT NULL AND (sync_checks.checked_at IS NULL OR excluded.checked_at>sync_checks.checked_at) THEN excluded.checked_at ELSE sync_checks.checked_at END,
          started_at=CASE WHEN excluded.started_at>=sync_checks.started_at THEN excluded.started_at ELSE sync_checks.started_at END,
          finished_at=CASE WHEN excluded.started_at>=sync_checks.started_at THEN excluded.finished_at ELSE sync_checks.finished_at END,
          outcome=CASE WHEN excluded.started_at>=sync_checks.started_at THEN excluded.outcome ELSE sync_checks.outcome END",
        params![scope,id,started,finished,outcome])?;
    Ok(())
}

pub fn mark_refreshed(conn: &Connection, session: &str) -> Result<()> {
    conn.execute(
        "UPDATE session_contexts SET refreshed_at=?2 WHERE session_id=?1",
        params![session, super::now()],
    )?;
    Ok(())
}
