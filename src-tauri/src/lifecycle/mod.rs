//! Session lifecycle orchestration: Trash, Restore and prepared permanent
//! deletion (Session Lifecycle & Deletion v0.1).
//!
//! Division of labor, mirroring the architecture boundaries:
//! - **this module** owns the rules — which transitions are legal, when a
//!   deletion job may advance, what the preview must contain;
//! - **storage::session_jobs** owns the SQL (jobs, purge, redaction);
//! - **the Agent adapter** owns source validation and the actual file
//!   removal — this module NEVER calls `remove_file` on a `raw_path` (§7):
//!   source deletion flows exclusively through
//!   `AgentAdapter::{prepare,execute}_source_session_deletion`.
//!
//! Order invariant (§6): the Agent source is deleted first, and only a
//! `Deleted` / `AlreadyAbsent` outcome unlocks the NoEnding purge — a source
//! deletion failure leaves the Session in Trash with all data intact.

use crate::adapters::{adapter_for, SourceDeletionPlan};
use crate::domain::{Agent, Session, SessionDeletionJob};
use crate::error::{other, AppError, Result};
use crate::storage::{session_jobs, Db, PermanentDeletionCounts};

/// Impact preview returned by `prepare_session_permanent_delete` (§20): the
/// frozen plan plus the counts the confirmation dialog shows. Everything is
/// backend-computed; the frontend submits back only the job id (§19).
#[derive(Debug, Clone, serde::Serialize)]
pub struct PermanentDeletionPreview {
    pub job_id: String,
    pub session_id: String,
    pub session_title: Option<String>,
    pub agent: Agent,
    pub agent_session_id: String,
    pub source_targets: Vec<crate::adapters::SourceDeletionTarget>,
    #[serde(flatten)]
    pub counts: PermanentDeletionCounts,
}

/// Outcome of `execute_session_permanent_delete` (§21/§22). On `purged` the
/// job and session no longer exist; otherwise the job carries the failure
/// state (`failed` / `stale`) and the Session stays safely in Trash.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PermanentDeletionResult {
    pub purged: bool,
    pub redacted_revisions: usize,
    pub job: Option<SessionDeletionJob>,
    pub error: Option<String>,
}

/// Normal → Trash (§6). Reversible; never touches the Agent source file.
pub fn trash_session(db: &Db, session_id: &str) -> Result<Session> {
    let changed =
        db.tx(|tx| session_jobs::trash_session_conn(tx, session_id, &crate::storage::now()))?;
    if !changed {
        return Err(other("Session 不存在或已在回收站"));
    }
    // §12: a trashed session leaves the search index; restore puts it back.
    session_jobs::unindex_session_conn(db.conn(), session_id);
    db.get_session(session_id)?
        .ok_or_else(|| other("Session 不存在"))
}

/// Trash → Normal (§7): same Session id; bindings, events and cursors were
/// never touched. Refused while a deletion job exists — the user cancels the
/// deletion first (§42).
pub fn restore_session(db: &Db, session_id: &str) -> Result<Session> {
    db.tx(|tx| session_jobs::restore_session_conn(tx, session_id))?;
    session_jobs::reindex_session_conn(db.conn(), session_id);
    db.get_session(session_id)?
        .ok_or_else(|| other("Session 不存在"))
}

/// Freeze a permanent deletion plan for a trashed Session (§19–§20).
///
/// The adapter validates the source and returns the plan; the preview counts
/// are read while the store is still complete; the job row lands in its own
/// transaction. Re-prepare over a `failed` / `stale` job replaces it; a
/// `deleting_source` job is never clobbered (§42).
pub fn prepare_session_permanent_delete(
    db: &Db,
    session_id: &str,
) -> Result<PermanentDeletionPreview> {
    let session = db
        .get_session(session_id)?
        .ok_or_else(|| other("Session 不存在"))?;
    // §42: only Trash may prepare a permanent deletion.
    if !session.is_trashed() {
        return Err(other("只有回收站中的会话才能永久删除"));
    }
    if let Some(job) = session_jobs::get_job_for_session_conn(db.conn(), session_id)? {
        if job.state == SessionDeletionJob::STATE_DELETING_SOURCE {
            return Err(other("永久删除正在执行中，请等待完成或重启应用后重试"));
        }
        // prepared / failed / stale → replaced below (re-prepare).
    }

    // Adapter-owned validation. Errors here (unsupported / unsafe / wrong
    // file) leave NO job behind: the session simply stays in Trash (§38).
    let plan = adapter_for(session.agent).prepare_source_session_deletion(&session)?;

    let counts = PermanentDeletionCounts::collect(db.conn(), session_id)?;

    let job = SessionDeletionJob {
        id: crate::storage::new_id(),
        session_id: session.id.clone(),
        state: SessionDeletionJob::STATE_PREPARED.to_string(),
        plan_json: serde_json::to_string(&plan)?,
        last_error: None,
        created_at: crate::storage::now(),
        updated_at: crate::storage::now(),
    };
    db.tx(|tx| session_jobs::insert_deletion_job_conn(tx, &job))?;

    Ok(PermanentDeletionPreview {
        job_id: job.id,
        session_id: session.id,
        session_title: session.title,
        agent: session.agent,
        agent_session_id: session.agent_session_id,
        source_targets: plan.targets,
        counts,
    })
}

/// Execute a prepared permanent deletion (§21–§24).
///
/// Never returns `Err` for an operational failure: a stale or failed source
/// deletion is a *state* the UI reacts to (retry / cancel), carried back in
/// the result with the updated job. `Err` is reserved for broken transitions
/// (unknown job, wrong state).
pub fn execute_session_permanent_delete(db: &Db, job_id: &str) -> Result<PermanentDeletionResult> {
    let mut job = session_jobs::get_deletion_job_conn(db.conn(), job_id)?
        .ok_or_else(|| other("永久删除任务不存在"))?;
    match job.state.as_str() {
        SessionDeletionJob::STATE_PREPARED | SessionDeletionJob::STATE_FAILED => {}
        SessionDeletionJob::STATE_STALE => {
            return Err(other("源会话已发生变化，请重新准备后再执行"));
        }
        _ => return Err(other("永久删除正在执行中，请等待完成或重启应用后重试")),
    }
    let session = match db.get_session(&job.session_id)? {
        Some(s) => s,
        None => {
            // Purge and job row die in one transaction, so a job without its
            // session means a foreign row — clean it up.
            session_jobs::delete_deletion_job_conn(db.conn(), &job.id)?;
            return Err(other("Session 不存在"));
        }
    };

    // Mark in-flight BEFORE touching the filesystem, so a crash during the
    // delete is diagnosable (§23 converts it to `failed` on next startup).
    session_jobs::set_job_state_conn(
        db.conn(),
        &job.id,
        SessionDeletionJob::STATE_DELETING_SOURCE,
        None,
    )?;
    job.state = SessionDeletionJob::STATE_DELETING_SOURCE.to_string();

    let plan: SourceDeletionPlan = serde_json::from_str(&job.plan_json)?;

    // Adapter-owned, revalidated deletion (§21). Core never removes the file.
    if let Err(e) = adapter_for(session.agent).execute_source_session_deletion(&plan) {
        // §22: the Session stays in Trash, NoEnding data fully preserved.
        let state = match &e {
            AppError::SourceDeletionStale(_) => SessionDeletionJob::STATE_STALE,
            _ => SessionDeletionJob::STATE_FAILED,
        };
        let msg = e.to_string();
        session_jobs::set_job_state_conn(db.conn(), &job.id, state, Some(&msg))?;
        job.state = state.to_string();
        job.last_error = Some(msg.clone());
        return Ok(PermanentDeletionResult {
            purged: false,
            redacted_revisions: 0,
            job: Some(job),
            error: Some(msg),
        });
    }

    // Source deleted (or already absent — the confirmed bytes are gone, which
    // is the goal). One transaction purges NoEnding's copy of the session,
    // including this job row. No tombstone survives (§5).
    let redacted = db.tx(|tx| session_jobs::purge_session_data_conn(tx, &session.id))?;

    Ok(PermanentDeletionResult {
        purged: true,
        redacted_revisions: redacted,
        job: None,
        error: None,
    })
}

/// Cancel a prepared permanent deletion (§42): drop the coordination row,
/// the Session stays in Trash. Refused only while the deletion is actually
/// in flight (after a crash, startup recovery has already flipped the state).
pub fn cancel_session_permanent_delete(db: &Db, job_id: &str) -> Result<()> {
    let job = session_jobs::get_deletion_job_conn(db.conn(), job_id)?
        .ok_or_else(|| other("永久删除任务不存在"))?;
    if job.state == SessionDeletionJob::STATE_DELETING_SOURCE {
        return Err(other("永久删除正在执行中，无法取消"));
    }
    session_jobs::delete_deletion_job_conn(db.conn(), job_id)
}

/// Current coordination row for a session, if any (UI state source).
pub fn get_session_deletion_job(db: &Db, session_id: &str) -> Result<Option<SessionDeletionJob>> {
    session_jobs::get_job_for_session_conn(db.conn(), session_id)
}

/// §23 startup recovery: `deleting_source` jobs from a previous process are
/// flipped to `failed` with a clear message — deletion is never silently
/// continued, the user decides (retry → AlreadyAbsent → purge completes).
pub fn recover_interrupted_deletions(db: &Db) -> Result<usize> {
    session_jobs::recover_interrupted_deletion_jobs(db.conn())
}
