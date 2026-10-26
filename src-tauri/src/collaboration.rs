//! Collaboration room operations shared by the desktop, CLI and MCP: room
//! views, candidates with real wake capability, start/stop/coordinator, the
//! shared message sequence and read acknowledgement. The plan's authority
//! rules live here so every entry point enforces the same contracts.
use crate::cli::{CliError, CliResult};
use crate::storage::collaboration as store;
use crate::storage::Db;
use crate::workspace::home::NoEndingHome;
use rmcp::schemars;
use serde_json::{json, Value};

/// Connection freshness: a lease renewed within this window is connected.
const CONNECTED_WINDOW_SECS: i64 = 30;

fn session_brief(db: &Db, session_id: &str) -> Value {
    match db.get_session(session_id) {
        Ok(Some(session)) => json!({
            "id": session.id,
            "title": session.title,
            "agent": session.agent.as_str(),
        }),
        // A purged member keeps its historical messages; the view degrades to
        // the bare id instead of fabricating a "user" or an empty row.
        _ => json!({"id": session_id, "title": Value::Null, "agent": Value::Null}),
    }
}

fn parse_timestamp(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|t| t.with_timezone(&chrono::Utc))
}

fn connection_state(
    participation: Option<&store::CollaborationParticipation>,
    now: chrono::DateTime<chrono::Utc>,
) -> &'static str {
    match participation {
        None => "pending",
        Some(p) if p.left_at.is_some() => "left",
        Some(p) => match parse_timestamp(&p.last_heartbeat_at) {
            Some(beat) if (now - beat).num_seconds() <= CONNECTED_WINDOW_SECS => "connected",
            _ => "disconnected",
        },
    }
}

/// The full room view for one task: configuration, current run and one member
/// entry per selected session, composed from four storage projections.
pub(crate) fn room_view(db: &Db, task_id: &str) -> CliResult<Value> {
    let Some((room, run)) = db.collaboration_room_by_task(task_id)? else {
        let title = db
            .get_workstream(task_id)?
            .map(|t| json!(t.title))
            .unwrap_or(Value::Null);
        return Ok(
            json!({"task_id":task_id,"title":title,"room":Value::Null,"run":Value::Null,"members":[]}),
        );
    };
    let title = db
        .get_workstream(task_id)?
        .map(|t| json!(t.title))
        .unwrap_or(Value::Null);
    let now = chrono::Utc::now();
    let participations: Vec<store::CollaborationParticipation> = match &run {
        Some(run) => db.collaboration_participations_for_run(&run.id)?,
        None => vec![],
    };
    let run_id = run.as_ref().map(|r| r.id.clone());
    let members = room
        .selected_members
        .iter()
        .map(|selected| {
            let participation = participations
                .iter()
                .find(|p| p.session_id == selected.session_id);
            let (wake_status, wake_error) = match (&run, &run_id) {
                (Some(_), Some(run_id)) => {
                    let member = db
                        .collaboration_run_members(run_id)?
                        .into_iter()
                        .find(|m| m.session_id == selected.session_id);
                    match member {
                        Some(m) => (json!(m.wake_status), m.wake_error.map(|e| json!(e))),
                        None => (Value::Null, None),
                    }
                }
                _ => (Value::Null, None),
            };
            let (last_read_at, last_read_seq, unread_count, awaiting_since) = match &run_id {
                Some(run_id) => {
                    let read = db.collaboration_read_of(&room.id, &selected.session_id, run_id)?;
                    let unread = db.collaboration_unread_of(&room.id, run_id, &selected.session_id)?;
                    (
                        read.as_ref().map(|r| json!(r.read_at)).unwrap_or(Value::Null),
                        read.as_ref().map(|r| json!(r.through_seq)).unwrap_or(Value::Null),
                        json!(unread.count),
                        unread
                            .awaiting_since
                            .map(|at| json!(at))
                            .unwrap_or(Value::Null),
                    )
                }
                _ => (Value::Null, Value::Null, json!(0), Value::Null),
            };
            Ok(json!({
                "session_id": selected.session_id,
                "wake_method": selected.wake_method,
                "wake_status": wake_status,
                "connection_state": connection_state(participation, now),
                "phase": participation.and_then(|p| p.phase.clone()).map(|p| json!(p)).unwrap_or(Value::Null),
                "last_heartbeat_at": participation.map(|p| json!(p.last_heartbeat_at)).unwrap_or(Value::Null),
                "last_read_at": last_read_at,
                "last_read_seq": last_read_seq,
                "unread_count": unread_count,
                "awaiting_since": awaiting_since,
                "error": wake_error,
            }))
        })
        .collect::<CliResult<Vec<_>>>()?;
    Ok(json!({
        "task_id": task_id,
        "title": title,
        "room": room,
        "run": run,
        "members": members,
    }))
}

/// Task-associated, unarchived sessions with their actually available wake
/// modes. Terminal capability reuses the launcher's resume validation
/// (adapter support, source presence, install); desktop capability follows the
/// adapter's resume route.
pub(crate) fn candidates(
    db: &Db,
    task_id: &str,
    cursor: Option<String>,
    limit: usize,
) -> CliResult<Value> {
    if !(1..=50).contains(&limit) {
        return Err(CliError::new(
            "invalid_arguments",
            "无效的分页大小（1～50）",
        ));
    }
    let task = db
        .get_workstream(task_id)?
        .ok_or_else(|| CliError::new("not_found", format!("task {task_id} 不存在")))?;
    let _ = task;
    let page = db.collaboration_candidate_sessions(task_id, cursor.as_deref(), limit)?;
    let states = crate::launcher::availability::terminal_resume_states(db, &page.items)?;
    let items = page
        .items
        .iter()
        .filter_map(|id| db.get_session(id).ok().flatten().map(|s| (id.clone(), s)))
        .map(|(id, session)| {
            let adapter = crate::adapters::adapter_for(session.agent);
            let desktop = matches!(
                adapter.desktop_resume_route(&session),
                crate::adapters::ResumeRoute::Desktop(_)
            );
            let terminal = states.get(&id).cloned().unwrap_or(
                crate::launcher::availability::TerminalResumeState {
                    available: false,
                    reason: None,
                },
            );
            let mut modes = Vec::new();
            if terminal.available {
                modes.push(json!(store::WAKE_TERMINAL));
            }
            if desktop {
                modes.push(json!(store::WAKE_DESKTOP));
            }
            let reason = if modes.is_empty() {
                terminal
                    .reason
                    .or_else(|| Some("该会话没有可用的唤醒方式".to_string()))
            } else {
                None
            };
            json!({
                "session_id": id,
                "title": session.title,
                "agent": session.agent.as_str(),
                "wake_modes": modes,
                "unavailable_reason": reason,
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({"items":items,"next_cursor":page.next_cursor}))
}

pub(crate) struct StartParams {
    pub task_id: String,
    pub members: Vec<store::SelectedMember>,
    pub coordinator_session_id: Option<String>,
    pub expected_revision: Option<i64>,
    pub initial_message: Option<String>,
    pub request_id: String,
}
pub(crate) fn start(db: &Db, params: StartParams) -> CliResult<Value> {
    // Revalidate the requested wake method against real capability; the user
    // picked from candidates, but the backend never trusts that filter.
    let states = crate::launcher::availability::terminal_resume_states(
        db,
        &params
            .members
            .iter()
            .map(|m| m.session_id.clone())
            .collect::<Vec<_>>(),
    )?;
    for member in &params.members {
        let session = db
            .get_session(&member.session_id)?
            .ok_or_else(|| CliError::new("not_found", "成员会话不存在"))?;
        let capable = match member.wake_method.as_str() {
            store::WAKE_TERMINAL => states.get(&member.session_id).is_some_and(|s| s.available),
            store::WAKE_DESKTOP => {
                matches!(
                    crate::adapters::adapter_for(session.agent).desktop_resume_route(&session),
                    crate::adapters::ResumeRoute::Desktop(_)
                )
            }
            _ => false,
        };
        if !capable {
            return Err(CliError::new(
                "invalid_arguments",
                format!(
                    "成员 {} 不支持所选唤醒方式 {}",
                    member.session_id, member.wake_method
                ),
            ));
        }
    }
    let outcome = db.collaboration_start(
        &params.task_id,
        &params.members,
        params.coordinator_session_id.as_deref(),
        params.expected_revision,
        params.initial_message.as_deref(),
        &params.request_id,
    )?;
    // The desktop always receives the same full RoomView shape as room_get.
    room_view(db, &params.task_id)
}

/// Every run-scoped operation answers only the room's CURRENT run: an old run
/// id is stale, a stopped run is over — both are `run_inactive`, never a
/// silent success that would leak into a newer run.
fn guard_current_run(db: &Db, room_id: &str, run_id: &str) -> CliResult<()> {
    let current = db
        .collaboration_current_run(room_id)?
        .ok_or_else(|| CliError::new("run_inactive", "协作室还没有任何运行"))?;
    if current.id != run_id {
        return Err(CliError::new(
            "run_inactive",
            "协作运行已更新，不能在旧运行上操作",
        ));
    }
    if current.status == "stopped" {
        return Err(CliError::new("run_inactive", "协作已停止"));
    }
    Ok(())
}

pub(crate) struct StopParams {
    pub room_id: String,
    pub run_id: String,
    pub expected_revision: Option<i64>,
    /// None = desktop user (always allowed). Some(session) = coordinator only.
    pub initiator_session_id: Option<String>,
    pub request_id: String,
}
pub(crate) fn stop(db: &Db, params: StopParams) -> CliResult<Value> {
    guard_current_run(db, &params.room_id, &params.run_id)?;
    if let Some(initiator) = &params.initiator_session_id {
        let room = db
            .collaboration_room_by_id(&params.room_id)?
            .ok_or_else(|| CliError::new("not_found", "协作室不存在"))?;
        if room.coordinator_session_id.as_deref() != Some(initiator.as_str()) {
            return Err(CliError::new(
                "operation_rejected",
                "只有当前负责人可以停止协作",
            ));
        }
    }
    if let Some(expected) = params.expected_revision {
        let room = db.collaboration_room_by_id(&params.room_id)?;
        if room.is_some_and(|room| room.revision != expected) {
            return Err(CliError::new(
                "operation_rejected",
                "协作室配置已被其他人修改，请刷新后重试",
            ));
        }
    }
    let (_, notice) = db.collaboration_stop(
        &params.room_id,
        &params.run_id,
        params.initiator_session_id.as_deref(),
        &params.request_id,
    )?;
    let task_id = db
        .collaboration_room_by_id(&params.room_id)?
        .map(|room| room.task_id)
        .unwrap_or_default();
    room_view(db, &task_id)
}

pub(crate) fn set_coordinator(
    db: &Db,
    room_id: &str,
    run_id: &str,
    session_id: Option<String>,
    expected_revision: i64,
    request_id: &str,
) -> CliResult<Value> {
    guard_current_run(db, room_id, run_id)?;
    db.collaboration_set_coordinator(
        room_id,
        run_id,
        session_id.as_deref(),
        expected_revision,
        request_id,
    )?;
    let task_id = db
        .collaboration_room_by_id(room_id)?
        .map(|room| room.task_id)
        .unwrap_or_default();
    room_view(db, &task_id)
}

pub(crate) fn send(
    db: &Db,
    home: &NoEndingHome,
    room_id: &str,
    run_id: &str,
    body: &str,
    claimed_sender: Option<&str>,
    request_id: &str,
) -> CliResult<Value> {
    guard_current_run(db, room_id, run_id)?;
    // CLI/MCP callers resolve through the mailbox actor rules (terminal
    // binding wins over an explicit claim); the desktop sends as the user.
    let sender = match claimed_sender {
        Some(_) => Some(crate::mailbox::actor(db, home, claimed_sender)?),
        None => None,
    };
    let message = db.collaboration_send(room_id, run_id, sender.as_deref(), body, request_id)?;
    Ok(json!(message))
}

pub(crate) fn messages(
    db: &Db,
    room_id: &str,
    run_id: Option<&str>,
    cursor: Option<String>,
    limit: usize,
) -> CliResult<Value> {
    let page = db.collaboration_messages_page(room_id, run_id, cursor.as_deref(), limit)?;
    Ok(json!(page))
}

/// CLI/MCP send contract: an explicit member session (the terminal binding
/// wins, mirroring mailbox rules) writing to the room's current run.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoomSend {
    pub room: String,
    pub run: String,
    pub from_session: String,
    pub message: String,
}
/// CLI/MCP stop contract: only the run's current coordinator may stop.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoomStop {
    pub room: String,
    pub run: String,
    pub from_session: String,
}
pub(crate) fn room_view_by_room(db: &Db, room_id: &str) -> CliResult<Value> {
    let room = db
        .collaboration_room_by_id(room_id)?
        .ok_or_else(|| CliError::new("not_found", format!("协作室 {room_id} 不存在")))?;
    room_view(db, &room.task_id)
}
