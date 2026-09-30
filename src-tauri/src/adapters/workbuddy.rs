//! WorkBuddy Adapter: `~/.workbuddy/projects/<slug>/<sessionId>.jsonl`.
//!
//! An Electron app (Tencent's WorkBuddy) whose conversation store is
//! append-only JSONL with no session header: the first line is already a
//! message, and `sessionId` / `cwd` repeat on every line.
//!
//! A user turn is wrapped in a `<system-reminder data-role="user-context">`
//! envelope with the prompt in `<user_query>…</user_query>` at the end, so only
//! that body is the human text. Compaction replays
//! (`<conversation_history_summary>`, `<cb_summary>`) are also `user`-role but
//! count as compaction; any other `<`-prefixed body is system noise and drops.
//!
//! Provenance: the row's `providerData.model` (mirrored by
//! `requestModelId`/`requestModelName`) is the model that answered — a real
//! registry id like `deepseek-v4.1-flash`, present on most rows (locally:
//! 86.5%). A row written under the `auto` preference says `auto` in all
//! three fields and the actual resolution is NOT recoverable from anywhere in
//! the transcript, so `auto` attributes nothing (locally: 13.5%). No
//! provider field exists → NULL.
//!
//! One root transcript, one ROOT member. `ai-title` supplies the native title
//! (last written wins); `function_call` / `function_call_result` are
//! observations, not text. There is no CLI — the bundle's only executable is
//! Electron — so this adapter ingests history only: `detect()` never succeeds.
//!
//! Sub-agents are SEPARATE files under the session's own directory,
//! `<slug>/<sessionId>/subagents/agent-<hex>.jsonl`, and become CHILD members
//! of that session. The directory is the only parent link: the child file's
//! `sessionId` is a fresh uuid and its `parentId` chain points at messages
//! inside the same file, so neither identifies the session it belongs to.
//! Children never contribute conversation — `parse_line` is root-gated.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::{
    detect_format, ms_epoch_to_rfc3339, read_jsonl_delta, AgentCommand, DesktopResume,
    DiscoveredMember, DiscoveredMemberKind, ExecOptions, MemberObservation, ParsedLine,
    ResumeRoute, SessionMessageRole,
};
use crate::domain::{Agent, SessionMember, SessionMemberCursor, SourceAvailability};
use crate::error::{other, Result};

pub struct WorkBuddyAdapter;

/// Text of a message's content blocks: `input_text` / `output_text` only.
fn content_text(content: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(arr) = content.as_array() {
        for item in arr {
            if matches!(
                item.get("type").and_then(|t| t.as_str()),
                Some("input_text") | Some("output_text")
            ) {
                if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                    parts.push(t.to_string());
                }
            }
        }
    } else if let Some(s) = content.as_str() {
        parts.push(s.to_string());
    }
    parts.join("\n")
}

fn timestamp_of(v: &Value) -> Option<String> {
    v.get("timestamp")
        .and_then(|t| t.as_i64())
        .and_then(ms_epoch_to_rfc3339)
}

/// The human prompt inside WorkBuddy's user-turn envelope, if there is one.
fn extract_user_query(text: &str) -> Option<String> {
    const OPEN: &str = "<user_query>";
    const CLOSE: &str = "</user_query>";
    let rest = &text[text.find(OPEN)? + OPEN.len()..];
    let inner = match rest.find(CLOSE) {
        Some(end) => &rest[..end],
        None => rest,
    };
    let inner = inner.trim();
    (!inner.is_empty()).then(|| inner.to_string())
}

/// The compaction replays that arrive as `user`-role messages but are machine
/// context, not a turn.
fn is_compaction_block(text: &str) -> bool {
    text.starts_with("<conversation_history_summary") || text.starts_with("<cb_summary")
}

/// What a `user`-role message really is (see the module doc).
enum UserTurn {
    Prompt(String),
    Compaction,
    Envelope,
}

fn classify_user_turn(raw: &str) -> UserTurn {
    // Compaction first: a replay can quote the user's words (and even a
    // `<user_query>` tag) inside its own block, and it is still not a turn.
    if is_compaction_block(raw) {
        return UserTurn::Compaction;
    }
    if let Some(prompt) = extract_user_query(raw) {
        return UserTurn::Prompt(prompt);
    }
    if crate::adapters::is_injected_preamble(raw) {
        return UserTurn::Envelope;
    }
    UserTurn::Prompt(raw.to_string())
}

impl WorkBuddyAdapter {
    /// `<slug>/<sessionId>/subagents/agent-*.jsonl` → the parent session id
    /// (the enclosing directory). The shape is the marker: a child file's head
    /// is plain WorkBuddy-shaped, so no content test can decide this.
    fn subagent_member(path: &Path) -> Option<String> {
        let name = path.file_name()?.to_str()?;
        if !name.starts_with("agent-") || !name.ends_with(".jsonl") {
            return None;
        }
        let subagents_dir = path.parent()?;
        if subagents_dir.file_name()?.to_str()? != "subagents" {
            return None;
        }
        Some(subagents_dir.parent()?.file_name()?.to_str()?.to_string())
    }

    /// A sub-agent transcript as its own CHILD member. Identity is derived
    /// (`<parent>:subagent:<stem>`) because the file carries no stable link to
    /// its session. No title sources — a child never names a Logical Session.
    fn parse_subagent_member(path: &Path, parent_id: &str) -> Result<Option<DiscoveredMember>> {
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            return Ok(None);
        };
        let meta = std::fs::metadata(path)?;
        let last_activity = meta
            .modified()
            .ok()
            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());
        Ok(Some(DiscoveredMember {
            agent: Agent::WorkBuddy,
            source_member_id: format!("{parent_id}:subagent:{stem}"),
            kind: DiscoveredMemberKind::Child,
            parent_source_member_id: Some(parent_id.to_string()),
            root_hint: Some(parent_id.to_string()),
            source_kind: "workbuddy_subagent_transcript".into(),
            source_path: path.to_path_buf(),
            // A child's cwd / start time are execution fact only, and the
            // child never names the session.
            cwd: None,
            started_at: None,
            last_activity_at: last_activity,
            native_title: None,
            first_user_text: None,
            first_agent_text: None,
            metadata: serde_json::json!({ "parent_session": parent_id }),
        }))
    }

    fn parse_member(path: &Path) -> Result<Option<DiscoveredMember>> {
        let file_name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .ok_or_else(|| other("无效的 session 文件名"))?;

        let lines = crate::adapters::read_jsonl_lines(path)?;
        let mut session_id: Option<String> = None;
        let mut cwd: Option<String> = None;
        let mut started_at: Option<String> = None;
        let mut last_ts: Option<String> = None;
        let mut native_title = None;
        let mut first_user_text = None;
        let mut first_agent_text = None;

        for (_, line) in &lines {
            let v: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if session_id.is_none() {
                session_id = v
                    .get("sessionId")
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string());
            }
            if cwd.is_none() {
                cwd = v
                    .get("cwd")
                    .and_then(|c| c.as_str())
                    .filter(|c| !c.is_empty())
                    .map(|c| c.to_string());
            }
            if let Some(ts) = timestamp_of(&v) {
                if started_at.is_none() {
                    started_at = Some(ts.clone());
                }
                last_ts = Some(ts);
            }
            // WorkBuddy rewrites its `ai-title` as the conversation moves on,
            // so the last one written is the Agent's final word on the session.
            if v.get("type").and_then(|t| t.as_str()) == Some("ai-title") {
                if let Some(t) = v
                    .get("aiTitle")
                    .and_then(|t| t.as_str())
                    .filter(|t| !t.trim().is_empty())
                {
                    native_title = Some(t.to_string());
                }
            }
            if v.get("type").and_then(|t| t.as_str()) == Some("message") {
                let role = v.get("role").and_then(|r| r.as_str()).unwrap_or("");
                if first_user_text.is_none() && role == "user" {
                    let raw = content_text(v.get("content").unwrap_or(&Value::Null));
                    if let UserTurn::Prompt(text) = classify_user_turn(&raw) {
                        first_user_text = Some(crate::adapters::truncate_text(&text, 400));
                    }
                }
                // Last resort for a title.
                if first_agent_text.is_none() && role == "assistant" {
                    let raw = content_text(v.get("content").unwrap_or(&Value::Null));
                    if !raw.trim().is_empty() {
                        first_agent_text = Some(crate::adapters::truncate_text(&raw, 400));
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
            agent: Agent::WorkBuddy,
            source_member_id: session_id.unwrap_or(file_name),
            kind: DiscoveredMemberKind::Root,
            parent_source_member_id: None,
            root_hint: None,
            source_kind: "workbuddy_transcript".into(),
            source_path: path.to_path_buf(),
            cwd,
            started_at,
            last_activity_at: last_activity.or(last_ts),
            native_title,
            first_user_text,
            first_agent_text,
            metadata: serde_json::json!({}),
        }))
    }
}

impl crate::adapters::AgentAdapter for WorkBuddyAdapter {
    fn agent(&self) -> Agent {
        Agent::WorkBuddy
    }

    /// Always `None`: an Electron GUI with no CLI to run.
    fn detect(&self) -> Option<crate::platform::exec_resolver::AgentInstallation> {
        None
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
                if unchanged(&p) {
                    continue;
                }
                if let Some(parent_id) = Self::subagent_member(&p) {
                    match Self::parse_subagent_member(&p, &parent_id) {
                        Ok(Some(m)) => out.push(m),
                        Ok(None) => {}
                        Err(e) => eprintln!("[discover] skip {}: {}", p.display(), e),
                    }
                    continue;
                }
                if detect_format(&p) != Some(Agent::WorkBuddy) {
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
            crate::adapters::StatsCapabilities::COMPACTION.with_tokens(),
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
        _install: &crate::platform::exec_resolver::AgentInstallation,
        _opts: &ExecOptions,
        _cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        Err(other("WorkBuddy 是 GUI 应用，没有可启动的 CLI"))
    }

    fn build_resume_command(
        &self,
        _install: &crate::platform::exec_resolver::AgentInstallation,
        _opts: &ExecOptions,
        _agent_session_id: &str,
        _cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        Err(other("WorkBuddy 是 GUI 应用，没有可启动的 CLI"))
    }

    fn build_exec_command(
        &self,
        _install: &crate::platform::exec_resolver::AgentInstallation,
        _opts: &ExecOptions,
        _prompt: &str,
    ) -> Result<AgentCommand> {
        Err(other("WorkBuddy 是 GUI 应用，没有可启动的 CLI"))
    }

    /// WorkBuddy's own task notifications deep-link `workbuddy://chat/{id}`,
    /// and the id is the transcript file's stem — this member's
    /// source_member_id. The one app whose Continue can land ON the
    /// conversation. App absent → refuse (there is no CLI to fall back to).
    fn desktop_app_name(&self) -> Option<&'static str> {
        Some("WorkBuddy")
    }

    fn continue_route(&self, member: &SessionMember) -> ResumeRoute {
        if !crate::platform::paths::app_bundle_present("WorkBuddy") {
            return ResumeRoute::Refused("未找到 WorkBuddy 桌面应用，无法继续该会话".into());
        }
        ResumeRoute::Desktop(DesktopResume {
            uri: format!("workbuddy://chat/{}", member.source_member_id),
            note: "将在 WorkBuddy 中打开并定位到该会话".into(),
        })
    }
}

/// `providerData.usage` of one model call, as counts. The source states each
/// number directly (`inputTokens` excludes the cached ones; the details arrays
/// name the cached / reasoning parts) — no derived sums.
fn detail_sum(usage: &Value, key: &str, field: &str) -> u64 {
    usage
        .get(key)
        .and_then(|d| d.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|i| i.get(field))
                .filter_map(|v| v.as_u64())
                .sum()
        })
        .unwrap_or(0)
}

fn usage_observation(v: &Value) -> MemberObservation {
    let provider = v.get("providerData");
    if let Some(usage) = provider
        .and_then(|p| p.get("usage"))
        .filter(|u| u.is_object())
    {
        let n = |k: &str| usage.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
        return MemberObservation {
            input_tokens: n("inputTokens"),
            output_tokens: n("outputTokens"),
            cached_tokens: detail_sum(usage, "inputTokensDetails", "cached_tokens"),
            reasoning_tokens: detail_sum(usage, "outputTokensDetails", "reasoning_tokens"),
            ..Default::default()
        };
    }
    // Some rows carry only `rawUsage`, the zhipu/OpenAI-style mirror of the
    // same call (`prompt_tokens` == `inputTokens`; locally the two always
    // ship together, but a build that writes only the raw shape must not
    // bill zero). Same call, same components, snake_case spelling.
    if let Some(raw) = provider
        .and_then(|p| p.get("rawUsage"))
        .filter(|u| u.is_object())
    {
        let n = |k: &str| raw.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
        let detail = |obj: &str, key: &str| {
            raw.get(obj)
                .and_then(|d| d.get(0).or_else(|| d.as_object().map(|_| d)))
                .and_then(|d| d.get(key))
                .and_then(|x| x.as_u64())
                .unwrap_or(0)
        };
        return MemberObservation {
            input_tokens: n("prompt_tokens"),
            output_tokens: n("completion_tokens"),
            cached_tokens: detail("prompt_tokens_details", "cached_tokens")
                .max(n("prompt_cache_hit_tokens")),
            reasoning_tokens: detail("completion_tokens_details", "reasoning_tokens")
                .max(n("completion_thinking_tokens")),
            ..Default::default()
        };
    }
    MemberObservation::default()
}

/// One line's contribution: message lines only, prose only, machine
/// traffic counted. Usage is the exception to the type gate — WorkBuddy hangs
/// it on whichever record made the call (a `function_call` for a tool turn,
/// the final assistant message for a text turn), and the two never share one
/// call, so every record that has usage is counted.
fn parse_line(v: &Value, is_root: bool) -> Option<ParsedLine> {
    let vtype = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if vtype == "function_call" {
        let usage = usage_observation(v);
        return (!usage.is_empty()).then(|| ParsedLine::observation_only(usage));
    }
    if vtype != "message" {
        return None;
    }
    let source_message_id = v.get("id").and_then(|s| s.as_str()).map(|s| s.to_string());
    let raw = content_text(v.get("content").unwrap_or(&Value::Null));
    let mut observation = usage_observation(v);
    if raw.trim().is_empty() {
        return (!observation.is_empty()).then(|| ParsedLine::observation_only(observation));
    }
    match v.get("role").and_then(|r| r.as_str()).unwrap_or("") {
        "user" => match classify_user_turn(&raw) {
            UserTurn::Prompt(p) => {
                observation.user_messages = 1;
                if !is_root {
                    return Some(ParsedLine::observation_only(observation));
                }
                Some(ParsedLine {
                    message: Some(crate::adapters::parsed_message(
                        source_message_id,
                        SessionMessageRole::User,
                        p,
                    )),
                    observation,
                })
            }
            // A compaction replay is a boundary count, never content.
            UserTurn::Compaction => Some(ParsedLine::observation_only(MemberObservation {
                compactions: 1,
                ..Default::default()
            })),
            _ => Some(ParsedLine::observation_only(observation)),
        },
        "assistant" => {
            observation.assistant_messages = 1;
            if !is_root {
                return Some(ParsedLine::observation_only(observation));
            }
            // The row's own `providerData.model` is the answering model's
            // registry id (`deepseek-v4.1-flash`, …). The `auto` preference
            // reuses the same slot and its resolution is not written anywhere
            // in the transcript — an `auto` row attributes nothing. No
            // provider field exists in the source → NULL.
            let model = v
                .get("providerData")
                .and_then(|p| p.get("model"))
                .and_then(|m| m.as_str())
                .filter(|m| !m.is_empty() && *m != "auto")
                .map(String::from);
            Some(ParsedLine {
                message: Some(crate::adapters::parsed_message(
                    source_message_id,
                    SessionMessageRole::Assistant,
                    raw,
                ))
                .map(|m| m.with_provenance(None, model)),
                observation,
            })
        }
        _ => Some(ParsedLine::observation_only(observation)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::AgentAdapter;
    use crate::domain::{SessionMemberRelation, StatsUpdate};

    fn unique_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("noending-workbuddy-{}-{}", tag, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn root_member(path: &Path) -> SessionMember {
        SessionMember {
            id: "mem-wb".into(),
            session_id: "sess-wb".into(),
            agent: Agent::WorkBuddy,
            source_member_id: "s1".into(),
            relation: SessionMemberRelation::Root,
            parent_source_member_id: None,
            source_kind: "workbuddy_transcript".into(),
            source_path: path.to_string_lossy().to_string(),
            cwd: None,
            started_at: None,
            last_activity_at: None,
            metadata: serde_json::json!({}),
        }
    }

    /// The shape measured on the real files: an enveloped opener whose prompt
    /// is buried in `<user_query>`, then a compaction replay, then a real turn.
    fn session_lines() -> String {
        [
            r#"{"id":"m1","timestamp":1783137449113,"type":"message","role":"user","content":[{"type":"input_text","text":"<system-reminder data-role=\"user-context\">\n<user_info>\nOS Version: darwin\n</user_info>\n</system-reminder>\n<user_query>整理一下昨天的会议纪要</user_query>"}],"providerData":{"provider":"zhipu"},"sessionId":"s1","cwd":"/Users/jqk/Workbuddy/x"}"#,
            r#"{"timestamp":1783137449152,"type":"file-history-snapshot","isSnapshotUpdate":false,"snapshot":{},"sessionId":"s1","cwd":"/Users/jqk/Workbuddy/x"}"#,
            r#"{"timestamp":1783137451207,"type":"ai-title","aiTitle":"整理会议纪要","sessionId":"s1","cwd":"/Users/jqk/Workbuddy/x"}"#,
            r#"{"id":"c1","timestamp":1783137453100,"type":"message","role":"user","content":[{"type":"input_text","text":"<cb_summary>Summary of the conversation so far: 很长的一段机器摘要</cb_summary>"}],"sessionId":"s1","cwd":"/Users/jqk/Workbuddy/x"}"#,
            r#"{"id":"r1","parentId":"m1","timestamp":1783137455212,"type":"reasoning","content":[{"type":"reasoning_text","text":"想一下"}],"rawContent":[{"type":"reasoning_text","text":"想一下"}],"sessionId":"s1","cwd":"/Users/jqk/Workbuddy/x"}"#,
            r#"{"id":"m2","parentId":"r1","timestamp":1783137455216,"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"整理好了。"}],"providerData":{"usage":{"requests":1,"inputTokens":300,"outputTokens":40,"totalTokens":340,"inputTokensDetails":[{"cached_tokens":120}],"outputTokensDetails":[{"reasoning_tokens":9}]}},"sessionId":"s1","cwd":"/Users/jqk/Workbuddy/x"}"#,
            r#"{"id":"m2","parentId":"r1","timestamp":1783137455225,"type":"function_call","callId":"call_1","name":"WebFetch","arguments":"{}","providerData":{"usage":{"requests":1,"inputTokens":100,"outputTokens":6,"totalTokens":106,"inputTokensDetails":[{"cached_tokens":20}],"outputTokensDetails":[{"reasoning_tokens":1}]}},"sessionId":"s1","cwd":"/Users/jqk/Workbuddy/x"}"#,
            r#"{"id":"r2","parentId":"m2","timestamp":1783137455299,"type":"function_call_result","callId":"call_1","name":"WebFetch","status":"completed","output":{"type":"text","text":"page"},"sessionId":"s1","cwd":"/Users/jqk/Workbuddy/x"}"#,
        ]
        .join("\n")
            + "\n"
    }

    #[test]
    fn discovery_reads_session_facts_and_the_first_real_prompt() {
        let dir = unique_dir("parse");
        let file = dir.join("s1.jsonl");
        std::fs::write(&file, session_lines()).unwrap();

        let found = WorkBuddyAdapter
            .discover_members_in(&[dir], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 1, "{found:#?}");
        let m = &found[0];
        assert_eq!(m.agent, Agent::WorkBuddy);
        assert_eq!(m.source_member_id, "s1");
        assert_eq!(m.kind, DiscoveredMemberKind::Root);
        assert_eq!(m.cwd.as_deref(), Some("/Users/jqk/Workbuddy/x"));
        assert_eq!(m.first_user_text.as_deref(), Some("整理一下昨天的会议纪要"));
        assert_eq!(
            m.native_title.as_deref(),
            Some("整理会议纪要"),
            "WorkBuddy titles the session itself"
        );
        // Epoch millis are normalized to the spelling every other agent uses.
        assert_eq!(
            m.started_at.as_deref(),
            Some("2026-07-04T03:57:29.113+00:00")
        );
    }

    /// Sub-agent transcripts sit in the session's own directory and must land
    /// as CHILD members of it — never as Logical Sessions of their own. The
    /// directory is the only parent link: the fixture mirrors the real files,
    /// where the child's own `sessionId` is a fresh uuid.
    #[test]
    fn a_subagent_transcript_becomes_a_child_member() {
        let dir = unique_dir("subagents");
        let parent_id = "b85e08f5-4c18-45b4-bfdd-302a1875912a";
        std::fs::write(
            dir.join(format!("{parent_id}.jsonl")),
            session_lines().replace("\"s1\"", &format!("\"{parent_id}\"")),
        )
        .unwrap();
        let sub_dir = dir.join(parent_id).join("subagents");
        std::fs::create_dir_all(&sub_dir).unwrap();
        let child_path = sub_dir.join("agent-5bc5e8f586a241e2.jsonl");
        std::fs::write(
            &child_path,
            session_lines().replace("\"s1\"", "\"01a0dd0a-ebff-7d30-80e0-e1a811c7192f\""),
        )
        .unwrap();

        let mut found = WorkBuddyAdapter
            .discover_members_in(&[dir], &|_| false)
            .unwrap();
        found.sort_by(|a, b| a.source_member_id.cmp(&b.source_member_id));
        assert_eq!(found.len(), 2, "{found:#?}");

        let child = found
            .iter()
            .find(|m| m.kind == DiscoveredMemberKind::Child)
            .expect("the subagent file is a child member");
        assert_eq!(
            child.source_member_id,
            format!("{parent_id}:subagent:agent-5bc5e8f586a241e2")
        );
        assert_eq!(child.parent_source_member_id.as_deref(), Some(parent_id));
        assert_eq!(child.root_hint.as_deref(), Some(parent_id));
        assert_eq!(
            child.source_kind, "workbuddy_subagent_transcript",
            "a child's source kind is its own"
        );

        // The parent link must land on the ROOT member's identity, or the
        // child would dangle on a session nobody owns.
        let root = found
            .iter()
            .find(|m| m.kind == DiscoveredMemberKind::Root)
            .expect("the session file is the root member");
        assert_eq!(root.source_member_id, parent_id);
        assert_eq!(
            child.parent_source_member_id.as_deref(),
            Some(root.source_member_id.as_str())
        );

        // Execution only: the same transcript contributes no conversation.
        let child_member = SessionMember {
            source_member_id: child.source_member_id.clone(),
            relation: SessionMemberRelation::Child,
            source_path: child_path.to_string_lossy().to_string(),
            ..root_member(&child_path)
        };
        let delta = WorkBuddyAdapter
            .read_member_delta(&child_member, &SessionMemberCursor::default())
            .unwrap();
        assert!(delta.messages.is_empty(), "{:?}", delta.messages);
    }

    /// WorkBuddy rewrites its `ai-title` as the conversation moves on; the
    /// last one written is the current one.
    #[test]
    fn the_last_ai_title_wins() {
        let dir = unique_dir("ai-title");
        let file = dir.join("s1.jsonl");
        std::fs::write(
            &file,
            concat!(
                r#"{"timestamp":1783137449113,"type":"message","role":"user","content":[{"type":"input_text","text":"<user_query>先看看通达信</user_query>"}],"sessionId":"s1","cwd":"/tmp/w"}"#,
                "\n",
                r#"{"timestamp":1783137451207,"type":"ai-title","aiTitle":"通达信连接功能介绍","sessionId":"s1"}"#,
                "\n",
                r#"{"timestamp":1783259041800,"type":"ai-title","aiTitle":"分析国轩高科股票","sessionId":"s1"}"#,
                "\n",
            ),
        )
        .unwrap();

        let found = WorkBuddyAdapter
            .discover_members_in(&[dir.clone()], &|_| false)
            .unwrap();
        assert_eq!(found[0].native_title.as_deref(), Some("分析国轩高科股票"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Prose survives the envelope; the compaction replay is a count;
    /// reasoning and function traffic never appear.
    #[test]
    fn ingest_keeps_prose_and_counts_the_machine_traffic() {
        let dir = unique_dir("delta");
        let file = dir.join("s1.jsonl");
        std::fs::write(&file, session_lines()).unwrap();

        let delta = WorkBuddyAdapter
            .read_member_delta(&root_member(&file), &SessionMemberCursor::default())
            .unwrap();
        let roles: Vec<(SessionMessageRole, &str)> = delta
            .messages
            .iter()
            .map(|m| (m.role, m.content.as_str()))
            .collect();
        assert_eq!(
            roles,
            vec![
                // the envelope is stripped to the prompt, not ingested whole
                (SessionMessageRole::User, "整理一下昨天的会议纪要"),
                (SessionMessageRole::Assistant, "整理好了。"),
            ],
            "got {roles:?}"
        );
        // The message timestamp survives the millis → RFC3339 normalization.
        assert_eq!(
            delta.messages[0].ts.as_deref(),
            Some("2026-07-04T03:57:29.113+00:00")
        );
        match delta.stats {
            Some(StatsUpdate::Snapshot(s)) => {
                assert_eq!(s.compaction_count, Some(1), "the cb_summary replay");
                assert_eq!(s.tool_call_count, None);
                assert_eq!(s.user_message_count, Some(1));
                assert_eq!(s.assistant_message_count, Some(1));
                assert_eq!(s.side_activity_count, None);
                // One model call each: the tool turn's usage rides on its
                // function_call, the final text turn's on the message. Both add.
                assert_eq!(s.input_tokens, Some(400));
                assert_eq!(s.output_tokens, Some(46));
                assert_eq!(s.cached_tokens, Some(140));
                assert_eq!(s.reasoning_tokens, Some(10));
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    /// The user turn is the app's envelope, not the human's words.
    #[test]
    fn the_envelope_is_not_the_user_turn() {
        let enveloped = "<system-reminder data-role=\"user-context\">\n<user_info>\nOS Version: darwin\n</user_info>\n</system-reminder>\n<user_query>看一下这只股票</user_query>";
        assert!(matches!(
            classify_user_turn(enveloped),
            UserTurn::Prompt(p) if p == "看一下这只股票"
        ));
        // A `user`-role compaction replay is machine context, not a turn.
        assert!(matches!(
            classify_user_turn("<cb_summary>Summary of the conversation so far: …</cb_summary>"),
            UserTurn::Compaction
        ));
        assert!(matches!(
            classify_user_turn("<conversation_history_summary>x</conversation_history_summary>"),
            UserTurn::Compaction
        ));
        // A bare envelope with no prompt inside carries nothing to ingest.
        assert!(matches!(
            classify_user_turn(
                "<system-reminder data-role=\"user-context\">only context</system-reminder>"
            ),
            UserTurn::Envelope
        ));
        // Plain prose is a prompt, and so is a continuation nudge.
        assert!(matches!(
            classify_user_turn("帮我看看这个"),
            UserTurn::Prompt(p) if p == "帮我看看这个"
        ));
    }

    #[test]
    fn another_agents_transcript_is_not_claimed() {
        let dir = unique_dir("foreign");
        // A pi-shaped file (which also uses sessionId-free headers) and a
        // Claude-shaped one must both be refused.
        std::fs::write(
            dir.join("pi.jsonl"),
            "{\"type\":\"session\",\"version\":3,\"id\":\"p1\",\"cwd\":\"/x\"}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("claude.jsonl"),
            "{\"type\":\"user\",\"sessionId\":\"c1\",\"uuid\":\"u1\",\"cwd\":\"/x\",\"message\":{\"role\":\"user\",\"content\":[]}}\n",
        )
        .unwrap();
        let found = WorkBuddyAdapter
            .discover_members_in(&[dir], &|_| false)
            .unwrap();
        assert!(found.is_empty(), "{found:#?}");
    }

    /// `providerData.model = "auto"` is the user's request preference, and the
    /// transcript records no resolution of it: nothing is attributed. A row
    /// written under an explicit model carries that registry id instead.
    #[test]
    fn the_auto_preference_is_not_provenance() {
        let dir = unique_dir("prov");
        let file = dir.join("s1.jsonl");
        std::fs::write(
            &file,
            [
                r#"{"id":"m1","timestamp":1783137449113,"type":"message","role":"user","content":[{"type":"input_text","text":"<user_query>问</user_query>"}],"sessionId":"s1","cwd":"/repo"}"#,
                r#"{"id":"m2","timestamp":1783137455216,"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"答"}],"providerData":{"agent":"x","model":"auto","requestModelId":"auto","requestModelName":"Auto","traceId":"t"},"sessionId":"s1","cwd":"/repo"}"#,
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();
        let delta = WorkBuddyAdapter
            .read_member_delta(&root_member(&file), &SessionMemberCursor::default())
            .unwrap();
        let assistant = delta
            .messages
            .iter()
            .find(|m| m.role == SessionMessageRole::Assistant)
            .expect("the assistant turn is conversation");
        assert_eq!(
            assistant.model, None,
            "a preference is not a generation fact"
        );
        assert_eq!(assistant.provider, None);
    }

    /// A missing model identity never suppresses the spend: the `auto` row's
    /// usage is real consumption and bills into the session stats whether or
    /// not the row attributes to a model.
    #[test]
    fn an_auto_row_still_bills_its_usage() {
        let dir = unique_dir("prov-auto-usage");
        let file = dir.join("s1.jsonl");
        std::fs::write(
            &file,
            [
                r#"{"id":"m1","timestamp":1783137449113,"type":"message","role":"user","content":[{"type":"input_text","text":"<user_query>问</user_query>"}],"sessionId":"s1","cwd":"/repo"}"#,
                r#"{"id":"m2","timestamp":1783137455216,"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"答"}],"providerData":{"agent":"x","model":"auto","requestModelId":"auto","requestModelName":"Auto","traceId":"t","usage":{"requests":1,"inputTokens":900,"outputTokens":70,"totalTokens":970,"inputTokensDetails":[{"cached_tokens":300}],"outputTokensDetails":[{"reasoning_tokens":12}]}},"sessionId":"s1","cwd":"/repo"}"#,
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();
        let delta = WorkBuddyAdapter
            .read_member_delta(&root_member(&file), &SessionMemberCursor::default())
            .unwrap();
        match delta.stats {
            Some(StatsUpdate::Snapshot(s)) => {
                assert_eq!(s.input_tokens, Some(900), "unattributed spend still bills");
                assert_eq!(s.output_tokens, Some(70));
                assert_eq!(s.cached_tokens, Some(300));
                assert_eq!(s.reasoning_tokens, Some(12));
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
        let assistant = delta
            .messages
            .iter()
            .find(|m| m.role == SessionMessageRole::Assistant)
            .unwrap();
        assert_eq!(assistant.model, None, "attribution stays unset");
    }

    /// A row carrying only `rawUsage` (the zhipu/OpenAI-style mirror of the
    /// same call) bills through the fallback: same components, snake_case.
    #[test]
    fn a_raw_usage_only_row_bills_through_the_fallback() {
        let dir = unique_dir("raw-usage");
        let file = dir.join("s1.jsonl");
        std::fs::write(
            &file,
            r#"{"id":"m2","timestamp":1783137455216,"type":"function_call","callId":"call_1","name":"WebFetch","arguments":"{}","providerData":{"model":"deepseek-v4.1-flash","rawUsage":{"prompt_tokens":13044,"completion_tokens":132,"total_tokens":13176,"completion_tokens_details":{"reasoning_tokens":49,"cached_tokens":0},"prompt_tokens_details":{"reasoning_tokens":0,"cached_tokens":6208},"prompt_cache_hit_tokens":6208}},"sessionId":"s1","cwd":"/repo"}"#,
        )
        .unwrap();

        let delta = WorkBuddyAdapter
            .read_member_delta(&root_member(&file), &SessionMemberCursor::default())
            .unwrap();
        match delta.stats {
            Some(StatsUpdate::Snapshot(s)) => {
                assert_eq!(s.input_tokens, Some(13044));
                assert_eq!(s.output_tokens, Some(132));
                assert_eq!(s.cached_tokens, Some(6208));
                assert_eq!(s.reasoning_tokens, Some(49));
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    /// An explicit-model row carries its registry id as provenance — the
    /// locally dominant shape (`deepseek-v4.1-flash` on 86.5% of rows).
    #[test]
    fn an_explicit_model_row_attributes_the_registry_id() {
        let dir = unique_dir("prov-real");
        let file = dir.join("s1.jsonl");
        std::fs::write(
            &file,
            [
                r#"{"id":"m1","timestamp":1783137449113,"type":"message","role":"user","content":[{"type":"input_text","text":"<user_query>问</user_query>"}],"sessionId":"s1","cwd":"/repo"}"#,
                r#"{"id":"m2","timestamp":1783137455216,"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"答"}],"providerData":{"agent":"x","model":"deepseek-v4.1-flash","requestModelId":"deepseek-v4.1-flash","requestModelName":"Deepseek-V4.1-Flash","traceId":"t"},"sessionId":"s1","cwd":"/repo"}"#,
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();
        let delta = WorkBuddyAdapter
            .read_member_delta(&root_member(&file), &SessionMemberCursor::default())
            .unwrap();
        let assistant = delta
            .messages
            .iter()
            .find(|m| m.role == SessionMessageRole::Assistant)
            .unwrap();
        assert_eq!(assistant.model.as_deref(), Some("deepseek-v4.1-flash"));
        assert_eq!(assistant.provider, None, "no provider field in the source");
    }
}
