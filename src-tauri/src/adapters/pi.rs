//! Pi Adapter: `~/.pi/agent/sessions/<encoded-cwd>/<ts>_<uuid>.jsonl`.
//! `PI_HOME` overrides the root. Pi sessions are plain JSONL trees, read-only
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

pub struct PiAdapter;

/// Text parts only: `thinking` blocks are model reasoning, and tool calls /
/// results are deliberately dropped (方案 §36.11) — they used to be appended
/// to the owning message's text as `[tool:…] …`.
fn content_text(content: &Value) -> String {
    let mut text_parts = Vec::new();
    if let Some(arr) = content.as_array() {
        for item in arr {
            if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                    text_parts.push(t.to_string());
                }
            }
        }
    } else if let Some(s) = content.as_str() {
        text_parts.push(s.to_string());
    }
    text_parts.join("\n")
}

impl PiAdapter {
    fn parse_session_file(path: &Path) -> Result<Option<DiscoveredSession>> {
        let lines = crate::adapters::read_jsonl_lines(path)?;
        let mut session_id: Option<String> = None;
        let mut cwd: Option<String> = None;
        let mut started_at: Option<String> = None;
        let mut first_user_text: Option<String> = None;
        let mut first_agent_text: Option<String> = None;

        for (_, line) in &lines {
            let v: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            match v.get("type").and_then(|t| t.as_str()) {
                Some("session") => {
                    session_id = v.get("id").and_then(|s| s.as_str()).map(|s| s.to_string());
                    cwd = v.get("cwd").and_then(|c| c.as_str()).map(|c| c.to_string());
                    started_at = v
                        .get("timestamp")
                        .and_then(|t| t.as_str())
                        .map(|t| t.to_string());
                }
                Some("message") => {
                    if let Some(msg) = v.get("message") {
                        let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
                        let text = content_text(msg.get("content").unwrap_or(&Value::Null));
                        // `<…>` environment blocks and `#`-prefixed injections
                        // (AGENTS.md, attached-file headers) are not user text.
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
                }
                _ => {}
            }
            if session_id.is_some() && first_user_text.is_some() {
                break;
            }
        }

        // fallback: filename carries <ts>_<uuid>
        let file_name = path.file_stem().map(|s| s.to_string_lossy().to_string());
        let session_id = session_id.or_else(|| {
            file_name
                .as_deref()
                .and_then(|n| n.rsplit('_').next())
                .map(|s| s.to_string())
        });
        let session_id = match session_id {
            Some(s) if !s.is_empty() => s,
            _ => return Ok(None),
        };

        let meta = std::fs::metadata(path)?;
        let last_activity = meta
            .modified()
            .ok()
            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());

        Ok(Some(DiscoveredSession {
            agent: Agent::Pi,
            agent_session_id: session_id,
            path: path.to_path_buf(),
            cwd,
            started_at,
            last_activity_at: last_activity,
            // pi writes no session title.
            native_title: None,
            first_user_text,
            first_agent_text,
            parent_agent_session_id: None,
        }))
    }
}

/// Runtime overrides in Pi's spelling — the only CLI of the three where all
/// three fields have a flag.
fn runtime_args(opts: &crate::adapters::ExecOptions) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(p) = &opts.provider {
        args.extend(["--provider".into(), p.clone()]);
    }
    if let Some(m) = &opts.model {
        args.extend(["--model".into(), m.clone()]);
    }
    if let Some(e) = &opts.effort {
        args.extend(["--thinking".into(), e.clone()]);
    }
    args
}

impl crate::adapters::AgentAdapter for PiAdapter {
    fn agent(&self) -> Agent {
        Agent::Pi
    }

    fn detect(&self) -> Option<AgentInstallation> {
        exec_resolver::resolve_quiet(Agent::Pi)
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
                if detect_format(&p) != Some(Agent::Pi) {
                    eprintln!(
                        "[discover] skip {} (content fingerprint is not pi)",
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
                        // Tool output is a `toolResult` MESSAGE in pi, so it used
                        // to land here as `system`. Not ingested (方案 §36.11).
                        "toolResult" | "tool_result" => return None,
                        _ => ("system", text),
                    }
                }
                "compaction" | "compact" => ("compact", "conversation compacted".into()),
                "session" => ("system", "session header".into()),
                _ => return None,
            };

            Some(ParsedLine {
                kind: kind.into(),
                text: Some(text),
                source_event_id,
                metadata: serde_json::json!({ "agent": "pi", "type": vtype }),
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
        // pi [options] [--] [@files...] [messages...] — options first, then
        // the session selector and the prompt.
        let mut args = runtime_args(opts);
        args.extend(["--session".into(), agent_session_id.into()]);
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
        // pi -p: non-interactive; --no-session keeps analysis ephemeral;
        // --no-tools makes it a pure text model call (safe + cheap).
        let mut args: Vec<String> = vec!["-p".into(), "--no-session".into(), "--no-tools".into()];
        args.extend(runtime_args(opts));
        args.push(prompt.to_string());
        Ok(AgentCommand {
            program: install.executable_path.clone(),
            args,
            cwd: None,
        })
    }

    /// Pi source deletion (方案 §15): the session source is the exact
    /// discovered `*.jsonl` transcript at `raw_path`. Validation and removal
    /// live here, in the adapter — Core never touches the file.
    fn prepare_source_session_deletion(
        &self,
        session: &Session,
    ) -> Result<crate::adapters::SourceDeletionPlan> {
        crate::adapters::prepare_single_file_source_deletion(
            session,
            Agent::Pi,
            "pi_session_transcript",
            &|p| Ok(Self::parse_session_file(p)?.map(|d| d.agent_session_id)),
        )
    }

    fn execute_source_session_deletion(
        &self,
        plan: &crate::adapters::SourceDeletionPlan,
    ) -> Result<crate::adapters::SourceDeletionOutcome> {
        crate::adapters::execute_single_file_source_deletion(plan, Agent::Pi, &|p| {
            Ok(Self::parse_session_file(p)?.map(|d| d.agent_session_id))
        })
    }
}
