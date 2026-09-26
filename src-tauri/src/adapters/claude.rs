//! Claude Code Adapter: `~/.claude/projects/<encoded-cwd>/<session>.jsonl`.
//! `CLAUDE_CONFIG_DIR` overrides the root. Raw transcripts stay untouched,
//! always — NoEnding never deletes an Agent-owned source.
//!
//! Member mapping: the main transcript is the ROOT member. Claude interleaves
//! `isSidechain=true` lines into the same file but gives them no stable
//! execution identity of their own, so no side member is fabricated: they are
//! counted as `side_activity` and their text never becomes conversation.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::{
    detect_format, read_jsonl_delta, AgentCommand, DiscoveredMember, DiscoveredMemberKind,
    MemberObservation, ParsedLine, SessionMessageRole,
};
use crate::domain::{Agent, SessionMember, SessionMemberCursor, SourceAvailability};
use crate::error::Result;
use crate::platform::exec_resolver::{self, AgentInstallation};

pub struct ClaudeAdapter;

/// Conversation text: `text` blocks only. `tool_use` / `tool_result` blocks
/// are machine traffic — counted, never folded into the message.
fn content_parts(content: &Value) -> (String, u64) {
    match content {
        Value::String(s) => (s.clone(), 0),
        Value::Array(arr) => {
            let mut parts = Vec::new();
            let mut tool_calls = 0u64;
            for item in arr {
                match item.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                            parts.push(t.to_string());
                        }
                    }
                    Some("tool_use") => tool_calls += 1,
                    // tool_result and everything else: no text, no count —
                    // the call itself was already counted on the assistant
                    // side.
                    _ => {}
                }
            }
            (parts.join("\n"), tool_calls)
        }
        _ => (String::new(), 0),
    }
}

impl ClaudeAdapter {
    fn parse_member(path: &Path) -> Result<Option<DiscoveredMember>> {
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
                    let (text, _) = content_parts(msg.get("content").unwrap_or(&Value::Null));
                    // `<…>` environment blocks and `#`-prefixed injections
                    // (AGENTS.md, attached-file headers) are not user text.
                    if !text.is_empty() && !crate::adapters::is_injected_preamble(&text) {
                        first_user_text = Some(crate::adapters::truncate_text(&text, 400));
                    }
                }
            }
            // Last resort for a title when the session has no user turn.
            if first_agent_text.is_none()
                && v.get("type").and_then(|t| t.as_str()) == Some("assistant")
                && v.get("isSidechain").and_then(|s| s.as_bool()) != Some(true)
            {
                if let Some(msg) = v.get("message") {
                    let (text, _) = content_parts(msg.get("content").unwrap_or(&Value::Null));
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

        Ok(Some(DiscoveredMember {
            agent: Agent::ClaudeCode,
            source_member_id: session_id.unwrap_or_else(|| file_name.clone()),
            kind: DiscoveredMemberKind::Root,
            parent_source_member_id: None,
            root_hint: None,
            source_kind: "claude_code_transcript".into(),
            source_path: path.to_path_buf(),
            cwd,
            started_at,
            last_activity_at: last_activity.or(last_ts),
            // Claude Code writes no session title.
            native_title: None,
            first_user_text,
            first_agent_text,
            metadata: serde_json::json!({}),
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
                if detect_format(&p) != Some(Agent::ClaudeCode) {
                    eprintln!(
                        "[discover] skip {} (content fingerprint is not claude_code)",
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
            crate::adapters::StatsCapabilities::TOOL_COMPACTION_AND_SIDE_ACTIVITY,
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
        let mut args = vec!["--resume".into(), agent_session_id.into()];
        args.extend(runtime_args(opts));
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

    fn build_context_extraction_command(
        &self,
        install: &AgentInstallation,
        opts: &crate::adapters::ExecOptions,
        prompt: &str,
        runtime_dir: &Path,
    ) -> Result<AgentCommand> {
        let mut args: Vec<String> = vec![
            "-p".into(),
            "--no-session-persistence".into(),
            "--output-format".into(),
            "text".into(),
        ];
        args.extend(runtime_args(opts));
        args.push(prompt.to_string());
        Ok(AgentCommand {
            program: install.executable_path.clone(),
            args,
            cwd: Some(runtime_dir.to_path_buf()),
        })
    }
}

/// One transcript line's contribution. `is_root` is false only for a
/// hypothetical non-root Claude member (none exists today); the flag keeps the
/// "child text is never conversation" rule explicit rather than implied.
fn parse_line(v: &Value, is_root: bool) -> Option<ParsedLine> {
    let vtype = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let sidechain = v
        .get("isSidechain")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);
    let source_message_id = v
        .get("uuid")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());

    match vtype {
        "user" | "assistant" => {
            let msg = v.get("message").unwrap_or(&Value::Null);
            let (text, tool_calls) = content_parts(msg.get("content").unwrap_or(&Value::Null));
            if sidechain {
                // Sub-agent chatter in the same file: no stable identity → no
                // member, no message — observed only.
                return Some(ParsedLine::observation_only(MemberObservation {
                    side_activity: 1,
                    ..Default::default()
                }));
            }
            let observation = MemberObservation {
                tool_calls,
                ..Default::default()
            };
            // isMeta lines are runtime output (command results), not turns.
            let is_meta = v.get("isMeta").and_then(|s| s.as_bool()).unwrap_or(false);
            if text.trim().is_empty() || is_meta || !is_root {
                return Some(ParsedLine::observation_only(observation));
            }
            let role = if vtype == "user" {
                SessionMessageRole::User
            } else {
                SessionMessageRole::Assistant
            };
            // Injected context is never conversation.
            if role == SessionMessageRole::User && crate::adapters::is_injected_preamble(&text) {
                return Some(ParsedLine::observation_only(observation));
            }
            // Every assistant row carries `message.model` — the actual
            // response model in the API-style envelope (verified: 0 rows
            // without it), which may be a gateway spelling like
            // "qwen/qwen3.8-27b" rather than a branding name. No provider
            // field exists in the source.
            let model = if role == SessionMessageRole::Assistant {
                msg.get("model")
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string())
            } else {
                None
            };
            Some(ParsedLine {
                message: Some(
                    crate::adapters::parsed_message(source_message_id, role, text)
                        .with_provenance(None, model),
                ),
                observation,
            })
        }
        // A compaction summary line is a boundary marker, never the
        // pruned content itself.
        "summary" => Some(ParsedLine::observation_only(MemberObservation {
            compactions: 1,
            ..Default::default()
        })),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::{AgentAdapter, MemberReadDelta};
    use crate::domain::StatsUpdate;

    fn unique_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("noending-claude-{}-{}", tag, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn root_member(path: &Path) -> SessionMember {
        SessionMember {
            id: "mem-claude".into(),
            session_id: "sess-claude".into(),
            agent: Agent::ClaudeCode,
            source_member_id: "s1".into(),
            relation: crate::domain::SessionMemberRelation::Root,
            parent_source_member_id: None,
            source_kind: "claude_code_transcript".into(),
            source_path: path.to_string_lossy().to_string(),
            cwd: None,
            started_at: None,
            last_activity_at: None,
            metadata: serde_json::json!({}),
        }
    }

    fn member_line(idx: usize, vtype: &str, extra: &str, role: &str, text: &str) -> String {
        let mut v = serde_json::json!({
            "type": vtype,
            "uuid": format!("u{idx}"),
            "sessionId": "s1",
            "cwd": "/repo",
            "timestamp": format!("2026-09-22T15:01:{idx:02}.000Z"),
            "message": { "role": role, "content": [{ "type": "text", "text": text }] },
        });
        if !extra.is_empty() {
            if let Some(fields) = serde_json::from_str::<serde_json::Value>(extra)
                .ok()
                .and_then(|f| f.as_object().cloned())
            {
                for (k, val) in fields {
                    v[k] = val;
                }
            }
        }
        v.to_string()
    }

    /// Prose in, machine traffic out: sidechain text becomes a side
    /// activity count, a tool_use block a tool call, `summary` a compaction,
    /// and two visible assistant prose segments around a tool call both stay.
    #[test]
    fn the_root_read_keeps_prose_and_counts_the_rest() {
        let dir = unique_dir("root-read");
        let path = dir.join("s1.jsonl");
        std::fs::write(
            &path,
            [
                member_line(1, "user", "", "user", "改一下详情页"),
                r#"{"type":"assistant","uuid":"u2","sessionId":"s1","cwd":"/repo","message":{"role":"assistant","content":[{"type":"text","text":"我先看现状。"},{"type":"tool_use","name":"Read","id":"t1"}]}}"#.to_string(),
                r#"{"type":"user","uuid":"u3","sessionId":"s1","cwd":"/repo","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"contents"}]}}"#.to_string(),
                r#"{"type":"user","uuid":"u4","sessionId":"s1","isSidechain":true,"cwd":"/repo","message":{"role":"user","content":[{"type":"text","text":"子任务的问题"}]}}"#.to_string(),
                r#"{"type":"assistant","uuid":"u5","sessionId":"s1","isSidechain":true,"cwd":"/repo","message":{"role":"assistant","content":[{"type":"text","text":"子任务的结论"}]}}"#.to_string(),
                member_line(6, "assistant", "", "assistant", "问题在这里，已经修改完成。"),
                r#"{"type":"summary","uuid":"u7","sessionId":"s1","summary":"compressed"}"#.to_string(),
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();

        let delta: MemberReadDelta = ClaudeAdapter
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
                (SessionMessageRole::User, "改一下详情页"),
                (SessionMessageRole::Assistant, "我先看现状。"),
                (SessionMessageRole::Assistant, "问题在这里，已经修改完成。"),
            ],
            "sidechain text and tool results never become conversation"
        );
        match delta.stats {
            Some(StatsUpdate::Snapshot(s)) => {
                assert_eq!(s.tool_call_count, Some(1));
                assert_eq!(s.compaction_count, Some(1));
                assert_eq!(s.side_activity_count, Some(2), "the two sidechain lines");
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// isMeta user lines are runtime output, and `<…>`/`#` preambles are
    /// injected context — neither is a human turn.
    #[test]
    fn meta_and_injected_user_lines_are_not_conversation() {
        let dir = unique_dir("meta");
        let path = dir.join("s1.jsonl");
        std::fs::write(
            &path,
            [
                r#"{"type":"user","uuid":"u1","isMeta":true,"sessionId":"s1","cwd":"/repo","message":{"role":"user","content":[{"type":"text","text":"<command-name>/clear</command-name>"}]}}"#,
                r#"{"type":"user","uuid":"u2","sessionId":"s1","cwd":"/repo","message":{"role":"user","content":[{"type":"text","text":"<system-reminder>context</system-reminder>"}]}}"#,
                r#"{"type":"user","uuid":"u3","sessionId":"s1","cwd":"/repo","message":{"role":"user","content":[{"type":"text","text":"真正的问题"}]}}"#,
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();
        let delta: MemberReadDelta = ClaudeAdapter
            .read_member_delta(&root_member(&path), &SessionMemberCursor::default())
            .unwrap();
        assert_eq!(delta.messages.len(), 1);
        assert_eq!(delta.messages[0].content, "真正的问题");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn discovery_reads_session_facts_and_skips_foreign_files() {
        let dir = unique_dir("discover");
        let good = dir.join("s1.jsonl");
        std::fs::write(
            &good,
            [
                member_line(1, "user", "", "user", "把日报整理一下"),
                member_line(2, "assistant", "", "assistant", "整理好了。"),
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();
        // A foreign shape is refused by the fingerprint.
        let foreign = dir.join("x.jsonl");
        std::fs::write(&foreign, "{\"hello\":\"world\"}\n").unwrap();

        let found = ClaudeAdapter
            .discover_members_in(&[dir], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, DiscoveredMemberKind::Root);
        assert_eq!(found[0].source_member_id, "s1");
        assert_eq!(found[0].cwd.as_deref(), Some("/repo"));
        assert_eq!(found[0].first_user_text.as_deref(), Some("把日报整理一下"));
    }

    #[test]
    fn inspect_reports_missing_only_for_a_confirmed_absent_file() {
        let dir = unique_dir("inspect");
        let path = dir.join("s1.jsonl");
        std::fs::write(&path, member_line(1, "user", "", "user", "hi")).unwrap();
        let member = root_member(&path);
        assert_eq!(
            ClaudeAdapter.inspect_member_source(&member).unwrap(),
            SourceAvailability::Present
        );
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            ClaudeAdapter.inspect_member_source(&member).unwrap(),
            SourceAvailability::Missing
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Direct evidence: the assistant row's `message.model` lands on the
    /// message; a missing field stays NULL and provider stays NULL.
    #[test]
    fn assistant_messages_carry_their_source_model() {
        let dir = unique_dir("prov");
        let file = dir.join("s.jsonl");
        let line = |idx: usize, model: Option<&str>| {
            let mut v = serde_json::json!({
                "type": "assistant",
                "uuid": format!("u{idx}"),
                "timestamp": "2026-09-22T15:01:00.000Z",
                "message": { "role": "assistant", "content": [ { "type": "text", "text": "回答" } ] },
            });
            if let Some(m) = model {
                v["message"]["model"] = serde_json::json!(m);
            }
            v.to_string()
        };
        std::fs::write(
            &file,
            [line(1, Some("qwen/qwen3.8-27b")), line(2, None)].join("\n") + "\n",
        )
        .unwrap();
        let delta = ClaudeAdapter
            .read_member_delta(&root_member(&file), &SessionMemberCursor::default())
            .unwrap();
        assert_eq!(delta.messages.len(), 2);
        assert_eq!(delta.messages[0].model.as_deref(), Some("qwen/qwen3.8-27b"));
        assert_eq!(
            delta.messages[0].provider, None,
            "no provider field in the source → NULL, never branding"
        );
        assert_eq!(delta.messages[1].model, None, "missing field stays NULL");
    }
}
