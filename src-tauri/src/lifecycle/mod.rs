//! Session lifecycle orchestration: Archive, Restore and permanent LOCAL delete
//!.
//!
//! Division of labor, mirroring the architecture boundaries:
//! - **this module** owns the rules — which transitions are legal, when a
//!   local purge is allowed, what the preview must contain;
//! - **storage::session_lifecycle** owns the SQL (guarded flips, redaction,
//!   purge);
//! - **the Agent adapter** owns the source verdict — this module NEVER
//!   touches the Agent's files, because there is nothing to touch:
//!   ```text
//!   NoEnding never deletes Agent-owned session sources.
//!   ```
//!
//! Permanent delete is a NoEnding-LOCAL purge and nothing else: Archive is the
//! only gate, one SQLite transaction, no Agent file ever touched. The root
//! source's verdict is COPY, not a gate — NoEnding never deletes an Agent
//! source, so a source that is still there just gets re-ingested as a new
//! Session (no tombstone).

use crate::adapters::adapter_for;
use crate::domain::SourceAvailability;
use crate::domain::{Agent, Session};
use crate::error::{other, Result};
use crate::storage::{session_lifecycle, Db, PermanentDeletionCounts};

/// Impact preview returned by `get_session_local_delete_preview`:
/// the fresh ROOT source verdict plus the counts the confirmation dialog
/// shows. Everything is backend-computed and stateless — there is no job row
/// between preview and execute; the frontend submits the session id again.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LocalDeletePreview {
    pub session_id: String,
    pub session_title: Option<String>,
    pub agent: Agent,
    pub root_agent_session_id: String,
    /// Fresh verdict on the ROOT source at preview time. COPY, not a gate: a
    /// `present` source will be re-ingested as a new Session.
    pub root_source_status: SourceAvailability,
    #[serde(flatten)]
    pub counts: PermanentDeletionCounts,
}

/// Outcome of `permanently_delete_session`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PermanentDeleteResult {
    pub purged: bool,
    pub redacted_revisions: usize,
}

/// Archive without changing messages, Context, search or the Agent source.
pub fn archive_session(db: &Db, session_id: &str) -> Result<Session> {
    let changed = db
        .tx(|tx| session_lifecycle::archive_session_conn(tx, session_id, &crate::storage::now()))?;
    if !changed {
        return Err(other("会话不存在或已经归档"));
    }
    db.get_session(session_id)?
        .ok_or_else(|| other("Session 不存在"))
}

/// Archive → Normal: same Session id; Owner, members, messages,
/// cursors and the Context frontier were never touched. The next reconcile
/// simply continues. Search and Context were never disabled.
pub fn restore_session(db: &Db, session_id: &str) -> Result<Session> {
    let restored = db.tx(|tx| session_lifecycle::restore_session_conn(tx, session_id))?;
    if !restored {
        return Err(other("会话不存在或尚未归档"));
    }
    db.get_session(session_id)?
        .ok_or_else(|| other("Session 不存在"))
}

/// The fresh source verdict for a session.
pub fn root_source_status(_db: &Db, session: &Session) -> Result<Option<SourceAvailability>> {
    Ok(Some(
        adapter_for(session.agent).inspect_session_source(session)?,
    ))
}

/// The only gate on a local purge: the Session must exist and be archived.
fn require_archived(db: &Db, session_id: &str) -> Result<Session> {
    let session = db
        .get_session(session_id)?
        .ok_or_else(|| other("Session 不存在"))?;
    if !session.is_archived() {
        return Err(other("只有已归档的会话才能永久删除"));
    }
    Ok(session)
}

/// Stateless preview of the local deletion: what would go, plus the fresh
/// ROOT source verdict the dialog's copy is built from.
pub fn get_session_local_delete_preview(db: &Db, session_id: &str) -> Result<LocalDeletePreview> {
    let session = require_archived(db, session_id)?;
    let status = root_source_status(db, &session)?;
    let counts = PermanentDeletionCounts::collect(&db.read(), session_id)?;
    Ok(LocalDeletePreview {
        session_id: session.id,
        session_title: session.title,
        agent: session.agent,
        root_agent_session_id: session.root_agent_session_id,
        root_source_status: status.unwrap_or(SourceAvailability::Unavailable),
        counts,
    })
}

/// Execute the permanent LOCAL deletion: ONE SQLite transaction that redacts
/// provenance and deletes every session-owned row ('s fixed order).
pub fn permanently_delete_session(db: &Db, session_id: &str) -> Result<PermanentDeleteResult> {
    let session = require_archived(db, session_id)?;
    let redacted = db.tx(|tx| session_lifecycle::purge_session_data_conn(tx, &session.id))?;
    Ok(PermanentDeleteResult {
        purged: true,
        redacted_revisions: redacted,
    })
}
