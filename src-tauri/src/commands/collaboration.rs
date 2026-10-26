//! Desktop entry points for collaboration rooms. Authority rules live in the
//! shared collaboration module: the user always may stop or swap the
//! coordinator here, while model-side stop is coordinator-only via CLI/MCP.
use super::{with_db, AppState};
use crate::cli::CliError;
use crate::error::Result;
use serde_json::Value;
use tauri::{AppHandle, Emitter, State};

const EVENT: &str = "collaboration-changed";

fn map_cli(result: crate::cli::CliResult<Value>) -> Result<Value> {
    result.map_err(|CliError { code, message, .. }| {
        crate::error::other(format!("[{code}] {message}"))
    })
}

fn emit_changed(app: &AppHandle, task_id: &str, room_id: &str, run_id: &str, category: &str) {
    let _ = app.emit(
        EVENT,
        serde_json::json!({"task_id":task_id,"room_id":room_id,"run_id":run_id,"category":category}),
    );
}

#[tauri::command]
pub fn collaboration_room_get(state: State<AppState>, task_id: String) -> Result<Value> {
    with_db(&state, |db| {
        map_cli(crate::collaboration::room_view(db, &task_id))
    })
}

#[tauri::command]
pub fn collaboration_candidates(
    state: State<AppState>,
    task_id: String,
    cursor: Option<String>,
    limit: Option<usize>,
) -> Result<Value> {
    with_db(&state, |db| {
        map_cli(crate::collaboration::candidates(
            db,
            &task_id,
            cursor,
            limit.unwrap_or(50),
        ))
    })
}

#[tauri::command]
pub fn collaboration_room_messages(
    state: State<AppState>,
    room_id: String,
    run_id: Option<String>,
    cursor: Option<String>,
    limit: Option<usize>,
) -> Result<Value> {
    with_db(&state, |db| {
        map_cli(crate::collaboration::messages(
            db,
            &room_id,
            run_id.as_deref(),
            cursor,
            limit.unwrap_or(50),
        ))
    })
}

/// The user sends with no session identity; dedupe comes from the request id.
#[tauri::command]
pub fn collaboration_room_send(
    app: AppHandle,
    state: State<AppState>,
    room_id: String,
    run_id: String,
    body: String,
    request_id: String,
) -> Result<crate::storage::collaboration::CollaborationMessage> {
    let message = with_db(&state, |db| {
        db.collaboration_send(&room_id, &run_id, None, &body, &request_id)
    })?;
    emit_changed(&app, "", &room_id, &run_id, "message");
    Ok(message)
}

#[tauri::command]
pub fn collaboration_room_start(
    app: AppHandle,
    state: State<AppState>,
    task_id: String,
    members: Vec<crate::storage::collaboration::SelectedMember>,
    coordinator_session_id: Option<String>,
    expected_revision: Option<i64>,
    initial_message: Option<String>,
    request_id: String,
) -> Result<Value> {
    let started = with_db(&state, |db| {
        map_cli(crate::collaboration::start(
            db,
            crate::collaboration::StartParams {
                task_id: task_id.clone(),
                members,
                coordinator_session_id,
                expected_revision,
                initial_message,
                request_id: request_id.clone(),
            },
        ))
    })?;
    let room_id = started["room"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let run_id = started["run"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    emit_changed(&app, &task_id, &room_id, &run_id, "run");
    Ok(started)
}

#[tauri::command]
pub fn collaboration_room_stop(
    app: AppHandle,
    state: State<AppState>,
    room_id: String,
    run_id: String,
    expected_revision: Option<i64>,
    request_id: String,
) -> Result<Value> {
    let (task_id, stopped) = with_db(&state, |db| {
        let task_id = db
            .collaboration_room_by_id(&room_id)?
            .map(|room| room.task_id)
            .unwrap_or_default();
        let stopped = map_cli(crate::collaboration::stop(
            db,
            crate::collaboration::StopParams {
                room_id: room_id.clone(),
                run_id: run_id.clone(),
                expected_revision,
                // The desktop entry is the user; no session identity.
                initiator_session_id: None,
                request_id: request_id.clone(),
            },
        ))?;
        Ok::<_, crate::error::AppError>((task_id, stopped))
    })?;
    emit_changed(&app, &task_id, &room_id, &run_id, "run");
    Ok(stopped)
}

#[tauri::command]
pub fn collaboration_room_coordinator(
    app: AppHandle,
    state: State<AppState>,
    room_id: String,
    run_id: String,
    session_id: Option<String>,
    expected_revision: i64,
    request_id: String,
) -> Result<Value> {
    let _ = request_id; // Swap idempotence: repeating a successful swap is a no-op.
    let (task_id, room) = with_db(&state, |db| {
        let room = map_cli(crate::collaboration::set_coordinator(
            db,
            &room_id,
            &run_id,
            session_id,
            expected_revision,
            &request_id,
        ))?;
        let task_id = room["task_id"].as_str().unwrap_or_default().to_string();
        Ok::<_, crate::error::AppError>((task_id, room))
    })?;
    emit_changed(&app, &task_id, &room_id, &run_id, "run");
    Ok(room)
}
