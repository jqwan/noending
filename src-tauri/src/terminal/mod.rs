//! Embedded terminal backend — one real PTY per embedded Resume.
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

pub mod spawn;

use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::sync::Mutex;

use base64::Engine;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

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
/// Per-bind fact: an unbound terminal just received its session identity.
/// Payload: [`TerminalBound`]. The standalone terminal view listens for this
/// to light up its session-detail entry the moment binding happens.
pub const EVENT_BOUND: &str = "terminal-bound";

/// The payload of `terminal-bound`.
#[derive(Debug, Clone, Serialize)]
pub struct TerminalBound {
    pub terminal_id: String,
    pub session_id: String,
}

/// The facts a terminal subpage needs to decide what it is looking at.
/// `session_id` is None for a NEW-session terminal: the Agent has been
/// launched but its session file is not on disk yet. Binding happens only
/// through exact facts — a prespecified session id (claude / pi) matched at
/// discovery, or the session page's on-demand verified match — never by
/// guessing from cwd + timing alone.
#[derive(Debug, Clone, Serialize)]
pub struct TerminalSummary {
    pub terminal_id: String,
    pub session_id: Option<String>,
    pub agent: Agent,
    pub cwd: Option<String>,
    pub created_at: String,
    pub live: bool,
    pub exit_code: Option<i32>,
    /// Filled by the `terminal_list` command (a DB lookup the registry
    /// itself never does): the bound session's display title. None for
    /// unbound terminals.
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

/// Remove ANSI escape sequences (CSI `ESC [ … final`, OSC `ESC ] … BEL/ST`,
/// and two-byte escapes) so TUI-rendered text becomes searchable. Raw PTY
/// bytes are full of cursor addressing; the message content survives as the
/// plain runs between sequences.
fn strip_ansi(bytes: &[u8]) -> String {
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != 0x1b {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        if i + 1 >= bytes.len() {
            break;
        }
        match bytes[i + 1] {
            b'[' => {
                i += 2;
                while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                    i += 1;
                }
                i += 1; // final byte
            }
            b']' => {
                i += 2;
                while i < bytes.len() {
                    if bytes[i] == 0x07 {
                        i += 1;
                        break;
                    }
                    if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            _ => i += 2, // two-byte escape (charset, save cursor, …)
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// TUIs wrap text at terminal width and re-render on resize: whitespace of
/// any kind must not break the needle. Collapsing everything (haystack and
/// needle alike) makes "hello world" match "hello\r\n  world" and costs
/// nothing in precision — the message text is high-entropy.
fn normalize_ws(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// The searchable form of a first message: whitespace-collapsed, and cut to
/// a prefix when very long — a TUI may truncate or the scrollback ring may
/// hold only the opening. A miss here means "no match", never a wrong bind.
fn search_needle(message: &str) -> String {
    let full = normalize_ws(message);
    const MAX: usize = 120;
    if full.chars().count() <= MAX {
        return full;
    }
    full.chars().take(80).collect()
}

/// RFC3339 parse for ordering comparisons; the registry writes `now()`
/// (chrono UTC) and ingestion timestamps share the family, but a foreign
/// format must not panic the matcher — None just means "cannot verify".
fn parse_ts(ts: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|t| t.with_timezone(&chrono::Utc))
}

/// One embedded terminal. Fields are behind one per-record mutex because the
/// reader thread (PTY output), the wait thread (exit), and the frontend
/// commands (input / resize / attach) all touch it — each holds the lock only
/// across its own field update, never across a Db call and never across an
/// emit, so contention is bounded to microseconds.
struct TerminalRecord {
    summary: TerminalSummary,
    /// For a NEW-session launch on a CLI that accepts a prespecified session
    /// id (claude / pi): the identity NoEnding generated before spawn. The
    /// discovered session will carry it as `root_agent_session_id`, which is
    /// what `bind_discovered` matches on — exact, never inferred.
    expected_root_session_id: Option<String>,
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
    app: Option<AppHandle>,
}

impl TerminalRegistry {
    pub fn new(app: Option<AppHandle>) -> Self {
        Self {
            records: Mutex::new(HashMap::new()),
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
    /// prepare 侧的 embedded 双开拒绝读这里：同一会话同时只有一个活内嵌
    /// 终端，第二个 Resume 必须先等它退出。
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

    /// Attach `session_id` to an unbound terminal. A terminal already bound
    /// (to any session) is left alone — binding happens once, from exact
    /// facts only. The transition is announced twice: `terminal-bound` (with
    /// the ids, for the attached terminal view) and `terminals-changed` (the
    /// sidebar entry can now show the session's name).
    pub fn bind(&self, terminal_id: &str, session_id: &str) {
        let Ok(records) = self.records.lock() else {
            return;
        };
        let Some(record) = records.get(terminal_id).cloned() else {
            return;
        };
        drop(records);
        let bound = {
            let Ok(mut record) = record.lock() else {
                return;
            };
            if record.summary.session_id.is_some() {
                return;
            }
            record.summary.session_id = Some(session_id.to_string());
            true
        };
        if bound {
            if let Some(app) = &self.app {
                let _ = app.emit(
                    EVENT_BOUND,
                    TerminalBound {
                        terminal_id: terminal_id.to_string(),
                        session_id: session_id.to_string(),
                    },
                );
                let _ = app.emit(EVENT_CHANGED, ());
            }
        }
    }

    /// Discovery's bind step. Only exact facts bind here: a NEW-session
    /// terminal whose command line carried a prespecified session id
    /// (claude / pi) is matched against the discovered session's
    /// `root_agent_session_id`. CLIs that generate their own ids (codex /
    /// agy) never bind automatically — they bind on demand through the
    /// session page's verified match, so a lookalike session in the same
    /// directory can never steal the identity.
    pub fn bind_discovered(&self, db: &crate::storage::Db) {
        let waiting: Vec<(String, Agent, String)> = {
            let Ok(records) = self.records.lock() else {
                return;
            };
            records
                .values()
                .filter_map(|record| {
                    let record = record.lock().ok()?;
                    (record.summary.live
                        && record.summary.session_id.is_none()
                        && record.expected_root_session_id.is_some())
                    .then(|| {
                        (
                            record.summary.terminal_id.clone(),
                            record.summary.agent,
                            record
                                .expected_root_session_id
                                .clone()
                                .expect("checked above"),
                        )
                    })
                })
                .collect()
        };
        for (terminal_id, agent, expected) in waiting {
            if let Ok(Some(session)) = db.find_session_by_root_agent_id(agent, &expected) {
                self.bind(&terminal_id, &session.id);
            }
        }
    }

    /// The session page's on-demand match (the terminal entry click). For a
    /// session whose CLI generates its own ids (codex / agy) there is no
    /// birth identity to match on; binding happens HERE instead — the user
    /// is asking for this session's terminal, so we verify before we claim:
    /// same Agent, same working directory, the terminal existed when the
    /// first message was sent, and the message text itself is visible in the
    /// terminal's scrollback. Three independent facts agreeing is as close to
    /// certain as the file side allows; a miss means "launch a fresh Resume
    /// terminal", never "bind anyway".
    ///
    /// Bindings made here are as trustworthy as prespecified-id ones: the
    /// terminal view may open its session-detail entry, the sidebar may show
    /// the session's name.
    pub fn match_unbound(
        &self,
        session_id: &str,
        agent: Agent,
        cwd: &str,
        first_message_at: &str,
        first_message: &str,
    ) -> Option<TerminalSummary> {
        let needle = search_needle(first_message);
        if needle.is_empty() {
            return None;
        }
        let sent_at = parse_ts(first_message_at);
        // The records guard covers only the scan: bind() takes the same lock,
        // and this mutex is not reentrant (a guard may never span a same-half
        // call — the storage-concurrency rule, again).
        let best = {
            let records = self.records.lock().ok()?;
            let mut best: Option<(TerminalSummary, String)> = None;
            for record in records.values() {
                let Ok(record) = record.lock() else { continue };
                if !record.summary.live
                    || record.summary.session_id.is_some()
                    || record.summary.agent != agent
                    || record.summary.cwd.as_deref() != Some(cwd)
                {
                    continue;
                }
                // The terminal must have existed when the message was sent.
                let created = parse_ts(&record.summary.created_at);
                match (&created, &sent_at) {
                    (Some(c), Some(s)) if c > s => continue,
                    (None, _) if sent_at.is_some() => continue,
                    _ => {}
                }
                let haystack = normalize_ws(&strip_ansi(&record.scrollback.bytes()));
                if !haystack.contains(&needle) {
                    continue;
                }
                let newer = best
                    .as_ref()
                    .map(|(prev, _)| record.summary.created_at > prev.created_at)
                    .unwrap_or(true);
                if newer {
                    best = Some((record.summary.clone(), record.summary.terminal_id.clone()));
                }
            }
            best
        };
        let (summary, terminal_id) = best?;
        self.bind(&terminal_id, session_id);
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
        expected_root_session_id: Option<String>,
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
            expected_root_session_id,
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
                        let (encoded, unbound_agent) = {
                            let Ok(mut record) = reader_record.lock() else {
                                break;
                            };
                            record.scrollback.push(&chunk[..n]);
                            // Output is the downstream signal of "something
                            // new to pick up": a TUI writing its session file
                            // cannot do so silently. Unbound terminals only —
                            // the throttle gate lives in the ingestion side
                            // and the branch disappears once bound.
                            let unbound = (record.summary.live
                                && record.summary.session_id.is_none())
                            .then_some(record.summary.agent);
                            (
                                base64::engine::general_purpose::STANDARD.encode(&chunk[..n]),
                                unbound,
                            )
                        };
                        if let Some(app) = &reader_app {
                            let _ = app.emit(&format!("{EVENT_OUTPUT_PREFIX}{reader_id}"), encoded);
                            if let Some(agent) = unbound_agent {
                                crate::commands::ingestion::trigger_unbound_agent(app, agent);
                            }
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
            if let Ok(mut record) = wait_record.lock() {
                record.summary.live = false;
                record.summary.exit_code = exit_code;
                record.writer = None;
            }
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
    /// registry entry starts unbound and binds later — by prespecified-id
    /// match at discovery, or the session page's on-demand verified match.
    pub session_id: Option<&'a str>,
    /// The prespecified session id the command line carries (claude / pi);
    /// None when the CLI generates its own ids (codex / agy).
    pub expected_root_session_id: Option<&'a str>,
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
    let terminal_id = new_id();
    let pty = spawn::PtyPair::open()?;
    let child = pty.spawn_command(&spawn::wrapped_command(cmd))?;
    let pid = child.process_id();
    let reader = pty.take_reader()?;
    let writer = pty.take_writer()?;
    let killer = child.clone_killer();

    let summary = TerminalSummary {
        terminal_id: terminal_id.clone(),
        session_id: target.session_id.map(str::to_string),
        agent: target.agent,
        cwd: cmd.cwd.as_ref().map(|p| p.to_string_lossy().to_string()),
        created_at: now(),
        live: true,
        exit_code: None,
        session_title: None,
    };

    let summary = target.registry.register(
        summary,
        target.expected_root_session_id.map(str::to_string),
        Scrollback::default(),
        pty.into_master(),
        writer,
        killer,
        reader,
        child,
    );

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
            expected_root_session_id: None,
            agent: Agent::Codex,
        }
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
    fn a_codex_terminal_stays_unbound_even_when_an_intent_claims_a_session() {
        let registry = registry();
        let mut cmd = sh("sleep 30");
        cmd.cwd = Some(std::path::PathBuf::from("/tmp"));
        let outcome = spawn_embedded(
            &cmd,
            &EmbeddedTarget {
                registry: &registry,
                session_id: None,
                expected_root_session_id: None,
                agent: Agent::Codex,
            },
        )
        .unwrap();
        let id = outcome.terminal_id.clone().unwrap();

        // codex generates its own session ids, so nothing on the file side
        // can identify "its" session with certainty. Even a MATCHED intent
        // carrying the same agent + cwd must NOT bind the terminal — the
        // old cwd+timing inference is retired; binding happens only through
        // the session page's verified match.
        let root =
            std::env::temp_dir().join(format!("noending-term-nobind-{}", crate::storage::new_id()));
        std::fs::create_dir_all(&root).unwrap();
        let db = crate::storage::Db::open(&root.join("db.sqlite")).unwrap();
        let ts = crate::storage::now();
        db.insert_launch_intent(&crate::domain::LaunchIntent {
            id: "intent-1".into(),
            launch_type: "new".into(),
            agent: Agent::Codex,
            owner_workstream_id: None,
            cwd: Some("/tmp".into()),
            process_id: None,
            launched_at: ts.clone(),
            matched_session_id: Some("s-external".into()),
            status: crate::domain::launch_status::MATCHED.into(),
            note: String::new(),
            created_at: ts.clone(),
            updated_at: ts,
        })
        .unwrap();

        registry.bind_discovered(&db);

        assert!(registry.for_session("s-external").is_none());
        assert!(
            registry
                .list_live()
                .iter()
                .any(|t| t.terminal_id == id && t.session_id.is_none()),
            "an id-less terminal is never auto-bound"
        );
        registry.kill_all();
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
                expected_root_session_id: Some("root-born-identity"),
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
    fn the_session_page_match_binds_a_verified_unbound_terminal() {
        let registry = registry();
        let mut cmd = sh("echo waiting; sleep 30");
        cmd.cwd = Some(std::path::PathBuf::from("/tmp"));
        let outcome = spawn_embedded(
            &cmd,
            &EmbeddedTarget {
                registry: &registry,
                session_id: None,
                expected_root_session_id: None,
                agent: Agent::Codex,
            },
        )
        .unwrap();
        let id = outcome.terminal_id.clone().unwrap();

        let ts = crate::storage::now();
        // Polls until the echo reached the scrollback; a None before that is
        // the honest "not verified yet", not a failure.
        let matched = until("the verified match lands", || {
            registry
                .match_unbound("s-page", Agent::Codex, "/tmp", &ts, "waiting")
                .map(|s| s.terminal_id)
        });
        assert_eq!(matched, id);
        assert!(registry.for_session("s-page").is_some());

        // A different session asking for the same directory gets nothing:
        // the terminal is bound once and its identity is not for sale.
        assert!(registry
            .match_unbound("s-other", Agent::Codex, "/tmp", &ts, "waiting")
            .is_none());
        // Wrong directory never matches.
        let registry2 = TerminalRegistry::new(None);
        let mut cmd2 = sh("echo hello; sleep 30");
        cmd2.cwd = Some(std::path::PathBuf::from("/elsewhere"));
        spawn_embedded(
            &cmd2,
            &EmbeddedTarget {
                registry: &registry2,
                session_id: None,
                expected_root_session_id: None,
                agent: Agent::Codex,
            },
        )
        .unwrap();
        assert!(
            registry2
                .match_unbound("s-page", Agent::Codex, "/tmp", &ts, "waiting")
                .is_none(),
            "cwd is a hard filter"
        );
        registry.kill_all();
        registry2.kill_all();
    }

    #[test]
    fn ansi_stripping_keeps_plain_text_searchable() {
        // CSI color + cursor addressing + OSC title around the message.
        let raw = b"\x1b[2J\x1b[1;1H\x1b]0;my title\x07\x1b[32mhello\x1b[0m world";
        let plain = strip_ansi(raw);
        assert!(normalize_ws(&plain).contains("helloworld"));
    }

    #[test]
    fn search_needle_handles_wrapped_and_long_messages() {
        let message = "first line\nsecond   line";
        let haystack = "prompt> first\r\nline second line $";
        assert!(normalize_ws(&strip_ansi(haystack.as_bytes())).contains(&search_needle(message)));
        let long: String = std::iter::repeat("甲").take(300).collect();
        let needle = search_needle(&long);
        assert_eq!(
            needle.chars().count(),
            80,
            "long messages match on a prefix"
        );
    }
}
