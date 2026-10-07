//! Agent Adapter layer.
//!
//! Each adapter owns its Agent's data-dir layout, formats, member identity,
//! resume parameters and new-session invocation. Upper layers must never
//! reference `~/.codex` / `~/.claude` / `~/.pi` directly — that is
//! PlatformPaths' job.
//!
//! The source unit is the **member**, not the session: one Logical Session is
//! the root member plus every child/side member resolving to it. Only root
//! members emit conversation; child/side members emit observations only.

pub mod antigravity;
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
    Agent, ParsedSessionMessage, Session, SessionMessageRole, SourceAvailability, SourceCursor,
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

/// What a discovered member IS in the execution graph.
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
    pub fn is_logical_root(self) -> bool {
        matches!(
            self,
            DiscoveredMemberKind::Root | DiscoveredMemberKind::ForkRoot
        )
    }
}

/// One discovered execution member before ingestion.
#[derive(Debug, Clone)]
pub struct DiscoveredMember {
    pub agent: Agent,
    /// Adapter-stable execution identity. Not required to equal the Agent's
    /// native session id — e.g. Qoder subagent transcripts repeat the
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
    /// Root only: first real user prose, for the title chain.
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
    /// NEW-session launches only: a UUID NoEnding generated before spawn.
    /// Adapters whose CLI accepts a prespecified session id (claude --session-id,
    /// pi --session-id) pass it on the command line, so the session file is born
    /// with a known identity and terminal binding becomes exact matching. Only
    /// set for adapters that answer `supports_prespecified_session_id()`.
    pub root_session_id: Option<String>,
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

/// One parsed source line's contribution: at most one conversation message.
/// Lines that are neither (reasoning bodies, bookkeeping, session meta) return
/// `None` from the adapter closure and never reach storage.
pub struct ParsedLine {
    /// The conversation message this line contributes, if any.
    pub message: Option<ParsedSessionMessage>,
}

impl ParsedLine {
    /// A line that carries one conversation message.
    pub fn message_only(message: ParsedSessionMessage) -> Self {
        Self {
            message: Some(message),
        }
    }
}

/// Result of one incremental member read: conversation messages (root members
/// only) and the source state AFTER reading. Messages carry no sequence — the
/// storage layer assigns stable identities.
///
/// `complete_snapshot` is true only when EVERY parsed frame of the read was
/// whole (an unfinished last line or a failed decode is NOT complete); an
/// incomplete full re-scan must leave projection, generation and cursor
/// untouched and record a retryable error instead.
#[derive(Debug, Clone, Default)]
pub struct MemberReadDelta {
    pub messages: Vec<ParsedSessionMessage>,
    pub source: Option<crate::domain::SourceCursorUpdate>,
    pub complete_snapshot: bool,
}

impl MemberReadDelta {
    /// A read that could not establish a whole-snapshot verdict (an error
    /// mid-read, a source that never re-scans): the storage layer treats it as
    /// incomplete.
    pub fn incomplete() -> Self {
        Self {
            complete_snapshot: false,
            ..Default::default()
        }
    }
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

/// Strict source availability for a FILE-backed member. `NotFound` is
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
/// pre-filters, not guarantees. The on-disk formats are mutually exclusive
/// *except* for Qoder, whose transcript is Claude's plus Qoder-only lines, so
/// the order of the checks in [`fingerprint_line`] matters.
///
/// Returns None when the file is not a recognizable session file of any known
/// agent — including genuinely ambiguous content, which must be left unclaimed
/// rather than guessed at.
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
    // Qoder BEFORE Claude, and deliberately so: its transcript is
    // Claude-shaped, so every Qoder message line would satisfy the Claude
    // branch below. These four structural line types are Qoder's own and are
    // re-emitted throughout the file (workspace-directories alone repeats
    // dozens of times), so the head always carries one. `last-prompt` is
    // deliberately NOT decisive: Claude Code 2.1.x writes a `last-prompt`
    // line too ({lastPrompt, leafUuid}), and a bookkeeping line must never
    // steal another agent's file.
    if matches!(
        v.get("type").and_then(|t| t.as_str()),
        Some("workspace-directories")
            | Some("runtime-config")
            | Some("worktree-state")
            | Some("active-leaf")
    ) {
        return Some(Agent::Qoder);
    }
    // Claude event chain: sessionId plus the parentUuid/uuid pair.
    // Housekeeping lines (mode, permission-mode, atis-latch — a 2.1.x
    // transcript HEAD is exactly those) carry sessionId alone and are
    // deliberately not decisive.
    if v.get("sessionId").is_some() && (v.get("parentUuid").is_some() || v.get("uuid").is_some()) {
        return Some(Agent::ClaudeCode);
    }
    // WorkBuddy: no session header, and unlike pi the message payload is NOT
    // nested — `role` / `content` sit at the top level next to `type`. Its
    // `ai-title` / `file-history-snapshot` bookkeeping lines are stamped with
    // an epoch-milli `timestamp`; Claude Code 2.1.x writes the same two line
    // types BARE (no timestamp at all — verified against its real head), so
    // the stamp decides who owns the line and the types alone can never
    // steal a Claude file.
    if matches!(
        v.get("type").and_then(|t| t.as_str()),
        Some("ai-title") | Some("file-history-snapshot")
    ) && v
        .get("timestamp")
        .map(serde_json::Value::is_number)
        .unwrap_or(false)
    {
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
/// Rescans are safe because storage dedups messages by identity: unchanged
/// messages are skipped, changed/new ones are added, and previously ingested
/// history is never touched. A cursor without
/// a usable prefix fingerprint cannot prove append-only continuity, so the
/// source is conservatively treated as a rewrite and fully re-scanned.
pub fn read_jsonl_delta(
    path: &Path,
    cursor: &SourceCursor,
    parse_line: &dyn Fn(usize, &serde_json::Value) -> Option<ParsedLine>,
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
                source: Some(crate::domain::SourceCursorUpdate {
                    file_identity: obs.identity,
                    generation: cursor.generation,
                    byte_offset: cursor.byte_offset,
                    last_seen_size: obs.size,
                    mtime: obs.mtime,
                    start_byte_offset: cursor.byte_offset,
                    prefix_hash: cursor.prefix_hash.clone(),
                }),
                complete_snapshot: true,
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

    let mut messages: Vec<ParsedSessionMessage> = Vec::new();
    let start_offset = start_offset as usize;
    let mut offset = 0usize;
    let mut complete_snapshot = true;
    let mut complete_end = text.len();
    for (idx, frame) in text.split_inclusive('\n').enumerate() {
        let line_start = offset;
        offset += frame.len();
        let line = frame
            .strip_suffix('\n')
            .unwrap_or(frame)
            .strip_suffix('\r')
            .unwrap_or_else(|| frame.strip_suffix('\n').unwrap_or(frame));
        if line_start < start_offset {
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                // Stop at the first damaged frame. The cursor remains before
                // it so an append or repair will retry this line and the tail.
                complete_snapshot = false;
                complete_end = line_start;
                break;
            }
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
        last_seen_size: if complete_snapshot { obs.size } else { 0 },
        mtime: obs.mtime,
        start_byte_offset: start_offset as u64,
        prefix_hash: prefix_hash_of(complete_end),
    };
    Ok(MemberReadDelta {
        messages,
        source: Some(source),
        complete_snapshot,
    })
}

/// Cursor update for readers that replay the whole source on every read (dsh's
/// zstd frames; ZCode's live store).
///
/// Offsets stay in the source's OWN coordinates — that is what the reconcile
/// pre-filter sees, and it is all that is known without decoding. A replay
/// always starts at genesis and message identity (the writer's own ids) absorbs
/// it, so re-reading stores nothing. A new generation is a change of shape
/// (file replaced or truncated), never a mere append.
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

/// Per-member content facts for a member of a live SQLite store. A store may
/// hold MANY members in ONE file (ZCode's `db.sqlite`), and a member's real
/// writes may sit in a `-wal` that any connection touches — even creates at
/// zero bytes — without this member changing. The container's size and mtime
/// therefore say nothing about THIS member: keyed on them, one session's
/// update moves every member's cursor and a mere open reads as activity. The
/// adapter reports each member's own facts instead.
pub struct SqliteMemberFacts {
    /// Monotonic per-member content position (max step idx, message count,
    /// …). A shrink means this member's content was replaced.
    pub position: i64,
    /// When this member's content last moved, in UNIX seconds — the store's
    /// own per-thread stamp or the newest step's time. `None` falls back to
    /// the container's later-of main/`-wal` mtime (members whose content
    /// carries no time at all).
    pub activity_epoch_secs: Option<f64>,
}

/// Source observation for a member of a live SQLite store, in the MEMBER's
/// own coordinates: `byte_offset`/`last_seen_size` carry the member's content
/// position and `mtime` its activity time, so "this member changed" is
/// decided per member — never by a shared file's size/mtime or WAL noise (the
/// activity stamp in the commit path uses the same mtime). `file_identity`
/// still keys on the container: a replaced database, like a content shrink,
/// is a shape change worth a generation bump. Every replay is a full scan;
/// message-identity dedup absorbs the re-read.
pub fn sqlite_replay_cursor_update(
    path: &Path,
    cursor: &SourceCursor,
    member: SqliteMemberFacts,
) -> Result<crate::domain::SourceCursorUpdate> {
    let meta = std::fs::metadata(path)?;
    let identity = file_identity(path);
    let mtime = match member.activity_epoch_secs {
        Some(secs) => Some(secs),
        None => {
            let mut mtime = mtime_secs(&meta);
            let wal_path = PathBuf::from(format!("{}-wal", path.display()));
            if let Ok(wal_meta) = std::fs::metadata(&wal_path) {
                if let Some(w) = mtime_secs(&wal_meta) {
                    mtime = match mtime {
                        Some(m) => Some(m.max(w)),
                        None => Some(w),
                    };
                }
            }
            mtime
        }
    };
    let position = member.position.max(0) as u64;
    let first_ever = cursor.source_file_identity.is_empty() && cursor.last_seen_size == 0;
    let generation = if first_ever {
        0
    } else if identity != cursor.source_file_identity || position < cursor.last_seen_size {
        cursor.generation + 1
    } else {
        cursor.generation
    };
    Ok(crate::domain::SourceCursorUpdate {
        prefix_hash: sha256_hex(
            format!("{}:{}:{}", identity, position, mtime.unwrap_or(0.0)).as_bytes(),
        ),
        file_identity: identity,
        generation,
        byte_offset: position,
        last_seen_size: position,
        mtime,
        start_byte_offset: 0,
    })
}

/// Extract a string field from an object if present.
pub(crate) fn str_field<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|s| s.as_str())
}

/// The Continue route the launcher takes for a session's ROOT member.
/// Adapters decide per member: one Agent can be CLI-resumable for one store
/// and app-only (or unsupported) for another (Antigravity).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeRoute {
    /// Build and run the terminal CLI command (`build_resume_command`).
    Terminal,
    /// Open the Agent's desktop app on this deep link instead.
    Desktop(DesktopResume),
    /// No viable Continue: refuse with this user-facing reason.
    Refused(String),
}

/// Opening a session in the Agent's own desktop app: the Continue route for
/// sessions whose CLI cannot resume them. `note` is user-facing Chinese,
/// rendered verbatim in the resume preview — an app activation that does NOT
/// land on the conversation must say so.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DesktopResume {
    /// The deep link / scheme URL dispatched via the platform's open-uri.
    pub uri: String,
    /// What the user should expect to see, stated in the preview.
    pub note: String,
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

    /// Read only the delta since the session's cursor for THIS session,
    /// detecting append / truncate / rewrite / replacement.
    fn read_session_delta(
        &self,
        session: &Session,
        cursor: &SourceCursor,
    ) -> Result<MemberReadDelta>;

    /// Strict availability verdict for the session's source. This is the
    /// permanent-delete and Resume authority: only a fresh `Missing` may ever
    /// enable a local purge.
    fn inspect_session_source(&self, session: &Session) -> Result<SourceAvailability>;

    /// Build the command line for a New Session.
    ///
    /// NoEnding never injects Context at launch: the command carries only the
    /// launch facts (Agent, cwd, runtime overrides). `opts` carries NoEnding's
    /// override intent only — a `None` field must produce no CLI argument at
    /// all, so the Agent's own configuration stays untouched and unguessed.
    fn build_new_command(
        &self,
        install: &AgentInstallation,
        opts: &ExecOptions,
        cwd: Option<&Path>,
    ) -> Result<AgentCommand>;

    /// Whether this CLI accepts a prespecified session id for a NEW session
    /// (claude `--session-id`, pi `--session-id`). Only then does the launcher
    /// generate one and set `opts.root_session_id` — the birth identity that
    /// makes embedded-terminal binding an exact match instead of a guess.
    /// CLIs that generate their own ids (codex, agy) stay false; their
    /// terminals bind on demand through the session page's verified match.
    fn supports_prespecified_session_id(&self) -> bool {
        false
    }

    fn build_resume_command(
        &self,
        install: &AgentInstallation,
        opts: &ExecOptions,
        agent_session_id: &str,
        cwd: Option<&Path>,
        source_path: Option<&str>,
    ) -> Result<AgentCommand>;

    /// The macOS application-bundle name of this Agent's desktop app, for
    /// presence probing before a desktop-mode Continue. `None` = the Agent
    /// has no desktop surface at all.
    fn desktop_app_name(&self) -> Option<&'static str> {
        None
    }

    /// Whether the Agent ships a TUI/CLI surface. Static product fact —
    /// the default derives from `cli_names`.
    fn has_terminal_cli(&self) -> bool {
        !crate::platform::exec_resolver::cli_names(self.agent()).is_empty()
    }

    /// How Continue surfaces for THIS member's session. `Terminal` keeps the
    /// CLI path; `Desktop` opens the Agent's own app (only when the app is
    /// actually present on this machine — a missing app must refuse, never
    /// dispatch a deep link into nothing); `Refused` states the reason.
    fn continue_route(&self, _session: &Session) -> ResumeRoute {
        ResumeRoute::Terminal
    }

    /// The desktop half of Continue for THIS member's session: the route
    /// continue_session_desktop takes. The default derives from
    /// [`Self::continue_route`] — its Desktop variant IS the desktop route —
    /// so a format whose continue already opens the app is desktop-openable
    /// without a second override. Formats with a richer desktop capability
    /// (codex: per-thread deep link) still override; formats whose continue
    /// is terminal-only refuse here, which is what makes them single-method.
    fn desktop_resume_route(&self, session: &Session) -> ResumeRoute {
        match self.continue_route(session) {
            ResumeRoute::Desktop(open) => ResumeRoute::Desktop(open),
            _ => ResumeRoute::Refused("该会话来源格式不支持在桌面端打开".into()),
        }
    }

    /// Non-interactive one-shot run (codex exec / claude -p / pi -p).
    /// The returned command is executed by platform::exec_runner.
    fn build_exec_command(
        &self,
        install: &AgentInstallation,
        opts: &ExecOptions,
        prompt: &str,
    ) -> Result<AgentCommand>;

    /// Build the isolated command used only for explicit Context extraction.
    /// Adapters must opt in so an unsupported Agent can never silently run an
    /// ordinary assistant command in the extraction runtime directory.
    fn build_context_extraction_command(
        &self,
        _install: &AgentInstallation,
        _opts: &ExecOptions,
        _prompt: &str,
        _runtime_dir: &Path,
    ) -> Result<AgentCommand> {
        Err(other("该 Agent 不支持 Context 提取"))
    }
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
        Box::new(antigravity::AntigravityAdapter),
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

// Shared JSONL helpers

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

/// An injected preamble rather than the user's own words. Adapters call this
/// when picking their first human turn AND when deciding what is Conversation
/// (injected context never becomes a SessionMessage).
///
/// `<…>` tag blocks stay blanket-injected: every agent's machine context is
/// tag-shaped, and a human turn does not open with a tag. `#` headers are NOT
/// blanket-injected any more — a Markdown heading is a normal way to start a
/// real question, and the blanket rule silently ate 21% of codex's real user
/// turns. Only the known generated headers are injected, and codex's envelope
/// that wraps the actual question under `## My request:` is peeled so the
/// wrapped words survive — see [`user_request_text`].
///
/// The test is on trimmed text because the runtime does not always put the
/// marker first: Codex writes the pasted-file block as
/// `"\n# Files pasted by the user: …"`.
pub fn is_injected_preamble(text: &str) -> bool {
    user_request_text(text).is_none()
}

/// `#`-headed machine blocks NoEnding has actually seen (generated constants,
/// matched verbatim). Anything else starting with `#` is the user's own
/// Markdown.
const HASH_INJECTION_HEADERS: &[&str] = &[
    "# AGENTS.md instructions",
    "# Files mentioned by the user:",
    "# Files pasted by the user:",
];

/// The words under a `## My request` header, if the text carries one — codex
/// wraps the user's actual question in that section (bare, or under a
/// mentioned/pasted-files header).
fn peeled_request_text(t: &str) -> Option<String> {
    let idx = t.find("## My request")?;
    let rest = t[idx..]
        .split_once('\n')
        .map(|(_, rest)| rest)
        .unwrap_or("")
        .trim();
    (!rest.is_empty()).then(|| rest.to_string())
}

/// The user's own words out of a turn that may carry an injection envelope.
/// `None` = injected context only, never conversation; `Some` = the text to
/// treat as the human turn (peeled to the request when the turn was an
/// envelope). Adapters that only need the verdict use [`is_injected_preamble`].
pub fn user_request_text(text: &str) -> Option<String> {
    let t = text.trim_start();
    if t.starts_with('<') {
        return None;
    }
    if t.starts_with("## My request") || HASH_INJECTION_HEADERS.iter().any(|h| t.starts_with(h)) {
        return peeled_request_text(t);
    }
    Some(text.to_string())
}

/// Session display title from one text candidate. `None` when there is
/// nothing title-worthy in it.
///
/// A machine blob is not a title: Codex's review threads open with
/// `{"risk_level":"medium",…}`, which is noise in the session list. Same call
/// the user-text tier makes for `<…>` / `#` injections.
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
/// adapter's closure constructs.
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
    }
}

pub(crate) use str_field as json_str_field;

#[cfg(test)]
mod jsonl_integrity_tests {
    use super::*;

    fn temp_file(body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("noending-jsonl-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn a_valid_final_json_frame_does_not_need_a_newline() {
        let frame = r#"{"type":"event","value":1}"#;
        let path = temp_file(frame);
        let delta = read_jsonl_delta(&path, &SourceCursor::default(), &|_, _| None).unwrap();

        assert!(delta.complete_snapshot);
        assert_eq!(delta.source.unwrap().byte_offset, frame.len() as u64);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn malformed_frames_make_the_snapshot_incomplete_and_hold_the_cursor() {
        let valid = r#"{"type":"event","value":1}"#;
        let body = format!("{valid}\n{{broken}}\n{valid}\n");
        let path = temp_file(&body);
        let delta = read_jsonl_delta(&path, &SourceCursor::default(), &|_, _| None).unwrap();

        assert!(!delta.complete_snapshot);
        let source = delta.source.unwrap();
        assert_eq!(source.byte_offset, (valid.len() + 1) as u64);
        assert_eq!(
            source.last_seen_size, 0,
            "discovery must retry the damaged tail"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}

#[cfg(test)]
mod context_extraction_command_tests {
    use super::*;

    #[test]
    fn context_commands_use_isolated_cwd_ephemeral_flags_and_runtime_overrides() {
        let install = |agent| AgentInstallation {
            agent,
            executable_path: "/usr/bin/agent-cli".into(),
            version: None,
            source: "test".into(),
            last_verified_at: "test".into(),
        };
        let cwd = Path::new("/custom/noending/runtime/context-extraction");
        let prompt = "do not persist this prompt";

        for (agent, opts, expected) in [
            (
                Agent::Codex,
                ExecOptions {
                    model: Some("model-codex".into()),
                    effort: Some("high".into()),
                    ..Default::default()
                },
                vec![
                    "--ephemeral",
                    "-m",
                    "model-codex",
                    "model_reasoning_effort=\"high\"",
                ],
            ),
            (
                Agent::ClaudeCode,
                ExecOptions {
                    model: Some("model-claude".into()),
                    effort: Some("high".into()),
                    ..Default::default()
                },
                vec![
                    "--no-session-persistence",
                    "--model",
                    "model-claude",
                    "--effort",
                    "high",
                ],
            ),
            (
                Agent::Pi,
                ExecOptions {
                    model: Some("model-pi".into()),
                    provider: Some("provider-pi".into()),
                    effort: Some("high".into()),
                    ..Default::default()
                },
                vec![
                    "--no-session",
                    "--no-tools",
                    "--provider",
                    "provider-pi",
                    "--model",
                    "model-pi",
                    "--thinking",
                    "high",
                ],
            ),
        ] {
            let adapter = adapter_for(agent);
            let installation = install(agent);
            let extraction = adapter
                .build_context_extraction_command(&installation, &opts, prompt, cwd)
                .unwrap();
            assert_eq!(extraction.cwd.as_deref(), Some(cwd));
            assert_eq!(extraction.args.last().map(String::as_str), Some(prompt));
            for arg in expected {
                assert!(
                    extraction.args.iter().any(|actual| actual == arg),
                    "{agent:?} missing {arg}"
                );
            }

            let ordinary = adapter
                .build_exec_command(&installation, &opts, prompt)
                .unwrap();
            assert_eq!(
                ordinary.cwd, None,
                "ordinary Assistant exec keeps its old cwd policy"
            );
            match agent {
                Agent::Codex => assert!(!ordinary.args.iter().any(|arg| arg == "--ephemeral")),
                Agent::ClaudeCode => assert!(!ordinary
                    .args
                    .iter()
                    .any(|arg| arg == "--no-session-persistence")),
                Agent::Pi => {
                    assert!(ordinary.args.iter().any(|arg| arg == "--no-session"));
                    assert!(ordinary.args.iter().any(|arg| arg == "--no-tools"));
                }
                _ => unreachable!(),
            }
        }
    }
}

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
    /// branch order in `fingerprint_line`.
    #[test]
    fn qoder_is_claimed_before_claude() {
        // A Qoder bookkeeping line: no uuid/sessionId pair, but decisive.
        let qoder = serde_json::json!({
            "type": "workspace-directories", "sessionId": "s", "directories": ["/repo"]
        });
        assert_eq!(fingerprint_line(&qoder), Some(Agent::Qoder));
        // `last-prompt` is written by BOTH qoder and Claude Code 2.1.x, so it
        // decides nothing — a bare one claims nobody.
        let last_prompt = serde_json::json!({"type": "last-prompt", "sessionId": "s"});
        assert_eq!(fingerprint_line(&last_prompt), None);

        // A Qoder message line is indistinguishable from Claude's on its own —
        // which is exactly why a structural bookkeeping line has to be in the
        // head. (Verified on the real corpus: every qoder transcript head
        // carries one of the four structural types.)
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
    /// the same fields under `message`. Its bookkeeping line types
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

        // Claude Code 2.1.x writes the SAME two line types, but bare — no
        // timestamp at all (its real head is exactly these shapes). The
        // missing stamp leaves the line to nobody; the next message line
        // claims the file for Claude.
        let claude_title = serde_json::json!({
            "type": "ai-title", "aiTitle": "multi-agent-context-workspace",
            "sessionId": "f6dcd71f"
        });
        assert_eq!(fingerprint_line(&claude_title), None);
        let claude_snapshot = serde_json::json!({
            "type": "file-history-snapshot", "messageId": "m", "isSnapshotUpdate": false,
            "snapshot": {"trackedFileBackups": {}}
        });
        assert_eq!(fingerprint_line(&claude_snapshot), None);

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

    /// A Claude Code 2.1.x transcript HEAD is four bookkeeping lines (mode →
    /// permission-mode → atis-latch → file-history-snapshot) before the first
    /// message line. Every one of them used to be stealable — the snapshot
    /// landed on WorkBuddy, a head that instead opened with `ai-title` would
    /// too, and `last-prompt` landed on Qoder — which made the newest, heaviest
    /// transcripts invisible to discovery. None of the four may decide
    /// anything now; the first message line claims the file.
    #[test]
    fn a_new_format_claude_head_resolves_to_claude() {
        let dir =
            std::env::temp_dir().join(format!("noending-fp-claude-head-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let head = [
            r#"{"type":"mode","mode":"default","sessionId":"f6dcd71f"}"#,
            r#"{"type":"permission-mode","permissionMode":"auto","sessionId":"f6dcd71f"}"#,
            r#"{"type":"atis-latch","atis":true,"sessionId":"f6dcd71f"}"#,
            r#"{"type":"file-history-snapshot","messageId":"28976ebe","isSnapshotUpdate":false,"snapshot":{}}"#,
            r#"{"type":"ai-title","aiTitle":"multi-agent-context-workspace","sessionId":"f6dcd71f"}"#,
            r#"{"type":"last-prompt","lastPrompt":"这是个什么工程","leafUuid":"b4ca0ea0","sessionId":"f6dcd71f"}"#,
        ];
        let first_turn = r#"{"type":"user","sessionId":"f6dcd71f","uuid":"2df3541a","parentUuid":null,"timestamp":"2026-10-01T13:00:00.000Z","cwd":"/repo","message":{"role":"user","content":[{"type":"text","text":"开工"}]}}"#;
        let path = dir.join("f6dcd71f.jsonl");
        std::fs::write(
            &path,
            head.iter()
                .copied()
                .chain([first_turn])
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap();
        assert_eq!(detect_format(&path), Some(Agent::ClaudeCode));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Real-data check against the local agent roots:
    /// `cargo test real_agent_files_match_fingerprints -- --ignored --nocapture`
    ///
    /// The guarantee asserted here is the one that matters: **discovery never
    /// returns a member belonging to another agent**. It is stated at the
    /// discovery level rather than per file, because a file's content can be
    /// genuinely ambiguous — Qoder's sub-agent transcripts repeat Claude
    /// Code's line shapes and are therefore claimed by nobody —
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
    use super::{is_injected_preamble, title_from_text, user_request_text};

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
    /// the session list is the wrong place for `{"risk_level":"medium",…}`.
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
    /// `"\n# Files pasted by the user: …"` through as a title.
    #[test]
    fn an_injected_preamble_is_recognized_after_leading_whitespace() {
        assert!(is_injected_preamble("# Files pasted by the user: ## a.png"));
        assert!(is_injected_preamble(
            "\n# Files pasted by the user: ## a.png"
        ));
        assert!(is_injected_preamble("  <environment_context>"));
        assert!(!is_injected_preamble("看一下这个"));
    }

    /// The blanket `#` rule used to eat real questions: a Markdown heading is
    /// a normal way to start a turn. Only the known machine headers stay
    /// injected, and the request wrapped under `## My request:` survives.
    #[test]
    fn hash_user_turns_are_kept_and_request_envelopes_are_peeled() {
        // A `#` heading is the user's own words now.
        assert_eq!(
            user_request_text("# 帮我改这个函数").as_deref(),
            Some("# 帮我改这个函数")
        );
        assert!(!is_injected_preamble("## 背景\n问题如下"));

        // The codex envelope: attachments above, the real question under
        // `## My request:` — peel to the words.
        assert_eq!(
            user_request_text(
                "# Files mentioned by the user:\n- a.rs\n\n## My request:\n修复这个报错"
            )
            .as_deref(),
            Some("修复这个报错")
        );
        assert_eq!(
            user_request_text("## My request for Codex:\n跑一下测试").as_deref(),
            Some("跑一下测试")
        );
        // A machine header with nothing under it is still injected.
        assert!(is_injected_preamble("## My request:"));
        assert!(is_injected_preamble("# AGENTS.md instructions\nbe helpful"));
    }
}
