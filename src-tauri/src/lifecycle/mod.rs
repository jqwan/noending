//! Session lifecycle orchestration: Trash, Restore and permanent LOCAL delete
//! (重构方案 §19 / §20).
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
//! Permanent delete is a NoEnding-LOCAL purge and nothing else. It is allowed
//! only for a TRASHED session whose ROOT source the adapter freshly confirmed
//! Missing — `Present` and `Unavailable` both refuse (§20.1). There is no
//! deletion job, no crash recovery and no filesystem step: the whole purge is
//! one SQLite transaction. If the source reappears afterwards, it is simply
//! re-ingested as a new Session (§20.4 — no tombstone).

use crate::adapters::adapter_for;
use crate::domain::SourceAvailability;
use crate::domain::{Agent, Session};
use crate::error::{other, Result};
use crate::storage::{session_lifecycle, Db, PermanentDeletionCounts};

/// Impact preview returned by `get_session_local_delete_preview` (§20.2):
/// the fresh ROOT source verdict plus the counts the confirmation dialog
/// shows. Everything is backend-computed and stateless — there is no job row
/// between preview and execute; the frontend submits the session id again.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LocalDeletePreview {
    pub session_id: String,
    pub session_title: Option<String>,
    pub agent: Agent,
    pub root_agent_session_id: String,
    /// Fresh verdict on the ROOT source at preview time. The confirmation
    /// copy keys off this; execute re-checks it again (§20.3).
    pub root_source_status: SourceAvailability,
    pub can_permanently_delete: bool,
    #[serde(flatten)]
    pub counts: PermanentDeletionCounts,
}

/// Outcome of `permanently_delete_session` (§20.3).
#[derive(Debug, Clone, serde::Serialize)]
pub struct PermanentDeleteResult {
    pub purged: bool,
    pub redacted_revisions: usize,
}

/// Normal → Trash (§19.1). Reversible; never touches the Agent source.
///
/// The FTS unindex commits INSIDE the same transaction as the lifecycle flip
/// (review P1-1): a crash can no longer leave a trashed session searchable —
/// there is no post-commit window and no reliance on eventual self-heal.
///
/// Trash freezes the session: member cursors are not touched, so ingestion
/// commits against it are refused by the commit-time guard and a Restore
/// resumes from exactly where things stopped.
pub fn trash_session(db: &Db, session_id: &str) -> Result<Session> {
    let changed = db.tx(|tx| {
        let changed =
            session_lifecycle::trash_session_conn(tx, session_id, &crate::storage::now())?;
        if changed {
            // §21: a trashed session leaves the search index, atomically
            // with the flip that hides it — an index failure rolls BOTH back.
            session_lifecycle::unindex_session_conn(tx, session_id)?;
        }
        Ok(changed)
    })?;
    if !changed {
        return Err(other("Session 不存在或已在回收站"));
    }
    db.get_session(session_id)?
        .ok_or_else(|| other("Session 不存在"))
}

/// Trash → Normal (§19.2): same Session id; Owner, members, messages,
/// cursors and the Context frontier were never touched. The next reconcile
/// simply resumes (and re-indexes the restored conversation).
pub fn restore_session(db: &Db, session_id: &str) -> Result<Session> {
    let restored = db.tx(|tx| {
        let restored = session_lifecycle::restore_session_conn(tx, session_id)?;
        if restored {
            session_lifecycle::reindex_session_conn(tx, session_id)?;
        }
        Ok(restored)
    })?;
    if !restored {
        return Err(other("Session 不存在或不在回收站"));
    }
    db.get_session(session_id)?
        .ok_or_else(|| other("Session 不存在"))
}

/// The fresh ROOT source verdict for a session (§20.1). `None` when the
/// session has no root member row (it never completed discovery).
pub fn root_source_status(db: &Db, session: &Session) -> Result<Option<SourceAvailability>> {
    let Some(root) = db.root_member_for_session(&session.id)? else {
        return Ok(None);
    };
    Ok(Some(
        adapter_for(session.agent).inspect_member_source(&root)?,
    ))
}

/// Stateless preview of the permanent LOCAL deletion (§20.2). Only a TRASHED
/// session may be previewed; the verdict on the ROOT source is taken fresh
/// right here, and the counts are read while the store is still complete.
pub fn get_session_local_delete_preview(db: &Db, session_id: &str) -> Result<LocalDeletePreview> {
    let session = db
        .get_session(session_id)?
        .ok_or_else(|| other("Session 不存在"))?;
    // §42-equivalent of the old rule: only Trash may head toward a permanent
    // deletion; an Active session is never purgeable, whatever its source.
    if !session.is_trashed() {
        return Err(other("只有回收站中的会话才能永久删除"));
    }
    let status = root_source_status(db, &session)?;
    let can_delete = status == Some(SourceAvailability::Missing);
    let counts = PermanentDeletionCounts::collect(&db.read(), session_id)?;
    Ok(LocalDeletePreview {
        session_id: session.id,
        session_title: session.title,
        agent: session.agent,
        root_agent_session_id: session.root_agent_session_id,
        root_source_status: status.unwrap_or(SourceAvailability::Unavailable),
        can_permanently_delete: can_delete,
        counts,
    })
}

/// Execute the permanent LOCAL deletion (§20.3). Guards, in order:
///
/// 1. the session exists;
/// 2. it is TRASHED;
/// 3. it still has a ROOT member;
/// 4. the adapter's FRESH verdict on the root source is `Missing` — taken
///    again right here, so a file that reappeared between preview and
///    execute aborts the purge;
///
/// then ONE SQLite transaction redacts provenance and deletes every
/// session-owned row (§20.3's fixed order). Child sources are irrelevant by
/// design: only the root is the deletion authority (§2.6).
pub fn permanently_delete_session(db: &Db, session_id: &str) -> Result<PermanentDeleteResult> {
    let session = db
        .get_session(session_id)?
        .ok_or_else(|| other("Session 不存在"))?;
    if !session.is_trashed() {
        return Err(other("只有回收站中的会话才能永久删除"));
    }
    let root = db
        .root_member_for_session(&session.id)?
        .ok_or_else(|| other("该会话尚未完成摄入（没有 root member），无法永久删除"))?;
    // Fresh re-check at execute time — the preview's verdict is advice, this
    // one is the gate. Only a confirmed-absent root may pass; Unavailable
    // (permission denied, parse error, store unreadable) never equals missing.
    let status = adapter_for(session.agent).inspect_member_source(&root)?;
    if status != SourceAvailability::Missing {
        return Err(other(format!(
            "Root 源当前状态为 {}，只有确认不存在（missing）才允许永久删除",
            status.as_str()
        )));
    }

    let redacted = db.tx(|tx| session_lifecycle::purge_session_data_conn(tx, &session.id))?;
    Ok(PermanentDeleteResult {
        purged: true,
        redacted_revisions: redacted,
    })
}
