//! Session commands: listing, detail, and Owner Workstream assignment.
//!
//! These are thin: they take the lock, map the wire shape and hand over to
//! `workspace::session`, which owns the rules.
//!
//! A Session has at most ONE Owner Workstream. Setting it writes
//! `sessions.owner_workstream_id` and nothing else — it never touches the
//! Workstream's ordered path list, the Session's cwd or its Project.
//!
//! The detail shape is the Logical Session view: the
//! conversation (`session_messages` only — the UI never sees compact/system/
//! sidechain kinds), the execution graph as MEMBERS (not other Sessions),
//! query-time aggregate stats, the two frontiers, and the source/lifecycle
//! facts (fresh root source status, resume and permanent-delete eligibility).
//! There is no parent/children pair any more: members are execution info, not
//! navigable Sessions.

use serde::Serialize;
use tauri::State;

use crate::domain::*;
use crate::error::{other, Result};

use super::{with_db, AppState};

// ---------------- Sessions ----------------

/// One member of the execution graph, as the detail page shows it: the
/// member facts plus its own stats snapshot when one exists.
#[derive(Serialize)]
pub struct SessionMemberView {
    #[serde(flatten)]
    pub member: SessionMember,
    pub stats: Option<SessionMemberStats>,
}

#[derive(Serialize)]
pub struct SessionDetail {
    pub session: Session,
    /// The Conversation: root user/assistant messages only.
    pub messages: Vec<SessionMessage>,
    /// The one Workstream this Session belongs to, or `None`.
    pub owner_workstream: Option<Workstream>,
    /// Session Detail shows the WorkspacePath and the Project behind it,
    /// read-only. They travel as display strings because `workspace_path_id`
    /// alone would make the UI join a table it has no command for — and the
    /// Project shown here is derived through that path, never picked by a user.
    pub workspace_path: Option<SessionWorkspacePath>,
    /// The execution graph: root / children / sides. Execution info,
    /// never other user-visible Sessions.
    pub members: Vec<SessionMemberView>,
    /// Query-time aggregate over the whole graph (no cache to drift).
    pub stats: crate::storage::SessionAggregateStats,
    /// The two frontiers the detail page shows: messages ingested, and how
    /// far Context processing has consumed them.
    pub ingested_message_sequence: i64,
    pub processed_message_sequence: i64,
    /// Fresh (detail-load time) verdict on the ROOT source.
    /// `missing` means the adapter confirmed it absent; every other failure
    /// stays `unavailable`.
    pub root_source_status: SourceAvailability,
    /// Resume eligibility: active session + a present root source.
    pub can_resume: bool,
    /// Permanent-delete eligibility: trashed + fresh root `missing`.
    pub can_permanently_delete: bool,
    /// when this session is a fork, its source Session summary.
    pub forked_from: Option<Session>,
}

#[derive(Serialize)]
pub struct SessionWorkspacePath {
    pub id: String,
    pub canonical_path: String,
    pub exists: bool,
    pub project_id: String,
    pub project_name: String,
}

// scope: active (default) | trash | all —; the recycle bin passes "trash".
#[tauri::command]
pub fn list_sessions(
    state: State<AppState>,
    project_id: Option<String>,
    agent: Option<String>,
    scope: Option<String>,
) -> Result<Vec<Session>> {
    let agent = agent.and_then(|a| Agent::parse(&a));
    let scope = scope
        .as_deref()
        .map(SessionListScope::parse)
        .unwrap_or_default();
    with_db(&state, |db| {
        db.list_sessions(crate::storage::SessionFilter {
            project_id,
            agent,
            scope,
        })
    })
}

#[tauri::command]
pub fn get_session_detail(state: State<AppState>, session_id: String) -> Result<SessionDetail> {
    with_db(&state, |db| {
        let session = db
            .get_session(&session_id)?
            .ok_or_else(|| other("Session 不存在"))?;
        let messages = db.get_messages(&session_id, None, 500)?;
        let members = db.members_for_session(&session_id)?;
        let member_views = members
            .iter()
            .map(|m| {
                Ok(SessionMemberView {
                    member: m.clone(),
                    stats: db.get_member_stats(&m.id)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let stats = db.aggregate_session_stats(&session_id)?;
        // The two frontiers the detail page shows: messages ingested, and how
        // far Context processing has consumed them.
        let ingested_message_sequence = db.ingested_message_sequence(&session_id)?;
        let processed_message_sequence = db
            .get_session_context(&session_id)?
            .map(|c| c.processed_through_seq)
            .unwrap_or(0);
        let owner_workstream = match session.owner_workstream_id.as_deref() {
            Some(id) => db.get_workstream(id)?,
            None => None,
        };
        let root_source_status = crate::lifecycle::root_source_status(db, &session)?
            .unwrap_or(SourceAvailability::Unavailable);
        let can_resume = !session.is_trashed() && root_source_status == SourceAvailability::Present;
        let can_permanently_delete =
            session.is_trashed() && root_source_status == SourceAvailability::Missing;
        let workspace_path = match session.workspace_path_id.as_deref() {
            Some(id) => db.get_workspace_path(id)?.map(|wp| {
                Ok::<_, crate::error::AppError>(SessionWorkspacePath {
                    id: wp.id.clone(),
                    canonical_path: wp.canonical_path.clone(),
                    exists: wp.exists,
                    project_name: db
                        .get_project(&wp.project_id)?
                        .map(|p| p.name)
                        .unwrap_or_default(),
                    project_id: wp.project_id.clone(),
                })
            }),
            None => None,
        };
        // the fork provenance, resolved to a summary when the source
        // session still exists locally.
        let forked_from = match session.forked_from_session_id.as_deref() {
            Some(id) => db.get_session(id)?,
            None => None,
        };
        Ok(SessionDetail {
            session,
            messages,
            owner_workstream,
            workspace_path: workspace_path.transpose()?,
            members: member_views,
            stats,
            ingested_message_sequence,
            processed_message_sequence,
            root_source_status,
            can_resume,
            can_permanently_delete,
            forked_from,
        })
    })
}

/// Set (or clear) a Session's Owner Workstream.
///
/// `workstream_id = None` clears ownership. This is the ONLY write path for
/// semantic ownership; it changes one column and nothing else.
#[tauri::command]
pub fn set_session_owner_workstream(
    state: State<AppState>,
    session_id: String,
    workstream_id: Option<String>,
) -> Result<Session> {
    with_db(&state, |db| {
        crate::workspace::session::set_session_owner(db, &session_id, workstream_id.as_deref())
    })
}

// ---------------- Ingestion diagnostics ----------------

/// The Settings → Ingestion Diagnostics list: repeat offenders only by
/// default (`observation_count >= 2`). Diagnostics are NOT sessions — they
/// have no Owner, no Resume, no Trash, no Context.
#[tauri::command]
pub fn list_ingestion_diagnostics(
    state: State<AppState>,
    min_observations: Option<i64>,
) -> Result<Vec<IngestionDiagnostic>> {
    with_db(&state, |db| {
        db.list_ingestion_diagnostics(min_observations.unwrap_or(2))
    })
}
