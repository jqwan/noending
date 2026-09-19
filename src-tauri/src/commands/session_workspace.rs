//! Session commands: listing, detail, and Workstream binding.
//!
//! Owned by `Agent D` (方案 §19). Binding a Session now ensures a WorkstreamPath
//! and records which one brought it in, and `assign_session_project` /
//! `suggest_session_project` are legacy surface that Wave 1 un-registers (方案 §11).

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::domain::*;
use crate::error::{other, Result};
use crate::storage::{new_id, now};

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
}

pub type WorkstreamTitle = String;

#[tauri::command]
pub fn list_sessions(
    state: State<AppState>,
    project_id: Option<String>,
    agent: Option<String>,
) -> Result<Vec<Session>> {
    let agent = agent.and_then(|a| Agent::parse(&a));
    with_db(&state, |db| {
        db.list_sessions(crate::storage::SessionFilter { project_id, agent })
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
        Ok(SessionDetail {
            session,
            events,
            bindings,
            cursor,
            processed_cursor,
            classification: classification.as_str().to_string(),
        })
    })
}

#[tauri::command]
pub fn assign_session_project(
    state: State<AppState>,
    session_id: String,
    project_id: Option<String>,
) -> Result<()> {
    with_db(&state, |db| {
        db.0.execute(
            "UPDATE sessions SET project_id = ?2 WHERE id = ?1",
            rusqlite::params![session_id, project_id],
        )?;
        // a user correction is itself the strongest kind of evidence
        if let Some(pid) = project_id {
            db.insert_evidence(&ProjectAffinityEvidence {
                id: new_id(),
                session_id: Some(session_id),
                workstream_id: None,
                project_id: pid,
                evidence_type: "user_correction".into(),
                source: "manual assignment".into(),
                score: 10.0,
                created_at: now(),
            })?;
        }
        Ok(())
    })
}

/// cwd/repo only ever *suggest*: return the scored suggestion from recorded
/// evidence, never assign anything.
#[tauri::command]
pub fn suggest_session_project(
    state: State<AppState>,
    session_id: String,
) -> Result<serde_json::Value> {
    with_db(&state, |db| {
        let session = db
            .get_session(&session_id)?
            .ok_or_else(|| other("Session 不存在"))?;
        // record fresh evidence for the current cwd, then resolve
        crate::ingestion::record_session_project_evidence(db, &session);
        match db.resolve_project_affinity(&session_id)? {
            Some((project_id, score)) => {
                let name = db
                    .get_project(&project_id)?
                    .map(|p| p.name)
                    .unwrap_or_default();
                Ok(
                    serde_json::json!({ "project_id": project_id, "project_name": name, "score": score }),
                )
            }
            None => Ok(serde_json::json!({ "project_id": null, "score": 0.0 })),
        }
    })
}

#[tauri::command]
pub fn bind_session_workstream(
    state: State<AppState>,
    session_id: String,
    workstream_id: String,
    role: String,
) -> Result<()> {
    with_db(&state, |db| {
        crate::launcher::record_binding(
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
#[tauri::command]
pub fn replace_session_bindings(
    state: State<AppState>,
    session_id: String,
    bindings: Vec<DesiredBinding>,
) -> Result<()> {
    let desired = bindings
        .into_iter()
        .map(|b| (b.workstream_id, b.role))
        .collect::<Vec<_>>();
    with_db(&state, |db| {
        crate::launcher::replace_session_bindings(db, &session_id, &desired)
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
        let mut st = db.0.prepare(
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
