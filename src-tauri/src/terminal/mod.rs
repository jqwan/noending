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

pub mod identity;
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
/// List-level fact: the set of live terminals changed (spawn / exit / bind).
/// Sidebar and any terminal-list consumer refetch once per event — no polling.
pub const EVENT_CHANGED: &str = "terminals-changed";
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
    pub identity_revision: u64,
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

/// attach 的回答：元数据 + 当前 scrollback 快照 + 几何。一次性全量重放
/// （cap 256 KiB），换来零 seq 协商——子页切换的频率下这是正确的取舍。
#[derive(Debug, Clone, Serialize)]
pub struct TerminalSnapshot {
    #[serde(flatten)]
    pub summary: TerminalSummary,
    /// base64 of the buffered raw PTY bytes.
    pub scrollback: String,
    pub cols: u16,
    pub rows: u16,
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
}

impl Scrollback {
    fn push(&mut self, bytes: &[u8]) {
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
    identity_bridge: Option<std::sync::Arc<identity::IdentityBridge>>,
    scrollback: Scrollback,
    cols: u16,
    rows: u16,
    writer: Option<Box<dyn std::io::Write + Send>>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
}

/// The backend's set of embedded terminals, keyed by `terminal_id`.
/// Managed app state; `app` is `Some` in production (events flow to the
/// webview) and `None` in tests (emits are skipped, everything else runs).
pub struct TerminalRegistry {
    records: Mutex<HashMap<String, std::sync::Arc<Mutex<TerminalRecord>>>>,
    /// Serializes NoEnding's own Resume launches; Agent-reported identity
    /// changes do not acquire this lock or impose exclusive Session ownership.
    /// Always acquired before `records`; never held across a process spawn.
    resume_reservations: Mutex<HashSet<String>>,
    app: Option<AppHandle>,
}

/// A pending Resume claim. Every return path, including a failed spawn,
/// releases it; a successful spawn has registered its live record by then.
pub struct ResumeReservation<'a> {
    registry: &'a TerminalRegistry,
    session_id: String,
}

impl Drop for ResumeReservation<'_> {
    fn drop(&mut self) {
        if let Ok(mut reservations) = self.registry.resume_reservations.lock() {
            reservations.remove(&self.session_id);
        }
    }
}

impl TerminalRegistry {
    pub fn new(app: Option<AppHandle>) -> Self {
        Self {
            records: Mutex::new(HashMap::new()),
            resume_reservations: Mutex::new(HashSet::new()),
            app,
        }
    }

    /// What the terminal subpage should attach to for `session_id`:
    /// a live terminal wins over a more recently created exited one —
    /// the live one is where the conversation is still happening.
    pub fn for_session(&self, session_id: &str) -> Option<TerminalSummary> {
        let records = self.records.lock().ok()?;
        let mut best: Option<TerminalSummary> = None;
        for record in records.values() {
            let Ok(record) = record.lock() else { continue };
            if record.summary.session_id.as_deref() != Some(session_id) {
                continue;
            }
            best = match best {
                None => Some(record.summary.clone()),
                Some(prev) => {
                    let live_wins = record.summary.live && !prev.live;
                    let newer = record.summary.created_at > prev.created_at;
                    if live_wins || (newer && prev.live == record.summary.live) {
                        Some(record.summary.clone())
                    } else {
                        Some(prev)
                    }
                }
            };
        }
        best
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

    /// Every live terminal, newest first. The sessions board renders the
    /// unbound ones as its 运行中 pseudo-rows and derives the "terminal is
    /// running" fact for bound sessions from the rest.
    pub fn list_live(&self) -> Vec<TerminalSummary> {
        let Ok(records) = self.records.lock() else {
            return Vec::new();
        };
        let mut live: Vec<TerminalSummary> = records
            .values()
            .filter_map(|record| {
                let record = record.lock().ok()?;
                record.summary.live.then(|| record.summary.clone())
            })
            .collect();
        live.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        live
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
        })
    }

    fn emit_identity(&self, summary: &TerminalSummary) {
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

    /// A native fact may rebind an existing terminal, even when other terminals
    /// or an in-flight NoEnding Resume already refer to the same Session.
    /// Clear the old Session immediately; resolving the new ID is a separate,
    /// version-checked step so slow ingestion cannot resurrect an old identity.
    pub fn report_native_identity(
        &self,
        db: &crate::storage::Db,
        terminal_id: &str,
        event: &identity::NativeIdentityEvent,
    ) -> Option<TerminalSummary> {
        if event.native_id.trim().is_empty() {
            return None;
        }
        let (summary, changed) = {
            let records = self.records.lock().ok()?;
            let mut record = records.get(terminal_id)?.lock().ok()?;
            // Turn completion only refreshes the current conversation. It
            // must not restore an old identity after a lifecycle switch.
            if matches!(event.source.as_deref(), Some("Stop" | "agent_end"))
                && record
                    .root_agent_session_id
                    .as_deref()
                    .is_some_and(|id| id != event.native_id)
            {
                return None;
            }
            let identity_changed =
                record.root_agent_session_id.as_deref() != Some(event.native_id.as_str());
            let cwd_changed = event
                .cwd
                .as_ref()
                .is_some_and(|cwd| record.summary.cwd.as_ref() != Some(cwd));
            if identity_changed {
                record.root_agent_session_id = Some(event.native_id.clone());
                record.summary.session_id = None;
                record.summary.session_title = None;
            }
            if let Some(cwd) = &event.cwd {
                record.summary.cwd = Some(cwd.clone());
            }
            if identity_changed || cwd_changed {
                record.summary.identity_revision += 1;
            }
            (record.summary.clone(), identity_changed || cwd_changed)
        };
        if changed {
            self.emit_identity(&summary);
        }
        if summary.session_id.is_none() {
            self.resolve_native_identity(db, &summary, &event.native_id);
        }
        self.attach(terminal_id)
            .ok()
            .map(|snapshot| snapshot.summary)
    }

    fn resolve_native_identity(
        &self,
        db: &crate::storage::Db,
        summary: &TerminalSummary,
        native_id: &str,
    ) {
        if let Ok(Some(session)) = db.find_session_by_root_agent_id(summary.agent, native_id) {
            self.finish_native_binding(summary, native_id, &session);
        }
    }

    /// Apply a lookup only to the exact identity version that requested it.
    /// Comparing both version and native ID also protects A -> B -> A switches.
    fn finish_native_binding(
        &self,
        expected: &TerminalSummary,
        native_id: &str,
        session: &crate::domain::Session,
    ) {
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
            if record.summary.identity_revision != expected.identity_revision
                || record.root_agent_session_id.as_deref() != Some(native_id)
                || record.summary.session_id.is_some()
                || record.summary.agent != session.agent
                || session.root_agent_session_id != native_id
            {
                return;
            }
            record.summary.session_id = Some(session.id.clone());
            record.summary.session_title = session.title.clone();
            record.summary.identity_revision += 1;
            record.summary.clone()
        };
        self.emit_identity(&bound);
    }

    /// Ingestion resolves pending native IDs. There is deliberately no message,
    /// cwd, timestamp, LaunchIntent or exclusive-occupancy inference here.
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
        let removed = {
            let Ok(mut records) = self.records.lock() else {
                return None;
            };
            records.remove(terminal_id)
        }?;
        let (summary, bridge) = {
            let Ok(mut record) = removed.lock() else {
                return None;
            };
            if record.summary.live {
                let _ = record.killer.kill();
            }
            (record.summary.clone(), record.identity_bridge.take())
        };
        drop(bridge);
        // The live set shrank (the record is gone outright).
        if let Some(app) = &self.app {
            let _ = app.emit(EVENT_CHANGED, ());
        }
        Some(summary)
    }

    pub fn attach(&self, terminal_id: &str) -> Result<TerminalSnapshot> {
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
        Ok(TerminalSnapshot {
            summary: record.summary.clone(),
            scrollback: base64::engine::general_purpose::STANDARD.encode(record.scrollback.bytes()),
            cols: record.cols,
            rows: record.rows,
        })
    }

    /// Frontend keystrokes → PTY master. `data` is the xterm-encoded string
    /// (xterm.js owns key encoding); an exited terminal refuses input with a
    /// stated reason instead of dropping bytes silently.
    pub fn write(&self, terminal_id: &str, data: &str) -> Result<()> {
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
        if !record.summary.live {
            return Err(other("该终端中的 Agent 已退出"));
        }
        let Some(writer) = record.writer.as_mut() else {
            return Err(other("终端输出流不可用"));
        };
        writer
            .write_all(data.as_bytes())
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
        record.cols = cols;
        record.rows = rows;
        record
            .master
            .resize(portable_pty::PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| other(format!("调整终端尺寸失败: {e}")))
    }

    /// App exit: end every embedded Agent. Children die with the PTY master
    /// on Unix anyway; killing explicitly makes it true on ConPTY too.
    pub fn kill_all(&self) {
        let Ok(records) = self.records.lock() else {
            return;
        };
        for record in records.values() {
            let Ok(mut record) = record.lock() else {
                continue;
            };
            let _ = record.killer.kill();
        }
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
    ) -> TerminalSummary {
        let terminal_id = summary.terminal_id.clone();
        let record = std::sync::Arc::new(Mutex::new(TerminalRecord {
            summary: summary.clone(),
            root_agent_session_id,
            identity_bridge,
            scrollback,
            cols: INITIAL_COLS,
            rows: INITIAL_ROWS,
            writer: Some(writer),
            master,
            killer,
        }));
        if let Ok(mut records) = self.records.lock() {
            records.insert(terminal_id.clone(), record.clone());
        }
        // The live set grew: sidebar and other list consumers refetch once.
        if let Some(app) = &self.app {
            let _ = app.emit(EVENT_CHANGED, ());
        }

        // Reader: PTY master → scrollback → webview event. The lock is taken
        // per chunk and dropped BEFORE the emit, so a slow webview never
        // stalls the pump.
        let reader_record = record.clone();
        let reader_app = self.app.clone();
        let reader_id = terminal_id.clone();
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut chunk = [0u8; 4096];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        let encoded = {
                            let Ok(mut record) = reader_record.lock() else {
                                break;
                            };
                            record.scrollback.push(&chunk[..n]);
                            base64::engine::general_purpose::STANDARD.encode(&chunk[..n])
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

        summary
    }
}

/// What an embedded Resume spawn needs beyond the `AgentCommand`. The
/// launcher builds this; [`spawn_embedded`] consumes it. Kept in this module
/// so the dependency direction stays launcher → terminal.
pub struct EmbeddedTarget<'a> {
    pub registry: &'a TerminalRegistry,
    /// None for a NEW-session launch: the session does not exist yet; the
    /// registry entry starts unbound and binds when its native identity is
    /// available in the ingested Session store.
    pub session_id: Option<&'a str>,
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
    // The webview uses xterm; a GUI launch may have no TERM or inherit "dumb",
    // which makes Codex stop before its lifecycle hooks can run.
    wrapped.env("TERM", "xterm-256color");
    let child = pty.spawn_command(&wrapped)?;
    let pid = child.process_id();
    let reader = pty.take_reader()?;
    let writer = pty.take_writer()?;
    let killer = child.clone_killer();

    let summary = TerminalSummary {
        terminal_id: terminal_id.clone(),
        session_id: target.session_id.map(str::to_string),
        identity_revision: 0,
        agent: target.agent,
        cwd: cmd.cwd.as_ref().map(|p| p.to_string_lossy().to_string()),
        created_at: started_at,
        live: true,
        exit_code: None,
        session_title: None,
    };

    let bridge = prepared_identity.map(|prepared| std::sync::Arc::new(prepared.bridge));
    let summary = target.registry.register(
        summary,
        target.root_agent_session_id.map(str::to_string),
        bridge.clone(),
        Scrollback::default(),
        pty.into_master(),
        writer,
        killer,
        reader,
        child,
    );

    if let (Some(bridge), Some(app)) = (bridge, target.registry.app.clone()) {
        let terminal_id = summary.terminal_id.clone();
        bridge.activate(std::sync::Arc::new(move |event| {
            let registry = app.state::<TerminalRegistry>();
            let state = app.state::<crate::commands::AppState>();
            if let Some(current) = registry.report_native_identity(&state.db, &terminal_id, &event)
            {
                if let Some(session_id) = current.session_id {
                    crate::commands::ingestion::enqueue(
                        &app,
                        crate::commands::ingestion::IngestScope::RefreshSession(session_id),
                    );
                } else if let Ok(sources) = state.db.list_ingest_sources() {
                    for source in sources
                        .into_iter()
                        .filter(|source| source.enabled && source.agent == current.agent)
                    {
                        crate::commands::ingestion::enqueue(
                            &app,
                            crate::commands::ingestion::IngestScope::ReconcileSource(source.id),
                        );
                    }
                }
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

    /// `sh` exists on every CI this suite runs on (macOS; the spawn tests are
    /// unix-gated below — ConPTY coverage rides on the same code paths).
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
        crate::storage::Db::open(&dir.join("db.sqlite")).unwrap()
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
        let pty = spawn::PtyPair::open().unwrap();
        let writer = pty.take_writer().unwrap();
        let terminal_id = new_id();
        let record = TerminalRecord {
            summary: TerminalSummary {
                terminal_id: terminal_id.clone(),
                session_id: session_id.map(str::to_string),
                identity_revision: 0,
                agent,
                cwd: Some(fixture_cwd()),
                created_at: now(),
                live: true,
                exit_code: None,
                session_title: None,
            },
            root_agent_session_id: native_id.map(str::to_string),
            identity_bridge: None,
            scrollback: Scrollback::default(),
            cols: INITIAL_COLS,
            rows: INITIAL_ROWS,
            writer: Some(writer),
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

    fn native_event(native_id: &str) -> identity::NativeIdentityEvent {
        identity::NativeIdentityEvent {
            native_id: native_id.into(),
            cwd: None,
            source_path: None,
            source: None,
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
        db.get_session(&id).unwrap().unwrap()
    }

    #[test]
    fn native_discovery_binds_multiple_terminals_even_with_a_pending_resume() {
        let registry = registry();
        let db = binding_db();
        let session = ingested_native(&db, Agent::Codex, "shared-native");
        let pending = registry.reserve_resume(&session.id).unwrap();
        let first = identity_terminal(&registry, Agent::Codex, Some("shared-native"), None);
        let second = identity_terminal(&registry, Agent::Codex, Some("shared-native"), None);

        registry.bind_discovered(&db);
        for id in [&first, &second] {
            let summary = registry.attach(id).unwrap().summary;
            assert_eq!(summary.session_id.as_deref(), Some(session.id.as_str()));
            assert_eq!(summary.identity_revision, 1);
        }
        assert_eq!(registry.list_live().len(), 2);
        assert!(registry
            .resume_reservations
            .lock()
            .unwrap()
            .contains(&session.id));
        drop(pending);
        assert!(registry.has_live(&session.id));
        assert!(registry.reserve_resume(&session.id).is_err());
    }

    #[test]
    fn native_notification_rebinds_a_to_b_despite_another_terminal_and_pending_resume() {
        let registry = registry();
        let db = binding_db();
        let a = ingested_native(&db, Agent::Pi, "native-a");
        let b = ingested_native(&db, Agent::Pi, "native-b");
        let terminal = identity_terminal(&registry, Agent::Pi, Some("native-a"), Some(&a.id));
        let pending = registry.reserve_resume(&b.id).unwrap();
        let other = identity_terminal(&registry, Agent::Pi, Some("native-b"), Some(&b.id));
        let before = registry.attach(&terminal).unwrap().summary;
        let mut event = native_event("native-b");
        event.cwd = Some(format!("{}/other", fixture_cwd()));

        let rebound = registry
            .report_native_identity(&db, &terminal, &event)
            .unwrap();
        assert_eq!(rebound.session_id.as_deref(), Some(b.id.as_str()));
        assert!(rebound.identity_revision > before.identity_revision);
        assert_eq!(rebound.cwd, event.cwd);
        assert_eq!(rebound.session_title.as_deref(), Some("native-b"));
        assert!(registry.for_session(&a.id).is_none());
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
                .list_live()
                .iter()
                .filter(|t| t.session_id.as_deref() == Some(b.id.as_str()))
                .count(),
            2
        );
        drop(pending);
    }

    #[test]
    fn native_switch_to_uningested_b_clears_a_and_waits_only_for_b() {
        let registry = registry();
        let db = binding_db();
        let a = ingested_native(&db, Agent::ClaudeCode, "native-a");
        let terminal =
            identity_terminal(&registry, Agent::ClaudeCode, Some("native-a"), Some(&a.id));
        let waiting = registry
            .report_native_identity(&db, &terminal, &native_event("native-b"))
            .unwrap();
        assert!(waiting.session_id.is_none());
        assert!(waiting.session_title.is_none());
        assert!(registry.for_session(&a.id).is_none());

        registry.bind_discovered(&db);
        let still_waiting = registry.attach(&terminal).unwrap().summary;
        assert!(still_waiting.session_id.is_none());
        assert_eq!(still_waiting.identity_revision, waiting.identity_revision);
        let b = ingested_native(&db, Agent::ClaudeCode, "native-b");
        registry.bind_discovered(&db);
        let bound = registry.attach(&terminal).unwrap().summary;
        assert_eq!(bound.session_id.as_deref(), Some(b.id.as_str()));
        assert!(bound.identity_revision > waiting.identity_revision);
        assert!(registry.for_session(&a.id).is_none());
    }

    #[test]
    fn late_turn_completion_cannot_restore_a_previous_native_identity() {
        let registry = registry();
        let db = binding_db();
        for (agent, completion_source) in [(Agent::ClaudeCode, "Stop"), (Agent::Pi, "agent_end")] {
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

        registry.finish_native_binding(&old_lookup, "native-a", &a);
        let after_old_lookup = registry.attach(&terminal).unwrap().summary;
        assert!(after_old_lookup.session_id.is_none());
        assert_eq!(
            after_old_lookup.identity_revision,
            current.identity_revision
        );
        let b = ingested_native(&db, Agent::Codex, "native-b");
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

        registry.finish_native_binding(&old_lookup, "native-a", &a);
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
    fn unknown_native_identity_does_not_infer_from_prompt_cwd_time_or_launch_intent() {
        let registry = registry();
        let db = binding_db();
        let source = ingested_native(&db, Agent::Codex, "external-native");
        let prompt = format!("{}\n完整首条消息", "long prompt".repeat(100));
        db.commit_ingest(
            &source.id,
            &[crate::domain::ParsedSessionMessage {
                source_message_id: Some("first".into()),
                source_position: "0".into(),
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
        db.insert_launch_intent(&crate::domain::LaunchIntent {
            id: "intent-matched".into(),
            agent: Agent::Codex,
            owner_workstream_id: None,
            cwd: source.cwd.clone(),
            launched_at: ts.clone(),
            matched_session_id: Some(source.id.clone()),
            status: crate::domain::launch_status::MATCHED.into(),
            note: String::new(),
            created_at: ts.clone(),
            updated_at: ts.clone(),
        })
        .unwrap();
        for native_id in [None, Some("not-in-the-store")] {
            let terminal = identity_terminal(&registry, Agent::Codex, native_id, None);
            {
                let records = registry.records.lock().unwrap();
                let mut record = records.get(&terminal).unwrap().lock().unwrap();
                record.summary.created_at = ts.clone();
                record.scrollback.push(prompt.as_bytes());
            }
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
        registry.finish_native_binding(&expected_pi, "same-native", &codex);
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
        registry.finish_native_binding(&pending_lookup, "native-a", &session);
        assert!(registry
            .report_native_identity(&db, &terminal, &native_event("native-a"))
            .is_none());
        registry.bind_discovered(&db);
        assert!(registry.attach(&terminal).is_err());
        assert!(registry.list_live().is_empty());
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
    fn scrollback_push_evicts_oldest_beyond_cap() {
        let mut sb = Scrollback::default();
        sb.push(&[1, 2, 3]);
        assert_eq!(sb.bytes(), vec![1, 2, 3]);
        // A single chunk larger than the cap keeps only its tail.
        let big: Vec<u8> = (0..SCROLLBACK_CAP + 10).map(|i| i as u8).collect();
        sb.push(&big);
        assert_eq!(sb.len(), SCROLLBACK_CAP);
        assert_eq!(sb.bytes()[0], 10_u8);
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
    fn terminal_declares_the_xterm_environment_to_the_agent() {
        let registry = registry();
        let outcome = spawn_embedded(
            &sh(r#"printf 'term=%s' "$TERM""#),
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
                .contains("term=xterm-256color")
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
    fn for_session_prefers_the_live_terminal_over_a_newer_exited_one() {
        let registry = registry();
        let live = spawn_embedded(&sh("sleep 30"), &target(&registry, "s-two")).unwrap();
        let exited = spawn_embedded(&sh("exit 0"), &target(&registry, "s-two")).unwrap();

        // Wait until the second terminal is a recorded EXIT first — before
        // that, for_session legitimately returns the newer (still-live)
        // second terminal and the preference is unobservable.
        until("exited terminal to be recorded", || {
            registry
                .attach(&exited.terminal_id.clone().unwrap())
                .ok()
                .filter(|s| !s.summary.live)
        });
        let summary = registry.for_session("s-two").unwrap();
        assert_eq!(
            summary.terminal_id,
            live.terminal_id.clone().unwrap(),
            "a live terminal must win over a newer exited one"
        );
        registry.kill_all();
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
                root_agent_session_id: None,
                agent: Agent::Codex,
            },
        )
        .unwrap();
        let id = outcome.terminal_id.clone().unwrap();
        assert!(registry.has_live("s-live"));

        registry.close(&id).expect("closed");

        // 记录整体移除：不进运行列表、查不到会话、attach 拒绝——显式关闭
        // 的终端没有回放价值，下一次 Resume 起的是全新终端。
        assert!(!registry.has_live("s-live"));
        assert!(registry.list_live().iter().all(|t| t.terminal_id != id));
        assert!(registry.for_session("s-live").is_none());
        assert!(registry.attach(&id).is_err());
        assert!(registry.close(&id).is_none(), "closing twice is a no-op");
    }
}
