//! Claude Code Adapter: `~/.claude/projects/<encoded-cwd>/<session>.jsonl`.
//! `CLAUDE_CONFIG_DIR` overrides the root. Raw transcripts stay untouched,
//! always — NoEnding never deletes an Agent-owned source.
//!
//! Member mapping: the main transcript is the ROOT member. Sub-agent
//! transcripts (`<session-uuid>/subagents/agent-<agentId>.jsonl`, official
//! layout) are CHILD members keyed by their `agentId` — their `sessionId`
//! names the PARENT session, so it can never be their identity. Old-format
//! inline `isSidechain=true` lines have no stable execution identity of their
//! own, so no member is fabricated for them: they are counted as
//! `side_activity` and their text never becomes conversation.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::{
    detect_format, read_jsonl_delta_with_prev, AgentCommand, DiscoveredMember,
    DiscoveredMemberKind, MemberObservation, ParsedLine, SessionMessageRole, UsageCategory,
    UsageNote,
};
use crate::domain::{Agent, SessionMember, SessionMemberCursor, SourceAvailability};
use crate::error::Result;
use crate::platform::exec_resolver::{self, AgentInstallation};

pub struct ClaudeAdapter;

/// Conversation text: `text` blocks only. `tool_use` / `tool_result` blocks
/// are machine traffic — counted, never folded into the message.
fn content_parts(content: &Value) -> (String, u64) {
    match content {
        Value::String(s) => (s.clone(), 0),
        Value::Array(arr) => {
            let mut parts = Vec::new();
            let mut tool_calls = 0u64;
            for item in arr {
                match item.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                            parts.push(t.to_string());
                        }
                    }
                    Some("tool_use") => tool_calls += 1,
                    // tool_result and everything else: no text, no count —
                    // the call itself was already counted on the assistant
                    // side.
                    _ => {}
                }
            }
            (parts.join("\n"), tool_calls)
        }
        _ => (String::new(), 0),
    }
}

fn usage_axis(usage: &Value, key: &str) -> u64 {
    usage.get(key).and_then(|v| v.as_u64()).unwrap_or(0)
}

fn thinking_tokens(usage: &Value) -> u64 {
    usage
        .get("output_tokens_details")
        .and_then(|d| d.get("thinking_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
}

/// `<session-uuid>/subagents/agent-<agentId>.jsonl` → (session uuid, agentId).
/// The shape is the marker: a child file's head is plain Claude-shaped (its
/// first line is a user event carrying sessionId + uuid), so no content test
/// can decide — the official layout does. Qoder uses the same `subagents/`
/// layout for its own children, but each adapter only ever scans its own
/// root, so the path routing never crosses the two.
fn subagent_member_identity(path: &Path) -> Option<(String, String)> {
    let name = path.file_name()?.to_str()?;
    let agent_id = name.strip_prefix("agent-")?.strip_suffix(".jsonl")?;
    if agent_id.is_empty() {
        return None;
    }
    let subagents_dir = path.parent()?;
    if subagents_dir.file_name()?.to_str()? != "subagents" {
        return None;
    }
    let session_dir = subagents_dir.parent()?;
    Some((
        session_dir.file_name()?.to_str()?.to_string(),
        agent_id.to_string(),
    ))
}

impl ClaudeAdapter {
    fn parse_member(path: &Path) -> Result<Option<DiscoveredMember>> {
        let file_name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .ok_or_else(|| crate::error::other("无效的 session 文件名"))?;

        let lines = crate::adapters::read_jsonl_lines(path)?;
        let mut session_id: Option<String> = None;
        let mut first_user_text = None;
        let mut first_agent_text = None;
        let mut started_at = None;
        let mut last_ts = None;
        // `ai-title` is REWRITTEN as the conversation moves on, so the last
        // one wins; `agent-name` carries the same value and is the fallback
        // for transcripts that only write that.
        let mut native_title: Option<String> = None;
        let mut agent_name: Option<String> = None;

        // The transcript carries the real cwd on its lines — authoritative
        // and lossless, unlike the encoded (and ambiguous) directory name.
        let mut cwd: Option<String> = None;
        for (_, line) in &lines {
            let v: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            match v.get("type").and_then(|t| t.as_str()) {
                Some("ai-title") => {
                    if let Some(t) = v
                        .get("aiTitle")
                        .and_then(|t| t.as_str())
                        .filter(|t| !t.trim().is_empty())
                    {
                        native_title = Some(t.to_string());
                    }
                }
                Some("agent-name") => {
                    if let Some(t) = v
                        .get("agentName")
                        .and_then(|t| t.as_str())
                        .filter(|t| !t.trim().is_empty())
                    {
                        agent_name = Some(t.to_string());
                    }
                }
                _ => {}
            }
            if cwd.is_none() {
                cwd = v
                    .get("cwd")
                    .and_then(|c| c.as_str())
                    .filter(|c| !c.is_empty())
                    .map(|c| c.to_string());
            }
            if session_id.is_none() {
                session_id = v
                    .get("sessionId")
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string());
            }
            if started_at.is_none() {
                started_at = v
                    .get("timestamp")
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string());
            }
            last_ts = v
                .get("timestamp")
                .and_then(|t| t.as_str())
                .map(|s| s.to_string());
            if first_user_text.is_none()
                && v.get("type").and_then(|t| t.as_str()) == Some("user")
                && v.get("isSidechain").and_then(|s| s.as_bool()) != Some(true)
                && v.get("isMeta").and_then(|s| s.as_bool()) != Some(true)
            {
                if let Some(msg) = v.get("message") {
                    let (text, _) = content_parts(msg.get("content").unwrap_or(&Value::Null));
                    // `<…>` environment blocks and `#`-prefixed injections
                    // (AGENTS.md, attached-file headers) are not user text.
                    if !text.is_empty() && !crate::adapters::is_injected_preamble(&text) {
                        first_user_text = Some(crate::adapters::truncate_text(&text, 400));
                    }
                }
            }
            // Last resort for a title when the session has no user turn.
            if first_agent_text.is_none()
                && v.get("type").and_then(|t| t.as_str()) == Some("assistant")
                && v.get("isSidechain").and_then(|s| s.as_bool()) != Some(true)
            {
                if let Some(msg) = v.get("message") {
                    let (text, _) = content_parts(msg.get("content").unwrap_or(&Value::Null));
                    if !text.is_empty() {
                        first_agent_text = Some(crate::adapters::truncate_text(&text, 400));
                    }
                }
            }
        }

        let meta = std::fs::metadata(path)?;
        let last_activity = meta
            .modified()
            .ok()
            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());

        Ok(Some(DiscoveredMember {
            agent: Agent::ClaudeCode,
            source_member_id: session_id.unwrap_or_else(|| file_name.clone()),
            kind: DiscoveredMemberKind::Root,
            parent_source_member_id: None,
            root_hint: None,
            source_kind: "claude_code_transcript".into(),
            source_path: path.to_path_buf(),
            cwd,
            started_at,
            last_activity_at: last_activity.or(last_ts),
            native_title: native_title.or(agent_name),
            first_user_text,
            first_agent_text,
            metadata: serde_json::json!({}),
        }))
    }

    /// A sub-agent transcript as its own CHILD member. Identity is the
    /// `agentId` (every row carries it and it equals the file name); the
    /// `sessionId` inside names the parent and would collide with the main
    /// transcript. No title sources — a child never names a Logical Session.
    fn parse_subagent_member(
        path: &Path,
        parent_session: &str,
    ) -> Result<Option<DiscoveredMember>> {
        let agent_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_prefix("agent-"))
            .ok_or_else(|| crate::error::other("无效的 subagent 文件名"))?
            .to_string();

        let lines = crate::adapters::read_jsonl_lines(path)?;
        let mut started_at = None;
        let mut last_ts = None;
        let mut cwd: Option<String> = None;
        for (_, line) in &lines {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if started_at.is_none() {
                started_at = v
                    .get("timestamp")
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string());
            }
            last_ts = v
                .get("timestamp")
                .and_then(|t| t.as_str())
                .map(|s| s.to_string());
            if cwd.is_none() {
                cwd = v
                    .get("cwd")
                    .and_then(|c| c.as_str())
                    .filter(|c| !c.is_empty())
                    .map(|c| c.to_string());
            }
        }

        let meta = std::fs::metadata(path)?;
        let last_activity = meta
            .modified()
            .ok()
            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());

        // The `agent-<id>.meta.json` sidecar: who this sub-agent was
        // (type/name/model/depth). Read leniently — the transcript alone
        // carries everything the facts need.
        let sidecar = path.with_file_name(format!("agent-{agent_id}.meta.json"));
        let sidecar_meta: Value = std::fs::read_to_string(&sidecar)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(Value::Null);
        let sidecar_field = |k: &str| {
            sidecar_meta
                .get(k)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from)
        };

        Ok(Some(DiscoveredMember {
            agent: Agent::ClaudeCode,
            source_member_id: agent_id.clone(),
            kind: DiscoveredMemberKind::Child,
            parent_source_member_id: Some(parent_session.to_string()),
            root_hint: Some(parent_session.to_string()),
            source_kind: "claude_code_subagent_transcript".into(),
            source_path: path.to_path_buf(),
            cwd,
            started_at,
            last_activity_at: last_activity.or(last_ts),
            native_title: None,
            first_user_text: None,
            first_agent_text: None,
            metadata: serde_json::json!({
                "parent_session": parent_session,
                "agent_type": sidecar_field("agentType"),
                "name": sidecar_field("name"),
                "model": sidecar_field("model"),
                "task_kind": sidecar_field("taskKind"),
                "team_name": sidecar_field("teamName"),
                "spawn_depth": sidecar_meta.get("spawnDepth").and_then(|d| d.as_i64()),
            }),
        }))
    }
}

/// Runtime overrides in Claude Code's spelling. Claude has no provider flag
/// (capability: unsupported), so `ExecOptions::provider` is not rendered.
fn runtime_args(opts: &crate::adapters::ExecOptions) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(m) = &opts.model {
        args.extend(["--model".into(), m.clone()]);
    }
    if let Some(e) = &opts.effort {
        args.extend(["--effort".into(), e.clone()]);
    }
    args
}

impl crate::adapters::AgentAdapter for ClaudeAdapter {
    fn agent(&self) -> Agent {
        Agent::ClaudeCode
    }

    fn detect(&self) -> Option<AgentInstallation> {
        exec_resolver::resolve_quiet(Agent::ClaudeCode)
    }

    fn discover_members_in(
        &self,
        roots: &[PathBuf],
        unchanged: &dyn Fn(&Path) -> bool,
    ) -> Result<Vec<DiscoveredMember>> {
        let mut out = Vec::new();
        let mut stack: Vec<PathBuf> = roots.to_vec();
        while let Some(dir) = stack.pop() {
            let rd = match std::fs::read_dir(&dir) {
                Ok(rd) => rd,
                Err(_) => continue,
            };
            for entry in rd.filter_map(|e| e.ok()) {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                // Fully ingested and stat-identical since the last pass: the
                // stored cursor is the source of truth, skip the parse.
                if unchanged(&p) {
                    continue;
                }
                // Sub-agent transcripts are members by PATH (official layout,
                // see `subagent_member_identity`) — the fingerprint below
                // would pass them too, but as ROOTs whose identity collides
                // with the parent (their `sessionId` names the parent).
                if let Some((parent_session, _)) = subagent_member_identity(&p) {
                    match Self::parse_subagent_member(&p, &parent_session) {
                        Ok(Some(m)) => out.push(m),
                        Ok(None) => {}
                        Err(e) => eprintln!("[discover] skip {}: {}", p.display(), e),
                    }
                    continue;
                }
                // Any .jsonl is a candidate; only the content fingerprint
                // accepts it. One bad file never aborts the whole scan.
                if detect_format(&p) != Some(Agent::ClaudeCode) {
                    eprintln!(
                        "[discover] skip {} (content fingerprint is not claude_code)",
                        p.display()
                    );
                    continue;
                }
                match Self::parse_member(&p) {
                    Ok(Some(m)) => out.push(m),
                    Ok(None) => {}
                    Err(e) => eprintln!("[discover] skip {}: {}", p.display(), e),
                }
            }
        }
        Ok(out)
    }

    fn read_member_delta(
        &self,
        member: &SessionMember,
        cursor: &SessionMemberCursor,
    ) -> Result<crate::adapters::MemberReadDelta> {
        // No registry in scope: every event bills (direct/test callers).
        self.read_member_delta_claimed(member, cursor, &|_| true)
    }

    fn read_member_delta_claimed(
        &self,
        member: &SessionMember,
        cursor: &SessionMemberCursor,
        claims: &dyn Fn(&str) -> bool,
    ) -> Result<crate::adapters::MemberReadDelta> {
        let path = PathBuf::from(&member.source_path);
        let is_root = member.relation.as_str() == "root";
        // Prev-aware: Claude writes one assistant message as one line per
        // content block, and the parser needs its predecessor — to charge a
        // root call exactly once, and to bill a sub-agent's streaming rows by
        // their delta.
        read_jsonl_delta_with_prev(
            &path,
            cursor,
            crate::adapters::StatsCapabilities::TOOL_AND_SIDE_ACTIVITY,
            &mut |_idx, v, prev| parse_line(v, is_root, prev, claims),
        )
    }

    fn inspect_member_source(&self, member: &SessionMember) -> Result<SourceAvailability> {
        Ok(crate::adapters::inspect_file_source(Path::new(
            &member.source_path,
        )))
    }

    fn build_new_command(
        &self,
        install: &AgentInstallation,
        opts: &crate::adapters::ExecOptions,
        cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        let args = runtime_args(opts);
        Ok(AgentCommand {
            program: install.executable_path.clone(),
            args,
            cwd: cwd.map(|p| p.to_path_buf()),
        })
    }

    fn build_resume_command(
        &self,
        install: &AgentInstallation,
        opts: &crate::adapters::ExecOptions,
        agent_session_id: &str,
        cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        let mut args = vec!["--resume".into(), agent_session_id.into()];
        args.extend(runtime_args(opts));
        Ok(AgentCommand {
            program: install.executable_path.clone(),
            args,
            cwd: cwd.map(|p| p.to_path_buf()),
        })
    }

    fn build_exec_command(
        &self,
        install: &AgentInstallation,
        opts: &crate::adapters::ExecOptions,
        prompt: &str,
    ) -> Result<AgentCommand> {
        let mut args: Vec<String> = vec!["-p".into(), "--output-format".into(), "text".into()];
        args.extend(runtime_args(opts));
        args.push(prompt.to_string());
        Ok(AgentCommand {
            program: install.executable_path.clone(),
            args,
            cwd: None,
        })
    }

    fn build_context_extraction_command(
        &self,
        install: &AgentInstallation,
        opts: &crate::adapters::ExecOptions,
        prompt: &str,
        runtime_dir: &Path,
    ) -> Result<AgentCommand> {
        let mut args: Vec<String> = vec![
            "-p".into(),
            "--no-session-persistence".into(),
            "--output-format".into(),
            "text".into(),
        ];
        args.extend(runtime_args(opts));
        args.push(prompt.to_string());
        Ok(AgentCommand {
            program: install.executable_path.clone(),
            args,
            cwd: Some(runtime_dir.to_path_buf()),
        })
    }
}

/// `message.usage` of one ROOT-transcript assistant call, as counts. Every
/// number is a field the source states: `input_tokens` excludes cache reads;
/// `output_tokens_details.thinking_tokens` is the reasoning output.
///
/// ONE call is written as SEVERAL lines: Claude emits an assistant message once
/// per content block (thinking / text / tool_use), and every one of those lines
/// repeats the message's `message.id` AND its complete `usage` (verified on
/// disk: 145 ids, zero line-to-line drift). Charging each line would multiply
/// the call by its block count, so a line is charged only when its
/// `message.id` differs from the line before it. When the id is absent the
/// source states no way to tell the lines apart and each is charged — the
/// pre-existing behaviour, never silently merged.
fn usage_observation(msg: &Value, prev: Option<&Value>) -> MemberObservation {
    let Some(usage) = msg.get("usage") else {
        return MemberObservation::default();
    };
    let prev_id = prev
        .and_then(|p| p.get("message"))
        .and_then(|m| m.get("id"))
        .and_then(|i| i.as_str());
    if let Some(id) = msg.get("id").and_then(|i| i.as_str()) {
        if Some(id) == prev_id {
            return MemberObservation::default();
        }
    }
    MemberObservation {
        input_tokens: usage_axis(usage, "input_tokens"),
        output_tokens: usage_axis(usage, "output_tokens"),
        cached_tokens: usage_axis(usage, "cache_read_input_tokens"),
        request_count: u64::from(usage.is_object()),
        reasoning_tokens: thinking_tokens(usage),
        ..Default::default()
    }
}

/// `message.usage` of one SUB-AGENT transcript row. There the same id's rows
/// do NOT repeat one usage — they STREAM it (thinking/text rows report
/// `output_tokens: 0`, only the closing tool_use row carries the real total;
/// verified across every sub-agent file on disk), so the root rule would
/// zero every output. Instead each row bills its DELTA against the row
/// before it: the deltas of a call sum to its true totals no matter where a
/// read is cut, and the predecessor crossing an append boundary keeps the
/// arithmetic honest across reads too.
///
/// The request is carried by the call's first row; a continuation row's
/// stored request count defaults back to 1 at write time (the ledger's
/// "0 = assume one" rule), so a multi-block sub-agent call is recorded as
/// slightly more than one request — token totals stay exact.
///
/// Same-call continuation requires the id AND the predecessor to say so; a
/// non-usage line between two rows of one call (not a shape the writer
/// produces) degrades that row to a full first-row bill.
fn streaming_usage_observation(msg: &Value, prev: Option<&Value>) -> MemberObservation {
    let Some(usage) = msg.get("usage") else {
        return MemberObservation::default();
    };
    let prev_msg = prev.and_then(|p| p.get("message"));
    let id = msg.get("id").and_then(|i| i.as_str());
    let prev_id = prev_msg.and_then(|m| m.get("id")).and_then(|i| i.as_str());
    let continues = id.is_some()
        && prev_id == id
        && usage.is_object()
        && prev_msg.and_then(|m| m.get("usage")).map(Value::is_object) == Some(true);
    if continues {
        let prev_usage = prev_msg.and_then(|m| m.get("usage")).unwrap();
        MemberObservation {
            input_tokens: usage_axis(usage, "input_tokens")
                .saturating_sub(usage_axis(prev_usage, "input_tokens")),
            output_tokens: usage_axis(usage, "output_tokens")
                .saturating_sub(usage_axis(prev_usage, "output_tokens")),
            cached_tokens: usage_axis(usage, "cache_read_input_tokens")
                .saturating_sub(usage_axis(prev_usage, "cache_read_input_tokens")),
            reasoning_tokens: thinking_tokens(usage).saturating_sub(thinking_tokens(prev_usage)),
            request_count: 0,
            ..Default::default()
        }
    } else {
        MemberObservation {
            input_tokens: usage_axis(usage, "input_tokens"),
            output_tokens: usage_axis(usage, "output_tokens"),
            cached_tokens: usage_axis(usage, "cache_read_input_tokens"),
            reasoning_tokens: thinking_tokens(usage),
            request_count: u64::from(usage.is_object()),
            ..Default::default()
        }
    }
}

/// A user row that is a machine courier, not a human turn: one Claude session
/// relaying to another in a multi-agent session. Counted as collaboration,
/// never as the user's voice.
fn is_teammate_courier(text: &str) -> bool {
    text.starts_with("Another Claude session sent a message")
}

/// One transcript line's contribution. `is_root` is false for sub-agent
/// transcripts (CHILD members): their prose is never this Session's
/// Conversation, and their usage bills by streaming delta. `prev` decides how
/// a line's usage is charged — see [`usage_observation`] /
/// [`streaming_usage_observation`]. `claims` gates the cross-file usage
/// identities (a continued session replays its predecessor's calls).
fn parse_line(
    v: &Value,
    is_root: bool,
    prev: Option<&Value>,
    claims: &dyn Fn(&str) -> bool,
) -> Option<ParsedLine> {
    let vtype = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match vtype {
        "user" | "assistant" => {}
        // A compaction summary line is a boundary marker, never the
        // pruned content itself.
        _ => return None,
    }
    // The source's own declarations: the compact-summary user row IS the
    // compaction boundary (its text is the pruned-and-replaced context), and
    // `isVisibleInTranscriptOnly` says the row must not surface as
    // conversation. Neither is a turn, and neither bills usage.
    let compact_summary = v
        .get("isCompactSummary")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);
    let transcript_only = v
        .get("isVisibleInTranscriptOnly")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);
    if compact_summary || transcript_only {
        return None;
    }

    let sidechain = v
        .get("isSidechain")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);
    let source_message_id = v
        .get("uuid")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());
    let message_id = v
        .get("message")
        .and_then(|m| m.get("id"))
        .and_then(|i| i.as_str());

    let msg = v.get("message").unwrap_or(&Value::Null);
    let (text, tool_calls) = content_parts(msg.get("content").unwrap_or(&Value::Null));
    let mut observation = if is_root {
        usage_observation(msg, prev)
    } else {
        streaming_usage_observation(msg, prev)
    };
    observation.tool_calls = tool_calls;
    // The response model travels on every assistant row (verified: 0 rows
    // without it) — the ledger anchor needs it on the no-message paths too
    // (a tool-call-only response billed its tokens all the same).
    let model = if vtype == "assistant" {
        msg.get("model").and_then(|m| m.as_str()).map(String::from)
    } else {
        None
    };
    let role = if vtype == "user" {
        SessionMessageRole::User
    } else {
        SessionMessageRole::Assistant
    };

    // Usage-carrying rows only: an identity claimed by another member (the
    // continuation replay) suppresses the tokens but keeps the message and
    // the counts. A row without usage never reaches the ledger, so it must
    // not reach the registry either — claiming it would block the call that
    // does bill under the same id.
    let bills_usage = observation.input_tokens > 0
        || observation.output_tokens > 0
        || observation.cached_tokens > 0
        || observation.reasoning_tokens > 0
        || observation.request_count > 0;
    // The cross-file identity. A root call is identified by its message.id
    // (what a continued transcript replays); a sub-agent row by its own row
    // uuid — one call's deltas are SEPARATE events under one message.id, and
    // a shared id would dedup them back down to the first (partial) one.
    let identity_key = if !bills_usage {
        None
    } else if is_root {
        message_id.map(|id| format!("claude:{id}"))
    } else {
        source_message_id
            .as_deref()
            .map(|u| format!("claude-row:{u}"))
    };
    let suppressed = matches!(identity_key.as_deref().map(claims), Some(false));
    if suppressed {
        observation.input_tokens = 0;
        observation.output_tokens = 0;
        observation.cached_tokens = 0;
        observation.reasoning_tokens = 0;
        observation.request_count = 0;
    }
    let category = if sidechain || !is_root {
        UsageCategory::SideActivity
    } else {
        UsageCategory::Conversation
    };
    // The ledger anchor. A row that will NOT become a message must always
    // carry one (its usage would otherwise have no event identity at all —
    // most of an agent loop); a message row needs one only to carry the
    // cross-file key, since the message itself derives its event.
    let billed_key = if suppressed { None } else { identity_key };
    let billed_note = UsageNote {
        category,
        model: model.clone(),
        provider: None,
        key: billed_key.clone(),
    };

    if sidechain {
        // Sub-agent chatter in the same file: no stable identity → no
        // member, no message — observed only. Its usage still counts:
        // those calls were billed like any other.
        observation.side_activity = 1;
        return Some(ParsedLine {
            message: None,
            observation,
            usage_note: Some(billed_note),
        });
    }
    // isMeta lines are runtime output (command results), not turns.
    let is_meta = v.get("isMeta").and_then(|s| s.as_bool()).unwrap_or(false);
    if text.trim().is_empty() || is_meta {
        // The common agent-loop shape: a model response that is only
        // tool calls. Billed, ledgered, never conversation.
        return Some(ParsedLine {
            message: None,
            observation,
            usage_note: Some(billed_note),
        });
    }
    if role == SessionMessageRole::User && is_teammate_courier(&text) {
        // A machine relay between sessions: collaboration, not the user.
        observation.side_activity = 1;
        return Some(ParsedLine {
            message: None,
            observation,
            usage_note: Some(billed_note),
        });
    }
    // Injected context is never conversation.
    if role == SessionMessageRole::User && crate::adapters::is_injected_preamble(&text) {
        return Some(ParsedLine {
            message: None,
            observation,
            usage_note: Some(billed_note),
        });
    }
    match role {
        SessionMessageRole::User => observation.user_messages = 1,
        SessionMessageRole::Assistant => observation.assistant_messages = 1,
    }
    if !is_root {
        // A sub-agent's turns are counted, but its prose is never this
        // Session's Conversation.
        return Some(ParsedLine {
            message: None,
            observation,
            usage_note: Some(billed_note),
        });
    }
    Some(ParsedLine {
        message: Some(
            crate::adapters::parsed_message(source_message_id, role, text)
                .with_provenance(None, model),
        ),
        observation,
        usage_note: billed_key.map(|_| billed_note),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::{AgentAdapter, MemberReadDelta};
    use crate::domain::StatsUpdate;

    #[test]
    fn cache_creation_is_ignored_but_the_request_is_preserved() {
        let source = serde_json::json!({
            "type": "assistant", "message": {
                "id": "request-1", "model": "claude-test", "content": [],
                "usage": {"cache_creation_input_tokens": 12345}
            }
        });
        let parsed = parse_line(&source, true, None, &|_| true).unwrap();
        let event = crate::adapters::UsageEvent::from_parsed_line(&parsed, None).unwrap();
        assert_eq!(event.request_count, 1);
        assert_eq!(
            (
                event.input_tokens,
                event.output_tokens,
                event.cached_tokens,
                event.reasoning_tokens
            ),
            (0, 0, 0, 0)
        );
        // A second content block for the same request must still be ignored.
        let repeated = parse_line(&source, true, Some(&source), &|_| true).unwrap();
        assert!(crate::adapters::UsageEvent::from_parsed_line(&repeated, None).is_none());
    }

    fn unique_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("noending-claude-{}-{}", tag, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn root_member(path: &Path) -> SessionMember {
        SessionMember {
            id: "mem-claude".into(),
            session_id: "sess-claude".into(),
            agent: Agent::ClaudeCode,
            source_member_id: "s1".into(),
            relation: crate::domain::SessionMemberRelation::Root,
            parent_source_member_id: None,
            source_kind: "claude_code_transcript".into(),
            source_path: path.to_string_lossy().to_string(),
            cwd: None,
            started_at: None,
            last_activity_at: None,
            metadata: serde_json::json!({}),
        }
    }

    fn child_member(path: &Path) -> SessionMember {
        SessionMember {
            relation: crate::domain::SessionMemberRelation::Child,
            source_member_id: "aantigravity-src-f3dcd875f4b2f03f".into(),
            parent_source_member_id: Some("s1".into()),
            source_kind: "claude_code_subagent_transcript".into(),
            ..root_member(path)
        }
    }

    fn member_line(idx: usize, vtype: &str, extra: &str, role: &str, text: &str) -> String {
        let mut v = serde_json::json!({
            "type": vtype,
            "uuid": format!("u{idx}"),
            "sessionId": "s1",
            "cwd": "/repo",
            "timestamp": format!("2026-09-22T15:01:{idx:02}.000Z"),
            "message": { "role": role, "content": [{ "type": "text", "text": text }] },
        });
        if !extra.is_empty() {
            if let Some(fields) = serde_json::from_str::<serde_json::Value>(extra)
                .ok()
                .and_then(|f| f.as_object().cloned())
            {
                for (k, val) in fields {
                    v[k] = val;
                }
            }
        }
        v.to_string()
    }

    /// Prose in, machine traffic out: sidechain text becomes a side
    /// activity count, a tool_use block a tool call, `summary` a compaction,
    /// and two visible assistant prose segments around a tool call both stay.
    #[test]
    fn the_root_read_keeps_prose_and_counts_the_rest() {
        let dir = unique_dir("root-read");
        let path = dir.join("s1.jsonl");
        std::fs::write(
            &path,
            [
                member_line(1, "user", "", "user", "改一下详情页"),
                r#"{"type":"assistant","uuid":"u2","sessionId":"s1","cwd":"/repo","message":{"role":"assistant","usage":{"input_tokens":100,"output_tokens":20,"cache_read_input_tokens":40,"output_tokens_details":{"thinking_tokens":7}},"content":[{"type":"text","text":"我先看现状。"},{"type":"tool_use","name":"Read","id":"t1"}]}}"#.to_string(),
                r#"{"type":"user","uuid":"u3","sessionId":"s1","cwd":"/repo","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"contents"}]}}"#.to_string(),
                r#"{"type":"user","uuid":"u4","sessionId":"s1","isSidechain":true,"cwd":"/repo","message":{"role":"user","content":[{"type":"text","text":"子任务的问题"}]}}"#.to_string(),
                r#"{"type":"assistant","uuid":"u5","sessionId":"s1","isSidechain":true,"cwd":"/repo","message":{"role":"assistant","usage":{"input_tokens":10,"output_tokens":5},"content":[{"type":"text","text":"子任务的结论"}]}}"#.to_string(),
                member_line(6, "assistant", "", "assistant", "问题在这里，已经修改完成。"),
                r#"{"type":"summary","uuid":"u7","sessionId":"s1","summary":"compressed"}"#.to_string(),
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();

        let delta: MemberReadDelta = ClaudeAdapter
            .read_member_delta(&root_member(&path), &SessionMemberCursor::default())
            .unwrap();
        let texts: Vec<(SessionMessageRole, &str)> = delta
            .messages
            .iter()
            .map(|m| (m.role, m.content.as_str()))
            .collect();
        assert_eq!(
            texts,
            vec![
                (SessionMessageRole::User, "改一下详情页"),
                (SessionMessageRole::Assistant, "我先看现状。"),
                (SessionMessageRole::Assistant, "问题在这里，已经修改完成。"),
            ],
            "sidechain text and tool results never become conversation"
        );
        let usage = crate::adapters::test_usage_tokens(&delta.usage_events);
        match delta.stats {
            Some(StatsUpdate::Snapshot(s)) => {
                assert_eq!(s.tool_call_count, Some(1));

                assert_eq!(s.side_activity_count, Some(2), "the two sidechain lines");
                // Usage is additive across calls, and a sidechain call's usage
                // counts like any other.
                assert_eq!(usage[0], 110);
                assert_eq!(usage[1], 25);
                assert_eq!(usage[2], 40, "cache_read_input_tokens");
                assert_eq!(usage[3], 7, "thinking_tokens");
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
        // Every usage-carrying call lands in the ledger — including the
        // sidechain call's, anchored as side activity.
        assert_eq!(delta.usage_events.len(), 2, "{:?}", delta.usage_events);
        assert_eq!(delta.usage_events[1].input_tokens, 10);
        assert_eq!(
            delta.usage_events[1].category,
            crate::adapters::UsageCategory::SideActivity
        );
        assert_eq!(
            delta
                .usage_events
                .iter()
                .map(|e| e.input_tokens)
                .sum::<u64>(),
            110
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Claude writes ONE assistant call as one line per content block
    /// (thinking / text / tool_use), and EVERY line repeats the same
    /// `message.id` and the whole `usage`. The call must be charged once, not
    /// once per block.
    #[test]
    fn one_call_split_across_content_blocks_is_charged_once() {
        let dir = unique_dir("blocks");
        let path = dir.join("s1.jsonl");
        let usage = r#""usage":{"input_tokens":12563,"output_tokens":113,"cache_read_input_tokens":12288,"output_tokens_details":{"thinking_tokens":0}}"#;
        let block = |uuid: &str, content: &str| {
            format!(
                r#"{{"type":"assistant","uuid":"{uuid}","sessionId":"s1","cwd":"/repo","message":{{"id":"msg_1","role":"assistant",{usage},"content":[{content}]}}}}"#
            )
        };
        std::fs::write(
            &path,
            [
                member_line(1, "user", "", "user", "解释一下"),
                block("a1", r#"{"type":"thinking","thinking":"hmm"}"#),
                block("a2", r#"{"type":"text","text":"答案。"}"#),
                block("a3", r#"{"type":"tool_use","name":"Read","id":"t1"}"#),
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();

        let delta = ClaudeAdapter
            .read_member_delta(&root_member(&path), &SessionMemberCursor::default())
            .unwrap();
        let usage = crate::adapters::test_usage_tokens(&delta.usage_events);
        match delta.stats {
            Some(StatsUpdate::Snapshot(s)) => {
                assert_eq!(usage[0], 12563, "charged once for the call");
                assert_eq!(usage[1], 113);
                assert_eq!(usage[2], 12288);
                assert_eq!(s.tool_call_count, Some(1));
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
        assert_eq!(
            delta
                .messages
                .iter()
                .filter(|m| m.role == SessionMessageRole::Assistant)
                .count(),
            1,
            "only the prose block is conversation"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The blocks of one call land SECONDS apart, so a read can catch the file
    /// mid-call. The next read starts on a line whose call was already charged:
    /// the predecessor it can now see is what keeps the usage from doubling.
    #[test]
    fn a_read_starting_mid_message_does_not_recharge_it() {
        let dir = unique_dir("split-append");
        let path = dir.join("s1.jsonl");
        let member = root_member(&path);
        let usage = r#""usage":{"input_tokens":12563,"output_tokens":113}"#;
        let block = |uuid: &str, content: &str| {
            format!(
                r#"{{"type":"assistant","uuid":"{uuid}","sessionId":"s1","cwd":"/repo","timestamp":"2026-09-23T14:10:52.275Z","message":{{"id":"msg_1","role":"assistant",{usage},"content":[{content}]}}}}"#
            )
        };
        // The thinking block lands alone and a read charges the call.
        std::fs::write(
            &path,
            format!(
                "{}\n",
                block("a1", r#"{"type":"thinking","thinking":"hmm"}"#)
            ),
        )
        .unwrap();
        let first = ClaudeAdapter
            .read_member_delta(&member, &SessionMemberCursor::default())
            .unwrap();
        let usage = crate::adapters::test_usage_tokens(&first.usage_events);
        match &first.stats {
            Some(StatsUpdate::Snapshot(_)) => assert_eq!(usage[0], 12563),
            other => panic!("expected snapshot, got {other:?}"),
        }
        let cursor = SessionMemberCursor::from_update(&member.id, first.source.as_ref().unwrap());

        // The remaining blocks arrive later, each repeating the SAME usage.
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            writeln!(f, "{}", block("a2", r#"{"type":"text","text":"答案。"}"#)).unwrap();
            writeln!(
                f,
                "{}",
                block("a3", r#"{"type":"tool_use","name":"Read","id":"t1"}"#)
            )
            .unwrap();
        }
        let second = ClaudeAdapter.read_member_delta(&member, &cursor).unwrap();
        match second.stats {
            Some(StatsUpdate::Delta(d)) => {
                assert_eq!(
                    second.usage_events.first().map(|e| e.input_tokens as i64),
                    None,
                    "the continued call is not charged twice"
                );
                assert_eq!(
                    second.usage_events.first().map(|e| e.output_tokens as i64),
                    None
                );
                assert_eq!(
                    d.tool_call_count,
                    Some(1),
                    "the tool block is still counted"
                );
            }
            other => panic!("expected an append delta, got {other:?}"),
        }
        assert_eq!(second.messages.len(), 1);
        assert_eq!(second.messages[0].content, "答案。");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// isMeta user lines are runtime output, and `<…>`/`#` preambles are
    /// injected context — neither is a human turn.
    #[test]
    fn meta_and_injected_user_lines_are_not_conversation() {
        let dir = unique_dir("meta");
        let path = dir.join("s1.jsonl");
        std::fs::write(
            &path,
            [
                r#"{"type":"user","uuid":"u1","isMeta":true,"sessionId":"s1","cwd":"/repo","message":{"role":"user","content":[{"type":"text","text":"<command-name>/clear</command-name>"}]}}"#,
                r#"{"type":"user","uuid":"u2","sessionId":"s1","cwd":"/repo","message":{"role":"user","content":[{"type":"text","text":"<system-reminder>context</system-reminder>"}]}}"#,
                r#"{"type":"user","uuid":"u3","sessionId":"s1","cwd":"/repo","message":{"role":"user","content":[{"type":"text","text":"真正的问题"}]}}"#,
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();
        let delta: MemberReadDelta = ClaudeAdapter
            .read_member_delta(&root_member(&path), &SessionMemberCursor::default())
            .unwrap();
        assert_eq!(delta.messages.len(), 1);
        assert_eq!(delta.messages[0].content, "真正的问题");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn discovery_reads_session_facts_and_skips_foreign_files() {
        let dir = unique_dir("discover");
        let good = dir.join("s1.jsonl");
        std::fs::write(
            &good,
            [
                member_line(1, "user", "", "user", "把日报整理一下"),
                member_line(2, "assistant", "", "assistant", "整理好了。"),
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();
        // A foreign shape is refused by the fingerprint.
        let foreign = dir.join("x.jsonl");
        std::fs::write(&foreign, "{\"hello\":\"world\"}\n").unwrap();

        let found = ClaudeAdapter
            .discover_members_in(&[dir], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, DiscoveredMemberKind::Root);
        assert_eq!(found[0].source_member_id, "s1");
        assert_eq!(found[0].cwd.as_deref(), Some("/repo"));
        assert_eq!(found[0].first_user_text.as_deref(), Some("把日报整理一下"));
    }

    #[test]
    fn inspect_reports_missing_only_for_a_confirmed_absent_file() {
        let dir = unique_dir("inspect");
        let path = dir.join("s1.jsonl");
        std::fs::write(&path, member_line(1, "user", "", "user", "hi")).unwrap();
        let member = root_member(&path);
        assert_eq!(
            ClaudeAdapter.inspect_member_source(&member).unwrap(),
            SourceAvailability::Present
        );
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            ClaudeAdapter.inspect_member_source(&member).unwrap(),
            SourceAvailability::Missing
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Direct evidence: the assistant row's `message.model` lands on the
    /// message; a missing field stays NULL and provider stays NULL.
    #[test]
    fn assistant_messages_carry_their_source_model() {
        let dir = unique_dir("prov");
        let file = dir.join("s.jsonl");
        let line = |idx: usize, model: Option<&str>| {
            let mut v = serde_json::json!({
                "type": "assistant",
                "uuid": format!("u{idx}"),
                "timestamp": "2026-09-22T15:01:00.000Z",
                "message": { "role": "assistant", "content": [ { "type": "text", "text": "回答" } ] },
            });
            if let Some(m) = model {
                v["message"]["model"] = serde_json::json!(m);
            }
            v.to_string()
        };
        std::fs::write(
            &file,
            [line(1, Some("qwen/qwen3.8-27b")), line(2, None)].join("\n") + "\n",
        )
        .unwrap();
        let delta = ClaudeAdapter
            .read_member_delta(&root_member(&file), &SessionMemberCursor::default())
            .unwrap();
        assert_eq!(delta.messages.len(), 2);
        assert_eq!(delta.messages[0].model.as_deref(), Some("qwen/qwen3.8-27b"));
        assert_eq!(
            delta.messages[0].provider, None,
            "no provider field in the source → NULL, never branding"
        );
        assert_eq!(delta.messages[1].model, None, "missing field stays NULL");
    }

    /// F16: a `subagents/agent-*.jsonl` transcript is a CHILD member keyed by
    /// its `agentId`, attached to the session whose directory holds it —
    /// never a ROOT, and never keyed by `sessionId` (which names the PARENT
    /// and would collide with the main transcript).
    #[test]
    fn subagent_transcripts_are_child_members_keyed_by_agent_id() {
        let dir = unique_dir("sub-discover");
        let session_id = "s1";
        let sub_dir = dir.join(session_id).join("subagents");
        std::fs::create_dir_all(&sub_dir).unwrap();
        let main = dir.join(format!("{session_id}.jsonl"));
        std::fs::write(
            &main,
            [
                member_line(1, "user", "", "user", "主会话的开场"),
                member_line(2, "assistant", "", "assistant", "主会话的回应"),
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();
        let agent_id = "aantigravity-src-f3dcd875f4b2f03f";
        let child = sub_dir.join(format!("agent-{agent_id}.jsonl"));
        std::fs::write(
            &child,
            [
                format!(r#"{{"type":"user","uuid":"c1","sessionId":"{session_id}","agentId":"{agent_id}","cwd":"/repo","timestamp":"2026-10-01T14:00:00.000Z","message":{{"role":"user","content":[{{"type":"text","text":"子任务指令"}}]}}}}"#),
                format!(r#"{{"type":"assistant","uuid":"c2","sessionId":"{session_id}","agentId":"{agent_id}","cwd":"/repo","timestamp":"2026-10-01T14:01:00.000Z","message":{{"id":"msg_c1","role":"assistant","model":"deepseek-flash","usage":{{"input_tokens":500,"output_tokens":30}},"content":[{{"type":"text","text":"子任务完成"}}]}}}}"#),
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();
        std::fs::write(
            sub_dir.join(format!("agent-{agent_id}.meta.json")),
            r#"{"agentType":"antigravity-src","name":"antigravity-src","spawnDepth":0,"model":"deepseek-flash","taskKind":"in_process_teammate","teamName":"session-6f332d32"}"#,
        )
        .unwrap();

        let found = ClaudeAdapter
            .discover_members_in(&[dir], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 2, "main + one subagent, no duplicates");
        let root = found
            .iter()
            .find(|m| m.kind == DiscoveredMemberKind::Root)
            .unwrap();
        assert_eq!(root.source_member_id, session_id);
        let child = found
            .iter()
            .find(|m| m.kind == DiscoveredMemberKind::Child)
            .unwrap();
        assert_eq!(child.source_member_id, agent_id);
        assert_eq!(child.parent_source_member_id.as_deref(), Some(session_id));
        assert_eq!(child.root_hint.as_deref(), Some(session_id));
        assert_eq!(child.native_title, None, "a child never names the session");
        assert_eq!(child.first_user_text, None);
        assert_eq!(child.metadata["agent_type"], "antigravity-src");
        assert_eq!(child.metadata["model"], "deepseek-flash");
        assert_eq!(child.metadata["spawn_depth"], 0);
    }

    /// F19: sub-agent rows STREAM one call's usage (output arrives only on
    /// the closing row), so a child member bills each row's DELTA against its
    /// predecessor — the deltas sum to the true totals. The child's prose is
    /// counted but never becomes conversation.
    #[test]
    fn subagent_usage_bills_the_streaming_delta() {
        let dir = unique_dir("sub-delta");
        let path = dir.join("agent-aantigravity-src-f3dcd875f4b2f03f.jsonl");
        let row = |uuid: &str, text: &str, usage: &str| {
            format!(
                r#"{{"type":"assistant","uuid":"{uuid}","sessionId":"s1","agentId":"aantigravity-src-f3dcd875f4b2f03f","cwd":"/repo","message":{{"id":"msg_1","role":"assistant","model":"deepseek-flash",{usage},"content":[{{"type":"text","text":"{text}"}}]}}}}"#
            )
        };
        std::fs::write(
            &path,
            [
                r#"{"type":"user","uuid":"c0","sessionId":"s1","agentId":"aantigravity-src-f3dcd875f4b2f03f","cwd":"/repo","message":{"role":"user","content":[{"type":"text","text":"子任务指令"}]}}"#.to_string(),
                row("c1", "(thinking)", r#""usage":{"input_tokens":500,"output_tokens":0,"cache_read_input_tokens":9000}"#),
                row("c2", "(draft)", r#""usage":{"input_tokens":500,"output_tokens":0,"cache_read_input_tokens":9000}"#),
                row("c3", "(done)", r#""usage":{"input_tokens":500,"output_tokens":57,"cache_read_input_tokens":9000,"output_tokens_details":{"thinking_tokens":12}}"#),
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();

        let delta = ClaudeAdapter
            .read_member_delta(&child_member(&path), &SessionMemberCursor::default())
            .unwrap();
        // Row 1 bills the prompt side (500/9000), row 2 adds nothing, row 3
        // adds the streamed output — the call's true totals.
        let usage = crate::adapters::test_usage_tokens(&delta.usage_events);
        assert_eq!(usage, [500, 57, 9000, 12], "{:?}", delta.usage_events);
        assert_eq!(
            delta.usage_events.len(),
            2,
            "the zero-delta row makes no event"
        );
        assert_eq!(
            delta.usage_events[1].category,
            crate::adapters::UsageCategory::SideActivity,
            "sub-agent spend is collaboration traffic"
        );
        assert_eq!(delta.messages.len(), 0, "child prose is never conversation");
        match delta.stats {
            Some(crate::domain::StatsUpdate::Snapshot(s)) => {
                assert_eq!(s.user_message_count, Some(1));
                assert_eq!(s.assistant_message_count, Some(3));
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    /// The streaming delta stays honest across an append: the read boundary
    /// hands the parser the last consumed row as its predecessor, so the
    /// continuation row bills only what appeared since.
    #[test]
    fn a_child_append_bills_only_its_delta() {
        let dir = unique_dir("sub-append");
        let path = dir.join("agent-aantigravity-src-f3dcd875f4b2f03f.jsonl");
        let member = child_member(&path);
        std::fs::write(
            &path,
            format!(
                "{}\n",
                r#"{"type":"assistant","uuid":"c1","sessionId":"s1","cwd":"/repo","message":{"id":"msg_1","role":"assistant","usage":{"input_tokens":500,"output_tokens":0,"cache_read_input_tokens":9000},"content":[{"type":"text","text":"(thinking)"}]}}"#
            ),
        )
        .unwrap();
        let first = ClaudeAdapter
            .read_member_delta(&member, &SessionMemberCursor::default())
            .unwrap();
        assert_eq!(
            crate::adapters::test_usage_tokens(&first.usage_events),
            [500, 0, 9000, 0]
        );
        let cursor = SessionMemberCursor::from_update(&member.id, first.source.as_ref().unwrap());

        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","uuid":"c2","sessionId":"s1","cwd":"/repo","message":{{"id":"msg_1","role":"assistant","usage":{{"input_tokens":500,"output_tokens":57,"cache_read_input_tokens":9000}},"content":[{{"type":"text","text":"(done)"}}]}}}}"#
        )
        .unwrap();
        drop(f);
        let second = ClaudeAdapter.read_member_delta(&member, &cursor).unwrap();
        assert_eq!(
            crate::adapters::test_usage_tokens(&second.usage_events),
            [0, 57, 0, 0],
            "only the streamed output is new"
        );
    }

    /// A continued session replays its predecessor's calls under the SAME
    /// message ids. The claim registry decides who bills: the first member's
    /// event carries the identity, the replay keeps its message but never
    /// its tokens.
    #[test]
    fn a_replayed_call_is_billed_once_across_members() {
        let row = serde_json::json!({
            "type": "assistant", "uuid": "u1", "sessionId": "s2", "cwd": "/repo",
            "message": {"id": "msg_q1", "role": "assistant", "model": "qwen/qwen3.8-27b",
                        "usage": {"input_tokens": 2000, "output_tokens": 465},
                        "content": [{"type": "text", "text": "同一个回答"}]}
        });
        let first = parse_line(&row, true, None, &|_| true).unwrap();
        let event = crate::adapters::UsageEvent::from_parsed_line(&first, None).unwrap();
        assert_eq!(event.key.as_deref(), Some("claude:msg_q1"));
        assert_eq!((event.input_tokens, event.output_tokens), (2000, 465));

        // The continuation transcript replays the same call: claims say the
        // identity is taken, so the row keeps its message but bills nothing.
        let replay = parse_line(&row, true, None, &|_| false).unwrap();
        assert!(replay.message.is_some(), "the conversation row still lands");
        assert_eq!(
            replay.observation.user_messages + replay.observation.assistant_messages,
            1
        );
        assert!(
            crate::adapters::UsageEvent::from_parsed_line(&replay, None).is_none(),
            "the replayed tokens are not billed twice"
        );
    }

    /// F17/D: the compact-summary user row (the source marks it
    /// `isCompactSummary` + `isVisibleInTranscriptOnly`) and the inter-agent
    /// courier text are machine traffic — the first is a compaction boundary,
    /// the second counts as collaboration; neither is ever the user's voice.
    #[test]
    fn fake_user_rows_never_become_conversation() {
        let dir = unique_dir("fake-user");
        let path = dir.join("s1.jsonl");
        let summary = "This session is being continued from a previous conversation…".repeat(200);
        std::fs::write(
            &path,
            [
                serde_json::json!({
                    "type": "user", "uuid": "u1", "sessionId": "s1", "cwd": "/repo",
                    "isCompactSummary": true, "isVisibleInTranscriptOnly": true,
                    "message": {"role": "user", "content": summary}
                })
                .to_string(),
                serde_json::json!({
                    "type": "user", "uuid": "u2", "sessionId": "s1", "cwd": "/repo",
                    "message": {"role": "user", "content": "Another Claude session sent a message:\n<teammate-message teammate_id=\"pi-src\">报告完成</teammate-message>"}
                })
                .to_string(),
                member_line(3, "user", "", "user", "真正的问题"),
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();
        let delta = ClaudeAdapter
            .read_member_delta(&root_member(&path), &SessionMemberCursor::default())
            .unwrap();
        assert_eq!(
            delta.messages.len(),
            1,
            "only the human turn is conversation"
        );
        assert_eq!(delta.messages[0].content, "真正的问题");
        match delta.stats {
            Some(crate::domain::StatsUpdate::Snapshot(s)) => {
                assert_eq!(s.user_message_count, Some(1));
                assert_eq!(s.side_activity_count, Some(1), "the courier relay");
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    /// F18: Claude writes its own title (`ai-title`, rewritten — last wins;
    /// `agent-name` as the fallback), so the title chain no longer needs the
    /// first-user-text fallback.
    #[test]
    fn native_title_comes_from_the_transcript() {
        let dir = unique_dir("ai-title");
        let path = dir.join("s1.jsonl");
        std::fs::write(
            &path,
            [
                member_line(1, "user", "", "user", " early prompt that is NOT the title"),
                r#"{"type":"ai-title","aiTitle":"第一个标题","sessionId":"s1"}"#.to_string(),
                r#"{"type":"ai-title","aiTitle":"multi-agent-context-workspace","sessionId":"s1"}"#
                    .to_string(),
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();
        let found = ClaudeAdapter
            .discover_members_in(&[dir.clone()], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].native_title.as_deref(),
            Some("multi-agent-context-workspace")
        );

        // A transcript that only writes `agent-name` still names itself.
        let path2 = dir.join("s2.jsonl");
        std::fs::write(
            &path2,
            [
                r#"{"type":"user","uuid":"n1","sessionId":"s2","cwd":"/repo","timestamp":"2026-09-22T15:01:00.000Z","message":{"role":"user","content":[{"type":"text","text":"问一下"}]}}"#,
                r#"{"type":"agent-name","agentName":"大扫除","sessionId":"s2"}"#,
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();
        let found = ClaudeAdapter
            .discover_members_in(&[dir.clone()], &|_| false)
            .unwrap();
        let s2 = found.iter().find(|m| m.source_member_id == "s2").unwrap();
        assert_eq!(s2.native_title.as_deref(), Some("大扫除"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
