//! Collaboration rooms: one per task, one shared message sequence, per-member
//! read positions and run-scoped participation leases. Config rows survive
//! member deletion only through explicit cleanup; messages never reference
//! sessions by foreign key so the real sender stays after a purge.
use super::{new_id, now, Db};
use crate::error::{other, Result};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

pub const WAKE_TERMINAL: &str = "terminal";
pub const WAKE_DESKTOP: &str = "desktop";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelectedMember {
    pub session_id: String,
    pub wake_method: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct CollaborationRoom {
    pub id: String,
    pub task_id: String,
    pub revision: i64,
    pub selected_members: Vec<SelectedMember>,
    pub coordinator_session_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct CollaborationRun {
    pub id: String,
    pub room_id: String,
    pub status: String,
    pub started_at: String,
    pub activated_at: Option<String>,
    pub stopped_at: Option<String>,
    pub stop_initiator_session_id: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct CollaborationRunMember {
    pub session_id: String,
    pub wake_method: String,
    pub wake_status: String,
    pub wake_error: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct CollaborationMessage {
    pub id: String,
    pub room_id: String,
    pub run_id: String,
    pub seq: i64,
    pub sender_session_id: Option<String>,
    pub kind: String,
    pub body: String,
    pub created_at: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct CollaborationStartOutcome {
    pub room: CollaborationRoom,
    pub run: CollaborationRun,
    pub members: Vec<CollaborationRunMember>,
    /// The messages this start appended: the run's start notice plus the
    /// caller's first user message when one was provided.
    pub appended: Vec<CollaborationMessage>,
}
#[derive(Debug, Clone, Serialize)]
pub struct CollaborationMessagesPage {
    pub items: Vec<CollaborationMessage>,
    pub next_cursor: Option<String>,
    pub latest_seq: i64,
}
#[derive(Debug, Clone, Serialize)]
pub struct CollaborationParticipation {
    pub id: String,
    pub room_id: String,
    pub run_id: String,
    pub session_id: String,
    pub holder: String,
    pub phase: Option<String>,
    pub phase_since: Option<String>,
    pub last_heartbeat_at: String,
    pub left_at: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct CollaborationRead {
    pub session_id: String,
    pub run_id: String,
    pub through_seq: i64,
    pub read_at: String,
}
/// Per-member unread facts: how many current-run messages sit past the
/// member's acknowledged position and when the oldest of them arrived.
#[derive(Debug, Clone, Serialize)]
pub struct CollaborationUnread {
    pub count: i64,
    /// Oldest not-yet-acknowledged message from someone else; own messages
    /// never make a member look unresponsive.
    pub awaiting_since: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct CollaborationCandidatePage {
    pub items: Vec<String>,
    pub next_cursor: Option<String>,
}

const ROOM_SELECT: &str = "SELECT id,task_id,revision,selected_members,coordinator_session_id,created_at,updated_at FROM collaboration_rooms";
type RoomRow = (String, String, i64, String, Option<String>, String, String);
const RUN_SELECT: &str = "SELECT id,room_id,status,started_at,activated_at,stopped_at,stop_initiator_session_id FROM collaboration_runs";
const MESSAGE_SELECT: &str = "SELECT id,room_id,run_id,seq,sender_session_id,kind,body,created_at FROM collaboration_messages";
const RUN_MEMBER_SELECT: &str =
    "SELECT session_id,wake_method,wake_status,wake_error FROM collaboration_run_members";

fn row_room(r: &Row<'_>) -> rusqlite::Result<RoomRow> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
    ))
}
fn parse_room(row: RoomRow) -> Result<CollaborationRoom> {
    let selected_members: Vec<SelectedMember> =
        serde_json::from_str(&row.3).map_err(|e| other(format!("协作室成员配置损坏：{e}")))?;
    Ok(CollaborationRoom {
        id: row.0,
        task_id: row.1,
        revision: row.2,
        selected_members,
        coordinator_session_id: row.4,
        created_at: row.5,
        updated_at: row.6,
    })
}
fn row_run(r: &Row<'_>) -> rusqlite::Result<CollaborationRun> {
    Ok(CollaborationRun {
        id: r.get(0)?,
        room_id: r.get(1)?,
        status: r.get(2)?,
        started_at: r.get(3)?,
        activated_at: r.get(4)?,
        stopped_at: r.get(5)?,
        stop_initiator_session_id: r.get(6)?,
    })
}
fn row_message(r: &Row<'_>) -> rusqlite::Result<CollaborationMessage> {
    Ok(CollaborationMessage {
        id: r.get(0)?,
        room_id: r.get(1)?,
        run_id: r.get(2)?,
        seq: r.get(3)?,
        sender_session_id: r.get(4)?,
        kind: r.get(5)?,
        body: r.get(6)?,
        created_at: r.get(7)?,
    })
}
fn row_run_member(r: &Row<'_>) -> rusqlite::Result<CollaborationRunMember> {
    Ok(CollaborationRunMember {
        session_id: r.get(0)?,
        wake_method: r.get(1)?,
        wake_status: r.get(2)?,
        wake_error: r.get(3)?,
    })
}

fn room_by_task_conn(c: &Connection, task_id: &str) -> Result<Option<CollaborationRoom>> {
    c.query_row(
        &format!("{ROOM_SELECT} WHERE task_id=?1"),
        [task_id],
        row_room,
    )
    .optional()?
    .map(parse_room)
    .transpose()
}
fn current_run_conn(c: &Connection, room_id: &str) -> Result<Option<CollaborationRun>> {
    Ok(c.query_row(
        &format!("{RUN_SELECT} WHERE room_id=?1 ORDER BY started_at DESC, id DESC LIMIT 1"),
        [room_id],
        row_run,
    )
    .optional()?)
}
fn run_conn(c: &Connection, run_id: &str) -> Result<Option<CollaborationRun>> {
    Ok(
        c.query_row(&format!("{RUN_SELECT} WHERE id=?1"), [run_id], row_run)
            .optional()?,
    )
}
fn run_members_conn(c: &Connection, run_id: &str) -> Result<Vec<CollaborationRunMember>> {
    let mut stmt = c.prepare(&format!(
        "{RUN_MEMBER_SELECT} WHERE run_id=?1 ORDER BY session_id"
    ))?;
    let rows = stmt
        .query_map([run_id], row_run_member)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
fn latest_seq_conn(c: &Connection, room_id: &str) -> Result<i64> {
    Ok(c.query_row(
        "SELECT COALESCE(MAX(seq),0) FROM collaboration_messages WHERE room_id=?1",
        [room_id],
        |r| r.get(0),
    )?)
}
fn require_task_open(c: &Connection, task_id: &str) -> Result<()> {
    c.query_row(
        "SELECT visibility FROM workstreams WHERE id=?1",
        [task_id],
        |r| r.get::<_, String>(0),
    )
    .optional()?
    .filter(|visibility| visibility == "normal")
    .ok_or_else(|| other("任务不存在或已归档，不能开启协作"))?;
    Ok(())
}
/// Every member must be a task-associated, unarchived session. Deleted
/// sessions fail by absence; the wake capability itself is checked by the
/// caller against the adapters.
fn require_members(c: &Connection, task_id: &str, session_ids: &[String]) -> Result<()> {
    for id in session_ids {
        let ok = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session_workstreams sw JOIN sessions s ON s.id=sw.session_id WHERE sw.workstream_id=?1 AND sw.session_id=?2 AND s.archived_at IS NULL)",
                params![task_id, id],
                |r| r.get::<_, bool>(0),
            )
            .map_err(crate::error::AppError::from)?;
        if !ok {
            return Err(other(format!("成员 {id} 不属于该任务、已删除或已归档")));
        }
    }
    Ok(())
}
/// One session may take part in only one in-progress collaboration at a time,
/// across every room of this Home.
fn require_sessions_free(c: &Connection, session_ids: &[String]) -> Result<()> {
    for id in session_ids {
        let busy = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM collaboration_runs r JOIN collaboration_run_members m ON m.run_id=r.id WHERE m.session_id=?1 AND r.status IN ('starting','active'))",
                [id],
                |r| r.get::<_, bool>(0),
            )
            .map_err(crate::error::AppError::from)?;
        if busy {
            return Err(other(format!(
                "会话 {id} 已参加进行中的协作，需先停止后再开启"
            )));
        }
    }
    Ok(())
}
fn require_trimmable_body(body: &str) -> Result<()> {
    if body.trim().is_empty() {
        return Err(other("消息不能为空"));
    }
    Ok(())
}
/// A member may only write as itself; user rows carry no sender.
fn require_sender_member(c: &Connection, run_id: &str, sender: &str) -> Result<()> {
    let member = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM collaboration_run_members WHERE run_id=?1 AND session_id=?2)",
            params![run_id, sender],
            |r| r.get::<_, bool>(0),
        )
        .map_err(crate::error::AppError::from)?;
    if !member {
        return Err(other("只有本次运行的成员才能在协作室发信"));
    }
    Ok(())
}
fn append_message_conn(
    c: &Connection,
    room_id: &str,
    run_id: &str,
    sender: Option<&str>,
    kind: &str,
    body: &str,
    request_id: Option<&str>,
) -> Result<CollaborationMessage> {
    require_trimmable_body(body)?;
    let seq = latest_seq_conn(c, room_id)? + 1;
    let id = new_id();
    c.execute(
        "INSERT INTO collaboration_messages(id,room_id,run_id,seq,sender_session_id,kind,body,created_at,request_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![id, room_id, run_id, seq, sender, kind, body, now(), request_id],
    )
    .map_err(crate::error::AppError::from)?;
    Ok(c.query_row(&format!("{MESSAGE_SELECT} WHERE id=?1"), [&id], row_message)?)
}

/// The history cursor encodes its room, run scope and direction, so a cursor
/// from another room, another run scope or an unknown shape is rejected
/// instead of misread. History only pages older messages; the wait flow reads
/// forward from a member's own acknowledged position.
fn encode_cursor(room_id: &str, run_scope: &str, seq: i64) -> String {
    use base64::Engine;
    let payload =
        serde_json::json!({"room":room_id,"run":run_scope,"dir":"back","seq":seq}).to_string();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.as_bytes())
}
fn run_scope(run_id: Option<&str>) -> &str {
    run_id.unwrap_or("all")
}
#[derive(Debug, Clone, Copy, PartialEq)]
enum PageDirection {
    Newest,
    Back,
    Forward,
}
fn decode_cursor(
    room_id: &str,
    expected_scope: &str,
    cursor: &str,
) -> Result<(PageDirection, i64)> {
    use base64::Engine;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor.trim().as_bytes())
        .map_err(|_| other("协作室游标无效"))?;
    let value: serde_json::Value =
        serde_json::from_slice(&raw).map_err(|_| other("协作室游标无效"))?;
    if value["room"].as_str() != Some(room_id) || value["run"].as_str() != Some(expected_scope) {
        return Err(other("协作室游标不属于该房间或运行范围"));
    }
    let direction = match value["dir"].as_str() {
        Some("back") => PageDirection::Back,
        // Forward cursors are minted only by the wait flow for batch
        // continuation; history pagination itself never produces them.
        Some("forward") => PageDirection::Forward,
        _ => return Err(other("协作室游标无效")),
    };
    let seq = value["seq"]
        .as_i64()
        .ok_or_else(|| other("协作室游标无效"))?;
    if seq < 0 {
        return Err(other("协作室游标无效"));
    }
    Ok((direction, seq))
}

impl Db {
    /// Whether this request id already ran, so a replay skips capability
    /// re-probing and returns the original result even if conditions changed.
    pub fn collaboration_request_exists(&self, request_id: &str) -> Result<bool> {
        let c = self.read();
        Ok(c.query_row(
            "SELECT EXISTS(SELECT 1 FROM collaboration_requests WHERE request_id=?1)",
            [request_id],
            |r| r.get::<_, bool>(0),
        )
        .map_err(crate::error::AppError::from)?)
    }
    pub fn collaboration_current_run(&self, room_id: &str) -> Result<Option<CollaborationRun>> {
        let c = self.read();
        current_run_conn(&c, room_id)
    }
    pub fn collaboration_room_by_id(&self, room_id: &str) -> Result<Option<CollaborationRoom>> {
        let c = self.read();
        c.query_row(&format!("{ROOM_SELECT} WHERE id=?1"), [room_id], row_room)
            .optional()?
            .map(parse_room)
            .transpose()
    }
    /// Candidate sessions for a room: task-associated, unarchived, stably
    /// ordered by id with an id cursor.
    pub fn collaboration_candidate_sessions(
        &self,
        task_id: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<CollaborationCandidatePage> {
        let c = self.read();
        // Archived sessions stay listed so the UI can show them greyed out
        // with a reason; start revalidation rejects them regardless.
        let mut stmt = c.prepare(
            "SELECT s.id FROM sessions s WHERE EXISTS(SELECT 1 FROM session_workstreams sw WHERE sw.session_id=s.id AND sw.workstream_id=?1) AND s.id>?2 ORDER BY s.id LIMIT ?3",
        )?;
        let rows = stmt
            .query_map(
                params![task_id, cursor.unwrap_or(""), limit as i64 + 1],
                |r| r.get::<_, String>(0),
            )?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(crate::error::AppError::from)?;
        let more = rows.len() > limit;
        let next_cursor = more.then(|| rows[limit - 1].clone());
        Ok(CollaborationCandidatePage {
            items: rows.into_iter().take(limit).collect(),
            next_cursor,
        })
    }

    pub fn collaboration_room_by_task(
        &self,
        task_id: &str,
    ) -> Result<Option<(CollaborationRoom, Option<CollaborationRun>)>> {
        let c = self.read();
        let Some(room) = room_by_task_conn(&c, task_id)? else {
            return Ok(None);
        };
        let run = current_run_conn(&c, &room.id)?;
        Ok(Some((room, run)))
    }
    pub fn collaboration_run_members(&self, run_id: &str) -> Result<Vec<CollaborationRunMember>> {
        let c = self.read();
        run_members_conn(&c, run_id)
    }

    /// Atomic start: validate the task, membership and exclusivity, persist the
    /// configuration, open the run and append the start notice plus the
    /// caller's first user message. A repeated request id returns the existing
    /// run's current state instead of starting again.
    pub fn collaboration_start(
        &self,
        task_id: &str,
        members: &[SelectedMember],
        coordinator: Option<&str>,
        expected_revision: Option<i64>,
        initial_message: Option<&str>,
        request_id: &str,
    ) -> Result<CollaborationStartOutcome> {
        let mut session_ids: Vec<String> = members.iter().map(|m| m.session_id.clone()).collect();
        session_ids.sort();
        session_ids.dedup();
        if members.len() != session_ids.len() {
            return Err(other("成员列表存在重复会话"));
        }
        if session_ids.len() < 2 {
            return Err(other("协作室至少需要 2 个会话成员"));
        }
        if let Some(coordinator) = coordinator {
            if !session_ids.iter().any(|id| id == coordinator) {
                return Err(other("负责人必须来自所选成员"));
            }
        }
        for member in members {
            if !session_ids.iter().any(|id| id == &member.session_id) {
                return Err(other("成员列表存在重复会话"));
            }
            if member.wake_method != WAKE_TERMINAL && member.wake_method != WAKE_DESKTOP {
                return Err(other("唤醒方式只能是 terminal 或 desktop"));
            }
        }
        let selected = serde_json::to_string(members).map_err(crate::error::AppError::from)?;
        // The fingerprint is the ORIGINAL request: task, members, coordinator
        // and first message — immutable, never the room's current config.
        let fingerprint = serde_json::json!({
            "task": task_id, "members": members,
            "coordinator": coordinator, "message": initial_message,
        })
        .to_string();
        self.tx(|tx| {
            if let Some(run_id) = request_hit_conn(tx, request_id, "start", &fingerprint)? {
                let run = run_conn(tx, &run_id)?.ok_or_else(|| other("协作运行不存在"))?;
                let room = tx
                    .query_row(&format!("{ROOM_SELECT} WHERE id=?1"), [&run.room_id], row_room)
                    .optional()?
                    .ok_or_else(|| other("协作室不存在"))?;
                return Ok(CollaborationStartOutcome {
                    members: run_members_conn(tx, &run_id)?,
                    appended: vec![],
                    room: parse_room(room)?,
                    run,
                });
            }
            require_task_open(tx, task_id)?;
            require_members(tx, task_id, &session_ids)?;
            let existing_room = room_by_task_conn(tx, task_id)?;
            if let Some(room) = &existing_room {
                if expected_revision.is_some_and(|expected| expected != room.revision) {
                    return Err(other(
                        "协作室配置已被其他人修改，请刷新后重试",
                    ));
                }
                if let Some(run) = current_run_conn(tx, &room.id)? {
                    if run.status != "stopped" {
                        return Err(other("该协作室已有进行中的运行，不能重复开启"));
                    }
                }
            }
            // Room-level checks first so restarting the same room reports the
            // room, not a confusing cross-room session conflict.
            require_sessions_free(tx, &session_ids)?;
            let (room_id, revision, created_at) = match &existing_room {
                Some(room) => {
                    tx.execute(
                        "UPDATE collaboration_rooms SET revision=revision+1,selected_members=?2,coordinator_session_id=?3,updated_at=?4 WHERE id=?1",
                        params![room.id, selected, coordinator, now()],
                    )
                    .map_err(crate::error::AppError::from)?;
                    (room.id.clone(), room.revision + 1, room.created_at.clone())
                }
                None => {
                    let id = new_id();
                    let at = now();
                    tx.execute(
                        "INSERT INTO collaboration_rooms(id,task_id,revision,selected_members,coordinator_session_id,created_at,updated_at) VALUES(?1,?2,1,?3,?4,?5,?5)",
                        params![id, task_id, selected, coordinator, at],
                    )
                    .map_err(crate::error::AppError::from)?;
                    (id, 1, at.clone())
                }
            };
            let run_id = new_id();
            let started_at = now();
            tx.execute(
                "INSERT INTO collaboration_runs(id,room_id,status,started_at,start_request_id) VALUES(?1,?2,'starting',?3,?4)",
                params![run_id, room_id, started_at, request_id],
            )
            .map_err(crate::error::AppError::from)?;
            record_request_conn(tx, request_id, "start", &room_id, &run_id, &fingerprint)?;
            for member in members {
                tx.execute(
                    "INSERT INTO collaboration_run_members(run_id,session_id,wake_method) VALUES(?1,?2,?3)",
                    params![run_id, member.session_id, member.wake_method],
                )
                .map_err(crate::error::AppError::from)?;
            }
            let notice_body = serde_json::json!({
                "task_id": task_id, "room_id": room_id, "run_id": run_id,
            })
            .to_string();
            let notice = append_message_conn(
                tx,
                &room_id,
                &run_id,
                None,
                "start",
                &notice_body,
                None,
            )?;
            let mut appended = vec![notice];
            if let Some(body) = initial_message {
                appended.push(append_message_conn(
                    tx,
                    &room_id,
                    &run_id,
                    None,
                    "text",
                    body,
                    None,
                )?);
            }
            let run = CollaborationRun {
                id: run_id.clone(),
                room_id: room_id.clone(),
                status: "starting".into(),
                started_at: started_at.clone(),
                activated_at: None,
                stopped_at: None,
                stop_initiator_session_id: None,
            };
            let room = CollaborationRoom {
                id: room_id,
                task_id: task_id.to_string(),
                revision,
                selected_members: members.to_vec(),
                coordinator_session_id: coordinator.map(str::to_owned),
                created_at,
                updated_at: started_at,
            };
            let members = run_members_conn(tx, &run_id)?;
            Ok(CollaborationStartOutcome {
                room,
                run,
                members,
                appended,
            })
        })
    }

    /// Atomic stop: flip the run status and append the stop notice with the
    /// real initiator (None = desktop user). Repeating a stop request id is a
    /// no-op returning the already-stopped run.
    pub fn collaboration_stop(
        &self,
        room_id: &str,
        run_id: &str,
        initiator: Option<&str>,
        request_id: &str,
    ) -> Result<(CollaborationRun, CollaborationMessage)> {
        let fingerprint = serde_json::json!({
            "room": room_id, "run": run_id, "initiator": initiator,
        })
        .to_string();
        self.tx(|tx| {
            if let Some(existing) = request_hit_conn(tx, request_id, "stop", &fingerprint)? {
                let run = run_conn(tx, &existing)?.ok_or_else(|| other("协作运行不存在"))?;
                let notice = tx
                    .query_row(
                        &format!("{MESSAGE_SELECT} WHERE run_id=?1 AND kind='stop' ORDER BY seq DESC LIMIT 1"),
                        [&existing],
                        row_message,
                    )
                    .optional()?
                    .ok_or_else(|| other("协作运行停止通知缺失"))?;
                return Ok((run, notice));
            }
            let run = run_conn(tx, run_id)?.ok_or_else(|| other("协作运行不存在"))?;
            if run.room_id != room_id {
                return Err(other("协作运行不属于该房间"));
            }
            if run.status == "stopped" {
                return Err(other("协作已停止"));
            }
            // Authority is checked inside the same transaction as the write:
            // a coordinator swap racing this stop cannot slip past the check.
            if let Some(initiator) = initiator {
                let coordinator: Option<String> = tx
                    .query_row(
                        "SELECT coordinator_session_id FROM collaboration_rooms WHERE id=?1",
                        [room_id],
                        |r| r.get::<_, Option<String>>(0),
                    )
                    .map_err(crate::error::AppError::from)?;
                if coordinator.as_deref() != Some(initiator) {
                    return Err(other("只有当前负责人可以停止协作"));
                }
            }
            tx.execute(
                "UPDATE collaboration_runs SET status='stopped',stopped_at=?2,stop_initiator_session_id=?3,stop_request_id=?4 WHERE id=?1",
                params![run_id, now(), initiator, request_id],
            )
            .map_err(crate::error::AppError::from)?;
            record_request_conn(tx, request_id, "stop", room_id, run_id, &fingerprint)?;
            let notice_body = serde_json::json!({
                "room_id": room_id, "run_id": run_id,
            })
            .to_string();
            let notice = append_message_conn(
                tx,
                room_id,
                run_id,
                initiator,
                "stop",
                &notice_body,
                None,
            )?;
            let run = run_conn(tx, run_id)?.ok_or_else(|| other("协作运行不存在"))?;
            Ok((run, notice))
        })
    }

    /// Swap or clear the coordinator. The run fence is enforced by the caller;
    /// here the revision guard keeps concurrent user edits honest and the
    /// request ledger makes a replay idempotent while rejecting a reused id
    /// with different arguments.
    pub fn collaboration_set_coordinator(
        &self,
        room_id: &str,
        run_id: &str,
        coordinator: Option<&str>,
        expected_revision: i64,
        request_id: &str,
    ) -> Result<CollaborationRoom> {
        let fingerprint = serde_json::json!({"coordinator":coordinator}).to_string();
        self.tx(|tx| {
            let row = tx
                .query_row(&format!("{ROOM_SELECT} WHERE id=?1"), [room_id], row_room)
                .optional()?
                .ok_or_else(|| other("协作室不存在"))?;
            let room = parse_room(row)?;
            let replay: Option<String> = tx
                .query_row(
                    "SELECT fingerprint FROM collaboration_requests WHERE request_id=?1",
                    [request_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(crate::error::AppError::from)?;
            if let Some(stored) = replay {
                if stored == fingerprint {
                    // The original swap already happened; answer with the
                    // current room without re-checking the stale revision.
                    return Ok(room);
                }
                return Err(other("请求编号已被用于不同的负责人操作"));
            }
            if room.revision != expected_revision {
                return Err(other("协作室配置已被其他人修改，请刷新后重试"));
            }
            if let Some(session_id) = coordinator {
                if !room
                    .selected_members
                    .iter()
                    .any(|m| m.session_id == session_id)
                {
                    return Err(other("负责人必须来自所选成员"));
                }
            }
            tx.execute(
                "INSERT INTO collaboration_requests(request_id,kind,room_id,run_id,fingerprint,created_at) VALUES(?1,'coordinator',?2,?3,?4,?5)",
                params![request_id, room_id, run_id, fingerprint, now()],
            )
            .map_err(crate::error::AppError::from)?;
            tx.execute(
                "UPDATE collaboration_rooms SET coordinator_session_id=?2,revision=revision+1,updated_at=?3 WHERE id=?1",
                params![room_id, coordinator, now()],
            )
            .map_err(crate::error::AppError::from)?;
            let updated = tx
                .query_row(&format!("{ROOM_SELECT} WHERE id=?1"), [room_id], row_room)
                .optional()?
                .ok_or_else(|| other("协作室不存在"))?;
            parse_room(updated)
        })
    }

    /// Append one message to the current run. Models write only as their own
    /// member session; the user writes through the desktop with no sender.
    /// Repeating a request id returns the stored message unchanged.
    pub fn collaboration_send(
        &self,
        room_id: &str,
        run_id: &str,
        sender: Option<&str>,
        body: &str,
        request_id: &str,
    ) -> Result<CollaborationMessage> {
        let fingerprint = serde_json::json!({
            "room": room_id, "run": run_id, "sender": sender, "body": body,
        })
        .to_string();
        self.tx(|tx| {
            if request_hit_conn(tx, request_id, "send", &fingerprint)?.is_some() {
                // The ledger's run_id is not the message id; the stored message
                // itself carries the request id.
                return tx
                    .query_row(
                        &format!("{MESSAGE_SELECT} WHERE request_id=?1"),
                        [request_id],
                        row_message,
                    )
                    .map_err(crate::error::AppError::from);
            }
            let run = run_conn(tx, run_id)?.ok_or_else(|| other("协作运行不存在"))?;
            if run.room_id != room_id {
                return Err(other("协作运行不属于该房间"));
            }
            if run.status == "stopped" {
                return Err(other("协作已停止，不能继续发信"));
            }
            if let Some(sender) = sender {
                require_sender_member(tx, run_id, sender)?;
            }
            let message =
                append_message_conn(tx, room_id, run_id, sender, "text", body, Some(request_id))?;
            record_request_conn(tx, request_id, "send", room_id, run_id, &fingerprint)?;
            Ok(message)
        })
    }

    /// One page of room history. Without a cursor the page is the newest
    /// `limit` messages in ascending order; `back` pages older messages,
    /// `forward` pages newer ones. Cursors are room-bound.
    pub fn collaboration_messages_page(
        &self,
        room_id: &str,
        run_id: Option<&str>,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<CollaborationMessagesPage> {
        if !(1..=100).contains(&limit) {
            return Err(other("协作室分页大小无效（1～100）"));
        }
        let c = self.read();
        if c.query_row(&format!("{ROOM_SELECT} WHERE id=?1"), [room_id], |r| {
            r.get::<_, String>(0)
        })
        .optional()?
        .is_none()
        {
            return Err(other("协作室不存在"));
        }
        let latest_seq = latest_seq_conn(&c, room_id)?;
        let scope = run_scope(run_id);
        let (direction, boundary) = match cursor {
            None => (PageDirection::Newest, i64::MIN),
            Some(cursor) => decode_cursor(room_id, scope, cursor)?,
        };
        let (comparison, order) = match direction {
            PageDirection::Back => ("<", "DESC"),
            PageDirection::Forward => (">", "ASC"),
            PageDirection::Newest => (">", "DESC"),
        };
        let run_clause = if run_id.is_some() {
            "AND run_id=?3"
        } else {
            "AND (?3 IS NULL OR run_id=?3)"
        };
        let mut stmt = c.prepare(&format!(
            "{MESSAGE_SELECT} WHERE room_id=?1 AND seq {comparison} ?2 {run_clause} ORDER BY seq {order} LIMIT ?4"
        ))?;
        let rows = stmt
            .query_map(
                params![room_id, boundary, run_id, limit as i64 + 1],
                row_message,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(crate::error::AppError::from)?;
        let has_more = rows.len() > limit;
        let mut items: Vec<CollaborationMessage> = rows.into_iter().take(limit).collect();
        if direction != PageDirection::Forward {
            items.reverse();
        }
        let next_cursor = has_more.then(|| {
            let edge = match direction {
                PageDirection::Forward => items.last().map(|m| m.seq).unwrap_or_default(),
                _ => items.first().map(|m| m.seq).unwrap_or_default(),
            };
            encode_cursor(room_id, scope, edge)
        });
        Ok(CollaborationMessagesPage {
            items,
            next_cursor,
            latest_seq,
        })
    }

    /// Explicit read acknowledgement. The sequence must sit inside the room's
    /// current history and may only move forward; old runs are rejected.
    pub fn collaboration_ack(
        &self,
        room_id: &str,
        run_id: &str,
        session_id: &str,
        through_seq: i64,
    ) -> Result<i64> {
        self.tx(|tx| {
            let run = run_conn(tx, run_id)?.ok_or_else(|| other("协作运行不存在"))?;
            if run.room_id != room_id {
                return Err(other("协作运行不属于该房间"));
            }
            if run.status == "stopped" {
                return Err(other("协作已停止，不能更新阅读位置"));
            }
            // Acknowledging is a participant action: an active participation
            // must exist (re-join after leaving to continue).
            let participating = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM collaboration_participations WHERE run_id=?1 AND session_id=?2 AND left_at IS NULL)",
                    params![run_id, session_id],
                    |r| r.get::<_, bool>(0),
                )
                .map_err(crate::error::AppError::from)?;
            if !participating {
                return Err(other("请先加入协作再确认阅读位置"));
            }
            let latest = latest_seq_conn(tx, room_id)?;
            if through_seq < 0 || through_seq > latest {
                return Err(other("确认位置超出协作室消息范围"));
            }
            let current: i64 = tx
                .query_row(
                    "SELECT COALESCE(MAX(seq),0) FROM collaboration_messages WHERE room_id=?1 AND run_id=?2",
                    params![room_id, run_id],
                    |r| r.get(0),
                )
                .map_err(crate::error::AppError::from)?;
            if through_seq > current {
                return Err(other("确认位置超出本次运行的消息范围"));
            }
            tx.execute(
                "INSERT INTO collaboration_reads(room_id,session_id,run_id,through_seq,read_at) VALUES(?1,?2,?3,?4,?5)
                 ON CONFLICT(room_id,session_id,run_id) DO UPDATE SET through_seq=MAX(through_seq,excluded.through_seq),read_at=excluded.read_at",
                params![room_id, session_id, run_id, through_seq, now()],
            )
            .map_err(crate::error::AppError::from)?;
            Ok(through_seq)
        })
    }

    /// Task archiving terminates the task room's in-flight runs: every
    /// non-stopped run flips to stopped with a user-initiated stop notice
    /// (waiters observe it on their next poll), and stays restartable only
    /// through a fresh start on a non-archived task.
    pub fn collaboration_stop_runs_for_archived_task(&self, task_id: &str) -> Result<usize> {
        self.tx(|tx| {
            let Some(room) = room_by_task_conn(tx, task_id)? else {
                return Ok(0);
            };
            let runs: Vec<String> = {
                let mut st = tx.prepare(
                    "SELECT id FROM collaboration_runs WHERE room_id=?1 AND status<>'stopped'",
                )?;
                let rows = st
                    .query_map([&room.id], |r| r.get::<_, String>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(crate::error::AppError::from)?;
                rows
            };
            let mut stopped = 0;
            for run_id in runs {
                let request_id = format!("archive-{}", run_id);
                let fingerprint = serde_json::json!({
                    "room": room.id, "run": run_id, "initiator": serde_json::Value::Null,
                })
                .to_string();
                if request_hit_conn(tx, &request_id, "stop", &fingerprint)?.is_some() {
                    continue;
                }
                tx.execute(
                    "UPDATE collaboration_runs SET status='stopped',stopped_at=?2,stop_initiator_session_id=NULL,stop_request_id=?3 WHERE id=?1 AND status<>'stopped'",
                    params![run_id, now(), request_id],
                )
                .map_err(crate::error::AppError::from)?;
                let notice_body = serde_json::json!({
                    "room_id": room.id, "run_id": run_id, "reason": "task_archived",
                })
                .to_string();
                append_message_conn(tx, &room.id, &run_id, None, "stop", &notice_body, None)?;
                record_request_conn(
                    tx,
                    &request_id,
                    "stop",
                    &room.id,
                    &run_id,
                    &fingerprint,
                )?;
                stopped += 1;
            }
            Ok(stopped)
        })
    }

    /// Record one wake attempt's outcome on the run member row.
    pub fn collaboration_set_wake_status(
        &self,
        run_id: &str,
        session_id: &str,
        status: &str,
        error: Option<&str>,
        request_id: &str,
    ) -> Result<()> {
        self.tx(|tx| {
            tx.execute(
                "UPDATE collaboration_run_members SET wake_status=?3,wake_error=?4,wake_request_id=?5 WHERE run_id=?1 AND session_id=?2",
                params![run_id, session_id, status, error, request_id],
            )
            .map_err(crate::error::AppError::from)?;
            Ok(())
        })
    }

    pub fn collaboration_participations_for_run(
        &self,
        run_id: &str,
    ) -> Result<Vec<CollaborationParticipation>> {
        let c = self.read();
        let mut stmt = c.prepare(
            "SELECT id,room_id,run_id,session_id,holder,phase,phase_since,last_heartbeat_at,left_at FROM collaboration_participations WHERE run_id=?1 ORDER BY session_id",
        )?;
        let rows = stmt
            .query_map([run_id], |r| {
                Ok(CollaborationParticipation {
                    id: r.get(0)?,
                    room_id: r.get(1)?,
                    run_id: r.get(2)?,
                    session_id: r.get(3)?,
                    holder: r.get(4)?,
                    phase: r.get(5)?,
                    phase_since: r.get(6)?,
                    last_heartbeat_at: r.get(7)?,
                    left_at: r.get(8)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(crate::error::AppError::from)?;
        Ok(rows)
    }
    pub fn collaboration_read_of(
        &self,
        room_id: &str,
        session_id: &str,
        run_id: &str,
    ) -> Result<Option<CollaborationRead>> {
        let c = self.read();
        Ok(c.query_row(
            "SELECT session_id,run_id,through_seq,read_at FROM collaboration_reads WHERE room_id=?1 AND session_id=?2 AND run_id=?3",
            params![room_id, session_id, run_id],
            |r| {
                Ok(CollaborationRead {
                    session_id: r.get(0)?,
                    run_id: r.get(1)?,
                    through_seq: r.get(2)?,
                    read_at: r.get(3)?,
                })
            },
        )
        .optional()?)
    }
    /// Unread facts for one member within one run. A member without an ack row
    /// starts before the run's start notice, so the notice itself is the first
    /// thing to process.
    pub fn collaboration_unread_of(
        &self,
        room_id: &str,
        run_id: &str,
        session_id: &str,
    ) -> Result<CollaborationUnread> {
        let c = self.read();
        let through: i64 = c
            .query_row(
                "SELECT through_seq FROM collaboration_reads WHERE room_id=?1 AND session_id=?2 AND run_id=?3",
                params![room_id, session_id, run_id],
                |r| r.get(0),
            )
            .optional()?
            .map(|v: i64| v)
            .unwrap_or_else(|| {
                // The run's minimum seq is the start notice; everything before
                // it (older runs) is never re-processed.
                c.query_row(
                    "SELECT COALESCE(MIN(seq),0) FROM collaboration_messages WHERE room_id=?1 AND run_id=?2",
                    params![room_id, run_id],
                    |r| r.get::<_, i64>(0),
                )
                .map(|min| min - 1)
                .unwrap_or(0)
            })
            .max(0);
        // Independent unread: the member's own messages never count against
        // them, while user messages (no sender) always do.
        let row = c
            .query_row(
                "SELECT COUNT(*),(SELECT MIN(created_at) FROM collaboration_messages WHERE room_id=?1 AND run_id=?2 AND seq>?3 AND (sender_session_id IS NULL OR sender_session_id<>?4)) FROM collaboration_messages WHERE room_id=?1 AND run_id=?2 AND seq>?3 AND (sender_session_id IS NULL OR sender_session_id<>?4)",
                params![room_id, run_id, through, session_id],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .map_err(crate::error::AppError::from)?;
        Ok(CollaborationUnread {
            count: row.0,
            awaiting_since: row.1,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CollaborationJoinOutcome {
    pub connection_id: String,
    /// True when a fresh participation was reused: no new resident resource
    /// may be spawned for it.
    pub reused: bool,
    pub role: Option<String>,
    /// The member's processing start inside this run: the acknowledged
    /// position, or before the run's start notice when none exists.
    pub through_seq: i64,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RenewOutcome {
    Renewed,
    /// The row exists but this connection may not renew it (the member left).
    Unowned,
    /// The row is gone (room/run/session deletion cascaded it away).
    Gone,
}

/// Resolve a request replay. A hit with the same kind and the exact original
/// fingerprint returns the recorded run; any mismatch is a hard conflict —
/// never an answer assembled from current, possibly-changed configuration.
fn request_hit_conn(
    tx: &Connection,
    request_id: &str,
    kind: &str,
    fingerprint: &str,
) -> Result<Option<String>> {
    let row: Option<(String, String, String)> = tx
        .query_row(
            "SELECT kind,run_id,fingerprint FROM collaboration_requests WHERE request_id=?1",
            [request_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(crate::error::AppError::from)?;
    match row {
        None => Ok(None),
        Some((stored_kind, run_id, stored_fingerprint)) => {
            if stored_kind != kind || stored_fingerprint != fingerprint {
                return Err(other("请求编号已被用于不同的操作参数"));
            }
            Ok(Some(run_id))
        }
    }
}
fn record_request_conn(
    tx: &Connection,
    request_id: &str,
    kind: &str,
    room_id: &str,
    run_id: &str,
    fingerprint: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO collaboration_requests(request_id,kind,room_id,run_id,fingerprint,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
        params![request_id, kind, room_id, run_id, fingerprint, now()],
    )
    .map_err(crate::error::AppError::from)?;
    Ok(())
}

/// Run activation: starting → active once at least two members have live
/// participations AND the designated coordinator (when one exists) is among
/// them. Runs without a coordinator rely on the two-member rule alone.
fn maybe_activate_run_conn(tx: &Connection, room_id: &str, run_id: &str) -> Result<()> {
    let starting = tx
        .query_row(
            "SELECT status='starting' FROM collaboration_runs WHERE id=?1",
            [run_id],
            |r| r.get::<_, bool>(0),
        )
        .map_err(crate::error::AppError::from)?;
    if !starting {
        return Ok(());
    }
    let coordinator: Option<String> = tx
        .query_row(
            "SELECT coordinator_session_id FROM collaboration_rooms WHERE id=?1",
            [room_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .map_err(crate::error::AppError::from)?;
    let joined: i64 = tx
        .query_row(
            "SELECT COUNT(DISTINCT p.session_id) FROM collaboration_participations p WHERE p.run_id=?1 AND p.left_at IS NULL AND p.last_heartbeat_at>=?2",
            params![run_id, (chrono::Utc::now() - chrono::Duration::seconds(30)).to_rfc3339()],
            |r| r.get(0),
        )
        .map_err(crate::error::AppError::from)?;
    if joined < 2 {
        return Ok(());
    }
    let coordinator_joined = match &coordinator {
        None => true,
        Some(id) => tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM collaboration_participations WHERE run_id=?1 AND session_id=?2 AND left_at IS NULL)",
                params![run_id, id],
                |r| r.get::<_, bool>(0),
            )
            .map_err(crate::error::AppError::from)?,
    };
    if coordinator_joined {
        tx.execute(
            "UPDATE collaboration_runs SET status='active',activated_at=?2 WHERE id=?1 AND status='starting'",
            params![run_id, now()],
        )
        .map_err(crate::error::AppError::from)?;
    }
    Ok(())
}

fn require_run_member_conn(c: &Connection, run_id: &str, session_id: &str) -> Result<()> {
    let member = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM collaboration_run_members WHERE run_id=?1 AND session_id=?2)",
            params![run_id, session_id],
            |r| r.get::<_, bool>(0),
        )
        .map_err(crate::error::AppError::from)?;
    if !member {
        return Err(other("只有本次运行的成员才能参与协作"));
    }
    Ok(())
}

impl Db {
    /// Atomic ownership handshake. The reuse check and the new connection id
    /// live in one transaction: a fresh (recent heartbeat, not left)
    /// participation is reused as-is — the reusing caller must NOT spawn a
    /// second resident resource — otherwise a new id claims the row and the
    /// previous holder loses its renewal at the next beat.
    pub fn collaboration_join(
        &self,
        room_id: &str,
        run_id: &str,
        session_id: &str,
        holder: &str,
        holder_instance: &str,
        fresh_cutoff: &str,
    ) -> Result<CollaborationJoinOutcome> {
        let connection_id = new_id();
        self.tx(|tx| {
            let run = run_conn(tx, run_id)?.ok_or_else(|| other("协作运行不存在"))?;
            if run.room_id != room_id {
                return Err(other("协作运行不属于该房间"));
            }
            require_run_member_conn(tx, run_id, session_id)?;
            let role: Option<String> = tx
                .query_row(
                    "SELECT coordinator_session_id FROM collaboration_rooms WHERE id=?1",
                    [room_id],
                    |r| r.get::<_, Option<String>>(0),
                )
                .map_err(crate::error::AppError::from)?
                .filter(|coordinator| coordinator == session_id);
            // Only the lease's OWN instance may reuse it: a new MCP
            // connection (or a CLI/MCP takeover) cannot renew someone else's
            // connection id, so it always claims a fresh one.
            let reusable: Option<String> = tx
                .query_row(
                    "SELECT id FROM collaboration_participations WHERE run_id=?1 AND session_id=?2 AND left_at IS NULL AND last_heartbeat_at>=?3 AND holder_instance=?4",
                    params![run_id, session_id, fresh_cutoff, holder_instance],
                    |r| r.get(0),
                )
                .optional()
                .map_err(crate::error::AppError::from)?;
            if let Some(existing_id) = reusable {
                let through_seq: i64 = tx
                    .query_row(
                        "SELECT through_seq FROM collaboration_reads WHERE room_id=?1 AND session_id=?2 AND run_id=?3",
                        params![room_id, session_id, run_id],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(crate::error::AppError::from)?
                    .unwrap_or_else(|| {
                        tx.query_row(
                            "SELECT COALESCE(MIN(seq),0) FROM collaboration_messages WHERE room_id=?1 AND run_id=?2",
                            params![room_id, run_id],
                            |r| r.get::<_, i64>(0),
                        )
                        .map(|min| (min - 1).max(0))
                        .unwrap_or(0)
                    });
                return Ok(CollaborationJoinOutcome {
                    connection_id: existing_id,
                    role,
                    through_seq,
                    reused: true,
                });
            }
            tx.execute(
                "INSERT INTO collaboration_participations(id,room_id,run_id,session_id,holder,holder_instance,last_heartbeat_at) VALUES(?1,?2,?3,?4,?5,?6,?7)
                 ON CONFLICT(run_id,session_id) DO UPDATE SET id=excluded.id,holder=excluded.holder,holder_instance=excluded.holder_instance,phase=NULL,phase_since=NULL,last_heartbeat_at=excluded.last_heartbeat_at,left_at=NULL",
                params![connection_id, room_id, run_id, session_id, holder, holder_instance, now()],
            )
            .map_err(crate::error::AppError::from)?;
            maybe_activate_run_conn(
                tx,
                room_id,
                run_id,
            )?;
            let through_seq: i64 = tx
                .query_row(
                    "SELECT through_seq FROM collaboration_reads WHERE room_id=?1 AND session_id=?2 AND run_id=?3",
                    params![room_id, session_id, run_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(crate::error::AppError::from)?
                .unwrap_or_else(|| {
                    tx.query_row(
                        "SELECT COALESCE(MIN(seq),0) FROM collaboration_messages WHERE room_id=?1 AND run_id=?2",
                        params![room_id, run_id],
                        |r| r.get::<_, i64>(0),
                    )
                    .map(|min| (min - 1).max(0))
                    .unwrap_or(0)
                });
            Ok(CollaborationJoinOutcome {
                connection_id,
                role,
                through_seq,
                reused: false,
            })
        })
    }

    /// One lease renewal. Only the current connection id may renew; anything
    /// else reports supersession or deletion instead of overwriting.
    pub fn collaboration_renew(&self, connection_id: &str) -> Result<RenewOutcome> {
        let mut writer = self.write();
        let tx = writer.transaction()?;
        let renewed = tx
            .execute(
                "UPDATE collaboration_participations SET last_heartbeat_at=?2 WHERE id=?1 AND left_at IS NULL",
                params![connection_id, now()],
            )
            .map_err(crate::error::AppError::from)?;
        tx.commit()?;
        if renewed > 0 {
            return Ok(RenewOutcome::Renewed);
        }
        let c = self.read();
        let exists = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM collaboration_participations WHERE id=?1)",
                [connection_id],
                |r| r.get::<_, bool>(0),
            )
            .map_err(crate::error::AppError::from)?;
        if exists {
            Ok(RenewOutcome::Unowned)
        } else {
            Ok(RenewOutcome::Gone)
        }
    }

    /// Explicit leave: idempotent, only clears this member's participation in
    /// this run and never touches a newer connection of another run.
    pub fn collaboration_leave(&self, room_id: &str, run_id: &str, session_id: &str) -> Result<()> {
        self.tx(|tx| {
            tx.execute(
                "UPDATE collaboration_participations SET left_at=?4,phase=NULL WHERE room_id=?1 AND run_id=?2 AND session_id=?3 AND left_at IS NULL",
                params![room_id, run_id, session_id, now()],
            )
            .map_err(crate::error::AppError::from)?;
            Ok(())
        })
    }

    /// The wait flow's phase marker: listening on entry, processing once a
    /// batch has been handed out. Acknowledgement never changes it.
    pub fn collaboration_set_phase(
        &self,
        room_id: &str,
        run_id: &str,
        session_id: &str,
        phase: Option<&str>,
    ) -> Result<()> {
        self.tx(|tx| {
            tx.execute(
                "UPDATE collaboration_participations SET phase=?4,phase_since=CASE WHEN ?4 IS NULL THEN NULL ELSE ?5 END WHERE room_id=?1 AND run_id=?2 AND session_id=?3 AND left_at IS NULL",
                params![room_id, run_id, session_id, phase, now()],
            )
            .map_err(crate::error::AppError::from)?;
            Ok(())
        })
    }

    /// Forward unread page for one member: messages past the acknowledged
    /// position, ascending, with a forward cursor for the next slice.
    pub fn collaboration_wait_page(
        &self,
        room_id: &str,
        run_id: &str,
        session_id: &str,
        limit: usize,
    ) -> Result<CollaborationMessagesPage> {
        if !(1..=100).contains(&limit) {
            return Err(other("协作室分页大小无效（1～100）"));
        }
        let c = self.read();
        let through: i64 = c
            .query_row(
                "SELECT through_seq FROM collaboration_reads WHERE room_id=?1 AND session_id=?2 AND run_id=?3",
                params![room_id, session_id, run_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(crate::error::AppError::from)?
            .unwrap_or_else(|| {
                c.query_row(
                    "SELECT COALESCE(MIN(seq),0) FROM collaboration_messages WHERE room_id=?1 AND run_id=?2",
                    params![room_id, run_id],
                    |r| r.get::<_, i64>(0),
                )
                .map(|min| (min - 1).max(0))
                .unwrap_or(0)
            });
        let latest_seq = latest_seq_conn(&c, room_id)?;
        let mut stmt = c.prepare(&format!(
            "{MESSAGE_SELECT} WHERE room_id=?1 AND run_id=?2 AND seq>?3 ORDER BY seq ASC LIMIT ?4"
        ))?;
        let rows = stmt
            .query_map(
                params![room_id, run_id, through, limit as i64 + 1],
                row_message,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(crate::error::AppError::from)?;
        let has_more = rows.len() > limit;
        let items: Vec<CollaborationMessage> = rows.into_iter().take(limit).collect();
        let next_cursor = has_more.then(|| {
            let edge = items.last().map(|m| m.seq).unwrap_or_default();
            encode_wait_cursor(room_id, run_id, edge)
        });
        Ok(CollaborationMessagesPage {
            items,
            next_cursor,
            latest_seq,
        })
    }
}
fn encode_wait_cursor(room_id: &str, run_id: &str, seq: i64) -> String {
    use base64::Engine;
    let payload =
        serde_json::json!({"room":room_id,"run":run_id,"dir":"forward","seq":seq}).to_string();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.as_bytes())
}
