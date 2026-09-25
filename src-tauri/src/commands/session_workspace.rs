//! Session commands: listing, detail, and Owner Workstream assignment.
//!
//! These are thin: they take the lock, map the wire shape and hand over to
//! `workspace::session`, which owns the rules.
//!
//! A Session has at most ONE Owner Workstream (方案 §3.3). Setting it writes
//! `sessions.owner_workstream_id` and nothing else — it never touches the
//! Workstream's ordered path list, the Session's cwd or its Project (§5.1).
//!
use serde::Serialize;
use tauri::State;

use crate::domain::*;
use crate::error::{other, Result};

use super::{with_db, AppState};

// ---------------- Sessions ----------------

#[derive(Serialize)]
pub struct SessionDetail {
    pub session: Session,
    pub events: Vec<SessionEvent>,
    /// The one Workstream this Session belongs to, or `None` (方案 §24).
    pub owner_workstream: Option<Workstream>,
    pub cursor: i64,
    pub processed_cursor: i64,
    /// Read-only observation of the Agent source file at detail-load time.
    /// `missing` means NotFound; other filesystem errors stay `unavailable`.
    pub raw_path_status: &'static str,
    /// §22: Session Detail shows the WorkspacePath and the Project behind it,
    /// read-only. They travel as display strings because `workspace_path_id`
    /// alone would make the UI join a table it has no command for — and the
    /// Project shown here is derived through that path, never picked by a user.
    pub workspace_path: Option<SessionWorkspacePath>,
    /// §37.20 — the other sessions this execution belongs to, as whole rows so
    /// the UI needs no second lookup. The parent is resolved in the source's own
    /// id space (`session.parent_agent_session_id` is an Agent-side id) and is
    /// simply `None` when that thread was never discovered — the field on
    /// `session` still says a parent exists, and the UI says so honestly.
    pub parent: Option<Session>,
    pub children: Vec<Session>,
}

#[derive(Serialize)]
pub struct SessionWorkspacePath {
    pub id: String,
    pub canonical_path: String,
    pub exists: bool,
    pub project_id: String,
    pub project_name: String,
}

fn raw_path_status(path: &str) -> &'static str {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => "present",
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "missing",
        _ => "unavailable",
    }
}

// scope: active (default) | trash | all — 方案 §11; the recycle bin passes "trash".
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
        let mut events = db.get_events(&session_id, None, 500)?;
        db.resolve_event_counterparts(&session, &mut events)?;
        let cursor = db.get_cursor(&session_id)?;
        let processed_cursor = db.get_processed_sequence(&session_id)?;
        let owner_workstream = match session.owner_workstream_id.as_deref() {
            Some(id) => db.get_workstream(id)?,
            None => None,
        };
        let raw_path_status = raw_path_status(&session.raw_path);
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
        // §37.20 — the session tree, read in the source's own id space. A
        // child's `parent_agent_session_id` is an Agent-side id, so the lookup
        // is scoped to the same agent (a Codex thread id means nothing to dsh).
        let parent = match session.parent_agent_session_id.as_deref() {
            Some(id) => db.find_session_by_agent_id(session.agent, id)?,
            None => None,
        };
        let children = db.child_sessions(session.agent, &session.agent_session_id)?;
        Ok(SessionDetail {
            session,
            events,
            owner_workstream,
            cursor,
            processed_cursor,
            raw_path_status,
            workspace_path: workspace_path.transpose()?,
            parent,
            children,
        })
    })
}

/// Set (or clear) a Session's Owner Workstream (方案 §23).
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
