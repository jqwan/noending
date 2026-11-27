//! Mail for a named assistant. Its archive path determines which sessions can work its inbox.
use crate::{
    error::{other, Result},
    workspace::home::NoEndingHome,
};
use rmcp::schemars::{self, JsonSchema};
use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};
use serde::{Deserialize, Serialize};

use super::{new_id, now, Db};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Filter {
    #[default]
    All,
    Unread,
    Pending,
    InProgress,
    Completed,
}

impl Filter {
    fn predicate(self) -> &'static str {
        match self {
            Self::All => "1",
            Self::Unread => "m.read_at IS NULL",
            Self::Pending => "m.handler_session_id IS NULL",
            Self::InProgress => "m.handler_session_id IS NOT NULL AND m.completed_at IS NULL",
            Self::Completed => "m.completed_at IS NOT NULL",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Read,
    Claim,
    Release,
    Complete,
    Transfer,
}

/// Only the desktop user can transfer a letter to a different conversation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub message_id: String,
    pub revision: i64,
    pub operation: Action,
    pub target_session: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Message {
    pub id: String,
    pub sender_session_id: Option<String>,
    pub body: String,
    pub created_at: String,
    pub read_at: Option<String>,
    pub handler_session_id: Option<String>,
    pub completed_at: Option<String>,
    pub revision: i64,
}

#[derive(Debug, Serialize)]
pub struct Page {
    pub items: Vec<Message>,
    pub unread: i64,
    pub next_cursor: Option<i64>,
}

struct Inbox {
    project_id: String,
    workspace_path_id: String,
    archived: bool,
}

const SELECT: &str = "SELECT m.id,m.sender_session_id,m.body,m.created_at,m.read_at,m.handler_session_id,m.completed_at,m.revision,m.seq
    FROM assistant_mailbox_messages m WHERE m.assistant_project_id=?1";

fn row_message(row: &Row<'_>) -> rusqlite::Result<Message> {
    Ok(Message {
        id: row.get(0)?,
        sender_session_id: row.get(1)?,
        body: row.get(2)?,
        created_at: row.get(3)?,
        read_at: row.get(4)?,
        handler_session_id: row.get(5)?,
        completed_at: row.get(6)?,
        revision: row.get(7)?,
    })
}

fn inbox(conn: &Connection, home: &NoEndingHome, assistant: &str) -> Result<Inbox> {
    let id = uuid::Uuid::parse_str(assistant).map_err(|_| other("智能体 ID 无效"))?;
    let path = crate::assistant::directory(home).join(id.to_string());
    let path_id = crate::workspace::path_identity_of(&path.to_string_lossy())
        .ok_or_else(|| other("智能体档案目录无效"))?;
    conn.query_row(
        "SELECT a.project_id,p.visibility='archived'
         FROM project_assistants a JOIN projects p ON p.id=a.project_id
         WHERE a.workspace_path_id=?1",
        [&path_id],
        |row| {
            Ok(Inbox {
                project_id: row.get(0)?,
                workspace_path_id: path_id.clone(),
                archived: row.get(1)?,
            })
        },
    )
    .optional()?
    .ok_or_else(|| other("智能体不存在"))
}

fn require_conversation(
    conn: &Connection,
    inbox: &Inbox,
    session: &str,
    active: bool,
) -> Result<()> {
    let belongs: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1 AND workspace_path_id=?2 AND (NOT ?3 OR archived_at IS NULL))",
        params![session, inbox.workspace_path_id, active],
        |row| row.get(0),
    )?;
    if !belongs {
        return Err(other("只有从该智能体档案开启的对话才能操作这个信箱"));
    }
    Ok(())
}

fn message(conn: &Connection, inbox: &Inbox, id: &str) -> Result<Message> {
    conn.query_row(
        &format!("{SELECT} AND m.id=?2"),
        params![inbox.project_id, id],
        row_message,
    )
    .optional()?
    .ok_or_else(|| other("来信不存在"))
}

impl Db {
    /// The desktop user passes no session. Agent-facing commands supply the resolved caller.
    pub fn assistant_mailbox_list(
        &self,
        home: &NoEndingHome,
        assistant: &str,
        caller: Option<&str>,
        filter: Filter,
        cursor: Option<i64>,
        limit: usize,
    ) -> Result<Page> {
        if !(1..=100).contains(&limit) || cursor.is_some_and(|cursor| cursor < 1) {
            return Err(other("信箱分页参数无效"));
        }
        let conn = self.read();
        let tx = conn.unchecked_transaction()?;
        let inbox = inbox(&tx, home, assistant)?;
        if let Some(session) = caller {
            require_conversation(&tx, &inbox, session, false)?;
        }
        let unread = tx.query_row(
            "SELECT COUNT(*) FROM assistant_mailbox_messages WHERE assistant_project_id=?1 AND read_at IS NULL",
            [&inbox.project_id], |row| row.get(0),
        )?;
        let mut stmt = tx.prepare(&format!(
            "{SELECT} AND (?2 IS NULL OR m.seq<?2) AND {} ORDER BY m.seq DESC LIMIT ?3",
            filter.predicate(),
        ))?;
        let mut rows = stmt
            .query_map(params![inbox.project_id, cursor, limit as i64 + 1], |row| {
                Ok((row_message(row)?, row.get::<_, i64>(8)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let next_cursor = (rows.len() > limit).then(|| rows[limit - 1].1);
        rows.truncate(limit);
        Ok(Page {
            items: rows.into_iter().map(|(message, _)| message).collect(),
            unread,
            next_cursor,
        })
    }

    pub fn assistant_mailbox_read(
        &self,
        home: &NoEndingHome,
        assistant: &str,
        caller: Option<&str>,
        message_id: &str,
    ) -> Result<Message> {
        let conn = self.read();
        let tx = conn.unchecked_transaction()?;
        let inbox = inbox(&tx, home, assistant)?;
        if let Some(session) = caller {
            require_conversation(&tx, &inbox, session, false)?;
        }
        message(&tx, &inbox, message_id)
    }

    pub fn assistant_mailbox_send(
        &self,
        home: &NoEndingHome,
        assistant: &str,
        sender: Option<&str>,
        body: &str,
    ) -> Result<Message> {
        if body.trim().is_empty() {
            return Err(other("请填写信件正文"));
        }
        let mut conn = self.write();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let inbox = inbox(&tx, home, assistant)?;
        if inbox.archived {
            return Err(other("智能体已归档"));
        }
        if let Some(sender) = sender {
            let sender_path = tx
                .query_row(
                    "SELECT workspace_path_id FROM sessions WHERE id=?1",
                    [sender],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
                .ok_or_else(|| other("发件对话不存在"))?;
            if sender_path.as_deref() == Some(&inbox.workspace_path_id) {
                return Err(other("不能给自己的智能体信箱发信"));
            }
        }
        let id = new_id();
        tx.execute(
            "INSERT INTO assistant_mailbox_messages(id,assistant_project_id,sender_session_id,body,created_at)
             VALUES(?1,?2,?3,?4,?5)",
            params![id, inbox.project_id, sender, body, now()],
        )?;
        let saved = message(&tx, &inbox, &id)?;
        tx.commit()?;
        Ok(saved)
    }

    pub fn assistant_mailbox_change(
        &self,
        home: &NoEndingHome,
        assistant: &str,
        caller: Option<&str>,
        change: &Change,
    ) -> Result<Message> {
        let mut conn = self.write();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let inbox = inbox(&tx, home, assistant)?;
        if let Some(caller) = caller {
            require_conversation(
                &tx,
                &inbox,
                caller,
                !matches!(change.operation, Action::Read | Action::Release),
            )?;
        }
        let current = message(&tx, &inbox, &change.message_id)?;
        if current.revision != change.revision {
            return Err(other("信件已变化，请重新读取"));
        }
        let mut read_at = current.read_at.clone();
        let mut handler = current.handler_session_id.clone();
        let mut completed_at = current.completed_at.clone();
        let own = caller.is_none() || caller == handler.as_deref();
        if !matches!(change.operation, Action::Transfer) && change.target_session.is_some() {
            return Err(other("只有转交时需要指定接手对话"));
        }
        match change.operation {
            Action::Read => {
                read_at.get_or_insert_with(now);
            }
            Action::Claim if !inbox.archived && handler.is_none() => {
                handler = Some(caller.ok_or_else(|| other("请指定接手对话"))?.to_owned());
            }
            Action::Release if own && handler.is_some() && completed_at.is_none() => {
                handler = None;
            }
            Action::Complete if own && handler.is_some() && completed_at.is_none() => {
                completed_at = Some(now());
            }
            Action::Transfer if caller.is_none() && !inbox.archived && completed_at.is_none() => {
                let target = change
                    .target_session
                    .as_deref()
                    .ok_or_else(|| other("请选择接手对话"))?;
                require_conversation(&tx, &inbox, target, true)?;
                handler = Some(target.to_owned());
            }
            _ => return Err(other("当前身份或信件状态不允许此操作")),
        }
        if (&read_at, &handler, &completed_at)
            != (
                &current.read_at,
                &current.handler_session_id,
                &current.completed_at,
            )
        {
            tx.execute(
                "UPDATE assistant_mailbox_messages SET read_at=?3,handler_session_id=?4,completed_at=?5,revision=revision+1
                 WHERE assistant_project_id=?1 AND id=?2",
                params![inbox.project_id, change.message_id, read_at, handler, completed_at],
            )?;
        }
        let saved = message(&tx, &inbox, &change.message_id)?;
        tx.commit()?;
        Ok(saved)
    }
}
