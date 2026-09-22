//! Qoder CN Adapter: `~/.qoder-cn/projects/<encoded-cwd>/<session>.jsonl`.
//!
//! Qoder's transcript is **Claude Code's format plus Qoder-only lines**
//! (`workspace-directories`, `runtime-config`, `worktree-state`, `active-leaf`,
//! `last-prompt`). That shared shape is why `fingerprint_line` must claim Qoder
//! *before* the Claude branch: read as Claude, every line would still parse and
//! nothing would look wrong — which is exactly how provenance gets lost
//! (AGENTS.md: Provenance Fidelity).
//!
//! **Sub-agent transcripts are deliberately not ingested.** `<session>/subagents/
//! agent-*.jsonl` repeats the parent's `sessionId` on every line and carries no
//! Qoder line type until its final line — measured on a real file: 75 message
//! lines, then a single `last-prompt` at the end. Their content is therefore
//! indistinguishable from Claude Code's, and the rule here is "宁可漏判不可错判"
//! (方案 §37.3): rather than guess a marker that might also appear in Claude's
//! files — which would silently re-label Claude sessions as Qoder — they are
//! left unclaimed. See §37.5 for what it would take to include them.
//!
//! Qoder is an IDE with no headless CLI (`~/.qoder-cn/bin` holds only
//! `qoder-cn-computer-use`, and the entry dispatcher looks for a `qoderclicn`
//! that does not exist), so this adapter **ingests history only**: `detect()`
//! never succeeds, and there is no new/resume/exec command to build. The raw
//! transcripts are opened read-only, always.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::{
    detect_format, read_jsonl_delta, AgentCommand, DiscoveredSession, ExecOptions, ParsedLine,
    ReadDelta,
};
use crate::domain::{Agent, Session, SourceCursor};
use crate::error::{other, Result};

pub struct QoderAdapter;

/// Text of a message's content blocks. Only `type:"text"` counts: Qoder folds
/// tool results into `user`-role lines (`toolUseResult` + a `tool_result`
/// block), and tool traffic is deliberately not ingested (方案 §36.11).
fn content_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(arr) => {
            let mut parts = Vec::new();
            for item in arr {
                if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                        parts.push(t.to_string());
                    }
                }
            }
            parts.join("\n")
        }
        _ => String::new(),
    }
}

impl QoderAdapter {
    /// `<encoded-cwd>/<main>.jsonl` is a session; `<encoded-cwd>/<main>/subagents/
    /// agent-*.jsonl` is a sub-agent transcript that stays unclaimed (module doc).
    fn is_subagent_transcript(path: &Path) -> bool {
        path.parent()
            .and_then(|d| d.file_name())
            .and_then(|n| n.to_str())
            .map(|n| n == "subagents")
            .unwrap_or(false)
    }

    fn parse_session_file(path: &Path) -> Result<Option<DiscoveredSession>> {
        let file_name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .ok_or_else(|| other("无效的 session 文件名"))?;

        let lines = crate::adapters::read_jsonl_lines(path)?;
        let mut session_id: Option<String> = None;
        let mut cwd: Option<String> = None;
        let mut started_at = None;
        let mut last_ts = None;
        let mut first_user_text = None;

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
                // The line-level cwd is authoritative; the directory name is an
                // encoded (and therefore ambiguous) copy of it.
                cwd = v
                    .get("cwd")
                    .and_then(|c| c.as_str())
                    .filter(|c| !c.is_empty())
                    .map(|c| c.to_string());
            }
            // Only string timestamps: Qoder also writes `runtime-config` lines
            // whose `timestamp` is epoch millis, and mixing the two would make
            // started/last activity incomparable.
            if let Some(ts) = v.get("timestamp").and_then(|t| t.as_str()) {
                if started_at.is_none() {
                    started_at = Some(ts.to_string());
                }
                last_ts = Some(ts.to_string());
            }
            if first_user_text.is_none()
                && v.get("type").and_then(|t| t.as_str()) == Some("user")
                && v.get("isSidechain").and_then(|s| s.as_bool()) != Some(true)
            {
                // A user line carrying toolUseResult is a tool result, not a
                // prompt; its text blocks are dropped above, so it yields "".
                let text = v
                    .get("message")
                    .map(|m| content_text(m.get("content").unwrap_or(&Value::Null)))
                    .unwrap_or_default();
                // `<…>` environment blocks and `#`-prefixed injections
                // (AGENTS.md, attached-file headers) are not user text.
                if !text.is_empty() && !text.starts_with('<') && !text.starts_with('#') {
                    first_user_text = Some(crate::adapters::truncate_text(&text, 400));
                }
            }
        }

        let meta = std::fs::metadata(path)?;
        let last_activity = meta
            .modified()
            .ok()
            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());

        let agent_session_id = session_id.unwrap_or_else(|| file_name.clone());

        Ok(Some(DiscoveredSession {
            agent: Agent::Qoder,
            agent_session_id,
            path: path.to_path_buf(),
            cwd,
            started_at,
            last_activity_at: last_activity.or(last_ts),
            first_user_text,
            parent_agent_session_id: None,
        }))
    }
}

impl crate::adapters::AgentAdapter for QoderAdapter {
    fn agent(&self) -> Agent {
        Agent::Qoder
    }

    /// Always `None`: Qoder ships no CLI, so there is nothing to detect and
    /// nothing to launch. Ingestion does not consult this (方案 §37.3).
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
                    // `<sessionId>/subagents/…` and the diagnostics segments
                    // under `~/.qoder-cn/logs` are walked too — the fingerprint
                    // is what decides, not the path.
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                if Self::is_subagent_transcript(&p) {
                    // Ambiguous by content (module doc): claimed by nobody.
                    continue;
                }
                if unchanged(&p) {
                    continue;
                }
                if detect_format(&p) != Some(Agent::Qoder) {
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
            let sidechain = v
                .get("isSidechain")
                .and_then(|s| s.as_bool())
                .unwrap_or(false);
            let source_event_id = v
                .get("uuid")
                .and_then(|s| s.as_str())
                .map(|s| s.to_string());

            let (kind, text, extra) = match vtype {
                "user" | "assistant" => {
                    let msg = v.get("message").unwrap_or(&Value::Null);
                    let text = content_text(msg.get("content").unwrap_or(&Value::Null));
                    if text.is_empty() {
                        return None;
                    }
                    if sidechain {
                        ("system", text, serde_json::json!({ "sidechain": true }))
                    } else {
                        let kind = if vtype == "user" {
                            "user_message"
                        } else {
                            "assistant_message"
                        };
                        (kind, text, serde_json::json!({}))
                    }
                }
                // `attachment` lines are injected context (skill listings,
                // system reminders) — machine chatter, same category as the
                // tool events dropped in §36.11.
                _ => return None,
            };

            let mut meta = serde_json::Map::new();
            meta.insert("agent".into(), serde_json::Value::String("qoder".into()));
            meta.insert("type".into(), serde_json::Value::String(vtype.to_string()));
            if let Some(obj) = extra.as_object() {
                for (k, v2) in obj {
                    meta.insert(k.clone(), v2.clone());
                }
            }

            Some(ParsedLine {
                kind: kind.into(),
                text: Some(text),
                source_event_id,
                metadata: serde_json::Value::Object(meta),
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
        Err(other("Qoder 没有可启动的 CLI，无法新建会话"))
    }

    fn build_resume_command(
        &self,
        _install: &crate::platform::exec_resolver::AgentInstallation,
        _opts: &ExecOptions,
        _agent_session_id: &str,
        _context_file: Option<&Path>,
        _cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        Err(other("Qoder 没有可启动的 CLI，无法恢复会话"))
    }

    fn build_exec_command(
        &self,
        _install: &crate::platform::exec_resolver::AgentInstallation,
        _opts: &ExecOptions,
        _prompt: &str,
    ) -> Result<AgentCommand> {
        Err(other("Qoder 没有可启动的 CLI，无法一次性执行"))
    }

    /// Qoder source deletion (方案 §15): the source is the exact discovered
    /// `*.jsonl` at `raw_path`. The parser is the same one discovery uses, so
    /// a sub-agent file's derived id (parent + stem) has to match too — which
    /// it does, because the id comes from that parser.
    fn prepare_source_session_deletion(
        &self,
        session: &Session,
    ) -> Result<crate::adapters::SourceDeletionPlan> {
        crate::adapters::prepare_single_file_source_deletion(
            session,
            Agent::Qoder,
            "qoder_transcript",
            &|p| Ok(Self::parse_session_file(p)?.map(|d| d.agent_session_id)),
        )
    }

    fn execute_source_session_deletion(
        &self,
        plan: &crate::adapters::SourceDeletionPlan,
    ) -> Result<crate::adapters::SourceDeletionOutcome> {
        crate::adapters::execute_single_file_source_deletion(plan, Agent::Qoder, &|p| {
            Ok(Self::parse_session_file(p)?.map(|d| d.agent_session_id))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::AgentAdapter;

    fn unique_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("noending-qoder-{}-{}", tag, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn main_lines() -> String {
        // Bookkeeping + a real prompt + an attachment + a tool result folded
        // into a user-role line, in the shapes the real files use.
        [
            r#"{"type":"workspace-directories","sessionId":"s-main","directories":["/repo"]}"#,
            r#"{"type":"runtime-config","sessionId":"s-main","model":"dfmodel","timestamp":1790089288825}"#,
            r#"{"type":"user","uuid":"u1","timestamp":"2026-09-22T15:01:29.551Z","cwd":"/repo","sessionId":"s-main","message":{"role":"user","content":[{"type":"text","text":"把详情页的消息改一下"}]}}"#,
            r#"{"type":"attachment","uuid":"a1","timestamp":"2026-09-22T15:01:29.552Z","sessionId":"s-main","attachment":{"type":"skill_listing"}}"#,
            r#"{"type":"assistant","uuid":"u2","timestamp":"2026-09-22T15:01:31.000Z","cwd":"/repo","sessionId":"s-main","message":{"role":"assistant","content":[{"type":"text","text":"好，我先看现状。"},{"type":"tool_use","name":"Read"}]}}"#,
            r#"{"type":"user","uuid":"u3","timestamp":"2026-09-22T15:01:32.000Z","cwd":"/repo","sessionId":"s-main","toolUseResult":{"ok":true},"message":{"role":"user","content":[{"type":"tool_result","content":"file contents"}]}}"#,
            r#"{"type":"assistant","uuid":"u4","timestamp":"2026-09-22T15:01:40.000Z","isSidechain":true,"cwd":"/repo","sessionId":"s-main","message":{"role":"assistant","content":[{"type":"text","text":"子 Agent 的自述"}]}}"#,
        ]
        .join("\n")
            + "\n"
    }

    #[test]
    fn ingest_keeps_prose_and_drops_machine_chatter() {
        let dir = unique_dir("parse");
        let file = dir.join("s-main.jsonl");
        std::fs::write(&file, main_lines()).unwrap();

        let found = QoderAdapter
            .discover_sessions_in(&[dir.clone()], &|_| false)
            .unwrap();
        assert_eq!(
            found.len(),
            1,
            "one session file, recognized by fingerprint"
        );
        let s = &found[0];
        assert_eq!(s.agent, Agent::Qoder);
        assert_eq!(s.agent_session_id, "s-main");
        assert_eq!(s.cwd.as_deref(), Some("/repo"));
        assert_eq!(s.parent_agent_session_id, None);
        assert_eq!(
            s.started_at.as_deref(),
            Some("2026-09-22T15:01:29.551Z"),
            "epoch-millis bookkeeping timestamps must not win over ISO ones"
        );
        assert_eq!(s.first_user_text.as_deref(), Some("把详情页的消息改一下"));

        let session = Session {
            id: "sess-1".into(),
            agent: Agent::Qoder,
            agent_session_id: "s-main".into(),
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
        let cursor = SourceCursor::default();
        let delta = QoderAdapter.read_delta(&session, &cursor).unwrap();
        let kinds: Vec<(&str, &str)> = delta
            .events
            .iter()
            .map(|e| (e.kind.as_str(), e.text.as_deref().unwrap_or("")))
            .collect();

        // The prompt and the assistant's prose survive; the assistant's text is
        // kept without its tool_use block.
        assert!(kinds.contains(&("user_message", "把详情页的消息改一下")));
        assert!(kinds.contains(&("assistant_message", "好，我先看现状。")));
        // Attachment lines, the tool-result user line and the bookkeeping lines
        // never become events (§36.11 / §37.5).
        assert_eq!(delta.events.len(), 3, "got {kinds:?}");
        assert_eq!(
            kinds.last().map(|(k, _)| *k),
            Some("system"),
            "a sidechain line is kept but marked"
        );
        assert_eq!(
            delta.events.last().unwrap().metadata["sidechain"],
            serde_json::json!(true)
        );
    }

    /// Sub-agent transcripts repeat the parent's `sessionId` and, in their
    /// head, look exactly like Claude Code's message lines (measured on a real
    /// file: 75 message lines, one `last-prompt` at the very end). They are
    /// therefore skipped rather than guessed at — a path-based guess would
    /// rewrite the parent's identity, and a key-based guess could re-label real
    /// Claude files (方案 §37.3: 宁可漏判不可错判).
    #[test]
    fn subagent_transcripts_are_left_unclaimed() {
        let dir = unique_dir("subagent");
        let parent = dir.join("-repo");
        let children = parent.join("s-main").join("subagents");
        std::fs::create_dir_all(&children).unwrap();
        std::fs::write(parent.join("s-main.jsonl"), main_lines()).unwrap();
        // No Qoder marker anywhere in the head, exactly like the real ones.
        std::fs::write(
            children.join("agent-aExplore-abc123.jsonl"),
            r#"{"type":"assistant","uuid":"c1","cwd":"/repo","sessionId":"s-main","message":{"role":"assistant","content":[{"type":"text","text":"子 Agent 的结论"}]}}
"#,
        )
        .unwrap();

        let found = QoderAdapter
            .discover_sessions_in(&[dir], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 1, "only the parent session is ingested");
        assert_eq!(found[0].agent_session_id, "s-main");
        assert_eq!(found[0].parent_agent_session_id, None);
    }
}
