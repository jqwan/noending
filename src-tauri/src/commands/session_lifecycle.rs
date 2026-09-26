//! Session lifecycle commands: Trash / Restore and the permanent LOCAL
//! deletion. Thin Tauri surface over `crate::lifecycle`,
//! which owns the rules. The frontend only ever submits session ids — never
//! paths, agent session ids or deletion targets, and there is no deletion job
//! to coordinate: preview is stateless, execute re-checks everything.

use tauri::State;

use crate::domain::Session;
use crate::error::Result;
use crate::lifecycle;

use super::{with_db, AppState};

#[tauri::command]
pub fn trash_session(state: State<AppState>, session_id: String) -> Result<Session> {
    with_db(&state, |db| lifecycle::trash_session(db, &session_id))
}

#[tauri::command]
pub fn restore_session(state: State<AppState>, session_id: String) -> Result<Session> {
    with_db(&state, |db| lifecycle::restore_session(db, &session_id))
}

/// Stateless preview of the permanent LOCAL deletion: fresh ROOT
/// source verdict plus the counts. No job row, nothing to cancel.
#[tauri::command]
pub fn get_session_local_delete_preview(
    state: State<AppState>,
    session_id: String,
) -> Result<lifecycle::LocalDeletePreview> {
    with_db(&state, |db| {
        lifecycle::get_session_local_delete_preview(db, &session_id)
    })
}

/// Execute the permanent LOCAL deletion. NoEnding data only — the
/// Agent source was already (re-)confirmed Missing inside, and no code path
/// here can touch it: NoEnding never deletes Agent-owned sources.
#[tauri::command]
pub fn permanently_delete_session(
    state: State<AppState>,
    session_id: String,
) -> Result<lifecycle::PermanentDeleteResult> {
    with_db(&state, |db| {
        lifecycle::permanently_delete_session(db, &session_id)
    })
}
