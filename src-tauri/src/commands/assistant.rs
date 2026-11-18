//! Assistant profiles share the native launcher, task service and Skill scopes.
use super::{noending_home, run_on_blocking_worker, AppState};
use crate::{
    agent_runtime::AgentRuntimeOverrides,
    assistant::{self, *},
    domain::SessionListItem,
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
pub async fn list_assistant_profiles(
    state: State<'_, AppState>,
    query: AssistantListQuery,
) -> Result<AssistantPage<AssistantCard>> {
    let db = state.db.clone();
    run_on_blocking_worker(move || assistant::list_profiles(&db, query)).await?
}
#[tauri::command(rename_all = "snake_case")]
pub async fn get_assistant_profile(
    state: State<'_, AppState>,
    id: String,
) -> Result<AssistantDetail> {
    let db = state.db.clone();
    run_on_blocking_worker(move || assistant::detail(&db, &id)).await?
}
#[tauri::command(rename_all = "snake_case")]
pub async fn create_assistant_profile(
    app: AppHandle,
    state: State<'_, AppState>,
    layer: State<'_, Arc<crate::workspace::wiring::WorkspaceLayer>>,
    input: AssistantCreate,
) -> Result<AssistantDetail> {
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
    revision: i64,
    patch: AssistantPatch,
) -> Result<AssistantDetail> {
    let home = home(&app)?;
    let db = state.db.clone();
    let result =
        run_on_blocking_worker(move || assistant::update_profile(&db, &home, &id, revision, patch))
            .await??;
    changed(&app);
    Ok(result)
}
#[tauri::command(rename_all = "snake_case")]
pub async fn set_assistant_responsibilities(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    revision: i64,
    project_ids: Vec<String>,
    task_ids: Vec<String>,
) -> Result<AssistantDetail> {
    let home = home(&app)?;
    let db = state.db.clone();
    let result = run_on_blocking_worker(move || {
        assistant::set_responsibilities(&db, &home, &id, revision, &project_ids, &task_ids)
    })
    .await??;
    changed(&app);
    Ok(result)
}
#[tauri::command(rename_all = "snake_case")]
pub async fn set_assistant_archived(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    revision: i64,
    archived: bool,
) -> Result<AssistantDetail> {
    let home = home(&app)?;
    let db = state.db.clone();
    let result = run_on_blocking_worker(move || {
        assistant::set_archived(&db, &home, &id, revision, archived)
    })
    .await??;
    changed(&app);
    Ok(result)
}
#[tauri::command(rename_all = "snake_case")]
pub async fn list_assistant_sessions(
    state: State<'_, AppState>,
    id: String,
    query: AssistantListQuery,
) -> Result<AssistantPage<SessionListItem>> {
    let db = state.db.clone();
    run_on_blocking_worker(move || assistant::list_profile_sessions(&db, &id, query)).await?
}
#[tauri::command(rename_all = "snake_case")]
pub async fn prepare_assistant_files(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    revision: i64,
) -> Result<AssistantDetail> {
    let home = home(&app)?;
    let db = state.db.clone();
    let result =
        run_on_blocking_worker(move || assistant::prepare_profile_files(&db, &home, &id, revision))
            .await??;
    changed(&app);
    Ok(result)
}
#[tauri::command(rename_all = "snake_case")]
pub fn start_assistant_session(
    app: AppHandle,
    state: State<'_, AppState>,
    terminal: State<'_, crate::terminal::TerminalRegistry>,
    id: String,
    task_ids: Option<Vec<String>>,
    initial_message: Option<String>,
    runtime_override: Option<AgentRuntimeOverrides>,
) -> Result<crate::launcher::LaunchResult> {
    let home = home(&app)?;
    let tasks = task_ids.unwrap_or_default();
    let (profile, path, runtime) =
        assistant::profile_launch(&state.db, &home, &id, &tasks, runtime_override.as_ref())?;
    let result = super::launch_embedded_new(
        app.clone(),
        state,
        terminal,
        profile.agent.as_str().into(),
        tasks,
        Some(path.canonical_path),
        initial_message,
        runtime,
    )?;
    changed(&app);
    Ok(result)
}
