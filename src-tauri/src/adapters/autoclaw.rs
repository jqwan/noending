//! AutoClaw Adapter: `~/.openclaw-autoclaw/agents/<agentId>/sessions/<uuid>.jsonl`.
//!
//! **AutoClaw's session storage is the pi agent core's format, byte for byte.**
//! Verified against both corpora: the header is `{type:"session", version:3,
//! id, timestamp, cwd}`, the line vocabulary is `session` / `message` /
//! `model_change` / `thinking_level_change` / `custom` / `compaction`, message
//! lines carry exactly `{type, id, parentId, timestamp, message}`, and the
//! `custom` entry's field set (`customType`, `data`, `id`, `parentId`,
//! `timestamp`) is the one pi's own `session-manager.js` writes
//! (`appendCustomEntry`). No content marker separates the two.
//!
//! Consequences, both deliberate (方案 §37.6):
//!
//! 1. **Provenance here is the directory, not the bytes.** A transcript is
//!    claimed only when its path really is
//!    `<root>/agents/<agentId>/sessions/<uuid>.jsonl` *and* its first line is
//!    that session header. `detect_format` keeps reporting Pi for these bytes
//!    — which is true at the byte level — so this adapter does not consult it.
//!    A file only becomes an AutoClaw session because AutoClaw's own state
//!    directory says so, the same way Codex's `rollout-` naming is a
//!    pre-filter rather than proof.
//! 2. **No source deletion exists.** NoEnding never deletes an Agent-owned
//!    source (重构方案 §2.6), so there is nothing to refuse — the adapter
//!    simply reads.
//!
//! Member mapping (§26.6): one transcript, one ROOT member. The CLI exists
//! (`openclaw`, inside the app bundle) but cannot be launched from here: every
//! invocation needs `OPENCLAW_STATE_DIR` pointing at `~/.openclaw-autoclaw`,
//! and `AgentCommand` carries no environment — without it the CLI would
//! silently use the default `~/.openclaw`, a different and empty state. So
//! this adapter ingests history only, and `cli_names` is empty.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::{
    read_jsonl_delta, AgentCommand, DiscoveredMember, DiscoveredMemberKind, ExecOptions,
    MemberObservation, ParsedLine, SessionMessageRole,
};
use crate::domain::{Agent, SessionMember, SessionMemberCursor, SourceAvailability};
use crate::error::{other, Result};

pub struct AutoClawAdapter;

/// Text of a message's content blocks — `text` only: `thinking` and `toolCall`
/// blocks are machine traffic the same way tool events are (方案 §36.11).
fn content_text(content: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(arr) = content.as_array() {
        for item in arr {
            if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                    parts.push(t.to_string());
                }
            }
        }
    } else if let Some(s) = content.as_str() {
        parts.push(s.to_string());
    }
    parts.join("\n")
}

/// `toolCall` blocks in a message's content, counted (§7).
fn tool_calls_of(content: &Value) -> u64 {
    content
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter(|i| i.get("type").and_then(|t| t.as_str()) == Some("toolCall"))
                .count() as u64
        })
        .unwrap_or(0)
}

impl AutoClawAdapter {
    /// `<root>/agents/<agentId>/sessions/<uuid>.jsonl` → the agent id.
    ///
    /// Everything else in that directory is not a transcript: `sessions.json`
    /// is the index (keyed `agent:<id>:<suffix>`, and the only place a resume
    /// key exists), and `<uuid>.trajectory*.jsonl` is the runtime trace.
    fn session_agent_id(path: &Path) -> Option<String> {
        let name = path.file_name()?.to_str()?;
        if !name.ends_with(".jsonl") || name.contains(".trajectory") {
            return None;
        }
        let sessions_dir = path.parent()?;
        if sessions_dir.file_name()?.to_str()? != "sessions" {
            return None;
        }
        let agent_dir = sessions_dir.parent()?;
        if agent_dir.parent()?.file_name()?.to_str()? != "agents" {
            return None;
        }
        Some(agent_dir.file_name()?.to_string_lossy().to_string())
    }

    fn parse_member(path: &Path) -> Result<Option<DiscoveredMember>> {
        let Some(agent_id) = Self::session_agent_id(path) else {
            return Ok(None);
        };
        let lines = crate::adapters::read_jsonl_lines(path)?;
        let mut header_id: Option<String> = None;
        let mut cwd: Option<String> = None;
        let mut started_at: Option<String> = None;
        let mut last_ts: Option<String> = None;
        let mut first_user_text: Option<String> = None;
        let mut first_agent_text: Option<String> = None;

        for (idx, line) in &lines {
            let v: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if let Some(ts) = v.get("timestamp").and_then(|t| t.as_str()) {
                if started_at.is_none() {
                    started_at = Some(ts.to_string());
                }
                last_ts = Some(ts.to_string());
            }
            match v.get("type").and_then(|t| t.as_str()) {
                // The header must be the first line; anything else means this
                // is not an AutoClaw transcript (the acceptance rule, §37.6).
                Some("session") => {
                    if *idx != 0 {
                        return Ok(None);
                    }
                    if v.get("version").is_none() || v.get("id").is_none() {
                        return Ok(None);
                    }
                    header_id = v.get("id").and_then(|s| s.as_str()).map(|s| s.to_string());
                    cwd = v
                        .get("cwd")
                        .and_then(|c| c.as_str())
                        .filter(|c| !c.is_empty())
                        .map(|c| c.to_string());
                    started_at = v
                        .get("timestamp")
                        .and_then(|t| t.as_str())
                        .map(|t| t.to_string())
                        .or(started_at);
                }
                Some("message") => {
                    let msg = v.get("message").unwrap_or(&Value::Null);
                    let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
                    let text = content_text(msg.get("content").unwrap_or(&Value::Null));
                    // `<…>` environment blocks and `#`-prefixed injections
                    // are not user text.
                    if first_user_text.is_none()
                        && role == "user"
                        && !text.is_empty()
                        && !crate::adapters::is_injected_preamble(&text)
                    {
                        first_user_text = Some(crate::adapters::truncate_text(&text, 400));
                    }
                    // Last resort for a title (§37.15).
                    if first_agent_text.is_none() && role == "assistant" && !text.is_empty() {
                        first_agent_text = Some(crate::adapters::truncate_text(&text, 400));
                    }
                }
                _ => {}
            }
        }

        // The header is the acceptance rule: without it the path alone is not
        // enough, and the filename is a weaker clue than the session's own id.
        let Some(id) = header_id else {
            return Ok(None);
        };

        let meta = std::fs::metadata(path)?;
        let last_activity = meta
            .modified()
            .ok()
            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());

        Ok(Some(DiscoveredMember {
            agent: Agent::AutoClaw,
            // The agent id is part of the identity: AutoClaw keys its own
            // sessions as `agent:<id>:<suffix>`, and two of its agents are
            // separate stores that could otherwise hand us the same uuid.
            source_member_id: format!("{agent_id}:{id}"),
            kind: DiscoveredMemberKind::Root,
            parent_source_member_id: None,
            root_hint: None,
            source_kind: "autoclaw_transcript".into(),
            source_path: path.to_path_buf(),
            cwd,
            started_at,
            last_activity_at: last_activity.or(last_ts),
            // AutoClaw writes no session title.
            native_title: None,
            first_user_text,
            first_agent_text,
            metadata: serde_json::json!({ "agent_id": agent_id }),
        }))
    }
}

impl crate::adapters::AgentAdapter for AutoClawAdapter {
    fn agent(&self) -> Agent {
        Agent::AutoClaw
    }

    /// Always `None` — see the module doc: the CLI needs an environment this
    /// layer cannot set, so there is nothing launchable to detect.
    fn detect(&self) -> Option<crate::platform::exec_resolver::AgentInstallation> {
        None
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
                if Self::session_agent_id(&p).is_none() {
                    continue;
                }
                if unchanged(&p) {
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
        let path = PathBuf::from(&member.source_path);
        read_jsonl_delta(
            &path,
            cursor,
            crate::adapters::StatsCapabilities::TOOL_AND_COMPACTION,
            &|_idx, v| parse_line(v, member.relation.as_str() == "root"),
        )
    }

    fn inspect_member_source(&self, member: &SessionMember) -> Result<SourceAvailability> {
        Ok(crate::adapters::inspect_file_source(Path::new(
            &member.source_path,
        )))
    }

    fn build_new_command(
        &self,
        _install: &crate::platform::exec_resolver::AgentInstallation,
        _opts: &ExecOptions,
        _context_file: Option<&Path>,
        _cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        Err(other(
            "AutoClaw 的 CLI 需要 OPENCLAW_STATE_DIR 才能找到自己的状态目录，NoEnding 暂时无法安全启动它",
        ))
    }

    fn build_resume_command(
        &self,
        _install: &crate::platform::exec_resolver::AgentInstallation,
        _opts: &ExecOptions,
        _agent_session_id: &str,
        _context_file: Option<&Path>,
        _cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        Err(other(
            "AutoClaw 的 CLI 需要 OPENCLAW_STATE_DIR 才能找到自己的状态目录，NoEnding 暂时无法安全恢复它",
        ))
    }

    fn build_exec_command(
        &self,
        _install: &crate::platform::exec_resolver::AgentInstallation,
        _opts: &ExecOptions,
        _prompt: &str,
    ) -> Result<AgentCommand> {
        Err(other(
            "AutoClaw 的 CLI 需要 OPENCLAW_STATE_DIR 才能找到自己的状态目录，NoEnding 暂时无法安全执行它",
        ))
    }
}

/// One line's contribution: prose only, with `toolCall` blocks counted and
/// `toolResult` messages refused.
fn parse_line(v: &Value, is_root: bool) -> Option<ParsedLine> {
    let vtype = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let source_message_id = v.get("id").and_then(|s| s.as_str()).map(|s| s.to_string());

    match vtype {
        "message" => {
            let msg = v.get("message").unwrap_or(&Value::Null);
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
            let content = msg.get("content").unwrap_or(&Value::Null);
            let text = content_text(content);
            let observation = MemberObservation {
                tool_calls: tool_calls_of(content),
                ..Default::default()
            };
            // A thinking/toolCall-only turn carries no prose but still counts
            // its observations; an observation-less empty line is nothing.
            if text.trim().is_empty() {
                let empty = observation.tool_calls == 0
                    && observation.tool_errors == 0
                    && observation.compactions == 0
                    && observation.side_activity == 0;
                return if empty {
                    None
                } else {
                    Some(ParsedLine::observation_only(observation))
                };
            }
            match (role, is_root) {
                ("user", true) if !crate::adapters::is_injected_preamble(&text) => {
                    Some(ParsedLine {
                        message: Some(crate::adapters::parsed_message(
                            source_message_id,
                            SessionMessageRole::User,
                            text,
                        )),
                        observation,
                    })
                }
                ("assistant", true) => Some(ParsedLine {
                    message: Some(crate::adapters::parsed_message(
                        source_message_id,
                        SessionMessageRole::Assistant,
                        text,
                    )),
                    observation,
                }),
                // Tool output is a `toolResult` message here; not conversation
                // (方案 §36.11).
                _ => Some(ParsedLine::observation_only(observation)),
            }
        }
        "compaction" | "compact" => Some(ParsedLine::observation_only(MemberObservation {
            compactions: 1,
            ..Default::default()
        })),
        // `session` / `model_change` / `thinking_level_change` / `custom` are
        // bookkeeping, not conversation.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::AgentAdapter;
    use crate::domain::{SessionMemberRelation, StatsUpdate};

    fn unique_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("noending-autoclaw-{}-{}", tag, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn root_member(path: &Path) -> SessionMember {
        SessionMember {
            id: "mem-ac".into(),
            session_id: "sess-ac".into(),
            agent: Agent::AutoClaw,
            source_member_id: "auto-coder:4a6255bc-612b-494b-9e7e-3c5d7e58f088".into(),
            relation: SessionMemberRelation::Root,
            parent_source_member_id: None,
            source_kind: "autoclaw_transcript".into(),
            source_path: path.to_string_lossy().to_string(),
            cwd: None,
            started_at: None,
            last_activity_at: None,
            metadata: serde_json::json!({}),
        }
    }

    /// The real shapes: a session header, the bookkeeping lines, a prompt, a
    /// tool-calling turn, and its `toolResult`.
    fn session_lines() -> String {
        [
            r#"{"type":"session","version":3,"id":"4a6255bc-612b-494b-9e7e-3c5d7e58f088","timestamp":"2026-09-18T12:40:00.000Z","cwd":"/Users/jqk/WorkBuddy/x"}"#,
            r#"{"type":"model_change","id":"e1","parentId":"4a6255bc","timestamp":"2026-09-18T12:40:01.000Z","provider":"zhipu","modelId":"glm-5"}"#,
            r#"{"type":"custom","customType":"session-context","data":{"k":1},"id":"e2","parentId":"e1","timestamp":"2026-09-18T12:40:02.000Z"}"#,
            r#"{"type":"message","id":"m1","parentId":"e2","timestamp":"2026-09-18T12:40:03.000Z","message":{"role":"user","content":[{"type":"text","text":"把日报整理一下"}]}}"#,
            r#"{"type":"message","id":"m2","parentId":"m1","timestamp":"2026-09-18T12:40:05.000Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"…"},{"type":"toolCall","name":"read","arguments":{}}]}}"#,
            r#"{"type":"message","id":"m3","parentId":"m2","timestamp":"2026-09-18T12:40:06.000Z","message":{"role":"toolResult","toolCallId":"t1","content":[{"type":"text","text":"file contents"}]}}"#,
            r#"{"type":"message","id":"m4","parentId":"m3","timestamp":"2026-09-18T12:40:09.000Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"…"},{"type":"text","text":"日报已整理好。"}]}}"#,
        ]
        .join("\n")
            + "\n"
    }

    #[test]
    fn discovery_claims_only_its_own_directory_shape() {
        let root = unique_dir("shape");
        let sessions = root.join("agents").join("auto-coder").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join("4a6255bc-612b-494b-9e7e-3c5d7e58f088.jsonl"),
            session_lines(),
        )
        .unwrap();
        // Not a transcript: the index, and the runtime trace beside it.
        std::fs::write(sessions.join("sessions.json"), "{}").unwrap();
        std::fs::write(
            sessions.join("4a6255bc-612b-494b-9e7e-3c5d7e58f088.trajectory.jsonl"),
            "{\"type\":\"trace\"}\n",
        )
        .unwrap();
        // Right file name, wrong place: the shape is the evidence.
        let stray = root.join("elsewhere");
        std::fs::create_dir_all(&stray).unwrap();
        std::fs::write(stray.join("abc.jsonl"), session_lines()).unwrap();
        // Right place, but not a session header first.
        std::fs::write(sessions.join("b2.jsonl"), "{\"hello\":\"world\"}\n").unwrap();

        let found = AutoClawAdapter
            .discover_members_in(&[root], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 1, "one transcript: {found:#?}");
        let m = &found[0];
        assert_eq!(m.agent, Agent::AutoClaw);
        assert_eq!(
            m.source_member_id,
            "auto-coder:4a6255bc-612b-494b-9e7e-3c5d7e58f088"
        );
        assert_eq!(m.kind, DiscoveredMemberKind::Root);
        assert_eq!(m.cwd.as_deref(), Some("/Users/jqk/WorkBuddy/x"));
        assert_eq!(m.started_at.as_deref(), Some("2026-09-18T12:40:00.000Z"));
        assert_eq!(m.first_user_text.as_deref(), Some("把日报整理一下"));
    }

    /// AutoClaw's bytes are pi's bytes; that is why the adapter must not lean
    /// on the content fingerprint, and why this test pins the fact.
    #[test]
    fn the_bytes_themselves_fingerprint_as_pi() {
        let dir = unique_dir("ambiguous");
        let file = dir.join("s.jsonl");
        std::fs::write(&file, session_lines()).unwrap();
        assert_eq!(
            crate::adapters::detect_format(&file),
            Some(Agent::Pi),
            "if this ever changes, the root-as-provenance rule can be revisited"
        );
    }

    /// §32.2 — the conversation is kept; the toolCall is counted; the
    /// toolResult message is out.
    #[test]
    fn ingest_keeps_the_conversation_and_counts_the_tool_traffic() {
        let dir = unique_dir("parse");
        let sessions = dir.join("agents").join("auto-coder").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let file = sessions.join("s.jsonl");
        std::fs::write(&file, session_lines()).unwrap();

        let delta = AutoClawAdapter
            .read_member_delta(&root_member(&file), &SessionMemberCursor::default())
            .unwrap();
        let roles: Vec<(SessionMessageRole, &str)> = delta
            .messages
            .iter()
            .map(|m| (m.role, m.content.as_str()))
            .collect();
        assert_eq!(
            roles,
            vec![
                (SessionMessageRole::User, "把日报整理一下"),
                // The thinking/toolCall-only turn yields no text; the reply
                // with the same shape yields its prose.
                (SessionMessageRole::Assistant, "日报已整理好。"),
            ],
            "got {roles:?}"
        );
        assert_eq!(
            delta.messages[0].source_message_id.as_deref(),
            Some("m1"),
            "identity comes from the entry id"
        );
        match delta.stats {
            Some(StatsUpdate::Snapshot(s)) => {
                assert_eq!(s.tool_call_count, Some(1));
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    #[test]
    fn inspect_reports_missing_only_for_a_confirmed_absent_file() {
        let dir = unique_dir("inspect");
        let sessions = dir.join("agents").join("auto-coder").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let file = sessions.join("s.jsonl");
        std::fs::write(&file, session_lines()).unwrap();
        let member = root_member(&file);
        assert_eq!(
            AutoClawAdapter.inspect_member_source(&member).unwrap(),
            SourceAvailability::Present
        );
        std::fs::remove_file(&file).unwrap();
        assert_eq!(
            AutoClawAdapter.inspect_member_source(&member).unwrap(),
            SourceAvailability::Missing
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
