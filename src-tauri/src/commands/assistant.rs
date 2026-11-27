//! Assistant profiles share the native launcher, task service and Skill scopes.
use super::{noending_home, run_on_blocking_worker, AppState};
use crate::{
    assistant::{self, *},
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
pub async fn create_assistant_profile(
    app: AppHandle,
    state: State<'_, AppState>,
    layer: State<'_, Arc<crate::workspace::wiring::WorkspaceLayer>>,
    input: AssistantCreate,
) -> Result<Project> {
    let home = home(&app)?;
    let db = state.db.clone();
    let layer = layer.inner().clone();
    let result =
        run_on_blocking_worker(move || assistant::create_profile(&db, &home, &layer, input))
            .await??;
    changed(&app);
    Ok(result)
}
#[tauri::command(rename_all = "snake_case")]
pub async fn update_assistant_profile(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    revision: String,
    patch: AssistantPatch,
) -> Result<ProjectAssistant> {
    let home = home(&app)?;
    let db = state.db.clone();
    let result = run_on_blocking_worker(move || {
        assistant::update_profile(&db, &home, &id, &revision, patch)
    })
    .await??;
    changed(&app);
    Ok(result)
}
#[tauri::command(rename_all = "snake_case")]
pub async fn prepare_assistant_files(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<ProjectAssistant> {
    let home = home(&app)?;
    let db = state.db.clone();
    let result =
        run_on_blocking_worker(move || assistant::prepare_profile_files(&db, &home, &id)).await??;
    changed(&app);
    Ok(result)
}

#[tauri::command]
pub fn assistant_mailbox_list(
    app: AppHandle,
    state: State<AppState>,
    assistant_id: String,
    cursor: Option<i64>,
) -> Result<crate::storage::assistant_mailbox::Page> {
    state.db.assistant_mailbox_list(
        &home(&app)?,
        &assistant_id,
        None,
        crate::storage::assistant_mailbox::Filter::All,
        cursor,
        30,
    )
}

#[tauri::command]
pub fn assistant_mailbox_read(
    app: AppHandle,
    state: State<AppState>,
    assistant_id: String,
    message_id: String,
) -> Result<crate::storage::assistant_mailbox::Message> {
    state
        .db
        .assistant_mailbox_read(&home(&app)?, &assistant_id, None, &message_id)
}

#[tauri::command]
pub fn assistant_mailbox_send(
    app: AppHandle,
    state: State<AppState>,
    assistant_id: String,
    body: String,
) -> Result<crate::storage::assistant_mailbox::Message> {
    let saved = state
        .db
        .assistant_mailbox_send(&home(&app)?, &assistant_id, None, &body)?;
    changed(&app);
    Ok(saved)
}

#[tauri::command]
pub fn assistant_mailbox_change(
    app: AppHandle,
    state: State<AppState>,
    assistant_id: String,
    change: crate::storage::assistant_mailbox::Change,
) -> Result<crate::storage::assistant_mailbox::Message> {
    let saved = state
        .db
        .assistant_mailbox_change(&home(&app)?, &assistant_id, None, &change)?;
    changed(&app);
    Ok(saved)
}
