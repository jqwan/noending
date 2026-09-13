//! Pi Adapter: `~/.pi/agent/sessions/<encoded-cwd>/<ts>_<uuid>.jsonl`.
//! `PI_HOME` overrides the root. Pi sessions are plain JSONL trees.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::{title_from_text, AgentCommand, DiscoveredSession, ReadDelta};
use crate::domain::{Agent, Session, SessionEvent};
use crate::error::Result;
use crate::platform::exec_resolver::{self, AgentInstallation};

pub struct PiAdapter;

fn sessions_root() -> Option<PathBuf> {
    crate::platform::paths::resolve_agent_data_dir(Agent::Pi)
        .map(|root| root.join("agent").join("sessions"))
}

fn content_text(content: &Value) -> (String, Vec<String>) {
    // returns (text parts, tool call summaries)
    let mut text_parts = Vec::new();
    let mut tool_parts = Vec::new();
    if let Some(arr) = content.as_array() {
        for item in arr {
            match item.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                        text_parts.push(t.to_string());
                    }
                }
                Some("thinking") => { /* model reasoning: skip */ }
                Some("toolCall") | Some("tool_call") => {
                    let name = item
                        .get("name")
                        .or_else(|| item.get("id"))
                        .and_then(|n| n.as_str())
                        .unwrap_or("tool");
                    let raw_args = item
                        .get("arguments")
                        .or_else(|| item.get("input"))
                        .map(|a| a.to_string())
                        .unwrap_or_default();
                    tool_parts.push(format!(
                        "[tool:{}] {}",
                        name,
                        crate::adapters::truncate_text(&raw_args, 150)
                    ));
                }
                _ => {}
            }
        }
    } else if let Some(s) = content.as_str() {
        text_parts.push(s.to_string());
    }
    (text_parts.join("\n"), tool_parts)
}

impl PiAdapter {
    fn parse_session_file(path: &Path) -> Result<Option<DiscoveredSession>> {
        let lines = crate::adapters::read_jsonl_lines(path)?;
        let mut session_id: Option<String> = None;
        let mut cwd: Option<String> = None;
        let mut started_at: Option<String> = None;
        let mut first_user_text: Option<String> = None;

        for (_, line) in &lines {
            let v: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            match v.get("type").and_then(|t| t.as_str()) {
                Some("session") => {
                    session_id = v.get("id").and_then(|s| s.as_str()).map(|s| s.to_string());
                    cwd = v.get("cwd").and_then(|c| c.as_str()).map(|c| c.to_string());
                    started_at = v.get("timestamp").and_then(|t| t.as_str()).map(|t| t.to_string());
                }
                Some("message") => {
                    if first_user_text.is_none() {
                        if let Some(msg) = v.get("message") {
                            if msg.get("role").and_then(|r| r.as_str()) == Some("user") {
                                let (text, _) =
                                    content_text(msg.get("content").unwrap_or(&Value::Null));
                                if !text.is_empty() {
                                    first_user_text = Some(crate::adapters::truncate_text(&text, 400));
                                }
                            }
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
            file_name.as_deref().and_then(|n| n.rsplit('_').next()).map(|s| s.to_string())
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
            first_user_text,
            parent_agent_session_id: None,
        }))
    }
}

impl crate::adapters::AgentAdapter for PiAdapter {
    fn agent(&self) -> Agent {
        Agent::Pi
    }

    fn detect(&self) -> Option<AgentInstallation> {
        exec_resolver::resolve_quiet(Agent::Pi)
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
                } else if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    if let Some(s) = Self::parse_session_file(&p)? {
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
            let vtype = v.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string();

            let (kind, text) = match vtype.as_str() {
                "message" => {
                    let msg = v.get("message").unwrap_or(&Value::Null);
                    let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
                    let (text, tools) = content_text(msg.get("content").unwrap_or(&Value::Null));
                    let mut text = text;
                    if !tools.is_empty() {
                        text.push('\n');
                        text.push_str(&tools.join("\n"));
                    }
                    if text.trim().is_empty() {
                        continue;
                    }
                    match role {
                        "user" => ("user_message", text),
                        "assistant" => ("assistant_message", text),
                        _ => ("system", text),
                    }
                }
                "compaction" | "compact" => ("compact", "conversation compacted".into()),
                "session" => ("system", "session header".into()),
                _ => continue,
            };

            events.push(
                SessionEvent {
                    session_id: session.id.clone(),
                    sequence: seq,
                    ts,
                    kind: kind.into(),
                    text: Some(text),
                    raw_ref: format!("{}#line:{}", path.display(), idx + 1),
                    metadata: serde_json::json!({ "agent": "pi", "type": vtype }),
                },
            );
        }

        Ok(ReadDelta {
            events,
            last_sequence: lines.len() as i64,
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
        let mut args = vec!["--session".into(), agent_session_id.into()];
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
        // pi -p: non-interactive; --no-session keeps analysis ephemeral;
        // --no-tools makes it a pure text model call (safe + cheap).
        let mut args: Vec<String> = vec![
            "-p".into(),
            "--no-session".into(),
            "--no-tools".into(),
        ];
        if let Some(p) = &opts.provider {
            args.extend(["--provider".into(), p.clone()]);
        }
        if let Some(m) = &opts.model {
            args.extend(["--model".into(), m.clone()]);
        }
        if let Some(e) = &opts.effort {
            args.extend(["--thinking".into(), e.clone()]);
        }
        args.push(prompt.to_string());
        Ok(AgentCommand {
            program: install.executable_path.clone(),
            args,
            prompt: None,
            cwd: None,
        })
    }
}

pub fn extract_title(d: &DiscoveredSession) -> Option<String> {
    d.first_user_text.as_deref().and_then(title_from_text)
}
