//! Session lifecycle commands: Trash / Restore / prepared permanent deletion
//! (Session Lifecycle & Deletion v0.1). Thin Tauri surface over
//! `crate::lifecycle`, which owns the rules. The frontend only ever submits
//! ids — never paths, agent session ids or deletion targets (方案 §19).

use tauri::State;

use crate::domain::{Session, SessionDeletionJob};
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

#[tauri::command]
pub fn prepare_session_permanent_delete(
    state: State<AppState>,
    session_id: String,
) -> Result<lifecycle::PermanentDeletionPreview> {
    with_db(&state, |db| {
        lifecycle::prepare_session_permanent_delete(db, &session_id)
    })
}

#[tauri::command]
pub fn execute_session_permanent_delete(
    state: State<AppState>,
    job_id: String,
) -> Result<lifecycle::PermanentDeletionResult> {
    with_db(&state, |db| {
        lifecycle::execute_session_permanent_delete(db, &job_id)
    })
}

#[tauri::command]
pub fn cancel_session_permanent_delete(state: State<AppState>, job_id: String) -> Result<()> {
    with_db(&state, |db| {
        lifecycle::cancel_session_permanent_delete(db, &job_id)
    })
}

#[tauri::command]
pub fn get_session_deletion_job(
    state: State<AppState>,
    session_id: String,
) -> Result<Option<SessionDeletionJob>> {
    with_db(&state, |db| {
        lifecycle::get_session_deletion_job(db, &session_id)
    })
}
