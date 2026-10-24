//! Persistent inboxes are separate from ingested conversation history.
use super::{new_id, now, Db};
use crate::error::{other, Result};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailboxMessage {
    pub id: String,
    pub recipient_session_id: String,
    /// None is the desktop user; CLI/MCP must always provide a valid Session.
    pub sender_session_id: Option<String>,
    pub body: String,
    pub created_at: String,
    pub read_at: Option<String>,
    pub revision: i64,
    pub notify_immediately: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct MailboxPage {
    pub items: Vec<MailboxMessage>,
    pub unread: i64,
    pub next_cursor: Option<i64>,
}
const SELECT: &str = "SELECT id,recipient_session_id,sender_session_id,body,created_at,read_at,revision,notify_immediately FROM mailbox_messages";
fn row_at(r: &Row<'_>, offset: usize) -> rusqlite::Result<MailboxMessage> {
    Ok(MailboxMessage {
        id: r.get(offset)?,
        recipient_session_id: r.get(offset + 1)?,
        sender_session_id: r.get(offset + 2)?,
        body: r.get(offset + 3)?,
        created_at: r.get(offset + 4)?,
        read_at: r.get(offset + 5)?,
        revision: r.get(offset + 6)?,
        notify_immediately: r.get(offset + 7)?,
    })
}
fn row(r: &Row<'_>) -> rusqlite::Result<MailboxMessage> {
    row_at(r, 0)
}
fn require_session(c: &Connection, id: &str) -> Result<()> {
    if !c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1)",
        [id],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(other("会话不存在"));
    }
    Ok(())
}
fn message(c: &Connection, session: &str, id: &str) -> Result<MailboxMessage> {
    c.query_row(
        &format!("{SELECT} WHERE recipient_session_id=?1 AND id=?2"),
        params![session, id],
        row,
    )
    .optional()?
    .ok_or_else(|| other("信箱消息不存在"))
}
impl Db {
    pub fn mailbox_send(
        &self,
        session: &str,
        sender: Option<&str>,
        body: &str,
    ) -> Result<MailboxMessage> {
        self.mailbox_send_with_notification(session, sender, body, false)
    }
    pub fn mailbox_send_with_notification(
        &self,
        session: &str,
        sender: Option<&str>,
        body: &str,
        notify_immediately: bool,
    ) -> Result<MailboxMessage> {
        if notify_immediately && sender.is_none() {
            return Err(other("立即通知请求必须来自会话"));
        }
        if body.trim().is_empty() {
            return Err(other("消息不能为空"));
        }
        let mut writer = self.write();
        let tx = writer.transaction()?;
        require_session(&tx, session)?;
        if let Some(sender) = sender {
            require_session(&tx, sender)?;
        }
        let id = new_id();
        tx.execute("INSERT INTO mailbox_messages(id,recipient_session_id,sender_session_id,body,created_at,notify_immediately) VALUES(?1,?2,?3,?4,?5,?6)",params![id,session,sender,body,now(),notify_immediately])?;
        let result = message(&tx, session, &id)?;
        tx.commit()?;
        Ok(result)
    }
    pub fn mailbox_read(&self, session: &str, id: &str) -> Result<MailboxMessage> {
        message(&self.read(), session, id)
    }
    pub fn mailbox_list(
        &self,
        session: &str,
        state: &str,
        cursor: Option<i64>,
        limit: usize,
    ) -> Result<MailboxPage> {
        if !matches!(state, "all" | "unread" | "read")
            || !(1..=100).contains(&limit)
            || cursor.is_some_and(|v| v < 0)
        {
            return Err(other("信箱筛选或分页参数无效"));
        }
        let c = self.read();
        let tx = c.unchecked_transaction()?;
        require_session(&tx, session)?;
        let unread=tx.query_row("SELECT count(*) FROM mailbox_messages WHERE recipient_session_id=?1 AND read_at IS NULL",[session],|r|r.get(0))?;
        let mut stmt=tx.prepare(&format!("SELECT seq,{} WHERE recipient_session_id=?1 AND seq>?2 AND (?3='all' OR (?3='unread' AND read_at IS NULL) OR (?3='read' AND read_at IS NOT NULL)) ORDER BY seq LIMIT ?4",SELECT.strip_prefix("SELECT ").unwrap()))?;
        let rows = stmt
            .query_map(
                params![session, cursor.unwrap_or(0), state, limit + 1],
                |r| Ok((r.get::<_, i64>(0)?, row_at(r, 1)?)),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let next_cursor = (rows.len() > limit).then(|| rows[limit - 1].0);
        let items = rows.into_iter().take(limit).map(|(_, m)| m).collect();
        drop(stmt);
        tx.commit()?;
        Ok(MailboxPage {
            items,
            unread,
            next_cursor,
        })
    }
    pub fn mailbox_unread(&self, session: &str) -> Result<i64> {
        Ok(self.read().query_row("SELECT count(*) FROM mailbox_messages WHERE recipient_session_id=?1 AND read_at IS NULL",[session],|r|r.get(0))?)
    }
    pub fn mailbox_immediate_unread(&self, session: &str) -> Result<Vec<String>> {
        let c = self.read();
        let mut stmt = c.prepare("SELECT id FROM mailbox_messages WHERE recipient_session_id=?1 AND read_at IS NULL AND notify_immediately=1 ORDER BY seq")?;
        let rows = stmt
            .query_map([session], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }
    pub fn mailbox_mark(
        &self,
        session: &str,
        id: &str,
        revision: i64,
        read: bool,
    ) -> Result<MailboxMessage> {
        let mut c = self.write();
        let tx = c.transaction()?;
        let old = message(&tx, session, id)?;
        if old.revision != revision {
            return Err(other("消息已变化，请重新读取"));
        }
        if old.read_at.is_some() != read {
            tx.execute("UPDATE mailbox_messages SET read_at=?3,revision=revision+1 WHERE recipient_session_id=?1 AND id=?2",params![session,id,read.then(now)])?;
        }
        let result = message(&tx, session, id)?;
        tx.commit()?;
        Ok(result)
    }
    /// Only the desktop's notification gate may call these user mutation methods.
    pub fn mailbox_edit(
        &self,
        session: &str,
        id: &str,
        revision: i64,
        body: &str,
    ) -> Result<MailboxMessage> {
        if body.trim().is_empty() {
            return Err(other("消息不能为空"));
        }
        let mut c = self.write();
        let tx = c.transaction()?;
        let old = message(&tx, session, id)?;
        if old.revision != revision || old.read_at.is_some() {
            return Err(other("消息已变化或已读，请刷新"));
        }
        tx.execute("UPDATE mailbox_messages SET body=?3,revision=revision+1 WHERE recipient_session_id=?1 AND id=?2",params![session,id,body])?;
        let result = message(&tx, session, id)?;
        tx.commit()?;
        Ok(result)
    }
    pub fn mailbox_delete(&self, session: &str, id: &str, revision: i64) -> Result<()> {
        let mut c = self.write();
        let tx = c.transaction()?;
        let old = message(&tx, session, id)?;
        if old.revision != revision || old.read_at.is_some() {
            return Err(other("消息已变化或已读，请刷新"));
        }
        tx.execute(
            "DELETE FROM mailbox_messages WHERE recipient_session_id=?1 AND id=?2",
            params![session, id],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn mailbox_references_path(&self, path: &str) -> Result<bool> {
        Ok(self.read().query_row(
            "SELECT EXISTS(SELECT 1 FROM mailbox_messages WHERE instr(body,?1)>0)",
            [path],
            |r| r.get(0),
        )?)
    }
    pub fn mailbox_frontier(&self) -> Result<i64> {
        Ok(self.read().query_row(
            "SELECT COALESCE(MAX(seq),0) FROM mailbox_messages",
            [],
            |r| r.get(0),
        )?)
    }
    pub fn mailbox_arrivals(&self, after: i64) -> Result<Vec<(i64, String, String, bool)>> {
        let c = self.read();
        let mut stmt=c.prepare("SELECT seq,recipient_session_id,id,notify_immediately FROM mailbox_messages WHERE seq>?1 ORDER BY seq LIMIT 256")?;
        let rows = stmt
            .query_map([after], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }
}
