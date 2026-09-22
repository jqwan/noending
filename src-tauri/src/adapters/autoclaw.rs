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
//! 2. **Source deletion is not offered.** `prepare_single_file_source_deletion`
//!    (§16.3) demands a content fingerprint that points back at this agent, and
//!    that proof does not exist for these bytes. The trait default returns a
//!    clear refusal instead of a best guess.
//!
//! The CLI exists (`openclaw`, inside the app bundle) but cannot be launched
//! from here: every invocation needs `OPENCLAW_STATE_DIR` pointing at
//! `~/.openclaw-autoclaw`, and `AgentCommand` carries no environment — without
//! it the CLI would silently use the default `~/.openclaw`, a different and
//! empty state. So this adapter ingests history only, and `cli_names` is empty.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::{
    read_jsonl_delta, AgentCommand, DiscoveredSession, ExecOptions, ParsedLine, ReadDelta,
};
use crate::domain::{Agent, Session, SourceCursor};
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

    fn parse_session_file(path: &Path) -> Result<Option<DiscoveredSession>> {
        let Some(agent_id) = Self::session_agent_id(path) else {
            return Ok(None);
        };
        let lines = crate::adapters::read_jsonl_lines(path)?;
        let mut header_id: Option<String> = None;
        let mut cwd: Option<String> = None;
        let mut started_at: Option<String> = None;
        let mut last_ts: Option<String> = None;
        let mut first_user_text: Option<String> = None;

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
                Some("message") if first_user_text.is_none() => {
                    let msg = v.get("message").unwrap_or(&Value::Null);
                    if msg.get("role").and_then(|r| r.as_str()) == Some("user") {
                        let text = content_text(msg.get("content").unwrap_or(&Value::Null));
                        // `<…>` environment blocks and `#`-prefixed injections
                        // are not user text.
                        if !text.is_empty() && !text.starts_with('<') && !text.starts_with('#') {
                            first_user_text = Some(crate::adapters::truncate_text(&text, 400));
                        }
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

        Ok(Some(DiscoveredSession {
            agent: Agent::AutoClaw,
            // The agent id is part of the identity: AutoClaw keys its own
            // sessions as `agent:<id>:<suffix>`, and two of its agents are
            // separate stores that could otherwise hand us the same uuid.
            agent_session_id: format!("{agent_id}:{id}"),
            path: path.to_path_buf(),
            cwd,
            started_at,
            last_activity_at: last_activity.or(last_ts),
            first_user_text,
            parent_agent_session_id: None,
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

    fn discover_sessions_in(
        &self,
        roots: &[PathBuf],
        unchanged: &dyn Fn(&Path) -> bool,
    ) -> Result<Vec<DiscoveredSession>> {
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
                match Self::parse_session_file(&p) {
                    Ok(Some(s)) => out.push(s),
                    Ok(None) => {}
                    Err(e) => eprintln!("[discover] skip {}: {}", p.display(), e),
                }
            }
        }
        Ok(out)
    }

    fn read_delta(&self, session: &Session, cursor: &SourceCursor) -> Result<ReadDelta> {
        let path = PathBuf::from(&session.raw_path);
        read_jsonl_delta(&path, cursor, &|_idx, v| {
            let vtype = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let source_event_id = v.get("id").and_then(|s| s.as_str()).map(|s| s.to_string());

            let (kind, text) = match vtype {
                "message" => {
                    let msg = v.get("message").unwrap_or(&Value::Null);
                    let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
                    let text = content_text(msg.get("content").unwrap_or(&Value::Null));
                    if text.trim().is_empty() {
                        return None;
                    }
                    match role {
                        "user" => ("user_message", text),
                        "assistant" => ("assistant_message", text),
                        // Tool output is a `toolResult` message here; not
                        // ingested (方案 §36.11).
                        "toolResult" | "tool_result" => return None,
                        _ => ("system", text),
                    }
                }
                "compaction" | "compact" => ("compact", "conversation compacted".into()),
                // `session` / `model_change` / `thinking_level_change` /
                // `custom` are bookkeeping, not conversation.
                _ => return None,
            };

            Some(ParsedLine {
                kind: kind.into(),
                text: Some(text),
                source_event_id,
                metadata: serde_json::json!({ "agent": "autoclaw", "type": vtype }),
            })
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::AgentAdapter;

    fn unique_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("noending-autoclaw-{}-{}", tag, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
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
            .discover_sessions_in(&[root], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 1, "one transcript: {found:#?}");
        let s = &found[0];
        assert_eq!(s.agent, Agent::AutoClaw);
        assert_eq!(
            s.agent_session_id,
            "auto-coder:4a6255bc-612b-494b-9e7e-3c5d7e58f088"
        );
        assert_eq!(s.cwd.as_deref(), Some("/Users/jqk/WorkBuddy/x"));
        assert_eq!(s.started_at.as_deref(), Some("2026-09-18T12:40:00.000Z"));
        assert_eq!(s.first_user_text.as_deref(), Some("把日报整理一下"));
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

    #[test]
    fn ingest_keeps_the_conversation_and_drops_the_tool_traffic() {
        let dir = unique_dir("parse");
        let sessions = dir.join("agents").join("auto-coder").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let file = sessions.join("s.jsonl");
        std::fs::write(&file, session_lines()).unwrap();

        let session = Session {
            id: "sess-1".into(),
            agent: Agent::AutoClaw,
            agent_session_id: "auto-coder:s".into(),
            title: None,
            cwd: None,
            workspace_path_id: None,
            project_id: None,
            raw_path: file.to_string_lossy().to_string(),
            parent_agent_session_id: None,
            started_at: None,
            last_activity_at: None,
            trashed_at: None,
        };
        let delta = AutoClawAdapter
            .read_delta(&session, &SourceCursor::default())
            .unwrap();
        let kinds: Vec<(&str, &str)> = delta
            .events
            .iter()
            .map(|e| (e.kind.as_str(), e.text.as_deref().unwrap_or("")))
            .collect();

        assert_eq!(
            kinds,
            vec![
                ("user_message", "把日报整理一下"),
                // The thinking/toolCall-only turn yields no text and is skipped.
                ("assistant_message", "日报已整理好。"),
            ],
            "got {kinds:?}"
        );
        assert_eq!(
            delta.events[0].source_event_id.as_deref(),
            Some("m1"),
            "identity comes from the entry id"
        );
    }
}
