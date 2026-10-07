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

pub mod spawn;

use std::collections::{HashMap, HashSet, VecDeque};
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
/// through a prespecified session id (claude / pi), or a unique match of the
/// submitted first message, Agent, directory and launch time after ingestion.
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
    /// Full submitted prompt for a NEW launch; kept only in memory and never
    /// reconstructed from TUI output or the truncated session title.
    initial_message: Option<String>,
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
    /// Also serializes discovery's query/claim step with Resume reservations.
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

#[derive(Clone)]
struct LaunchEvidence {
    terminal_id: String,
    agent: Agent,
    cwd: Option<String>,
    started_at: String,
    expected_root_session_id: Option<String>,
    initial_message: Option<String>,
}

fn normalized_message(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim()
        .to_string()
}

fn matches_first_message(
    launch: &LaunchEvidence,
    session: &crate::storage::TerminalBindingCandidate,
) -> bool {
    if launch.agent != session.agent
        || (launch.agent == Agent::Antigravity
            && session.source_kind != "antigravity_cli_conversation")
    {
        return false;
    }
    let Some(started) = parse_ts(&launch.started_at) else {
        return false;
    };
    let Some(source_started) = session.started_at.as_deref().and_then(parse_ts) else {
        return false;
    };
    // Some source stores have only second precision. A bounded startup window
    // prevents a much later conversation with the same prompt being claimed.
    if source_started < started - chrono::Duration::seconds(1)
        || source_started > started + chrono::Duration::minutes(5)
    {
        return false;
    }
    let Some(cwd) = launch
        .cwd
        .as_deref()
        .and_then(crate::workspace::normalize_path)
    else {
        return false;
    };
    let Some(source_cwd) = session
        .cwd
        .as_deref()
        .and_then(crate::workspace::normalize_path)
    else {
        return false;
    };
    if !crate::workspace::identity::same_location(&cwd, &source_cwd) {
        return false;
    }
    let Some(message) = launch.initial_message.as_deref().map(normalized_message) else {
        return false;
    };
    !message.is_empty()
        && session
            .first_user_message
            .as_deref()
            .map(normalized_message)
            .as_deref()
            == Some(message.as_str())
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

    fn waiting_records(
        records: &HashMap<String, std::sync::Arc<Mutex<TerminalRecord>>>,
    ) -> Vec<LaunchEvidence> {
        records
            .values()
            .filter_map(|record| {
                let record = record.lock().ok()?;
                (record.summary.live && record.summary.session_id.is_none()).then(|| {
                    LaunchEvidence {
                        terminal_id: record.summary.terminal_id.clone(),
                        agent: record.summary.agent,
                        cwd: record.summary.cwd.clone(),
                        started_at: record.summary.created_at.clone(),
                        expected_root_session_id: record.expected_root_session_id.clone(),
                        initial_message: record.initial_message.clone(),
                    }
                })
            })
            .collect()
    }

    /// Match ingested roots to live NEW terminals. Native IDs take priority;
    /// otherwise the complete submitted first message, Agent, strict cwd and
    /// launch time must match, with exactly one candidate on both sides.
    /// The claim gate covers DB reads through binding so a concurrent Resume
    /// cannot reserve a session while its existing terminal is being matched.
    pub fn bind_discovered(&self, db: &crate::storage::Db) {
        let Ok(reservations) = self.resume_reservations.lock() else {
            return;
        };
        let waiting = {
            let Ok(records) = self.records.lock() else {
                return;
            };
            Self::waiting_records(&records)
        };
        let mut native_matches = HashMap::new();
        let mut earliest = HashMap::<&str, (Agent, chrono::DateTime<chrono::Utc>)>::new();
        for launch in &waiting {
            if let Some(expected) = &launch.expected_root_session_id {
                if let Ok(Some(session)) = db.find_session_by_root_agent_id(launch.agent, expected)
                {
                    if session.trashed_at.is_none() {
                        native_matches
                            .insert((launch.agent.as_str(), expected.clone()), session.id);
                    }
                }
            } else if launch.initial_message.is_some() {
                if let Some(started) = parse_ts(&launch.started_at) {
                    earliest
                        .entry(launch.agent.as_str())
                        .and_modify(|(_, ts)| *ts = (*ts).min(started))
                        .or_insert((launch.agent, started));
                }
            }
        }
        let mut candidates = Vec::new();
        for &(agent, started) in earliest.values() {
            if let Ok(mut roots) = db.terminal_binding_candidates(
                agent,
                &(started - chrono::Duration::seconds(1)).to_rfc3339(),
            ) {
                candidates.append(&mut roots);
            }
        }
        // Include NEW records registered while DB queries ran in the uniqueness
        // decision, holding records through the decision and commit.
        let Ok(records) = self.records.lock() else {
            return;
        };
        let waiting = Self::waiting_records(&records);
        // A slow spawn may register AFTER our first snapshot despite starting
        // earlier. Its source could have fallen below the queried time floor.
        // Defer that Agent's text claims until a normal later ingestion pass.
        let incomplete_agents: HashSet<_> = waiting
            .iter()
            .filter_map(|launch| {
                if launch.expected_root_session_id.is_some() || launch.initial_message.is_none() {
                    return None;
                }
                let started = parse_ts(&launch.started_at)?;
                (!earliest
                    .get(launch.agent.as_str())
                    .is_some_and(|(_, floor)| started >= *floor))
                .then_some(launch.agent.as_str())
            })
            .collect();
        let native_sessions: HashSet<_> = native_matches.values().cloned().collect();
        let matches: Vec<(String, Vec<String>)> = waiting
            .iter()
            .map(|launch| {
                let ids = if let Some(expected) = &launch.expected_root_session_id {
                    native_matches
                        .get(&(launch.agent.as_str(), expected.clone()))
                        .cloned()
                        .into_iter()
                        .collect()
                } else if incomplete_agents.contains(launch.agent.as_str()) {
                    Vec::new()
                } else {
                    candidates
                        .iter()
                        .filter(|session| {
                            !native_sessions.contains(&session.session_id)
                                && matches_first_message(launch, session)
                        })
                        .map(|session| session.session_id.clone())
                        .collect()
                };
                (launch.terminal_id.clone(), ids)
            })
            .collect();
        let mut claim_counts = HashMap::<&str, usize>::new();
        for (_, ids) in &matches {
            for id in ids {
                *claim_counts.entry(id).or_default() += 1;
            }
        }
        let mut bound = Vec::new();
        {
            let mut occupied: HashSet<String> = records
                .values()
                .filter_map(|record| {
                    let record = record.lock().ok()?;
                    record
                        .summary
                        .live
                        .then(|| record.summary.session_id.clone())
                        .flatten()
                })
                .collect();
            for (terminal_id, ids) in &matches {
                if ids.len() != 1 {
                    continue;
                }
                let session_id = &ids[0];
                if claim_counts.get(session_id.as_str()) != Some(&1)
                    || reservations.contains(session_id)
                    || occupied.contains(session_id)
                {
                    continue;
                }
                let Some(record) = records.get(terminal_id) else {
                    continue;
                };
                let Ok(mut record) = record.lock() else {
                    continue;
                };
                if !record.summary.live || record.summary.session_id.is_some() {
                    continue;
                }
                record.summary.session_id = Some(session_id.clone());
                occupied.insert(session_id.clone());
                bound.push(TerminalBound {
                    terminal_id: terminal_id.clone(),
                    session_id: session_id.clone(),
                });
            }
        }
        drop(records);
        drop(reservations);
        if let Some(app) = &self.app {
            for fact in &bound {
                let _ = app.emit(EVENT_BOUND, fact);
            }
            if !bound.is_empty() {
                let _ = app.emit(EVENT_CHANGED, ());
            }
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
        let summary = {
            let Ok(mut record) = removed.lock() else {
                return None;
            };
            if record.summary.live {
                let _ = record.killer.kill();
            }
            record.summary.clone()
        };
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
        expected_root_session_id: Option<String>,
        initial_message: Option<String>,
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
            initial_message,
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
    /// registry entry starts unbound and binds after ingestion supplies identity
    /// or unique first-message evidence.
    pub session_id: Option<&'a str>,
    /// The prespecified session id the command line carries (claude / pi);
    /// None when the CLI generates its own ids (codex / agy).
    pub expected_root_session_id: Option<&'a str>,
    pub initial_message: Option<&'a str>,
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
        created_at: started_at,
        live: true,
        exit_code: None,
        session_title: None,
    };

    let summary = target.registry.register(
        summary,
        target.expected_root_session_id.map(str::to_string),
        target.initial_message.map(str::to_string),
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
            initial_message: None,
            agent: Agent::Codex,
        }
    }

    fn evidence() -> LaunchEvidence {
        LaunchEvidence {
            terminal_id: "terminal".into(),
            agent: Agent::Codex,
            cwd: Some(if cfg!(windows) { r"C:\repo" } else { "/repo" }.into()),
            started_at: "2026-10-07T12:00:00.500Z".into(),
            expected_root_session_id: None,
            initial_message: Some(format!("{}\r\n保留  内部空格\r\n", "完整消息".repeat(150))),
        }
    }

    fn candidate(launch: &LaunchEvidence) -> crate::storage::TerminalBindingCandidate {
        crate::storage::TerminalBindingCandidate {
            session_id: "session".into(),
            agent: launch.agent,
            source_kind: "codex_rollout".into(),
            cwd: launch.cwd.clone().map(|cwd| format!("{cwd}/")),
            started_at: Some("2026-10-07T12:00:00Z".into()),
            first_user_message: launch.initial_message.as_deref().map(normalized_message),
        }
    }

    #[test]
    fn first_message_matching_requires_full_text_strict_cwd_agent_and_time() {
        let launch = evidence();
        let good = candidate(&launch);
        assert!(matches_first_message(&launch, &good));
        let mut wrong = good.clone();
        wrong.first_user_message = Some(
            normalized_message(launch.initial_message.as_ref().unwrap())
                .chars()
                .take(400)
                .collect(),
        );
        assert!(
            !matches_first_message(&launch, &wrong),
            "titles/prefixes are insufficient"
        );
        wrong = good.clone();
        wrong.first_user_message = wrong.first_user_message.map(|text| text.replace("  ", " "));
        assert!(
            !matches_first_message(&launch, &wrong),
            "internal whitespace is evidence"
        );
        for cwd in [
            None,
            Some("relative/path".into()),
            Some(format!("{}/child", launch.cwd.as_ref().unwrap())),
        ] {
            wrong = good.clone();
            wrong.cwd = cwd;
            assert!(!matches_first_message(&launch, &wrong));
        }
        for ts in [
            None,
            Some("bad"),
            Some("2026-10-07T11:59:59Z"),
            Some("2026-10-07T12:05:01Z"),
        ] {
            wrong = good.clone();
            wrong.started_at = ts.map(str::to_string);
            assert!(!matches_first_message(&launch, &wrong));
        }
        wrong = good.clone();
        wrong.agent = Agent::ClaudeCode;
        assert!(!matches_first_message(&launch, &wrong));
        wrong = good.clone();
        wrong.first_user_message = None;
        assert!(!matches_first_message(&launch, &wrong));
        let mut blank = launch.clone();
        blank.initial_message = Some("  ".into());
        wrong.first_user_message = Some("  ".into());
        assert!(!matches_first_message(&blank, &wrong));
    }

    #[test]
    fn agy_first_message_matching_excludes_desktop_sources() {
        let mut launch = evidence();
        launch.agent = Agent::Antigravity;
        let mut source = candidate(&launch);
        source.source_kind = "antigravity_ide_conversation".into();
        assert!(!matches_first_message(&launch, &source));
        source.source_kind = "antigravity_cli_conversation".into();
        assert!(matches_first_message(&launch, &source));
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

    #[cfg(unix)]
    fn new_with_message(registry: &TerminalRegistry, message: &str) -> String {
        let mut cmd = sh("sleep 30");
        cmd.cwd = Some("/tmp".into());
        spawn_embedded(
            &cmd,
            &EmbeddedTarget {
                registry,
                session_id: None,
                expected_root_session_id: None,
                initial_message: Some(message),
                agent: Agent::Codex,
            },
        )
        .unwrap()
        .terminal_id
        .unwrap()
    }

    #[cfg(unix)]
    fn binding_db() -> crate::storage::Db {
        let dir = std::env::temp_dir().join(format!("noending-term-binding-{}", new_id()));
        crate::storage::Db::open(&dir.join("db.sqlite")).unwrap()
    }

    #[cfg(unix)]
    fn ingested_root(db: &crate::storage::Db, native_id: &str, prompt: &str) -> String {
        let ts = now();
        let id = db
            .upsert_logical_session(
                Agent::Codex,
                native_id,
                None,
                None,
                Some("/tmp"),
                None,
                None,
                Some(&ts),
                Some(&ts),
                "codex_rollout",
                "/tmp/source.jsonl",
                &serde_json::json!({}),
            )
            .unwrap()
            .0;
        db.commit_ingest(
            &id,
            &[crate::domain::ParsedSessionMessage {
                source_message_id: Some("first".into()),
                source_position: "0".into(),
                ts: Some(ts),
                role: crate::domain::SessionMessageRole::User,
                content: prompt.into(),
            }],
            &crate::domain::SourceCursorUpdate {
                file_identity: native_id.into(),
                generation: 0,
                byte_offset: 100,
                last_seen_size: 100,
                mtime: None,
                start_byte_offset: 0,
                prefix_hash: String::new(),
            },
        )
        .unwrap();
        id
    }

    #[cfg(unix)]
    #[test]
    fn discovered_full_first_message_binds_the_original_terminal_and_blocks_resume() {
        let registry = registry();
        let db = binding_db();
        let prompt = format!("{}\n最后一行", "超过标题截断长度".repeat(100));
        let terminal = new_with_message(&registry, &prompt);
        let session = ingested_root(&db, "native-new", &prompt);
        registry.bind_discovered(&db);
        assert_eq!(
            registry.for_session(&session).unwrap().terminal_id,
            terminal
        );
        assert!(registry.reserve_resume(&session).is_err());
        registry.kill_all();
    }

    #[cfg(unix)]
    #[test]
    fn discovered_message_requires_uniqueness_in_both_directions() {
        for (terminal_count, source_count) in [(1, 2), (2, 1)] {
            let registry = registry();
            let db = binding_db();
            for _ in 0..terminal_count {
                new_with_message(&registry, "same prompt");
            }
            for i in 0..source_count {
                ingested_root(&db, &format!("root-{i}"), "same prompt");
            }
            registry.bind_discovered(&db);
            assert_eq!(registry.list_live().len(), terminal_count);
            assert!(registry
                .list_live()
                .iter()
                .all(|terminal| terminal.session_id.is_none()));
            registry.kill_all();
        }
    }

    #[cfg(unix)]
    #[test]
    fn discovery_respects_pending_and_live_resume_claims() {
        let registry = registry();
        let db = binding_db();
        let terminal = new_with_message(&registry, "prompt");
        let session = ingested_root(&db, "root", "prompt");
        let pending = registry.reserve_resume(&session).unwrap();
        registry.bind_discovered(&db);
        assert!(registry.for_session(&session).is_none());
        drop(pending);
        registry.bind_discovered(&db);
        assert_eq!(
            registry.for_session(&session).unwrap().terminal_id,
            terminal
        );
        // A second matching NEW must not take a session already held by a live terminal.
        let second = new_with_message(&registry, "prompt");
        registry.bind_discovered(&db);
        assert!(registry
            .attach(&second)
            .unwrap()
            .summary
            .session_id
            .is_none());
        registry.kill_all();
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
                initial_message: None,
                agent: Agent::Codex,
            },
        )
        .unwrap();
        let id = outcome.terminal_id.clone().unwrap();

        // LaunchIntent ownership is not terminal identity. Without a submitted
        // first message (or a native ID), matching agent + cwd cannot bind.
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
            "a terminal without first-message evidence must stay unbound"
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
                initial_message: Some("message evidence is unnecessary for a native ID"),
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
                expected_root_session_id: None,
                initial_message: None,
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
