//! Session commands: listing, detail, and Workstream binding.
//!
//! These are thin: they take the lock, map the
//! wire shape and hand over to `workspace::session`, which owns the rules.
//!
//! Binding a Session now also decides the Workstream's ordered path list — the
//! Session's own WorkspacePath is reused if it is already there, appended if a
//! user action says it should be, and recorded on the binding by exact equality
//! (§1.8, §42.3-M1). Unbinding removes the binding only (§1.9). Which
//! WorkstreamPath brought a Session in is a derived fact, so no command here
//! accepts it from the UI.
//!
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::domain::*;
use crate::error::{other, Result};

use super::{with_db, AppState};

// ---------------- Sessions ----------------

#[derive(Serialize)]
pub struct SessionDetail {
    pub session: Session,
    pub events: Vec<SessionEvent>,
    pub bindings: Vec<(SessionWorkstreamBinding, Option<WorkstreamTitle>)>,
    pub cursor: i64,
    pub processed_cursor: i64,
    pub classification: String,
    /// Read-only observation of the Agent source file at detail-load time.
    /// `missing` means NotFound; other filesystem errors stay `unavailable`.
    pub raw_path_status: &'static str,
    /// §22: Session Detail shows the WorkspacePath and the Project behind it,
    /// read-only. They travel as display strings because `workspace_path_id`
    /// alone would make the UI join a table it has no command for — and the
    /// Project shown here is derived through that path, never picked by a user.
    pub workspace_path: Option<SessionWorkspacePath>,
}

#[derive(Serialize)]
pub struct SessionWorkspacePath {
    pub id: String,
    pub canonical_path: String,
    pub exists: bool,
    pub project_id: String,
    pub project_name: String,
}

pub type WorkstreamTitle = String;

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
        let events = db.get_events(&session_id, None, 500)?;
        let cursor = db.get_cursor(&session_id)?;
        let processed_cursor = db.get_processed_sequence(&session_id)?;
        let bindings = db
            .bindings_for_session(&session_id)?
            .into_iter()
            .map(|b| {
                let title = db.get_workstream(&b.workstream_id)?.map(|w| w.title);
                Ok((b, title))
            })
            .collect::<Result<Vec<_>>>()?;
        let classification = SessionClassificationState::derive(&{
            bindings.iter().map(|(b, _)| b.clone()).collect::<Vec<_>>()
        });
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
        Ok(SessionDetail {
            session,
            events,
            bindings,
            cursor,
            processed_cursor,
            classification: classification.as_str().to_string(),
            raw_path_status,
            workspace_path: workspace_path.transpose()?,
        })
    })
}

/// Bind a Session into a Workstream (user action).
///
/// v0.2: the same click also decides the Workstream's path list — if the
/// Session's own WorkspacePath is not in it, it is appended (position 0 for a
/// Workstream with no path yet) — and the binding records that WorkstreamPath by
/// exact equality (§1.8, §42.3-M1). One transaction, so a binding never exists
/// without its claim and a path never exists without the binding that asked for
/// it.
#[tauri::command]
pub fn bind_session_workstream(
    state: State<AppState>,
    session_id: String,
    workstream_id: String,
    role: String,
) -> Result<()> {
    with_db(&state, |db| {
        crate::workspace::session::record_user_binding(
            db,
            &session_id,
            &workstream_id,
            &role,
            binding_source::USER_ASSIGNED,
            1.0,
        )
    })
}

/// Remove a Session ↔ Workstream binding (user edit via Session Detail).
///
/// Only the binding goes away: the WorkstreamPath that brought the Session in
/// stays in the user's list (§1.9), and the pair leaves a permanent removal
/// tombstone so auto-classification cannot re-propose it.
#[tauri::command]
pub fn unbind_session_workstream(
    state: State<AppState>,
    session_id: String,
    workstream_id: String,
) -> Result<()> {
    with_db(&state, |db| db.unbind(&session_id, &workstream_id))
}

#[derive(Deserialize)]
pub struct DesiredBinding {
    pub workstream_id: String,
    pub role: String,
}

/// Atomic replace of a Session's Workstream bindings (Binding Modal save):
/// the backend diffs desired vs. current inside one transaction — unchanged
/// rows keep their provenance/created_at/cursors, role edits update only the
/// role, removed rows are deleted, and only newly added rows become
/// user_assigned. The frontend never orchestrates unbind+bind itself.
///
/// v0.2 adds the path half of the join: an added binding ensures its
/// WorkstreamPath (§1.8), a kept binding gets a NULL claim repaired when the
/// Session's own path is now in the list (§5.6), and no removal here ever
/// removes a WorkstreamPath (§1.9). The wire shape stays `(workstream_id,
/// role)` on purpose: which WorkstreamPath a binding came through is a derived
/// fact, not something the UI may assert.
#[tauri::command]
pub fn replace_session_bindings(
    state: State<AppState>,
    session_id: String,
    bindings: Vec<DesiredBinding>,
) -> Result<()> {
    let desired = bindings
        .into_iter()
        .map(|b| crate::workspace::session::DesiredBinding {
            workstream_id: b.workstream_id,
            role: b.role,
            workstream_path_id: None,
        })
        .collect::<Vec<_>>();
    with_db(&state, |db| {
        crate::workspace::session::replace_session_bindings(db, &session_id, &desired)
    })
}

/// Binding rows with workstream titles — lets the Sessions table show a
/// Workstream column and Assigned filters without N queries.
#[derive(Serialize)]
pub struct SessionBindingRow {
    pub session_id: String,
    pub workstream_id: String,
    pub role: String,
    pub workstream_title: String,
    /// Which `workstream_paths` entry brought this Session in, or `NULL` when
    /// the binding no longer corresponds to a path in the list (方案 §42.3-M2).
    pub workstream_path_id: Option<String>,
}

#[tauri::command]
pub fn list_session_bindings(state: State<AppState>) -> Result<Vec<SessionBindingRow>> {
    with_db(&state, |db| {
        let conn = db.read();
        let mut st = conn.prepare(
            "SELECT b.session_id, b.workstream_id, b.role, COALESCE(w.title, b.workstream_id),
                    b.workstream_path_id
             FROM session_workstream_bindings b
             LEFT JOIN workstreams w ON w.id = b.workstream_id",
        )?;
        let rows = st
            .query_map([], |r| {
                Ok(SessionBindingRow {
                    session_id: r.get(0)?,
                    workstream_id: r.get(1)?,
                    role: r.get(2)?,
                    workstream_title: r.get(3)?,
                    workstream_path_id: r.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}
