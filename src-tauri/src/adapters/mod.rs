//! Agent Adapter layer.
//!
//! Each adapter owns: the member layout of its Agent's data directory, the
//! file/record formats, member identity, optional environment info, resume
//! parameters, new-session invocation. Upper layers must never reference
//! `~/.codex` / `~/.claude` / `~/.pi` directly — that is PlatformPaths' job.
//!
//! The source unit is the **member** (`DiscoveredMember` / `SessionMember`),
//! not the session: one Logical Session is the root member plus every child /
//! side member that resolves to it (重构方案 §9).
//!
//! Context Integrity rules enforced here:
//! - adapters never generate shell fragments (no `$(cat …)`): context is
//!   passed as a real argv string produced by [`context_prompt`];
//! - [`read_jsonl_delta`] classifies every read as append / truncate /
//!   rewrite / file-replacement and only ever returns the *new* portion;
//! - adapters emit CONVERSATION (`role` user/assistant, user-visible prose
//!   only) plus execution OBSERVATIONS (tool/compaction/side counts). Child
//!   and side members never emit messages at all — the core's commit would
//!   reject them, and adapter tests keep that from ever being relied on;
//! - re-ingesting already-seen content is prevented by the storage layer's
//!   content-identity dedup, so compaction can never overwrite history.

pub mod claude;
pub mod codex;
pub mod dsh;
pub mod pi;
pub mod qoder;
pub mod workbuddy;
pub mod zcode;

use std::io::BufRead;
use std::path::{Path, PathBuf};

use crate::domain::{
    Agent, MemberObservation, MemberStatsDelta, ParsedSessionMessage, SessionMember,
    SessionMemberCursor, SessionMemberStatsSnapshot, SessionMessageRole, SourceAvailability,
    StatsUpdate,
};
use crate::error::{other, Result};
use crate::platform::exec_resolver::AgentInstallation;

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

/// What a discovered member IS in the execution graph (重构方案 §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveredMemberKind {
    /// Establishes / joins the Logical Session as its root conversation.
    Root,
    /// Internal execution (subagent thread); never conversation.
    Child,
    /// Side execution (sidechain / review thread); never conversation.
    Side,
    /// Itself a new, independently continuable Logical Session; the parent is
    /// only fork provenance (`sessions.forked_from_session_id`).
    ForkRoot,
}

impl DiscoveredMemberKind {
    /// The member relation this discovery kind lands as. A ForkRoot becomes
    /// the `root` member of its own new Logical Session.
    pub fn relation(self) -> crate::domain::SessionMemberRelation {
        use crate::domain::SessionMemberRelation as R;
        match self {
            DiscoveredMemberKind::Root | DiscoveredMemberKind::ForkRoot => R::Root,
            DiscoveredMemberKind::Child => R::Child,
            DiscoveredMemberKind::Side => R::Side,
        }
    }

    pub fn is_logical_root(self) -> bool {
        matches!(
            self,
            DiscoveredMemberKind::Root | DiscoveredMemberKind::ForkRoot
        )
    }
}

/// One discovered execution member before ingestion (重构方案 §9).
#[derive(Debug, Clone)]
pub struct DiscoveredMember {
    pub agent: Agent,
    /// Adapter-stable execution identity. Not required to equal the Agent's
    /// native session id (§5.1) — e.g. Qoder subagent transcripts repeat the
    /// parent id, so the adapter derives `root-id:subagent:<file-stem>`.
    pub source_member_id: String,
    pub kind: DiscoveredMemberKind,
    /// Source-side parent identity; may dangle until the parent's own batch
    /// or a later reconcile resolves it.
    pub parent_source_member_id: Option<String>,
    /// When the adapter knows it, the root identity this member belongs to —
    /// a cross-file shortcut for attribution, never an authority by itself.
    pub root_hint: Option<String>,
    /// Adapter-owned descriptor (`codex_rollout`, `transcript_file`,
    /// `sqlite_record`, `subagent_file`, `zstd_rollup`, …).
    pub source_kind: String,
    pub source_path: PathBuf,
    pub cwd: Option<String>,
    pub started_at: Option<String>,
    pub last_activity_at: Option<String>,
    /// Root only; child/side must leave it `None`.
    pub native_title: Option<String>,
    /// Root only: first real user prose, for the title chain (§4.2).
    pub first_user_text: Option<String>,
    /// Root only: first visible assistant prose — the last title resort.
    pub first_agent_text: Option<String>,
    pub metadata: serde_json::Value,
}

impl DiscoveredMember {
    /// Root identity for Logical-Session creation. Only Root/ForkRoot carry
    /// one; calling this on a child/side is a caller bug.
    pub fn root_agent_session_id(&self) -> &str {
        debug_assert!(
            self.kind.is_logical_root(),
            "root_agent_session_id requested for a non-root member"
        );
        &self.source_member_id
    }
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

/// One parsed source line's contribution: at most one conversation message
/// plus the execution observations the line carries. Lines that are neither
/// (reasoning bodies, bookkeeping, session meta) return `None` from the
/// adapter closure and never reach storage.
pub struct ParsedLine {
    /// The conversation message this line contributes, if any.
    pub message: Option<ParsedSessionMessage>,
    /// Execution observations (tool calls, compactions, side activity) this
    /// line contributes — counted even on lines that carry no message.
    pub observation: MemberObservation,
}

impl ParsedLine {
    /// A line that carries only observations (tool call, compaction marker,
    /// side chatter).
    pub fn observation_only(observation: MemberObservation) -> Self {
        Self {
            message: None,
            observation,
        }
    }

    /// A line that carries one conversation message and no counters.
    pub fn message_only(message: ParsedSessionMessage) -> Self {
        Self {
            message: Some(message),
            observation: MemberObservation::default(),
        }
    }
}

/// Result of one incremental member read: conversation messages (root members
/// only), how the read updates stats, and the source state AFTER reading.
/// Messages carry no sequence — the storage layer assigns stable identities.
///
/// `next_active_provider` / `next_active_model` are the stateful provenance
/// frontier after this read (Provenance 方案 §15) — `None`/`None` for every
/// adapter whose evidence is direct per-message or absent.
#[derive(Debug, Clone, Default)]
pub struct MemberReadDelta {
    pub messages: Vec<ParsedSessionMessage>,
    pub stats: Option<StatsUpdate>,
    pub source: Option<crate::domain::SourceCursorUpdate>,
    pub next_active_provider: Option<String>,
    pub next_active_model: Option<String>,
}

/// The stateful provenance frontier a stateful-evidence adapter threads
/// through one read (Provenance 方案 §13B/§14): the generation provenance the
/// source has explicitly confirmed and that still governs messages to come.
/// Seeded from the member cursor on appends, reset on any full re-scan.
#[derive(Debug, Clone, Default)]
pub struct ProvenanceState {
    pub provider: Option<String>,
    pub model: Option<String>,
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

/// Strict source availability for a FILE-backed member (§9.1). `NotFound` is
/// the only missing; every other failure is `Unavailable` — an unreadable
/// path must never read as an absent source.
pub fn inspect_file_source(path: &Path) -> SourceAvailability {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() => SourceAvailability::Present,
        Ok(_) => SourceAvailability::Unavailable,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => SourceAvailability::Missing,
        Err(_) => SourceAvailability::Unavailable,
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

/// Counters an adapter can reliably observe in its source format.
#[derive(Debug, Clone, Copy)]
pub struct StatsCapabilities {
    tool_calls: bool,
    tool_errors: bool,
    compactions: bool,
    side_activity: bool,
}

impl StatsCapabilities {
    pub const TOOL_AND_COMPACTION: Self = Self::new(true, false, true, false);
    pub const COMPACTION: Self = Self::new(false, false, true, false);
    pub const TOOL_AND_SIDE_ACTIVITY: Self = Self::new(true, false, false, true);
    pub const TOOL_COMPACTION_AND_SIDE_ACTIVITY: Self = Self::new(true, false, true, true);

    const fn new(
        tool_calls: bool,
        tool_errors: bool,
        compactions: bool,
        side_activity: bool,
    ) -> Self {
        Self {
            tool_calls,
            tool_errors,
            compactions,
            side_activity,
        }
    }
}

/// A genesis read is a full scan, so supported counters replace the snapshot;
/// an append only adds supported observations. `None` means unsupported.
pub fn stats_update_from(
    observation: &MemberObservation,
    source: &crate::domain::SourceCursorUpdate,
    capabilities: StatsCapabilities,
) -> Option<StatsUpdate> {
    if source.start_byte_offset == 0 {
        return Some(StatsUpdate::Snapshot(SessionMemberStatsSnapshot {
            tool_call_count: capabilities
                .tool_calls
                .then_some(observation.tool_calls as i64),
            tool_error_count: capabilities
                .tool_errors
                .then_some(observation.tool_errors as i64),
            compaction_count: capabilities
                .compactions
                .then_some(observation.compactions as i64),
            side_activity_count: capabilities
                .side_activity
                .then_some(observation.side_activity as i64),
        }));
    }
    let empty = (!capabilities.tool_calls || observation.tool_calls == 0)
        && (!capabilities.tool_errors || observation.tool_errors == 0)
        && (!capabilities.compactions || observation.compactions == 0)
        && (!capabilities.side_activity || observation.side_activity == 0);
    if empty {
        None
    } else {
        Some(StatsUpdate::Delta(MemberStatsDelta {
            tool_call_count: (capabilities.tool_calls && observation.tool_calls > 0)
                .then_some(observation.tool_calls as i64),
            tool_error_count: (capabilities.tool_errors && observation.tool_errors > 0)
                .then_some(observation.tool_errors as i64),
            compaction_count: (capabilities.compactions && observation.compactions > 0)
                .then_some(observation.compactions as i64),
            side_activity_count: (capabilities.side_activity && observation.side_activity > 0)
                .then_some(observation.side_activity as i64),
        }))
    }
}

/// Shared incremental JSONL reader used by every file-backed adapter.
///
/// Classification of the read (compared against the stored member cursor):
/// - append:      same file, size grew AND the stored prefix fingerprint
///                still matches the file's prefix → parse only new complete
///                lines. A rewrite that GREW the file diverges here and is
///                correctly treated as a rewrite.
/// - no change:   same file, size identical, mtime unchanged → nothing
/// - truncate:    same file, size shrank        → new generation, full rescan
/// - rewrite:     same file, same size, mtime changed → new generation, full rescan
/// - replacement: different file identity       → new generation, full rescan
///
/// Rescans are safe because storage dedups messages by identity and stats
/// snapshots replace: unchanged messages are skipped, changed/new ones are
/// added, and previously ingested history is never touched. A cursor without
/// a usable prefix fingerprint cannot prove append-only continuity, so the
/// source is conservatively treated as a rewrite and fully re-scanned.
pub fn read_jsonl_delta(
    path: &Path,
    cursor: &SessionMemberCursor,
    capabilities: StatsCapabilities,
    parse_line: &dyn Fn(usize, &serde_json::Value) -> Option<ParsedLine>,
) -> Result<MemberReadDelta> {
    read_jsonl_delta_stateful(
        path,
        cursor,
        capabilities,
        &mut ProvenanceState::default(),
        &mut |idx, v, _state| parse_line(idx, v),
    )
}

/// The stateful variant (Provenance 方案 §14/§15). `state` is the provenance
/// frontier: seeded by the caller from the member cursor, and RESET by this
/// function whenever the read starts at byte 0 (first read or any full
/// re-scan) — a fresh scan re-derives the state from the source itself
/// instead of trusting a frontier earned under a different file layout.
/// The closure may mutate the state as lines are parsed and attaches the
/// current state to the messages it emits; the final state lands on the
/// returned delta's `next_active_*` fields and is committed with the same
/// transaction as the messages.
pub fn read_jsonl_delta_stateful(
    path: &Path,
    cursor: &SessionMemberCursor,
    capabilities: StatsCapabilities,
    state: &mut ProvenanceState,
    parse_line: &mut dyn FnMut(
        usize,
        &serde_json::Value,
        &mut ProvenanceState,
    ) -> Option<ParsedLine>,
) -> Result<MemberReadDelta> {
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
            // nothing new at all — the frontier stays exactly as seeded
            return Ok(MemberReadDelta {
                messages: vec![],
                stats: None,
                source: Some(crate::domain::SourceCursorUpdate {
                    file_identity: obs.identity,
                    generation: cursor.generation,
                    byte_offset: cursor.byte_offset,
                    last_seen_size: obs.size,
                    mtime: obs.mtime,
                    start_byte_offset: cursor.byte_offset,
                    prefix_hash: cursor.prefix_hash.clone(),
                }),
                next_active_provider: state.provider.clone(),
                next_active_model: state.model.clone(),
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
            (cursor.generation + 1, 0) // rewrite-grow: continuity unprovable
        }
    };

    // A full re-scan re-derives provenance from the source itself: the seed
    // was earned under a file layout that no longer governs this read.
    if start_offset == 0 {
        *state = ProvenanceState::default();
    }

    // Byte offset of the end of the last complete (newline-terminated) line.
    let complete_end: usize = if text.ends_with('\n') {
        text.len()
    } else {
        text.rfind('\n').map(|i| i + 1).unwrap_or(0)
    };

    let mut messages: Vec<ParsedSessionMessage> = Vec::new();
    let mut observation = MemberObservation::default();
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
        if let Some(p) = parse_line(idx, &v, state) {
            observation.add(&p.observation);
            if let Some(mut m) = p.message {
                if m.content.trim().is_empty() {
                    continue;
                }
                if m.source_position.is_empty() {
                    m.source_position = format!("line:{}", idx + 1);
                }
                if m.ts.is_none() {
                    m.ts = ts;
                }
                messages.push(m);
            }
        }
    }

    let source = crate::domain::SourceCursorUpdate {
        file_identity: obs.identity,
        generation,
        byte_offset: complete_end as u64,
        last_seen_size: obs.size,
        mtime: obs.mtime,
        start_byte_offset: start_offset as u64,
        prefix_hash: prefix_hash_of(complete_end),
    };
    Ok(MemberReadDelta {
        stats: stats_update_from(&observation, &source, capabilities),
        messages,
        source: Some(source),
        next_active_provider: state.provider.clone(),
        next_active_model: state.model.clone(),
    })
}

/// Cursor update for readers that replay the whole source on every read (dsh's
/// zstd frames — 方案 §37.8; ZCode's live store).
///
/// Their offsets must stay in the source's OWN coordinates: that is what the
/// reconcile pre-filter stats, and it is the only thing known without decoding
/// or replaying. A replay therefore always starts at genesis, and message
/// identity — the writer's own ids — absorbs it: re-reading a source stores
/// nothing. A new generation is a change of shape (the file was replaced, or
/// truncation removed bytes), never a mere append. Because every replay is a
/// full scan, its observations become a stats SNAPSHOT (§7.3).
pub fn replay_cursor_update(
    path: &Path,
    cursor: &SessionMemberCursor,
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

pub trait AgentAdapter: Send + Sync {
    fn agent(&self) -> Agent;

    /// CLI detect() via ExecutableResolver + data-dir presence.
    fn detect(&self) -> Option<AgentInstallation>;

    /// Discover execution members under the given roots (the user-enabled
    /// ingest sources). Each root is scanned recursively with the adapter's
    /// own matching rules; missing directories are skipped quietly.
    ///
    /// `unchanged` is the store's "this member's source is already fully
    /// ingested and its cursor still matches the source on disk" verdict: a
    /// candidate that passes it is skipped WITHOUT being read or parsed, so a
    /// steady-state reconcile pass touches only sources that actually changed.
    fn discover_members_in(
        &self,
        roots: &[PathBuf],
        unchanged: &dyn Fn(&Path) -> bool,
    ) -> Result<Vec<DiscoveredMember>>;

    /// Read only the delta since `cursor` for THIS member, detecting append /
    /// truncate / rewrite / replacement. Only Root members may return
    /// messages; child/side members contribute observations only — the core
    /// commit rejects any message whose member is not the root (§6).
    fn read_member_delta(
        &self,
        member: &SessionMember,
        cursor: &SessionMemberCursor,
    ) -> Result<MemberReadDelta>;

    /// Strict availability verdict for the member's source (§9.1). For a
    /// ROOT member this is the permanent-delete and Resume authority: only a
    /// fresh `Missing` may ever enable a local purge (§20).
    fn inspect_member_source(&self, member: &SessionMember) -> Result<SourceAvailability>;

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
}

pub fn all_adapters() -> Vec<Box<dyn AgentAdapter>> {
    vec![
        Box::new(codex::CodexAdapter),
        Box::new(claude::ClaudeAdapter),
        Box::new(pi::PiAdapter),
        Box::new(qoder::QoderAdapter),
        Box::new(workbuddy::WorkBuddyAdapter),
        Box::new(dsh::DshAdapter),
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

/// First non-empty line of a JSONL file, parsed. Session headers open these
/// files in every format NoEnding reads, so one line is enough to learn how the
/// writer labelled the record — without paying for the whole file.
pub fn read_first_json_line(path: &Path) -> Option<serde_json::Value> {
    let file = std::fs::File::open(path).ok()?;
    for line in std::io::BufReader::new(file).lines() {
        let Ok(line) = line else { return None };
        if line.trim().is_empty() {
            continue;
        }
        return serde_json::from_str(&line).ok();
    }
    None
}

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

/// An injected preamble rather than the user's own words: `<…>` environment
/// blocks and `#`-prefixed injections (AGENTS.md, attached-file headers).
///
/// The test is on the trimmed text because the runtime does not always put the
/// marker first: Codex writes the pasted-file block as `"\n# Files pasted by
/// the user: …"`, and testing the raw text let exactly that become a title
/// (§37.15). Adapters call this when picking their first human turn AND when
/// deciding what is Conversation (§2.3: injected context never becomes a
/// SessionMessage).
pub fn is_injected_preamble(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with('<') || t.starts_with('#')
}

/// Session display title from one text candidate. `None` when there is
/// nothing title-worthy in it.
///
/// A machine blob is not a title (§37.15): Codex's review threads open with
/// `{"risk_level":"medium","user_authorization":"high","outcome":"allow"}` (27
/// of the 50 internal threads on this machine do), and putting that in the
/// session list is noise, not a title. Same call the user-text tier already
/// makes for `<…>` / `#` injections — this is simply the agent-role spelling
/// of it.
pub fn title_from_text(text: &str) -> Option<String> {
    let t = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if t.is_empty() || t.starts_with('{') || t.starts_with('[') {
        None
    } else {
        Some(truncate_text(&t, 36))
    }
}

/// A conversation message out of one parsed line — the spelling every
/// adapter's closure constructs. Provenance defaults to `None`/`None`:
/// most messages (every user message, unknown-provenance assistant turns)
/// need nothing more (Provenance 方案 §26).
pub fn parsed_message(
    source_message_id: Option<String>,
    role: SessionMessageRole,
    content: String,
) -> ParsedSessionMessage {
    ParsedSessionMessage {
        source_message_id,
        source_position: String::new(),
        ts: None,
        role,
        content,
        provider: None,
        model: None,
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
    /// returns a member belonging to another agent**. It is stated at the
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
                .discover_members_in(&[root.clone()], &|_| false)
                .unwrap_or_else(|e| panic!("{} discovery failed: {e}", agent.display_name()));
            for m in &discovered {
                assert_eq!(
                    m.agent,
                    *agent,
                    "cross-agent misattribution: {}",
                    m.source_path.display()
                );
                assert!(
                    !m.source_member_id.is_empty() && m.source_path.starts_with(&root),
                    "{}: bad discovery result {:?}",
                    agent.display_name(),
                    m.source_path
                );
            }
            eprintln!(
                "[fingerprint] {}: {} members discovered under {}",
                agent.display_name(),
                discovered.len(),
                root.display()
            );
        }
    }
}

#[cfg(test)]
mod title_tests {
    use super::{is_injected_preamble, title_from_text};

    #[test]
    fn a_title_is_one_line_and_short() {
        // Lines are joined with a space (a title is one line) and the ends are
        // trimmed; inner spacing is left as the writer wrote it.
        assert_eq!(
            title_from_text("  帮我\n看下这个模块 ").as_deref(),
            Some("帮我 看下这个模块")
        );
        assert_eq!(title_from_text("   \n  "), None);
        let long = "字".repeat(60);
        let t = title_from_text(&long).unwrap();
        assert_eq!(t.chars().count(), 37, "36 chars plus the ellipsis");
    }

    /// A structured blob is a machine payload, not a title. Codex's review
    /// threads open with one — 27 of this machine's 50 internal threads do — and
    /// the session list is the wrong place for `{"risk_level":"medium",…}`
    /// (方案 §37.15).
    #[test]
    fn a_machine_blob_is_not_a_title() {
        assert_eq!(
            title_from_text("{\"risk_level\":\"medium\",\"outcome\":\"allow\"}"),
            None
        );
        assert_eq!(title_from_text("[1, 2, 3]"), None);
        assert_eq!(
            title_from_text(" {\"a\":1}"),
            None,
            "leading space included"
        );
    }

    /// The runtime does not always put the marker on the first byte: Codex's
    /// pasted-file block opens with a blank line, and testing the raw text let
    /// `"\n# Files pasted by the user: …"` through as a title (§37.15).
    #[test]
    fn an_injected_preamble_is_recognized_after_leading_whitespace() {
        assert!(is_injected_preamble("# Files pasted by the user: ## a.png"));
        assert!(is_injected_preamble(
            "\n# Files pasted by the user: ## a.png"
        ));
        assert!(is_injected_preamble("  <environment_context>"));
        assert!(!is_injected_preamble("看一下这个"));
    }
}
