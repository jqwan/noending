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

pub mod claude;
pub mod codex;
pub mod pi;

use std::io::BufRead;
use std::path::{Path, PathBuf};

use crate::domain::{Agent, ParsedEvent, Session, SourceCursor};
use crate::error::{other, Result};
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

fn mtime_secs(meta: &std::fs::Metadata) -> Option<f64> {
    let t: chrono::DateTime<chrono::Utc> = meta.modified().ok()?.into();
    Some(t.timestamp() as f64 + t.timestamp_subsec_nanos() as f64 / 1e9)
}

/// Content fingerprint: which agent wrote this session file?
///
/// Filename conventions (rollout-*.jsonl, *.jsonl under some directory) are
/// pre-filters, not guarantees — and no format except Codex names its
/// writer. The three on-disk formats are mutually exclusive, so the first
/// few parseable lines decide deterministically:
/// - Codex: every line is the `{ordinal, payload, type}` envelope;
/// - Claude Code: event lines carry `{sessionId, parentUuid/uuid, message}`;
/// - Pi: `{type:"session", id}` header or `{parentId, provider|modelId|
///   thinkingLevel}` event lines.
///
/// Returns None when the file is not a recognizable session file of any
/// known agent.
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
    // Claude event chain: sessionId plus the parentUuid/uuid pair.
    // Housekeeping lines (queue-operation etc.) carry sessionId alone and
    // are deliberately not decisive.
    if v.get("sessionId").is_some() && (v.get("parentUuid").is_some() || v.get("uuid").is_some()) {
        return Some(Agent::ClaudeCode);
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
fn sha256_hex(data: &[u8]) -> String {
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
        let ts = v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .map(|s| s.to_string());
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

/// Extract a string field from an object if present.
pub(crate) fn str_field<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|s| s.as_str())
}

pub trait AgentAdapter: Send + Sync {
    fn agent(&self) -> Agent;

    /// CLI detect() via ExecutableResolver + data-dir presence.
    fn detect(&self) -> Option<AgentInstallation>;

    /// Discover external sessions under the given roots (the user-enabled
    /// ingest sources). Each root is scanned recursively with the adapter's
    /// own file-matching rules; missing directories are skipped quietly.
    fn discover_sessions_in(&self, roots: &[PathBuf]) -> Result<Vec<DiscoveredSession>>;

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
}

pub fn all_adapters() -> Vec<Box<dyn AgentAdapter>> {
    vec![
        Box::new(codex::CodexAdapter),
        Box::new(claude::ClaudeAdapter),
        Box::new(pi::PiAdapter),
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
        Some(truncate_text(&t, 80))
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
    #[test]
    #[ignore]
    fn real_agent_files_match_fingerprints() {
        use crate::platform::paths::resolve_agent_data_dir;
        for agent in Agent::all() {
            let Some(root) = resolve_agent_data_dir(agent) else {
                continue;
            };
            if !root.is_dir() {
                continue;
            }
            let mut checked = 0usize;
            let mut skipped = 0usize;
            let mut stack = vec![root];
            while let Some(dir) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for entry in rd.filter_map(|e| e.ok()) {
                    let p = entry.path();
                    if p.is_dir() {
                        stack.push(p);
                        continue;
                    }
                    let is_jsonl = p.extension().and_then(|e| e.to_str()) == Some("jsonl");
                    let is_rollout = p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with("rollout-"))
                        .unwrap_or(false);
                    let candidate = match agent {
                        Agent::Codex => is_jsonl && is_rollout,
                        _ => is_jsonl,
                    };
                    if !candidate {
                        continue;
                    }
                    checked += 1;
                    // The hard guarantee: never misattribute a file to the
                    // wrong agent. Non-session files (e.g. ~/.claude/
                    // history.jsonl) legitimately fingerprint as None.
                    match detect_format(&p) {
                        Some(detected) => assert_eq!(
                            detected,
                            agent,
                            "cross-agent misattribution: {}",
                            p.display()
                        ),
                        None => skipped += 1,
                    }
                }
            }
            eprintln!(
                "[fingerprint] {}: {} files verified, {} non-session files skipped",
                agent.display_name(),
                checked - skipped,
                skipped
            );
        }
    }
}
