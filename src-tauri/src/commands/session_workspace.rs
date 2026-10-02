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
const DETAIL_MESSAGE_PREVIEW: i64 = 10;

/// Page size for the conversation reader: what one upward scroll asks for.
const MESSAGE_PAGE_DEFAULT: i64 = 60;
const MESSAGE_PAGE_MAX: i64 = 200;

/// One member of the execution graph, as the detail page shows it: the
/// member facts, its own stats snapshot when one exists, and a fresh verdict
/// on ITS source — a child or side transcript goes away on its own schedule,
/// so the row that names it has to say when it is gone.
#[derive(Serialize)]
pub struct SessionMemberView {
    #[serde(flatten)]
    pub member: SessionMember,
    pub stats: Option<SessionMemberStats>,
    pub source_status: SourceAvailability,
}

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
    /// The execution graph: root / children / sides. Execution info,
    /// never other user-visible Sessions.
    pub members: Vec<SessionMemberView>,
    /// Query-time aggregate over the whole graph (no cache to drift).
    pub stats: crate::storage::SessionAggregateStats,
    /// The two frontiers the detail page shows: messages ingested, and how
    /// far Context processing has consumed them.
    pub ingested_message_sequence: i64,
    pub processed_message_sequence: i64,
    /// Fresh (detail-load time) verdict on the ROOT source.
    /// `missing` means the adapter confirmed it absent; every other failure
    /// stays `unavailable`.
    pub root_source_status: SourceAvailability,
    /// Resume eligibility: active session + a present root source.
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
        let messages = db.recent_messages(&session_id, DETAIL_MESSAGE_PREVIEW)?;
        // A Logical Session is one Agent's graph, so one adapter covers every
        // member — root, child and side alike.
        let adapter = crate::adapters::adapter_for(session.agent);
        let members = db.members_for_session(&session_id)?;
        let member_views = members
            .iter()
            .map(|m| {
                Ok(SessionMemberView {
                    member: m.clone(),
                    stats: db.get_member_stats(&m.id)?,
                    source_status: adapter.inspect_member_source(m)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let stats = db.aggregate_session_stats(&session_id)?;
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
        // The root's verdict is already in `member_views`; reading it there keeps
        // the root from being inspected twice for the same page.
        let root_source_status = member_views
            .iter()
            .find(|v| v.member.relation == SessionMemberRelation::Root)
            .map(|v| v.source_status)
            .unwrap_or(SourceAvailability::Unavailable);
        let can_resume = !session.is_trashed() && root_source_status == SourceAvailability::Present;
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
            members: member_views,
            stats,
            ingested_message_sequence,
            processed_message_sequence,
            root_source_status,
            can_resume,
            forked_from,
        })
    })
}

/// Locate the current Root transcript from the stored Session identity.
#[tauri::command]
pub fn reveal_session_source(state: State<AppState>, session_id: String) -> Result<()> {
    let root = state
        .db
        .root_member_for_session(&session_id)?
        .ok_or_else(|| other("源会话路径不可用"))?;
    crate::platform::paths::reveal_file(std::path::Path::new(&root.source_path))
}

/// Locate ONE member's transcript: the child/side rows in the execution graph
/// reveal their own source. The member is resolved against the session the
/// page is already showing, so the webview never gets a free-form path reveal.
#[tauri::command]
pub fn reveal_session_member_source(
    state: State<AppState>,
    session_id: String,
    member_id: String,
) -> Result<()> {
    let member = state
        .db
        .members_for_session(&session_id)?
        .into_iter()
        .find(|m| m.id == member_id)
        .ok_or_else(|| other("会话成员不存在"))?;
    crate::platform::paths::reveal_file(std::path::Path::new(&member.source_path))
}

/// One message of a window: the message plus the projection ordinal that puts
/// it in the conversation. The reader orders by `ordinal`, so the rendered
/// order never depends on the order pages were fetched or merged in.
#[derive(Serialize)]
pub struct SessionWindowMessage {
    pub ordinal: i64,
    #[serde(flatten)]
    pub message: SessionMessage,
}

/// One page of a Session's Conversation, read backward from the newest message.
#[derive(Serialize)]
pub struct SessionMessageWindow {
    pub messages: Vec<SessionWindowMessage>,
    /// The fact generation the page was read from. A caller paging upward that
    /// sees it change must reload from the tail: the conversation was rewritten.
    pub generation: i64,
    /// How many messages the CURRENT conversation holds.
    pub total: i64,
    /// Cursor for the next, older page; `None` means the beginning was reached.
    pub next_before_ordinal: Option<i64>,
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
                })
                .collect(),
            generation: window.generation,
            total: window.total,
            next_before_ordinal: window.next_before_ordinal,
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

// ---------------- Ingestion diagnostics ----------------

/// The Settings → Ingestion Diagnostics list: repeat offenders only by
/// default (`observation_count >= 2`). Diagnostics are NOT sessions — they
/// have no Owner, no Resume, no Trash, no Context.
#[tauri::command]
pub fn list_ingestion_diagnostics(
    state: State<AppState>,
    min_observations: Option<i64>,
) -> Result<Vec<IngestionDiagnostic>> {
    with_db(&state, |db| {
        db.prune_missing_ingestion_diagnostics()?;
        db.list_ingestion_diagnostics(min_observations.unwrap_or(2))
    })
}

// ---------------- Usage panel ----------------

/// 全库用量汇总：成员快照提供累计计数和 Token；用量账本提供模型归因、
/// 请求数、时间序列和调用类别。纯本地读取。
#[tauri::command]
pub fn get_usage_overview(state: State<AppState>) -> Result<crate::storage::UsageOverview> {
    with_db(&state, |db| db.usage_overview())
}
