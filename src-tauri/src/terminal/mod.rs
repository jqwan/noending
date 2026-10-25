//! Embedded terminal backend — one real PTY per embedded session.
//!
//! The four TUI agents (codex / claude / pi / agy) expect a terminal, not
//! pipes: raw mode, alternate screen, `isatty`, window-size ioctls. This
//! module owns that terminal for as long as the app runs. The frontend
//! terminal view is an attach/detach client — the scrollback ring buffer
//! here is the source of truth, so switching session subpages (a React
//! remount) never touches the running Agent.
//!
//! Deliberate product behavior: closing NoEnding kills the embedded Agents.
//! An Agent that must outlive the app belongs in the external terminal,
//! which keeps working exactly as before.

mod activity;
#[cfg(any(windows, test))]
mod flow;
pub mod images;
pub use activity::TerminalActivity;
pub mod identity;
pub mod messaging;
pub mod notifications;
pub mod spawn;

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Read;
use std::sync::Mutex;

use base64::Engine;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::adapters::AgentCommand;
use crate::domain::Agent;
use crate::error::{other, Result};
use crate::storage::{new_id, now};

/// Scrollback kept per terminal. Bounded by design — an agent redrawing a
/// fullscreen TUI can emit megabytes per minute, and the buffer exists so a
/// subpage switch can repaint, not to archive the whole conversation (the
/// ingested Session is the archive).
const SCROLLBACK_CAP: usize = 256 * 1024;

/// Initial PTY size. The frontend resizes on attach as soon as it knows the
/// real cell geometry; TUIs handle the early SIGWINCH fine.
const INITIAL_COLS: u16 = 80;
const INITIAL_ROWS: u16 = 24;

pub const EVENT_OUTPUT_PREFIX: &str = "terminal-output://";
pub const EVENT_EXIT_PREFIX: &str = "terminal-exit://";
/// List-level fact: terminal records or their state changed (spawn / exit / bind / close).
/// Sidebar and any terminal-list consumer refetch once per event — no polling.
pub const EVENT_CHANGED: &str = "terminals-changed";
/// Lightweight state refresh; does not invalidate session/project discovery.
pub const EVENT_ACTIVITY: &str = "terminal-activity";
/// Identity fact: a terminal acquired, changed or cleared its Session binding.
/// The revision orders events against attach snapshots and delayed DB reads.
pub const EVENT_BOUND: &str = "terminal-bound";

/// The payload of `terminal-bound`.
#[derive(Debug, Clone, Serialize)]
pub struct TerminalBound {
    pub terminal_id: String,
    pub session_id: Option<String>,
    pub identity_revision: u64,
    pub cwd: Option<String>,
}

/// The facts a terminal subpage needs to decide what it is looking at.
/// `session_id` is None for a NEW-session terminal: the Agent has been
/// launched but its identity is unknown or its source has not been ingested.
/// Only a prespecified or Agent-reported native ID establishes identity.
#[derive(Debug, Clone, Serialize)]
pub struct TerminalSummary {
    pub terminal_id: String,
    pub session_id: Option<String>,
    /// Selected tasks before first binding; cleared after the initial tracking is consumed.
    pub initial_workstream_ids: Vec<String>,
    pub identity_revision: u64,
    pub activity: TerminalActivity,
    pub agent: Agent,
    pub cwd: Option<String>,
    pub created_at: String,
    pub live: bool,
    pub exit_code: Option<i32>,
    /// The bound Session's display title; list and attach refresh it from DB.
    /// None for unbound terminals.
    #[serde(default)]
    pub session_title: Option<String>,
}

/// Attach snapshot and its output byte boundary. Events subscribed before
/// attach may overlap this snapshot; the boundary lets clients replay once.
#[derive(Debug, Clone, Serialize)]
pub struct TerminalSnapshot {
    #[serde(flatten)]
    pub summary: TerminalSummary,
    /// base64 of the buffered raw PTY bytes.
    pub scrollback: String,
    /// Total PTY bytes observed, including bytes evicted from scrollback.
    pub output_offset: u64,
    pub cols: u16,
    pub rows: u16,
}

/// The output payload of `terminal-output://{id}`.
#[derive(Debug, Clone, Serialize)]
pub struct TerminalOutput {
    /// Offset of the first byte in `data` in this terminal's output stream.
    pub offset: u64,
    /// Base64-encoded raw PTY bytes.
    pub data: String,
}

/// The exit payload of `terminal-exit://{id}`.
#[derive(Debug, Clone, Serialize)]
pub struct TerminalExit {
    pub exit_code: Option<i32>,
}

/// Bounded raw-byte scrollback. A struct of its own so the eviction rule is
/// unit-testable without a real PTY on every platform.
#[derive(Default)]
struct Scrollback {
    data: VecDeque<u8>,
    output_offset: u64,
}

impl Scrollback {
    fn push(&mut self, bytes: &[u8]) {
        self.output_offset += bytes.len() as u64;
        if bytes.len() >= SCROLLBACK_CAP {
            self.data.clear();
            let keep = &bytes[bytes.len() - SCROLLBACK_CAP..];
            self.data.extend(keep.iter().copied());
            return;
        }
        self.data.extend(bytes.iter().copied());
        if self.data.len() > SCROLLBACK_CAP {
            let excess = self.data.len() - SCROLLBACK_CAP;
            self.data.drain(..excess);
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.data.len()
    }

    fn bytes(&self) -> Vec<u8> {
        self.data.iter().copied().collect()
    }
}

/// One embedded terminal. Fields are behind one per-record mutex because the
/// reader thread (PTY output), the wait thread (exit), and the frontend
/// commands (input / resize / attach) all touch it — each holds the lock only
/// across its own field update, never across a Db call and never across an
/// emit, so contention is bounded to microseconds.
struct TerminalRecord {
    summary: TerminalSummary,
    /// Current native root identity, supplied by the launch or an Agent event.
    /// It also retains a switched-to identity while its source is not ingested.
    root_agent_session_id: Option<String>,
    /// One-shot task assignment for the launch's first native conversation.
    pending_tracking: Option<PendingTracking>,
    identity_bridge: Option<std::sync::Arc<identity::IdentityBridge>>,
    scrollback: Scrollback,
    cols: u16,
    rows: u16,
    writer: Option<std::sync::Arc<Mutex<Box<dyn std::io::Write + Send>>>>,
    notification: notifications::NotificationState,
    observed_inputs: HashSet<String>,
    runtime_files: Vec<crate::platform::runtime_file::RuntimeFile>,
    started_turns: u64,
    master: Box<dyn portable_pty::MasterPty + Send>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
}

struct PendingTracking {
    workstream_ids: Vec<String>,
    native_id: Option<String>,
}

/// The backend's set of embedded terminals, keyed by `terminal_id`.
/// Managed app state; `app` is `Some` in production (events flow to the
/// webview) and `None` in tests (emits are skipped, everything else runs).
pub struct TerminalRegistry {
    records: Mutex<HashMap<String, std::sync::Arc<Mutex<TerminalRecord>>>>,
    #[cfg(windows)]
    output_flows: Mutex<HashMap<String, std::sync::Arc<flow::OutputFlow>>>,
    /// Prevent close/switch from racing the first binding's task tracking write.
    /// Database work happens without holding a terminal record or PTY lock.
    identity_mutations: Mutex<()>,
    /// Serializes Session startup and excludes conflicting native bindings.
    /// Always acquired before `records`; never held across a process spawn.
    resume_reservations: Mutex<HashSet<String>>,
    mailbox_frontier: Mutex<i64>,
    waking_sessions: Mutex<HashSet<String>>,
    notification_actions: Mutex<()>,
    notification_worker: std::sync::atomic::AtomicBool,
    notification_dirty: std::sync::atomic::AtomicBool,
    app: Option<AppHandle>,
}
/// A pending Resume claim. Every return path, including a failed spawn,
/// releases it; a successful spawn has registered its live record by then.
pub struct ResumeReservation<'a> {
    registry: &'a TerminalRegistry,
    session_id: String,
    stopped_terminals: Vec<String>,
}

impl ResumeReservation<'_> {
    pub(crate) fn finish_reconnect(&self) {
        for id in &self.stopped_terminals {
            self.registry.close(id);
        }
    }
}

impl Drop for ResumeReservation<'_> {
    fn drop(&mut self) {
        if let Ok(mut reservations) = self.registry.resume_reservations.lock() {
            reservations.remove(&self.session_id);
        }
    }
}

impl Drop for TerminalRegistry {
    fn drop(&mut self) {
        self.kill_all();
    }
}

impl TerminalRegistry {
    pub fn retain_runtime_file(
        &self,
        id: &str,
        file: crate::platform::runtime_file::RuntimeFile,
    ) -> Result<()> {
        let records = self.records.lock().unwrap();
        let mut r = records
            .get(id)
            .ok_or_else(|| other("终端不存在"))?
            .lock()
            .unwrap();
        r.runtime_files.push(file);
        Ok(())
    }
    pub fn new(app: Option<AppHandle>) -> Self {
        Self {
            records: Mutex::new(HashMap::new()),
            #[cfg(windows)]
            output_flows: Mutex::new(HashMap::new()),
            identity_mutations: Mutex::new(()),
            resume_reservations: Mutex::new(HashSet::new()),
            mailbox_frontier: Mutex::new(0),
            waking_sessions: Mutex::new(HashSet::new()),
            notification_actions: Mutex::new(()),
            notification_worker: std::sync::atomic::AtomicBool::new(false),
            notification_dirty: std::sync::atomic::AtomicBool::new(false),
            app,
        }
    }

    /// A Session's Continue entry only reuses live terminals. Exited terminals
    /// remain attachable by id for scrollback and explicit reconnection.
    pub fn for_session(&self, session_id: &str) -> Option<TerminalSummary> {
        let records = self.records.lock().ok()?;
        records
            .values()
            .filter_map(|record| {
                let record = record.lock().ok()?;
                (record.summary.live && record.summary.session_id.as_deref() == Some(session_id))
                    .then(|| record.summary.clone())
            })
            .max_by(|a, b| a.created_at.cmp(&b.created_at))
    }

    /// Whether `session_id` has a terminal whose Agent is still running.
    /// NoEnding's Resume entry can reuse it; an Agent's internal Resume may
    /// independently associate another terminal with the same Session.
    pub fn has_live(&self, session_id: &str) -> bool {
        self.records
            .lock()
            .map(|records| {
                records.values().any(|record| {
                    let Ok(record) = record.lock() else {
                        return false;
                    };
                    record.summary.session_id.as_deref() == Some(session_id) && record.summary.live
                })
            })
            .unwrap_or(false)
    }

    /// Every retained terminal, newest first. Exited terminals stay visible
    /// until the user removes or reconnects them.
    pub fn list(&self) -> Vec<TerminalSummary> {
        let Ok(records) = self.records.lock() else {
            return Vec::new();
        };
        let mut summaries: Vec<TerminalSummary> = records
            .values()
            .filter_map(|record| {
                let record = record.lock().ok()?;
                Some(record.summary.clone())
            })
            .collect();
        summaries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        summaries
    }

    /// Atomically reject both a live Resume and another pending spawn.
    pub fn reserve_resume(
        &self,
        session_id: &str,
    ) -> std::result::Result<ResumeReservation<'_>, String> {
        let mut reservations = self
            .resume_reservations
            .lock()
            .map_err(|_| "terminal reservation lock poisoned".to_string())?;
        let records = self
            .records
            .lock()
            .map_err(|_| "terminal registry lock poisoned".to_string())?;
        let mut live = false;
        for record in records.values() {
            let record = record
                .lock()
                .map_err(|_| "terminal record lock poisoned".to_string())?;
            live |= record.summary.live && record.summary.session_id.as_deref() == Some(session_id);
        }
        if reservations.contains(session_id) || live {
            return Err("该会话已有运行中或正在启动的内嵌终端；等它退出后可以再次启动".into());
        }
        reservations.insert(session_id.to_string());
        Ok(ResumeReservation {
            registry: self,
            session_id: session_id.to_string(),
            stopped_terminals: Vec::new(),
        })
    }

    /// Stop all terminals of the verified Session before starting its replacement.
    /// Keep stopped records on failure for inspection and retry.
    pub fn reserve_reconnect(
        &self,
        expected: &TerminalSummary,
        session_id: &str,
    ) -> Result<ResumeReservation<'_>> {
        let _identity = self
            .identity_mutations
            .lock()
            .map_err(|_| other("终端状态不可用"))?;
        let mut reservations = self
            .resume_reservations
            .lock()
            .map_err(|_| other("terminal reservation lock poisoned"))?;
        if reservations.contains(session_id) {
            return Err(other("该会话正在连接，请稍后重试"));
        }
        let records = self
            .records
            .lock()
            .map_err(|_| other("terminal registry lock poisoned"))?;
        {
            let record = records
                .get(&expected.terminal_id)
                .ok_or_else(|| other("终端已关闭，无法重新连接"))?;
            let record = record
                .lock()
                .map_err(|_| other("terminal record lock poisoned"))?;
            if record.summary.session_id.as_deref() != Some(session_id)
                || record.summary.identity_revision != expected.identity_revision
            {
                return Err(other("终端会话已切换，请重试重新连接"));
            }
        }
        let mut stopped_terminals = Vec::new();
        for record in records.values() {
            let mut record = record
                .lock()
                .map_err(|_| other("terminal record lock poisoned"))?;
            if record.summary.session_id.as_deref() != Some(session_id) {
                continue;
            }
            if record.summary.live {
                record
                    .killer
                    .kill()
                    .map_err(|e| other(format!("无法停止旧终端：{e}")))?;
            }
            record.summary.live = false;
            record.writer = None;
            record.notification.reset();
            record.identity_bridge.take();
            stopped_terminals.push(record.summary.terminal_id.clone());
            if let Some(app) = &self.app {
                let _ = app.emit(
                    &format!("{EVENT_EXIT_PREFIX}{}", record.summary.terminal_id),
                    TerminalExit {
                        exit_code: record.summary.exit_code,
                    },
                );
            }
        }
        reservations.insert(session_id.to_owned());
        if let Some(app) = &self.app {
            let _ = app.emit(EVENT_CHANGED, ());
        }
        Ok(ResumeReservation {
            registry: self,
            session_id: session_id.to_owned(),
            stopped_terminals,
        })
    }

    fn emit_activity(&self) {
        if let Some(app) = &self.app {
            let _ = app.emit(EVENT_ACTIVITY, ());
        }
    }

    /// A read receipt only clears the exact state rendered by the caller.
    pub fn mark_activity_seen(&self, terminal_id: &str, revision: u64) {
        let changed = self
            .records
            .lock()
            .ok()
            .and_then(|records| {
                let mut record = records.get(terminal_id)?.lock().ok()?;
                Some(record.summary.activity.mark_seen(revision))
            })
            .unwrap_or(false);
        if changed {
            self.emit_activity();
            self.kick_notifications(terminal_id);
        }
    }

    fn emit_identity(&self, summary: &TerminalSummary) {
        self.kick_notifications(&summary.terminal_id);
        if let Some(app) = &self.app {
            let _ = app.emit(
                EVENT_BOUND,
                TerminalBound {
                    terminal_id: summary.terminal_id.clone(),
                    session_id: summary.session_id.clone(),
                    identity_revision: summary.identity_revision,
                    cwd: summary.cwd.clone(),
                },
            );
            let _ = app.emit(EVENT_CHANGED, ());
        }
    }

    fn stop_conflicting_terminal(&self, record: &mut TerminalRecord, owner: &str) {
        if record.summary.live {
            let _ = record.killer.kill();
        }
        record.summary.live = false;
        record.writer = None;
        record.notification.reset();
        let text = format!("\r\n[NoEnding] 目标会话已在终端 {owner} 中运行，已停止当前终端。\r\n");
        let offset = record.scrollback.output_offset;
        record.scrollback.push(text.as_bytes());
        if let Some(app) = &self.app {
            let _ = app.emit(
                &format!("{EVENT_OUTPUT_PREFIX}{}", record.summary.terminal_id),
                TerminalOutput {
                    offset,
                    data: base64::engine::general_purpose::STANDARD.encode(text),
                },
            );
            let _ = app.emit(
                &format!("{EVENT_EXIT_PREFIX}{}", record.summary.terminal_id),
                TerminalExit {
                    exit_code: record.summary.exit_code,
                },
            );
            let _ = app.emit(EVENT_CHANGED, ());
        }
    }

    /// A native fact may rebind a terminal only if its target is unoccupied.
    /// Clear the old Session immediately; resolving the new ID is a separate,
    /// version-checked step so slow ingestion cannot resurrect an old identity.
    pub fn report_native_identity(
        &self,
        db: &crate::storage::Db,
        terminal_id: &str,
        event: &identity::NativeIdentityEvent,
    ) -> Option<TerminalSummary> {
        self.apply_native_identity(db, terminal_id, event)
            .map(|(summary, _)| summary)
    }

    /// Native hooks only schedule history work. The ingestion worker owns all
    /// source reads and database updates, independently of mailbox notifications.
    fn native_ingest_request(
        &self,
        db: &crate::storage::Db,
        terminal_id: &str,
        event: &identity::NativeIdentityEvent,
    ) -> Option<crate::commands::ingestion::IngestScope> {
        let (current, new_input) = self.apply_native_identity(db, terminal_id, event)?;
        if let Some(session) = current.session_id.as_ref().filter(|_| new_input) {
            // Reuse the cursor-based member refresh. Claude/Pi must not discover
            // every neighboring transcript whenever a new input starts.
            return Some(crate::commands::ingestion::IngestScope::RefreshSession(
                session.clone(),
            ));
        }
        // A user input can start while the Agent is already Working. Conversely,
        // a new model invocation alone is not a new input (notably Antigravity).
        if current.session_id.is_some()
            && !new_input
            && (event.input.is_some()
                || event.activity.as_ref().is_some_and(|signal| {
                    matches!(
                        signal.state,
                        activity::ActivityState::Working | activity::ActivityState::Waiting
                    )
                }))
        {
            return None;
        }
        Some(crate::commands::ingestion::IngestScope::ReconcileNative {
            agent: current.agent,
            native_id: event.native_id.clone(),
            source_path: event.source_path.clone(),
        })
    }

    fn apply_native_identity(
        &self,
        db: &crate::storage::Db,
        terminal_id: &str,
        event: &identity::NativeIdentityEvent,
    ) -> Option<(TerminalSummary, bool)> {
        if event.native_id.trim().is_empty() {
            return None;
        }
        let (summary, changed, new_input) = {
            let _identity = self.identity_mutations.lock().ok()?;
            let records = self.records.lock().ok()?;
            let mut record = records.get(terminal_id)?.lock().ok()?;
            // Turn completion only refreshes the current conversation. It
            // must not restore an old identity after a lifecycle switch.
            if !record.summary.live {
                return None;
            }
            if (matches!(
                event.source.as_deref(),
                Some("Stop" | "agent_end" | "agent_settled" | "message_start" | "user_message")
            ) || event
                .activity
                .as_ref()
                .is_some_and(|signal| !signal.starts_turn))
                && record
                    .root_agent_session_id
                    .as_deref()
                    .is_some_and(|id| id != event.native_id)
            {
                return None;
            }
            let conflict = records
                .iter()
                .filter(|(id, _)| id.as_str() != terminal_id)
                .find_map(|(id, peer)| {
                    let peer = peer.lock().ok()?;
                    (peer.summary.live
                        && peer.summary.agent == record.summary.agent
                        && peer.root_agent_session_id.as_deref() == Some(&event.native_id))
                    .then(|| id.clone())
                });
            if let Some(owner) = conflict {
                self.stop_conflicting_terminal(&mut record, &owner);
                return None;
            }
            let identity_changed =
                record.root_agent_session_id.as_deref() != Some(event.native_id.as_str());
            let cwd_changed = event
                .cwd
                .as_ref()
                .is_some_and(|cwd| record.summary.cwd.as_ref() != Some(cwd));
            if identity_changed {
                if let Some(pending) = &mut record.pending_tracking {
                    if pending
                        .native_id
                        .as_ref()
                        .is_some_and(|id| id != &event.native_id)
                    {
                        // The TUI switched before initial ingestion completed.
                        // The launch tasks never follow an unrelated conversation.
                        record.pending_tracking = None;
                    } else {
                        pending.native_id = Some(event.native_id.clone());
                    }
                }
                record.summary.activity.reset();
                record.root_agent_session_id = Some(event.native_id.clone());
                record.summary.session_id = None;
                record.summary.session_title = None;
                record.summary.initial_workstream_ids = record
                    .pending_tracking
                    .as_ref()
                    .map(|pending| pending.workstream_ids.clone())
                    .unwrap_or_default();
            }
            if let Some(cwd) = &event.cwd {
                record.summary.cwd = Some(cwd.clone());
            }
            if identity_changed || cwd_changed {
                record.summary.identity_revision += 1;
            }
            if identity_changed {
                record.observed_inputs.clear();
                record.notification.reset();
                record.started_turns = 0;
            }
            if event
                .activity
                .as_ref()
                .is_some_and(|signal| signal.starts_turn)
            {
                record.started_turns += 1;
            }
            let activity_changed = event
                .activity
                .as_ref()
                .is_some_and(|signal| record.summary.activity.apply(signal));
            let new_input = event.input.as_ref().is_some_and(|input| {
                let key = input
                    .id
                    .clone()
                    .unwrap_or_else(|| record.summary.activity.revision.to_string());
                if record.observed_inputs.len() >= 256 {
                    record.observed_inputs.clear();
                }
                record.observed_inputs.insert(key)
            });
            let state = record.summary.activity.state;
            record.notification.observe(state);
            (
                record.summary.clone(),
                (identity_changed || cwd_changed, activity_changed),
                new_input,
            )
        };
        if changed.0 {
            self.emit_identity(&summary);
        } else if changed.1 {
            self.emit_activity();
        }
        if summary.session_id.is_none() {
            self.resolve_native_identity(db, &summary, &event.native_id);
        }
        self.kick_notifications(terminal_id);
        self.attach(terminal_id)
            .ok()
            .map(|snapshot| (snapshot.summary, new_input))
    }

    fn resolve_native_identity(
        &self,
        db: &crate::storage::Db,
        summary: &TerminalSummary,
        native_id: &str,
    ) {
        if let Ok(Some(session)) = db.find_session_by_root_agent_id(summary.agent, native_id) {
            self.finish_native_binding(db, summary, native_id, &session);
        }
    }

    /// Apply a lookup only to the exact identity version that requested it.
    /// Comparing both version and native ID also protects A -> B -> A switches.
    fn finish_native_binding(
        &self,
        db: &crate::storage::Db,
        expected: &TerminalSummary,
        native_id: &str,
        session: &crate::domain::Session,
    ) {
        let Ok(identity_guard) = self.identity_mutations.lock() else {
            return;
        };
        let Ok(reservations) = self.resume_reservations.lock() else {
            return;
        };
        let owner = {
            let Ok(records) = self.records.lock() else {
                return;
            };
            let Some(record) = records.get(&expected.terminal_id) else {
                return;
            };
            let Ok(record) = record.lock() else {
                return;
            };
            if record.summary.identity_revision != expected.identity_revision
                || record.root_agent_session_id.as_deref() != Some(native_id)
                || record.summary.session_id.is_some()
                || record.summary.agent != session.agent
                || session.root_agent_session_id != native_id
            {
                return;
            }
            let conflict = records
                .iter()
                .filter(|(id, _)| id.as_str() != expected.terminal_id)
                .find_map(|(id, peer)| {
                    let peer = peer.lock().ok()?;
                    (peer.summary.live && peer.summary.session_id.as_deref() == Some(&session.id))
                        .then(|| id.clone())
                });
            if conflict.is_some() || reservations.contains(&session.id) {
                drop(record);
                let mut record = records[&expected.terminal_id].lock().unwrap();
                self.stop_conflicting_terminal(
                    &mut record,
                    conflict.as_deref().unwrap_or("正在启动的新终端"),
                );
                return;
            }
            record
                .pending_tracking
                .as_ref()
                .filter(|pending| pending.native_id.as_deref() == Some(native_id))
                .map(|pending| pending.workstream_ids.clone())
                .unwrap_or_default()
        };
        if let Err(error) = db.inherit_terminal_tracking(session, &owner) {
            eprintln!("[terminal] native binding failed: {error}");
            return;
        }
        let bound = {
            let Ok(records) = self.records.lock() else {
                return;
            };
            let Some(record) = records.get(&expected.terminal_id) else {
                return;
            };
            let Ok(mut record) = record.lock() else {
                return;
            };
            record.summary.session_id = Some(session.id.clone());
            record.summary.initial_workstream_ids.clear();
            record.pending_tracking = None;
            record.summary.session_title = session.title.clone();
            record.summary.identity_revision += 1;
            let state = record.summary.activity.state;
            record.notification.observe(state);
            record.summary.clone()
        };
        drop(reservations);
        drop(identity_guard);
        self.emit_identity(&bound);
    }

    /// Ingestion resolves pending native IDs. There is deliberately no message,
    /// cwd, timestamp or exclusive-occupancy inference here.
    pub fn bind_discovered(&self, db: &crate::storage::Db) {
        let waiting: Vec<_> = {
            let Ok(records) = self.records.lock() else {
                return;
            };
            records
                .values()
                .filter_map(|record| {
                    let record = record.lock().ok()?;
                    if record.summary.session_id.is_some() {
                        return None;
                    }
                    Some((
                        record.summary.clone(),
                        record.root_agent_session_id.clone()?,
                    ))
                })
                .collect()
        };
        for (summary, native_id) in waiting {
            self.resolve_native_identity(db, &summary, &native_id);
        }
    }

    /// Explicit close from the sidebar's 运行中 item: kill the child and
    /// REMOVE the record. Removal is deliberate — an explicitly closed
    /// terminal has no replay value (ingestion keeps the conversation), and
    /// a removed record cannot be resurrected by terminal_for_session, so
    /// the next Resume spawns a fresh terminal instead of landing on a
    /// corpse. The wait thread still reaps the child (it holds its own Arc)
    /// and publishes terminal-exit + terminals-changed.
    pub fn close(&self, terminal_id: &str) -> Option<TerminalSummary> {
        let _identity = self.identity_mutations.lock().ok()?;
        self.close_locked(terminal_id)
    }

    /// Keep native bindings stable across deletion; only a successful commit
    /// removes its terminals. Other sessions and their processes are untouched.
    pub fn with_session_removal<T>(
        &self,
        session_id: &str,
        remove: impl FnOnce() -> Result<T>,
    ) -> Result<(T, Vec<String>)> {
        let _identity = self
            .identity_mutations
            .lock()
            .map_err(|_| other("终端状态不可用"))?;
        let result = remove()?;
        let ids: Vec<_> = self
            .list()
            .into_iter()
            .filter(|t| t.session_id.as_deref() == Some(session_id))
            .map(|t| t.terminal_id)
            .collect();
        for id in &ids {
            self.close_locked(id);
        }

        if let Some(app) = &self.app {
            if let Some(home) = crate::commands::noending_home(app) {
                let state = app.state::<crate::commands::AppState>();
                images::cleanup(&home, &state.db, session_id, None);
            }
        }
        Ok((result, ids))
    }

    fn close_locked(&self, terminal_id: &str) -> Option<TerminalSummary> {
        let removed = {
            let Ok(mut records) = self.records.lock() else {
                return None;
            };
            records.remove(terminal_id)
        }?;
        #[cfg(windows)]
        if let Some(flow) = self.output_flows.lock().ok()?.remove(terminal_id) {
            flow.close();
        }
        let (summary, bridge) = {
            let Ok(mut record) = removed.lock() else {
                return None;
            };
            if record.summary.live {
                let _ = record.killer.kill();
                record.summary.live = false;
            }
            record.notification.reset();
            (record.summary.clone(), record.identity_bridge.take())
        };
        drop(bridge);
        // The retained set shrank (the record is gone outright).
        if let Some(app) = &self.app {
            let _ = app.emit(EVENT_CHANGED, ());
        }
        Some(summary)
    }

    pub fn attach(&self, terminal_id: &str) -> Result<TerminalSnapshot> {
        self.snapshot(terminal_id, None)
    }

    fn snapshot(&self, terminal_id: &str, consumer: Option<&str>) -> Result<TerminalSnapshot> {
        let records = self
            .records
            .lock()
            .map_err(|_| other("terminal registry lock poisoned"))?;
        let record = records
            .get(terminal_id)
            .ok_or_else(|| other("终端不存在或已随应用重启失效"))?
            .clone();
        drop(records);
        let record = record
            .lock()
            .map_err(|_| other("terminal record lock poisoned"))?;
        #[cfg(windows)]
        if let Some(consumer) = consumer {
            if let Some(flow) = self.output_flows.lock().unwrap().get(terminal_id) {
                flow.attach(
                    consumer,
                    record.scrollback.output_offset - record.scrollback.data.len() as u64,
                );
            }
        }
        #[cfg(not(windows))]
        let _ = consumer;
        Ok(TerminalSnapshot {
            summary: record.summary.clone(),
            scrollback: base64::engine::general_purpose::STANDARD.encode(record.scrollback.bytes()),
            output_offset: record.scrollback.output_offset,
            cols: record.cols,
            rows: record.rows,
        })
    }

    /// Only a subscribed frontend may acquire output credit. Identity lookups
    /// and other snapshot reads deliberately remain side-effect free.
    pub fn attach_output(&self, terminal_id: &str, consumer: &str) -> Result<TerminalSnapshot> {
        self.snapshot(terminal_id, Some(consumer))
    }

    pub fn release_output(&self, terminal_id: &str, consumer: &str) {
        #[cfg(windows)]
        if let Some(flow) = self.output_flows.lock().unwrap().get(terminal_id) {
            flow.release(consumer);
        }
        #[cfg(not(windows))]
        let _ = (terminal_id, consumer);
    }

    /// Absolute raw-byte position parsed by xterm, not merely received by JS.
    pub fn acknowledge_output(&self, terminal_id: &str, consumer: &str, offset: u64) {
        #[cfg(windows)]
        if let Some(flow) = self.output_flows.lock().unwrap().get(terminal_id) {
            flow.acknowledge(consumer, offset);
        }
        #[cfg(not(windows))]
        let _ = (terminal_id, consumer, offset);
    }

    /// Frontend keystrokes → PTY master. `data` is the xterm-encoded string
    /// (xterm.js owns key encoding); an exited terminal refuses input with a
    /// stated reason instead of dropping bytes silently.
    pub fn write(&self, terminal_id: &str, data: &str) -> Result<()> {
        self.write_bytes(terminal_id, data.as_bytes(), None)
    }

    /// xterm onBinary uses one Unicode codepoint per raw byte, not UTF-8.
    pub fn write_binary(&self, terminal_id: &str, data: &str) -> Result<()> {
        let bytes = data
            .chars()
            .map(|ch| u8::try_from(ch as u32).map_err(|_| other("无效的终端字节输入")))
            .collect::<Result<Vec<_>>>()?;
        self.write_bytes(terminal_id, &bytes, None)
    }

    pub fn write_guarded(
        &self,
        terminal_id: &str,
        data: &str,
        binary: bool,
        revision: u64,
    ) -> Result<()> {
        let bytes = if binary {
            data.chars()
                .map(|ch| u8::try_from(ch as u32).map_err(|_| other("无效的终端字节输入")))
                .collect::<Result<Vec<_>>>()?
        } else {
            data.as_bytes().to_vec()
        };
        self.write_bytes(terminal_id, &bytes, Some(revision))
    }

    fn write_bytes(
        &self,
        terminal_id: &str,
        data: &[u8],
        input_revision: Option<u64>,
    ) -> Result<()> {
        let records = self
            .records
            .lock()
            .map_err(|_| other("terminal registry lock poisoned"))?;
        let record = records
            .get(terminal_id)
            .ok_or_else(|| other("终端不存在"))?
            .clone();
        drop(records);
        let writer = {
            let record = record.lock().map_err(|_| other("终端不可用"))?;
            if !record.summary.live {
                return Err(other("该终端中的 Agent 已退出"));
            }
            record
                .writer
                .clone()
                .ok_or_else(|| other("终端输入流不可用"))?
        };
        let mut writer = writer.lock().map_err(|_| other("终端输入流不可用"))?;
        let record = record
            .lock()
            .map_err(|_| other("terminal record lock poisoned"))?;
        if !record.summary.live {
            return Err(other("该终端中的 Agent 已退出"));
        }
        let reply = messaging::terminal_reply(data);
        if !reply
            && (record.notification.enabled
                || record.notification.writing
                || input_revision.is_some_and(|epoch| epoch != record.notification.input_revision))
        {
            return Err(other("终端自动通知已开启或输入状态已变化，请使用信箱"));
        }
        drop(record);
        // Never hold a record lock across blocking PTY I/O.
        writer
            .write_all(data)
            .and_then(|_| writer.flush())
            .map_err(|e| other(format!("写入终端失败: {e}")))
    }

    pub fn resize(&self, terminal_id: &str, cols: u16, rows: u16) -> Result<()> {
        let records = self
            .records
            .lock()
            .map_err(|_| other("terminal registry lock poisoned"))?;
        let record = records
            .get(terminal_id)
            .ok_or_else(|| other("终端不存在"))?
            .clone();
        drop(records);
        let mut record = record
            .lock()
            .map_err(|_| other("terminal record lock poisoned"))?;
        let cols = cols.clamp(1, i16::MAX as u16);
        let rows = rows.clamp(1, i16::MAX as u16);
        if record.cols == cols && record.rows == rows {
            return Ok(());
        }
        record
            .master
            .resize(portable_pty::PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| other(format!("调整终端尺寸失败: {e}")))?;
        record.cols = cols;
        record.rows = rows;
        Ok(())
    }

    /// App exit: end every embedded Agent. Children die with the PTY master
    /// on Unix anyway; killing explicitly makes it true on ConPTY too.
    pub fn kill_all(&self) {
        #[cfg(windows)]
        for flow in self.output_flows.lock().unwrap().values() {
            flow.close();
        }
        let Ok(records) = self.records.lock() else {
            return;
        };
        for record in records.values() {
            let Ok(mut record) = record.lock() else {
                continue;
            };
            record.notification.reset();
            if record.summary.live {
                let _ = record.killer.kill();
                record.summary.live = false;
            }
        }
        drop(records);
    }

    /// Insert a freshly spawned terminal and start its reader/wait threads.
    /// Called only by [`spawn_embedded`]; the threads get `Arc` clones so the
    /// registry lock is never held across a thread's work.
    fn register(
        &self,
        summary: TerminalSummary,
        root_agent_session_id: Option<String>,
        identity_bridge: Option<std::sync::Arc<identity::IdentityBridge>>,
        scrollback: Scrollback,
        master: Box<dyn portable_pty::MasterPty + Send>,
        writer: Box<dyn std::io::Write + Send>,
        killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
        reader: Box<dyn Read + Send>,
        child: Box<dyn portable_pty::Child + Send + Sync>,
    ) -> Result<TerminalSummary> {
        let terminal_id = summary.terminal_id.clone();
        #[cfg(windows)]
        let output_flow = {
            let flow = std::sync::Arc::new(flow::OutputFlow::default());
            self.output_flows
                .lock()
                .unwrap()
                .insert(terminal_id.clone(), flow.clone());
            flow
        };
        let record = std::sync::Arc::new(Mutex::new(TerminalRecord {
            pending_tracking: (summary.session_id.is_none()
                && !summary.initial_workstream_ids.is_empty())
            .then(|| PendingTracking {
                workstream_ids: summary.initial_workstream_ids.clone(),
                native_id: root_agent_session_id.clone(),
            }),
            summary: summary.clone(),
            root_agent_session_id,
            identity_bridge,
            scrollback,
            cols: INITIAL_COLS,
            rows: INITIAL_ROWS,
            writer: Some(std::sync::Arc::new(Mutex::new(writer))),
            notification: notifications::NotificationState::default(),
            observed_inputs: HashSet::new(),
            runtime_files: Vec::new(),
            started_turns: 0,
            master,
            killer,
        }));
        let conflict = {
            let _identity = self.identity_mutations.lock().unwrap();
            let mut records = self.records.lock().unwrap();
            let incoming = record.lock().unwrap();
            let conflict = records.iter().find_map(|(id, peer)| {
                let peer = peer.lock().ok()?;
                (peer.summary.live
                    && (summary.session_id.is_some()
                        && peer.summary.session_id == summary.session_id
                        || incoming.root_agent_session_id.is_some()
                            && peer.summary.agent == summary.agent
                            && peer.root_agent_session_id == incoming.root_agent_session_id))
                    .then(|| id.clone())
            });
            drop(incoming);
            if let Some(owner) = &conflict {
                self.stop_conflicting_terminal(&mut record.lock().unwrap(), owner);
            } else {
                records.insert(terminal_id.clone(), record.clone());
            }
            conflict
        };
        #[cfg(windows)]
        if conflict.is_some() {
            if let Some(flow) = self.output_flows.lock().unwrap().remove(&terminal_id) {
                flow.close();
            }
        }
        // The live set grew: sidebar and other list consumers refetch once.
        if let Some(app) = &self.app {
            let _ = app.emit(EVENT_CHANGED, ());
        }

        // Reader: PTY master → scrollback → webview event. The lock is taken
        // per chunk and dropped BEFORE the emit. On Windows parser ACKs bound
        // unread output; a slow WebView2 applies pressure only to its own PTY.
        let reader_record = record.clone();
        let reader_app = self.app.clone();
        let reader_id = terminal_id.clone();
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut chunk = [0u8; 4096];
            #[cfg(windows)]
            let mut startup_query = spawn::StartupQuery::default();
            loop {
                #[cfg(windows)]
                if !output_flow.wait() {
                    break;
                }
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        let data = &chunk[..n];
                        #[cfg(windows)]
                        let data = {
                            let (output, reply) = startup_query.feed(data);
                            if reply {
                                let writer = reader_record
                                    .lock()
                                    .ok()
                                    .and_then(|record| record.writer.clone());
                                if let Some(writer) = writer {
                                    let Ok(mut writer) = writer.lock() else {
                                        break;
                                    };
                                    use std::io::Write;
                                    if writer
                                        .write_all(b"\x1b[1;1R")
                                        .and_then(|_| writer.flush())
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                            }
                            output
                        };
                        if data.is_empty() {
                            continue;
                        }
                        let encoded = {
                            let Ok(mut record) = reader_record.lock() else {
                                break;
                            };
                            let offset = record.scrollback.output_offset;
                            record.scrollback.push(&data);
                            #[cfg(windows)]
                            output_flow.produced(record.scrollback.output_offset);
                            TerminalOutput {
                                offset,
                                data: base64::engine::general_purpose::STANDARD.encode(&data),
                            }
                        };
                        if let Some(app) = &reader_app {
                            let _ = app.emit(&format!("{EVENT_OUTPUT_PREFIX}{reader_id}"), encoded);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });

        // Wait: reap the child so it never lingers as a zombie, then publish
        // the exit as a fact.
        let wait_record = record;
        let wait_app = self.app.clone();
        let wait_id = terminal_id;
        std::thread::spawn(move || {
            let mut child = child;
            let exit_code = child.wait().ok().map(|status| status.exit_code() as i32);
            let bridge = if let Ok(mut record) = wait_record.lock() {
                record.summary.live = false;
                record.summary.exit_code = exit_code;
                record.writer = None;
                record.notification.reset();
                record.identity_bridge.take()
            } else {
                None
            };
            drop(bridge);
            if let Some(app) = &wait_app {
                let _ = app.emit(
                    &format!("{EVENT_EXIT_PREFIX}{wait_id}"),
                    TerminalExit { exit_code },
                );
                let _ = app.emit(EVENT_CHANGED, ());
            }
        });

        if conflict.is_some() {
            Err(other("该会话已有运行终端，已停止重复启动的终端"))
        } else {
            Ok(summary)
        }
    }
}

/// What an embedded New / Resume spawn needs beyond the `AgentCommand`. The
/// launcher builds this; [`spawn_embedded`] consumes it. Kept in this module
/// so the dependency direction stays launcher → terminal.
pub struct EmbeddedTarget<'a> {
    pub registry: &'a TerminalRegistry,
    /// None for a NEW-session launch: the session does not exist yet; the
    /// registry entry starts unbound and binds when its native identity is
    /// available in the ingested Session store.
    pub session_id: Option<&'a str>,
    /// New launches assign these tasks once; Resume preserves current tracking.
    pub initial_workstream_ids: &'a [String],
    /// Prespecified ID for NEW, or the known native ID for Resume. Future
    /// Agent events replace this identity when its TUI switches conversations.
    pub root_agent_session_id: Option<&'a str>,
    pub agent: Agent,
}

/// The embedded spawn step of `launch_prepared`: run the resume command
/// inside a backend PTY instead of an external terminal window. Signature
/// mirrors the launcher's injected `spawn`/`open` functions so tests can
/// substitute it the same way.
pub fn spawn_embedded(
    cmd: &AgentCommand,
    target: &EmbeddedTarget,
) -> Result<crate::platform::launcher::LaunchOutcome> {
    // Capture before spawning: a fast CLI can write its source before register.
    let started_at = now();
    let terminal_id = new_id();
    let prepared_identity = if target.registry.app.is_some() {
        identity::prepare(cmd, target.agent, &terminal_id)?
    } else {
        None
    };
    let runtime_cmd = prepared_identity
        .as_ref()
        .map(|prepared| &prepared.command)
        .unwrap_or(cmd);
    let pty = spawn::PtyPair::open()?;
    let mut wrapped = spawn::wrapped_command(runtime_cmd);
    if let Some(prepared) = &prepared_identity {
        for (name, value) in &prepared.environment {
            wrapped.env(name, value);
        }
    }
    // The webview uses xterm; a GUI launch may have no TERM or inherit "dumb",
    // which makes Codex stop before its lifecycle hooks can run.
    wrapped.env(crate::session_control::CALLER_TERMINAL_ENV, &terminal_id);
    wrapped.env("TERM", "xterm-256color");
    #[cfg(windows)]
    wrapped.env("COLORTERM", "truecolor");
    let child = pty.spawn_command(&wrapped)?;
    let pid = child.process_id();
    let reader = pty
        .take_reader()
        .map_err(|e| crate::error::AppError::AgentLaunchUncertain(e.to_string()))?;
    let writer = pty
        .take_writer()
        .map_err(|e| crate::error::AppError::AgentLaunchUncertain(e.to_string()))?;
    #[cfg(windows)]
    let killer = spawn::process_tree_killer(child.as_ref());
    #[cfg(not(windows))]
    let killer = child.clone_killer();

    let summary = TerminalSummary {
        terminal_id: terminal_id.clone(),
        session_id: target.session_id.map(str::to_string),
        initial_workstream_ids: target.initial_workstream_ids.to_vec(),
        identity_revision: 0,
        activity: TerminalActivity::default(),
        agent: target.agent,
        cwd: cmd.cwd.as_ref().map(|p| p.to_string_lossy().to_string()),
        created_at: started_at,
        live: true,
        exit_code: None,
        session_title: None,
    };

    let bridge = prepared_identity.map(|prepared| std::sync::Arc::new(prepared.bridge));
    let summary = target
        .registry
        .register(
            summary,
            target.root_agent_session_id.map(str::to_string),
            bridge.clone(),
            Scrollback::default(),
            pty.into_master(),
            writer,
            killer,
            reader,
            child,
        )
        .map_err(|e| crate::error::AppError::AgentLaunchUncertain(e.to_string()))?;

    if let (Some(bridge), Some(app)) = (bridge, target.registry.app.clone()) {
        let terminal_id = summary.terminal_id.clone();
        bridge.activate(std::sync::Arc::new(move |event| {
            let registry = app.state::<TerminalRegistry>();
            let state = app.state::<crate::commands::AppState>();
            if let Some(scope) = registry.native_ingest_request(&state.db, &terminal_id, &event) {
                crate::commands::ingestion::enqueue(&app, scope);
            }
        }));
    }

    Ok(crate::platform::launcher::LaunchOutcome {
        launched_via: "内嵌终端".into(),
        command_line: cmd.display(),
        pid,
        terminal_id: Some(summary.terminal_id),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Poll until `f` answers, with a generous deadline — PTY output is
    /// async, tests must not sleep-and-hope.
    fn until<T>(desc: &str, f: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut f = f;
        loop {
            if let Some(v) = f() {
                return v;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {desc}");
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn registry() -> TerminalRegistry {
        TerminalRegistry::new(None)
    }

    /// Real Windows ConPTY and built-in PowerShell are prerequisites, and
    /// startup cursor queries must be answered just as xterm does in the app.
    #[test]
    #[cfg(windows)]
    fn windows_conpty_preserves_mouse_modes_before_agent_raw_input() {
        use std::os::windows::process::CommandExt;
        assert!(std::process::Command::new("node")
            .arg("--version")
            .creation_flags(0x08000000)
            .output()
            .expect("protocol regression requires installed Node.js")
            .status
            .success());
        let registry = registry();
        struct Cleanup<'a>(&'a TerminalRegistry);
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                self.0.kill_all();
            }
        }
        let _cleanup = Cleanup(&registry);
        // Pi's exact order: enter alternate screen and enable mouse, THEN
        // initialize Node raw input. Cooked ConPTY otherwise eats DECSET.
        let cmd = AgentCommand { program: "node".into(), args: vec!["-e".into(),
            "process.stdout.write('\\x1b[?1049h\\x1b[?1000h\\x1b[?1006h'); process.stdin.setRawMode(true); process.stdout.write('MOUSE-READY\\x1b[4;3H\\x1b[?25l'); process.stdin.on('data',d=>process.stdout.write('HEX='+d.toString('hex'))); setInterval(()=>{},1000);".into()], cwd: None };
        let id = spawn_embedded(&cmd, &target(&registry, "windows-mouse"))
            .unwrap()
            .terminal_id
            .unwrap();
        let read = || {
            let s = registry.attach(&id).unwrap();
            registry.acknowledge_output(&id, "test", s.output_offset);
            String::from_utf8(
                base64::engine::general_purpose::STANDARD
                    .decode(s.scrollback)
                    .unwrap(),
            )
            .unwrap()
        };
        let mut answered = false;
        let output = until("mouse mode before raw initialization", || {
            let text = read();
            if !answered && text.contains("\x1b[6n") {
                registry.write(&id, "\x1b[1;1R").unwrap();
                registry.write(&id, "\x1b[I").unwrap();
                answered = true;
            }
            text.contains("MOUSE-READY").then_some(text)
        });
        assert!(
            output.contains("\x1b[?1000h"),
            "ConPTY dropped mouse tracking: {output:?}"
        );
        assert!(
            output.contains("\x1b[?1006h"),
            "ConPTY dropped SGR encoding: {output:?}"
        );
        registry.write(&id, "\x1b[<64;3;4M").unwrap();
        until("SGR wheel delivered to Agent", || {
            read().contains("1b5b3c36343b333b344d").then_some(())
        });
    }

    #[test]
    #[cfg(windows)]
    fn windows_conpty_resize_input_exit_and_process_tree_close() {
        use std::os::windows::process::CommandExt;
        let registry = registry();
        struct Cleanup<'a>(&'a TerminalRegistry);
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                self.0.kill_all();
            }
        }
        let _cleanup = Cleanup(&registry);
        let cmd = AgentCommand {
            program: "powershell".into(),
            args: vec!["-NoProfile".into(), "-Command".into(),
                "$OutputEncoding = [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false); Write-Output ('CHILD=' + $PID); Write-Output 'READY'; $line = [Console]::ReadLine(); Write-Output ('ECHO=' + $line); Write-Output ('SIZE=' + [Console]::WindowWidth + 'x' + [Console]::WindowHeight); Start-Sleep -Seconds 120".into()],
            cwd: None,
        };
        let launched = spawn_embedded(&cmd, &target(&registry, "windows-probe")).unwrap();
        let id = launched.terminal_id.unwrap();
        let output = until("ConPTY child ready", || {
            let snapshot = registry.attach(&id).unwrap();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(snapshot.scrollback)
                .unwrap();
            registry.acknowledge_output(&id, "test", snapshot.output_offset);
            let text = String::from_utf8_lossy(&bytes).to_string();
            if text.contains("\x1b[6n") && !text.contains("READY") {
                registry.write(&id, "\x1b[1;1R").unwrap();
            }
            text.contains("READY").then_some(text)
        });
        let child_pid: u32 = output
            .split("CHILD=")
            .nth(1)
            .unwrap()
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .unwrap();
        registry.resize(&id, 101, 31).unwrap();
        let size = registry.attach(&id).unwrap();
        assert_eq!((size.cols, size.rows), (101, 31));
        registry.write(&id, "中文 input\r").unwrap();
        until("Unicode PTY input echoed by child", || {
            let snapshot = registry.attach(&id).unwrap();
            registry.acknowledge_output(&id, "test", snapshot.output_offset);
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(snapshot.scrollback)
                .unwrap();
            let text = String::from_utf8_lossy(&bytes);
            (text.contains("ECHO=中文 input") && text.contains("SIZE=101x31")).then_some(())
        });
        registry.close(&id).unwrap();
        assert!(registry.attach(&id).is_err());
        until("Agent descendant terminated", || {
            let output = std::process::Command::new("tasklist.exe")
                .args(["/FI", &format!("PID eq {child_pid}"), "/FO", "CSV", "/NH"])
                .creation_flags(0x08000000)
                .output()
                .unwrap();
            assert!(output.status.success());
            (!String::from_utf8_lossy(&output.stdout).contains(&format!("\"{child_pid}\"")))
                .then_some(())
        });
    }

    #[test]
    #[cfg(windows)]
    fn windows_snapshot_lookup_keeps_unviewed_output_running() {
        let registry = registry();
        struct Cleanup<'a>(&'a TerminalRegistry);
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                self.0.kill_all();
            }
        }
        let _cleanup = Cleanup(&registry);
        let cmd = AgentCommand {
            program: "powershell".into(),
            args: vec!["-NoProfile".into(), "-Command".into(),
                "[Console]::Write('READY'); [Console]::ReadLine() | Out-Null; [Console]::Write(('x' * 600000)); [Console]::Write('UNVIEWED-END')".into()],
            cwd: None,
        };
        let id = spawn_embedded(&cmd, &target(&registry, "unviewed-probe"))
            .unwrap()
            .terminal_id
            .unwrap();
        let mut answered = false;
        until("unviewed child ready", || {
            let snapshot = registry.attach(&id).unwrap();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(snapshot.scrollback)
                .unwrap();
            let text = String::from_utf8_lossy(&bytes);
            if !answered && text.contains("\x1b[6n") {
                registry.write(&id, "\x1b[1;1R").unwrap();
                answered = true;
            }
            text.contains("READY").then_some(())
        });
        registry.write(&id, "\r").unwrap();
        until("unviewed process finishes without parser ACK", || {
            registry
                .list()
                .iter()
                .find(|t| t.terminal_id == id && !t.live)
                .map(|_| ())
        });
        until("unviewed output tail drains", || {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(registry.attach(&id).unwrap().scrollback)
                .unwrap();
            String::from_utf8_lossy(&bytes)
                .contains("UNVIEWED-END")
                .then_some(())
        });
    }

    #[test]
    #[cfg(windows)]
    fn windows_conpty_output_backpressure_resumes_and_drains_tail() {
        let registry = registry();
        struct Cleanup<'a>(&'a TerminalRegistry);
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                self.0.kill_all();
            }
        }
        let _cleanup = Cleanup(&registry);
        let cmd = AgentCommand {
            program: "powershell".into(),
            args: vec![
                "-NoProfile".into(),
                "-Command".into(),
                "[Console]::Write(('x' * 600000)); [Console]::Write('DRAINED-END')".into(),
            ],
            cwd: None,
        };
        let id = spawn_embedded(&cmd, &target(&registry, "flow-probe"))
            .unwrap()
            .terminal_id
            .unwrap();
        registry.attach_output(&id, "test").unwrap();
        let mut answered = false;
        let stalled = until("high output watermark", || {
            let snapshot = registry.attach(&id).unwrap();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&snapshot.scrollback)
                .unwrap();
            if !answered && bytes.windows(4).any(|bytes| bytes == b"\x1b[6n") {
                registry.write(&id, "\x1b[1;1R").unwrap();
                answered = true;
            }
            (snapshot.output_offset >= 256 * 1024).then_some(snapshot.output_offset)
        });
        assert!(
            stalled <= 256 * 1024 + 4096,
            "reader exceeded its bounded credit: {stalled}"
        );
        // Observe progress without acquiring a new output receiver or granting
        // replay credit. Read-only snapshot lookups must not alter the flow.
        std::thread::sleep(Duration::from_millis(50));
        let offset = registry
            .records
            .lock()
            .unwrap()
            .get(&id)
            .unwrap()
            .lock()
            .unwrap()
            .scrollback
            .output_offset;
        assert_eq!(offset, stalled);
        registry.acknowledge_output(&id, "test", stalled);
        until("final bytes after backpressure resumes", || {
            let snapshot = registry.attach(&id).unwrap();
            registry.acknowledge_output(&id, "test", snapshot.output_offset);
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(snapshot.scrollback)
                .unwrap();
            String::from_utf8_lossy(&bytes)
                .contains("DRAINED-END")
                .then_some(())
        });
    }

    /// `sh` exists on every CI this suite runs on (macOS; the spawn tests are
    /// unix-gated below — ConPTY coverage rides on the same code paths).
    #[cfg(unix)]
    fn sh(script: &str) -> AgentCommand {
        AgentCommand {
            program: "/bin/sh".into(),
            args: ["-c".to_string(), script.to_string()].to_vec(),
            cwd: None,
        }
    }

    fn target<'a>(registry: &'a TerminalRegistry, session: &'a str) -> EmbeddedTarget<'a> {
        EmbeddedTarget {
            registry,
            session_id: Some(session),
            initial_workstream_ids: &[],
            root_agent_session_id: None,
            agent: Agent::Codex,
        }
    }

    #[derive(Debug)]
    struct NoopChildKiller;

    impl portable_pty::ChildKiller for NoopChildKiller {
        fn kill(&mut self) -> std::io::Result<()> {
            Ok(())
        }

        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(Self)
        }
    }

    fn binding_db() -> crate::storage::Db {
        let dir = std::env::temp_dir().join(format!("noending-term-binding-{}", new_id()));
        let db = crate::storage::Db::open(&dir.join("db.sqlite")).unwrap();
        db.write()
            .execute("DELETE FROM ingest_sources", [])
            .unwrap();
        let p = crate::domain::Project {
            id: "native-project".into(),
            name: "Native".into(),
            git_id: None,
            name_customized: false,
            visibility: "normal".into(),
            created_at: now(),
            updated_at: now(),
        };
        db.upsert_project(&p).unwrap();
        crate::storage::workspace::insert_workspace_path_conn(&db.write(), &fixture_cwd(), &p.id)
            .unwrap();
        db
    }

    fn fixture_cwd() -> String {
        if cfg!(windows) { r"C:\repo" } else { "/repo" }.into()
    }

    /// Identity tests need a terminal record, not a running CLI. An empty
    /// native PTY supplies the platform handle without spawning a child.
    fn identity_terminal(
        registry: &TerminalRegistry,
        agent: Agent,
        native_id: Option<&str>,
        session_id: Option<&str>,
    ) -> String {
        identity_terminal_with_owner(registry, agent, native_id, session_id, None)
    }

    fn identity_terminal_with_owner(
        registry: &TerminalRegistry,
        agent: Agent,
        native_id: Option<&str>,
        session_id: Option<&str>,
        owner: Option<&str>,
    ) -> String {
        let pty = spawn::PtyPair::open().unwrap();
        let writer = pty.take_writer().unwrap();
        let terminal_id = new_id();
        let record = TerminalRecord {
            summary: TerminalSummary {
                terminal_id: terminal_id.clone(),
                session_id: session_id.map(str::to_string),
                initial_workstream_ids: owner.into_iter().map(str::to_string).collect(),
                identity_revision: 0,
                activity: TerminalActivity::default(),
                agent,
                cwd: Some(fixture_cwd()),
                created_at: now(),
                live: true,
                exit_code: None,
                session_title: None,
            },
            root_agent_session_id: native_id.map(str::to_string),
            pending_tracking: owner.map(|owner| PendingTracking {
                workstream_ids: vec![owner.into()],
                native_id: native_id.map(str::to_string),
            }),
            identity_bridge: None,
            scrollback: Scrollback::default(),
            cols: INITIAL_COLS,
            rows: INITIAL_ROWS,
            writer: Some(std::sync::Arc::new(Mutex::new(writer))),
            notification: notifications::NotificationState::default(),
            observed_inputs: HashSet::new(),
            runtime_files: Vec::new(),
            started_turns: 0,
            master: pty.into_master(),
            killer: Box::new(NoopChildKiller),
        };
        registry
            .records
            .lock()
            .unwrap()
            .insert(terminal_id.clone(), std::sync::Arc::new(Mutex::new(record)));
        terminal_id
    }

    #[test]
    fn current_terminal_identity_waits_for_its_exact_binding_and_rejects_exited_or_stale_context() {
        let db = binding_db();
        let registry = TerminalRegistry::new(None);
        let terminal = identity_terminal(&registry, Agent::Codex, Some("current-native"), None);
        let other = ingested_native(&db, Agent::Codex, "other-native");
        assert_eq!(
            crate::session_control::sender_session(&registry, &db, Some(&terminal), None)
                .unwrap_err()
                .code,
            "sender_unbound"
        );
        let current = ingested_native(&db, Agent::Codex, "current-native");
        assert_eq!(
            crate::session_control::sender_session(&registry, &db, Some(&terminal), None).unwrap(),
            Some(current.id)
        );
        assert_eq!(
            crate::session_control::sender_session(
                &registry,
                &db,
                Some(&terminal),
                Some(&other.id),
            )
            .unwrap_err()
            .code,
            "sender_identity_changed"
        );
        registry.records.lock().unwrap()[&terminal]
            .lock()
            .unwrap()
            .summary
            .live = false;
        assert_eq!(
            crate::session_control::sender_session(&registry, &db, Some(&terminal), None)
                .unwrap_err()
                .code,
            "sender_unavailable"
        );
        registry.close(&terminal);
        assert_eq!(
            crate::session_control::sender_session(&registry, &db, Some(&terminal), None)
                .unwrap_err()
                .code,
            "sender_unavailable"
        );
    }

    fn native_event(native_id: &str) -> identity::NativeIdentityEvent {
        identity::NativeIdentityEvent {
            native_id: native_id.into(),
            cwd: None,
            source_path: None,
            source: None,
            activity: None,
            input: None,
        }
    }

    fn ingested_native(
        db: &crate::storage::Db,
        agent: Agent,
        native_id: &str,
    ) -> crate::domain::Session {
        let ts = now();
        let id = db
            .upsert_logical_session(
                agent,
                native_id,
                Some(native_id),
                None,
                Some(&fixture_cwd()),
                None,
                None,
                Some(&ts),
                Some(&ts),
                "test_root",
                "/tmp/source.jsonl",
                &serde_json::json!({}),
            )
            .unwrap()
            .0;
        let path = db
            .list_workspace_paths()
            .unwrap()
            .into_iter()
            .find(|p| p.canonical_path == fixture_cwd())
            .unwrap();
        crate::storage::session_paths::attach_session_workspace_path_conn(
            &db.write(),
            &id,
            Some(&path.id),
        )
        .unwrap();
        db.get_session(&id).unwrap().unwrap()
    }

    fn selected_task(db: &crate::storage::Db, title: &str) -> String {
        let id = new_id();
        db.upsert_workstream(&crate::domain::Workstream {
            id: id.clone(),
            title: title.into(),
            description: String::new(),
            visibility: "normal".into(),
            created_at: now(),
            updated_at: now(),
        })
        .unwrap();
        crate::workspace::workstream::set_workstream_projects(db, &id, &["native-project".into()])
            .unwrap();
        id
    }

    fn owner(db: &crate::storage::Db, session: &crate::domain::Session) -> Option<String> {
        db.get_session(&session.id)
            .unwrap()
            .unwrap()
            .tracked_workstream_ids
            .into_iter()
            .next()
    }

    #[test]
    fn activity_is_per_terminal_and_rejects_stale_turns_identities_and_read_receipts() {
        use activity::{ActivitySignal, ActivityState::*};
        let db = binding_db();
        let registry = registry();
        let first = identity_terminal(&registry, Agent::Codex, Some("a"), None);
        let peer = identity_terminal(&registry, Agent::Codex, Some("peer-native"), None);
        let notify = |id: &str, native: &str, turn: &str, state, starts_turn| {
            let mut event = native_event(native);
            event.activity = Some(ActivitySignal {
                state,
                turn_id: Some(turn.into()),
                starts_turn,
            });
            registry.report_native_identity(&db, id, &event);
        };
        notify(&first, "a", "1", Working, true);
        notify(&peer, "peer-native", "peer", Working, true);
        notify(&first, "a", "1", Waiting, false);
        assert_eq!(
            registry.attach(&first).unwrap().summary.activity.state,
            Waiting
        );
        notify(&first, "a", "1", Completed, false);
        let completed = registry.attach(&first).unwrap().summary.activity;
        assert!(completed.unread);
        assert_eq!(
            registry.attach(&peer).unwrap().summary.activity.state,
            Working
        );
        registry.mark_activity_seen(&first, completed.revision);
        assert!(!registry.attach(&first).unwrap().summary.activity.unread);
        notify(&first, "a", "1", Completed, false);
        assert_eq!(
            registry.attach(&first).unwrap().summary.activity.state,
            Idle
        );
        assert!(!registry.attach(&first).unwrap().summary.activity.unread);
        notify(&first, "a", "2", Working, true);
        notify(&first, "a", "1", Completed, false);
        assert_eq!(
            registry.attach(&first).unwrap().summary.activity.state,
            Working
        );
        notify(&first, "a", "2", Completed, false);
        registry.mark_activity_seen(&first, completed.revision);
        assert!(registry.attach(&first).unwrap().summary.activity.unread);
        // Even the correct old receipt cannot clear a different native session.
        let receipt = registry.attach(&first).unwrap().summary.activity.revision;
        registry.report_native_identity(&db, &first, &native_event("b"));
        notify(&first, "a", "2", Completed, false);
        assert_eq!(
            registry.attach(&first).unwrap().summary.activity.state,
            Unknown
        );
        notify(&first, "b", "3", Working, true);
        notify(&first, "b", "3", Completed, false);
        registry.mark_activity_seen(&first, receipt);
        assert!(registry.attach(&first).unwrap().summary.activity.unread);
        registry.close(&first);
        notify(&first, "b", "3", Completed, false);
        assert!(registry.attach(&first).is_err());
        assert_eq!(
            registry.attach(&peer).unwrap().summary.activity.state,
            Working
        );
    }

    #[test]
    fn launch_task_binds_by_agent_and_native_id_once_for_all_four_clis() {
        let registry = registry();
        let db = binding_db();
        for agent in [
            Agent::Codex,
            Agent::ClaudeCode,
            Agent::Pi,
            Agent::Antigravity,
        ] {
            let first_task = selected_task(&db, "Initialtask");
            let second_task = selected_task(&db, "Concurrenttask");
            let initial = matches!(agent, Agent::ClaudeCode | Agent::Pi).then_some("native-a");
            let terminal =
                identity_terminal_with_owner(&registry, agent, initial, None, Some(&first_task));
            let other = identity_terminal_with_owner(
                &registry,
                agent,
                Some("native-b"),
                None,
                Some(&second_task),
            );
            // An empty TUI may not report a native identity until the user
            // submits inside it. Other sessions can be ingested meanwhile.
            let b = ingested_native(&db, agent, "native-b");
            attach_fixture_paths(&db);
            registry.bind_discovered(&db);
            assert!(registry
                .attach(&terminal)
                .unwrap()
                .summary
                .session_id
                .is_none());
            assert_eq!(owner(&db, &b).as_deref(), Some(second_task.as_str()));
            assert_eq!(
                registry
                    .attach(&other)
                    .unwrap()
                    .summary
                    .session_id
                    .as_deref(),
                Some(b.id.as_str())
            );
            assert_eq!(
                registry
                    .attach(&terminal)
                    .unwrap()
                    .summary
                    .initial_workstream_ids,
                vec![first_task.clone()]
            );
            registry
                .report_native_identity(&db, &terminal, &native_event("native-a"))
                .unwrap();
            let a = ingested_native(&db, agent, "native-a");
            assert_eq!(
                owner(&db, &a),
                None,
                "ingestion alone does not infer ownership from cwd or time"
            );
            attach_fixture_paths(&db);
            registry.bind_discovered(&db);
            assert_eq!(owner(&db, &a).as_deref(), Some(first_task.as_str()));
            assert_eq!(
                registry
                    .attach(&terminal)
                    .unwrap()
                    .summary
                    .initial_workstream_ids,
                Vec::<String>::new()
            );
            assert!(crate::search::sessions::page(
                &db,
                "Initialtask",
                &crate::search::sessions::SessionFilter {
                    scope: "all".into(),
                    ..Default::default()
                },
                50,
                0,
                None
            )
            .unwrap()
            .0
            .iter()
            .any(|row| row.hit.kind == "session" && row.hit.ref_id == a.id));
            // Clearing ownership explicitly cannot be undone by another lifecycle notice.
            crate::storage::seed_tracking(&db, &a.id, None).unwrap();
            registry.report_native_identity(&db, &terminal, &native_event("native-a"));
            attach_fixture_paths(&db);
            registry.bind_discovered(&db);
            assert_eq!(owner(&db, &a), None);
            registry.close(&other).unwrap();
            // Switching to an unoccupied conversation preserves its current task.
            let current = registry
                .report_native_identity(&db, &terminal, &native_event("native-b"))
                .unwrap();
            assert_eq!(current.initial_workstream_ids, Vec::<String>::new());
            let c = ingested_native(&db, agent, "native-c");
            registry.report_native_identity(&db, &terminal, &native_event("native-c"));
            assert_eq!(owner(&db, &c), None);
        }
    }

    #[test]
    fn native_notice_after_unchanged_source_scan_still_assigns_the_launch_task() {
        let registry = registry();
        let db = binding_db();
        let task = selected_task(&db, "Afterdiscovery");
        let native = new_id();
        let terminal =
            identity_terminal_with_owner(&registry, Agent::Codex, None, None, Some(&task));
        let root = std::env::temp_dir().join(format!("noending-owner-rollout-{}", new_id()));
        std::fs::create_dir_all(&root).unwrap();
        let source = db
            .add_ingest_source(Agent::Codex, &root.to_string_lossy())
            .unwrap();
        let meta = serde_json::json!({"ordinal": 0, "type": "session_meta", "timestamp": now(), "payload": {
            "id": native, "timestamp": now(), "cwd": fixture_cwd(),
        }});
        let message = serde_json::json!({"ordinal": 1, "type": "response_item", "timestamp": now(), "payload": {
            "type": "message", "id": "first", "role": "user", "content": [{"type": "input_text", "text": "真实首条消息"}],
        }});
        std::fs::write(
            root.join(format!("rollout-2026-10-10T12-00-00-{native}.jsonl")),
            format!("{meta}\n{message}\n"),
        )
        .unwrap();
        let (discovered, messages) =
            crate::ingestion::reconcile_source(&db, &source, &|_| {}).unwrap();
        assert_eq!((discovered, messages), (1, 1));
        let session = db
            .find_session_by_root_agent_id(Agent::Codex, &native)
            .unwrap()
            .unwrap();
        attach_fixture_paths(&db);
        registry.bind_discovered(&db);
        assert_eq!(owner(&db, &session), None);
        assert!(registry
            .attach(&terminal)
            .unwrap()
            .summary
            .session_id
            .is_none());
        assert_eq!(
            crate::ingestion::reconcile_source(&db, &source, &|_| {}).unwrap(),
            (0, 0)
        );
        let bound = registry
            .report_native_identity(&db, &terminal, &native_event(&native))
            .unwrap();
        assert_eq!(bound.session_id.as_deref(), Some(session.id.as_str()));
        assert_eq!(owner(&db, &session).as_deref(), Some(task.as_str()));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn native_sync_binds_task_without_importing_neighbors() {
        let registry = registry();
        let db = binding_db();
        let task = selected_task(&db, "Native history");
        let native = new_id();
        let unrelated = new_id();
        let terminal =
            identity_terminal_with_owner(&registry, Agent::Codex, None, None, Some(&task));
        let root = std::env::temp_dir().join(format!("noending-native-sync-{}", new_id()));
        std::fs::create_dir_all(&root).unwrap();
        db.add_ingest_source(Agent::Codex, &root.to_string_lossy())
            .unwrap();
        let write_rollout = |id: &str, text: Option<&str>| {
            let path = root.join(format!("rollout-2026-10-10T12-00-00-{id}.jsonl"));
            let meta = serde_json::json!({"ordinal":0,"type":"session_meta","timestamp":now(),"payload":{"id":id,"timestamp":now(),"cwd":fixture_cwd()}});
            let message = text.map(|text| serde_json::json!({"ordinal":1,"type":"response_item","timestamp":now(),"payload":{"type":"message","id":"first","role":"user","content":[{"type":"input_text","text":text}]}}).to_string()).unwrap_or_default();
            std::fs::write(&path, format!("{meta}\n{message}\n")).unwrap();
            path
        };
        let path = write_rollout(&native, None);
        write_rollout(&unrelated, Some("unrelated history"));
        let event = native_event(&native);
        registry
            .report_native_identity(&db, &terminal, &event)
            .unwrap();
        crate::ingestion::reconcile_native(
            &db,
            Agent::Codex,
            &native,
            Some(&path.to_string_lossy()),
            &|_| {},
        )
        .unwrap();
        attach_fixture_paths(&db);
        registry.bind_discovered(&db);
        assert!(
            db.find_session_by_root_agent_id(Agent::Codex, &native)
                .unwrap()
                .is_none(),
            "metadata alone must wait for a real message"
        );
        write_rollout(&native, Some("first real user message"));
        assert_eq!(
            crate::ingestion::reconcile_native(
                &db,
                Agent::Codex,
                &native,
                Some(&path.to_string_lossy()),
                &|_| {}
            )
            .unwrap(),
            (1, 1)
        );
        attach_fixture_paths(&db);
        registry.bind_discovered(&db);
        let session = db
            .find_session_by_root_agent_id(Agent::Codex, &native)
            .unwrap()
            .unwrap();
        assert_eq!(
            registry
                .attach(&terminal)
                .unwrap()
                .summary
                .session_id
                .as_deref(),
            Some(session.id.as_str())
        );
        assert_eq!(owner(&db, &session).as_deref(), Some(task.as_str()));
        assert!(db
            .find_session_by_root_agent_id(Agent::Codex, &unrelated)
            .unwrap()
            .is_none());
        assert_eq!(
            crate::ingestion::reconcile_native(&db, Agent::Codex, &native, None, &|_| {}).unwrap(),
            (1, 0)
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn binding_preserves_explicit_owner_and_handles_deleted_launch_task() {
        let registry = registry();
        let db = binding_db();
        let chosen = selected_task(&db, "Chosen");
        let explicit = selected_task(&db, "Explicit");
        let terminal = identity_terminal_with_owner(
            &registry,
            Agent::Codex,
            Some("existing"),
            None,
            Some(&chosen),
        );
        let session = ingested_native(&db, Agent::Codex, "existing");
        crate::storage::seed_tracking(&db, &session.id, Some(&explicit)).unwrap();
        attach_fixture_paths(&db);
        registry.bind_discovered(&db);
        assert_eq!(
            db.get_session(&session.id)
                .unwrap()
                .unwrap()
                .tracked_workstream_ids
                .len(),
            2
        );
        assert_eq!(
            registry
                .attach(&terminal)
                .unwrap()
                .summary
                .initial_workstream_ids,
            Vec::<String>::new()
        );
        let terminal = identity_terminal_with_owner(
            &registry,
            Agent::Pi,
            Some("deleted"),
            None,
            Some(&chosen),
        );
        db.delete_workstream(&chosen).unwrap();
        let session = ingested_native(&db, Agent::Pi, "deleted");
        attach_fixture_paths(&db);
        registry.bind_discovered(&db);
        let current = registry.attach(&terminal).unwrap().summary;
        assert_eq!(current.session_id.as_deref(), Some(session.id.as_str()));
        assert_eq!(current.initial_workstream_ids, Vec::<String>::new());
        assert_eq!(owner(&db, &session), None);
    }

    #[test]
    fn failed_ownership_write_retries_without_publishing_partial_binding() {
        let registry = registry();
        let db = binding_db();
        let task = selected_task(&db, "Retrytask");
        let terminal =
            identity_terminal_with_owner(&registry, Agent::Codex, Some("retry"), None, Some(&task));
        let session = ingested_native(&db, Agent::Codex, "retry");
        db.write().execute_batch("CREATE TRIGGER reject_owner BEFORE INSERT ON session_workstreams BEGIN SELECT RAISE(ABORT, 'test failure'); END;").unwrap();
        attach_fixture_paths(&db);
        registry.bind_discovered(&db);
        assert!(registry
            .attach(&terminal)
            .unwrap()
            .summary
            .session_id
            .is_none());
        assert_eq!(owner(&db, &session), None);
        assert!(!crate::search::sessions::page(
            &db,
            "Retrytask",
            &crate::search::sessions::SessionFilter {
                scope: "all".into(),
                ..Default::default()
            },
            50,
            0,
            None
        )
        .unwrap()
        .0
        .iter()
        .any(|row| row.hit.kind == "session" && row.hit.ref_id == session.id));
        db.write()
            .execute_batch("DROP TRIGGER reject_owner;")
            .unwrap();
        attach_fixture_paths(&db);
        registry.bind_discovered(&db);
        assert_eq!(
            registry
                .attach(&terminal)
                .unwrap()
                .summary
                .session_id
                .as_deref(),
            Some(session.id.as_str())
        );
        assert_eq!(owner(&db, &session).as_deref(), Some(task.as_str()));
    }

    #[test]
    fn launch_task_is_lost_on_close_restart_or_switch_before_binding() {
        let db = binding_db();
        let task = selected_task(&db, "Losttask");
        let registry = registry();
        let closed =
            identity_terminal_with_owner(&registry, Agent::Pi, Some("closed"), None, Some(&task));
        let delayed = registry.attach(&closed).unwrap().summary;
        registry.close(&closed).unwrap();
        let session = ingested_native(&db, Agent::Pi, "closed");
        registry.finish_native_binding(&db, &delayed, "closed", &session);
        attach_fixture_paths(&db);
        registry.bind_discovered(&db);
        assert_eq!(owner(&db, &session), None);
        let switched =
            identity_terminal_with_owner(&registry, Agent::Pi, Some("original"), None, Some(&task));
        registry.report_native_identity(&db, &switched, &native_event("switched"));
        let original = ingested_native(&db, Agent::Pi, "original");
        let later = ingested_native(&db, Agent::Pi, "switched");
        attach_fixture_paths(&db);
        registry.bind_discovered(&db);
        assert_eq!(owner(&db, &original), None);
        assert_eq!(owner(&db, &later), None);
        assert_eq!(
            registry
                .attach(&switched)
                .unwrap()
                .summary
                .session_id
                .as_deref(),
            Some(later.id.as_str())
        );
        identity_terminal_with_owner(&registry, Agent::Codex, Some("restart"), None, Some(&task));
        drop(registry);
        let session = ingested_native(&db, Agent::Codex, "restart");
        TerminalRegistry::new(None).bind_discovered(&db);
        assert_eq!(owner(&db, &session), None);
    }

    #[cfg(unix)]
    #[test]
    fn spawned_new_terminal_retains_task_until_discovery_even_after_agent_exits() {
        let registry = registry();
        let db = binding_db();
        let task = selected_task(&db, "Actualspawn");
        let second_task = selected_task(&db, "Secondtask");
        let outcome = spawn_embedded(
            &sh("exit 0"),
            &EmbeddedTarget {
                registry: &registry,
                session_id: None,
                initial_workstream_ids: &[task.clone(), second_task.clone()],
                root_agent_session_id: Some("spawned-native"),
                agent: Agent::Pi,
            },
        )
        .unwrap();
        let id = outcome.terminal_id.unwrap();
        let initial = registry.attach(&id).unwrap().summary;
        assert_eq!(
            initial.initial_workstream_ids,
            vec![task.clone(), second_task.clone()]
        );
        assert!(initial.session_id.is_none());
        until("spawned terminal to exit", || {
            registry
                .attach(&id)
                .ok()
                .filter(|snapshot| !snapshot.summary.live)
        });
        let session = ingested_native(&db, Agent::Pi, "spawned-native");
        attach_fixture_paths(&db);
        registry.bind_discovered(&db);
        assert_eq!(
            db.get_session(&session.id)
                .unwrap()
                .unwrap()
                .tracked_workstream_ids
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            [task, second_task]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
        );
        assert_eq!(
            registry.attach(&id).unwrap().summary.session_id.as_deref(),
            Some(session.id.as_str())
        );
        registry.close(&id).unwrap();
    }

    #[test]
    fn native_discovery_respects_pending_startup_then_allows_one_binding() {
        let registry = registry();
        let db = binding_db();
        let session = ingested_native(&db, Agent::Codex, "shared-native");
        let pending = registry.reserve_resume(&session.id).unwrap();
        let conflicting = identity_terminal(&registry, Agent::Codex, Some("shared-native"), None);
        registry.bind_discovered(&db);
        let stopped = registry.attach(&conflicting).unwrap().summary;
        assert!(!stopped.live);
        assert!(stopped.session_id.is_none());
        drop(pending);
        let owner = identity_terminal(&registry, Agent::Codex, Some("shared-native"), None);
        registry.bind_discovered(&db);
        assert_eq!(
            registry.for_session(&session.id).unwrap().terminal_id,
            owner
        );
        let duplicate = identity_terminal(&registry, Agent::Codex, Some("shared-native"), None);
        registry.bind_discovered(&db);
        assert!(!registry.attach(&duplicate).unwrap().summary.live);
        assert!(registry
            .attach(&duplicate)
            .unwrap()
            .summary
            .session_id
            .is_none());
        assert!(registry.attach(&owner).unwrap().summary.live);
    }

    #[test]
    fn native_switch_to_an_occupied_session_stops_incoming_terminal_and_preserves_owner() {
        let registry = registry();
        let db = binding_db();
        let a = ingested_native(&db, Agent::Pi, "native-a");
        let b = ingested_native(&db, Agent::Pi, "native-b");
        let incoming = identity_terminal(&registry, Agent::Pi, Some("native-a"), Some(&a.id));
        let owner = identity_terminal(&registry, Agent::Pi, Some("native-b"), Some(&b.id));
        assert!(registry
            .report_native_identity(&db, &incoming, &native_event("native-b"))
            .is_none());
        assert!(!registry.attach(&incoming).unwrap().summary.live);
        assert_eq!(registry.for_session(&b.id).unwrap().terminal_id, owner);
        assert!(registry.attach(&owner).unwrap().summary.live);
        registry.close(&owner).unwrap();
        let free = identity_terminal(&registry, Agent::Pi, None, None);
        assert_eq!(
            registry
                .report_native_identity(&db, &free, &native_event("native-b"))
                .unwrap()
                .session_id
                .as_deref(),
            Some(b.id.as_str())
        );
    }

    #[test]
    fn native_switch_to_uningested_b_clears_a_and_waits_only_for_b() {
        let registry = registry();
        let db = binding_db();
        for agent in [Agent::ClaudeCode, Agent::Antigravity] {
            let a = ingested_native(&db, agent, "native-a");
            let terminal = identity_terminal(&registry, agent, Some("native-a"), Some(&a.id));
            let waiting = registry
                .report_native_identity(&db, &terminal, &native_event("native-b"))
                .unwrap();
            assert!(waiting.session_id.is_none());
            assert!(waiting.session_title.is_none());
            assert!(registry.for_session(&a.id).is_none());

            attach_fixture_paths(&db);
            registry.bind_discovered(&db);
            let still_waiting = registry.attach(&terminal).unwrap().summary;
            assert!(still_waiting.session_id.is_none());
            assert_eq!(still_waiting.identity_revision, waiting.identity_revision);
            let b = ingested_native(&db, agent, "native-b");
            attach_fixture_paths(&db);
            registry.bind_discovered(&db);
            let bound = registry.attach(&terminal).unwrap().summary;
            assert_eq!(bound.session_id.as_deref(), Some(b.id.as_str()));
            assert!(bound.identity_revision > waiting.identity_revision);
            assert!(registry.for_session(&a.id).is_none());
        }
    }

    #[test]
    fn late_turn_completion_cannot_restore_a_previous_native_identity() {
        let registry = registry();
        let db = binding_db();
        for (agent, completion_source) in [
            (Agent::ClaudeCode, "Stop"),
            (Agent::Codex, "Stop"),
            (Agent::Pi, "agent_end"),
            (Agent::Antigravity, "Stop"),
        ] {
            let a = ingested_native(&db, agent, "native-a");
            let b = ingested_native(&db, agent, "native-b");
            let terminal = identity_terminal(&registry, agent, Some("native-a"), Some(&a.id));
            let current = registry
                .report_native_identity(&db, &terminal, &native_event("native-b"))
                .unwrap();
            assert_eq!(current.session_id.as_deref(), Some(b.id.as_str()));

            let mut stale_completion = native_event("native-a");
            stale_completion.source = Some(completion_source.into());
            stale_completion.cwd = Some(format!("{}/old-cwd", fixture_cwd()));
            assert!(registry
                .report_native_identity(&db, &terminal, &stale_completion)
                .is_none());
            let after_stale = registry.attach(&terminal).unwrap().summary;
            assert_eq!(after_stale.session_id, current.session_id);
            assert_eq!(after_stale.identity_revision, current.identity_revision);
            assert_eq!(after_stale.cwd, current.cwd);

            let mut current_completion = native_event("native-b");
            current_completion.source = Some(completion_source.into());
            let refreshed = registry
                .report_native_identity(&db, &terminal, &current_completion)
                .expect("completion for the current native ID still refreshes ingestion");
            assert_eq!(refreshed.session_id, current.session_id);
            assert_eq!(refreshed.identity_revision, current.identity_revision);
        }
    }

    #[test]
    fn repeating_the_same_native_identity_does_not_advance_revision() {
        let registry = registry();
        let db = binding_db();
        let a = ingested_native(&db, Agent::Codex, "native-a");
        let terminal = identity_terminal(&registry, Agent::Codex, Some("native-a"), Some(&a.id));
        let initial = registry.attach(&terminal).unwrap().summary;
        let repeated = registry
            .report_native_identity(&db, &terminal, &native_event("native-a"))
            .unwrap();
        assert_eq!(repeated.identity_revision, initial.identity_revision);
        assert_eq!(repeated.session_id, initial.session_id);

        let waiting = registry
            .report_native_identity(&db, &terminal, &native_event("not-ingested"))
            .unwrap();
        let repeated_waiting = registry
            .report_native_identity(&db, &terminal, &native_event("not-ingested"))
            .unwrap();
        assert!(repeated_waiting.session_id.is_none());
        assert_eq!(
            repeated_waiting.identity_revision,
            waiting.identity_revision
        );
    }

    #[test]
    fn delayed_native_lookup_cannot_overwrite_a_newer_identity() {
        let registry = registry();
        let db = binding_db();
        let a = ingested_native(&db, Agent::Codex, "native-a");
        let terminal = identity_terminal(&registry, Agent::Codex, Some("native-a"), None);
        let old_lookup = registry.attach(&terminal).unwrap().summary;
        let current = registry
            .report_native_identity(&db, &terminal, &native_event("native-b"))
            .unwrap();
        assert!(current.session_id.is_none());

        registry.finish_native_binding(&db, &old_lookup, "native-a", &a);
        let after_old_lookup = registry.attach(&terminal).unwrap().summary;
        assert!(after_old_lookup.session_id.is_none());
        assert_eq!(
            after_old_lookup.identity_revision,
            current.identity_revision
        );
        let b = ingested_native(&db, Agent::Codex, "native-b");
        attach_fixture_paths(&db);
        registry.bind_discovered(&db);
        assert_eq!(
            registry
                .attach(&terminal)
                .unwrap()
                .summary
                .session_id
                .as_deref(),
            Some(b.id.as_str())
        );
    }

    #[test]
    fn delayed_native_lookup_is_rejected_after_a_b_a_identity_changes() {
        let registry = registry();
        let db = binding_db();
        let a = ingested_native(&db, Agent::Codex, "native-a");
        let terminal = identity_terminal(&registry, Agent::Codex, Some("native-a"), None);
        let old_lookup = registry.attach(&terminal).unwrap().summary;
        // The old read finished before a purge; its callback is delivered after
        // A -> B -> A, while the latest A still has no current source row.
        db.write()
            .execute(
                "DELETE FROM sessions WHERE id = ?1",
                rusqlite::params![a.id],
            )
            .unwrap();
        registry
            .report_native_identity(&db, &terminal, &native_event("native-b"))
            .unwrap();
        let current = registry
            .report_native_identity(&db, &terminal, &native_event("native-a"))
            .unwrap();
        assert!(current.session_id.is_none());
        assert!(current.identity_revision > old_lookup.identity_revision);

        registry.finish_native_binding(&db, &old_lookup, "native-a", &a);
        let after_old_lookup = registry.attach(&terminal).unwrap().summary;
        assert!(
            after_old_lookup.session_id.is_none(),
            "matching native IDs do not bypass the revision check"
        );
        assert_eq!(
            after_old_lookup.identity_revision,
            current.identity_revision
        );
        let new_a = ingested_native(&db, Agent::Codex, "native-a");
        attach_fixture_paths(&db);
        registry.bind_discovered(&db);
        assert_eq!(
            registry
                .attach(&terminal)
                .unwrap()
                .summary
                .session_id
                .as_deref(),
            Some(new_a.id.as_str())
        );
    }

    #[test]
    fn unknown_native_identity_does_not_infer_from_prompt_cwd_or_time() {
        let registry = registry();
        let db = binding_db();
        let source = ingested_native(&db, Agent::Codex, "external-native");
        let prompt = format!("{}\n完整首条消息", "long prompt".repeat(100));
        db.commit_ingest(
            &source.id,
            &[crate::domain::ParsedSessionMessage {
                source_message_id: Some("first".into()),
                ts: source.started_at.clone(),
                role: crate::domain::SessionMessageRole::User,
                content: prompt.clone(),
            }],
            &crate::domain::SourceCursorUpdate {
                file_identity: "external-native".into(),
                generation: 0,
                byte_offset: 100,
                last_seen_size: 100,
                mtime: None,
                start_byte_offset: 0,
                prefix_hash: String::new(),
            },
        )
        .unwrap();
        let ts = source.started_at.as_ref().unwrap();
        for native_id in [None, Some("not-in-the-store")] {
            let terminal = identity_terminal(&registry, Agent::Codex, native_id, None);
            {
                let records = registry.records.lock().unwrap();
                let mut record = records.get(&terminal).unwrap().lock().unwrap();
                record.summary.created_at = ts.clone();
                record.scrollback.push(prompt.as_bytes());
            }
            attach_fixture_paths(&db);
            registry.bind_discovered(&db);
            let summary = registry.attach(&terminal).unwrap().summary;
            assert!(summary.session_id.is_none());
            assert_eq!(summary.identity_revision, 0);
        }
        assert!(registry.for_session(&source.id).is_none());
    }

    #[test]
    fn native_identity_lookups_are_scoped_to_the_terminal_agent() {
        let registry = registry();
        let db = binding_db();
        let codex = ingested_native(&db, Agent::Codex, "same-native");
        let claude = ingested_native(&db, Agent::ClaudeCode, "same-native");
        let codex_terminal = identity_terminal(&registry, Agent::Codex, Some("same-native"), None);
        let claude_terminal =
            identity_terminal(&registry, Agent::ClaudeCode, Some("same-native"), None);
        let pi_terminal = identity_terminal(&registry, Agent::Pi, Some("same-native"), None);
        attach_fixture_paths(&db);
        registry.bind_discovered(&db);
        assert_eq!(
            registry
                .attach(&codex_terminal)
                .unwrap()
                .summary
                .session_id
                .as_deref(),
            Some(codex.id.as_str())
        );
        assert_eq!(
            registry
                .attach(&claude_terminal)
                .unwrap()
                .summary
                .session_id
                .as_deref(),
            Some(claude.id.as_str())
        );
        let expected_pi = registry.attach(&pi_terminal).unwrap().summary;
        registry.finish_native_binding(&db, &expected_pi, "same-native", &codex);
        assert!(registry
            .attach(&pi_terminal)
            .unwrap()
            .summary
            .session_id
            .is_none());
    }

    #[test]
    fn native_notifications_and_delayed_lookups_cannot_resurrect_a_closed_terminal() {
        let registry = registry();
        let db = binding_db();
        let session = ingested_native(&db, Agent::Pi, "native-a");
        let terminal = identity_terminal(&registry, Agent::Pi, Some("native-a"), None);
        let pending_lookup = registry.attach(&terminal).unwrap().summary;
        assert!(registry.close(&terminal).is_some());
        registry.finish_native_binding(&db, &pending_lookup, "native-a", &session);
        assert!(registry
            .report_native_identity(&db, &terminal, &native_event("native-a"))
            .is_none());
        attach_fixture_paths(&db);
        registry.bind_discovered(&db);
        assert!(registry.attach(&terminal).is_err());
        assert!(registry.list().is_empty());
        assert!(registry.for_session(&session.id).is_none());
    }

    #[test]
    fn resume_reservation_is_exclusive_and_released_on_drop() {
        let registry = registry();
        let start = std::sync::Barrier::new(8);
        let claimed = std::sync::Barrier::new(8);
        let results = std::thread::scope(|scope| {
            let threads: Vec<_> = (0..8)
                .map(|_| {
                    let registry = &registry;
                    let start = &start;
                    let claimed = &claimed;
                    scope.spawn(move || {
                        start.wait();
                        let reservation = registry.reserve_resume("same-session");
                        claimed.wait();
                        reservation.is_ok()
                    })
                })
                .collect();
            threads
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(results.into_iter().filter(|claimed| *claimed).count(), 1);
        let claim = registry.reserve_resume("same-session").unwrap();
        assert!(registry.reserve_resume("another-session").is_ok());
        assert!(registry.reserve_resume("same-session").is_err());
        drop(claim);
        assert!(registry.reserve_resume("same-session").is_ok());
    }

    #[test]
    fn continue_ignores_exited_terminals_but_scrollback_stays_attachable() {
        let registry = registry();
        let id = identity_terminal(&registry, Agent::Codex, Some("native"), Some("session"));
        {
            let records = registry.records.lock().unwrap();
            let mut record = records[&id].lock().unwrap();
            record.summary.live = false;
            record.scrollback.push(b"retained output");
        }
        assert!(registry.for_session("session").is_none());
        let listed = registry.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].terminal_id, id);
        assert!(!listed[0].live);
        let snapshot = registry.attach(&id).unwrap();
        assert!(!snapshot.summary.live);
        assert!(!snapshot.scrollback.is_empty());
        assert!(registry.reserve_resume("session").is_ok());
    }

    #[test]
    fn retained_terminal_list_includes_live_and_exited_in_creation_order() {
        let registry = registry();
        let older = identity_terminal(&registry, Agent::Codex, Some("native"), Some("session"));
        let newer = identity_terminal(&registry, Agent::Codex, Some("native"), Some("session"));
        {
            let records = registry.records.lock().unwrap();
            records[&older].lock().unwrap().summary.created_at = "2026-10-08T00:00:00Z".into();
            let mut record = records[&newer].lock().unwrap();
            record.summary.created_at = "2026-10-08T01:00:00Z".into();
            record.summary.live = false;
        }
        let listed = registry.list();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].terminal_id, newer);
        assert!(!listed[0].live);
        assert_eq!(listed[1].terminal_id, older);
        assert!(listed[1].live);
        assert_eq!(registry.for_session("session").unwrap().terminal_id, older);
        assert!(registry.has_live("session"));
    }

    #[test]
    fn session_deletion_removes_only_its_terminals_after_successful_commit() {
        let db = binding_db();
        let registry = TerminalRegistry::new(None);
        let a = ingested_native(&db, Agent::Codex, "delete-a");
        let b = ingested_native(&db, Agent::Codex, "keep-b");
        let live = identity_terminal(&registry, Agent::Codex, Some("delete-a"), Some(&a.id));
        let exited = identity_terminal(&registry, Agent::Codex, Some("delete-a"), Some(&a.id));
        registry.records.lock().unwrap()[&exited]
            .lock()
            .unwrap()
            .summary
            .live = false;
        let keep = identity_terminal(&registry, Agent::Codex, Some("keep-b"), Some(&b.id));
        let mail = db
            .mailbox_send(&a.id, None, "retained until deletion commits")
            .unwrap();
        // Unarchived deletion fails without dropping a terminal or its output.
        assert!(registry
            .with_session_removal(&a.id, || crate::lifecycle::permanently_delete_session(
                &db, &a.id
            ))
            .is_err());
        assert_eq!(registry.list().len(), 3);
        assert_eq!(
            db.mailbox_read(&a.id, &mail.id).unwrap().body,
            "retained until deletion commits"
        );
        crate::lifecycle::archive_session(&db, &a.id).unwrap();
        let (outcome, removed) = registry
            .with_session_removal(&a.id, || {
                crate::lifecycle::permanently_delete_session(&db, &a.id)
            })
            .unwrap();
        assert!(outcome.purged);
        assert_eq!(removed.len(), 2);
        assert!(removed.contains(&live) && removed.contains(&exited));
        assert!(db.get_session(&a.id).unwrap().is_none());
        assert!(registry.attach(&live).is_err() && registry.attach(&exited).is_err());
        assert!(registry.attach(&keep).unwrap().summary.live);
        assert!(db.get_session(&b.id).unwrap().is_some());
        assert!(db.mailbox_read(&a.id, &mail.id).is_err());
        // Rediscovering the same native identity cannot resurrect deleted work.
    }

    #[test]
    fn reconnect_stops_all_bound_terminals_preserves_other_sessions_and_can_be_retried() {
        let registry = registry();
        let old = identity_terminal(&registry, Agent::Codex, Some("native"), Some("session"));
        let other = identity_terminal(&registry, Agent::Codex, Some("native"), Some("session"));
        let unrelated = identity_terminal(
            &registry,
            Agent::Codex,
            Some("other-native"),
            Some("other-session"),
        );
        let expected = registry.attach(&old).unwrap().summary;
        let claim = registry.reserve_reconnect(&expected, "session").unwrap();
        assert!(!registry.attach(&old).unwrap().summary.live);
        assert!(!registry.attach(&other).unwrap().summary.live);
        assert!(registry.attach(&unrelated).unwrap().summary.live);
        assert!(registry.for_session("session").is_none());
        assert!(registry.reserve_reconnect(&expected, "session").is_err());
        assert!(registry.reserve_resume("session").is_err());
        drop(claim);
        // Failed spawn drops the claim, retaining the exited terminal for retry.
        let retry = registry.reserve_reconnect(&expected, "session").unwrap();
        drop(retry);
        assert!(registry.attach(&old).is_ok());
    }

    #[test]
    fn reconnect_refuses_a_stale_identity_without_stopping_the_new_session() {
        let registry = registry();
        let db = binding_db();
        let a = ingested_native(&db, Agent::Codex, "native-a");
        let b = ingested_native(&db, Agent::Codex, "native-b");
        let id = identity_terminal(&registry, Agent::Codex, Some("native-a"), Some(&a.id));
        let expected = registry.attach(&id).unwrap().summary;
        registry.report_native_identity(&db, &id, &native_event("native-b"));
        assert!(registry.reserve_reconnect(&expected, &a.id).is_err());
        let current = registry.attach(&id).unwrap().summary;
        assert!(current.live);
        assert_eq!(current.session_id.as_deref(), Some(b.id.as_str()));
        assert!(registry.reserve_resume(&a.id).is_ok());
    }

    #[test]
    fn scrollback_push_evicts_oldest_beyond_cap() {
        let mut sb = Scrollback::default();
        sb.push(&[1, 2, 3]);
        assert_eq!(sb.bytes(), vec![1, 2, 3]);
        assert_eq!(sb.output_offset, 3);
        // A single chunk larger than the cap keeps only its tail.
        let big: Vec<u8> = (0..SCROLLBACK_CAP + 10).map(|i| i as u8).collect();
        sb.push(&big);
        assert_eq!(sb.len(), SCROLLBACK_CAP);
        assert_eq!(sb.bytes()[0], 10_u8);
        assert_eq!(sb.output_offset, (SCROLLBACK_CAP + 13) as u64);
        // Eviction across multiple chunks must not reset the stream boundary.
        sb.push(&[42, 43]);
        assert_eq!(sb.output_offset, (SCROLLBACK_CAP + 15) as u64);
        assert_eq!(sb.len(), SCROLLBACK_CAP);
        assert_eq!(sb.bytes()[SCROLLBACK_CAP - 2..], [42, 43]);
    }

    #[cfg(windows)]
    #[test]
    fn terminal_conpty_launch_runs_without_opening_the_terminal_page() {
        let registry = registry();
        let cmd = AgentCommand {
            program: "cmd.exe".into(),
            args: vec!["/c".into(), "echo NOENDING_CONPTY_READY".into()],
            cwd: None,
        };
        let outcome = spawn_embedded(&cmd, &target(&registry, "s-conpty")).unwrap();
        let id = outcome.terminal_id.unwrap();
        struct Cleanup<'a>(&'a TerminalRegistry);
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                self.0.kill_all();
            }
        }
        let _cleanup = Cleanup(&registry);
        let output = until("command output without frontend replies", || {
            let snapshot = registry.attach(&id).ok()?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(snapshot.scrollback)
                .ok()?;
            String::from_utf8_lossy(&bytes)
                .contains("NOENDING_CONPTY_READY")
                .then_some(bytes)
        });
        assert!(!output.windows(4).any(|bytes| bytes == b"\x1b[6n"));
        registry.kill_all();
    }

    #[cfg(unix)]
    #[test]
    fn terminal_spawn_replays_output_and_reports_exit() {
        let registry = registry();
        let outcome = spawn_embedded(
            &sh("printf 'hello-pty'; exit 7"),
            &target(&registry, "s-exit"),
        )
        .unwrap();
        let id = outcome.terminal_id.clone().unwrap();

        until("terminal to report exit", || {
            registry.attach(&id).ok().filter(|s| !s.summary.live)
        });
        let snap = registry.attach(&id).unwrap();
        assert_eq!(snap.summary.exit_code, Some(7));
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&snap.scrollback)
            .unwrap();
        let replayed = String::from_utf8_lossy(&decoded);
        assert!(
            replayed.contains("hello-pty"),
            "scrollback must replay after exit, got: {replayed:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn terminal_input_preserves_binary_mouse_bytes_and_utf8_text() {
        let registry = registry();
        let outcome = spawn_embedded(
            &sh("stty raw -echo; printf 'INPUT_READY'; dd bs=1 count=9 2>/dev/null | od -An -tx1"),
            &target(&registry, "s-bytes"),
        )
        .unwrap();
        let id = outcome.terminal_id.unwrap();
        let output = || {
            let snapshot = registry.attach(&id).ok()?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(snapshot.scrollback)
                .ok()?;
            Some(String::from_utf8_lossy(&bytes).into_owned())
        };
        until("raw-mode PTY", || {
            output().filter(|s| s.contains("INPUT_READY"))
        });
        assert!(registry.write_binary(&id, "你").is_err());
        registry
            .write_binary(&id, "\u{1b}[M\u{80}\u{c8}\u{ff}")
            .unwrap();
        registry.write(&id, "你").unwrap();
        let dump = until("PTY byte dump", || output().filter(|s| s.contains("a0")));
        let hex: Vec<_> = dump
            .strip_prefix("INPUT_READY")
            .unwrap()
            .split_whitespace()
            .collect();
        assert_eq!(hex, ["1b", "5b", "4d", "80", "c8", "ff", "e4", "bd", "a0"]);
    }

    #[cfg(unix)]
    #[test]
    fn terminal_exports_xterm_and_its_own_id_to_agent_child_processes() {
        let registry = registry();
        let outcome = spawn_embedded(
            &sh(r#"printf 'term=%s;caller=%s' "$TERM" "$(sh -c 'printf %s "$NOENDING_TERMINAL_ID"')""#),
            &target(&registry, "s-term"),
        )
        .unwrap();
        let id = outcome.terminal_id.unwrap();
        until("terminal environment output", || {
            let snapshot = registry.attach(&id).ok()?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(snapshot.scrollback)
                .ok()?;
            String::from_utf8_lossy(&bytes)
                .contains(&format!("term=xterm-256color;caller={id}"))
                .then_some(())
        });
    }

    #[cfg(unix)]
    #[test]
    fn terminal_input_echoes_through_the_pty() {
        let registry = registry();
        let outcome = spawn_embedded(&sh("cat"), &target(&registry, "s-cat")).unwrap();
        let id = outcome.terminal_id.clone().unwrap();

        registry.write(&id, "ping-through-pty").unwrap();
        until("echo to appear in scrollback", || {
            registry.attach(&id).ok().filter(|s| {
                String::from_utf8_lossy(
                    &base64::engine::general_purpose::STANDARD
                        .decode(&s.scrollback)
                        .unwrap(),
                )
                .contains("ping-through-pty")
            })
        });
        registry.kill_all();
    }

    #[cfg(unix)]
    #[test]
    fn terminal_write_after_exit_is_refused_with_reason() {
        let registry = registry();
        let outcome = spawn_embedded(&sh("exit 0"), &target(&registry, "s-dead")).unwrap();
        let id = outcome.terminal_id.clone().unwrap();
        until("terminal to exit", || {
            registry.attach(&id).ok().filter(|s| !s.summary.live)
        });
        let err = registry.write(&id, "x").unwrap_err();
        assert!(err.to_string().contains("已退出"));
    }

    #[cfg(unix)]
    #[test]
    fn duplicate_spawn_is_refused_and_a_session_can_restart_after_its_owner_exits() {
        let registry = registry();
        let live = spawn_embedded(&sh("sleep 30"), &target(&registry, "s-two")).unwrap();
        assert!(spawn_embedded(&sh("sleep 30"), &target(&registry, "s-two")).is_err());
        assert_eq!(
            registry.for_session("s-two").unwrap().terminal_id,
            live.terminal_id.clone().unwrap()
        );
        assert_eq!(registry.list().len(), 1);
        registry
            .close(live.terminal_id.as_deref().unwrap())
            .unwrap();
        let replacement = spawn_embedded(&sh("sleep 30"), &target(&registry, "s-two")).unwrap();
        assert_eq!(
            registry.for_session("s-two").unwrap().terminal_id,
            replacement.terminal_id.unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn resize_updates_geometry_without_panicking() {
        let registry = registry();
        let outcome = spawn_embedded(&sh("sleep 5"), &target(&registry, "s-resize")).unwrap();
        let id = outcome.terminal_id.clone().unwrap();
        registry.resize(&id, 120, 40).unwrap();
        let snap = registry.attach(&id).unwrap();
        assert_eq!((snap.cols, snap.rows), (120, 40));
        registry.kill_all();
    }

    #[cfg(unix)]
    #[test]
    fn has_live_tracks_the_wait_thread_fact() {
        let registry = registry();
        assert!(!registry.has_live("s-live"));
        let outcome = spawn_embedded(&sh("sleep 30"), &target(&registry, "s-live")).unwrap();
        assert!(registry.has_live("s-live"));
        registry.kill_all();
        until("terminal to die after kill", || {
            registry
                .attach(&outcome.terminal_id.clone().unwrap())
                .ok()
                .filter(|s| !s.summary.live)
        });
        assert!(!registry.has_live("s-live"));
    }
    #[cfg(unix)]
    #[test]
    fn a_prespecified_id_binds_at_discovery() {
        let registry = registry();
        let mut cmd = sh("sleep 30");
        cmd.cwd = Some(std::path::PathBuf::from("/tmp"));
        let outcome = spawn_embedded(
            &cmd,
            &EmbeddedTarget {
                registry: &registry,
                session_id: None,
                initial_workstream_ids: &[],
                root_agent_session_id: Some("root-born-identity"),
                agent: Agent::Pi,
            },
        )
        .unwrap();
        let id = outcome.terminal_id.clone().unwrap();

        let root = std::env::temp_dir().join(format!(
            "noending-term-prespec-{}",
            crate::storage::new_id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let db = crate::storage::Db::open(&root.join("db.sqlite")).unwrap();
        let (session_id, _) = db
            .upsert_logical_session(
                Agent::Pi,
                "root-born-identity",
                None,
                Some("出生即有身份"),
                Some("/tmp"),
                None,
                None,
                None,
                None,
                "pi",
                "/tmp/fake-session.jsonl",
                &serde_json::json!({}),
            )
            .unwrap();

        attach_fixture_paths(&db);
        registry.bind_discovered(&db);

        let bound = registry
            .for_session(&session_id)
            .expect("bound by exact id");
        assert_eq!(bound.terminal_id, id);
        assert!(registry.has_live(&session_id));
        registry.kill_all();
    }

    #[cfg(unix)]
    #[test]
    fn an_explicit_close_kills_the_child_and_removes_the_record() {
        let registry = registry();
        let outcome = spawn_embedded(
            &sh("sleep 30"),
            &EmbeddedTarget {
                registry: &registry,
                session_id: Some("s-live"),
                initial_workstream_ids: &[],
                root_agent_session_id: None,
                agent: Agent::Codex,
            },
        )
        .unwrap();
        let id = outcome.terminal_id.clone().unwrap();
        assert!(registry.has_live("s-live"));

        registry.close(&id).expect("closed");

        assert!(!registry.has_live("s-live"));
        assert!(registry.list().iter().all(|t| t.terminal_id != id));
        assert!(registry.for_session("s-live").is_none());
        assert!(registry.attach(&id).is_err());
        assert!(registry.close(&id).is_none(), "closing twice is a no-op");
    }
    struct ReminderWriter(std::sync::Arc<Mutex<Vec<u8>>>, bool);
    impl std::io::Write for ReminderWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.1 {
                return Err(std::io::Error::other("fixture disconnected"));
            }
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    fn mailbox_terminal(
        agent: Agent,
    ) -> (
        TerminalRegistry,
        crate::storage::Db,
        String,
        String,
        std::sync::Arc<Mutex<Vec<u8>>>,
    ) {
        let registry = registry();
        let db = binding_db();
        let session = ingested_native(&db, agent, "mailbox-native");
        let terminal =
            identity_terminal(&registry, agent, Some("mailbox-native"), Some(&session.id));
        let bytes = std::sync::Arc::new(Mutex::new(Vec::new()));
        registry.records.lock().unwrap()[&terminal]
            .lock()
            .unwrap()
            .writer = Some(std::sync::Arc::new(Mutex::new(Box::new(ReminderWriter(
            bytes.clone(),
            false,
        )))));
        let mut event = native_event("mailbox-native");
        event.activity = Some(activity::ActivitySignal {
            state: activity::ActivityState::Idle,
            turn_id: None,
            starts_turn: false,
        });
        registry.report_native_identity(&db, &terminal, &event);
        registry.initialize_mailboxes(&db).unwrap();
        (registry, db, terminal, session.id, bytes)
    }
    #[test]
    fn automatic_reminders_merge_new_mail_and_do_not_wait_for_agent_receipts_or_repeat_old_unread()
    {
        let (registry, db, id, session, bytes) = mailbox_terminal(Agent::Codex);
        let identity = registry.attach(&id).unwrap().summary.identity_revision;
        registry
            .set_auto_notifications(&db, &id, identity, true)
            .unwrap();
        let first = db.mailbox_send(&session, None, "private body one").unwrap();
        db.mailbox_send(&session, None, "private body two").unwrap();
        registry.check_mailboxes(&db).unwrap();
        assert_eq!(
            &*bytes.lock().unwrap(),
            "\x1b[200~noending信箱中有2条未读信息，请查收\x1b[201~\t".as_bytes()
        );
        assert!(db
            .mailbox_read(&session, &first.id)
            .unwrap()
            .read_at
            .is_none());
        registry.check_mailboxes(&db).unwrap();
        assert_eq!(
            String::from_utf8(bytes.lock().unwrap().clone())
                .unwrap()
                .matches("请查收")
                .count(),
            1
        );
        db.mailbox_send(&session, None, "three").unwrap();
        registry.check_mailboxes(&db).unwrap();
        assert_eq!(
            String::from_utf8(bytes.lock().unwrap().clone())
                .unwrap()
                .matches("请查收")
                .count(),
            2
        );
    }
    #[test]
    fn manual_reminders_use_steering_in_working_or_unconfirmed_states_and_skip_read_mail() {
        use activity::ActivityState::*;
        for state in [Working, Unknown, Interrupted, Failed] {
            let (registry, db, id, session, bytes) = mailbox_terminal(Agent::Codex);
            let m = db
                .mailbox_send(&session, None, "not pasted into terminal")
                .unwrap();
            let mut e = native_event("mailbox-native");
            e.activity = Some(activity::ActivitySignal {
                state,
                turn_id: Some("turn".into()),
                starts_turn: state == Working,
            });
            registry.report_native_identity(&db, &id, &e);
            registry.check_mailboxes(&db).unwrap();
            assert!(bytes.lock().unwrap().is_empty());
            registry.request_reminder(&id).unwrap();
            // Another native status notice must not discard a user's manual request.
            registry.report_native_identity(&db, &id, &e);
            registry.check_mailboxes(&db).unwrap();
            assert_eq!(
                &*bytes.lock().unwrap(),
                "\x1b[200~noending信箱中有1条未读信息，请查收\x1b[201~\r".as_bytes()
            );
            assert_eq!(registry.attach(&id).unwrap().summary.activity.state, state);
            assert!(!registry.notification_state(&id).unwrap().enabled);
            db.mailbox_mark(&session, &m.id, m.revision, true).unwrap();
            let before = bytes.lock().unwrap().clone();
            registry.request_reminder(&id).unwrap();
            registry.check_mailboxes(&db).unwrap();
            assert_eq!(*bytes.lock().unwrap(), before);
            e.activity = Some(activity::ActivitySignal {
                state: Waiting,
                turn_id: Some("turn".into()),
                starts_turn: false,
            });
            registry.report_native_identity(&db, &id, &e);
            assert!(registry.request_reminder(&id).is_err());
            assert!(registry
                .remind(&db, &id, notifications::ReminderKind::Manual)
                .is_err());
        }
    }
    #[test]
    fn unknown_allows_auto_reminders_and_keeps_pending_requests_without_claiming_idle() {
        use activity::ActivityState::*;
        for immediate in [false, true] {
            let (registry, db, id, session, bytes) = mailbox_terminal(Agent::Codex);
            let identity = registry.attach(&id).unwrap().summary.identity_revision;
            let mut event = native_event("mailbox-native");
            event.activity = Some(activity::ActivitySignal {
                state: Unknown,
                turn_id: None,
                starts_turn: false,
            });
            registry.report_native_identity(&db, &id, &event);
            let first = db
                .mailbox_send_with_notification(&session, Some(&session), "first", immediate)
                .unwrap();
            assert!(
                registry
                    .set_auto_notifications(&db, &id, identity, true)
                    .unwrap()
                    .enabled
            );
            // Identity-only native notices must not cancel notifications while activity remains unknown.
            event.activity = None;
            registry.report_native_identity(&db, &id, &event);
            registry.check_mailboxes(&db).unwrap();
            let key = if immediate { "\r" } else { "\t" };
            let first_frame = format!("\x1b[200~noending信箱中有1条未读信息，请查收\x1b[201~{key}");
            assert_eq!(*bytes.lock().unwrap(), first_frame.as_bytes());
            assert_eq!(
                registry.attach(&id).unwrap().summary.activity.state,
                Unknown
            );
            assert!(registry.notification_state(&id).unwrap().enabled);
            let seen = registry.attach(&id).unwrap().summary.activity;
            registry.mark_activity_seen(&id, seen.revision);
            assert_eq!(
                registry.attach(&id).unwrap().summary.activity.state,
                Unknown
            );
            assert!(db
                .mailbox_read(&session, &first.id)
                .unwrap()
                .read_at
                .is_none());
            event.activity = Some(activity::ActivitySignal {
                state: Waiting,
                turn_id: None,
                starts_turn: false,
            });
            registry.report_native_identity(&db, &id, &event);
            db.mailbox_send_with_notification(&session, Some(&session), "second", immediate)
                .unwrap();
            registry.check_mailboxes(&db).unwrap();
            assert_eq!(*bytes.lock().unwrap(), first_frame.as_bytes());
            event.activity.as_mut().unwrap().state = Unknown;
            registry.report_native_identity(&db, &id, &event);
            registry.check_mailboxes(&db).unwrap();
            let frames =
                format!("{first_frame}\x1b[200~noending信箱中有2条未读信息，请查收\x1b[201~{key}");
            assert_eq!(*bytes.lock().unwrap(), frames.as_bytes());
            assert!(registry.notification_state(&id).unwrap().enabled);
            assert_eq!(
                registry.attach(&id).unwrap().summary.activity.state,
                Unknown
            );
            registry.check_mailboxes(&db).unwrap();
            assert_eq!(*bytes.lock().unwrap(), frames.as_bytes());
        }
    }
    #[test]
    fn failed_and_interrupted_disable_automatic_notifications_and_return_to_idle_when_viewed() {
        use activity::ActivityState::*;
        for state in [Interrupted, Failed] {
            let (registry, db, id, _, _) = mailbox_terminal(Agent::Codex);
            let identity = registry.attach(&id).unwrap().summary.identity_revision;
            registry
                .set_auto_notifications(&db, &id, identity, true)
                .unwrap();
            let mut e = native_event("mailbox-native");
            e.activity = Some(activity::ActivitySignal {
                state,
                turn_id: None,
                starts_turn: false,
            });
            registry.report_native_identity(&db, &id, &e);
            assert!(!registry.notification_state(&id).unwrap().enabled);
            assert!(registry
                .set_auto_notifications(&db, &id, identity, true)
                .is_err());
            let seen = registry.attach(&id).unwrap().summary.activity;
            registry.mark_activity_seen(&id, seen.revision);
            assert_eq!(registry.attach(&id).unwrap().summary.activity.state, Idle);
            assert!(!registry.notification_state(&id).unwrap().enabled);
            e.activity = Some(activity::ActivitySignal {
                state: Working,
                turn_id: Some("next".into()),
                starts_turn: true,
            });
            registry.report_native_identity(&db, &id, &e);
            assert_eq!(
                registry.attach(&id).unwrap().summary.activity.state,
                Working
            );
            registry.mark_activity_seen(&id, seen.revision);
            assert_eq!(
                registry.attach(&id).unwrap().summary.activity.state,
                Working
            );
        }
    }
    #[test]
    fn disabled_auto_notifications_allow_editing_both_senders_and_failed_reminders_leave_mail_unread(
    ) {
        let (registry, db, id, session, bytes) = mailbox_terminal(Agent::Codex);
        let sender = ingested_native(&db, Agent::Pi, "sender");
        let m = db.mailbox_send(&session, Some(&sender.id), "old").unwrap();
        let identity = registry.attach(&id).unwrap().summary.identity_revision;
        registry
            .set_auto_notifications(&db, &id, identity, true)
            .unwrap();
        assert!(registry
            .edit_mailbox(&db, &id, identity, &m.id, m.revision, Some("new"))
            .is_err());
        registry
            .set_auto_notifications(&db, &id, identity, false)
            .unwrap();
        let edited = registry
            .edit_mailbox(&db, &id, identity, &m.id, m.revision, Some("new"))
            .unwrap()
            .unwrap();
        assert_eq!(edited.body, "new");
        assert_eq!(edited.sender_session_id, Some(sender.id));
        registry.records.lock().unwrap()[&id].lock().unwrap().writer = Some(std::sync::Arc::new(
            Mutex::new(Box::new(ReminderWriter(bytes, true))),
        ));
        assert!(registry
            .remind(&db, &id, notifications::ReminderKind::Manual)
            .is_err());
        assert!(db.mailbox_read(&session, &m.id).unwrap().read_at.is_none());
        assert!(!registry.notification_state(&id).unwrap().enabled);
        registry
            .edit_mailbox(&db, &id, identity, &m.id, edited.revision, None)
            .unwrap();
        assert!(db.mailbox_read(&session, &m.id).is_err());
    }

    #[test]
    fn pending_manual_checks_merge_and_override_automatic_queue_submission() {
        let (registry, db, id, session, bytes) = mailbox_terminal(Agent::Codex);
        let identity = registry.attach(&id).unwrap().summary.identity_revision;
        registry
            .set_auto_notifications(&db, &id, identity, true)
            .unwrap();
        db.mailbox_send(&session, None, "one").unwrap();
        db.mailbox_send(&session, None, "two").unwrap();
        registry.request_reminder(&id).unwrap();
        registry.request_reminder(&id).unwrap();
        registry.check_mailboxes(&db).unwrap();
        assert_eq!(
            &*bytes.lock().unwrap(),
            "\x1b[200~noending信箱中有2条未读信息，请查收\x1b[201~\r".as_bytes()
        );
        registry.check_mailboxes(&db).unwrap();
        assert_eq!(
            String::from_utf8(bytes.lock().unwrap().clone())
                .unwrap()
                .matches("请查收")
                .count(),
            1
        );
    }
    #[test]
    fn automatic_reminders_recheck_enabled_and_unread_before_first_write() {
        use notifications::ReminderKind::{Automatic, Manual};
        for (immediate, mark_read) in [(false, false), (true, false), (true, true)] {
            let (registry, db, id, session, bytes) = mailbox_terminal(Agent::Codex);
            let registry = std::sync::Arc::new(registry);
            let db = std::sync::Arc::new(db);
            let identity = registry.attach(&id).unwrap().summary.identity_revision;
            let mail = db
                .mailbox_send_with_notification(&session, Some(&session), "one", immediate)
                .unwrap();
            registry
                .set_auto_notifications(&db, &id, identity, true)
                .unwrap();
            let writer = registry.records.lock().unwrap()[&id]
                .lock()
                .unwrap()
                .writer
                .clone()
                .unwrap();
            let gate = writer.lock().unwrap();
            let (tx, rx) = std::sync::mpsc::channel();
            let r = registry.clone();
            let d = db.clone();
            let t = id.clone();
            let worker = std::thread::spawn(move || {
                tx.send(()).unwrap();
                r.remind(&d, &t, Automatic)
            });
            rx.recv().unwrap();
            if mark_read {
                db.mailbox_mark(&session, &mail.id, mail.revision, true)
                    .unwrap();
            } else {
                registry
                    .set_auto_notifications(&db, &id, identity, false)
                    .unwrap();
            }
            drop(gate);
            worker.join().unwrap().unwrap();
            assert!(bytes.lock().unwrap().is_empty());
            if !mark_read {
                registry.remind(&db, &id, Manual).unwrap();
                assert!(!bytes.lock().unwrap().is_empty());
            }
        }
    }
    #[test]
    fn immediate_requests_merge_with_ordinary_mail_in_working_and_keep_mailbox_arrival_order() {
        use activity::ActivityState::*;
        let (registry, db, id, session, bytes) = mailbox_terminal(Agent::Codex);
        let identity = registry.attach(&id).unwrap().summary.identity_revision;
        registry
            .set_auto_notifications(&db, &id, identity, true)
            .unwrap();
        let mut e = native_event("mailbox-native");
        e.activity = Some(activity::ActivitySignal {
            state: Waiting,
            turn_id: Some("t".into()),
            starts_turn: false,
        });
        registry.report_native_identity(&db, &id, &e);
        let ordinary = db.mailbox_send(&session, None, "first ordinary").unwrap();
        let requested = db
            .mailbox_send_with_notification(&session, Some(&session), "second immediate", true)
            .unwrap();
        let requested_two = db
            .mailbox_send_with_notification(&session, Some(&session), "third immediate", true)
            .unwrap();
        registry.check_mailboxes(&db).unwrap();
        assert!(bytes.lock().unwrap().is_empty());
        e.activity.as_mut().unwrap().state = Working;
        e.activity.as_mut().unwrap().starts_turn = true;
        registry.report_native_identity(&db, &id, &e);
        registry.check_mailboxes(&db).unwrap();
        let expected = "\x1b[200~noending信箱中有3条未读信息，请查收\x1b[201~\r";
        assert_eq!(*bytes.lock().unwrap(), expected.as_bytes());
        let page = db.mailbox_list(&session, "unread", None, 50).unwrap();
        assert_eq!(
            page.items.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec![
                ordinary.id.as_str(),
                requested.id.as_str(),
                requested_two.id.as_str()
            ]
        );
        registry.check_mailboxes(&db).unwrap();
        assert_eq!(*bytes.lock().unwrap(), expected.as_bytes());
        // An old, still-unread request must not grant the Working exception to a new ordinary message.
        db.mailbox_send(&session, None, "fourth ordinary").unwrap();
        registry.check_mailboxes(&db).unwrap();
        assert_eq!(*bytes.lock().unwrap(), expected.as_bytes());
        e.activity.as_mut().unwrap().state = Idle;
        e.activity.as_mut().unwrap().starts_turn = false;
        registry.report_native_identity(&db, &id, &e);
        registry.check_mailboxes(&db).unwrap();
        assert_eq!(
            *bytes.lock().unwrap(),
            format!("{expected}\x1b[200~noending信箱中有4条未读信息，请查收\x1b[201~\t").as_bytes()
        );
    }
    #[test]
    fn immediate_requests_require_auto_notifications_and_steer_in_each_allowed_state() {
        use activity::ActivityState::*;
        for state in [Idle, Completed, Working, Unknown] {
            let (registry, db, id, session, bytes) = mailbox_terminal(Agent::Codex);
            let identity = registry.attach(&id).unwrap().summary.identity_revision;
            db.mailbox_send_with_notification(&session, Some(&session), "request", true)
                .unwrap();
            registry.initialize_mailboxes(&db).unwrap();
            let mut e = native_event("mailbox-native");
            e.activity = Some(activity::ActivitySignal {
                state,
                turn_id: None,
                starts_turn: state == Working,
            });
            registry.report_native_identity(&db, &id, &e);
            registry.check_mailboxes(&db).unwrap();
            assert!(bytes.lock().unwrap().is_empty());
            registry
                .set_auto_notifications(&db, &id, identity, true)
                .unwrap();
            registry
                .set_auto_notifications(&db, &id, identity, false)
                .unwrap();
            registry.check_mailboxes(&db).unwrap();
            assert!(bytes.lock().unwrap().is_empty());
            registry
                .set_auto_notifications(&db, &id, identity, true)
                .unwrap();
            registry.check_mailboxes(&db).unwrap();
            let expected = "\x1b[200~noending信箱中有1条未读信息，请查收\x1b[201~\r";
            assert_eq!(*bytes.lock().unwrap(), expected.as_bytes());
            registry.check_mailboxes(&db).unwrap();
            assert_eq!(*bytes.lock().unwrap(), expected.as_bytes());
        }
    }
    #[test]
    fn read_or_deleted_immediate_requests_do_not_release_other_unread_mail_while_working() {
        use activity::ActivityState::*;
        let (registry, db, id, session, bytes) = mailbox_terminal(Agent::Codex);
        let identity = registry.attach(&id).unwrap().summary.identity_revision;
        registry
            .set_auto_notifications(&db, &id, identity, true)
            .unwrap();
        let mut e = native_event("mailbox-native");
        e.activity = Some(activity::ActivitySignal {
            state: Waiting,
            turn_id: Some("t".into()),
            starts_turn: false,
        });
        registry.report_native_identity(&db, &id, &e);
        db.mailbox_send(&session, None, "ordinary").unwrap();
        let read = db
            .mailbox_send_with_notification(&session, Some(&session), "read", true)
            .unwrap();
        let deleted = db
            .mailbox_send_with_notification(&session, Some(&session), "deleted", true)
            .unwrap();
        registry.check_mailboxes(&db).unwrap();
        db.mailbox_mark(&session, &read.id, read.revision, true)
            .unwrap();
        db.mailbox_delete(&session, &deleted.id, deleted.revision)
            .unwrap();
        e.activity.as_mut().unwrap().state = Working;
        e.activity.as_mut().unwrap().starts_turn = true;
        registry.report_native_identity(&db, &id, &e);
        registry.check_mailboxes(&db).unwrap();
        assert!(bytes.lock().unwrap().is_empty());
        e.activity.as_mut().unwrap().state = Idle;
        e.activity.as_mut().unwrap().starts_turn = false;
        registry.report_native_identity(&db, &id, &e);
        registry.check_mailboxes(&db).unwrap();
        assert_eq!(
            *bytes.lock().unwrap(),
            "\x1b[200~noending信箱中有1条未读信息，请查收\x1b[201~\t".as_bytes()
        );
    }
    #[test]
    fn failed_and_interrupted_cancel_immediate_requests_until_user_enables_auto_again() {
        use activity::ActivityState::*;
        for state in [Interrupted, Failed] {
            let (registry, db, id, session, bytes) = mailbox_terminal(Agent::Codex);
            let identity = registry.attach(&id).unwrap().summary.identity_revision;
            registry
                .set_auto_notifications(&db, &id, identity, true)
                .unwrap();
            let mut e = native_event("mailbox-native");
            e.activity = Some(activity::ActivitySignal {
                state: Waiting,
                turn_id: Some("t".into()),
                starts_turn: false,
            });
            registry.report_native_identity(&db, &id, &e);
            db.mailbox_send_with_notification(&session, Some(&session), "request", true)
                .unwrap();
            registry.check_mailboxes(&db).unwrap();
            e.activity.as_mut().unwrap().state = state;
            e.activity.as_mut().unwrap().starts_turn = false;
            registry.report_native_identity(&db, &id, &e);
            registry.check_mailboxes(&db).unwrap();
            assert!(!registry.notification_state(&id).unwrap().enabled);
            assert!(bytes.lock().unwrap().is_empty());
            e.activity.as_mut().unwrap().state = Idle;
            registry.report_native_identity(&db, &id, &e);
            registry.check_mailboxes(&db).unwrap();
            assert!(bytes.lock().unwrap().is_empty());
            registry
                .set_auto_notifications(&db, &id, identity, true)
                .unwrap();
            registry.check_mailboxes(&db).unwrap();
            assert_eq!(
                *bytes.lock().unwrap(),
                "\x1b[200~noending信箱中有1条未读信息，请查收\x1b[201~\r".as_bytes()
            );
        }
    }
    #[test]
    fn native_inputs_trigger_history_sync_even_while_already_working_and_tools_do_not() {
        let (registry, db, id, session, _) = mailbox_terminal(Agent::Codex);
        let mut event = native_event("mailbox-native");
        event.activity = Some(activity::ActivitySignal {
            state: activity::ActivityState::Working,
            turn_id: Some("t".into()),
            starts_turn: true,
        });
        for input in ["one", "two"] {
            event.input = Some(identity::NativeInput {
                id: Some(input.into()),
            });
            assert!(
                matches!(registry.native_ingest_request(&db,&id,&event),Some(crate::commands::ingestion::IngestScope::RefreshSession(s)) if s==session)
            );
        }
        assert!(registry.native_ingest_request(&db, &id, &event).is_none());
        event.input = None;
        event.activity.as_mut().unwrap().starts_turn = false;
        assert!(registry.native_ingest_request(&db, &id, &event).is_none());
    }
}

#[cfg(test)]
fn attach_fixture_paths(db: &crate::storage::Db) {
    let paths = db.list_workspace_paths().unwrap();
    for session in db
        .list_sessions(crate::storage::SessionFilter::default())
        .unwrap()
    {
        if session.workspace_path_id.is_none() {
            if let Some(path) = paths
                .iter()
                .find(|p| session.cwd.as_deref() == Some(p.canonical_path.as_str()))
            {
                crate::storage::session_paths::attach_session_workspace_path_conn(
                    &db.write(),
                    &session.id,
                    Some(&path.id),
                )
                .unwrap();
            }
        }
    }
}
