//! Session commands: listing, detail, and Owner Workstream assignment.
//!
//! These are thin: they take the lock, map the wire shape and hand over to
//! `workspace::session`, which owns the rules.
//!
//! A Session has at most ONE Owner Workstream. Setting it writes
//! `sessions.owner_workstream_id` and nothing else — it never touches the
//! Workstream's ordered path list, the Session's cwd or its Project.
//!
//! The detail shape is the Logical Session view: the
//! conversation (`session_messages` only — the UI never sees compact/system/
//! sidechain kinds), the execution graph as MEMBERS (not other Sessions),
//! query-time aggregate stats, the two frontiers, and the source/lifecycle
//! facts (fresh root source status, resume and permanent-delete eligibility).
//! There is no parent/children pair any more: members are execution info, not
//! navigable Sessions.

use serde::Serialize;
use tauri::State;

use crate::domain::*;
use crate::error::{other, Result};

use super::{with_db, AppState};

// ---------------- Sessions ----------------

/// How many Conversation messages the detail page previews. The conversation
/// itself is read through `get_session_messages`, never wholesale.
const DETAIL_MESSAGE_PREVIEW: i64 = 5;

/// Page size for the conversation reader: what one upward scroll asks for.
const MESSAGE_PAGE_DEFAULT: i64 = 60;
const MESSAGE_PAGE_MAX: i64 = 200;
/// 一轮中间回复的展开上限：单轮通常几十条，超过就截断并如实告知。
const TURN_INTERMEDIATES_MAX: i64 = 400;

#[derive(Serialize)]
pub struct SessionDetail {
    pub session: Session,
    /// The Conversation: root user/assistant messages only.
    pub messages: Vec<SessionMessage>,
    /// The one Workstream this Session belongs to, or `None`.
    pub owner_workstream: Option<Workstream>,
    /// Session Detail shows the WorkspacePath and the Project behind it,
    /// read-only. They travel as display strings because `workspace_path_id`
    /// alone would make the UI join a table it has no command for — and the
    /// Project shown here is derived through that path, never picked by a user.
    pub workspace_path: Option<SessionWorkspacePath>,
    /// The two frontiers the detail page shows: messages ingested, and how
    /// far Context processing has consumed them.
    pub ingested_message_sequence: i64,
    pub processed_message_sequence: i64,
    /// Fresh (detail-load time) verdict on the session's source.
    /// `missing` means the adapter confirmed it absent; every other failure
    /// stays `unavailable`.
    pub source_status: SourceAvailability,
    /// Resume eligibility: active session + a present source.
    pub can_resume: bool,
    /// when this session is a fork, its source Session summary.
    pub forked_from: Option<Session>,
}

#[derive(Serialize)]
pub struct SessionWorkspacePath {
    pub id: String,
    pub canonical_path: String,
    pub exists: bool,
    pub project_id: String,
    pub project_name: String,
}

// scope: active (default) | trash | all —; the recycle bin passes "trash".
#[tauri::command]
pub fn list_sessions(
    state: State<AppState>,
    project_id: Option<String>,
    agent: Option<String>,
    scope: Option<String>,
) -> Result<Vec<Session>> {
    let agent = agent.and_then(|a| Agent::parse(&a));
    let scope = scope
        .as_deref()
        .map(SessionListScope::parse)
        .unwrap_or_default();
    with_db(&state, |db| {
        db.list_sessions(crate::storage::SessionFilter {
            project_id,
            agent,
            scope,
        })
    })
}

#[tauri::command]
pub fn get_session_detail(state: State<AppState>, session_id: String) -> Result<SessionDetail> {
    with_db(&state, |db| {
        let session = db
            .get_session(&session_id)?
            .ok_or_else(|| other("Session 不存在"))?;
        // 详情页预览只保留对话的骨架：用户消息 + 每轮的最终回复。
        let messages = db.recent_turn_messages(&session_id, DETAIL_MESSAGE_PREVIEW)?;
        let adapter = crate::adapters::adapter_for(session.agent);
        let source_status = adapter.inspect_session_source(&session)?;
        // The two frontiers the detail page shows: messages ingested, and how
        // far Context processing has consumed them.
        let ingested_message_sequence = db.ingested_message_sequence(&session_id)?;
        let processed_message_sequence = db
            .get_session_context(&session_id)?
            .map(|c| c.processed_through_seq)
            .unwrap_or(0);
        let owner_workstream = match session.owner_workstream_id.as_deref() {
            Some(id) => db.get_workstream(id)?,
            None => None,
        };
        let can_resume = !session.is_trashed() && source_status == SourceAvailability::Present;
        let workspace_path = match session.workspace_path_id.as_deref() {
            Some(id) => db.get_workspace_path(id)?.map(|wp| {
                Ok::<_, crate::error::AppError>(SessionWorkspacePath {
                    id: wp.id.clone(),
                    canonical_path: wp.canonical_path.clone(),
                    exists: wp.exists,
                    project_name: db
                        .get_project(&wp.project_id)?
                        .map(|p| p.name)
                        .unwrap_or_default(),
                    project_id: wp.project_id.clone(),
                })
            }),
            None => None,
        };
        // the fork provenance, resolved to a summary when the source
        // session still exists locally.
        let forked_from = match session.forked_from_session_id.as_deref() {
            Some(id) => db.get_session(id)?,
            None => None,
        };
        Ok(SessionDetail {
            session,
            messages,
            owner_workstream,
            workspace_path: workspace_path.transpose()?,
            ingested_message_sequence,
            processed_message_sequence,
            source_status,
            can_resume,
            forked_from,
        })
    })
}

/// Locate the session's source from the stored identity.
#[tauri::command]
pub fn reveal_session_source(state: State<AppState>, session_id: String) -> Result<()> {
    let session = state
        .db
        .get_session(&session_id)?
        .ok_or_else(|| other("源会话路径不可用"))?;
    crate::platform::paths::reveal_file(std::path::Path::new(&session.source_path))
}

/// One message of a window: the message plus the projection ordinal that puts
/// it in the conversation. The reader orders by `ordinal`, so the rendered
/// order never depends on the order pages were fetched or merged in.
#[derive(Serialize)]
pub struct SessionWindowMessage {
    pub ordinal: i64,
    #[serde(flatten)]
    pub message: SessionMessage,
    /// Final replies only: the turn's collapsible intermediate block. Absent
    /// when there is nothing to collapse.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn: Option<SessionTurnSummary>,
}

/// The turn a final reply closes, as its reader-facing summary.
#[derive(Serialize)]
pub struct SessionTurnSummary {
    /// The turn's user message — the exclusive lower bound of the expand
    /// range. `0` means the turn opens at the conversation's beginning.
    pub boundary_ordinal: i64,
    /// That user message's ts; the block header derives a duration from it.
    pub boundary_ts: Option<String>,
    /// How many intermediate messages the range holds.
    pub count: i64,
}

/// One page of a Session's Conversation, read backward from the newest message.
#[derive(Serialize)]
pub struct SessionMessageWindow {
    pub messages: Vec<SessionWindowMessage>,
    /// The fact generation the page was read from. A caller paging upward that
    /// sees it change must reload from the tail: the conversation was rewritten.
    pub generation: i64,
    /// How many messages the conversation holds in total（含中间回复）——头部
    /// 的「共 N 条」拿它计数；分页本身按骨架走。
    pub total: i64,
    /// The max projection ordinal of the conversation："是否已在尾部"
    /// 拿它比，不拿 total 比。
    pub tail_ordinal: i64,
    /// 上方还有多少条骨架消息没加载（「加载更早」的计数）。
    pub remaining: i64,
    /// Cursor for the next, older page; `None` means the beginning was reached.
    pub next_before_ordinal: Option<i64>,
}

/// One turn's expanded intermediates.
#[derive(Serialize)]
pub struct TurnIntermediates {
    pub messages: Vec<SessionMessage>,
    /// The range exceeded the cap and was cut short.
    pub truncated: bool,
}

/// One mark on the conversation's navigation rail: a USER message's position.
#[derive(Serialize)]
pub struct SessionMessageMark {
    pub ordinal: i64,
    pub preview: String,
}

/// The Conversation as pages. `before_ordinal = None` is the newest page, and
/// an older page is fetched by passing the previous page's cursor back.
/// `after_ordinal` reads the other direction — the messages NEWER than it —
/// which is how a reader that jumped into the middle keeps going forward.
/// 一页按「用户消息 + 每轮最终回复」计（骨架），代理的中间输出不占加载配额，
/// 由每条最终回复的 `turn` 摘要标注成可展开的块。
#[tauri::command]
pub fn get_session_messages(
    state: State<AppState>,
    session_id: String,
    before_ordinal: Option<i64>,
    after_ordinal: Option<i64>,
    limit: Option<i64>,
) -> Result<SessionMessageWindow> {
    let limit = limit
        .unwrap_or(MESSAGE_PAGE_DEFAULT)
        .clamp(1, MESSAGE_PAGE_MAX);
    with_db(&state, |db| {
        let window = match after_ordinal {
            Some(after) => db.newer_window(&session_id, after, limit)?,
            None => db.message_window(&session_id, before_ordinal, limit)?,
        };
        Ok(SessionMessageWindow {
            messages: window
                .messages
                .into_iter()
                .map(|row| SessionWindowMessage {
                    ordinal: row.ordinal,
                    message: row.message,
                    turn: row.turn.map(|turn| SessionTurnSummary {
                        boundary_ordinal: turn.boundary_ordinal,
                        boundary_ts: turn.boundary_ts,
                        count: turn.count,
                    }),
                })
                .collect(),
            generation: window.generation,
            total: window.total,
            tail_ordinal: window.tail_ordinal,
            remaining: window.remaining,
            next_before_ordinal: window.next_before_ordinal,
        })
    })
}

/// 一轮展开后的中间回复：用户消息（`after_ordinal`）与最终回复（`before_ordinal`）
/// 之间的全部消息，旧→新。中间回复块展开时取。
#[tauri::command]
pub fn get_turn_intermediates(
    state: State<AppState>,
    session_id: String,
    after_ordinal: i64,
    before_ordinal: i64,
) -> Result<TurnIntermediates> {
    with_db(&state, |db| {
        let (messages, truncated) = db.turn_intermediates(
            &session_id,
            after_ordinal,
            before_ordinal,
            TURN_INTERMEDIATES_MAX,
        )?;
        Ok(TurnIntermediates {
            messages,
            truncated,
        })
    })
}

/// Where the USER messages sit in the CURRENT conversation, in order: what the
/// navigation rail draws and jumps to.
#[tauri::command]
pub fn get_session_user_message_marks(
    state: State<AppState>,
    session_id: String,
) -> Result<Vec<SessionMessageMark>> {
    with_db(&state, |db| {
        Ok(db
            .user_message_marks(&session_id)?
            .into_iter()
            .map(|mark| SessionMessageMark {
                ordinal: mark.ordinal,
                preview: mark.preview,
            })
            .collect())
    })
}

/// Set (or clear) a Session's Owner Workstream.
///
/// `workstream_id = None` clears ownership. This is the ONLY write path for
/// semantic ownership; it changes one column and nothing else.
#[tauri::command]
pub fn set_session_owner_workstream(
    state: State<AppState>,
    session_id: String,
    workstream_id: Option<String>,
) -> Result<Session> {
    with_db(&state, |db| {
        crate::workspace::session::set_session_owner(db, &session_id, workstream_id.as_deref())
    })
}
