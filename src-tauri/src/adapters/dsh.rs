//! dsh Adapter (DeepSeek Harness): `$DSH_HOME/sessions/<encoded-cwd>/<id>/session.v2.jsonl.zstd`,
//! `$DSH_HOME` defaulting to `~/.dsh`.
//!
//! The only agent whose source bytes are not JSONL: transcripts are zstd, and the
//! file grows by **appending complete zstd frames** (one checksummed frame per
//! write batch). Committed bytes are never rewritten — the two truncations dsh
//! performs roll back an in-flight append or repair a torn tail, removing only the
//! *last, incomplete* frame — so the cursor still detects append / truncate /
//! replacement like the plain JSONL readers; only the coordinates differ.
//!
//! Record shape (decoded): line 0 is the header `{type:"session", version, id,
//! createdAt, cwd, …}`, the only record without a `seq`; every later record is
//! `{type, seq, time, data}`. A header `parentSession` makes the transcript a
//! CHILD member of that parent (otherwise ROOT); `origin` / `delegationDepth` ride
//! along as metadata.
//!
//! Usage lives on the STEP: `assistant/message.data.usage` is
//! `{inputTokens, outputTokens, cacheReadTokens?}`, one entry per generation step,
//! so the steps add up. dsh states no cost. A step that produced no prose (tool
//! calls only) still spent tokens, so usage is counted independently of whether
//! the step becomes conversation.
//!
//! Only the ROOT's conversation is ingested: `user/message` (real user turns) and
//! `assistant/message`. A `user/message` carrying `data.source.senderSessionId` was
//! sent by another session — side activity, never conversation. `assistant/attempt`
//! (a retried request — real spend, no prose) and `compaction/summary` (the
//! shadowed range really goes to the model) count USAGE without becoming
//! conversation; `assistant/chunk`, `tool/*`, `todo/write`, `request/*`,
//! `session/title*` and turn/step bookkeeping are dropped or counted. `seq` is
//! writer-assigned and monotonic within a generation, so it is the native
//! message id — and the fork boundary: a header `seedLength` marks how many
//! leading events a fork copied verbatim from its parent, and rows below it are
//! never billed here again.
//!
//! Two shapes need real work:
//! - **The user turn is not always the user's words.** dsh injects its own
//!   `user/message` records for runtime context ("Current runtime context. …" and
//!   `<system-reminder>` blocks); ingesting those would open every session with a
//!   machine preamble, so they are dropped.
//! - **Cursor coordinates.** Offsets live in decoded bytes, not on disk, so this
//!   adapter reads the whole decoded body every time and lets message identity
//!   absorb it, while the stored cursor keeps raw-file identity/size/mtime so the
//!   reconcile pre-filter still works. Every replay is a full scan → stats
//!   SNAPSHOT.
//!
//! The desktop app (DeepSeek Harness) is the only launch surface; the `dsh`
//! CLI needs a profile name that is the user's own setup, and guessing one
//! would boot the wrong tree.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::{
    ms_epoch_to_rfc3339, replay_cursor_update, AgentCommand, DesktopResume, DiscoveredMember,
    DiscoveredMemberKind, ExecOptions, MemberObservation, MemberReadDelta, ParsedLine, ResumeRoute,
    SessionMessageRole,
};
use crate::domain::{Agent, SessionMember, SessionMemberCursor, SourceAvailability};
use crate::error::{other, Result};
use crate::platform::exec_resolver::AgentInstallation;

pub struct DshAdapter;

/// Generations dsh publishes beside each other, newest last (v0 has no version
/// infix; a migration publishes a NEW file and never touches the old one). A
/// session directory can hold several after an upgrade, all describing the same
/// session id, so discovery claims exactly one: the highest generation
/// present — the `.zstd` spelling wins a tie so the claim is deterministic
/// when a directory holds both spellings of one generation. The same
/// generations exist UNCOMPRESSED: a backend configured with
/// `compression: none` writes plain `session*.jsonl` beside the zstd
/// spellings, and the rows are identical.
fn generation_of(file_name: &str) -> Option<u8> {
    match file_name {
        "session.v2.jsonl.zstd" | "session.v2.jsonl" => Some(2),
        "session.v1.jsonl.zstd" | "session.v1.jsonl" => Some(1),
        "session.jsonl.zstd" | "session.jsonl" => Some(0),
        _ => None,
    }
}

/// Zstandard frame magic (RFC 8478 §3.1.1): how a transcript's bytes say
/// whether they are framed or plain.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// Does this decoded record open a dsh transcript? The header is a `session`
/// record with a `createdAt`; requiring it is what keeps a pi header (same
/// `type`/`version`/`id` shape, no `createdAt`) from ever being claimed.
fn is_session_header(v: &Value) -> bool {
    v.get("type").and_then(|t| t.as_str()) == Some("session")
        && v.get("createdAt").map(|c| c.is_number()).unwrap_or(false)
        && v.get("id").map(|i| i.is_string()).unwrap_or(false)
}

/// Text of a content-block array, keeping only human prose. Reasoning and
/// tool-call blocks are machine traffic.
fn text_blocks(content: Option<&Value>) -> String {
    let mut parts = Vec::new();
    if let Some(arr) = content.and_then(|c| c.as_array()) {
        for item in arr {
            if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                    parts.push(t.to_string());
                }
            }
        }
    }
    parts.join("\n")
}

/// dsh's own injected context, written as a `user/message` but not written by
/// the user: the runtime snapshot and `<system-reminder>` blocks.
fn is_machine_context(text: &str) -> bool {
    text.starts_with('<') || text.starts_with("Current runtime context")
}

/// dsh's own task envelope: the parent's task description, pasted in as the
/// first "user" turn of a delegated session. It is not a human turn, and its
/// first line names the envelope, not the session.
///
/// Deliberately NOT folded into [`is_machine_context`]: that one also decides
/// what gets stored as a user turn, and this is a title-only call.
fn is_task_envelope(text: &str) -> bool {
    text.starts_with("## Task context")
}

/// Decoded lines of a transcript, stopping early when `keep_going` says so.
/// A torn final frame ends the walk instead of erroring — the complete prefix
/// is exactly what a mid-write file has to offer, and the next read sees the
/// finished frame.
///
/// Dispatch is on the zstd frame magic, not the file name: a
/// `compression: none` backend writes the identical rows without frames, and
/// those walk as plain lines (a trailing byte run without a newline is the
/// unfinished write, same verdict as a torn frame).
fn scan_lines(raw: &[u8], mut keep_going: impl FnMut(&str) -> bool) -> bool {
    if !raw.starts_with(&ZSTD_MAGIC) {
        let mut lines = raw.split(|b| *b == b'\n').peekable();
        while let Some(line) = lines.next() {
            let is_last = lines.peek().is_none();
            if is_last && line.is_empty() {
                continue; // the final newline's terminator, not a partial line
            }
            let text = String::from_utf8_lossy(line);
            if !keep_going(&text) {
                return false;
            }
            if is_last {
                return false;
            }
        }
        return true;
    }
    let Ok(mut decoder) = zstd::stream::read::Decoder::new(raw) else {
        return false;
    };
    let mut pending: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let n = match decoder.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            // A corrupt / still-being-written frame: the body could NOT be
            // fully decoded, so this read is not a complete snapshot.
            Err(_) => return false,
        };
        pending.extend_from_slice(&chunk[..n]);
        while let Some(end) = pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = pending.drain(..=end).collect();
            let text = String::from_utf8_lossy(&line[..line.len() - 1]);
            if !keep_going(&text) {
                return false;
            }
        }
    }
    if !pending.is_empty() {
        keep_going(&String::from_utf8_lossy(&pending));
        // A trailing partial line: the last frame is not finished.
        return false;
    }
    true
}

/// The subtree that actually holds transcripts. dsh's layout is fixed —
/// `<root>/sessions/<encoded-cwd>/<id>/` — and the rest of `$DSH_HOME` is its
/// plugin tree (a `node_modules` forest): walking that is both enormous and a
/// false-positive risk, so it is never descended into.
fn session_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    roots
        .iter()
        .map(|root| {
            if root.file_name().and_then(|n| n.to_str()) == Some("sessions") {
                return root.clone();
            }
            let sessions = root.join("sessions");
            if sessions.is_dir() {
                sessions
            } else {
                root.clone()
            }
        })
        .collect()
}

/// `data.usage` of one assistant step, in dsh's camelCase spelling. Every step
/// carries its own usage, so they add up. dsh states no cost, and its own
/// `totalTokens` is a derived sum: only the components are stored, so the UI
/// never shows an invented number.
fn usage_observation(v: &Value) -> MemberObservation {
    let Some(usage) = v.pointer("/data/usage") else {
        return MemberObservation::default();
    };
    let n = |k: &str| usage.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    MemberObservation {
        input_tokens: n("inputTokens"),
        output_tokens: n("outputTokens"),
        cached_tokens: n("cacheReadTokens"),
        ..Default::default()
    }
}

/// One decoded record's contribution to the member read. Root members
/// produce conversation; child members produce observations only.
fn parsed_line(v: &Value, is_root: bool, seed_length: i64) -> Option<ParsedLine> {
    let vtype = v.get("type").and_then(|t| t.as_str())?;
    let seq = v.get("seq").and_then(|s| s.as_i64());
    let source_message_id = seq.map(|s| s.to_string());
    // A fork's header records how many events were inherited verbatim from the
    // parent (`seedLength`, `SessionStore::fork`): rows below that boundary are
    // the parent's work — billing them again would charge the parent's calls
    // to this session. The guard covers every spending row type; it is
    // dormant on installs whose headers carry no `seedLength` (observed on
    // 0.2.x: none do).
    let inherited = seq.is_some_and(|seq| seq < seed_length);
    // A `user/message` is not necessarily the user: dsh labels the writer in
    // `data.source`, and the kinds bringing a `senderSessionId` are the messages
    // another session sent.
    let counterpart_id = v
        .pointer("/data/source/senderSessionId")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    match vtype {
        "user/message" => {
            let text = text_blocks(v.pointer("/data/content"));
            if text.trim().is_empty() || is_machine_context(&text) {
                return None;
            }
            if counterpart_id.is_some() {
                // Agent-to-agent relay: observed, never conversation.
                return Some(ParsedLine::observation_only(MemberObservation {
                    side_activity: 1,
                    ..Default::default()
                }));
            }
            let observation = MemberObservation {
                user_messages: 1,
                ..Default::default()
            };
            if !is_root {
                return Some(ParsedLine::observation_only(observation));
            }
            Some(ParsedLine {
                message: Some(crate::adapters::parsed_message(
                    source_message_id,
                    SessionMessageRole::User,
                    text,
                )),
                observation,
            })
        }
        "assistant/message" => {
            // Usage belongs to the step, not to its prose: a step that only
            // called tools spent tokens too, and a CHILD member's own steps are
            // its own execution cost. So it is counted before either the
            // empty-text or the non-root early return can drop it.
            let mut usage = if inherited {
                MemberObservation::default()
            } else {
                usage_observation(v)
            };
            let text = text_blocks(v.pointer("/data/message/content"));
            if text.trim().is_empty() {
                return Some(ParsedLine::observation_only(usage));
            }
            usage.assistant_messages = 1;
            if !is_root {
                return Some(ParsedLine::observation_only(usage));
            }
            // The assistant record itself carries `data.message.source` — a
            // discriminated union gated on `kind == "model"` holding the actual
            // generation identity (`source.provider` / `source.model`).
            // Profile/preset config is NOT message-level fact and is never read.
            let source_meta = v.pointer("/data/message/source").unwrap_or(&Value::Null);
            let prov = if source_meta.get("kind").and_then(|k| k.as_str()) == Some("model") {
                (
                    source_meta
                        .get("provider")
                        .and_then(|p| p.as_str())
                        .map(String::from),
                    source_meta
                        .get("model")
                        .and_then(|m| m.as_str())
                        .map(String::from),
                )
            } else {
                (None, None)
            };
            let message = crate::adapters::parsed_message(
                source_message_id,
                SessionMessageRole::Assistant,
                text,
            )
            .with_provenance(prov.0, prov.1);
            Some(ParsedLine {
                message: Some(message),
                observation: usage,
            })
        }
        // A retried request attempt bills like the settled reply even when it
        // never produced prose; it is execution cost, never conversation.
        "assistant/attempt" => {
            if inherited {
                return None;
            }
            Some(ParsedLine::observation_only(usage_observation(v)))
        }
        // The compaction summary is a real provider call — the shadowed range
        // goes to the model — and its spend is counted in addition to the
        // boundary it writes. (tokscale #1152: falling through to "other"
        // billed these calls at zero.)
        "compaction/summary" => {
            if inherited {
                return None;
            }
            let mut observation = usage_observation(v);
            observation.compactions = 1;
            Some(ParsedLine::observation_only(observation))
        }
        // Other compaction rows are boundaries worth counting, but their
        // payload is machine bookkeeping: a marker, never the pruned content.
        t if t.starts_with("compaction/") => {
            if inherited {
                return None;
            }
            Some(ParsedLine::observation_only(MemberObservation {
                compactions: 1,
                ..Default::default()
            }))
        }
        "tool/call" | "tool/result" => Some(ParsedLine::observation_only(MemberObservation {
            tool_calls: 1,
            ..Default::default()
        })),
        _ => None,
    }
}

impl DshAdapter {
    fn parse_member(path: &Path) -> Result<Option<DiscoveredMember>> {
        let raw = std::fs::read(path)?;
        let mut header: Option<Value> = None;
        let mut native_title: Option<String> = None;
        let mut first_user_text: Option<String> = None;
        let mut first_agent_text: Option<String> = None;

        scan_lines(&raw, |line| {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                return true;
            };
            if header.is_none() {
                if is_session_header(&v) {
                    header = Some(v);
                }
                // The header opens the file; anything else means this is not a
                // dsh transcript, and a mid-file header is not evidence.
                return header.is_some();
            }
            match v.get("type").and_then(|t| t.as_str()) {
                Some("user/message") if first_user_text.is_none() => {
                    let text = text_blocks(v.pointer("/data/content"));
                    if !text.trim().is_empty()
                        && !is_machine_context(&text)
                        && !is_task_envelope(&text)
                        && v.pointer("/data/source/senderSessionId").is_none()
                    {
                        first_user_text = Some(crate::adapters::truncate_text(&text, 400));
                    }
                }
                // Only consulted when the session has no user turn.
                Some("assistant/message") if first_agent_text.is_none() => {
                    let text = text_blocks(v.pointer("/data/message/content"));
                    if !text.trim().is_empty() {
                        first_agent_text = Some(crate::adapters::truncate_text(&text, 400));
                    }
                }
                // dsh names its own sessions and REWRITES the name; measured
                // order is always `fallback` → `provider` → `user`, so the last
                // record is the current one and no ranking is needed.
                //
                // `source.kind` says who named it. `fallback` is dsh truncating
                // the first line of the first message — often the literal machine
                // preamble `## Task context task title:` — i.e. exactly the naive
                // truncation this tier exists to beat, so it is skipped.
                Some("session/title") => {
                    let named_by = v
                        .pointer("/data/source/kind")
                        .and_then(|k| k.as_str())
                        .unwrap_or("");
                    let title = v
                        .pointer("/data/title")
                        .and_then(|t| t.as_str())
                        .filter(|t| !t.trim().is_empty());
                    if named_by != "fallback" {
                        if let Some(t) = title {
                            native_title = Some(t.to_string());
                        }
                    }
                }
                _ => {}
            }
            // The whole transcript is scanned, because the title is the LAST one
            // written and can appear at any position. Only CHANGED files are read
            // at all.
            true
        });

        let Some(header) = header else {
            return Ok(None);
        };
        let id = header
            .get("id")
            .and_then(|i| i.as_str())
            .ok_or_else(|| other("dsh session 头缺少 id"))?;
        let parent = header
            .get("parentSession")
            .and_then(|p| p.as_str())
            .filter(|p| !p.is_empty())
            .map(|p| p.to_string());
        // The header names the member graph directly: a parentSession
        // makes this a CHILD of that parent; origin/delegationDepth ride along
        // as metadata facts.
        let kind = if parent.is_some() {
            DiscoveredMemberKind::Child
        } else {
            DiscoveredMemberKind::Root
        };

        let meta = std::fs::metadata(path)?;
        let last_activity = meta
            .modified()
            .ok()
            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());

        Ok(Some(DiscoveredMember {
            agent: Agent::Dsh,
            source_member_id: id.to_string(),
            kind,
            parent_source_member_id: parent.clone(),
            root_hint: parent,
            source_kind: "dsh_zstd_transcript".into(),
            source_path: path.to_path_buf(),
            cwd: header
                .get("cwd")
                .and_then(|c| c.as_str())
                .filter(|c| !c.is_empty())
                .map(|c| c.to_string()),
            started_at: header
                .get("createdAt")
                .and_then(|c| c.as_i64())
                .and_then(ms_epoch_to_rfc3339),
            last_activity_at: last_activity,
            native_title: if kind.is_logical_root() {
                native_title
            } else {
                None
            },
            first_user_text: if kind.is_logical_root() {
                first_user_text
            } else {
                None
            },
            first_agent_text: if kind.is_logical_root() {
                first_agent_text
            } else {
                None
            },
            metadata: serde_json::json!({
                "origin": header.get("origin").and_then(|o| o.as_str()),
                "delegation_depth": header.get("delegationDepth").and_then(|d| d.as_i64()),
                // How many leading events a fork copied verbatim from its
                // parent: rows below this `seq` are the parent's work and are
                // never billed here. Absent on 0.2.x installs.
                "seed_length": header.get("seedLength").and_then(|s| s.as_i64()),
            }),
        }))
    }
}

impl crate::adapters::AgentAdapter for DshAdapter {
    fn agent(&self) -> Agent {
        Agent::Dsh
    }

    /// Always `None`: `dsh` has a CLI, but every invocation must name a profile
    /// under `$DSH_HOME/profiles` that only the user knows.
    fn detect(&self) -> Option<AgentInstallation> {
        None
    }

    fn discover_members_in(
        &self,
        roots: &[PathBuf],
        unchanged: &dyn Fn(&Path) -> bool,
    ) -> Result<Vec<DiscoveredMember>> {
        let mut out = Vec::new();
        let mut stack: Vec<PathBuf> = session_roots(roots);
        while let Some(dir) = stack.pop() {
            let rd = match std::fs::read_dir(&dir) {
                Ok(rd) => rd,
                Err(_) => continue,
            };
            // One session per directory: the newest published generation wins,
            // so an upgraded session is never ingested twice under one id.
            let mut newest: Option<(u8, PathBuf)> = None;
            for entry in rd.filter_map(|e| e.ok()) {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let Some(generation) = generation_of(name) else {
                    continue;
                };
                // Equal generation in both spellings: the `.zstd` claim wins,
                // so what a directory with a mixed pair reports is stable.
                let wins = match &newest {
                    None => true,
                    Some((g, _)) if generation > *g => true,
                    Some((g, stored)) => {
                        generation == *g
                            && name.ends_with(".zstd")
                            && !stored.to_string_lossy().ends_with(".zstd")
                    }
                };
                if wins {
                    newest = Some((generation, p));
                }
            }
            let Some((_, path)) = newest else {
                continue;
            };
            if unchanged(&path) {
                continue;
            }
            match Self::parse_member(&path) {
                Ok(Some(m)) => out.push(m),
                Ok(None) => {}
                Err(e) => eprintln!("[discover] skip {}: {}", path.display(), e),
            }
        }
        Ok(out)
    }

    fn read_member_delta(
        &self,
        member: &SessionMember,
        cursor: &SessionMemberCursor,
    ) -> Result<MemberReadDelta> {
        let path = PathBuf::from(&member.source_path);
        let raw = std::fs::read(&path)?;

        // Decoded text cannot be seeked into, so every read replays the whole
        // body and leans on message identity: each record carries the writer's
        // own `seq`, so a replay stores nothing it already has.
        let is_root = member.relation.as_str() == "root";
        let seed_length = member
            .metadata
            .get("seed_length")
            .and_then(|s| s.as_i64())
            .unwrap_or(0);
        let mut messages = Vec::new();
        let mut observation = MemberObservation::default();
        let complete_snapshot = scan_lines(&raw, |line| {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                return true;
            };
            let Some(p) = parsed_line(&v, is_root, seed_length) else {
                return true;
            };
            observation.add(&p.observation);
            if let Some(mut m) = p.message {
                if !m.content.trim().is_empty() {
                    if m.ts.is_none() {
                        m.ts = v
                            .get("time")
                            .and_then(|t| t.as_i64())
                            .and_then(ms_epoch_to_rfc3339);
                    }
                    if m.source_position.is_empty() {
                        m.source_position =
                            format!("seq:{}", m.source_message_id.clone().unwrap_or_default());
                    }
                    messages.push(m);
                }
            }
            true
        });

        let source = replay_cursor_update(&path, cursor, &raw)?;
        Ok(MemberReadDelta {
            stats: crate::adapters::stats_update_from(
                &observation,
                &source,
                crate::adapters::StatsCapabilities::TOOL_COMPACTION_AND_SIDE_ACTIVITY.with_tokens(),
            ),
            messages,
            source: Some(source),
            complete_snapshot,
            next_active_provider: None,
            next_active_model: None,
        })
    }

    fn inspect_member_source(&self, member: &SessionMember) -> Result<SourceAvailability> {
        Ok(crate::adapters::inspect_file_source(Path::new(
            &member.source_path,
        )))
    }

    /// The desktop app is dsh's only launch surface: the `dsh` CLI needs a
    /// profile name only the user knows, so NoEnding never launches it in a
    /// terminal.
    fn desktop_app_name(&self) -> Option<&'static str> {
        Some("DeepSeek Harness")
    }

    /// The registered scheme `dsh://` has no known session route, so Continue
    /// can only activate the app. App absent → refuse.
    fn continue_route(&self, _member: &SessionMember) -> ResumeRoute {
        if !crate::platform::paths::app_bundle_present("DeepSeek Harness") {
            return ResumeRoute::Refused("未找到 dsh 桌面应用，无法继续该会话".into());
        }
        ResumeRoute::Desktop(DesktopResume {
            uri: "dsh://".into(),
            note: "将打开 dsh 桌面应用（应用内不定位到该会话）".into(),
        })
    }

    fn build_new_command(
        &self,
        _install: &AgentInstallation,
        _opts: &ExecOptions,
        _cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        Err(other("dsh 需要指定 profile，NoEnding 无法替用户选择"))
    }

    fn build_resume_command(
        &self,
        _install: &AgentInstallation,
        _opts: &ExecOptions,
        _agent_session_id: &str,
        _cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        Err(other("dsh 需要指定 profile，NoEnding 无法替用户选择"))
    }

    fn build_exec_command(
        &self,
        _install: &AgentInstallation,
        _opts: &ExecOptions,
        _prompt: &str,
    ) -> Result<AgentCommand> {
        Err(other("dsh 需要指定 profile，NoEnding 无法替用户选择"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::AgentAdapter;
    use crate::domain::{SessionMemberRelation, StatsUpdate};

    fn unique_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "noending-dsh-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn frame(body: &str) -> Vec<u8> {
        zstd::encode_all(body.as_bytes(), 3).unwrap()
    }

    /// A session directory as dsh writes it: `<root>/<slug>/<id>/<file>`, one
    /// zstd frame per write batch.
    fn session_dir(root: &Path, id: &str, file_name: &str, frames: &[&str]) -> PathBuf {
        let dir = root.join("proj").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        let mut bytes = Vec::new();
        for f in frames {
            // One frame per write batch; every record is newline-terminated.
            bytes.extend(frame(&format!("{f}\n")));
        }
        std::fs::write(dir.join(file_name), bytes).unwrap();
        dir
    }

    fn header(id: &str, extra: &str) -> String {
        format!(
            r#"{{"type":"session","version":2,"id":"{id}","createdAt":1788969915099,"cwd":"/repo","isSeeded":false,"delegationDepth":0,"agentPreset":"standard"{extra}}}"#
        )
    }

    fn user(seq: i64, text: &str) -> String {
        format!(
            r#"{{"type":"user/message","seq":{seq},"time":1788969927205,"data":{{"content":[{{"type":"text","text":"{text}"}}],"role":"user"}}}}"#
        )
    }

    fn assistant(seq: i64, text: &str) -> String {
        format!(
            r#"{{"type":"assistant/message","seq":{seq},"time":1788969948545,"data":{{"turn":1,"step":1,"message":{{"role":"assistant","content":[{{"type":"reasoning","text":"thinking"}},{{"type":"text","text":"{text}"}}]}}}}}}"#
        )
    }

    const RUNTIME_CTX: &str = r#"{"type":"user/message","seq":1,"time":1788969927205,"data":{"content":[{"type":"text","text":"Current runtime context. This snapshot supersedes earlier runtime-context snapshots."}],"role":"user"}}"#;
    const REMINDER: &str = r#"{"type":"user/message","seq":2,"time":1788969927206,"data":{"content":[{"type":"text","text":"<system-reminder>\nworkspace instructions\n</system-reminder>"}],"role":"user"}}"#;

    fn member_at(path: &Path, relation: SessionMemberRelation) -> SessionMember {
        SessionMember {
            id: "mem-dsh".into(),
            session_id: "sess-dsh".into(),
            agent: Agent::Dsh,
            source_member_id: "session-x".into(),
            relation,
            parent_source_member_id: None,
            source_kind: "dsh_zstd_transcript".into(),
            source_path: path.to_string_lossy().to_string(),
            cwd: None,
            started_at: None,
            last_activity_at: None,
            metadata: serde_json::json!({}),
        }
    }

    #[test]
    fn discovery_reads_header_facts_and_skips_injected_context() {
        let root = unique_dir("parse");
        let id = "session-abc";
        session_dir(
            &root,
            id,
            "session.v2.jsonl.zstd",
            &[
                &header(id, ""),
                RUNTIME_CTX,
                &user(3, "请审计这个仓库"),
                &assistant(4, "好的。"),
            ],
        );
        let found = DshAdapter
            .discover_members_in(&[root.clone()], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 1, "{found:#?}");
        let m = &found[0];
        assert_eq!(m.agent, Agent::Dsh);
        assert_eq!(m.source_member_id, id);
        assert_eq!(m.kind, DiscoveredMemberKind::Root);
        assert_eq!(m.cwd.as_deref(), Some("/repo"));
        // The runtime snapshot is a user/message too, but not the user's words.
        assert_eq!(m.first_user_text.as_deref(), Some("请审计这个仓库"));
        assert_eq!(
            m.started_at.as_deref(),
            Some("2026-09-09T16:05:15.099+00:00")
        );
        assert!(m.parent_source_member_id.is_none());
    }

    /// The header names the member graph: a `parentSession` makes the
    /// transcript a CHILD of that parent, and a child never carries a title
    /// source.
    #[test]
    fn a_parent_session_makes_the_transcript_a_child_member() {
        let root = unique_dir("subagent");
        let child = "06a1b2c3-0000-4000-8000-000000000001";
        let parent = "session-parent";
        session_dir(
            &root,
            child,
            "session.v2.jsonl.zstd",
            &[
                &header(
                    child,
                    r#","parentSession":"session-parent","origin":"subagent","delegationDepth":1"#,
                ),
                &user(1, "delegated task"),
                &assistant(2, "done"),
            ],
        );
        let found = DshAdapter.discover_members_in(&[root], &|_| false).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].source_member_id, child);
        assert_eq!(found[0].kind, DiscoveredMemberKind::Child);
        assert_eq!(found[0].parent_source_member_id.as_deref(), Some(parent));
        assert_eq!(found[0].root_hint.as_deref(), Some(parent));
        assert_eq!(
            found[0].native_title, None,
            "a child never names a Logical Session"
        );
        assert_eq!(
            found[0].metadata["origin"].as_str(),
            Some("subagent"),
            "the header facts ride along as execution metadata"
        );
    }

    #[test]
    fn the_newest_published_generation_wins() {
        let root = unique_dir("generations");
        let id = "12f75210-3fd8-4500-ab7d-014646eb1462";
        let dir = session_dir(
            &root,
            id,
            "session.v2.jsonl.zstd",
            &[
                &header(id, ""),
                &user(1, "second generation prompt"),
                &assistant(2, "second generation reply"),
            ],
        );
        // An upgraded session keeps its older files beside the new one, and
        // they describe the same id: two claims would fight over one row.
        std::fs::write(
            dir.join("session.jsonl.zstd"),
            frame(&format!(
                "{}\n{}\n",
                r#"{"type":"session","version":0,"id":"12f75210-3fd8-4500-ab7d-014646eb1462","createdAt":1788355822116,"cwd":"/repo"}"#,
                user(7, "legacy prompt")
            )),
        )
        .unwrap();

        let found = DshAdapter.discover_members_in(&[root], &|_| false).unwrap();
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(
            found[0].first_user_text.as_deref(),
            Some("second generation prompt")
        );
        assert!(
            found[0].source_path.ends_with("session.v2.jsonl.zstd"),
            "the newest generation is the source: {:?}",
            found[0].source_path
        );
    }

    /// The root read keeps the conversation and drops machine traffic; the
    /// writer's own `seq` is the native message id.
    #[test]
    fn the_root_read_keeps_the_conversation_and_drops_machine_traffic() {
        let id = "session-ingest";
        let dir = session_dir(
            &unique_dir("ingest"),
            id,
            "session.v2.jsonl.zstd",
            &[
                &header(id, ""),
                r#"{"type":"permission/preset","seq":0,"time":1,"data":{"preset":"workspace-write"}}"#,
                RUNTIME_CTX,
                REMINDER,
                &user(3, "帮我看看这个 bug"),
                r#"{"type":"tool/call","seq":4,"time":1788969930000,"data":{"name":"read"}}"#,
                &assistant(5, "看完了。"),
                r#"{"type":"session/title","seq":6,"time":2,"data":{"title":"看 bug"}}"#,
                r#"{"type":"compaction/prune","seq":7,"time":3,"data":{}}"#,
            ],
        );
        let file = dir.join("session.v2.jsonl.zstd");
        let delta = DshAdapter
            .read_member_delta(
                &member_at(&file, SessionMemberRelation::Root),
                &SessionMemberCursor::default(),
            )
            .unwrap();
        let texts: Vec<(SessionMessageRole, &str)> = delta
            .messages
            .iter()
            .map(|m| (m.role, m.content.as_str()))
            .collect();
        assert_eq!(
            texts,
            vec![
                (SessionMessageRole::User, "帮我看看这个 bug"),
                (SessionMessageRole::Assistant, "看完了。"),
            ],
        );
        // The writer's own seq is the native id, so dedup is exact.
        assert_eq!(delta.messages[0].source_message_id.as_deref(), Some("3"));
        assert_eq!(
            delta.messages[0].ts.as_deref(),
            Some("2026-09-09T16:05:27.205+00:00")
        );
        match delta.stats {
            Some(StatsUpdate::Snapshot(s)) => {
                assert_eq!(s.tool_call_count, Some(1));
                assert_eq!(s.compaction_count, Some(1));
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    /// A `user/message` that carries a `senderSessionId` was written by another
    /// session, not by the user — dsh says so in `data.source`. It is
    /// execution observation now, never conversation.
    #[test]
    fn a_message_from_another_session_is_side_activity() {
        let id = "session-agent-msg";
        let dir = session_dir(
            &unique_dir("agent-msg"),
            id,
            "session.v2.jsonl.zstd",
            &[
                &header(id, ""),
                &user(3, "怎么拆这个任务"),
                r#"{"type":"user/message","seq":8,"time":4,"data":{"content":[{"type":"text","text":"Background subagent reported:"},{"type":"text","text":"只读检查完成。"}],"role":"user","source":{"kind":"subagent-report","form":"relay","senderSessionId":"a73db4b1-ade0-4635-ac0b-a8666803f733"}}}"#,
                r#"{"type":"user/message","seq":9,"time":5,"data":{"content":[{"type":"text","text":"我的补充结论。"}],"role":"user","source":{"kind":"agent-message","form":"relay","senderSessionId":"session-peer"}}}"#,
                // A plugin notice is also a `user/message`, and it is NOT an
                // agent message: no sender.
                r#"{"type":"user/message","seq":10,"time":6,"data":{"content":[{"type":"text","text":"The approval policy changed from \"ask\" to \"never\"."}],"role":"user","source":{"kind":"plugin","plugin":"user-approval"}}}"#,
            ],
        );
        let file = dir.join("session.v2.jsonl.zstd");
        let delta = DshAdapter
            .read_member_delta(
                &member_at(&file, SessionMemberRelation::Root),
                &SessionMemberCursor::default(),
            )
            .unwrap();
        let roles: Vec<SessionMessageRole> = delta.messages.iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            vec![SessionMessageRole::User, SessionMessageRole::User],
            "the two relayed messages are out of the conversation"
        );
        assert_eq!(delta.messages[0].content, "怎么拆这个任务");
        assert_eq!(
            delta.messages[1].content,
            "The approval policy changed from \"ask\" to \"never\"."
        );
        match delta.stats {
            Some(StatsUpdate::Snapshot(s)) => {
                assert_eq!(s.side_activity_count, Some(2));
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// dsh names its own sessions and rewrites the name; measured order is
    /// always `fallback` → `provider` → `user`, so the last record is the
    /// current one — no ranking table needed.
    #[test]
    fn the_last_session_title_wins() {
        let root = unique_dir("dsh-title");
        let lines = [
            r#"{"type":"session","version":2,"id":"sess-t","createdAt":1788969915099,"cwd":"/tmp/proj"}"#,
            r#"{"type":"user/message","seq":9,"time":1788969927205,"data":{"content":[{"type":"text","text":"demo"}],"role":"user"}}"#,
            r#"{"type":"session/title","seq":12,"time":1788969928000,"data":{"title":"demo","messageSeqs":[9],"source":{"kind":"fallback"}}}"#,
            r#"{"type":"session/title","seq":13,"time":1788969930000,"data":{"title":"了解 feedback 指令的用途","messageSeqs":[9],"source":{"kind":"provider"}}}"#,
        ];
        session_dir(&root, "sess-t", "session.v2.jsonl.zstd", &lines);

        let found = DshAdapter
            .discover_members_in(&[root.clone()], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(
            found[0].native_title.as_deref(),
            Some("了解 feedback 指令的用途")
        );
        assert_eq!(found[0].first_user_text.as_deref(), Some("demo"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `fallback` is dsh truncating the first line of the first message, which is
    /// often the literal machine preamble `## Task context task title:` — not a
    /// title anyone wrote, and not the parent's words as a user turn either.
    #[test]
    fn the_task_envelope_is_not_the_user_turn() {
        let root = unique_dir("dsh-envelope");
        let lines = [
            r#"{"type":"session","version":2,"id":"sess-e","createdAt":1788969915099,"cwd":"/tmp/proj"}"#,
            r###"{"type":"user/message","seq":8,"time":1788969927000,"data":{"content":[{"type":"text","text":"## Task context\ntask title: 回收站中的任务及会话打开逻辑"}],"role":"user"}}"###,
        ];
        session_dir(&root, "sess-e", "session.v2.jsonl.zstd", &lines);

        let found = DshAdapter
            .discover_members_in(&[root.clone()], &|_| false)
            .unwrap();
        assert_eq!(found[0].first_user_text, None, "not a human turn");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_fallback_title_is_not_a_native_title() {
        let root = unique_dir("dsh-fallback");
        let lines = [
            r#"{"type":"session","version":2,"id":"sess-f","createdAt":1788969915099,"cwd":"/tmp/proj"}"#,
            r#"{"type":"user/message","seq":8,"time":1788969927000,"data":{"content":[{"type":"text","text":"看下这个"}],"role":"user"}}"#,
            r###"{"type":"session/title","seq":12,"time":1788969928000,"data":{"title":"## Task context task title:","messageSeqs":[8],"source":{"kind":"fallback"}}}"###,
        ];
        session_dir(&root, "sess-f", "session.v2.jsonl.zstd", &lines);

        let found = DshAdapter
            .discover_members_in(&[root.clone()], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].native_title, None, "the fallback is not a title");
        assert_eq!(found[0].first_user_text.as_deref(), Some("看下这个"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The decoded header is pi's header shape plus `createdAt`. Only the
    /// decoded header distinguishes them, so the bytes alone never do — and
    /// the adapter must not claim a file whose header is pi's.
    #[test]
    fn a_pi_header_inside_a_zstd_file_is_not_claimed() {
        let root = unique_dir("pi-shaped");
        let dir = root.join("proj").join("p1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("session.v2.jsonl.zstd"),
            frame(r#"{"type":"session","version":3,"id":"p1","cwd":"/x"}"#),
        )
        .unwrap();
        let found = DshAdapter.discover_members_in(&[root], &|_| false).unwrap();
        assert!(found.is_empty(), "{found:#?}");
    }

    /// A torn final frame (dsh mid-write) must not cost the complete prefix:
    /// the frames that finished are ingested, the partial one waits.
    #[test]
    fn a_torn_final_frame_leaves_the_complete_prefix_intact() {
        let id = "session-torn";
        let dir = session_dir(
            &unique_dir("torn"),
            id,
            "session.v2.jsonl.zstd",
            &[
                &header(id, ""),
                &user(3, "first prompt"),
                &assistant(4, "first reply"),
            ],
        );
        let file = dir.join("session.v2.jsonl.zstd");

        // Append half of the next frame: the bytes on disk are not a frame.
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&file)
                .unwrap();
            let partial = frame(&format!("{}\n", user(5, "half written")));
            f.write_all(&partial[..partial.len() / 2]).unwrap();
        }

        let delta = DshAdapter
            .read_member_delta(
                &member_at(&file, SessionMemberRelation::Root),
                &SessionMemberCursor::default(),
            )
            .unwrap();
        let texts: Vec<&str> = delta.messages.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(texts, vec!["first prompt", "first reply"], "{texts:?}");
    }

    /// A child member replays to observations only.
    #[test]
    fn a_child_member_produces_observations_only() {
        let id = "session-child";
        let dir = session_dir(
            &unique_dir("child-read"),
            id,
            "session.v2.jsonl.zstd",
            &[
                &header(
                    id,
                    r#","parentSession":"session-parent","origin":"subagent","delegationDepth":1"#,
                ),
                &user(1, "delegated task"),
                &assistant(2, "done"),
                // A child's own steps are its own execution cost.
                r#"{"type":"assistant/message","seq":3,"time":3,"data":{"turn":1,"step":1,"usage":{"inputTokens":42,"outputTokens":7},"message":{"role":"assistant","content":[{"type":"text","text":"child"}]}}}"#,
            ],
        );
        let file = dir.join("session.v2.jsonl.zstd");
        let delta = DshAdapter
            .read_member_delta(
                &member_at(&file, SessionMemberRelation::Child),
                &SessionMemberCursor::default(),
            )
            .unwrap();
        assert!(delta.messages.is_empty());
        match delta.stats {
            Some(StatsUpdate::Snapshot(s)) => {
                assert_eq!(s.input_tokens, Some(42), "a child's own usage is counted");
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Usage is per STEP and adds up across the session. A step that emitted no
    /// prose (tool calls only) spent tokens too, so it is counted without
    /// becoming conversation.
    #[test]
    fn usage_adds_up_per_step_including_a_prose_less_step() {
        let id = "session-usage";
        let dir = session_dir(
            &unique_dir("usage"),
            id,
            "session.v2.jsonl.zstd",
            &[
                &header(id, ""),
                &user(3, "跑一下"),
                r#"{"type":"assistant/message","seq":4,"time":1,"data":{"turn":1,"step":1,"usage":{"inputTokens":100,"outputTokens":10,"totalTokens":110},"message":{"role":"assistant","content":[{"type":"tool_use","name":"read"}]}}}"#,
                r#"{"type":"assistant/message","seq":5,"time":2,"data":{"turn":1,"step":2,"usage":{"inputTokens":8459,"outputTokens":64,"cacheReadTokens":7000,"totalTokens":8523},"message":{"role":"assistant","content":[{"type":"text","text":"好了。"}]}}}"#,
            ],
        );
        let file = dir.join("session.v2.jsonl.zstd");
        let delta = DshAdapter
            .read_member_delta(
                &member_at(&file, SessionMemberRelation::Root),
                &SessionMemberCursor::default(),
            )
            .unwrap();
        match delta.stats {
            Some(StatsUpdate::Snapshot(s)) => {
                assert_eq!(s.input_tokens, Some(8559), "both steps, one prose-less");
                assert_eq!(s.output_tokens, Some(74));
                assert_eq!(s.cached_tokens, Some(7000), "cacheReadTokens");
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
        // The prose-less step produced no conversation, but the other did.
        assert_eq!(delta.messages.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `data.message.source` gated on `kind == "model"` attributes the
    /// generation identity; any other source kind stays NULL.
    #[test]
    fn assistant_source_kind_model_attributes_the_generation_identity() {
        let id = "session-prov";
        let with_model = r#"{"type":"assistant/message","seq":10,"time":1788969948545,"data":{"message":{"role":"assistant","content":[{"type":"text","text":"答一"}],"source":{"kind":"model","provider":"openai-codex","model":"gpt-5.6-luna"}}}}"#;
        let non_model = r#"{"type":"assistant/message","seq":11,"time":1788969948546,"data":{"message":{"role":"assistant","content":[{"type":"text","text":"答二"}],"source":{"kind":"plugin","provider":"x","model":"y"}}}}"#;
        let no_source = r#"{"type":"assistant/message","seq":12,"time":1788969948547,"data":{"message":{"role":"assistant","content":[{"type":"text","text":"答三"}]}}}"#;
        let dir = session_dir(
            &unique_dir("prov"),
            id,
            "session.v2.jsonl.zstd",
            &[
                &header(id, ""),
                RUNTIME_CTX,
                REMINDER,
                &user(3, "问"),
                with_model,
                non_model,
                no_source,
            ],
        );
        let file = dir.join("session.v2.jsonl.zstd");
        let delta = DshAdapter
            .read_member_delta(
                &member_at(&file, SessionMemberRelation::Root),
                &SessionMemberCursor::default(),
            )
            .unwrap();
        let assistant: Vec<(Option<&str>, Option<&str>)> = delta
            .messages
            .iter()
            .filter(|m| m.role == SessionMessageRole::Assistant)
            .map(|m| (m.provider.as_deref(), m.model.as_deref()))
            .collect();
        assert_eq!(
            assistant,
            vec![
                (Some("openai-codex"), Some("gpt-5.6-luna")),
                (None, None),
                (None, None),
            ]
        );
    }

    /// dsh 只有桌面端这一个启动面。回归守卫：它曾被误标为 TUI/CLI。
    #[test]
    fn dsh_is_desktop_only() {
        assert!(!DshAdapter.has_terminal_cli());
        assert_eq!(DshAdapter.desktop_app_name(), Some("DeepSeek Harness"));
    }

    /// 应用在场时 Continue 打开桌面应用，缺席时明确拒绝。
    #[test]
    fn continue_route_opens_the_desktop_app_or_refuses() {
        let member = member_at(Path::new("/tmp/dsh-session"), SessionMemberRelation::Root);
        let route = DshAdapter.continue_route(&member);
        if crate::platform::paths::app_bundle_present("DeepSeek Harness") {
            match route {
                ResumeRoute::Desktop(d) => assert_eq!(d.uri, "dsh://"),
                other => panic!("expected a desktop open, got {other:?}"),
            }
        } else {
            assert!(matches!(route, ResumeRoute::Refused(_)));
        }
    }

    /// A `compression: none` backend writes the same rows without zstd frames:
    /// discovery claims the plain spelling and the read decodes it by magic.
    #[test]
    fn a_plain_uncompressed_transcript_is_ingested() {
        let root = unique_dir("plain");
        let id = "session-plain";
        let dir = root.join("proj").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("session.v2.jsonl"),
            format!(
                "{}\n{}\n{}\n",
                header(id, ""),
                user(3, "plain question"),
                assistant(4, "plain answer")
            ),
        )
        .unwrap();

        let found = DshAdapter
            .discover_members_in(&[root.clone()], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 1, "{found:#?}");
        assert!(found[0].source_path.ends_with("session.v2.jsonl"));

        let delta = DshAdapter
            .read_member_delta(
                &member_at(&found[0].source_path, SessionMemberRelation::Root),
                &SessionMemberCursor::default(),
            )
            .unwrap();
        let texts: Vec<&str> = delta.messages.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(texts, vec!["plain question", "plain answer"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Both spellings of one generation in one directory: the `.zstd` claim
    /// wins, deterministically.
    #[test]
    fn the_zstd_spelling_wins_a_generation_tie() {
        let root = unique_dir("tie");
        let id = "session-tie";
        let dir = root.join("proj").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.jsonl"), format!("{}\n", header(id, ""))).unwrap();
        std::fs::write(
            dir.join("session.jsonl.zstd"),
            frame(&format!("{}\n", header(id, ""))),
        )
        .unwrap();

        let found = DshAdapter.discover_members_in(&[root], &|_| false).unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].source_path.ends_with("session.jsonl.zstd"));
    }

    /// The compaction summary is a real provider call: its usage is counted in
    /// addition to the boundary, and it never becomes conversation.
    #[test]
    fn a_compaction_summary_bills_its_call() {
        let id = "session-summary";
        let dir = session_dir(
            &unique_dir("summary"),
            id,
            "session.v2.jsonl.zstd",
            &[
                &header(id, ""),
                &user(1, "长对话"),
                r#"{"type":"compaction/summary","seq":2,"time":2,"data":{"usage":{"inputTokens":5000,"outputTokens":800,"cacheReadTokens":12000}}}"#,
            ],
        );
        let file = dir.join("session.v2.jsonl.zstd");
        let delta = DshAdapter
            .read_member_delta(
                &member_at(&file, SessionMemberRelation::Root),
                &SessionMemberCursor::default(),
            )
            .unwrap();
        assert!(
            delta
                .messages
                .iter()
                .all(|m| m.role == SessionMessageRole::User),
            "the summary never becomes conversation"
        );
        match delta.stats {
            Some(StatsUpdate::Snapshot(s)) => {
                assert_eq!(s.input_tokens, Some(5000));
                assert_eq!(s.output_tokens, Some(800));
                assert_eq!(s.cached_tokens, Some(12000));
                assert_eq!(s.compaction_count, Some(1));
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A retried attempt bills its own call but never becomes conversation —
    /// only the settled reply counts as an assistant message.
    #[test]
    fn an_assistant_attempt_bills_without_becoming_conversation() {
        let id = "session-attempt";
        let dir = session_dir(
            &unique_dir("attempt"),
            id,
            "session.v2.jsonl.zstd",
            &[
                &header(id, ""),
                &user(1, "问题"),
                r#"{"type":"assistant/attempt","seq":2,"time":2,"data":{"usage":{"inputTokens":700,"outputTokens":0}}}"#,
                &assistant(3, "最终回答"),
            ],
        );
        let file = dir.join("session.v2.jsonl.zstd");
        let delta = DshAdapter
            .read_member_delta(
                &member_at(&file, SessionMemberRelation::Root),
                &SessionMemberCursor::default(),
            )
            .unwrap();
        assert_eq!(delta.messages.len(), 2, "the attempt is not conversation");
        match delta.stats {
            Some(StatsUpdate::Snapshot(s)) => {
                assert_eq!(s.input_tokens, Some(700));
                assert_eq!(s.assistant_message_count, Some(1), "only the settled reply");
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A fork's `seedLength` marks the inherited prefix: rows below the
    /// boundary are the parent's work and are never billed here, while the
    /// inherited history stays part of the transcript.
    #[test]
    fn a_seeded_fork_does_not_bill_its_inherited_prefix() {
        let id = "session-seed";
        let dir = session_dir(
            &unique_dir("seed"),
            id,
            "session.v2.jsonl.zstd",
            &[
                &header(id, r#","seedLength":3"#),
                &user(1, "inherited question"),
                r#"{"type":"assistant/message","seq":2,"time":2,"data":{"usage":{"inputTokens":900,"outputTokens":60},"message":{"role":"assistant","content":[{"type":"text","text":"inherited answer"}]}}}"#,
                r#"{"type":"assistant/message","seq":4,"time":4,"data":{"usage":{"inputTokens":120,"outputTokens":9},"message":{"role":"assistant","content":[{"type":"text","text":"own answer"}]}}}"#,
            ],
        );
        let file = dir.join("session.v2.jsonl.zstd");

        let found = DshAdapter
            .discover_members_in(&[dir.clone()], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].metadata["seed_length"], 3,
            "discovery carries the boundary"
        );

        let mut member = member_at(&file, SessionMemberRelation::Root);
        member.metadata = serde_json::json!({ "seed_length": 3 });
        let delta = DshAdapter
            .read_member_delta(&member, &SessionMemberCursor::default())
            .unwrap();
        let texts: Vec<&str> = delta.messages.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(
            texts,
            vec!["inherited question", "inherited answer", "own answer"],
            "the inherited history stays part of the transcript"
        );
        match delta.stats {
            Some(StatsUpdate::Snapshot(s)) => {
                assert_eq!(
                    s.input_tokens,
                    Some(120),
                    "only the fork's own work is billed"
                );
                assert_eq!(s.output_tokens, Some(9));
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
