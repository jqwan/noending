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
    MemberObservation, ParsedLine, SessionMessageRole,
};
use crate::domain::{Agent, SessionMember, SessionMemberCursor, SourceAvailability};
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

    fn read_member_delta(
        &self,
        member: &SessionMember,
        cursor: &SessionMemberCursor,
    ) -> Result<crate::adapters::MemberReadDelta> {
        let path = PathBuf::from(&member.source_path);
        let is_root = member.relation.as_str() == "root";
        read_jsonl_delta(
            &path,
            cursor,
            crate::adapters::StatsCapabilities::TOOL_CALLS,
            &|_idx, v| parse_line(v, is_root),
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
        cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        let args = runtime_args(opts);
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
    ) -> Result<AgentCommand> {
        // pi [options] [--] [@files...] [messages...] — options first, then
        // the session selector.
        let mut args = runtime_args(opts);
        args.extend(["--session".into(), agent_session_id.into()]);
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
/// `message.usage` of one assistant call, as counts. The source states each
/// number directly — no derived sums: `input` excludes `cacheRead` / `cacheWrite`
/// and folding them in would change the source's individual axes.
fn usage_observation(msg: &Value) -> MemberObservation {
    let Some(usage) = msg.get("usage") else {
        return MemberObservation::default();
    };
    let n = |k: &str| usage.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    MemberObservation {
        input_tokens: n("input"),
        output_tokens: n("output"),
        cached_tokens: n("cacheRead"),
        reasoning_tokens: n("reasoning"),
        request_count: 1,
        ..Default::default()
    }
}

fn parse_line(v: &Value, is_root: bool) -> Option<ParsedLine> {
    let vtype = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let source_message_id = v.get("id").and_then(|s| s.as_str()).map(|s| s.to_string());

    match vtype {
        "message" => {
            let msg = v.get("message").unwrap_or(&Value::Null);
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
            let text = content_text(msg.get("content").unwrap_or(&Value::Null));
            // Usage rides on the assistant entry itself (verified: 63 usage
            // records, all `message`/`assistant`). Counting it before the text
            // gate keeps a usage-bearing entry from being dropped for having no
            // prose.
            let mut observation = usage_observation(msg);
            // Tool calls are initiated by assistant `toolCall` blocks — counted
            // (the call), never folded into the text.
            observation.tool_calls = msg
                .get("content")
                .and_then(|c| c.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("toolCall"))
                        .count() as u64
                })
                .unwrap_or(0);
            // The generation identity travels on assistant entries; the
            // ledger anchors need it on the no-message paths too.
            let model = if role == "assistant" {
                msg.get("model").and_then(|m| m.as_str()).map(String::from)
            } else {
                None
            };
            let provider = if role == "assistant" {
                msg.get("provider")
                    .and_then(|p| p.as_str())
                    .map(String::from)
            } else {
                None
            };
            if text.trim().is_empty() {
                return (!observation.is_empty()).then(|| {
                    ParsedLine::billed_without_message(
                        model,
                        crate::adapters::UsageCategory::Conversation,
                        observation,
                    )
                    .with_usage_provider(provider.clone())
                });
            }
            let role = match role {
                // pi has no injection concept — its UserMessage is plain
                // {role, content}, so nothing is filtered on the user arm.
                "user" => SessionMessageRole::User,
                "assistant" => SessionMessageRole::Assistant,
                // Tool output is a `toolResult` MESSAGE in pi; every other
                // non-conversation role is runtime chatter. Usage, when the
                // entry has any, is still counted.
                _ => {
                    return (!observation.is_empty()).then(|| {
                        ParsedLine::billed_without_message(
                            model,
                            crate::adapters::UsageCategory::Conversation,
                            observation,
                        )
                        .with_usage_provider(provider.clone())
                    })
                }
            };
            match role {
                SessionMessageRole::User => observation.user_messages = 1,
                SessionMessageRole::Assistant => observation.assistant_messages = 1,
            }
            if !is_root {
                return Some(
                    ParsedLine::billed_without_message(
                        model,
                        crate::adapters::UsageCategory::Conversation,
                        observation,
                    )
                    .with_usage_provider(provider.clone()),
                );
            }
            // The assistant entry itself carries `message.provider` /
            // `message.model`, the actual generation identity (present on every
            // assistant entry in the real corpus). Direct evidence wins over the
            // redundant `model_change` events.
            let (provider, model) = if role == SessionMessageRole::Assistant {
                (
                    msg.get("provider")
                        .and_then(|p| p.as_str())
                        .map(String::from),
                    msg.get("model").and_then(|m| m.as_str()).map(String::from),
                )
            } else {
                (None, None)
            };
            Some(ParsedLine {
                message: Some(
                    crate::adapters::parsed_message(source_message_id, role, text)
                        .with_provenance(provider, model),
                ),
                observation,
                usage_note: None,
            })
        }
        // A compaction is a billed model call: the entry carries its usage at
        // the top level, its own id is the stable ledger identity. The source
        // states no model/provider on it, so the event stays unattributed —
        // that is the source's truth, not a gap to invent values into.
        "compaction" | "compact" => {
            let observation = usage_observation(v);
            (!observation.is_empty()).then(|| {
                ParsedLine::billed_observation(
                    observation,
                    crate::adapters::UsageNote {
                        category: crate::adapters::UsageCategory::Compaction,
                        model: None,
                        provider: None,
                        key: source_message_id,
                    },
                )
            })
        }
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
        let usage = crate::adapters::test_usage_tokens(&delta.usage_events);
        match delta.stats {
            Some(StatsUpdate::Snapshot(s)) => {
                assert_eq!(s.tool_call_count, Some(1), "the assistant toolCall block");
                assert_eq!(s.user_message_count, Some(1));
                assert_eq!(s.assistant_message_count, Some(1));
                assert_eq!(s.side_activity_count, None);
                // Usage rides on the assistant entry; source cost is ignored.
                // The compaction call bills in the same ledger.
                assert_eq!(usage[0], 1746 + 1000);
                assert_eq!(usage[1], 210 + 50);
                assert_eq!(usage[2], 30, "cacheRead");
                assert_eq!(usage[3], 47 + 10);
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
        assert_eq!(delta.usage_events.len(), 2);
        assert_eq!(
            delta.usage_events[1].category,
            crate::adapters::UsageCategory::Compaction
        );
        assert_eq!(delta.usage_events[1].key.as_deref(), Some("c1"));
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

    /// Direct evidence: the assistant entry's own `message.provider` /
    /// `message.model` land on the message; an entry without them stays NULL.
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
    #[test]
    fn billed_generations_keep_their_channels_without_conversation_messages() {
        for is_root in [true, false] {
            for (provider, content) in [
                (
                    "openai",
                    serde_json::json!([{"type":"text","text":"answer"}]),
                ),
                (
                    "302ai",
                    serde_json::json!([{"type":"toolCall","name":"read"}]),
                ),
            ] {
                let source = serde_json::json!({"type":"message", "message":{
                    "role":"assistant", "provider":provider, "model":"gpt-test", "content":content,
                    "usage":{"input":100,"output":10}
                }});
                let parsed = parse_line(&source, is_root).unwrap();
                let event = crate::adapters::UsageEvent::from_parsed_line(&parsed, None).unwrap();
                assert_eq!(event.provider.as_deref(), Some(provider));
                assert_eq!(event.model.as_deref(), Some("gpt-test"));
                if !is_root || provider == "302ai" {
                    assert!(parsed.message.is_none());
                }
            }
        }
    }
}
