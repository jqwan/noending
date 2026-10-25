//! Mailbox data contracts shared by CLI, MCP and desktop. No PTY delivery state.
use crate::{
    cli::{CliError, CliResult},
    storage::Db,
    workspace::home::NoEndingHome,
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct List {
    pub session: String,
    #[serde(default = "unread")]
    pub state: String,
    pub cursor: Option<i64>,
    #[serde(default = "limit")]
    pub limit: usize,
}
fn unread() -> String {
    "unread".into()
}
fn limit() -> usize {
    50
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Read {
    pub session: String,
    pub message_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Send {
    pub session: String,
    pub message: String,
    /// Explicit sender Session ID; an inherited terminal binding takes precedence.
    pub from_session: String,
    /// Request a merged unread reminder, also allowed in Working when automatic notifications are enabled.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub notify_immediately: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Mark {
    pub session: String,
    pub message_id: String,
    pub revision: i64,
    pub state: String,
    pub from_session: Option<String>,
}

pub(crate) fn actor(db: &Db, home: &NoEndingHome, claimed: Option<&str>) -> CliResult<String> {
    let terminal = std::env::var(crate::session_control::CALLER_TERMINAL_ENV)
        .ok()
        .filter(|v| !v.is_empty());
    let session = if terminal.is_some() {
        crate::session_control::request(
            home,
            crate::session_control::ControlCommand::Identity { claimed: None },
            None,
        )?["session_id"]
            .as_str()
            .map(str::to_owned)
    } else {
        claimed.map(str::to_owned)
    }
    .ok_or_else(|| {
        CliError::new(
            "sender_unavailable",
            "需要可识别的发件会话 ID；请先解析原生会话身份",
        )
    })?;
    if db.get_session(&session)?.is_none() {
        return Err(CliError::new("sender_unavailable", "来源会话不存在"));
    }
    Ok(session)
}
pub(crate) fn send(db: &Db, home: &NoEndingHome, p: Send) -> CliResult<serde_json::Value> {
    let sender = actor(db, home, Some(&p.from_session))?;
    let m = db.mailbox_send_with_notification(
        &p.session,
        Some(&sender),
        &p.message,
        p.notify_immediately,
    )?;
    // Receipts never retain a second copy of editable/deletable message content.
    Ok(
        serde_json::json!({"message_id":m.id,"session":m.recipient_session_id,"created_at":m.created_at}),
    )
}
pub(crate) fn mark(db: &Db, home: &NoEndingHome, p: Mark) -> CliResult<serde_json::Value> {
    let current = actor(db, home, p.from_session.as_deref())?;
    if p.from_session.as_deref().is_some_and(|id| id != current) {
        return Err(CliError::new(
            "sender_identity_changed",
            "来源会话与当前终端绑定不一致",
        ));
    }
    if current != p.session {
        return Err(CliError::new("identity_mismatch", "只能标记自己信箱的消息"));
    }
    let read = match p.state.as_str() {
        "read" => true,
        "unread" => false,
        _ => {
            return Err(CliError::new(
                "invalid_arguments",
                "state 只能为 read 或 unread",
            ))
        }
    };
    let m = db.mailbox_mark(&p.session, &p.message_id, p.revision, read)?;
    Ok(serde_json::json!({"message_id":m.id,"revision":m.revision,"read_at":m.read_at}))
}
