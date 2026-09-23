//! Codex Adapter: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`.
//! `CODEX_HOME` overrides the root. Raw data is read-only during normal
//! operation; the one exception is the adapter-owned, user-confirmed
//! permanent source deletion below (方案 §39).

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::{
    detect_format, json_str_field, read_jsonl_delta, truncate_text, AgentCommand,
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

/// The uuid Codex wrote into a rollout's name: the file's OWN thread id.
/// 77 of this machine's 78 rollouts have it equal to `payload.id`, and the one
/// that does not is exactly the forked page below (方案 §37.12).
fn thread_id_from_filename(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_string_lossy().to_string();
    let rest = stem.strip_prefix("rollout-")?;
    let own = rest.rsplit('_').next()?;
    let parts: Vec<&str> = own.rsplit('-').take(5).collect();
    if parts.len() < 5 {
        return None;
    }
    Some(parts.into_iter().rev().collect::<Vec<_>>().join("-"))
}

/// A forked page's name carries `_<own uuid>` after the id it forked from;
/// this is Codex's own mark that the file continues another rollout rather than
/// being it (方案 §37.12).
fn is_forked_page_name(path: &Path) -> bool {
    path.file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_prefix("rollout-"))
        .map(|rest| rest.contains('_'))
        .unwrap_or(false)
}

impl CodexAdapter {
    fn parse_rollout(path: &Path) -> Result<Option<DiscoveredSession>> {
        let lines = crate::adapters::read_jsonl_lines(path)?;
        let mut meta_seen = false;
        let mut meta_id = None;
        let mut legacy_session_id = None;
        let mut cwd = None;
        let mut started_at = None;
        let mut parent = None;
        let mut fork_base = None;
        // Codex's own review subagents: their user turns are prompts Codex wrote
        // (the parent transcript re-embedded) and their developer turns are its
        // instructions, so there is no user text to name them after.
        let mut internal_thread = false;
        let mut first_user_text = None;

        for (_, line) in &lines {
            let v: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let vtype = v.get("type").and_then(|t| t.as_str());
            if vtype == Some("session_meta") {
                let p = v.get("payload").unwrap_or(&v);
                meta_seen = true;
                meta_id = p.get("id").and_then(|s| s.as_str()).map(|s| s.to_string());
                // Legacy last resort only: `session_id` is the conversation the
                // writer was joined to, not this thread, so it never decides
                // identity while a real thread id is available (§37.12).
                legacy_session_id = p
                    .get("session_id")
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
                // A forked page records where the prefix it inherited lives.
                fork_base = p
                    .get("history_base")
                    .and_then(|h| h.get("thread_id"))
                    .and_then(|t| t.as_str())
                    .map(|t| t.to_string());
                internal_thread = matches!(
                    p.get("thread_source").and_then(|t| t.as_str()),
                    Some("subagent") | Some("guardian_review")
                );
            } else if first_user_text.is_none()
                && !internal_thread
                && vtype == Some("response_item")
            {
                let p = v.get("payload").unwrap_or(&v);
                if p.get("type").and_then(|t| t.as_str()) == Some("message")
                    && p.get("role").and_then(|r| r.as_str()) == Some("user")
                {
                    let text = extract_text(p.get("content").unwrap_or(&Value::Null));
                    // `<…>` environment_context wrappers and `#`-prefixed
                    // injections (AGENTS.md, attached-file headers) are not
                    // user text.
                    if !text.is_empty() && !text.starts_with("<") && !text.starts_with('#') {
                        first_user_text = Some(truncate_text(&text, 400));
                    }
                }
            }
            // session_meta opens a rollout file, so stopping at the meta
            // line itself would leave first_user_text — and every title —
            // unset. Stop only once the meta is parsed and either the first user
            // text is in hand or this is a thread Codex wrote itself; a
            // meta-only session (no user turn yet) scans to EOF.
            if meta_seen && (internal_thread || first_user_text.is_some()) {
                break;
            }
        }

        // Identity is the file's OWN thread id. A forked page's meta still
        // names the thread it forked FROM, so there the name has to decide;
        // everywhere else the meta is Codex's own statement of the thread id
        // and leads. Keying on `session_id` instead is what collapsed this
        // machine's 78 rollouts into 37 sessions (§37.12).
        let by_name = || thread_id_from_filename(path);
        let session_id = if is_forked_page_name(path) {
            by_name().or(meta_id).or(legacy_session_id)
        } else {
            meta_id.or_else(by_name).or(legacy_session_id)
        }
        .ok_or_else(|| {
            crate::error::other(format!("无法解析 Codex session id: {}", path.display()))
        })?;

        // A fork whose only parent link is `history_base` still gets a real
        // parent; a self-reference would be a lie.
        let parent = parent.or_else(|| fork_base.filter(|b| *b != session_id));

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
                let is_rollout_jsonl = p.extension().and_then(|e| e.to_str()) == Some("jsonl")
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with("rollout-"))
                        .unwrap_or(false);
                if !is_rollout_jsonl {
                    continue;
                }
                // Fully ingested and stat-identical since the last pass: the
                // stored cursor is the source of truth, skip the parse.
                if unchanged(&p) {
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
        // Internal review threads contribute only the subagent's own replies:
        // their user turns are prompts Codex wrote and their developer turns are
        // its instructions — machine chatter in the user role, which is the same
        // call ZCode makes from `semantics` (方案 §36.11 / §37.12).
        let internal = crate::adapters::read_first_json_line(&path)
            .map(|v| {
                let p = v.get("payload").unwrap_or(&v);
                matches!(
                    p.get("thread_source").and_then(|t| t.as_str()),
                    Some("subagent") | Some("guardian_review")
                )
            })
            .unwrap_or(false);
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
                        // Tool traffic is deliberately not ingested (方案 §36.11):
                        // machine chatter whose payload shape also drifts across
                        // Codex versions (function_call vs custom_tool_call).
                        "function_call"
                        | "function_call_output"
                        | "custom_tool_call"
                        | "custom_tool_call_output" => return None,
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

            // An internal review thread contributes only its own replies; every
            // other line is a prompt Codex wrote, a compaction marker, or the
            // synthetic session-meta event.
            if internal && kind != "assistant_message" {
                return None;
            }

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

    /// Codex source deletion (方案 §15): the session source is the exact
    /// `rollout-*.jsonl` at `raw_path`. Validation and removal live here, in
    /// the adapter — Core never touches the file.
    fn prepare_source_session_deletion(
        &self,
        session: &Session,
    ) -> Result<crate::adapters::SourceDeletionPlan> {
        crate::adapters::prepare_single_file_source_deletion(
            session,
            Agent::Codex,
            "codex_rollout",
            &|p| Ok(Self::parse_rollout(p)?.map(|d| d.agent_session_id)),
        )
    }

    fn execute_source_session_deletion(
        &self,
        plan: &crate::adapters::SourceDeletionPlan,
    ) -> Result<crate::adapters::SourceDeletionOutcome> {
        crate::adapters::execute_single_file_source_deletion(plan, Agent::Codex, &|p| {
            Ok(Self::parse_rollout(p)?.map(|d| d.agent_session_id))
        })
    }
}

#[cfg(test)]
mod rollout_tests {
    use super::*;

    const SESSION_ID: &str = "01a0bee7-6afb-7622-afcd-e26c61dd545d";

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("noending-codex-{}-{}", tag, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_rollout(dir: &Path, name: &str, lines: &[String]) -> PathBuf {
        let mut body = lines.join("\n");
        body.push('\n');
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    /// Line shapes mirror real `~/.codex/sessions` rollout files: meta on
    /// ordinal 0, then a developer `<app-context>` payload, then user-role
    /// wrapper payloads before the first real user turn.
    fn meta_line() -> String {
        serde_json::json!({
            "timestamp": "2026-09-20T13:01:47.832Z", "ordinal": 0,
            "type": "session_meta",
            "payload": {
                "session_id": SESSION_ID,
                "id": SESSION_ID,
                "timestamp": "2026-09-20T13:00:32.388Z",
                "cwd": "/tmp/proj"
            }
        })
        .to_string()
    }

    fn message_line(ordinal: usize, role: &str, text: &str) -> String {
        serde_json::json!({
            "timestamp": "2026-09-20T13:01:48.000Z", "ordinal": ordinal,
            "type": "response_item",
            "payload": {
                "type": "message",
                "id": format!("m{ordinal}"),
                "role": role,
                "content": [{ "type": "input_text", "text": text }]
            }
        })
        .to_string()
    }

    /// Regression: session_meta opens every rollout file, so the scan must
    /// continue past it to reach the first user message — stopping at the
    /// meta line left every Codex session without a title source.
    #[test]
    fn first_user_text_survives_meta_on_line_zero() {
        let dir = temp_dir("titled");
        let path = write_rollout(
            &dir,
            "rollout-titled.jsonl",
            &[
                meta_line(),
                message_line(2, "developer", "<app-context>\n# Codex desktop context"),
                message_line(5, "user", "<recommended_plugins>\nDro is available…"),
                message_line(6, "user", "# AGENTS.md instructions for /tmp/proj"),
                message_line(8, "user", "我想对整体工程进行代码瘦身，请给出优化方案"),
            ],
        );
        let d = CodexAdapter::parse_rollout(&path).unwrap().unwrap();
        assert_eq!(d.agent_session_id, SESSION_ID);
        assert_eq!(d.cwd.as_deref(), Some("/tmp/proj"));
        assert_eq!(
            d.first_user_text.as_deref(),
            Some("我想对整体工程进行代码瘦身，请给出优化方案")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn meta_line_with(payload: serde_json::Value) -> String {
        serde_json::json!({
            "timestamp": "2026-09-20T13:01:47.832Z", "ordinal": 0,
            "type": "session_meta", "payload": payload
        })
        .to_string()
    }

    /// A forked page carries `_<own uuid>` in its name yet still names the
    /// thread it forked FROM in `payload.id`: identity must follow the name, or
    /// the fork collapses back into the thread it came from (方案 §37.12).
    #[test]
    fn a_forked_page_is_its_own_session_with_the_fork_as_parent() {
        const BASE: &str = "019f135a-621c-76a1-a76c-7c71021847aa";
        const OWN: &str = "01a0c943-53b8-7e82-8f84-0c2b33da8801";
        let dir = temp_dir("forked-page");
        let path = write_rollout(
            &dir,
            &format!("rollout-2026-09-22T21-17-07-{BASE}_{OWN}.jsonl"),
            &[
                meta_line_with(serde_json::json!({
                    "session_id": BASE, "id": BASE, "cwd": "/tmp/proj",
                    "thread_source": "user", "history_mode": "paginated",
                    "history_base": { "thread_id": BASE, "end_ordinal_exclusive": 90 }
                })),
                message_line(90, "user", "接着上面继续"),
            ],
        );
        let d = CodexAdapter::parse_rollout(&path).unwrap().unwrap();
        assert_eq!(d.agent_session_id, OWN, "the file's name owns the identity");
        assert_eq!(d.parent_agent_session_id.as_deref(), Some(BASE));
        assert_eq!(d.first_user_text.as_deref(), Some("接着上面继续"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Codex's review subagents have no user turns of their own: their first
    /// "user" message is a prompt Codex wrote (the parent transcript
    /// re-embedded). Naming the session after it would put a paragraph of
    /// machine text in the list (方案 §37.12).
    #[test]
    fn an_internal_review_thread_gets_no_title() {
        let dir = temp_dir("internal-thread");
        let parent = "019f135a-621c-76a1-a76c-7c71021847aa";
        let path = write_rollout(
            &dir,
            &format!("rollout-2026-09-20T21-01-47-{SESSION_ID}.jsonl"),
            &[
                meta_line_with(serde_json::json!({
                    "session_id": SESSION_ID, "id": SESSION_ID, "cwd": "/tmp/proj",
                    "thread_source": "subagent", "parent_thread_id": parent
                })),
                message_line(2, "developer", "<permissions instructions>"),
                message_line(
                    5,
                    "user",
                    "The following is the Codex agent history whose request action you are assessing.",
                ),
                message_line(9, "assistant", "{\"outcome\":\"allow\"}"),
            ],
        );
        let d = CodexAdapter::parse_rollout(&path).unwrap().unwrap();
        assert_eq!(d.agent_session_id, SESSION_ID);
        assert_eq!(d.first_user_text, None, "no user text, so no title");
        assert_eq!(d.parent_agent_session_id.as_deref(), Some(parent));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A just-started session has a meta line but no user turn yet: parse
    /// still succeeds (scanning to EOF) and simply yields no user text.
    #[test]
    fn meta_only_file_parses_without_user_text() {
        let dir = temp_dir("meta-only");
        let path = write_rollout(&dir, "rollout-meta-only.jsonl", &[meta_line()]);
        let d = CodexAdapter::parse_rollout(&path).unwrap().unwrap();
        assert_eq!(d.agent_session_id, SESSION_ID);
        assert_eq!(d.cwd.as_deref(), Some("/tmp/proj"));
        assert_eq!(d.first_user_text, None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
