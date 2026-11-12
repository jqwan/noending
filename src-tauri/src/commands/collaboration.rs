//! Desktop entry points for collaboration rooms. Authority rules live in the
//! shared collaboration module: the user always may stop or swap the
//! coordinator here, while model-side stop is coordinator-only via CLI/MCP.
use super::{with_db, AppState};
use crate::cli::CliError;
use crate::error::Result;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

const EVENT: &str = "collaboration-changed";

fn map_cli<T>(result: crate::cli::CliResult<T>) -> Result<T> {
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
    let room_id = started.room.id;
    let run_id = started.run.id;
    if !started.appended.is_empty() {
        let _ = app.emit("mailbox-changed", ());
        app.state::<crate::terminal::TerminalRegistry>()
            .kick_notifications("");
    }
    // The mailbox handles terminal invitations. Opening a desktop app is a
    // separate, idempotent action for members who selected that route.
    for member in started
        .members
        .into_iter()
        .filter(|member| member.wake_method == crate::storage::collaboration::WAKE_DESKTOP)
    {
        let session_id = member.session_id;
        let derived = format!("{request_id}:{session_id}");
        let _ = wake_with_ledger(&app, &state, &room_id, &run_id, &session_id, &derived);
    }
    emit_changed(&app, &task_id, &room_id, &run_id, "run");
    with_db(&state, |db| {
        map_cli(crate::collaboration::room_view(db, &task_id))
    })
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
    cache: &mut std::collections::HashMap<String, String>,
) {
    if let Ok(events) = external_changes(db, cache) {
        for event in events {
            let _ = app.emit(EVENT, event);
        }
    }
}

fn external_changes(
    db: &crate::storage::Db,
    cache: &mut std::collections::HashMap<String, String>,
) -> Result<Vec<Value>> {
    // One composite signature per room covering every externally-writable
    // surface the room view projects: run identity/status, message seq,
    // heartbeat, leave, read positions, phase and the config revision. A
    // member that left stops heartbeating, so left_at must be watched or the
    // UI would stay on a stale "connected".
    // Optional timestamps stay empty until members read, change phase or leave.
    let rooms = {
        let c = db.read();
        let mut stmt = c.prepare(
            "SELECT r.id, r.task_id,
                    COALESCE(run.id,'') || '|' || COALESCE(run.status,'') || '|' ||
                    COALESCE((SELECT MAX(seq) FROM collaboration_messages m WHERE m.room_id=r.id),0) || '|' ||
                    COALESCE((SELECT MAX(p.last_heartbeat_at) FROM collaboration_participations p WHERE p.room_id=r.id),'') || '|' ||
                    COALESCE((SELECT MAX(p.left_at) FROM collaboration_participations p WHERE p.room_id=r.id),'') || '|' ||
                    COALESCE((SELECT MAX(rd.read_at) FROM collaboration_reads rd WHERE rd.room_id=r.id),'') || '|' ||
                    COALESCE((SELECT MAX(p.phase_since) FROM collaboration_participations p WHERE p.room_id=r.id),'') || '|' ||
                    r.revision
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
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    let mut events = Vec::new();
    for (room_id, task_id, signature) in rooms {
        let previous = cache.get(&room_id);
        if previous == Some(&signature) {
            continue;
        }
        // First sight: announce only rooms that actually have a run.
        if previous.is_none() && signature.split('|').next().unwrap_or_default().is_empty() {
            continue;
        }
        let fields: Vec<&str> = signature.split('|').collect();
        let category = match previous {
            None => "run",
            Some(prev) => {
                let prev_fields: Vec<&str> = prev.split('|').collect();
                if prev_fields.get(2) != fields.get(2) {
                    "message"
                } else if prev_fields.get(0) != fields.get(0) || prev_fields.get(1) != fields.get(1)
                {
                    "run"
                } else if prev_fields.get(7) != fields.get(7) {
                    "room"
                } else {
                    "member"
                }
            }
        };
        let run_id = fields.first().copied().unwrap_or_default().to_string();
        cache.insert(room_id.clone(), signature);
        events.push(
            serde_json::json!({"task_id":task_id,"room_id":room_id,"run_id":run_id,"category":category}),
        );
    }
    Ok(events)
}

/// Open the member's selected app. For terminals this is the user's manual
/// action. Automatic wake and reminders run through mailbox arrivals.
fn wake_one(app: &AppHandle, state: &AppState, session_id: &str, wake_method: &str) -> Result<()> {
    match wake_method {
        crate::storage::collaboration::WAKE_TERMINAL => {
            crate::commands::wake_mailbox_terminal(app, session_id, false)?;
        }
        crate::storage::collaboration::WAKE_DESKTOP => {
            let route = with_db(state, |db| {
                let session = db
                    .get_session(session_id)?
                    .ok_or_else(|| crate::error::other("成员会话不存在"))?;
                Ok(crate::adapters::adapter_for(session.agent).desktop_resume_route(&session))
            })?;
            match route {
                crate::adapters::ResumeRoute::Desktop(open) => {
                    crate::platform::launcher::open_uri(&open.uri)?;
                    crate::commands::ingestion::enqueue(
                        app,
                        crate::commands::ingestion::IngestScope::RefreshSession(
                            session_id.to_string(),
                        ),
                    );
                }
                _ => return Err(crate::error::other("该会话没有桌面端打开方式")),
            }
        }
        other => return Err(crate::error::other(format!("未知唤醒方式 {other}"))),
    }
    Ok(())
}

/// One member's wake. The ledger row is claimed atomically BEFORE the side
/// effect, so concurrent equal requests cannot both launch and a replay of a
/// different operation is rejected by kind/fingerprint. A determinate failure
/// is saved on the member. A replay keeps that outcome, and an explicit
/// retry uses a new request ID.
fn wake_with_ledger(
    app: &AppHandle,
    state: &AppState,
    room_id: &str,
    run_id: &str,
    session_id: &str,
    request_id: &str,
) -> Result<()> {
    let wake_method = with_db(state, |db| {
        db.collaboration_run_members(run_id)?
            .into_iter()
            .find(|m| m.session_id == session_id)
            .map(|m| m.wake_method)
            .ok_or_else(|| crate::error::other("该会话不是本次运行成员"))
    })?;
    let claimed = with_db(state, |db| {
        db.collaboration_claim_wake(request_id, room_id, run_id, session_id)
    })?;
    if !claimed {
        return Ok(());
    }
    let error = wake_one(app, state, session_id, &wake_method)
        .err()
        .map(|error| error.to_string());
    let status = if error.is_some() { "failed" } else { "opened" };
    with_db(state, |db| {
        db.collaboration_set_wake_status(run_id, session_id, status, error.as_deref(), request_id)
    })
}

fn member_state(state: &AppState, task_id: &str, session_id: &str) -> Result<Value> {
    let view = with_db(state, |db| {
        map_cli(crate::collaboration::room_view(db, task_id))
    })?;
    Ok(view["members"]
        .as_array()
        .and_then(|members| {
            members
                .iter()
                .find(|m| m["session_id"] == json!(session_id))
        })
        .cloned()
        .unwrap_or(Value::Null))
}

/// Retry one member's wake (user action). Fails cleanly for old/stopped runs.
#[tauri::command]
pub fn collaboration_member_wake(
    app: AppHandle,
    state: State<AppState>,
    room_id: String,
    run_id: String,
    session_id: String,
    request_id: String,
) -> Result<Value> {
    let task_id = with_db(&state, |db| {
        map_cli(
            crate::collaboration::guard_current_run(db, &room_id, &run_id).map(|_| Value::Null),
        )?;
        db.collaboration_room_by_id(&room_id)?
            .map(|room| room.task_id)
            .ok_or_else(|| crate::error::other("协作室不存在"))
    })?;
    wake_with_ledger(&app, &state, &room_id, &run_id, &session_id, &request_id)?;
    let member = member_state(&state, &task_id, &session_id)?;
    emit_changed(&app, &task_id, &room_id, &run_id, "member");
    Ok(member)
}

#[cfg(test)]
mod tests {
    use super::external_changes;
    use crate::storage::{collaboration::SelectedMember, Db};
    use std::{
        collections::HashMap,
        path::PathBuf,
        sync::{Arc, Mutex},
        time::Duration,
    };

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "noending-room-refresh-{}",
                crate::storage::new_id()
            )))
        }

        fn open(&self) -> Db {
            Db::open(&self.0.join("noending.db")).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn external_message_notifies_the_open_room_before_any_member_has_left() {
        let fixture = Fixture::new();
        let db = Arc::new(fixture.open());
        db.write().execute_batch(
            "INSERT INTO workstreams(id,title,created_at,updated_at) VALUES
              ('task','Live room','2026-01-01','2026-01-01'),
              ('empty-task','Not started','2026-01-01','2026-01-01');
             INSERT INTO sessions(id,agent,root_agent_session_id,source_kind,source_path) VALUES
              ('a','codex','a','jsonl','/a'),('b','codex','b','jsonl','/b');
             INSERT INTO session_workstreams(session_id,workstream_id) VALUES ('a','task'),('b','task');
             INSERT INTO collaboration_rooms(id,task_id,selected_members,created_at,updated_at)
              VALUES ('empty-room','empty-task','[]','2026-01-01','2026-01-01');"
        ).unwrap();
        let members = ["a", "b"].map(|id| SelectedMember {
            session_id: id.into(),
            wake_method: "desktop".into(),
        });
        let started = db
            .collaboration_start("task", &members, None, None, None, "start")
            .unwrap();
        let room = started.room.id;
        let run = started.run.id;
        let mut cache = HashMap::new();
        let initial = external_changes(&db, &mut cache)
            .expect("rooms without optional member timestamps remain observable");
        assert_eq!(
            initial,
            [serde_json::json!({"task_id":"task","room_id":room,"run_id":run,"category":"run"})]
        );
        assert!(external_changes(&db, &mut cache).unwrap().is_empty());

        let external = fixture.open();
        external
            .collaboration_join(
                &room,
                &run,
                "a",
                "cli",
                "external-member",
                &crate::storage::now(),
            )
            .unwrap();
        let member_change = external_changes(&db, &mut cache).unwrap();
        assert_eq!(member_change[0]["category"], "member");
        // Reads, phases and exits are still absent. One such room must not
        // suppress notifications for itself or any other room.
        let (send, receive) = std::sync::mpsc::channel();
        let observed = db.clone();
        let cache = Mutex::new(cache);
        let watcher = crate::cli::watch_changes(db.clone(), move || {
            for event in external_changes(&observed, &mut cache.lock().unwrap()).unwrap() {
                let _ = send.send(event);
            }
        });
        let message = external
            .collaboration_send(&room, &run, Some("a"), "external CLI message", "message")
            .unwrap();
        let event = receive
            .recv_timeout(Duration::from_secs(5))
            .expect("message event without navigating away");
        assert_eq!(
            event,
            serde_json::json!({"task_id":"task","room_id":room,"run_id":run,"category":"message"})
        );
        assert_eq!(
            db.collaboration_messages_page(&room, Some(&run), None, 10)
                .unwrap()
                .items
                .last()
                .unwrap()
                .id,
            message.id
        );
        watcher.shutdown();
    }
}
