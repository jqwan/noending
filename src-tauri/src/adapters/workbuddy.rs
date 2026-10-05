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
//! observations, not text. The bundle DOES embed a Node CLI
//! (`@genie/agent-cli`, `codebuddy`), but its dist exposes no
//! `--resume` / `--continue` / `--session-id`, so there is nothing to
//! resume with: `detect()` never succeeds and this adapter ingests
//! history only.
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
    DiscoveredMember, DiscoveredMemberKind, ExecOptions, ParsedLine, ResumeRoute,
    SessionMessageRole,
};
use crate::domain::{Agent, Session, SourceAvailability, SourceCursor};
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

/// The row's own compaction declaration. `providerData` marks replayed
/// summary turns authoritatively (`isCompactInternal` / `isCompacted` /
/// `isSummary` / `compactType`); the text prefix stays as the fallback for
/// rows written without the marker.
fn is_compaction_row(v: &Value) -> bool {
    let pd = v.get("providerData");
    let flag = |k: &str| {
        pd.and_then(|d| d.get(k))
            .and_then(|b| b.as_bool())
            .unwrap_or(false)
    };
    flag("isCompactInternal")
        || flag("isCompacted")
        || flag("isSummary")
        || pd
            .and_then(|d| d.get("compactType"))
            .and_then(|c| c.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false)
}

fn classify_user_turn(raw: &str, provider_marked_compaction: bool) -> UserTurn {
    // Compaction first: a replay can quote the user's words (and even a
    // `<user_query>` tag) inside its own block, and it is still not a turn.
    if provider_marked_compaction || is_compaction_block(raw) {
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

/// A UUIDv7's first 48 bits are the creation instant in unix milliseconds.
fn uuidv7_ms(id: &str) -> Option<i64> {
    let hex: String = id.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() < 12 {
        return None;
    }
    i64::from_str_radix(&hex[..12], 16).ok()
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
    /// The child's own `sessionId` is a UUIDv7, so its first 48 bits ARE the
    /// creation wall clock — the one start time a fresh child id carries.
    fn parse_subagent_member(path: &Path, parent_id: &str) -> Result<Option<DiscoveredMember>> {
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            return Ok(None);
        };
        let mut started_at = None;
        let mut cwd: Option<String> = None;
        for (_, line) in &crate::adapters::read_jsonl_lines(path)? {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if started_at.is_none() {
                started_at = v
                    .get("sessionId")
                    .and_then(|s| s.as_str())
                    .and_then(uuidv7_ms)
                    .and_then(crate::adapters::ms_epoch_to_rfc3339);
            }
            if cwd.is_none() {
                cwd = v
                    .get("cwd")
                    .and_then(|c| c.as_str())
                    .filter(|c| !c.is_empty())
                    .map(String::from);
            }
            if started_at.is_some() && cwd.is_some() {
                break;
            }
        }
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
            cwd,
            started_at,
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
                    if let UserTurn::Prompt(text) = classify_user_turn(&raw, is_compaction_row(&v))
                    {
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
                    // Audit/trace/snapshot trees are not transcripts; the
                    // fingerprint would refuse their files anyway, so this is
                    // scan cost and misjudgment surface, not data.
                    if p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| {
                            matches!(
                                n,
                                "audit-log"
                                    | "traces"
                                    | "shell-snapshots"
                                    | "local_storage"
                                    | "logs"
                            )
                        })
                        .unwrap_or(false)
                    {
                        continue;
                    }
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
                // Anything else living in a `subagents/` directory is not a
                // session: it must never fall through to a ROOT member.
                if p.parent()
                    .and_then(|d| d.file_name())
                    .and_then(|n| n.to_str())
                    == Some("subagents")
                {
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

    fn continue_route(&self, session: &Session) -> ResumeRoute {
        if !crate::platform::paths::app_bundle_present("WorkBuddy") {
            return ResumeRoute::Refused("未找到 WorkBuddy 桌面应用，无法继续该会话".into());
        }
        ResumeRoute::Desktop(DesktopResume {
            uri: format!("workbuddy://chat/{}", session.root_agent_session_id),
            note: "将在 WorkBuddy 中打开并定位到该会话".into(),
        })
    }
}

/// One line's contribution: root message lines only — a real user prompt's
/// text or assistant prose. Machine traffic (function_call, tool results,
/// compaction replays, injected envelopes) stays out.
fn parse_line(v: &Value, is_root: bool) -> Option<ParsedLine> {
    let vtype = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if vtype != "message" {
        return None;
    }
    let source_message_id = v.get("id").and_then(|s| s.as_str()).map(|s| s.to_string());
    let raw = content_text(v.get("content").unwrap_or(&Value::Null));
    if raw.trim().is_empty() {
        return None;
    }
    match v.get("role").and_then(|r| r.as_str()).unwrap_or("") {
        "user" => match classify_user_turn(&raw, is_compaction_row(&v)) {
            UserTurn::Prompt(p) => {
                if !is_root {
                    return None;
                }
                Some(ParsedLine::message_only(crate::adapters::parsed_message(
                    source_message_id,
                    SessionMessageRole::User,
                    p,
                )))
            }
            _ => None,
        },
        "assistant" => {
            if !is_root {
                return None;
            }
            Some(ParsedLine::message_only(crate::adapters::parsed_message(
                source_message_id,
                SessionMessageRole::Assistant,
                raw,
            )))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::AgentAdapter;

    fn unique_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("noending-workbuddy-{}-{}", tag, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
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
    fn the_envelope_is_not_the_user_turn() {
        let enveloped = "<system-reminder data-role=\"user-context\">\n<user_info>\nOS Version: darwin\n</user_info>\n</system-reminder>\n<user_query>看一下这只股票</user_query>";
        assert!(matches!(
            classify_user_turn(enveloped, false),
            UserTurn::Prompt(p) if p == "看一下这只股票"
        ));
        // A `user`-role compaction replay is machine context, not a turn.
        assert!(matches!(
            classify_user_turn(
                "<cb_summary>Summary of the conversation so far: …</cb_summary>",
                false
            ),
            UserTurn::Compaction
        ));
        assert!(matches!(
            classify_user_turn(
                "<conversation_history_summary>x</conversation_history_summary>",
                false
            ),
            UserTurn::Compaction
        ));
        // A bare envelope with no prompt inside carries nothing to ingest.
        assert!(matches!(
            classify_user_turn(
                "<system-reminder data-role=\"user-context\">only context</system-reminder>",
                false,
            ),
            UserTurn::Envelope
        ));
        // Plain prose is a prompt, and so is a continuation nudge.
        assert!(matches!(
            classify_user_turn("帮我看看这个", false),
            UserTurn::Prompt(p) if p == "帮我看看这个"
        ));
        // The providerData markers are the authoritative compaction verdict —
        // a replay that quotes the user without a `<cb_summary>` prefix is
        // still machine context.
        assert!(matches!(
            classify_user_turn("被压缩的那段对话原文", true),
            UserTurn::Compaction
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
}
