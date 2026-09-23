//! WorkBuddy Adapter: `~/.workbuddy/projects/<slug>/<sessionId>.jsonl`.
//!
//! An Electron app (Tencent's WorkBuddy) whose conversation store is
//! append-only JSONL without a session header: the first line is already a
//! message, and the session's facts (`sessionId`, `cwd`) repeat on every line.
//! Line types measured on the three real files (712 lines total): `message`
//! (95; role `user` / `assistant`, content blocks typed `input_text` /
//! `output_text`), `function_call` / `function_call_result` (237 each),
//! `reasoning` (101), `file-history-snapshot` (35) and `ai-title` (7) — machine
//! traffic that is not ingested (方案 §36.11).
//!
//! The one shape that needs real work is the user turn. WorkBuddy wraps every
//! human turn in a `<system-reminder data-role="user-context">` envelope and
//! puts the prompt in `<user_query>…</user_query>` at the very end — measured
//! on all three sessions, where the bare opening message is a 9 KB identity
//! preamble (`SOUL.md` and friends) that no human typed. Three consequences:
//! - the human text is the `<user_query>` body, not the envelope;
//! - the compaction replays (`<conversation_history_summary>`, `<cb_summary>`)
//!   are also `user`-role messages but no human wrote them → `compact`, and
//!   only a marker is kept, never the tens of KB of machine summary;
//! - anything else that starts with `<` is system noise → dropped.
//!
//! Two things this adapter deliberately does not use:
//! - `ai-title` carries the app's own AI-generated title (`aiTitle`). NoEnding's
//!   session title is derived from the first user message and is write-once, so
//!   folding this in would be a separate decision, not a parsing detail.
//! - `function_call` / `function_call_result` have `name` / `arguments` /
//!   `output`; per §36.11 they are dropped rather than folded into the text.
//!
//! There is no CLI (the bundle's only executable is Electron), so this adapter
//! ingests history only: `detect()` never succeeds and no command is built.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::{
    detect_format, ms_epoch_to_rfc3339, read_jsonl_delta, AgentCommand, DiscoveredSession,
    ExecOptions, ParsedLine, ReadDelta,
};
use crate::domain::{Agent, Session, SourceCursor};
use crate::error::{other, Result};

pub struct WorkBuddyAdapter;

/// Text of a message's content blocks: `input_text` / `output_text` only.
fn content_text(content: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(arr) = content.as_array() {
        for item in arr {
            if matches!(
                item.get("type").and_then(|t| t.as_str()),
                Some("input_text") | Some("output_text")
            ) {
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

fn timestamp_of(v: &Value) -> Option<String> {
    v.get("timestamp")
        .and_then(|t| t.as_i64())
        .and_then(ms_epoch_to_rfc3339)
}

/// The human prompt inside WorkBuddy's user-turn envelope, if there is one.
fn extract_user_query(text: &str) -> Option<String> {
    const OPEN: &str = "<user_query>";
    const CLOSE: &str = "</user_query>";
    let rest = &text[text.find(OPEN)? + OPEN.len()..];
    let inner = match rest.find(CLOSE) {
        Some(end) => &rest[..end],
        None => rest,
    };
    let inner = inner.trim();
    (!inner.is_empty()).then(|| inner.to_string())
}

/// The compaction replays that arrive as `user`-role messages but are machine
/// context, not a turn.
fn is_compaction_block(text: &str) -> bool {
    text.starts_with("<conversation_history_summary") || text.starts_with("<cb_summary")
}

/// What a `user`-role message really is (see the module doc).
enum UserTurn {
    Prompt(String),
    Compaction,
    Envelope,
}

fn classify_user_turn(raw: &str) -> UserTurn {
    // Compaction first: a replay can quote the user's words (and even a
    // `<user_query>` tag) inside its own block, and it is still not a turn.
    if is_compaction_block(raw) {
        return UserTurn::Compaction;
    }
    if let Some(prompt) = extract_user_query(raw) {
        return UserTurn::Prompt(prompt);
    }
    if crate::adapters::is_injected_preamble(raw) {
        return UserTurn::Envelope;
    }
    UserTurn::Prompt(raw.to_string())
}

impl WorkBuddyAdapter {
    fn parse_session_file(path: &Path) -> Result<Option<DiscoveredSession>> {
        let file_name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .ok_or_else(|| other("无效的 session 文件名"))?;

        let lines = crate::adapters::read_jsonl_lines(path)?;
        let mut session_id: Option<String> = None;
        let mut cwd: Option<String> = None;
        let mut started_at: Option<String> = None;
        let mut last_ts: Option<String> = None;
        let mut native_title = None;
        let mut first_user_text = None;
        let mut first_agent_text = None;

        for (_, line) in &lines {
            let v: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if session_id.is_none() {
                session_id = v
                    .get("sessionId")
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string());
            }
            if cwd.is_none() {
                cwd = v
                    .get("cwd")
                    .and_then(|c| c.as_str())
                    .filter(|c| !c.is_empty())
                    .map(|c| c.to_string());
            }
            if let Some(ts) = timestamp_of(&v) {
                if started_at.is_none() {
                    started_at = Some(ts.clone());
                }
                last_ts = Some(ts);
            }
            // WorkBuddy writes its own `ai-title` and REWRITES it as the
            // conversation moves on (one session here carries four, drifting
            // from 「通达信连接功能介绍」 to 「分析国轩高科股票」). The last one
            // written is the Agent's final word on what the session is about,
            // so it is the one worth showing (§37.15).
            if v.get("type").and_then(|t| t.as_str()) == Some("ai-title") {
                if let Some(t) = v
                    .get("aiTitle")
                    .and_then(|t| t.as_str())
                    .filter(|t| !t.trim().is_empty())
                {
                    native_title = Some(t.to_string());
                }
            }
            if v.get("type").and_then(|t| t.as_str()) == Some("message") {
                let role = v.get("role").and_then(|r| r.as_str()).unwrap_or("");
                if first_user_text.is_none() && role == "user" {
                    let raw = content_text(v.get("content").unwrap_or(&Value::Null));
                    if let UserTurn::Prompt(text) = classify_user_turn(&raw) {
                        first_user_text = Some(crate::adapters::truncate_text(&text, 400));
                    }
                }
                // Last resort for a title (§37.15).
                if first_agent_text.is_none() && role == "assistant" {
                    let raw = content_text(v.get("content").unwrap_or(&Value::Null));
                    if !raw.trim().is_empty() {
                        first_agent_text = Some(crate::adapters::truncate_text(&raw, 400));
                    }
                }
            }
        }

        let meta = std::fs::metadata(path)?;
        let last_activity = meta
            .modified()
            .ok()
            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());

        Ok(Some(DiscoveredSession {
            agent: Agent::WorkBuddy,
            agent_session_id: session_id.unwrap_or(file_name),
            path: path.to_path_buf(),
            cwd,
            started_at,
            last_activity_at: last_activity.or(last_ts),
            native_title,
            first_user_text,
            first_agent_text,
            parent_agent_session_id: None,
        }))
    }
}

impl crate::adapters::AgentAdapter for WorkBuddyAdapter {
    fn agent(&self) -> Agent {
        Agent::WorkBuddy
    }

    /// Always `None`: an Electron GUI with no CLI to run.
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
                if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                if unchanged(&p) {
                    continue;
                }
                if detect_format(&p) != Some(Agent::WorkBuddy) {
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
            if v.get("type").and_then(|t| t.as_str()) != Some("message") {
                return None;
            }
            let source_event_id = v.get("id").and_then(|s| s.as_str()).map(|s| s.to_string());
            let raw = content_text(v.get("content").unwrap_or(&Value::Null));
            if raw.trim().is_empty() {
                return None;
            }
            let (kind, text) = match v.get("role").and_then(|r| r.as_str()).unwrap_or("") {
                "user" => match classify_user_turn(&raw) {
                    UserTurn::Prompt(p) => ("user_message", p),
                    UserTurn::Compaction => ("compact", "conversation compacted".into()),
                    UserTurn::Envelope => return None,
                },
                "assistant" => ("assistant_message", raw),
                _ => ("system", raw),
            };
            Some(ParsedLine {
                kind: kind.into(),
                text: Some(text),
                source_event_id,
                metadata: serde_json::json!({ "agent": "workbuddy", "type": "message" }),
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
        Err(other("WorkBuddy 是 GUI 应用，没有可启动的 CLI"))
    }

    fn build_resume_command(
        &self,
        _install: &crate::platform::exec_resolver::AgentInstallation,
        _opts: &ExecOptions,
        _agent_session_id: &str,
        _context_file: Option<&Path>,
        _cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        Err(other("WorkBuddy 是 GUI 应用，没有可启动的 CLI"))
    }

    fn build_exec_command(
        &self,
        _install: &crate::platform::exec_resolver::AgentInstallation,
        _opts: &ExecOptions,
        _prompt: &str,
    ) -> Result<AgentCommand> {
        Err(other("WorkBuddy 是 GUI 应用，没有可启动的 CLI"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::AgentAdapter;

    fn unique_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("noending-workbuddy-{}-{}", tag, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The shape measured on the real files: an enveloped opener whose prompt
    /// is buried in `<user_query>`, then a compaction replay, then a real turn.
    fn session_lines() -> String {
        [
            r#"{"id":"m1","timestamp":1783137449113,"type":"message","role":"user","content":[{"type":"input_text","text":"<system-reminder data-role=\"user-context\">\n<user_info>\nOS Version: darwin\n</user_info>\n</system-reminder>\n<user_query>整理一下昨天的会议纪要</user_query>"}],"providerData":{"provider":"zhipu"},"sessionId":"s1","cwd":"/Users/jqk/Workbuddy/x"}"#,
            r#"{"timestamp":1783137449152,"type":"file-history-snapshot","isSnapshotUpdate":false,"snapshot":{},"sessionId":"s1","cwd":"/Users/jqk/Workbuddy/x"}"#,
            r#"{"timestamp":1783137451207,"type":"ai-title","aiTitle":"整理会议纪要","sessionId":"s1","cwd":"/Users/jqk/Workbuddy/x"}"#,
            r#"{"id":"c1","timestamp":1783137453100,"type":"message","role":"user","content":[{"type":"input_text","text":"<cb_summary>Summary of the conversation so far: 很长的一段机器摘要</cb_summary>"}],"sessionId":"s1","cwd":"/Users/jqk/Workbuddy/x"}"#,
            r#"{"id":"r1","parentId":"m1","timestamp":1783137455212,"type":"reasoning","content":[{"type":"reasoning_text","text":"想一下"}],"rawContent":[{"type":"reasoning_text","text":"想一下"}],"sessionId":"s1","cwd":"/Users/jqk/Workbuddy/x"}"#,
            r#"{"id":"m2","parentId":"r1","timestamp":1783137455216,"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"整理好了。"}],"providerData":{},"sessionId":"s1","cwd":"/Users/jqk/Workbuddy/x"}"#,
            r#"{"id":"m2","parentId":"r1","timestamp":1783137455225,"type":"function_call","callId":"call_1","name":"WebFetch","arguments":"{}","providerData":{},"sessionId":"s1","cwd":"/Users/jqk/Workbuddy/x"}"#,
            r#"{"id":"r2","parentId":"m2","timestamp":1783137455299,"type":"function_call_result","callId":"call_1","name":"WebFetch","status":"completed","output":{"type":"text","text":"page"},"sessionId":"s1","cwd":"/Users/jqk/Workbuddy/x"}"#,
        ]
        .join("\n")
            + "\n"
    }

    #[test]
    fn discovery_reads_session_facts_and_the_first_real_prompt() {
        let dir = unique_dir("parse");
        let file = dir.join("s1.jsonl");
        std::fs::write(&file, session_lines()).unwrap();

        let found = WorkBuddyAdapter
            .discover_sessions_in(&[dir], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 1, "{found:#?}");
        let s = &found[0];
        assert_eq!(s.agent, Agent::WorkBuddy);
        assert_eq!(s.agent_session_id, "s1");
        assert_eq!(s.cwd.as_deref(), Some("/Users/jqk/Workbuddy/x"));
        assert_eq!(s.first_user_text.as_deref(), Some("整理一下昨天的会议纪要"));
        assert_eq!(
            s.native_title.as_deref(),
            Some("整理会议纪要"),
            "WorkBuddy titles the session itself"
        );
        // Epoch millis are normalized to the spelling every other agent uses.
        assert_eq!(
            s.started_at.as_deref(),
            Some("2026-07-04T03:57:29.113+00:00")
        );
    }

    /// WorkBuddy rewrites its `ai-title` as the conversation moves on — one real
    /// session here carries four, drifting from 「通达信连接功能介绍」 to
    /// 「分析国轩高科股票」. The last one written is the current one (§37.15).
    #[test]
    fn the_last_ai_title_wins() {
        let dir = unique_dir("ai-title");
        let file = dir.join("s1.jsonl");
        std::fs::write(
            &file,
            concat!(
                r#"{"timestamp":1783137449113,"type":"message","role":"user","content":[{"type":"input_text","text":"<user_query>先看看通达信</user_query>"}],"sessionId":"s1","cwd":"/tmp/w"}"#,
                "\n",
                r#"{"timestamp":1783137451207,"type":"ai-title","aiTitle":"通达信连接功能介绍","sessionId":"s1"}"#,
                "\n",
                r#"{"timestamp":1783259041800,"type":"ai-title","aiTitle":"分析国轩高科股票","sessionId":"s1"}"#,
                "\n",
            ),
        )
        .unwrap();

        let found = WorkBuddyAdapter
            .discover_sessions_in(&[dir.clone()], &|_| false)
            .unwrap();
        assert_eq!(found[0].native_title.as_deref(), Some("分析国轩高科股票"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ingest_keeps_prose_and_drops_reasoning_and_tool_traffic() {
        let dir = unique_dir("delta");
        let file = dir.join("s1.jsonl");
        std::fs::write(&file, session_lines()).unwrap();

        let session = Session {
            id: "sess-1".into(),
            agent: Agent::WorkBuddy,
            agent_session_id: "s1".into(),
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
        let delta = WorkBuddyAdapter
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
                // the envelope is stripped to the prompt, not ingested whole
                ("user_message", "整理一下昨天的会议纪要"),
                ("compact", "conversation compacted"),
                ("assistant_message", "整理好了。"),
            ],
            "got {kinds:?}"
        );
        // The event timestamp survives the millis → RFC3339 normalization.
        assert_eq!(
            delta.events[0].ts.as_deref(),
            Some("2026-07-04T03:57:29.113+00:00")
        );
    }

    /// The user turn is the app's envelope, not the human's words (§37.7).
    #[test]
    fn the_envelope_is_not_the_user_turn() {
        let enveloped = "<system-reminder data-role=\"user-context\">\n<user_info>\nOS Version: darwin\n</user_info>\n</system-reminder>\n<user_query>看一下这只股票</user_query>";
        assert!(matches!(
            classify_user_turn(enveloped),
            UserTurn::Prompt(p) if p == "看一下这只股票"
        ));
        // A `user`-role compaction replay is machine context, not a turn.
        assert!(matches!(
            classify_user_turn("<cb_summary>Summary of the conversation so far: …</cb_summary>"),
            UserTurn::Compaction
        ));
        assert!(matches!(
            classify_user_turn("<conversation_history_summary>x</conversation_history_summary>"),
            UserTurn::Compaction
        ));
        // A bare envelope with no prompt inside carries nothing to ingest.
        assert!(matches!(
            classify_user_turn(
                "<system-reminder data-role=\"user-context\">only context</system-reminder>"
            ),
            UserTurn::Envelope
        ));
        // Plain prose is a prompt, and so is a continuation nudge.
        assert!(matches!(
            classify_user_turn("帮我看看这个"),
            UserTurn::Prompt(p) if p == "帮我看看这个"
        ));
    }

    #[test]
    fn another_agents_transcript_is_not_claimed() {
        let dir = unique_dir("foreign");
        // A pi-shaped file (which also uses sessionId-free headers) and a
        // Claude-shaped one must both be refused.
        std::fs::write(
            dir.join("pi.jsonl"),
            "{\"type\":\"session\",\"version\":3,\"id\":\"p1\",\"cwd\":\"/x\"}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("claude.jsonl"),
            "{\"type\":\"user\",\"sessionId\":\"c1\",\"uuid\":\"u1\",\"cwd\":\"/x\",\"message\":{\"role\":\"user\",\"content\":[]}}\n",
        )
        .unwrap();
        let found = WorkBuddyAdapter
            .discover_sessions_in(&[dir], &|_| false)
            .unwrap();
        assert!(found.is_empty(), "{found:#?}");
    }
}
