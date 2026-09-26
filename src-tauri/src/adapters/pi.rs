//! Pi Adapter: `~/.pi/agent/sessions/<encoded-cwd>/<ts>_<uuid>.jsonl`.
//! `PI_HOME` overrides the root. Pi sessions are plain JSONL trees, read-only
//! always — NoEnding never deletes an Agent-owned source.
//!
//! Member mapping: today's sources are single-root — one transcript,
//! one ROOT member. The existing conversation filters stay: thinking blocks,
//! tool traffic and runtime injections never become conversation; a
//! compaction marker is an observation. If a future source exposes child/side
//! identities, Member discovery extends — the Conversation schema does not.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::{
    detect_format, read_jsonl_delta, AgentCommand, DiscoveredMember, DiscoveredMemberKind,
    MemberObservation, ParsedLine, SessionMessageRole,
};
use crate::domain::{Agent, SessionMember, SessionMemberCursor, SourceAvailability};
use crate::error::Result;
use crate::platform::exec_resolver::{self, AgentInstallation};

pub struct PiAdapter;

/// Text parts only: `thinking` blocks are model reasoning, and tool calls /
/// results are deliberately dropped — they used to be appended
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
    fn parse_member(path: &Path) -> Result<Option<DiscoveredMember>> {
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
                        // Last resort for a title.
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
            // pi writes no session title.
            native_title: None,
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

    fn read_member_delta(
        &self,
        member: &SessionMember,
        cursor: &SessionMemberCursor,
    ) -> Result<crate::adapters::MemberReadDelta> {
        let path = PathBuf::from(&member.source_path);
        read_jsonl_delta(
            &path,
            cursor,
            crate::adapters::StatsCapabilities::COMPACTION,
            &|_idx, v| parse_line(v, member.relation.as_str() == "root"),
        )
    }

    fn inspect_member_source(&self, member: &SessionMember) -> Result<SourceAvailability> {
        Ok(crate::adapters::inspect_file_source(Path::new(
            &member.source_path,
        )))
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
            match (role, is_root) {
                ("user", true) if !crate::adapters::is_injected_preamble(&text) => {
                    Some(ParsedLine::message_only(crate::adapters::parsed_message(
                        source_message_id,
                        SessionMessageRole::User,
                        text,
                    )))
                }
                ("assistant", true) => {
                    // Message provenance: the
                    // assistant entry itself carries `message.provider` and
                    // `message.model` — the actual generation identity
                    // (verified: 1203/1203 assistant entries in the real
                    // corpus carry both). Direct evidence wins over the
                    // `model_change` events, which exist in the file but are
                    // redundant here.
                    Some(ParsedLine::message_only(
                        crate::adapters::parsed_message(
                            source_message_id,
                            SessionMessageRole::Assistant,
                            text,
                        )
                        .with_provenance(
                            msg.get("provider")
                                .and_then(|p| p.as_str())
                                .map(String::from),
                            msg.get("model").and_then(|m| m.as_str()).map(String::from),
                        ),
                    ))
                }
                // Tool output is a `toolResult` MESSAGE in pi; every other
                // non-conversation role is runtime chatter.
                _ => None,
            }
        }
        "compaction" | "compact" => Some(ParsedLine::observation_only(MemberObservation {
            compactions: 1,
            ..Default::default()
        })),
        _ => None, // session headers and bookkeeping carry nothing
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::AgentAdapter;
    use crate::domain::{SessionMemberRelation, StatsUpdate};

    fn unique_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("noending-pi-{}-{}", tag, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn root_member(path: &Path) -> SessionMember {
        SessionMember {
            id: "mem-pi".into(),
            session_id: "sess-pi".into(),
            agent: Agent::Pi,
            source_member_id: "p1".into(),
            relation: SessionMemberRelation::Root,
            parent_source_member_id: None,
            source_kind: "pi_session_transcript".into(),
            source_path: path.to_string_lossy().to_string(),
            cwd: None,
            started_at: None,
            last_activity_at: None,
            metadata: serde_json::json!({}),
        }
    }

    fn session_lines() -> String {
        [
            r#"{"type":"session","id":"p1","cwd":"/repo","version":3,"timestamp":"2026-09-18T12:40:00.000Z"}"#,
            r#"{"type":"message","id":"m1","parentId":"p1","timestamp":"2026-09-18T12:40:03.000Z","message":{"role":"user","content":[{"type":"text","text":"帮我看看这个"}]}}"#,
            r#"{"type":"message","id":"m2","parentId":"m1","timestamp":"2026-09-18T12:40:05.000Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"…"},{"type":"text","text":"看完了。"}]}}"#,
            r#"{"type":"message","id":"m3","parentId":"m2","timestamp":"2026-09-18T12:40:06.000Z","message":{"role":"toolResult","content":[{"type":"text","text":"file contents"}]}}"#,
            r#"{"type":"compaction","id":"c1","parentId":"m3","timestamp":"2026-09-18T12:40:07.000Z"}"#,
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
    fn the_root_read_keeps_prose_and_counts_compaction() {
        let dir = unique_dir("parse");
        let path = dir.join("a.jsonl");
        std::fs::write(&path, session_lines()).unwrap();

        let delta = PiAdapter
            .read_member_delta(&root_member(&path), &SessionMemberCursor::default())
            .unwrap();
        let texts: Vec<(SessionMessageRole, &str)> = delta
            .messages
            .iter()
            .map(|m| (m.role, m.content.as_str()))
            .collect();
        assert_eq!(
            texts,
            vec![
                (SessionMessageRole::User, "帮我看看这个"),
                (SessionMessageRole::Assistant, "看完了。")
            ],
            "the toolResult message stays out of the conversation"
        );
        assert_eq!(delta.messages[0].source_message_id.as_deref(), Some("m1"));
        match delta.stats {
            Some(StatsUpdate::Snapshot(s)) => {
                assert_eq!(s.compaction_count, Some(1));
                assert_eq!(s.tool_call_count, None);
                assert_eq!(s.tool_error_count, None);
                assert_eq!(s.side_activity_count, None);
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inspect_reports_missing_only_for_a_confirmed_absent_file() {
        let dir = unique_dir("inspect");
        let path = dir.join("a.jsonl");
        std::fs::write(&path, session_lines()).unwrap();
        let member = root_member(&path);
        assert_eq!(
            PiAdapter.inspect_member_source(&member).unwrap(),
            SourceAvailability::Present
        );
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            PiAdapter.inspect_member_source(&member).unwrap(),
            SourceAvailability::Missing
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Direct evidence: the assistant entry's own
    /// `message.provider` / `message.model` land on the message; an entry
    /// without them stays NULL.
    #[test]
    fn assistant_entries_carry_source_provider_and_model() {
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
            .read_member_delta(&root_member(&file), &SessionMemberCursor::default())
            .unwrap();
        assert_eq!(delta.messages.len(), 2);
        assert_eq!(delta.messages[0].provider.as_deref(), Some("openai-codex"));
        assert_eq!(delta.messages[0].model.as_deref(), Some("gpt-5.6-luna"));
        assert_eq!(delta.messages[1].provider, None);
        assert_eq!(delta.messages[1].model, None, "missing fields stay NULL");
    }
}
