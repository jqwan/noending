//! Best-effort mailbox reminders. No delivery receipts, retries or source reads.
use super::{activity::ActivityState, messaging, TerminalRegistry};
use crate::{
    error::{other, Result},
    storage::Db,
};
use serde::Serialize;
use std::{collections::HashMap, io::Write, sync::atomic::Ordering};
use tauri::{Emitter, Manager};

#[derive(Default)]
pub(super) struct NotificationState {
    pub enabled: bool,
    pub writing: bool,
    pub input_revision: u64,
    pending: bool,
    manual_check: bool,
    immediate_ids: std::collections::HashSet<String>,
    pub error: Option<String>,
}
impl NotificationState {
    pub fn reset(&mut self) {
        self.enabled = false;
        self.pending = false;
        self.manual_check = false;
        self.immediate_ids.clear();
        self.input_revision += 1;
    }
    pub fn observe(&mut self, state: ActivityState) {
        if matches!(state, ActivityState::Failed | ActivityState::Interrupted)
            && (self.enabled || self.pending)
        {
            self.enabled = false;
            self.pending = false;
            self.immediate_ids.clear();
            self.input_revision += 1;
        }
    }
}
#[derive(Clone, Serialize)]
pub struct NotificationSnapshot {
    pub terminal_id: String,
    pub session_id: Option<String>,
    pub enabled: bool,
    pub writing: bool,
    pub input_revision: u64,
    pub identity_revision: u64,
    pub error: Option<String>,
    pub revision: u64,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReminderKind {
    Automatic,
    Manual,
}
impl ReminderKind {
    fn eligible(self, state: ActivityState, immediate: bool) -> bool {
        use ActivityState::*;
        matches!(state, Idle | Completed | Unknown)
            || match self {
                Self::Manual => matches!(state, Working | Interrupted | Failed),
                Self::Automatic => immediate && state == Working,
            }
    }
}
fn text(count: i64) -> String {
    format!("noending信箱中有{count}条未读信息，请查收")
}
impl TerminalRegistry {
    pub fn notification_state(&self, id: &str) -> Result<NotificationSnapshot> {
        let records = self.records.lock().map_err(|_| other("终端不可用"))?;
        let r = records
            .get(id)
            .ok_or_else(|| other("终端不存在"))?
            .lock()
            .map_err(|_| other("终端不可用"))?;
        Ok(NotificationSnapshot {
            terminal_id: id.into(),
            session_id: r.summary.session_id.clone(),
            enabled: r.notification.enabled,
            writing: r.notification.writing,
            input_revision: r.notification.input_revision,
            identity_revision: r.summary.identity_revision,
            error: r.notification.error.clone(),
            revision: r.notification.input_revision,
        })
    }
    pub fn set_auto_notifications(
        &self,
        db: &Db,
        id: &str,
        identity: u64,
        enabled: bool,
    ) -> Result<NotificationSnapshot> {
        {
            let records = self.records.lock().map_err(|_| other("终端不可用"))?;
            let mut r = records
                .get(id)
                .ok_or_else(|| other("终端不存在"))?
                .lock()
                .map_err(|_| other("终端不可用"))?;
            if !r.summary.live
                || r.summary.identity_revision != identity
                || r.summary.session_id.is_none()
            {
                return Err(other("终端身份已变化或尚未绑定"));
            }
            if enabled
                && matches!(
                    r.summary.activity.state,
                    ActivityState::Failed | ActivityState::Interrupted
                )
            {
                return Err(other("Agent 状态尚不允许自动通知"));
            }
            if !crate::adapters::adapter_for(r.summary.agent).has_terminal_cli() {
                return Err(other("此 Agent 不支持终端通知"));
            }
            let session = r.summary.session_id.as_deref().unwrap();
            let immediate = if enabled {
                db.mailbox_immediate_unread(session)?
            } else {
                Vec::new()
            };
            let pending = enabled && db.mailbox_unread(session)? > 0;
            r.notification.immediate_ids = immediate.into_iter().collect();
            r.notification.enabled = enabled;
            r.notification.pending = pending;
            r.notification.error = None;
            r.notification.input_revision += 1;
        }
        self.emit_notifications();
        self.kick_notifications(id);
        self.notification_state(id)
    }
    pub fn initialize_mailboxes(&self, db: &Db) -> Result<()> {
        *self.mailbox_frontier.lock().unwrap() = db.mailbox_frontier()?;
        Ok(())
    }
    pub(crate) fn kick_notifications(&self, _terminal: &str) {
        self.notification_dirty.store(true, Ordering::Release);
        let Some(app) = self.app.clone() else { return };
        if self.notification_worker.swap(true, Ordering::AcqRel) {
            return;
        }
        std::thread::spawn(move || {
            let registry = app.state::<TerminalRegistry>();
            loop {
                registry.notification_dirty.store(false, Ordering::Release);
                let state = app.state::<crate::commands::AppState>();
                if let Err(e) = registry.check_mailboxes(&state.db) {
                    eprintln!("[mailbox] {e}");
                }
                if registry.notification_dirty.load(Ordering::Acquire) {
                    continue;
                }
                registry.notification_worker.store(false, Ordering::Release);
                if registry.notification_dirty.load(Ordering::Acquire) {
                    registry.kick_notifications("");
                }
                break;
            }
        });
    }
    pub(crate) fn emit_notifications(&self) {
        if let Some(app) = &self.app {
            let _ = app.emit("terminal-notifications-changed", ());
        }
    }
    pub(crate) fn check_mailboxes(&self, db: &Db) -> Result<()> {
        let mut arrivals: HashMap<String, Vec<String>> = HashMap::new();
        loop {
            let mut frontier = self.mailbox_frontier.lock().unwrap();
            let rows = db.mailbox_arrivals(*frontier)?;
            if rows.is_empty() {
                break;
            }
            for (seq, session, id, immediate) in rows {
                *frontier = seq;
                let entry = arrivals.entry(session).or_default();
                if immediate {
                    entry.push(id);
                }
            }
        }
        for (session, immediate) in arrivals {
            if let Some(t) = self.for_session(&session) {
                let records = self.records.lock().unwrap();
                if let Some(r) = records.get(&t.terminal_id) {
                    let mut r = r.lock().unwrap();
                    if r.notification.enabled {
                        r.notification.pending = true;
                        r.notification.immediate_ids.extend(immediate);
                    }
                }
            } else if let Some(s) = db.get_session(&session)? {
                let adapter = crate::adapters::adapter_for(s.agent);
                if s.archived_at.is_none()
                    && adapter.has_terminal_cli()
                    && adapter.continue_route(&s) == crate::adapters::ResumeRoute::Terminal
                {
                    let count = db.mailbox_unread(&session)?;
                    if count > 0 {
                        if let Some(app) = &self.app {
                            let mut waking = self.waking_sessions.lock().unwrap();
                            if waking.insert(session.clone()) {
                                let app = app.clone();
                                let session = session.clone();
                                std::thread::spawn(move || {
                                    let state = app.state::<crate::commands::AppState>();
                                    let result = (|| -> Result<()> {
                                        let unread = state.db.mailbox_unread(&session)?;
                                        if unread > 0 {
                                            crate::commands::launch_mailbox_terminal(
                                                &app,
                                                &session,
                                                &text(unread),
                                            )?;
                                        }
                                        Ok(())
                                    })();
                                    app.state::<TerminalRegistry>()
                                        .waking_sessions
                                        .lock()
                                        .unwrap()
                                        .remove(&session);
                                    if let Err(error) = result {
                                        let _=app.emit("mailbox-notification-error",serde_json::json!({"session_id":session,"error":error.to_string()}));
                                    }
                                });
                            }
                        }
                    }
                }
            }
        }
        let targets: Vec<_> = {
            let records = self.records.lock().unwrap();
            records
                .iter()
                .filter_map(|(id, r)| {
                    let mut r = r.lock().ok()?;
                    if !r.summary.live {
                        return None;
                    }
                    let kind = if r.notification.manual_check
                        && ReminderKind::Manual.eligible(r.summary.activity.state, false)
                    {
                        r.notification.manual_check = false;
                        ReminderKind::Manual
                    } else if r.notification.enabled
                        && r.notification.pending
                        && ReminderKind::Automatic.eligible(
                            r.summary.activity.state,
                            !r.notification.immediate_ids.is_empty(),
                        )
                    {
                        ReminderKind::Automatic
                    } else {
                        return None;
                    };
                    Some((id.clone(), kind))
                })
                .collect()
        };
        for (id, kind) in targets {
            let _ = self.remind(db, &id, kind);
        }
        self.emit_notifications();
        Ok(())
    }
    pub fn request_reminder(&self, id: &str) -> Result<()> {
        {
            let records = self.records.lock().map_err(|_| other("终端不可用"))?;
            let mut r = records
                .get(id)
                .ok_or_else(|| other("终端不存在"))?
                .lock()
                .map_err(|_| other("终端不可用"))?;
            if !r.summary.live
                || r.summary.session_id.is_none()
                || !ReminderKind::Manual.eligible(r.summary.activity.state, false)
            {
                return Err(other("当前 Agent 状态不允许提醒"));
            }
            r.notification.manual_check = true;
        }
        self.kick_notifications(id);
        Ok(())
    }
    pub(super) fn remind(&self, db: &Db, id: &str, kind: ReminderKind) -> Result<()> {
        let result = self.write_reminder(db, id, kind);
        if let Err(error) = &result {
            let records = self.records.lock().unwrap();
            if let Some(record) = records.get(id) {
                let mut r = record.lock().unwrap();
                r.notification.error = Some(error.to_string());
                if kind != ReminderKind::Manual {
                    r.notification.pending = false;
                    r.notification.immediate_ids.clear();
                }
            }
            drop(records);
            self.emit_notifications();
        }
        result
    }
    fn write_reminder(&self, db: &Db, id: &str, kind: ReminderKind) -> Result<()> {
        // One local operation owns the writer; no lock is retained for a model response.
        let _action = self
            .notification_actions
            .lock()
            .map_err(|_| other("通知不可用"))?;
        let record = self
            .records
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| other("终端不存在"))?;
        let (summary, writer, immediate_ids) = {
            let r = record.lock().unwrap();
            if !r.summary.live
                || !kind.eligible(
                    r.summary.activity.state,
                    !r.notification.immediate_ids.is_empty(),
                )
                || kind != ReminderKind::Manual && !r.notification.enabled
            {
                return if kind == ReminderKind::Manual {
                    Err(other("当前 Agent 状态不允许提醒"))
                } else {
                    Ok(())
                };
            }
            (
                r.summary.clone(),
                r.writer.clone().ok_or_else(|| other("终端输入不可用"))?,
                r.notification.immediate_ids.clone(),
            )
        };
        let session = summary
            .session_id
            .as_deref()
            .ok_or_else(|| other("终端尚未绑定会话"))?;
        let mut writer = writer.lock().map_err(|_| other("终端输入不可用"))?;
        let unread = db.mailbox_unread(session)?;
        let unread_immediate: std::collections::HashSet<_> = if immediate_ids.is_empty() {
            Default::default()
        } else {
            db.mailbox_immediate_unread(session)?.into_iter().collect()
        };
        let immediate = immediate_ids.iter().any(|id| unread_immediate.contains(id));
        if unread == 0 {
            let mut r = record.lock().unwrap();
            r.notification.pending = false;
            r.notification.immediate_ids.clear();
            return Ok(());
        }
        let content = text(unread);
        let delivery = if kind == ReminderKind::Manual || immediate {
            messaging::MessageDelivery::Steer
        } else {
            messaging::MessageDelivery::Queue
        };
        let key = messaging::submit_key(summary.agent, delivery).map_err(|e| other(e.message))?;
        {
            let mut r = record.lock().unwrap();
            for stale in immediate_ids.difference(&unread_immediate) {
                r.notification.immediate_ids.remove(stale);
            }
            if !r.summary.live
                || r.summary.identity_revision != summary.identity_revision
                || !kind.eligible(r.summary.activity.state, immediate)
                || kind != ReminderKind::Manual && !r.notification.enabled
            {
                return if kind == ReminderKind::Manual {
                    Err(other("通知条件已变化"))
                } else {
                    Ok(())
                };
            }
            r.notification.writing = true;
            r.notification.input_revision += 1;
            r.notification.pending = false;
            r.notification.immediate_ids.clear();
        }
        self.emit_notifications();
        let result = (|| -> std::io::Result<()> {
            writer.write_all(format!("\x1b[200~{content}\x1b[201~").as_bytes())?;
            {
                let r = record.lock().unwrap();
                if !r.summary.live || r.summary.identity_revision != summary.identity_revision {
                    return Err(std::io::Error::other("终端已关闭或身份变化"));
                }
            }
            messaging::write_submit(&mut **writer, key)
        })();
        {
            let mut r = record.lock().unwrap();
            r.notification.writing = false;
            r.notification.input_revision += 1;
            if let Err(e) = &result {
                r.notification.reset();
                r.notification.error = Some(format!("提醒写入失败，请检查终端输入框：{e}"));
            }
        }
        self.emit_notifications();
        result.map_err(|e| other(format!("提醒写入失败：{e}")))
    }
    pub fn edit_mailbox(
        &self,
        db: &Db,
        id: &str,
        identity: u64,
        message: &str,
        revision: i64,
        body: Option<&str>,
    ) -> Result<Option<crate::storage::mailbox::MailboxMessage>> {
        let _action = self
            .notification_actions
            .lock()
            .map_err(|_| other("通知不可用"))?;
        // Hold the record across the short mutation, making toggle/edit atomic.
        let records = self.records.lock().unwrap();
        let r = records
            .get(id)
            .ok_or_else(|| other("终端不存在"))?
            .lock()
            .unwrap();
        if r.notification.enabled
            || r.notification.writing
            || !r.summary.live
            || r.summary.identity_revision != identity
        {
            return Err(other("请关闭自动通知并刷新终端身份后编辑"));
        }
        let session = r
            .summary
            .session_id
            .as_deref()
            .ok_or_else(|| other("终端尚未绑定会话"))?;
        let old = db.mailbox_read(session, message)?;
        let result = if let Some(body) = body {
            Some(db.mailbox_edit(session, message, revision, body)?)
        } else {
            db.mailbox_delete(session, message, revision)?;
            None
        };
        let session = session.to_owned();
        drop(r);
        drop(records);
        if let Some(home) = self.app.as_ref().and_then(crate::commands::noending_home) {
            super::images::cleanup(&home, db, &session, Some(&old.body));
        }
        if let Some(app) = &self.app {
            let _ = app.emit("mailbox-changed", ());
        }
        Ok(result)
    }
}
