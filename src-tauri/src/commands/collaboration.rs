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

/// Watcher-side diff: classify external collaboration writes into one targeted
/// event per room. Message appends outrank run changes, which outrank member
/// (heartbeat/phase) churn; the desktop's own mutations already emit from
/// their command handlers.
pub(crate) fn emit_external_changes(
    app: &AppHandle,
    db: &crate::storage::Db,
    cache: &mut std::collections::HashMap<String, (String, String, i64, String)>,
) {
    let Ok(rooms) = (|| -> std::result::Result<Vec<(String, String, String, String, i64, String)>, crate::error::AppError> {
        let c = db.read();
        let mut stmt = c.prepare(
            "SELECT r.id, r.task_id, COALESCE(run.id,''), COALESCE(run.status,''),
                    COALESCE((SELECT MAX(seq) FROM collaboration_messages m WHERE m.room_id=r.id),0),
                    COALESCE((SELECT MAX(p.last_heartbeat_at) FROM collaboration_participations p WHERE p.room_id=r.id),'')
             FROM collaboration_rooms r
             LEFT JOIN collaboration_runs run ON run.room_id=r.id
               AND run.id=(SELECT id FROM collaboration_runs cr WHERE cr.room_id=r.id ORDER BY started_at DESC, id DESC LIMIT 1)",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(crate::error::AppError::from)?;
        Ok(rows)
    })() else {
        return;
    };
    for (room_id, task_id, run_id, status, max_seq, heartbeat) in rooms {
        let previous = cache.get(&room_id);
        let changed = match previous {
            // First sight: announce only rooms that actually have a run or mail.
            None => !run_id.is_empty() || max_seq > 0,
            Some((_, prev_status, prev_seq, prev_beat)) => {
                prev_seq != &max_seq || prev_status != &status || prev_beat != &heartbeat
            }
        };
        if !changed {
            continue;
        }
        let category = match previous {
            Some((_, prev_status, prev_seq, _)) if prev_seq != &max_seq => "message",
            Some((prev_run, prev_status, _, _))
                if prev_run != &run_id || prev_status != &status =>
            {
                "run"
            }
            _ => "member",
        };
        cache.insert(
            room_id.clone(),
            (run_id.clone(), status.clone(), max_seq, heartbeat.clone()),
        );
        let _ = app.emit(
            EVENT,
            serde_json::json!({"task_id":task_id,"room_id":room_id,"run_id":run_id,"category":category}),
        );
    }
}
