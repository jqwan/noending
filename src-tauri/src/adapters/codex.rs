//! Codex Adapter: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`.
//! `CODEX_HOME` overrides the root. Never deletes or modifies raw data.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::{
    detect_format, json_str_field, read_jsonl_delta, title_from_text, truncate_text, AgentCommand,
    DiscoveredSession, ParsedLine, ReadDelta,
};
use crate::domain::{Agent, Session, SourceCursor};
use crate::error::Result;
use crate::platform::exec_resolver::{self, AgentInstallation};

pub struct CodexAdapter;

fn extract_text(content: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(arr) = content.as_array() {
        for item in arr {
            if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                parts.push(t.to_string());
            }
        }
    } else if let Some(t) = content.as_str() {
        parts.push(t.to_string());
    }
    parts.join("\n")
}

impl CodexAdapter {
    fn parse_rollout(path: &Path) -> Result<Option<DiscoveredSession>> {
        let lines = crate::adapters::read_jsonl_lines(path)?;
        let mut session_id = None;
        let mut cwd = None;
        let mut started_at = None;
        let mut parent = None;
        let mut first_user_text = None;

        for (_, line) in &lines {
            let v: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if v.get("type").and_then(|t| t.as_str()) == Some("session_meta") {
                let p = v.get("payload").unwrap_or(&v);
                session_id = p
                    .get("session_id")
                    .or_else(|| p.get("id"))
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string());
                cwd = p.get("cwd").and_then(|c| c.as_str()).map(|c| c.to_string());
                started_at = p
                    .get("timestamp")
                    .and_then(|t| t.as_str())
                    .map(|t| t.to_string());
                parent = p
                    .get("parent_thread_id")
                    .and_then(|t| t.as_str())
                    .map(|t| t.to_string());
                if session_id.is_some() {
                    break;
                }
            } else if first_user_text.is_none()
                && v.get("type").and_then(|t| t.as_str()) == Some("response_item")
            {
                let p = v.get("payload").unwrap_or(&v);
                if p.get("type").and_then(|t| t.as_str()) == Some("message")
                    && p.get("role").and_then(|r| r.as_str()) == Some("user")
                {
                    let text = extract_text(p.get("content").unwrap_or(&Value::Null));
                    // skip environment_context style wrapper payloads
                    if !text.is_empty() && !text.starts_with("<") {
                        first_user_text = Some(truncate_text(&text, 400));
                    }
                }
            }
        }

        let session_id = match session_id {
            Some(id) => id,
            None => {
                // fall back to filename-derived uuid
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .ok_or_else(|| crate::error::other("无效的 rollout 文件名"))?;
                let id = stem.rsplit('-').take(5).collect::<Vec<_>>();
                if id.len() < 5 {
                    return Err(crate::error::other(format!(
                        "无法解析 Codex session id: {}",
                        stem
                    )));
                }
                id.into_iter().rev().collect::<Vec<_>>().join("-")
            }
        };

        let meta = std::fs::metadata(path)?;
        let last_activity = meta
            .modified()
            .ok()
            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());

        Ok(Some(DiscoveredSession {
            agent: Agent::Codex,
            agent_session_id: session_id,
            path: path.to_path_buf(),
            cwd,
            started_at,
            last_activity_at: last_activity,
            first_user_text,
            parent_agent_session_id: parent,
        }))
    }
}

/// Runtime overrides in Codex's own spelling: `-m <model>` plus a config
/// override for reasoning effort. Codex has no provider flag, so
/// `ExecOptions::provider` is deliberately not rendered here.
fn runtime_args(opts: &crate::adapters::ExecOptions) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(m) = &opts.model {
        args.extend(["-m".into(), m.clone()]);
    }
    if let Some(e) = &opts.effort {
        args.extend(["-c".into(), format!("model_reasoning_effort=\"{}\"", e)]);
    }
    args
}

impl crate::adapters::AgentAdapter for CodexAdapter {
    fn agent(&self) -> Agent {
        Agent::Codex
    }

    fn detect(&self) -> Option<AgentInstallation> {
        exec_resolver::resolve_quiet(Agent::Codex)
    }

    fn discover_sessions_in(&self, roots: &[PathBuf]) -> Result<Vec<DiscoveredSession>> {
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
                let is_rollout_jsonl = p.extension().and_then(|e| e.to_str()) == Some("jsonl")
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with("rollout-"))
                        .unwrap_or(false);
                if !is_rollout_jsonl {
                    continue;
                }
                // The filename only pre-filters; the content decides. One
                // bad file never aborts the whole scan.
                if detect_format(&p) != Some(Agent::Codex) {
                    eprintln!(
                        "[discover] skip {} (content fingerprint is not codex)",
                        p.display()
                    );
                    continue;
                }
                match Self::parse_rollout(&p) {
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
            let payload = v.get("payload").cloned().unwrap_or(Value::Null);
            let source_event_id = json_str_field(v, "id")
                .or_else(|| json_str_field(&payload, "id"))
                .map(|s| s.to_string());

            let (kind, text) = match vtype {
                "response_item" => {
                    let ptype = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    match ptype {
                        "message" => {
                            let role = payload.get("role").and_then(|r| r.as_str()).unwrap_or("");
                            let text = extract_text(payload.get("content").unwrap_or(&Value::Null));
                            if text.is_empty() {
                                return None;
                            }
                            match role {
                                "user" => ("user_message", text),
                                "assistant" => ("assistant_message", text),
                                _ => ("system", text),
                            }
                        }
                        "function_call" => (
                            "tool_call",
                            format!(
                                "{} {}",
                                payload
                                    .get("name")
                                    .and_then(|n| n.as_str())
                                    .unwrap_or("tool"),
                                payload
                                    .get("arguments")
                                    .and_then(|a| a.as_str())
                                    .map(|s| truncate_text(s, 200))
                                    .unwrap_or_default()
                            ),
                        ),
                        "function_call_output" => (
                            "tool_result",
                            truncate_text(
                                payload.get("output").and_then(|o| o.as_str()).unwrap_or(""),
                                200,
                            ),
                        ),
                        "reasoning" => return None, // internal model reasoning: not meaningful context
                        _ => ("unknown", String::new()),
                    }
                }
                "event_msg" => {
                    let ptype = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    if ptype == "compact" || ptype.contains("compact") {
                        ("compact", "conversation compacted".into())
                    } else {
                        return None;
                    }
                }
                "session_meta" => ("system", "session meta".into()),
                _ => return None, // unrecognized payload types carry no meaningful text
            };

            Some(ParsedLine {
                kind: kind.into(),
                text: Some(text),
                source_event_id,
                metadata: serde_json::json!({ "agent": "codex", "type": vtype }),
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
        // codex resume [OPTIONS] [SESSION_ID] [PROMPT]
        let mut args = vec!["resume".into()];
        args.extend(runtime_args(opts));
        args.push(agent_session_id.into());
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
        let mut args: Vec<String> = vec![
            "exec".into(),
            "-s".into(),
            "read-only".into(),
            "--skip-git-repo-check".into(),
        ];
        args.extend(runtime_args(opts));
        args.push(prompt.to_string());
        Ok(AgentCommand {
            program: install.executable_path.clone(),
            args,
            cwd: None, // headless analysis never touches user repos
        })
    }
}

pub fn extract_title(d: &DiscoveredSession) -> Option<String> {
    d.first_user_text.as_deref().and_then(title_from_text)
}
