//! Pi Adapter: `~/.pi/agent/sessions/<encoded-cwd>/<ts>_<uuid>.jsonl`.
//! `PI_CODING_AGENT_DIR` / `PI_CODING_AGENT_SESSION_DIR` override the sessions
//! root (see `platform::paths`). Pi sessions are plain JSONL trees, read-only
//! always — NoEnding never deletes an Agent-owned source.
//!
//! Member mapping: today's sources are single-root — one transcript, one ROOT
//! member. Thinking blocks, tool traffic and runtime injections never become
//! conversation; a compaction marker is an observation.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::{
    detect_format, read_jsonl_delta, AgentCommand, DiscoveredMember, DiscoveredMemberKind,
    ParsedLine, SessionMessageRole,
};
use crate::domain::{Agent, Session, SourceAvailability, SourceCursor};
use crate::error::Result;
use crate::platform::exec_resolver::{self, AgentInstallation};

pub struct PiAdapter;

/// Text parts only: `thinking` blocks are model reasoning, and tool calls /
/// results are deliberately dropped.
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
    fn parse_member(path: &Path) -> Result<Option<DiscoveredMember>> {
        let lines = crate::adapters::read_jsonl_lines(path)?;
        let mut session_id: Option<String> = None;
        let mut cwd: Option<String> = None;
        let mut started_at: Option<String> = None;
        let mut first_user_text: Option<String> = None;
        let mut first_agent_text: Option<String> = None;
        // `session_info` is pi's own session name (`/name`, `--name`); renames
        // APPEND a new entry, so the last one wins. It can land anywhere in the
        // file, which is why discovery scans past the first user turn.
        let mut native_title: Option<String> = None;

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
                Some("session_info") => {
                    if let Some(t) = v
                        .get("name")
                        .and_then(|t| t.as_str())
                        .filter(|t| !t.trim().is_empty())
                    {
                        native_title = Some(t.to_string());
                    }
                }
                Some("message") => {
                    if let Some(msg) = v.get("message") {
                        let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
                        let text = content_text(msg.get("content").unwrap_or(&Value::Null));
                        // pi has no injection concept — the format's UserMessage is
                        // just {role, content}, and the agent never wraps machine
                        // context into user turns. A `#`-headed question is the
                        // user's own Markdown; nothing is filtered here.
                        if first_user_text.is_none() && role == "user" && !text.is_empty() {
                            first_user_text = Some(crate::adapters::truncate_text(&text, 400));
                        }
                        // Last resort for a title.
                        if first_agent_text.is_none() && role == "assistant" && !text.is_empty() {
                            first_agent_text = Some(crate::adapters::truncate_text(&text, 400));
                        }
                    }
                }
                _ => {}
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

        Ok(Some(DiscoveredMember {
            agent: Agent::Pi,
            source_member_id: session_id,
            kind: DiscoveredMemberKind::Root,
            parent_source_member_id: None,
            root_hint: None,
            source_kind: "pi_session_transcript".into(),
            source_path: path.to_path_buf(),
            cwd,
            started_at,
            last_activity_at: last_activity,
            native_title,
            first_user_text,
            first_agent_text,
            metadata: serde_json::json!({}),
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

    fn discover_members_in(
        &self,
        roots: &[PathBuf],
        unchanged: &dyn Fn(&Path) -> bool,
    ) -> Result<Vec<DiscoveredMember>> {
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
                match Self::parse_member(&p) {
                    Ok(Some(m)) => out.push(m),
                    Ok(None) => {}
                    Err(e) => eprintln!("[discover] skip {}: {}", p.display(), e),
                }
            }
        }
        Ok(out)
    }

    fn read_session_delta(
        &self,
        session: &Session,
        cursor: &SourceCursor,
    ) -> Result<crate::adapters::MemberReadDelta> {
        let path = PathBuf::from(&session.source_path);
        let is_root = true; // every stored session is its root source
        read_jsonl_delta(&path, cursor, &|_idx, v| parse_line(v, is_root))
    }

    fn inspect_session_source(&self, session: &Session) -> Result<SourceAvailability> {
        Ok(crate::adapters::inspect_file_source(Path::new(
            &session.source_path,
        )))
    }

    fn supports_prespecified_session_id(&self) -> bool {
        true
    }

    fn build_new_command(
        &self,
        install: &AgentInstallation,
        opts: &crate::adapters::ExecOptions,
        cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        let mut args = runtime_args(opts);
        // The launcher's prespecified id: pi creates the session with this
        // exact id ("creating it if missing"), so the embedded terminal binds
        // by exact match instead of cwd+time inference.
        if let Some(id) = &opts.root_session_id {
            args.extend(["--session-id".into(), id.clone()]);
        }
        if let Some(message) = opts.initial_message() {
            // Pi treats an argv beginning with `@` as a file attachment even
            // after `--`. A leading space keeps that user message textual;
            // all other input is passed exactly as written.
            let message = if message.starts_with('@') {
                format!(" {message}")
            } else {
                message.to_string()
            };
            args.extend(["--".into(), message]);
        }
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
        cwd: Option<&Path>,
        source_path: Option<&str>,
    ) -> Result<AgentCommand> {
        // pi [options] [--] [@files...] [messages...] — options first, then
        // the session selector.
        //
        // `--session <id>` searches pi's DEFAULT session dir only, so a
        // session created with a project-local `--session-dir` is invisible
        // to it ("No session found matching …"). `--session <path|id>`
        // accepts a session FILE: the transcript path we ingested from is
        // authoritative and works regardless of where pi stored it.
        let selector = source_path
            .filter(|p| !p.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| agent_session_id.to_string());
        let mut args = runtime_args(opts);
        args.extend(["--session".into(), selector]);
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

    fn build_context_extraction_command(
        &self,
        install: &AgentInstallation,
        opts: &crate::adapters::ExecOptions,
        prompt: &str,
        runtime_dir: &Path,
    ) -> Result<AgentCommand> {
        // Pi already marks its one-shot, tool-free invocation as ephemeral.
        let mut cmd = self.build_exec_command(install, opts, prompt)?;
        cmd.cwd = Some(runtime_dir.to_path_buf());
        Ok(cmd)
    }
}

/// One line's contribution. `toolResult` messages and every other role are
/// machine traffic; thinking blocks are filtered by `content_text`.
fn parse_line(v: &Value, is_root: bool) -> Option<ParsedLine> {
    let vtype = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let source_message_id = v.get("id").and_then(|s| s.as_str()).map(|s| s.to_string());

    match vtype {
        "message" => {
            let msg = v.get("message").unwrap_or(&Value::Null);
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
            let text = content_text(msg.get("content").unwrap_or(&Value::Null));
            if text.trim().is_empty() {
                return None;
            }
            let role = match role {
                // pi has no injection concept — its UserMessage is plain
                // {role, content}, so nothing is filtered on the user arm.
                "user" => SessionMessageRole::User,
                "assistant" => SessionMessageRole::Assistant,
                // Tool output is a `toolResult` MESSAGE in pi; every other
                // non-conversation role is runtime chatter.
                _ => return None,
            };
            if !is_root {
                return None;
            }
            Some(ParsedLine::message_only(crate::adapters::parsed_message(
                source_message_id,
                role,
                text,
            )))
        }
        // A compaction is a billed model call, but usage is no longer recorded.
        "compaction" | "compact" => None,
        _ => None, // session headers and bookkeeping carry nothing
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::AgentAdapter;

    fn unique_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("noending-pi-{}-{}", tag, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn root_session(path: &Path) -> Session {
        Session {
            id: "sess-pi".into(),
            agent: Agent::Pi,
            root_agent_session_id: "p1".into(),
            title: None,
            cwd: None,
            workspace_path_id: None,
            project_id: None,
            owner_workstream_id: None,
            forked_from_session_id: None,
            started_at: None,
            last_activity_at: None,
            last_conversation_at: None,
            archived_at: None,
            source_kind: "pi_session_transcript".into(),
            source_path: path.to_string_lossy().to_string(),
            metadata: serde_json::json!({}),
            source_file_identity: String::new(),
            source_generation: 0,
            source_byte_offset: 0,
            source_last_seen_size: 0,
            source_mtime: None,
            source_prefix_hash: String::new(),
            source_tail_hash: String::new(),
            fact_generation: 0,
            latest_message_seq: 0,
        }
    }

    fn session_lines() -> String {
        [
            r#"{"type":"session","id":"p1","cwd":"/repo","version":3,"timestamp":"2026-09-18T12:40:00.000Z"}"#,
            r#"{"type":"message","id":"m1","parentId":"p1","timestamp":"2026-09-18T12:40:03.000Z","message":{"role":"user","content":[{"type":"text","text":"帮我看看这个"}]}}"#,
            r#"{"type":"message","id":"m2","parentId":"m1","timestamp":"2026-09-18T12:40:05.000Z","message":{"role":"assistant","usage":{"input":1746,"output":210,"cacheRead":30,"reasoning":47,"totalTokens":1956,"cost":{"total":0.007515}},"content":[{"type":"thinking","thinking":"…"},{"type":"toolCall","toolCallId":"t1"},{"type":"text","text":"看完了。"}]}}"#,
            r#"{"type":"message","id":"m3","parentId":"m2","timestamp":"2026-09-18T12:40:06.000Z","message":{"role":"toolResult","content":[{"type":"text","text":"file contents"}]}}"#,
            r#"{"type":"compaction","id":"c1","parentId":"m3","timestamp":"2026-09-18T12:40:07.000Z","usage":{"input":1000,"output":50,"cacheRead":0,"reasoning":10}}"#,
        ]
        .join("\n")
            + "\n"
    }

    #[test]
    fn discovery_claims_pi_files_by_fingerprint() {
        let dir = unique_dir("discover");
        std::fs::write(dir.join("a.jsonl"), session_lines()).unwrap();
        std::fs::write(dir.join("b.jsonl"), "{\"hello\":1}\n").unwrap();

        let found = PiAdapter.discover_members_in(&[dir], &|_| false).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].source_member_id, "p1");
        assert_eq!(found[0].kind, DiscoveredMemberKind::Root);
        assert_eq!(found[0].cwd.as_deref(), Some("/repo"));
    }

    /// Prose only; the injected env block and the toolResult message
    /// are not conversation; the compaction marker is a count.
    #[test]
    fn inspect_reports_missing_only_for_a_confirmed_absent_file() {
        let dir = unique_dir("inspect");
        let path = dir.join("a.jsonl");
        std::fs::write(&path, session_lines()).unwrap();
        let session = root_session(&path);
        assert_eq!(
            PiAdapter.inspect_session_source(&session).unwrap(),
            SourceAvailability::Present
        );
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            PiAdapter.inspect_session_source(&session).unwrap(),
            SourceAvailability::Missing
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Entries carrying `message.provider` / `message.model` (or lacking
    /// them) parse to the same prose either way since the provenance
    /// retirement.
    #[test]
    fn assistant_entries_parse_alike_with_or_without_model_fields() {
        let dir = unique_dir("prov");
        let file = dir.join("a.jsonl");
        let line = |id: &str, provider: Option<&str>, model: Option<&str>| {
            let mut m = serde_json::json!({
                "type": "message", "id": id, "parentId": "p",
                "timestamp": "2026-09-18T12:40:05.000Z",
                "message": { "role": "assistant", "content": [ { "type": "text", "text": "看完了。" } ] },
            });
            if let Some(p) = provider {
                m["message"]["provider"] = serde_json::json!(p);
            }
            if let Some(mo) = model {
                m["message"]["model"] = serde_json::json!(mo);
            }
            m.to_string()
        };
        std::fs::write(
            &file,
            [
                line("m1", Some("openai-codex"), Some("gpt-5.6-luna")),
                line("m2", None, None),
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();
        let delta = PiAdapter
            .read_session_delta(
                &root_session(&file),
                &crate::domain::SourceCursor::default(),
            )
            .unwrap();
        assert_eq!(delta.messages.len(), 2);
    }

    #[test]
    fn resume_prefers_the_source_file_path_over_the_session_id() {
        let adapter = PiAdapter;
        let install = crate::platform::exec_resolver::AgentInstallation {
            agent: Agent::Pi,
            executable_path: "/usr/local/bin/pi".into(),
            version: None,
            source: "test".into(),
            last_verified_at: "test".into(),
        };

        // A project-local session file: `--session <id>` cannot see it, the
        // path can. The path is the selector whenever we have one.
        let cmd = adapter
            .build_resume_command(
                &install,
                &crate::adapters::ExecOptions::default(),
                "01a0682c-d5ca-7fb5-8dd5-e9785dc380af",
                None,
                Some("/Users/x/projects/workspace/sessions/b9f1-cb4d.jsonl"),
            )
            .unwrap();
        assert_eq!(
            cmd.args,
            vec![
                "--session".to_string(),
                "/Users/x/projects/workspace/sessions/b9f1-cb4d.jsonl".to_string()
            ]
        );

        // Without a recorded path (should not happen for ingested sessions,
        // but stay honest) fall back to the id selector.
        let cmd = adapter
            .build_resume_command(
                &install,
                &crate::adapters::ExecOptions::default(),
                "01a0682c-d5ca-7fb5-8dd5-e9785dc380af",
                None,
                None,
            )
            .unwrap();
        assert_eq!(
            cmd.args,
            vec![
                "--session".to_string(),
                "01a0682c-d5ca-7fb5-8dd5-e9785dc380af".to_string()
            ]
        );
    }
}
