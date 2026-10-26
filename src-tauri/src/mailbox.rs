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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Wait {
    pub session: String,
    /// Server-side wait bound in seconds; omit to keep waiting until unread
    /// mail, cancellation or a real error. Client tool timeouts stay the
    /// caller's responsibility — pass a bound that fits the host and call again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default = "limit")]
    pub limit: usize,
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
const WAIT_POLL: std::time::Duration = std::time::Duration::from_millis(250);
pub(crate) fn wait(db: &Db, p: Wait) -> CliResult<serde_json::Value> {
    if !(1..=100).contains(&p.limit) {
        return Err(CliError::new("invalid_arguments", "limit 需在 1～100 之间"));
    }
    let deadline = p
        .timeout_seconds
        .map(|s| {
            std::time::Instant::now()
                .checked_add(std::time::Duration::from_secs(s))
                .ok_or_else(|| CliError::new("invalid_arguments", "timeout 超出可等待范围"))
        })
        .transpose()?;
    let mut fetch = || {
        db.mailbox_list(&p.session, "unread", None, p.limit)
            .map_err(CliError::from)
    };
    wait_loop(db, &p.session, deadline, &mut fetch)
}
fn wait_loop(
    db: &Db,
    session: &str,
    deadline: Option<std::time::Instant>,
    fetch: &mut dyn FnMut() -> CliResult<crate::storage::mailbox::MailboxPage>,
) -> CliResult<serde_json::Value> {
    loop {
        crate::platform::cancellation::check().map_err(CliError::from)?;
        let (alive, unread) = db.mailbox_wait_probe(session)?;
        if !alive {
            return Err(recipient_deleted(session));
        }
        let expired = deadline.is_some_and(|d| std::time::Instant::now() >= d);
        if unread > 0 || expired {
            // The same page contract as mailbox_list; wait only decides when to fetch it.
            let page = match fetch() {
                Ok(page) => page,
                Err(error) => {
                    if !db.mailbox_wait_probe(session)?.0 {
                        return Err(recipient_deleted(session));
                    }
                    return Err(error);
                }
            };
            if !page.items.is_empty() {
                return Ok(wait_value("messages", &page));
            }
            if expired {
                return Ok(wait_value("timeout", &page));
            }
            // The batch was read or deleted elsewhere between the probe and the
            // fetch; that is not a timeout, so keep waiting.
        }
        std::thread::sleep(WAIT_POLL);
    }
}
fn recipient_deleted(session: &str) -> CliError {
    CliError::new(
        "not_found",
        format!("session {session} 不存在或已删除，等待结束"),
    )
}
fn wait_value(status: &str, page: &crate::storage::mailbox::MailboxPage) -> serde_json::Value {
    serde_json::json!({
        "status":status,
        "items":page.items,
        "total":page.total,
        "unread":page.unread,
        "next_cursor":page.next_cursor,
    })
}
#[cfg(test)]
mod wait_tests {
    use super::*;
    use crate::storage::mailbox::MailboxPage;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct TempDb(std::path::PathBuf, Option<Db>);
    impl TempDb {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("ne-wait-{}", crate::storage::new_id()));
            let db = Db::open(&root.join("noending.db")).unwrap();
            db.write()
                .execute_batch("INSERT INTO sessions(id,agent,root_agent_session_id,source_kind,source_path) VALUES('a','codex','na','jsonl','/a')")
                .unwrap();
            Self(root, Some(db))
        }
        fn db(&self) -> &Db {
            self.1.as_ref().unwrap()
        }
    }
    impl Drop for TempDb {
        fn drop(&mut self) {
            drop(self.1.take());
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    #[test]
    fn wait_keeps_waiting_when_the_batch_disappears_before_fetch_and_returns_only_real_mail() {
        let t = TempDb::new();
        let mail = t.db().mailbox_send("a", None, "稍纵即逝").unwrap();
        let deliver = AtomicBool::new(false);
        let mut fetch = || {
            if deliver.load(Ordering::SeqCst) {
                let message = t.db().mailbox_read("a", &mail.id).unwrap();
                Ok(MailboxPage {
                    items: vec![message],
                    total: 1,
                    unread: 1,
                    next_cursor: None,
                })
            } else {
                // The probe reported the unread message, but the page came back
                // empty because it was consumed elsewhere in between.
                Ok(MailboxPage {
                    items: vec![],
                    total: 1,
                    unread: 1,
                    next_cursor: None,
                })
            }
        };
        std::thread::scope(|s| {
            let handle = s.spawn(|| wait_loop(t.db(), "a", None, &mut fetch));
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
    #[test]
    fn wait_returns_timeout_only_after_the_deadline_has_actually_passed() {
        let t = TempDb::new();
        t.db().mailbox_send("a", None, "到期").unwrap();
        let mut fetch = || {
            Ok(MailboxPage {
                items: vec![],
                total: 1,
                unread: 1,
                next_cursor: None,
            })
        };
        let value = wait_loop(t.db(), "a", Some(std::time::Instant::now()), &mut fetch).unwrap();
        assert_eq!(value["status"], "timeout");
    }

    #[test]
    fn wait_reports_not_found_when_the_recipient_is_deleted_between_probe_and_fetch() {
        let t = TempDb::new();
        t.db().mailbox_send("a", None, "删除前到达").unwrap();
        let mut fetch = || {
            t.db()
                .write()
                .execute("DELETE FROM sessions WHERE id='a'", [])
                .unwrap();
            t.db()
                .mailbox_list("a", "unread", None, 50)
                .map_err(CliError::from)
        };
        let error = wait_loop(t.db(), "a", None, &mut fetch).unwrap_err();
        assert_eq!(error.code, "not_found");
    }
}
