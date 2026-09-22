//! Agent Adapter layer.
//!
//! Each adapter owns: session directory layout, file format, session id,
//! optional environment info, resume parameters, new-session invocation.
//! Upper layers must never reference `~/.codex` / `~/.claude` / `~/.pi`
//! directly — that is PlatformPaths' job.
//!
//! Context Integrity rules enforced here:
//! - adapters never generate shell fragments (no `$(cat …)`): context is
//!   passed as a real argv string produced by [`context_prompt`];
//! - [`read_jsonl_delta`] classifies every read as append / truncate /
//!   rewrite / file-replacement and only ever returns the *new* portion;
//!   re-ingesting already-seen content is prevented by the storage layer's
//!   content-identity dedup, so compaction can never overwrite history.

pub mod autoclaw;
pub mod claude;
pub mod codex;
pub mod dsh;
pub mod gemini;
pub mod pi;
pub mod qoder;
pub mod workbuddy;
pub mod zcode;

use std::io::BufRead;
use std::path::{Path, PathBuf};

use crate::domain::{Agent, ParsedEvent, Session, SourceCursor};
use crate::error::{other, AppError, Result};
use crate::platform::exec_resolver::AgentInstallation;

// Re-export so adapter submodules and callers can share one import site.
pub use crate::domain::ReadDelta;

/// Declarative command produced by an adapter; execution is delegated to
/// the PlatformLauncher so OS differences stay out of adapters.
///
/// The adapter decides WHERE a prompt/context goes (argument position and
/// order); the payload is always the literal string, never a shell
/// expression. The Platform layer owns how argv is safely quoted.
#[derive(Debug, Clone)]
pub struct AgentCommand {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
}

impl AgentCommand {
    /// Human-readable rendering for diagnostics / LaunchResult.
    pub fn display(&self) -> String {
        let mut parts = vec![self.program.clone()];
        parts.extend(self.args.iter().map(|a| {
            if a.contains(' ') || a.contains('\n') || a.contains('"') {
                format!("{:?}", a)
            } else {
                a.clone()
            }
        }));
        parts.join(" ")
    }
}

/// One discovered external session before ingestion.
#[derive(Debug, Clone)]
pub struct DiscoveredSession {
    pub agent: Agent,
    pub agent_session_id: String,
    pub path: PathBuf,
    pub cwd: Option<String>,
    pub started_at: Option<String>,
    pub last_activity_at: Option<String>,
    pub first_user_text: Option<String>,
    pub parent_agent_session_id: Option<String>,
}

/// Options for a headless ("exec"/print-mode) invocation of the agent CLI.
/// The Workspace Assistant uses these to run intelligence through the
/// user's already-authenticated agent CLIs.
#[derive(Debug, Clone, Default)]
pub struct ExecOptions {
    pub model: Option<String>,
    pub provider: Option<String>, // pi: --provider (e.g. openai-codex, lmstudio)
    pub effort: Option<String>,   // codex model_reasoning_effort / pi --thinking
}

impl ExecOptions {
    pub fn is_default(&self) -> bool {
        self.model.is_none() && self.provider.is_none() && self.effort.is_none()
    }

    /// Audit rendering of what NoEnding will actually pass: `agent-default`
    /// means no runtime flag at all, so a record can never be read as
    /// "NoEnding chose this model" when the Agent did.
    pub fn override_summary(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(m) = &self.model {
            parts.push(format!("model={m}"));
        }
        if let Some(p) = &self.provider {
            parts.push(format!("provider={p}"));
        }
        if let Some(e) = &self.effort {
            parts.push(format!("effort={e}"));
        }
        if parts.is_empty() {
            "agent-default".to_string()
        } else {
            parts.join(",")
        }
    }
}

/// Read the context bundle file and return its literal content.
///
/// This replaces the old `$(cat 'file')` shell-substitution helper: the
/// content travels as a single argv element (quoted by the Platform layer),
/// so the agent receives exactly this text on every OS.
pub fn context_prompt(context_file: Option<&Path>) -> Result<Option<String>> {
    match context_file {
        None => Ok(None),
        Some(p) => Ok(Some(std::fs::read_to_string(p).map_err(|e| {
            other(format!("读取上下文文件失败 {}: {}", p.display(), e))
        })?)),
    }
}

/// A parsed source line, before NoEnding assigns sequence/identity.
pub struct ParsedLine {
    pub kind: String,
    pub text: Option<String>,
    pub source_event_id: Option<String>,
    pub metadata: serde_json::Value,
}

/// Stable identity of the file itself (not its content): inode/device on
/// unix, canonical path + creation time elsewhere. A change of identity
/// means the file at this path was replaced — never an append.
pub fn file_identity(path: &Path) -> String {
    match std::fs::metadata(path) {
        Ok(meta) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                format!("unix:dev:{}:ino:{}", meta.dev(), meta.ino())
            }
            #[cfg(windows)]
            {
                let canon = std::fs::canonicalize(path)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| path.to_string_lossy().to_string());
                let created = meta
                    .created()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                format!("win:path:{}:created:{}", canon, created)
            }
            #[cfg(not(any(unix, windows)))]
            {
                let canon = std::fs::canonicalize(path)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| path.to_string_lossy().to_string());
                format!("path:{}", canon)
            }
        }
        Err(_) => String::new(),
    }
}

/// Epoch milliseconds → RFC3339, the spelling every other agent's transcripts
/// already use for their timestamps.
pub fn ms_epoch_to_rfc3339(ms: i64) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(ms).map(|t| t.to_rfc3339())
}

pub fn mtime_secs(meta: &std::fs::Metadata) -> Option<f64> {
    let t: chrono::DateTime<chrono::Utc> = meta.modified().ok()?.into();
    Some(t.timestamp() as f64 + t.timestamp_subsec_nanos() as f64 / 1e9)
}

/// Content fingerprint: which agent wrote this session file?
///
/// Filename conventions (rollout-*.jsonl, *.jsonl under some directory) are
/// pre-filters, not guarantees — and no format except Codex names its
/// writer. The on-disk formats are mutually exclusive *except* for Qoder,
/// whose transcript is Claude's plus Qoder-only lines, so order matters:
/// - Codex: every line is the `{ordinal, payload, type}` envelope;
/// - Qoder: one of its own line types (`workspace-directories`,
///   `runtime-config`, `worktree-state`, `active-leaf`, `last-prompt`) —
///   checked before Claude, or every Qoder file would read as Claude;
/// - Claude Code: event lines carry `{sessionId, parentUuid/uuid, message}`;
/// - Pi: `{type:"session", id}` header or `{parentId, provider|modelId|
///   thinkingLevel}` event lines;
/// - WorkBuddy: `{type:"message", role, content:[…]}` with the payload at the
///   top level, plus its own `ai-title` / `file-history-snapshot` lines.
///
/// Returns None when the file is not a recognizable session file of any
/// known agent — including files whose content is genuinely ambiguous, which
/// must be left unclaimed rather than guessed at (方案 §37.5).
pub fn detect_format(path: &Path) -> Option<Agent> {
    const MAX_PARSE_LINES: usize = 10;
    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);
    let mut parsed = 0usize;
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(t) else {
            continue;
        };
        parsed += 1;
        if let Some(agent) = fingerprint_line(&v) {
            return Some(agent);
        }
        if parsed >= MAX_PARSE_LINES {
            break;
        }
    }
    None
}

fn fingerprint_line(v: &serde_json::Value) -> Option<Agent> {
    if !v.is_object() {
        return None;
    }
    // Codex envelope: {ordinal, payload, type} on every line.
    if v.get("ordinal").is_some()
        && v.get("payload").map(|p| p.is_object()).unwrap_or(false)
        && v.get("type").is_some()
    {
        return Some(Agent::Codex);
    }
    // Qoder BEFORE Claude, and deliberately so (方案 §37.5): its transcript is
    // Claude-shaped, so every Qoder message line would satisfy the Claude
    // branch below. These five line types are Qoder's own bookkeeping and are
    // re-emitted throughout the file (workspace-directories alone repeats
    // dozens of times), so the head always carries one.
    if matches!(
        v.get("type").and_then(|t| t.as_str()),
        Some("workspace-directories")
            | Some("runtime-config")
            | Some("worktree-state")
            | Some("active-leaf")
            | Some("last-prompt")
    ) {
        return Some(Agent::Qoder);
    }
    // Claude event chain: sessionId plus the parentUuid/uuid pair.
    // Housekeeping lines (queue-operation etc.) carry sessionId alone and
    // are deliberately not decisive.
    if v.get("sessionId").is_some() && (v.get("parentUuid").is_some() || v.get("uuid").is_some()) {
        return Some(Agent::ClaudeCode);
    }
    // WorkBuddy: no session header, and unlike pi the message payload is NOT
    // nested — `role` / `content` sit at the top level next to `type`. Its
    // `ai-title` / `file-history-snapshot` bookkeeping lines are unique to it,
    // and they always appear within the first few lines.
    if matches!(
        v.get("type").and_then(|t| t.as_str()),
        Some("ai-title") | Some("file-history-snapshot")
    ) {
        return Some(Agent::WorkBuddy);
    }
    if v.get("type").and_then(|t| t.as_str()) == Some("message")
        && v.get("role").is_some()
        && v.get("content").map(|c| c.is_array()).unwrap_or(false)
    {
        return Some(Agent::WorkBuddy);
    }
    // Pi: self-describing session header, or event lines with a parentId
    // plus one of the pi-specific fields.
    if v.get("type").and_then(|t| t.as_str()) == Some("session") && v.get("id").is_some() {
        return Some(Agent::Pi);
    }
    if v.get("parentId").is_some()
        && ["provider", "modelId", "thinkingLevel"]
            .iter()
            .any(|k| v.get(*k).is_some())
    {
        return Some(Agent::Pi);
    }
    None
}

struct FileObservation {
    identity: String,
    size: u64,
    mtime: Option<f64>,
}

fn observe(path: &Path) -> Result<(FileObservation, String)> {
    let data = std::fs::read(path).map_err(|e| other(format!("读取 session 文件失败: {}", e)))?;
    let meta = std::fs::metadata(path)?;
    Ok((
        FileObservation {
            identity: file_identity(path),
            size: data.len() as u64,
            mtime: mtime_secs(&meta),
        },
        String::from_utf8_lossy(&data).to_string(),
    ))
}

/// Hex SHA-256 of a byte slice — the prefix fingerprint stored on cursors.
pub(crate) fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    use std::fmt::Write;
    let mut h = sha2::Sha256::new();
    h.update(data);
    h.finalize()
        .iter()
        .fold(String::with_capacity(64), |mut out, b| {
            write!(out, "{:02x}", b).ok();
            out
        })
}

/// Shared incremental JSONL reader used by every adapter.
///
/// Classification of the read (compared against the stored cursor):
/// - append:      same file, size grew AND the stored prefix fingerprint
///                still matches the file's prefix → parse only new complete
///                lines. A rewrite that GREW the file diverges here and is
///                correctly treated as a rewrite.
/// - no change:   same file, size identical, mtime unchanged → nothing
/// - truncate:    same file, size shrank        → new generation, full rescan
/// - rewrite:     same file, same size, mtime changed → new generation, full rescan
/// - replacement: different file identity       → new generation, full rescan
///
/// Rescans are safe because storage dedups by event identity: unchanged
/// events are skipped, changed/new ones are added as new events, and
/// previously ingested history is never touched. Legacy cursors without a
/// stored prefix fingerprint take one full rescan to backfill it.
pub fn read_jsonl_delta(
    path: &Path,
    cursor: &SourceCursor,
    parse_line: &dyn Fn(usize, &serde_json::Value) -> Option<ParsedLine>,
) -> Result<ReadDelta> {
    let (obs, text) = observe(path)?;
    // Prefix fingerprint over the (deterministic) lossy-decoded bytes; the
    // reader's offsets live in these coordinates too.
    let prefix_hash_of = |upto: usize| -> String {
        let upto = upto.min(text.len());
        sha256_hex(&text.as_bytes()[..upto])
    };

    let first_ever = cursor.source_file_identity.is_empty() && cursor.last_seen_size == 0;
    let (generation, start_offset): (i64, u64) = if first_ever {
        (0, 0)
    } else if obs.identity != cursor.source_file_identity {
        (cursor.generation + 1, 0) // file replacement
    } else if (obs.size as i64) < (cursor.last_seen_size as i64) {
        (cursor.generation + 1, 0) // truncate / compact
    } else if obs.size == cursor.last_seen_size {
        let mtime_changed = match (cursor.mtime, obs.mtime) {
            (Some(a), Some(b)) => (a - b).abs() > 1e-6,
            (None, Some(_)) | (Some(_), None) => true,
            (None, None) => false,
        };
        if !mtime_changed {
            // nothing new at all
            return Ok(ReadDelta {
                events: vec![],
                source: Some(crate::domain::SourceCursorUpdate {
                    file_identity: obs.identity,
                    generation: cursor.generation,
                    byte_offset: cursor.byte_offset,
                    last_seen_size: obs.size,
                    mtime: obs.mtime,
                    start_byte_offset: cursor.byte_offset,
                    prefix_hash: cursor.prefix_hash.clone(),
                }),
            });
        }
        (cursor.generation + 1, 0) // same-size rewrite / touch
    } else {
        // size grew: accept as append ONLY if the previously consumed prefix
        // is still the file's prefix; otherwise the old head was rewritten.
        let prefix_end = (cursor.byte_offset as usize).min(text.len());
        let prefix_ok = !cursor.prefix_hash.is_empty()
            && (cursor.byte_offset as u64) <= obs.size
            && prefix_hash_of(prefix_end) == cursor.prefix_hash;
        if prefix_ok {
            (cursor.generation, cursor.byte_offset.min(obs.size)) // append
        } else {
            (cursor.generation + 1, 0) // rewrite-grow (or legacy cursor: backfill rescan)
        }
    };

    // Byte offset of the end of the last complete (newline-terminated) line.
    let complete_end: usize = if text.ends_with('\n') {
        text.len()
    } else {
        text.rfind('\n').map(|i| i + 1).unwrap_or(0)
    };

    let mut events: Vec<ParsedEvent> = Vec::new();
    let start_offset = start_offset as usize;
    let mut offset = 0usize;
    for (idx, line) in text.lines().enumerate() {
        let line_start = offset;
        offset += line.len() + 1; // +1 for '\n' (off-by-one at EOF is harmless)
        if line_start < start_offset || line_start >= complete_end {
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        // Timestamps are RFC3339 strings in every format but WorkBuddy's,
        // which writes epoch millis as a number. Both are normalized here so
        // downstream (display, ordering) sees one spelling.
        let ts = match v.get("timestamp") {
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            Some(serde_json::Value::Number(n)) => n.as_i64().and_then(ms_epoch_to_rfc3339),
            _ => None,
        };
        if let Some(p) = parse_line(idx, &v) {
            if p.text
                .as_deref()
                .map(|t| t.trim().is_empty())
                .unwrap_or(true)
            {
                continue;
            }
            events.push(ParsedEvent {
                source_event_id: p.source_event_id,
                source_position: format!("line:{}", idx + 1),
                ts,
                kind: p.kind,
                text: p.text,
                metadata: p.metadata,
            });
        }
    }

    Ok(ReadDelta {
        events,
        source: Some(crate::domain::SourceCursorUpdate {
            file_identity: obs.identity,
            generation,
            byte_offset: complete_end as u64,
            last_seen_size: obs.size,
            mtime: obs.mtime,
            start_byte_offset: start_offset as u64,
            prefix_hash: prefix_hash_of(complete_end),
        }),
    })
}

/// Cursor update for readers that replay the whole source on every read (dsh's
/// zstd frames, Gemini's checkpoint log — 方案 §37.8/§37.9).
///
/// Their offsets must stay in the file's OWN coordinates: that is what the
/// reconcile pre-filter stats, and it is the only thing known without decoding
/// or replaying. A replay therefore always starts at genesis, and event
/// identity — the writer's own ids in both formats — absorbs it: re-reading a
/// source stores nothing. A new generation is a change of shape (the file was
/// replaced, or truncation removed bytes), never a mere append.
pub fn replay_cursor_update(
    path: &Path,
    cursor: &SourceCursor,
    raw: &[u8],
) -> Result<crate::domain::SourceCursorUpdate> {
    let meta = std::fs::metadata(path)?;
    let identity = file_identity(path);
    let len = raw.len() as u64;
    let first_ever = cursor.source_file_identity.is_empty() && cursor.last_seen_size == 0;
    let generation = if first_ever {
        0
    } else if identity != cursor.source_file_identity
        || (len as i64) < (cursor.last_seen_size as i64)
    {
        cursor.generation + 1
    } else {
        cursor.generation
    };
    Ok(crate::domain::SourceCursorUpdate {
        file_identity: identity,
        generation,
        byte_offset: len,
        last_seen_size: len,
        mtime: mtime_secs(&meta),
        start_byte_offset: 0,
        prefix_hash: sha256_hex(raw),
    })
}

/// Extract a string field from an object if present.
pub(crate) fn str_field<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|s| s.as_str())
}

// Source session deletion contract (方案 §13–§18, Hardening §2–§4) --------

/// Current frozen-plan format. A stored job whose version differs is stale.
/// v2 added `source` (verified-present vs confirmed-absent preparation).
pub const SOURCE_DELETION_PLAN_VERSION: u32 = 2;

/// What prepare concluded about the raw source (Hardening §2/§4):
/// - `VerifiedPresent`: the file existed and passed the full §16 proof;
/// - `ConfirmedAbsent`: the file was definitively gone;
/// - `Unverified`: the source could not be safely validated. The caller may
///   still purge NoEnding data, but execute must not touch the source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceDeletionState {
    VerifiedPresent,
    ConfirmedAbsent,
    Unverified,
}

impl SourceDeletionState {
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceDeletionState::VerifiedPresent => "verified_present",
            SourceDeletionState::ConfirmedAbsent => "confirmed_absent",
            SourceDeletionState::Unverified => "unverified",
        }
    }
}

/// One file that a permanent deletion will remove from disk.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SourceDeletionTarget {
    pub path: String,
    /// Adapter-owned descriptor of what this file is (e.g. "codex_rollout").
    pub kind: String,
    /// Identity of the file itself (see [`file_identity`]), frozen at prepare.
    pub file_identity: String,
    pub size: u64,
    /// Full-file SHA-256 frozen at prepare time. Permanent deletion is a
    /// low-frequency operation: completeness beats the saved milliseconds.
    pub sha256: String,
}

/// Frozen deletion plan — "what you confirm is what gets deleted" (方案 §19).
/// Built only by the owning adapter, stored as JSON in `session_deletion_jobs`,
/// and revalidated against the live file before anything is removed. The
/// frontend only ever submits the job id; paths never travel from the UI.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SourceDeletionPlan {
    pub version: u32,
    pub agent: Agent,
    pub agent_session_id: String,
    pub source: SourceDeletionState,
    /// The recorded source path(s); exactly one here. The identity fields are
    /// zeroed when the source is `ConfirmedAbsent` or `Unverified`; the path
    /// is still kept for the preview.
    pub targets: Vec<SourceDeletionTarget>,
}

/// Outcome of the adapter-owned source deletion (方案 §24). The lifecycle
/// layer records other adapter errors while still purging NoEnding data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceDeletionOutcome {
    /// The file existed, validated exactly, and was removed.
    Deleted,
    /// The file was already gone (crash between remove and purge, or a
    /// concurrent deletion). Treated as success — the purge may proceed.
    AlreadyAbsent,
}

pub trait AgentAdapter: Send + Sync {
    fn agent(&self) -> Agent;

    /// CLI detect() via ExecutableResolver + data-dir presence.
    fn detect(&self) -> Option<AgentInstallation>;

    /// Discover external sessions under the given roots (the user-enabled
    /// ingest sources). Each root is scanned recursively with the adapter's
    /// own file-matching rules; missing directories are skipped quietly.
    ///
    /// `unchanged` is the store's "this transcript is already fully ingested
    /// and its cursor still matches the file on disk" verdict: a candidate
    /// that passes it is skipped WITHOUT being read or parsed, so a steady-
    /// state reconcile pass touches only files that actually changed.
    fn discover_sessions_in(
        &self,
        roots: &[PathBuf],
        unchanged: &dyn Fn(&Path) -> bool,
    ) -> Result<Vec<DiscoveredSession>>;

    /// Read only the delta since `cursor`, detecting append / truncate /
    /// rewrite / file-replacement. The returned events carry no sequence —
    /// the storage layer assigns stable identities.
    fn read_delta(&self, session: &Session, cursor: &SourceCursor) -> Result<ReadDelta>;

    /// Build the command line for a New Session. `context_file` is None when
    /// the user chose to start without any Workstream Context — adapters then
    /// launch the plain CLI with no injected prompt.
    ///
    /// `opts` carries NoEnding's override intent only. A `None` field must
    /// produce no CLI argument at all: the Agent's own configuration stays
    /// untouched and unguessed.
    fn build_new_command(
        &self,
        install: &AgentInstallation,
        opts: &ExecOptions,
        context_file: Option<&Path>,
        cwd: Option<&Path>,
    ) -> Result<AgentCommand>;

    fn build_resume_command(
        &self,
        install: &AgentInstallation,
        opts: &ExecOptions,
        agent_session_id: &str,
        context_file: Option<&Path>,
        cwd: Option<&Path>,
    ) -> Result<AgentCommand>;

    /// Non-interactive one-shot run (codex exec / claude -p / pi -p).
    /// The returned command is executed by platform::exec_runner.
    fn build_exec_command(
        &self,
        install: &AgentInstallation,
        opts: &ExecOptions,
        prompt: &str,
    ) -> Result<AgentCommand>;

    /// Freeze a verifiable plan for permanently deleting this session's raw
    /// Agent source (方案 §13). The adapter must fully prove the source before
    /// returning a plan (§16): regular file, not a symlink, content
    /// fingerprint belongs to this agent, parsed session id equals
    /// `session.agent_session_id`, and the file is exactly what discovery
    /// would recognize. Any doubt is an error, never a best guess.
    fn prepare_source_session_deletion(&self, session: &Session) -> Result<SourceDeletionPlan> {
        let _ = session;
        Err(other(format!(
            "{} 暂不支持安全的源会话删除",
            self.agent().display_name()
        )))
    }

    /// Execute a previously frozen plan. Must revalidate identity, size,
    /// sha256 and the parsed session id against the live file first (§21):
    /// exact match → delete; already absent → [`SourceDeletionOutcome::
    /// AlreadyAbsent`]; anything else → stale error WITHOUT deleting (a same
    /// path holding different content must never be removed). Only adapter
    /// code may remove raw Agent files — Core never calls `remove_file` on a
    /// session's `raw_path`.
    fn execute_source_session_deletion(
        &self,
        plan: &SourceDeletionPlan,
    ) -> Result<SourceDeletionOutcome> {
        let _ = plan;
        Err(other(format!(
            "{} 暂不支持安全的源会话删除",
            self.agent().display_name()
        )))
    }
}

pub fn all_adapters() -> Vec<Box<dyn AgentAdapter>> {
    vec![
        Box::new(codex::CodexAdapter),
        Box::new(claude::ClaudeAdapter),
        Box::new(pi::PiAdapter),
        Box::new(qoder::QoderAdapter),
        Box::new(autoclaw::AutoClawAdapter),
        Box::new(workbuddy::WorkBuddyAdapter),
        Box::new(dsh::DshAdapter),
        Box::new(gemini::GeminiAdapter),
        Box::new(zcode::ZCodeAdapter),
    ]
}

pub fn adapter_for(agent: Agent) -> &'static dyn AgentAdapter {
    use std::sync::OnceLock;
    static REGISTRY: OnceLock<Vec<Box<dyn AgentAdapter>>> = OnceLock::new();
    let reg = REGISTRY.get_or_init(all_adapters);
    for a in reg {
        if a.agent() == agent {
            // Safe: registry outlives 'static and elements are never removed.
            return unsafe { &*(a.as_ref() as *const dyn AgentAdapter) };
        }
    }
    unreachable!("adapter for {:?} missing", agent)
}

// Shared JSONL helpers -------------------------------------------------

pub fn read_jsonl_lines(path: &Path) -> Result<Vec<(usize, String)>> {
    let data = std::fs::read(path)?;
    let text = String::from_utf8_lossy(&data);
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if !line.trim().is_empty() {
            out.push((i, line.to_string()));
        }
    }
    Ok(out)
}

pub fn truncate_text(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{}…", t)
    }
}

/// Session display title from the first meaningful user text.
pub fn title_from_text(text: &str) -> Option<String> {
    let t = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if t.is_empty() {
        None
    } else {
        Some(truncate_text(&t, 36))
    }
}

// Shared single-file source-deletion machinery (方案 §15) ----------------
//
// All three current adapters model a session as ONE JSONL transcript, so
// v0.1 shares the freeze / validate / delete logic below. It is still
// INVOKED BY each adapter with its own parser — Core never assumes every
// Agent is single-file JSONL, and Core never removes a file itself. The
// safety boundary is "the adapter can strictly prove this file IS that
// Agent session", not "the file happens to live under ~/.codex" (§17), so
// custom ingest sources are deletable too.

/// Freeze a one-file deletion plan after full §16 validation.
///
/// `parse_session_id` must be the adapter's own discovery parser (by
/// construction: a file whose parsed id matches is a file discovery would
/// identify — §16.5).
///
/// A bare `io::ErrorKind::NotFound` produces a `ConfirmedAbsent` plan. Other
/// lookup or read failures are handled as an unverified best-effort deletion.
pub(crate) fn prepare_single_file_source_deletion(
    session: &Session,
    expected_agent: Agent,
    kind: &str,
    parse_session_id: &dyn Fn(&Path) -> Result<Option<String>>,
) -> Result<SourceDeletionPlan> {
    if session.agent != expected_agent {
        return Err(other("源会话文件所属 Agent 与该会话不一致"));
    }
    let path = PathBuf::from(&session.raw_path);

    // Definitively gone → confirmed-absent plan; other errors are handled as
    // an unverified best-effort deletion by the lifecycle layer.
    let link_meta = match std::fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SourceDeletionPlan {
                version: SOURCE_DELETION_PLAN_VERSION,
                agent: expected_agent,
                agent_session_id: session.agent_session_id.clone(),
                source: SourceDeletionState::ConfirmedAbsent,
                targets: vec![SourceDeletionTarget {
                    path: path.to_string_lossy().to_string(),
                    kind: kind.to_string(),
                    // Nothing provable about bytes that do not exist.
                    file_identity: String::new(),
                    size: 0,
                    sha256: String::new(),
                }],
            });
        }
        Err(e) => {
            return Err(other(format!(
                "无法确认源会话文件状态 {}: {}（不将其视为已删除）",
                path.display(),
                e
            )))
        }
    };

    // §16.1/§16.2 + §18: regular file only, never a symlink. symlink_metadata
    // does not follow the link, so a link pointing at a regular file is
    // still refused — v0.1 does not guess "link or target?".
    if link_meta.file_type().is_symlink() {
        return Err(other("源会话文件是符号链接，永久删除暂不支持"));
    }
    if !link_meta.is_file() {
        return Err(other("源会话路径不是常规文件，拒绝永久删除"));
    }

    // §16.3: the content fingerprint must belong to the session's agent.
    let detected = detect_format(&path).ok_or_else(|| other("无法识别源会话文件格式"))?;
    if detected != expected_agent {
        return Err(other("源会话文件内容不属于该 Agent，拒绝永久删除"));
    }

    // §16.4: the file must parse to the very session being deleted.
    let parsed_id =
        parse_session_id(&path)?.ok_or_else(|| other("无法从源会话文件解析出会话 id"))?;
    if parsed_id != session.agent_session_id {
        return Err(other("源会话文件解析出的会话 id 不一致，拒绝永久删除"));
    }

    let data = std::fs::read(&path)?;
    let meta = std::fs::metadata(&path)?;
    let size = meta.len();
    if size != data.len() as u64 {
        return Err(other("源会话文件在校验期间发生变化，请重试"));
    }

    Ok(SourceDeletionPlan {
        version: SOURCE_DELETION_PLAN_VERSION,
        agent: expected_agent,
        agent_session_id: session.agent_session_id.clone(),
        source: SourceDeletionState::VerifiedPresent,
        targets: vec![SourceDeletionTarget {
            path: path.to_string_lossy().to_string(),
            kind: kind.to_string(),
            file_identity: file_identity(&path),
            size,
            sha256: sha256_hex(&data),
        }],
    })
}

/// Revalidate a frozen one-file plan against the live file (§21) and remove
/// it. Classification:
/// - absent at any check point → [`SourceDeletionOutcome::AlreadyAbsent`];
/// - present but identity / size / sha256 / session id diverge → stale error
///   (file type changed to something else counts as divergence — never
///   delete a path that no longer holds the confirmed bytes);
/// - exact match → remove and report [`SourceDeletionOutcome::Deleted`].
///
/// Hardening §2: a `ConfirmedAbsent` plan has nothing to delete. Execute
/// only re-confirms the absence — still absent → AlreadyAbsent; an
/// indeterminable or reappeared file returns an error and is never removed.
/// The lifecycle layer may still purge NoEnding data after that error.
pub(crate) fn execute_single_file_source_deletion(
    plan: &SourceDeletionPlan,
    expected_agent: Agent,
    parse_session_id: &dyn Fn(&Path) -> Result<Option<String>>,
) -> Result<SourceDeletionOutcome> {
    if plan.version != SOURCE_DELETION_PLAN_VERSION {
        return Err(other("源删除计划版本不受支持，请重新准备"));
    }
    if plan.agent != expected_agent {
        return Err(other("源删除计划与适配器能力不一致，请重新准备"));
    }
    if plan.targets.len() != 1 {
        return Err(other("源删除计划与适配器能力不一致，请重新准备"));
    }
    if plan.source == SourceDeletionState::Unverified {
        return Err(other("源会话文件未能安全验证，跳过源文件删除"));
    }
    if plan.source == SourceDeletionState::ConfirmedAbsent {
        // Re-confirm the absence at the SAME path (Hardening §2): still gone
        // → AlreadyAbsent; indeterminable → failure; reappeared → stale,
        // because bytes that were never confirmed are never removed.
        let path = PathBuf::from(&plan.targets[0].path);
        return match std::fs::symlink_metadata(&path) {
            Ok(_) => Err(AppError::SourceDeletionStale(
                "源会话文件在准备删除后重新出现，请重新准备".to_string(),
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(SourceDeletionOutcome::AlreadyAbsent)
            }
            Err(e) => Err(other(format!(
                "无法确认源会话文件状态: {}（不视为已删除）",
                e
            ))),
        };
    }
    let target = &plan.targets[0];
    let path = PathBuf::from(&target.path);

    let data = match std::fs::read(&path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SourceDeletionOutcome::AlreadyAbsent)
        }
        Err(e) => return Err(other(format!("源会话文件不可访问: {}", e))),
    };
    let link_meta = std::fs::symlink_metadata(&path)
        .map_err(|e| other(format!("源会话文件不可访问: {}", e)))?;
    if link_meta.file_type().is_symlink() || !link_meta.is_file() {
        return Err(AppError::SourceDeletionStale(
            "源会话路径的文件类型已改变".to_string(),
        ));
    }

    let stale = |why: String| AppError::SourceDeletionStale(why);
    if file_identity(&path) != target.file_identity {
        return Err(stale("源会话文件已被替换（文件身份不一致）".to_string()));
    }
    if data.len() as u64 != target.size {
        return Err(stale("源会话文件大小已改变".to_string()));
    }
    if sha256_hex(&data) != target.sha256 {
        return Err(stale("源会话文件内容已改变".to_string()));
    }
    let parsed_id = parse_session_id(&path)?
        .ok_or_else(|| stale("源会话文件已无法解析出会话 id".to_string()))?;
    if parsed_id != plan.agent_session_id {
        return Err(stale("源会话文件内容已是另一个会话".to_string()));
    }

    match std::fs::remove_file(&path) {
        Ok(()) => Ok(SourceDeletionOutcome::Deleted),
        // Raced with an external removal between validation and unlink —
        // the confirmed bytes are gone, which is the goal.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(SourceDeletionOutcome::AlreadyAbsent)
        }
        // Windows sharing violation, read-only/permission failure, …: the
        // caller keeps the Session in Trash with NoEnding data intact.
        Err(e) => Err(other(format!("删除源会话文件失败: {}", e))),
    }
}

pub(crate) use str_field as json_str_field;

#[cfg(test)]
mod fingerprint_tests {
    use super::*;

    #[test]
    fn fingerprints_match_real_line_shapes() {
        // Codex: the {ordinal, payload, type} envelope
        let codex = serde_json::json!({
            "ordinal": 0, "type": "session_meta", "timestamp": "t",
            "payload": {"session_id": "s", "cwd": "/x"}
        });
        assert_eq!(fingerprint_line(&codex), Some(Agent::Codex));

        // Claude: event chain line (parentUuid may be null on the first one)
        let claude = serde_json::json!({
            "type": "user", "sessionId": "s", "uuid": "u", "parentUuid": null,
            "message": {"role": "user", "content": []}
        });
        assert_eq!(fingerprint_line(&claude), Some(Agent::ClaudeCode));

        // Claude housekeeping lines are NOT decisive on their own
        let queue_op = serde_json::json!({
            "type": "queue-operation", "operation": "enqueue", "sessionId": "s", "timestamp": "t"
        });
        assert_eq!(fingerprint_line(&queue_op), None);

        // Pi: self-describing session header, or provider/modelId/thinkingLevel lines
        let pi_header =
            serde_json::json!({"type": "session", "id": "p1", "cwd": "/x", "version": 3});
        assert_eq!(fingerprint_line(&pi_header), Some(Agent::Pi));
        let pi_event = serde_json::json!({
            "type": "message", "id": "m1", "parentId": "p1",
            "provider": "openai", "modelId": "m"
        });
        assert_eq!(fingerprint_line(&pi_event), Some(Agent::Pi));

        // Anything else (including foreign tools' exports) is not a session
        let other = serde_json::json!({"hello": "world", "data": [1, 2, 3]});
        assert_eq!(fingerprint_line(&other), None);
    }

    /// Qoder's transcript IS Claude's format plus bookkeeping lines, so the
    /// only thing standing between a Qoder file and being read as Claude is
    /// branch order in `fingerprint_line` (方案 §37.5).
    #[test]
    fn qoder_is_claimed_before_claude() {
        // A Qoder bookkeeping line: no uuid/sessionId pair, but decisive.
        let qoder = serde_json::json!({
            "type": "workspace-directories", "sessionId": "s", "directories": ["/repo"]
        });
        assert_eq!(fingerprint_line(&qoder), Some(Agent::Qoder));
        let last_prompt = serde_json::json!({"type": "last-prompt", "sessionId": "s"});
        assert_eq!(fingerprint_line(&last_prompt), Some(Agent::Qoder));

        // A Qoder message line is indistinguishable from Claude's on its own —
        // which is exactly why the bookkeeping line has to be in the head.
        let claude_shaped = serde_json::json!({
            "type": "user", "sessionId": "s", "uuid": "u", "parentUuid": null,
            "message": {"role": "user", "content": []}
        });
        assert_eq!(fingerprint_line(&claude_shaped), Some(Agent::ClaudeCode));

        // A whole Qoder file (head included) resolves to Qoder.
        let dir = std::env::temp_dir().join(format!("noending-fp-qoder-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.jsonl");
        std::fs::write(
            &path,
            format!(
                "{}\n{}\n",
                serde_json::to_string(&qoder).unwrap(),
                serde_json::to_string(&claude_shaped).unwrap()
            ),
        )
        .unwrap();
        assert_eq!(detect_format(&path), Some(Agent::Qoder));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// WorkBuddy keeps its message payload at the top level (`role` /
    /// `content` next to `type`), unlike pi, whose `type:"message"` line nests
    /// the same fields under `message` (方案 §37.7). Its bookkeeping line types
    /// are decisive on their own.
    #[test]
    fn workbuddy_is_claimed_by_its_top_level_shape() {
        let msg = serde_json::json!({
            "id": "m1", "timestamp": 1783137449113i64, "type": "message",
            "role": "user", "content": [{"type": "input_text", "text": "hi"}],
            "sessionId": "s", "cwd": "/repo"
        });
        assert_eq!(fingerprint_line(&msg), Some(Agent::WorkBuddy));

        let title = serde_json::json!({
            "timestamp": 1783137451207i64, "type": "ai-title", "aiTitle": "t", "sessionId": "s"
        });
        assert_eq!(fingerprint_line(&title), Some(Agent::WorkBuddy));
        let snapshot = serde_json::json!({
            "timestamp": 1783137449152i64, "type": "file-history-snapshot", "sessionId": "s"
        });
        assert_eq!(fingerprint_line(&snapshot), Some(Agent::WorkBuddy));

        // pi's nested message line must stay pi — the top-level `role` is the
        // only thing separating the two shapes.
        let pi_event = serde_json::json!({
            "type": "message", "id": "m1", "parentId": "p1", "provider": "openai",
            "message": {"role": "user", "content": [{"type": "text", "text": "hi"}]}
        });
        assert_eq!(fingerprint_line(&pi_event), Some(Agent::Pi));
    }

    #[test]
    fn detect_format_reads_only_the_head_of_a_file() {
        let dir = std::env::temp_dir().join(format!("noending-fp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mixed.jsonl");
        // junk lines first, then a decisive claude event line
        std::fs::write(
            &path,
            "not json at all\n{\"a\":1}\n{\"sessionId\":\"s\",\"uuid\":\"u\",\"message\":{\"role\":\"user\",\"content\":[]}}\n",
        )
        .unwrap();
        assert_eq!(detect_format(&path), Some(Agent::ClaudeCode));

        // a file with no decisive lines is not a session file
        let path2 = dir.join("foreign.jsonl");
        std::fs::write(&path2, "{\"hello\":\"world\"}\n{\"more\":\"data\"}\n").unwrap();
        assert_eq!(detect_format(&path2), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Real-data check against the local agent roots:
    /// `cargo test real_agent_files_match_fingerprints -- --ignored --nocapture`
    ///
    /// The guarantee asserted here is the one that matters: **discovery never
    /// returns a session belonging to another agent**. It is stated at the
    /// discovery level rather than per file, because a file's content can be
    /// genuinely ambiguous — Qoder's sub-agent transcripts repeat Claude
    /// Code's line shapes and are therefore claimed by nobody (方案 §37.5) —
    /// and a file nobody claims is not a misattribution.
    #[test]
    #[ignore]
    fn real_agent_files_match_fingerprints() {
        use crate::platform::paths::resolve_agent_data_dir;
        for agent in Agent::all() {
            let Some(root) = resolve_agent_data_dir(*agent) else {
                continue;
            };
            if !root.is_dir() {
                continue;
            }
            let discovered = adapter_for(*agent)
                .discover_sessions_in(&[root.clone()], &|_| false)
                .unwrap_or_else(|e| panic!("{} discovery failed: {e}", agent.display_name()));
            for s in &discovered {
                assert_eq!(
                    s.agent,
                    *agent,
                    "cross-agent misattribution: {}",
                    s.path.display()
                );
                assert!(
                    !s.agent_session_id.is_empty() && s.path.starts_with(&root),
                    "{}: bad discovery result {:?}",
                    agent.display_name(),
                    s.path
                );
            }
            eprintln!(
                "[fingerprint] {}: {} sessions discovered under {}",
                agent.display_name(),
                discovered.len(),
                root.display()
            );
        }
    }
}
