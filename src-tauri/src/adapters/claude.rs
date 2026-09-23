//! Claude Code Adapter: `~/.claude/projects/<encoded-cwd>/<session>.jsonl`.
//! `CLAUDE_CONFIG_DIR` overrides the root. Raw transcripts stay untouched
//! during normal operation; the one exception is the adapter-owned,
//! user-confirmed permanent source deletion below (方案 §39).

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::{
    detect_format, read_jsonl_delta, AgentCommand, DiscoveredSession, ParsedLine, ReadDelta,
};
use crate::domain::{Agent, Session, SourceCursor};
use crate::error::Result;
use crate::platform::exec_resolver::{self, AgentInstallation};

pub struct ClaudeAdapter;

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
                    // Tool traffic is deliberately dropped (方案 §36.11): the
                    // `[tool_use:…]` / `[tool_result] …` fragments used to be
                    // folded into the owning message's text.
                    _ => {}
                }
            }
            parts.join("\n")
        }
        _ => String::new(),
    }
}

impl ClaudeAdapter {
    fn parse_session_file(path: &Path) -> Result<Option<DiscoveredSession>> {
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

        // The transcript carries the real cwd on its lines — authoritative
        // and lossless, unlike the encoded (and ambiguous) directory name.
        let mut cwd: Option<String> = None;
        for (_, line) in &lines {
            let v: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
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
            // Last resort for a title when the session has no user turn (§37.15).
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

        Ok(Some(DiscoveredSession {
            agent: Agent::ClaudeCode,
            agent_session_id: session_id.unwrap_or_else(|| file_name.clone()),
            path: path.to_path_buf(),
            cwd,
            started_at,
            last_activity_at: last_activity.or(last_ts),
            // Claude Code writes no session title.
            native_title: None,
            first_user_text,
            first_agent_text,
            parent_agent_session_id: None,
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
                // Fully ingested and stat-identical since the last pass: the
                // stored cursor is the source of truth, skip the parse.
                if unchanged(&p) {
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
                    if sidechain {
                        // sub-agent chatter: keep but mark, low value for sync pre-filter
                        let text = v
                            .get("message")
                            .map(|m| content_text(m.get("content").unwrap_or(&Value::Null)))
                            .unwrap_or_default();
                        ("system", text, serde_json::json!({ "sidechain": true }))
                    } else {
                        let msg = v.get("message").unwrap_or(&Value::Null);
                        let text = content_text(msg.get("content").unwrap_or(&Value::Null));
                        if text.is_empty() {
                            return None;
                        }
                        let kind = if vtype == "user" {
                            "user_message"
                        } else {
                            "assistant_message"
                        };
                        (kind, text, serde_json::json!({}))
                    }
                }
                "summary" => (
                    "compact",
                    v.get("summary")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string(),
                    serde_json::json!({}),
                ),
                _ => return None,
            };

            let mut meta = serde_json::Map::new();
            meta.insert(
                "agent".into(),
                serde_json::Value::String("claude_code".into()),
            );
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
        install: &AgentInstallation,
        opts: &crate::adapters::ExecOptions,
        context_file: Option<&Path>,
        cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        let mut args = runtime_args(opts);
        args.extend(crate::adapters::context_prompt(context_file)?);
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
        context_file: Option<&Path>,
        cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        let mut args = vec!["--resume".into(), agent_session_id.into()];
        args.extend(runtime_args(opts));
        args.extend(crate::adapters::context_prompt(context_file)?);
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

    /// Claude Code source deletion (方案 §15): the session source is the
    /// exact discovered `*.jsonl` transcript at `raw_path`. Validation and
    /// removal live here, in the adapter — Core never touches the file.
    fn prepare_source_session_deletion(
        &self,
        session: &Session,
    ) -> Result<crate::adapters::SourceDeletionPlan> {
        crate::adapters::prepare_single_file_source_deletion(
            session,
            Agent::ClaudeCode,
            "claude_code_transcript",
            &|p| Ok(Self::parse_session_file(p)?.map(|d| d.agent_session_id)),
        )
    }

    fn execute_source_session_deletion(
        &self,
        plan: &crate::adapters::SourceDeletionPlan,
    ) -> Result<crate::adapters::SourceDeletionOutcome> {
        crate::adapters::execute_single_file_source_deletion(plan, Agent::ClaudeCode, &|p| {
            Ok(Self::parse_session_file(p)?.map(|d| d.agent_session_id))
        })
    }
}
