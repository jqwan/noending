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
    detect_format, read_jsonl_delta, AgentCommand, DiscoveredMember, DiscoveredMemberKind,
    ParsedLine, SessionMessageRole,
};
use crate::domain::{Agent, Session, SourceAvailability, SourceCursor};
use crate::error::Result;
use crate::platform::exec_resolver::{self, AgentInstallation};

pub struct ClaudeAdapter;

/// Conversation text: `text` blocks only. `tool_use` / `tool_result` blocks
/// are machine traffic, never folded into the message.
fn content_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(arr) => {
            let mut parts = Vec::new();
            for item in arr {
                match item.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                            parts.push(t.to_string());
                        }
                    }
                    // tool_result and everything else: no text.
                    _ => {}
                }
            }
            parts.join("\n")
        }
        _ => String::new(),
    }
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
                    let text = content_text(msg.get("content").unwrap_or(&Value::Null));
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
                    let text = content_text(msg.get("content").unwrap_or(&Value::Null));
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

    fn read_session_delta(
        &self,
        session: &Session,
        cursor: &SourceCursor,
    ) -> Result<crate::adapters::MemberReadDelta> {
        let path = PathBuf::from(&session.source_path);
        let is_root = true; // every stored session is its root source
        read_jsonl_delta(&path, cursor, &|_idx, v| parse_line(v, is_root))
    }

    fn inspect_session_source(&self, session: &Session) -> Result<SourceAvailability> {
        Ok(crate::adapters::inspect_file_source(Path::new(
            &session.source_path,
        )))
    }

    fn supports_prespecified_session_id(&self) -> bool {
        true
    }

    fn build_new_command(
        &self,
        install: &AgentInstallation,
        opts: &crate::adapters::ExecOptions,
        cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        let mut args = runtime_args(opts);
        // The launcher's prespecified id: the session file is born with a
        // known identity, so the embedded terminal binds by exact match.
        if let Some(id) = &opts.root_session_id {
            args.extend(["--session-id".into(), id.clone()]);
        }
        if let Some(message) = opts.initial_message() {
            args.extend(["--".into(), message.into()]);
        }
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
        _source_path: Option<&str>,
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

/// A user-row text that is actually a machine relay from another session's
/// teammate (the exact envelope wording the writer emits).
fn is_teammate_courier(text: &str) -> bool {
    text.starts_with("Another Claude session sent a message")
}

/// One transcript line's contribution. `is_root` is false for sub-agent
/// transcripts (CHILD members): their prose is never this Session's
/// Conversation, and with execution stats gone a child read observes nothing
/// at all. The machine-traffic filters are message-level facts and stay:
/// compact summaries, transcript-only rows, sidechain chatter, meta rows,
/// teammate couriers and injected preambles never become conversation.
fn parse_line(v: &Value, is_root: bool) -> Option<ParsedLine> {
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
    // conversation.
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
    if sidechain {
        // Sub-agent chatter in the same file: no stable identity → no
        // member, no message.
        return None;
    }
    let source_message_id = v
        .get("uuid")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());

    let msg = v.get("message").unwrap_or(&Value::Null);
    let text = content_text(msg.get("content").unwrap_or(&Value::Null));
    // isMeta lines are runtime output (command results), not turns.
    let is_meta = v.get("isMeta").and_then(|s| s.as_bool()).unwrap_or(false);
    if text.trim().is_empty() || is_meta {
        return None;
    }
    let role = if vtype == "user" {
        SessionMessageRole::User
    } else {
        SessionMessageRole::Assistant
    };
    if role == SessionMessageRole::User && is_teammate_courier(&text) {
        // A machine relay between sessions: collaboration, not the user.
        return None;
    }
    // Injected context is never conversation.
    if role == SessionMessageRole::User && crate::adapters::is_injected_preamble(&text) {
        return None;
    }
    if !is_root {
        // A sub-agent's prose is never this Session's Conversation.
        return None;
    }
    Some(ParsedLine::message_only(crate::adapters::parsed_message(
        source_message_id,
        role,
        text,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::{AgentAdapter, MemberReadDelta};

    fn unique_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("noending-claude-{}-{}", tag, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn root_session(path: &Path) -> Session {
        Session {
            id: "sess-claude".into(),
            agent: Agent::ClaudeCode,
            root_agent_session_id: "s1".into(),
            title: None,
            cwd: None,
            workspace_path_id: None,
            project_id: None,
            owner_workstream_id: None,
            forked_from_session_id: None,
            started_at: None,
            last_activity_at: None,
            last_conversation_at: None,
            trashed_at: None,
            source_kind: "claude_code_transcript".into(),
            source_path: path.to_string_lossy().to_string(),
            metadata: serde_json::json!({}),
            source_file_identity: String::new(),
            source_generation: 0,
            source_byte_offset: 0,
            source_last_seen_size: 0,
            source_mtime: None,
            source_prefix_hash: String::new(),
            source_tail_hash: String::new(),
            fact_generation: 0,
            latest_message_seq: 0,
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
            .read_session_delta(
                &root_session(&path),
                &crate::domain::SourceCursor::default(),
            )
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
        let session = root_session(&path);
        assert_eq!(
            ClaudeAdapter.inspect_session_source(&session).unwrap(),
            SourceAvailability::Present
        );
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            ClaudeAdapter.inspect_session_source(&session).unwrap(),
            SourceAvailability::Missing
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Rows carrying a `message.model` (or lacking it) parse to the same
    /// prose either way since the provenance retirement.
    #[test]
    fn assistant_messages_parse_alike_with_or_without_a_model_field() {
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
            .read_session_delta(
                &root_session(&file),
                &crate::domain::SourceCursor::default(),
            )
            .unwrap();
        assert_eq!(delta.messages.len(), 2);
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
            .read_session_delta(
                &root_session(&path),
                &crate::domain::SourceCursor::default(),
            )
            .unwrap();
        assert_eq!(
            delta.messages.len(),
            1,
            "only the human turn is conversation"
        );
        assert_eq!(delta.messages[0].content, "真正的问题");
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
