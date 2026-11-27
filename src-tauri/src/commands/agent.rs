//! Agent profiles share the native launcher, task service and Skill scopes.
use super::{noending_home, run_on_blocking_worker, AppState};
use crate::{
    agent::{self, *},
    domain::Project,
    error::{other, Result},
};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
fn home(app: &AppHandle) -> Result<crate::workspace::home::NoEndingHome> {
    noending_home(app).ok_or_else(|| other("NoEnding Home 未初始化"))
}
fn changed(app: &AppHandle) {
    let _ = app.emit("cli-data-changed", ());
}
#[tauri::command(rename_all = "snake_case")]
pub async fn create_agent_profile(
    app: AppHandle,
    state: State<'_, AppState>,
    layer: State<'_, Arc<crate::workspace::wiring::WorkspaceLayer>>,
    input: AgentCreate,
) -> Result<Project> {
    let home = home(&app)?;
    let db = state.db.clone();
    let layer = layer.inner().clone();
    let result =
        run_on_blocking_worker(move || agent::create_profile(&db, &home, &layer, input)).await??;
    changed(&app);
    Ok(result)
}
#[tauri::command(rename_all = "snake_case")]
pub async fn update_agent_profile(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    revision: String,
    patch: AgentPatch,
) -> Result<ProjectAgent> {
    let home = home(&app)?;
    let db = state.db.clone();
    let result =
        run_on_blocking_worker(move || agent::update_profile(&db, &home, &id, &revision, patch))
            .await??;
    changed(&app);
    Ok(result)
}
#[tauri::command(rename_all = "snake_case")]
pub async fn prepare_agent_files(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<ProjectAgent> {
    let home = home(&app)?;
    let db = state.db.clone();
    let result =
        run_on_blocking_worker(move || agent::prepare_profile_files(&db, &home, &id)).await??;
    changed(&app);
    Ok(result)
}

#[tauri::command]
pub fn agent_mailbox_list(
    app: AppHandle,
    state: State<AppState>,
    agent_id: String,
    cursor: Option<i64>,
) -> Result<crate::storage::agent_mailbox::Page> {
    state.db.agent_mailbox_list(
        &home(&app)?,
        &agent_id,
        None,
        crate::storage::agent_mailbox::Filter::All,
        cursor,
        30,
    )
}

#[tauri::command]
pub fn agent_mailbox_read(
    app: AppHandle,
    state: State<AppState>,
    agent_id: String,
    message_id: String,
) -> Result<crate::storage::agent_mailbox::Message> {
    state
        .db
        .agent_mailbox_read(&home(&app)?, &agent_id, None, &message_id)
}

#[tauri::command]
pub fn agent_mailbox_send(
    app: AppHandle,
    state: State<AppState>,
    agent_id: String,
    body: String,
) -> Result<crate::storage::agent_mailbox::Message> {
    let saved = state
        .db
        .agent_mailbox_send(&home(&app)?, &agent_id, None, &body)?;
    changed(&app);
    Ok(saved)
}

#[tauri::command]
pub fn agent_mailbox_change(
    app: AppHandle,
    state: State<AppState>,
    agent_id: String,
    change: crate::storage::agent_mailbox::Change,
) -> Result<crate::storage::agent_mailbox::Message> {
    let saved = state
        .db
        .agent_mailbox_change(&home(&app)?, &agent_id, None, &change)?;
    changed(&app);
    Ok(saved)
}
