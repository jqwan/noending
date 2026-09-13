//! Codex Adapter: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`.
//! `CODEX_HOME` overrides the root. Never deletes or modifies raw data.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::{title_from_text, truncate_text, AgentCommand, DiscoveredSession, ReadDelta};
use crate::domain::{Agent, Session, SessionEvent};
use crate::error::{other, Result};
use crate::platform::exec_resolver::{self, AgentInstallation};

pub struct CodexAdapter;

fn sessions_root() -> Option<PathBuf> {
    crate::platform::paths::resolve_agent_data_dir(Agent::Codex).map(|root| root.join("sessions"))
}

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
                    return Err(other(format!("无法解析 Codex session id: {}", stem)));
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

impl crate::adapters::AgentAdapter for CodexAdapter {
    fn agent(&self) -> Agent {
        Agent::Codex
    }

    fn detect(&self) -> Option<AgentInstallation> {
        exec_resolver::resolve_quiet(Agent::Codex)
    }

    fn session_root(&self) -> Option<PathBuf> {
        sessions_root()
    }

    fn discover_sessions(&self) -> Result<Vec<DiscoveredSession>> {
        let root = match sessions_root() {
            Some(r) if r.is_dir() => r,
            _ => return Ok(vec![]),
        };
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let rd = match std::fs::read_dir(&dir) {
                Ok(rd) => rd,
                Err(_) => continue,
            };
            for entry in rd.filter_map(|e| e.ok()) {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().and_then(|e| e.to_str()) == Some("jsonl")
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with("rollout-"))
                        .unwrap_or(false)
                {
                    if let Some(s) = Self::parse_rollout(&p)? {
                        out.push(s);
                    }
                }
            }
        }
        Ok(out)
    }

    fn read_delta(&self, session: &Session, from_sequence: i64) -> Result<ReadDelta> {
        let path = PathBuf::from(&session.raw_path);
        let file_size = std::fs::metadata(&path)?.len() as i64;
        let lines = crate::adapters::read_jsonl_lines(&path)?;
        let mut events = Vec::new();

        for (idx, line) in &lines {
            let seq = *idx as i64 + 1;
            if seq <= from_sequence {
                continue;
            }
            let v: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let ts = v.get("timestamp").and_then(|t| t.as_str()).map(|s| s.to_string());
            let vtype = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let payload = v.get("payload").cloned().unwrap_or(Value::Null);

            let (kind, text) = match vtype {
                "response_item" => {
                    let ptype = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    match ptype {
                        "message" => {
                            let role = payload.get("role").and_then(|r| r.as_str()).unwrap_or("");
                            let text = extract_text(payload.get("content").unwrap_or(&Value::Null));
                            if text.is_empty() {
                                continue;
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
                                payload.get("name").and_then(|n| n.as_str()).unwrap_or("tool"),
                                payload
                                    .get("arguments")
                                    .and_then(|a| a.as_str())
                                    .map(truncate_text_arg)
                                    .unwrap_or_default()
                            ),
                        ),
                        "function_call_output" => (
                            "tool_result",
                            truncate_text_arg(payload.get("output").and_then(|o| o.as_str()).unwrap_or("")),
                        ),
                        "reasoning" => continue, // internal model reasoning: not meaningful context
                        _ => ("unknown", String::new()),
                    }
                }
                "event_msg" => {
                    let ptype = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    if ptype == "compact" || ptype.contains("compact") {
                        ("compact", "conversation compacted".into())
                    } else {
                        continue;
                    }
                }
                "session_meta" => ("system", "session meta".into()),
                _ => continue, // unrecognized payload types carry no meaningful text
            };

            if text.trim().is_empty() {
                continue;
            }

            events.push(
                SessionEvent {
                    session_id: session.id.clone(),
                    sequence: seq,
                    ts: ts.clone(),
                    kind: kind.into(),
                    text: if text.is_empty() { None } else { Some(text) },
                    raw_ref: format!("{}#line:{}", path.display(), idx + 1),
                    metadata: serde_json::json!({ "agent": "codex", "type": vtype }),
                },
            );
        }

        let last_sequence = lines.len() as i64;
        Ok(ReadDelta {
            events,
            last_sequence,
            file_size,
        })
    }

    fn build_new_command(
        &self,
        install: &AgentInstallation,
        context_file: Option<&Path>,
        cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        Ok(AgentCommand {
            program: install.executable_path.clone(),
            args: crate::adapters::prompt_from_context_file(context_file)
                .into_iter()
                .collect(),
            prompt: None,
            cwd: cwd.map(|p| p.to_path_buf()),
        })
    }

    fn build_resume_command(
        &self,
        install: &AgentInstallation,
        agent_session_id: &str,
        context_file: Option<&Path>,
        cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        let mut args = vec!["resume".into(), agent_session_id.into()];
        args.extend(crate::adapters::prompt_from_context_file(context_file));
        Ok(AgentCommand {
            program: install.executable_path.clone(),
            args,
            prompt: None,
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
        if let Some(m) = &opts.model {
            args.extend(["-m".into(), m.clone()]);
        }
        if let Some(e) = &opts.effort {
            args.extend(["-c".into(), format!("model_reasoning_effort=\"{}\"", e)]);
        }
        args.push(prompt.to_string());
        Ok(AgentCommand {
            program: install.executable_path.clone(),
            args,
            prompt: None,
            cwd: None, // headless analysis never touches user repos
        })
    }
}

fn truncate_text_arg(s: &str) -> String {
    truncate_text(s, 200)
}

pub fn extract_title(d: &DiscoveredSession) -> Option<String> {
    d.first_user_text.as_deref().and_then(title_from_text)
}
