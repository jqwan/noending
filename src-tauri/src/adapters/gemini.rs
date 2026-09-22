//! Gemini CLI Adapter: `~/.gemini/tmp/<project>/chats/session-*.jsonl`.
//!
//! The transcript is **an operation log, not a message list**: the file is
//! appended to (`fs.appendFileSync`) and replayed in order to reach the current
//! conversation (`packages/core/src/services/chatRecordingService.ts`,
//! `loadConversationRecord`). Four record kinds exist and only the last of them
//! is what most adapters see:
//! - plain metadata: `{sessionId, projectHash, startTime, lastUpdated, kind}`;
//! - a plain message record `{id, timestamp, type, content, …}` → upsert by id;
//! - `{$set: {…}}` — with `messages: […]` this is a **checkpoint**: the
//!   conversation is cleared and rebuilt from that array; other keys merge into
//!   the metadata;
//! - `{$rewindTo: <message id>}` → drop that message and everything after it.
//!
//! Two consequences shape this adapter:
//! - a checkpoint carries many messages in ONE line, which the shared
//!   one-event-per-line reader cannot express — hence a bespoke reader that
//!   replays the log and emits the conversation it ends on;
//! - the metadata `sessionId` is **not usable as identity**: the A2A server
//!   writes the literal `"a2a-server"` into every recording it makes (measured:
//!   all ten local files), so the id comes from the file name, exactly as it
//!   does for Qoder's sub-agent transcripts. The name is
//!   `session-<timestamp>-<id[:8]>.jsonl`, i.e. unique per file.
//!
//! Only `user` and `gemini` messages are ingested; `info` / `error` / `warning`
//! are UI notices, and tool traffic lives in `toolCalls` / `thoughts`, which are
//! machine chatter (§36.11). The workspace the session ran in is a fact on
//! disk — the `<project>/.project_root` marker beside `chats/` — not something
//! to infer from the hash-named directory.
//!
//! There is no launchable CLI here: the machine has no `gemini` (方案 §37.3
//! rule 1), and even where one exists `--resume` only names `latest` or an
//! index, never a session, so NoEnding's resume-by-id flow cannot be expressed.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::{
    replay_cursor_update, AgentCommand, DiscoveredSession, ExecOptions, ParsedEvent, ReadDelta,
};
use crate::domain::{Agent, Session, SourceCursor};
use crate::error::{other, Result};
use crate::platform::exec_resolver::AgentInstallation;

pub struct GeminiAdapter;

/// Message types that are conversation. `info` / `error` / `warning` are the
/// CLI's own notices.
fn kind_of(message_type: &str) -> Option<&'static str> {
    match message_type {
        "user" => Some("user_message"),
        "gemini" => Some("assistant_message"),
        _ => None,
    }
}

/// `content` is `PartListUnion`: a bare string, one part, or part[]. Only text
/// parts are prose; function calls and responses are tool traffic.
fn content_text(content: &Value) -> String {
    let part = |p: &Value| -> Option<String> {
        p.get("text")
            .and_then(|t| t.as_str())
            .map(|t| t.to_string())
    };
    match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().filter_map(part).collect::<Vec<_>>().join("\n"),
        Value::Object(_) => part(content).unwrap_or_default(),
        _ => String::new(),
    }
}

/// The CLI's startup context ("<session_context>…") is a `user` message but
/// not something the user wrote.
fn is_machine_context(text: &str) -> bool {
    text.starts_with('<')
}

/// The conversation a recording ends on, by replaying its log.
fn replay(text: &str) -> Vec<Value> {
    let mut messages: Vec<Value> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(rewind_to) = v.get("$rewindTo").and_then(|r| r.as_str()) {
            match messages
                .iter()
                .position(|m| m.get("id").and_then(|i| i.as_str()) == Some(rewind_to))
            {
                Some(index) => messages.truncate(index),
                // The loader clears everything when the anchor is gone; the
                // store is append-only either way, so this cannot lose history.
                None => messages.clear(),
            }
            continue;
        }
        if let Some(set) = v.get("$set") {
            if let Some(checkpoint) = set.get("messages").and_then(|m| m.as_array()) {
                messages = checkpoint.clone();
            }
            continue;
        }
        if let Some(id) = v.get("id").and_then(|i| i.as_str()) {
            match messages
                .iter()
                .position(|m| m.get("id").and_then(|i| i.as_str()) == Some(id))
            {
                Some(index) => messages[index] = v,
                None => messages.push(v),
            }
        }
    }
    messages
}

fn metadata_field(text: &str, field: &str) -> Option<String> {
    let mut found = None;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let holder = match v.get("$set") {
            Some(set) if set.get("messages").is_none() => set,
            Some(_) => continue,
            None if v.get("$rewindTo").is_none() && v.get("id").is_none() => &v,
            None => continue,
        };
        if let Some(value) = holder.get(field).and_then(|f| f.as_str()) {
            found = Some(value.to_string());
        }
    }
    found
}

/// The conversation as ingestable events, in order. The message id is the
/// native event id, so replaying the log never stores the same turn twice.
fn conversation_events(messages: &[Value]) -> Vec<ParsedEvent> {
    let mut out = Vec::new();
    for (index, msg) in messages.iter().enumerate() {
        let msg_type = msg.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let Some(kind) = kind_of(msg_type) else {
            continue;
        };
        let text = content_text(msg.get("content").unwrap_or(&Value::Null));
        if text.trim().is_empty() || (kind == "user_message" && is_machine_context(&text)) {
            continue;
        }
        out.push(ParsedEvent {
            source_event_id: msg
                .get("id")
                .and_then(|i| i.as_str())
                .map(|i| i.to_string()),
            source_position: format!("msg:{}", index + 1),
            ts: msg
                .get("timestamp")
                .and_then(|t| t.as_str())
                .map(|t| t.to_string()),
            kind: kind.into(),
            text: Some(text),
            metadata: serde_json::json!({ "agent": "gemini", "type": msg_type }),
        });
    }
    out
}

/// One session file per recording: `<project>/chats/session-*.jsonl` under
/// `<root>/tmp`. The rest of `~/.gemini` (the bundled IDE's brain, a git-backed
/// history) is never walked.
fn chat_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    roots
        .iter()
        .map(|root| {
            let tmp = root.join("tmp");
            if tmp.is_dir() {
                tmp
            } else {
                root.clone()
            }
        })
        .collect()
}

fn is_recording(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let in_chats = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        == Some("chats");
    in_chats && name.starts_with("session-") && name.ends_with(".jsonl")
}

/// Acceptance is by content, not by the name: a recording opens with the
/// metadata record the CLI writes before anything else.
fn is_metadata_record(v: &Value) -> bool {
    v.get("sessionId").is_some() && v.get("projectHash").is_some() && v.get("startTime").is_some()
}

impl GeminiAdapter {
    fn parse_session_file(path: &Path) -> Result<Option<DiscoveredSession>> {
        let text = std::fs::read_to_string(path)?;
        let first = text
            .lines()
            .find(|l| !l.trim().is_empty())
            .and_then(|l| serde_json::from_str::<Value>(l).ok());
        match first {
            Some(v) if is_metadata_record(&v) => {}
            _ => return Ok(None),
        }

        let messages = replay(&text);
        let first_user_text = conversation_events(&messages)
            .into_iter()
            .find(|e| e.kind == "user_message")
            .and_then(|e| e.text)
            .map(|t| crate::adapters::truncate_text(&t, 400));

        // The workspace the recording was made in: `<project>/chats/…` sits
        // beside a `.project_root` marker holding the absolute path.
        let cwd = path
            .parent()
            .and_then(|chats| chats.parent())
            .map(|project| project.join(".project_root"))
            .filter(|marker| marker.is_file())
            .and_then(|marker| std::fs::read_to_string(marker).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let id = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .ok_or_else(|| other("无效的 Gemini session 文件名"))?;
        let meta = std::fs::metadata(path)?;
        let last_activity = meta
            .modified()
            .ok()
            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());

        Ok(Some(DiscoveredSession {
            agent: Agent::Gemini,
            agent_session_id: id,
            path: path.to_path_buf(),
            cwd,
            started_at: metadata_field(&text, "startTime"),
            last_activity_at: last_activity,
            first_user_text,
            // `kind: "subagent"` exists, but the record names no parent — so
            // there is no link to record rather than a guessed one.
            parent_agent_session_id: None,
        }))
    }
}

impl crate::adapters::AgentAdapter for GeminiAdapter {
    fn agent(&self) -> Agent {
        Agent::Gemini
    }

    /// Always `None`: this machine has no `gemini` binary, and `--resume` names
    /// `latest`/an index rather than a session anyway (方案 §37.9).
    fn detect(&self) -> Option<AgentInstallation> {
        None
    }

    fn discover_sessions_in(
        &self,
        roots: &[PathBuf],
        unchanged: &dyn Fn(&Path) -> bool,
    ) -> Result<Vec<DiscoveredSession>> {
        let mut out = Vec::new();
        let mut stack: Vec<PathBuf> = chat_roots(roots);
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
                if !is_recording(&p) || unchanged(&p) {
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
        let raw = std::fs::read(&path)?;
        let text = String::from_utf8_lossy(&raw);

        Ok(ReadDelta {
            events: conversation_events(&replay(&text)),
            source: Some(replay_cursor_update(&path, cursor, &raw)?),
        })
    }

    fn build_new_command(
        &self,
        _install: &AgentInstallation,
        _opts: &ExecOptions,
        _context_file: Option<&Path>,
        _cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        Err(other("Gemini CLI 未安装，NoEnding 只读取它的历史会话"))
    }

    fn build_resume_command(
        &self,
        _install: &AgentInstallation,
        _opts: &ExecOptions,
        _agent_session_id: &str,
        _context_file: Option<&Path>,
        _cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        Err(other(
            "Gemini CLI 的 --resume 只接受 latest 或序号，不能用会话 id 恢复",
        ))
    }

    fn build_exec_command(
        &self,
        _install: &AgentInstallation,
        _opts: &ExecOptions,
        _prompt: &str,
    ) -> Result<AgentCommand> {
        Err(other("Gemini CLI 未安装，NoEnding 只读取它的历史会话"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::AgentAdapter;

    fn unique_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "noending-gemini-{}-{}-{}",
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

    /// A project temp dir as the CLI lays it out: `<root>/tmp/<project>/chats/`
    /// beside a `.project_root` marker.
    fn recording(root: &Path, project: &str, name: &str, body: &str) -> PathBuf {
        let chats = root.join("tmp").join(project).join("chats");
        std::fs::create_dir_all(&chats).unwrap();
        std::fs::write(
            root.join("tmp").join(project).join(".project_root"),
            "/repo/here",
        )
        .unwrap();
        let path = chats.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn metadata(session_id: &str) -> String {
        format!(
            r#"{{"sessionId":"{session_id}","projectHash":"303dd790","startTime":"2026-09-14T15:20:39.089Z","lastUpdated":"2026-09-14T15:20:39.089Z","kind":"main"}}"#
        )
    }

    fn message(id: &str, kind: &str, text: &str) -> String {
        format!(
            r#"{{"id":"{id}","timestamp":"2026-09-14T15:20:39.089Z","type":"{kind}","content":[{{"text":"{text}"}}]}}"#
        )
    }

    fn checkpoint(messages: &[String]) -> String {
        format!(r#"{{"$set":{{"messages":[{}]}}}}"#, messages.join(","))
    }

    fn session_of(path: &Path, id: &str) -> Session {
        Session {
            id: "sess-gemini".into(),
            agent: Agent::Gemini,
            agent_session_id: id.into(),
            title: None,
            cwd: None,
            workspace_path_id: None,
            project_id: None,
            raw_path: path.to_string_lossy().to_string(),
            parent_agent_session_id: None,
            started_at: None,
            last_activity_at: None,
            trashed_at: None,
        }
    }

    #[test]
    fn discovery_reads_the_workspace_marker_and_the_first_real_prompt() {
        let root = unique_dir("parse");
        let body = format!(
            "{}\n{}\n",
            metadata("a2a-server"),
            checkpoint(&[
                message(
                    "m1",
                    "user",
                    "<session_context>This is the Gemini CLI.</session_context>"
                ),
                message("m2", "user", "帮我看下这个仓库"),
                message("m3", "gemini", "好的。"),
            ])
        );
        recording(
            &root,
            "noending",
            "session-2026-09-14T15-20-a2a-serv.jsonl",
            &body,
        );

        let found = GeminiAdapter
            .discover_sessions_in(&[root], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 1, "{found:#?}");
        let s = &found[0];
        assert_eq!(s.agent, Agent::Gemini);
        // Identity is the file, not the metadata: every A2A recording says
        // `sessionId: "a2a-server"`, so trusting it would merge them all.
        assert_eq!(s.agent_session_id, "session-2026-09-14T15-20-a2a-serv");
        // The workspace is a marker file beside `chats/`, not a guess.
        assert_eq!(s.cwd.as_deref(), Some("/repo/here"));
        assert_eq!(s.started_at.as_deref(), Some("2026-09-14T15:20:39.089Z"));
        assert_eq!(s.first_user_text.as_deref(), Some("帮我看下这个仓库"));
    }

    #[test]
    fn the_log_is_replayed_to_the_conversation_it_ends_on() {
        let dir = unique_dir("replay");
        let path = dir.join("session-x.jsonl");
        let body = format!(
            "{}\n{}\n{}\n{}\n",
            metadata("a2a-server"),
            // message records arrive one per line in the normal writer…
            message("m1", "user", "第一句"),
            message("m2", "gemini", "回应一"),
            // …and a checkpoint replaces the whole conversation, then a rewind
            // trims it back.
            checkpoint(&[
                message("m1", "user", "第一句"),
                message("m2", "gemini", "回应一"),
                message("m3", "user", "第二句"),
            ]),
        );
        std::fs::write(&path, &body).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let events = conversation_events(&replay(&text));
        let texts: Vec<&str> = events
            .iter()
            .map(|e| e.text.as_deref().unwrap_or(""))
            .collect();
        assert_eq!(texts, vec!["第一句", "回应一", "第二句"]);
        assert_eq!(
            events[2].source_event_id.as_deref(),
            Some("m3"),
            "the message id is the native event id"
        );

        // A rewind removes the anchor and everything after it, i.e. it restores
        // the state just BEFORE that message.
        std::fs::write(
            &path,
            format!("{}\n{}\n", body.trim_end(), r#"{"$rewindTo":"m3"}"#),
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let texts: Vec<String> = conversation_events(&replay(&text))
            .into_iter()
            .map(|e| e.text.unwrap_or_default())
            .collect();
        assert_eq!(texts, vec!["第一句", "回应一"], "m3 is gone from the state");
    }

    #[test]
    fn only_conversation_is_ingested() {
        let root = unique_dir("kinds");
        let body = format!(
            "{}\n{}\n",
            metadata("a2a-server"),
            checkpoint(&[
                message("i1", "info", "Using model gemini-3-pro"),
                message("w1", "warning", "Approval required"),
                message("e1", "error", "Tool failed"),
                message("m1", "user", "真正的提问"),
                message("m2", "gemini", "回答"),
            ])
        );
        let path = recording(&root, "p", "session-kinds.jsonl", &body);
        let delta = GeminiAdapter
            .read_delta(
                &session_of(&path, "session-kinds"),
                &SourceCursor::default(),
            )
            .unwrap();
        let kinds: Vec<(&str, &str)> = delta
            .events
            .iter()
            .map(|e| (e.kind.as_str(), e.text.as_deref().unwrap_or("")))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("user_message", "真正的提问"),
                ("assistant_message", "回答")
            ],
            "got {kinds:?}"
        );
    }

    #[test]
    fn two_recordings_of_the_placeholder_session_stay_distinct() {
        let root = unique_dir("placeholder");
        let body = format!(
            "{}\n{}\n",
            metadata("a2a-server"),
            checkpoint(&[message("m1", "user", "第一次")])
        );
        recording(&root, "p", "session-2026-09-14T15-19-a2a-serv.jsonl", &body);
        recording(&root, "p", "session-2026-09-14T15-20-a2a-serv.jsonl", &body);

        let mut found = GeminiAdapter
            .discover_sessions_in(&[root], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 2, "{found:#?}");
        found.sort_by(|a, b| a.agent_session_id.cmp(&b.agent_session_id));
        assert_ne!(found[0].agent_session_id, found[1].agent_session_id);
        assert!(found
            .iter()
            .all(|s| s.agent_session_id.starts_with("session-")));
    }

    #[test]
    fn files_that_are_not_recordings_are_not_claimed() {
        let root = unique_dir("foreign");
        // A `session-*.jsonl` that does not sit in a `chats/` directory (the
        // bundled IDE's transcript lives in one).
        let stray = root.join("brain").join("abc");
        std::fs::create_dir_all(&stray).unwrap();
        std::fs::write(
            stray.join("session-transcript.jsonl"),
            format!("{}\n", message("m1", "user", "hi")),
        )
        .unwrap();
        // A file in `chats/` whose first record is not the CLI's metadata.
        let chats = root.join("tmp").join("p").join("chats");
        std::fs::create_dir_all(&chats).unwrap();
        std::fs::write(
            chats.join("session-not-gemini.jsonl"),
            format!("{}\n", message("m1", "user", "hi")),
        )
        .unwrap();

        let found = GeminiAdapter
            .discover_sessions_in(&[root], &|_| false)
            .unwrap();
        assert!(found.is_empty(), "{found:#?}");
    }
}
