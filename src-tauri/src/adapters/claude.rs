//! Claude Code Adapter: `~/.claude/projects/<encoded-cwd>/<session>.jsonl`.
//! `CLAUDE_CONFIG_DIR` overrides the root. Raw transcripts stay untouched.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::{title_from_text, AgentCommand, DiscoveredSession, ReadDelta};
use crate::domain::{Agent, Session, SessionEvent};
use crate::error::Result;
use crate::platform::exec_resolver::{self, AgentInstallation};

pub struct ClaudeAdapter;

fn projects_root() -> Option<PathBuf> {
    crate::platform::paths::resolve_agent_data_dir(Agent::ClaudeCode).map(|root| root.join("projects"))
}

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
                    Some("tool_use") => {
                        let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                        let input = item
                            .get("input")
                            .map(|i| crate::adapters::truncate_text(&i.to_string(), 200))
                            .unwrap_or_default();
                        parts.push(format!("[tool_use:{}] {}", name, input));
                    }
                    Some("tool_result") => {
                        let t = content_text(item.get("content").unwrap_or(&Value::Null));
                        parts.push(crate::adapters::truncate_text(&format!("[tool_result] {}", t), 300));
                    }
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
        let cwd = path
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .and_then(|s| crate::platform::paths::decode_cwd_dir_name(&s));

        let lines = crate::adapters::read_jsonl_lines(path)?;
        let mut session_id: Option<String> = None;
        let mut first_user_text = None;
        let mut started_at = None;
        let mut last_ts = None;

        for (_, line) in &lines {
            let v: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if session_id.is_none() {
                session_id = v.get("sessionId").and_then(|s| s.as_str()).map(|s| s.to_string());
            }
            if started_at.is_none() {
                started_at = v.get("timestamp").and_then(|t| t.as_str()).map(|s| s.to_string());
            }
            last_ts = v.get("timestamp").and_then(|t| t.as_str()).map(|s| s.to_string());
            if first_user_text.is_none()
                && v.get("type").and_then(|t| t.as_str()) == Some("user")
                && v.get("isSidechain").and_then(|s| s.as_bool()) != Some(true)
                && v.get("isMeta").and_then(|s| s.as_bool()) != Some(true)
            {
                if let Some(msg) = v.get("message") {
                    let text = content_text(msg.get("content").unwrap_or(&Value::Null));
                    if !text.is_empty() && !text.starts_with('<') {
                        first_user_text = Some(crate::adapters::truncate_text(&text, 400));
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
            first_user_text,
            parent_agent_session_id: None,
        }))
    }
}

impl crate::adapters::AgentAdapter for ClaudeAdapter {
    fn agent(&self) -> Agent {
        Agent::ClaudeCode
    }

    fn detect(&self) -> Option<AgentInstallation> {
        exec_resolver::resolve_quiet(Agent::ClaudeCode)
    }

    fn session_root(&self) -> Option<PathBuf> {
        projects_root()
    }

    fn discover_sessions(&self) -> Result<Vec<DiscoveredSession>> {
        let root = match projects_root() {
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
            let vtype = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let sidechain = v.get("isSidechain").and_then(|s| s.as_bool()).unwrap_or(false);

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
                            continue;
                        }
                        let kind = if vtype == "user" {
                            "user_message"
                        } else {
                            "assistant_message"
                        };
                        (kind, text, serde_json::json!({}))
                    }
                }
                "summary" => ("compact", v.get("summary").and_then(|s| s.as_str()).unwrap_or("").to_string(), serde_json::json!({})),
                _ => continue,
            };

            if text.trim().is_empty() {
                continue;
            }

            let mut meta = serde_json::Map::new();
            meta.insert("agent".into(), serde_json::Value::String("claude_code".into()));
            meta.insert("type".into(), serde_json::Value::String(vtype.to_string()));
            if let Some(obj) = extra.as_object() {
                for (k, v2) in obj {
                    meta.insert(k.clone(), v2.clone());
                }
            }

            events.push(
                SessionEvent {
                    session_id: session.id.clone(),
                    sequence: seq,
                    ts,
                    kind: kind.into(),
                    text: Some(text),
                    raw_ref: format!("{}#line:{}", path.display(), idx + 1),
                    metadata: serde_json::Value::Object(meta),
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
        let mut args = vec!["--resume".into(), agent_session_id.into()];
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
        let mut args: Vec<String> = vec!["-p".into(), "--output-format".into(), "text".into()];
        if let Some(m) = &opts.model {
            args.extend(["--model".into(), m.clone()]);
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
