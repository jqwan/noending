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

/// The participation rules every entry point shows. The desktop wake hint and
/// `member.join_prompt` render this same text, so the UI copies one fact instead
/// of re-typing rules that would drift from the backend.
pub(crate) fn join_prompt(task_id: &str, room_id: &str, run_id: &str, session_id: &str) -> String {
    format!(
        "[NoEnding 协作室] 任务 {task_id}｜房间 {room_id}｜运行 {run_id}｜你的会话 {session_id}\n\
         协作期间只用本房间沟通：暂无任务、发言完毕或一次 wait timeout 都不表示协作结束，不要因此提交宿主最终回复。\n\
         闭环：room join → room wait → 完整读完本批（next_cursor 非空先续读）→ 按需回复 → room ack 到已读 seq → 继续 wait；纯已读用 ack 表达，不刷「收到」。\n\
         只有收到 stopped、用户明确退出或不可恢复限制时才收尾；正常退场先 ack 已读部分再 room leave。\n\
         身份与参数规则见统一 noending 技能的协作参考。"
    )
}

/// The full room view for one task: configuration, current run and one member
/// entry per selected session, composed from four storage projections.
pub(crate) fn room_view(db: &Db, task_id: &str) -> CliResult<Value> {
    let title = db
        .get_workstream(task_id)?
        .ok_or_else(|| CliError::new("not_found", "任务不存在"))?
        .title;
    let Some((room, run)) = db.collaboration_room_by_task(task_id)? else {
        return Ok(
            json!({"task_id":task_id,"title":title,"room":Value::Null,"run":Value::Null,"members":[]}),
        );
    };
    let now = chrono::Utc::now();
    let participations: Vec<store::CollaborationParticipation> = match &run {
        Some(run) => db.collaboration_participations_for_run(&run.id)?,
        None => vec![],
    };
    let run_id = run.as_ref().map(|r| r.id.clone());
    // A stopped or missing run has nothing to join, so members get no prompt.
    let active_run = run
        .as_ref()
        .filter(|run| run.status != "stopped")
        .map(|run| run.id.as_str());
    let run_members = match &run_id {
        Some(id) => db.collaboration_run_members(id)?,
        None => vec![],
    };
    let members = room
        .selected_members
        .iter()
        .map(|selected| {
            let participation = participations
                .iter()
                .find(|p| p.session_id == selected.session_id);
            let (wake_status, wake_error) = match (&run, &run_id) {
                (Some(_), Some(_)) => {
                    let member = run_members
                        .iter()
                        .find(|m| m.session_id == selected.session_id);
                    match member {
                        Some(m) => (json!(m.wake_status), m.wake_error.as_ref().map(|e| json!(e))),
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
                "join_prompt": match active_run {
                    Some(run_id) => json!(join_prompt(
                        task_id,
                        &room.id,
                        run_id,
                        &selected.session_id
                    )),
                    None => Value::Null,
                },
                "connection_state": connection_state(participation, now),
                "phase": participation.and_then(|p| p.phase.clone()).map(|p| json!(p)).unwrap_or(Value::Null),
                "phase_since": participation.and_then(|p| p.phase_since.clone()).map(|p| json!(p)).unwrap_or(Value::Null),
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
            let archived = session.is_archived();
            let desktop = !archived
                && matches!(
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
                if archived {
                    Some("已归档的会话不能继续".to_string())
                } else {
                    terminal
                        .reason
                        .or_else(|| Some("该会话没有可用的唤醒方式".to_string()))
                }
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
pub(crate) fn start(db: &Db, params: StartParams) -> CliResult<store::CollaborationStartOutcome> {
    // A replay must be recognised BEFORE capability re-probing: the original
    // start succeeded once, so the same request returns that result even if a
    // member's CLI has since disappeared.
    let replay = db.collaboration_request_exists(&params.request_id)?;
    if !replay {
        // Revalidate the requested wake method against real capability; the
        // user picked from candidates, but the backend never trusts that.
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
    }
    Ok(db.collaboration_start(
        &params.task_id,
        &params.members,
        params.coordinator_session_id.as_deref(),
        params.expected_revision,
        params.initial_message.as_deref(),
        &params.request_id,
    )?)
}

/// Every run-scoped operation answers only the room's CURRENT run: an old run
/// id is stale, a stopped run is over — both are `run_inactive`, never a
/// silent success that would leak into a newer run.
pub(crate) fn guard_current_run(db: &Db, room_id: &str, run_id: &str) -> CliResult<()> {
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
    db.collaboration_stop(
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

/// Heartbeat cadence and the disconnected window (plan §4): renew about every
/// 10s; 30s without a valid renewal means the connection is gone.
pub(crate) const BEAT_SECS: u64 = 10;

pub(crate) struct RoomJoin {
    pub room: String,
    pub run: String,
    pub from_session: String,
    pub holder: String,
    /// The concrete holder instance: an MCP server connection id, or the CLI
    /// resident kind. Reuse is bound to it, not just to the holder kind.
    pub holder_instance: String,
}
pub(crate) fn join(db: &Db, home: &NoEndingHome, p: RoomJoin) -> CliResult<Value> {
    let session = crate::mailbox::actor(db, home, Some(&p.from_session))?;
    guard_current_run(db, &p.room, &p.run)?;
    let cutoff =
        (chrono::Utc::now() - chrono::Duration::seconds(CONNECTED_WINDOW_SECS)).to_rfc3339();
    let outcome = db.collaboration_join(
        &p.room,
        &p.run,
        &session,
        &p.holder,
        &p.holder_instance,
        &cutoff,
    )?;
    Ok(json!({
        "connection_id": outcome.connection_id,
        "session": session,
        // A reusing joiner must not spawn a second resident resource.
        "reused": outcome.reused,
        "role": outcome.role.map(|r| json!(r)).unwrap_or(Value::Null),
        "through_seq": outcome.through_seq,
        "beat_seconds": BEAT_SECS,
        "notice": if outcome.reused {
            "已复用现有参与连接：不要再起第二个监听进程，直接 room wait 取未读。"
        } else {
            "已加入：room wait 取未读；完整读完这一批再 ack 到已读 seq；空闲或 timeout 都继续 wait。"
        },
    }))
}

pub(crate) struct RoomAck {
    pub room: String,
    pub run: String,
    pub from_session: String,
    pub through_seq: i64,
}
pub(crate) fn ack(db: &Db, home: &NoEndingHome, p: RoomAck) -> CliResult<Value> {
    let session = crate::mailbox::actor(db, home, Some(&p.from_session))?;
    guard_current_run(db, &p.room, &p.run)?;
    let through = db.collaboration_ack(&p.room, &p.run, &session, p.through_seq)?;
    Ok(json!({"through_seq": through}))
}

pub(crate) struct RoomLeave {
    pub room: String,
    pub run: String,
    pub from_session: String,
}
/// Leaving is member cleanup: allowed on a stopped run, idempotent, and it
/// never touches another run's participation.
pub(crate) fn leave(db: &Db, home: &NoEndingHome, p: RoomLeave) -> CliResult<Value> {
    let session = crate::mailbox::actor(db, home, Some(&p.from_session))?;
    db.collaboration_leave(&p.room, &p.run, &session)?;
    Ok(json!({"left": true}))
}

pub(crate) struct RoomWait {
    pub room: String,
    pub run: String,
    pub from_session: String,
    pub timeout_seconds: Option<u64>,
    pub limit: usize,
}
/// Wait for the member's unread batch in the current run. Entry marks the
/// participation listening; handing out a batch marks it processing until the
/// next wait. A stopped run ends the wait with status=stopped instead of an
/// error, so every waiter learns about the stop.
pub(crate) fn room_wait(db: &Db, home: &NoEndingHome, p: RoomWait) -> CliResult<Value> {
    if !(1..=100).contains(&p.limit) {
        return Err(CliError::new("invalid_arguments", "limit 需在 1～100 之间"));
    }
    let session = crate::mailbox::actor(db, home, Some(&p.from_session))?;
    let current = db
        .collaboration_current_run(&p.room)?
        .ok_or_else(|| CliError::new("run_inactive", "协作室还没有任何运行"))?;
    if current.id != p.run {
        return Err(CliError::new(
            "run_inactive",
            "协作运行已更新，不能在旧运行上操作",
        ));
    }
    let entry = db
        .collaboration_participations_for_run(&p.run)?
        .into_iter()
        .find(|part| {
            part.session_id == session && part.left_at.is_none() && part.room_id == p.room
        });
    let entry_connection = match entry {
        Some(part) => part.id.clone(),
        None => return Err(CliError::new("join_required", "请先加入协作再等待消息")),
    };
    let deadline = p.timeout_seconds.map(|s| {
        std::time::Instant::now()
            .checked_add(std::time::Duration::from_secs(s))
            .ok_or_else(|| CliError::new("invalid_arguments", "timeout 超出可等待范围"))
    });
    let deadline = deadline.transpose()?;
    // Entry marks listening; a delivered batch marks processing. Both ride on
    // the entry connection so a superseded wait cannot repaint the new one.
    db.collaboration_set_phase(&entry_connection, Some("listening"))?;
    let mut fetch = || {
        db.collaboration_wait_page(&p.room, &p.run, &session, p.limit)
            .map_err(CliError::from)
    };
    room_wait_loop(
        db,
        &p.room,
        &p.run,
        &session,
        &entry_connection,
        &current.status,
        deadline,
        &mut fetch,
    )
}

fn room_wait_loop(
    db: &Db,
    room_id: &str,
    run_id: &str,
    session_id: &str,
    entry_connection: &str,
    entry_status: &str,
    deadline: Option<std::time::Instant>,
    fetch: &mut dyn FnMut() -> CliResult<store::CollaborationMessagesPage>,
) -> CliResult<Value> {
    if entry_status == "stopped" {
        return Ok(wait_value(
            "stopped",
            &store::CollaborationMessagesPage {
                items: vec![],
                next_cursor: None,
                latest_seq: 0,
            },
        ));
    }
    loop {
        crate::platform::cancellation::check().map_err(CliError::from)?;
        // Liveness first: a deleted room, task or session must end the wait.
        let room_alive = db.collaboration_room_by_id(room_id)?.is_some();
        let session_alive = db.get_session(session_id)?.is_some();
        if !room_alive || !session_alive {
            return Err(CliError::new(
                "not_found",
                "协作室或成员会话已删除，等待结束",
            ));
        }
        let Some(current) = db.collaboration_current_run(room_id)? else {
            return Err(CliError::new("not_found", "协作运行不存在，等待结束"));
        };
        if current.id != run_id {
            // A newer run replaced ours; the old wait has nothing to serve.
            return Err(CliError::new(
                "run_inactive",
                "协作运行已更新，不能在旧运行上操作",
            ));
        }
        if current.status == "stopped" {
            db.collaboration_set_phase(entry_connection, None)?;
            return Ok(wait_value(
                "stopped",
                &store::CollaborationMessagesPage {
                    items: vec![],
                    next_cursor: None,
                    latest_seq: 0,
                },
            ));
        }
        // The waiting request owns its entry connection only: a leave, a
        // takeover or a deleted row ends it instead of letting a stale waiter
        // consume mail that belongs to a newer connection.
        match db.collaboration_participation_state(room_id, run_id, session_id)? {
            Some((id, left)) if id == entry_connection && !left => {}
            _ => {
                return Err(CliError::new(
                    "join_required",
                    "参与连接已结束，请重新加入后再等待",
                ))
            }
        }
        let expired = deadline.is_some_and(|d| std::time::Instant::now() >= d);
        let unread = db.collaboration_unread_of(room_id, run_id, session_id)?;
        if unread.count > 0 || expired {
            let page = fetch()?;
            if !page.items.is_empty() {
                db.collaboration_set_phase(entry_connection, Some("processing"))?;
                return Ok(wait_value("messages", &page));
            }
            if expired {
                return Ok(wait_value(
                    "timeout",
                    &store::CollaborationMessagesPage {
                        items: vec![],
                        next_cursor: None,
                        latest_seq: page.latest_seq,
                    },
                ));
            }
            // The batch vanished between the probe and the fetch (acked or
            // deleted elsewhere); that is not a timeout, so keep waiting.
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

fn wait_value(status: &str, page: &store::CollaborationMessagesPage) -> Value {
    // Every wait answer names the next action, so a timeout reads as "this wait
    // ended" instead of "the collaboration ended".
    let notice = match status {
        "messages" if page.next_cursor.is_some() => {
            "本批未读完：用 next_cursor 续读，读完再 ack，然后继续 wait。"
        }
        "messages" => "完整读完本批，按需回复，ack 到已读到的 seq，然后继续 wait。",
        "timeout" => "本次等待结束，协作未结束：继续 room wait，不提交宿主最终回复。",
        _ => "本运行已停止：不再 wait 或接新工作，leave 清理本人参与后收尾。",
    };
    json!({
        "status": status,
        "items": page.items,
        "next_cursor": page.next_cursor,
        "latest_seq": page.latest_seq,
        "notice": notice,
    })
}

/// The MCP holder for one participation: a tokio task renewing the lease
/// across wait requests within the connection. It exits when superseded, the
/// run moves on or stops, the targets are deleted, or the connection ends
/// (the cancellation registry signals it during shutdown). If no runtime is
/// reachable the lease simply expires, which the UI reports honestly.
pub(crate) fn spawn_mcp_heartbeat(
    db: std::sync::Arc<Db>,
    room_id: String,
    run_id: String,
    connection_id: String,
) -> bool {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return false;
    };
    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    crate::platform::cancellation::track_flag(flag.clone());
    runtime.spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(BEAT_SECS)).await;
            if flag.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            match db.collaboration_renew(&connection_id) {
                Ok(store::RenewOutcome::Renewed) => {}
                _ => return,
            }
            match db.collaboration_current_run(&room_id) {
                Ok(Some(current)) if current.id == run_id && current.status != "stopped" => {}
                _ => return,
            }
        }
    });
    true
}

/// The CLI resident listener: renews one connection id every beat and exits
/// when superseded, deleted, the run moves on or stops, or the console is
/// cancelled. It never marks the participation left — a vanished process must
/// look disconnected, not like an intentional leave.
pub(crate) fn listen(db: &Db, room_id: &str, run_id: &str, connection_id: &str) -> i32 {
    loop {
        match db.collaboration_renew(connection_id) {
            Ok(store::RenewOutcome::Renewed) => {}
            Ok(store::RenewOutcome::Unowned) => return 0,
            Ok(store::RenewOutcome::Gone) => return 0,
            Err(_) => return 1,
        }
        match db.collaboration_current_run(room_id) {
            Ok(Some(current)) if current.id == run_id && current.status != "stopped" => {}
            // Stopped run, replaced run, or deleted room: the resident part
            // of this participation ends.
            _ => return 0,
        }
        if crate::platform::cancellation::requested() {
            return 0;
        }
        std::thread::sleep(std::time::Duration::from_secs(BEAT_SECS));
    }
}

#[cfg(test)]
mod room_wait_tests {
    use super::*;
    use crate::storage::collaboration::{CollaborationMessage, CollaborationMessagesPage};
    use std::sync::atomic::{AtomicBool, Ordering};

    struct TempDb(std::path::PathBuf, Db);
    impl TempDb {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("ne-room-wait-{}", crate::storage::new_id()));
            let db = Db::open(&root.join("noending.db")).unwrap();
            db.write()
                .execute_batch(
                    "INSERT INTO workstreams(id,title,created_at,updated_at) VALUES('t','任务','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z');
                     INSERT INTO sessions(id,agent,root_agent_session_id,source_kind,source_path) VALUES
                       ('a','codex','na','jsonl','/a'),('b','codex','nb','jsonl','/b');
                     INSERT INTO session_workstreams(session_id,workstream_id) VALUES ('a','t'),('b','t');",
                )
                .unwrap();
            Self(root, db)
        }
    }
    impl Drop for TempDb {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn page(items: Vec<CollaborationMessage>, latest_seq: i64) -> CollaborationMessagesPage {
        CollaborationMessagesPage {
            items,
            next_cursor: None,
            latest_seq,
        }
    }

    #[test]
    fn room_wait_keeps_waiting_when_the_batch_disappears_before_fetch() {
        let t = TempDb::new();
        let started =
            t.1.collaboration_start(
                "t",
                &[
                    store::SelectedMember {
                        session_id: "a".into(),
                        wake_method: "desktop".into(),
                    },
                    store::SelectedMember {
                        session_id: "b".into(),
                        wake_method: "desktop".into(),
                    },
                ],
                None,
                None,
                None,
                "r1",
            )
            .unwrap();
        let cutoff = (chrono::Utc::now() - chrono::Duration::seconds(30)).to_rfc3339();
        // The author joins before posting.
        t.1.collaboration_join(
            &started.room.id,
            &started.run.id,
            "a",
            "cli",
            "cli-author",
            &cutoff,
        )
        .unwrap();
        let mail =
            t.1.collaboration_send(
                &started.room.id,
                &started.run.id,
                Some("a"),
                "稍纵即逝",
                "m1",
            )
            .unwrap();
        // The wait loop verifies its participation each iteration; it holds
        // the connection id the join produced.
        let participation =
            t.1.collaboration_join(
                &started.room.id,
                &started.run.id,
                "b",
                "cli",
                "cli-resident",
                &cutoff,
            )
            .unwrap();
        let entry_connection = participation.connection_id;
        let deliver = AtomicBool::new(false);
        let mut fetch = || {
            if deliver.load(Ordering::SeqCst) {
                let items = t
                    .1
                    .collaboration_messages_page(&started.room.id, Some(&started.run.id), None, 50)
                    .unwrap()
                    .items;
                Ok(page(
                    items.into_iter().filter(|m| m.id == mail.id).collect(),
                    mail.seq,
                ))
            } else {
                // The probe reported unread mail, but the page came back empty
                // because it was consumed elsewhere in between.
                Ok(page(vec![], mail.seq))
            }
        };
        std::thread::scope(|scope| {
            let handle = scope.spawn(|| {
                room_wait_loop(
                    &t.1,
                    &started.room.id,
                    &started.run.id,
                    "b",
                    &entry_connection,
                    "starting",
                    None,
                    &mut fetch,
                )
            });
            std::thread::sleep(std::time::Duration::from_millis(800));
            assert!(
                !handle.is_finished(),
                "an empty fetched batch must keep waiting instead of returning"
            );
            deliver.store(true, Ordering::SeqCst);
            let value = handle.join().unwrap().unwrap();
            assert_eq!(value["status"], "messages");
            assert_eq!(value["items"][0]["body"], "稍纵即逝");
        });
    }
}
