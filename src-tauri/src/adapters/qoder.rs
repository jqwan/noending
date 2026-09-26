//! Qoder CN Adapter: `~/.qoder-cn/projects/<encoded-cwd>/<session>.jsonl`.
//!
//! Qoder's transcript is **Claude Code's format plus Qoder-only lines**
//! (`workspace-directories`, `runtime-config`, `worktree-state`, `active-leaf`,
//! `last-prompt`). That shared shape is why `fingerprint_line` must claim Qoder
//! *before* the Claude branch: read as Claude, every line would still parse and
//! nothing would look wrong — which is exactly how provenance gets lost
//! (AGENTS.md: Provenance Fidelity).
//!
//! Member mapping:
//! - `<encoded-cwd>/<main>.jsonl` → the ROOT member;
//! - `<encoded-cwd>/<main>/subagents/agent-*.jsonl` → CHILD members. Their
//!   every line repeats the ROOT's `sessionId`, so the member identity is
//!   Adapter-derived and stable: `<root-session-id>:subagent:<file-stem>`.
//!   Their text never becomes conversation — they contribute
//!   execution observations only.
//!
//! Qoder is an IDE with no headless CLI, so this adapter **ingests history
//! only**: `detect()` never succeeds, and there is no new/resume/exec command
//! to build. The raw transcripts are opened read-only, always.
//!
//! **One fact comes from a second store.** The transcript carries no title, so
//! discovery reads the app's own `chat_sessions` for it — read-only, looked up
//! by the very session id the transcript reports, and silent when that store is
//! absent. Nothing else about the session is taken from there.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::adapters::{
    detect_format, read_jsonl_delta, AgentCommand, DiscoveredMember, DiscoveredMemberKind,
    ExecOptions, MemberObservation, ParsedLine, SessionMessageRole,
};
use crate::domain::{Agent, SessionMember, SessionMemberCursor, SourceAvailability};
use crate::error::{other, Result};

pub struct QoderAdapter;

/// Qoder's bundle id, i.e. the name of the folder it keeps its app data in.
const QODER_BUNDLE: &str = "com.qodercn.app.stable";
/// The app database inside that folder; `chat_sessions` is the table we read.
const QODER_DB: &str = "main.sqlite";

/// The titles Qoder itself shows, read from the app's own database.
///
/// The transcript has none, which is why discovery reads a second source for
/// this one fact. `chat_sessions.session_id` is the id the transcript
/// carries (7/7 on this machine), so the join is exact; `title` is the model's
/// short name for the session — except when Qoder says otherwise, which it does
/// out loud: `extra_json.titleSource`.
struct SessionTitles(Option<Connection>);

impl SessionTitles {
    /// `None` when the store is absent, locked, or shaped differently — a
    /// missing title source must cost nothing but the title.
    fn open() -> Self {
        let path = crate::platform::paths::resolve_external_app_support(QODER_BUNDLE)
            .map(|dir| dir.join(QODER_DB));
        Self::open_at(path.as_deref())
    }

    /// Split from [`Self::open`] so a fixture database can be read without the
    /// app's own folder existing.
    fn open_at(path: Option<&Path>) -> Self {
        let conn = path.and_then(|path| {
            Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .ok()
        });
        Self(conn)
    }

    fn title_for(&self, session_id: &str) -> Option<String> {
        let mut stmt = self
            .0
            .as_ref()?
            .prepare("SELECT title, extra_json FROM chat_sessions WHERE session_id = ?1")
            .ok()?;
        let (title, extra): (String, Option<String>) = stmt
            .query_row([session_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .ok()?;
        let source = extra
            .as_deref()
            .and_then(|e| serde_json::from_str::<Value>(e).ok())
            .and_then(|v| {
                v.get("titleSource")
                    .and_then(|s| s.as_str())
                    .map(str::to_string)
            });
        is_resolved_title(source.as_deref())
            .then_some(title)
            .filter(|t| !t.trim().is_empty())
    }
}

/// Whether Qoder's `title` is a name it settled on.
///
/// The two values it writes: `ai` (6 of 7 sessions locally — 「修改任务
/// 编辑功能」, 「了解工程概况」) and `provisional` (the first user message, shown
/// until the model answers). The rule is therefore a deny-list, not an
/// allow-list: a future `custom` (the user renaming the session) is a title too.
/// An absent field means a store from before the field existed, when `title`
/// was the raw prompt — treated as provisional.
fn is_resolved_title(source: Option<&str>) -> bool {
    matches!(source, Some(s) if s != "provisional")
}

/// Text of a message's content blocks. Only `type:"text"` counts: Qoder folds
/// tool results into `user`-role lines (`toolUseResult` + a `tool_result`
/// block), and tool traffic is deliberately not ingested.
fn content_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(arr) => {
            let mut parts = Vec::new();
            for item in arr {
                if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                        parts.push(t.to_string());
                    }
                }
            }
            parts.join("\n")
        }
        _ => String::new(),
    }
}

impl QoderAdapter {
    /// `<encoded-cwd>/<main>/subagents/agent-*.jsonl` is a sub-agent
    /// transcript: the directory shape is the marker (the real files' head is
    /// plain Claude-shaped, so no content test can decide this).
    /// The stem and the enclosing session directory are the stable identity.
    fn subagent_member(path: &Path) -> Option<(String, String)> {
        let name = path.file_name()?.to_str()?;
        if !name.starts_with("agent-") || !name.ends_with(".jsonl") {
            return None;
        }
        let stem = path.file_stem()?.to_str()?;
        let subagents_dir = path.parent()?;
        if subagents_dir.file_name()?.to_str()? != "subagents" {
            return None;
        }
        let session_dir = subagents_dir.parent()?;
        Some((session_dir.to_string_lossy().to_string(), stem.to_string()))
    }

    /// The transcript's own `sessionId` — the ROOT's id, repeated on every
    /// line of a subagent transcript too.
    fn transcript_session_id(path: &Path) -> Option<String> {
        let lines = crate::adapters::read_jsonl_lines(path).ok()?;
        for (_, line) in &lines {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if let Some(id) = v.get("sessionId").and_then(|s| s.as_str()) {
                if !id.is_empty() {
                    return Some(id.to_string());
                }
            }
        }
        None
    }

    fn parse_root_member(path: &Path) -> Result<Option<DiscoveredMember>> {
        let file_name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .ok_or_else(|| other("无效的 session 文件名"))?;

        let lines = crate::adapters::read_jsonl_lines(path)?;
        let mut session_id: Option<String> = None;
        let mut cwd: Option<String> = None;
        let mut started_at = None;
        let mut last_ts = None;
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
                // The line-level cwd is authoritative; the directory name is an
                // encoded (and therefore ambiguous) copy of it.
                cwd = v
                    .get("cwd")
                    .and_then(|c| c.as_str())
                    .filter(|c| !c.is_empty())
                    .map(|c| c.to_string());
            }
            // Only string timestamps: Qoder also writes `runtime-config` lines
            // whose `timestamp` is epoch millis, and mixing the two would make
            // started/last activity incomparable.
            if let Some(ts) = v.get("timestamp").and_then(|t| t.as_str()) {
                if started_at.is_none() {
                    started_at = Some(ts.to_string());
                }
                last_ts = Some(ts.to_string());
            }
            if first_user_text.is_none()
                && v.get("type").and_then(|t| t.as_str()) == Some("user")
                && v.get("isSidechain").and_then(|s| s.as_bool()) != Some(true)
            {
                // A user line carrying toolUseResult is a tool result, not a
                // prompt; its text blocks are dropped above, so it yields "".
                let text = v
                    .get("message")
                    .map(|m| content_text(m.get("content").unwrap_or(&Value::Null)))
                    .unwrap_or_default();
                // `<…>` environment blocks and `#`-prefixed injections
                // (AGENTS.md, attached-file headers) are not user text.
                if !text.is_empty() && !crate::adapters::is_injected_preamble(&text) {
                    first_user_text = Some(crate::adapters::truncate_text(&text, 400));
                }
            }
            // Last resort for a title.
            if first_agent_text.is_none()
                && v.get("type").and_then(|t| t.as_str()) == Some("assistant")
                && v.get("isSidechain").and_then(|s| s.as_bool()) != Some(true)
            {
                let text = v
                    .get("message")
                    .map(|m| content_text(m.get("content").unwrap_or(&Value::Null)))
                    .unwrap_or_default();
                if !text.is_empty() {
                    first_agent_text = Some(crate::adapters::truncate_text(&text, 400));
                }
            }
        }

        let meta = std::fs::metadata(path)?;
        let last_activity = meta
            .modified()
            .ok()
            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());

        let agent_session_id = session_id.unwrap_or_else(|| file_name.clone());

        Ok(Some(DiscoveredMember {
            agent: Agent::Qoder,
            source_member_id: agent_session_id,
            kind: DiscoveredMemberKind::Root,
            parent_source_member_id: None,
            root_hint: None,
            source_kind: "qoder_transcript".into(),
            source_path: path.to_path_buf(),
            cwd,
            started_at,
            last_activity_at: last_activity.or(last_ts),
            // The transcript carries no title; discovery fills this in from the
            // app's own database.
            native_title: None,
            first_user_text,
            first_agent_text,
            metadata: serde_json::json!({}),
        }))
    }

    /// A sub-agent transcript as its own CHILD member: identity is the
    /// Adapter-derived `<root>:subagent:<stem>`, the parent is the root id the
    /// transcript itself repeats. No title sources — a child never names a
    /// Logical Session.
    fn parse_subagent_member(
        path: &Path,
        session_dir: &str,
        stem: &str,
    ) -> Result<Option<DiscoveredMember>> {
        let Some(root_id) = Self::transcript_session_id(path) else {
            return Ok(None);
        };
        let meta = std::fs::metadata(path)?;
        let last_activity = meta
            .modified()
            .ok()
            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());
        Ok(Some(DiscoveredMember {
            agent: Agent::Qoder,
            source_member_id: format!("{root_id}:subagent:{stem}"),
            kind: DiscoveredMemberKind::Child,
            parent_source_member_id: Some(root_id.clone()),
            root_hint: Some(root_id),
            source_kind: "qoder_subagent_transcript".into(),
            source_path: path.to_path_buf(),
            // The subagent file carries the root's cwd on its lines, but a
            // child's cwd is execution fact only — reported for the member row.
            cwd: None,
            started_at: None,
            last_activity_at: last_activity,
            native_title: None,
            first_user_text: None,
            first_agent_text: None,
            metadata: serde_json::json!({ "session_dir": session_dir }),
        }))
    }
}

impl crate::adapters::AgentAdapter for QoderAdapter {
    fn agent(&self) -> Agent {
        Agent::Qoder
    }

    /// Always `None`: Qoder ships no CLI, so there is nothing to detect and
    /// nothing to launch. Ingestion does not consult this.
    fn detect(&self) -> Option<crate::platform::exec_resolver::AgentInstallation> {
        None
    }

    fn discover_members_in(
        &self,
        roots: &[PathBuf],
        unchanged: &dyn Fn(&Path) -> bool,
    ) -> Result<Vec<DiscoveredMember>> {
        let mut out = Vec::new();
        let titles = SessionTitles::open();
        let mut stack: Vec<PathBuf> = roots.to_vec();
        while let Some(dir) = stack.pop() {
            let rd = match std::fs::read_dir(&dir) {
                Ok(rd) => rd,
                Err(_) => continue,
            };
            for entry in rd.filter_map(|e| e.ok()) {
                let p = entry.path();
                if p.is_dir() {
                    // `<sessionId>/subagents/…` and the diagnostics segments
                    // under `~/.qoder-cn/logs` are walked too — the shape is
                    // what decides, not the path.
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                if unchanged(&p) {
                    continue;
                }
                if let Some((session_dir, stem)) = Self::subagent_member(&p) {
                    match Self::parse_subagent_member(&p, &session_dir, &stem) {
                        Ok(Some(m)) => out.push(m),
                        Ok(None) => {}
                        Err(e) => eprintln!("[discover] skip {}: {}", p.display(), e),
                    }
                    continue;
                }
                if detect_format(&p) != Some(Agent::Qoder) {
                    continue;
                }
                match Self::parse_root_member(&p) {
                    Ok(Some(mut m)) => {
                        m.native_title = titles.title_for(&m.source_member_id);
                        out.push(m)
                    }
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
            crate::adapters::StatsCapabilities::TOOL_AND_SIDE_ACTIVITY,
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
        _install: &crate::platform::exec_resolver::AgentInstallation,
        _opts: &ExecOptions,
        _context_file: Option<&Path>,
        _cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        Err(other("Qoder 没有可启动的 CLI，无法新建会话"))
    }

    fn build_resume_command(
        &self,
        _install: &crate::platform::exec_resolver::AgentInstallation,
        _opts: &ExecOptions,
        _agent_session_id: &str,
        _context_file: Option<&Path>,
        _cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        Err(other("Qoder 没有可启动的 CLI，无法恢复会话"))
    }

    fn build_exec_command(
        &self,
        _install: &crate::platform::exec_resolver::AgentInstallation,
        _opts: &ExecOptions,
        _prompt: &str,
    ) -> Result<AgentCommand> {
        Err(other("Qoder 没有可启动的 CLI，无法一次性执行"))
    }
}

/// One transcript line's contribution. Root members produce
/// conversation; child members produce observations only.
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
            let text = content_text(msg.get("content").unwrap_or(&Value::Null));
            // A user line with toolUseResult is a completed tool call.
            let tool_call = vtype == "user" && v.get("toolUseResult").is_some();
            let observation = MemberObservation {
                tool_calls: u64::from(tool_call),
                side_activity: u64::from(sidechain),
                ..Default::default()
            };
            if sidechain {
                return Some(ParsedLine::observation_only(observation));
            }
            if text.trim().is_empty() || !is_root {
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
            // Message provenance: the assistant
            // row itself carries `message.model` — same envelope position as
            // Claude Code's actual response model, source-native opaque ids
            // ("dfmodel", "qfmodel", …; 4728/4728 rows in the real corpus).
            // This is NOT the `runtime-config.model` broadcast: the field sits
            // on the message row. Locally synthesized
            // rows ("<synthetic>" error notices) are not real generations and
            // stay NULL. No provider field exists → NULL.
            let model = if role == SessionMessageRole::Assistant {
                msg.get("model")
                    .and_then(|m| m.as_str())
                    .filter(|m| *m != "<synthetic>")
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
        // `attachment` lines are injected context (skill listings, system
        // reminders) — machine chatter, same category as the tool events
        // deliberately dropped.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::{AgentAdapter, MemberReadDelta};
    use crate::domain::{SessionMemberRelation, StatsUpdate};

    fn unique_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("noending-qoder-{}-{}", tag, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn main_lines() -> String {
        // Bookkeeping + a real prompt + an attachment + a tool result folded
        // into a user-role line, in the shapes the real files use.
        [
            r#"{"type":"workspace-directories","sessionId":"s-main","directories":["/repo"]}"#,
            r#"{"type":"runtime-config","sessionId":"s-main","model":"dfmodel","timestamp":1790089288825}"#,
            r#"{"type":"user","uuid":"u1","timestamp":"2026-09-22T15:01:29.551Z","cwd":"/repo","sessionId":"s-main","message":{"role":"user","content":[{"type":"text","text":"把详情页的消息改一下"}]}}"#,
            r#"{"type":"attachment","uuid":"a1","timestamp":"2026-09-22T15:01:29.552Z","sessionId":"s-main","attachment":{"type":"skill_listing"}}"#,
            r#"{"type":"assistant","uuid":"u2","timestamp":"2026-09-22T15:01:31.000Z","cwd":"/repo","sessionId":"s-main","message":{"role":"assistant","content":[{"type":"text","text":"好，我先看现状。"},{"type":"tool_use","name":"Read"}]}}"#,
            r#"{"type":"user","uuid":"u3","timestamp":"2026-09-22T15:01:32.000Z","cwd":"/repo","sessionId":"s-main","toolUseResult":{"ok":true},"message":{"role":"user","content":[{"type":"tool_result","content":"file contents"}]}}"#,
            r#"{"type":"assistant","uuid":"u4","timestamp":"2026-09-22T15:01:40.000Z","isSidechain":true,"cwd":"/repo","sessionId":"s-main","message":{"role":"assistant","content":[{"type":"text","text":"子 Agent 的自述"}]}}"#,
        ]
        .join("\n")
            + "\n"
    }

    fn root_member(path: &Path) -> SessionMember {
        SessionMember {
            id: "mem-qoder".into(),
            session_id: "sess-qoder".into(),
            agent: Agent::Qoder,
            source_member_id: "s-main".into(),
            relation: SessionMemberRelation::Root,
            parent_source_member_id: None,
            source_kind: "qoder_transcript".into(),
            source_path: path.to_string_lossy().to_string(),
            cwd: None,
            started_at: None,
            last_activity_at: None,
            metadata: serde_json::json!({}),
        }
    }

    /// Prose survives; attachments, tool results and bookkeeping lines
    /// never become messages.
    #[test]
    fn the_root_read_keeps_prose_and_counts_the_rest() {
        let dir = unique_dir("parse");
        let file = dir.join("s-main.jsonl");
        std::fs::write(&file, main_lines()).unwrap();

        let delta: MemberReadDelta = QoderAdapter
            .read_member_delta(&root_member(&file), &SessionMemberCursor::default())
            .unwrap();
        let texts: Vec<(SessionMessageRole, &str)> = delta
            .messages
            .iter()
            .map(|m| (m.role, m.content.as_str()))
            .collect();
        assert_eq!(
            texts,
            vec![
                (SessionMessageRole::User, "把详情页的消息改一下"),
                (SessionMessageRole::Assistant, "好，我先看现状。"),
            ],
            "the sidechain line and the tool-result line never become conversation"
        );
        match delta.stats {
            Some(StatsUpdate::Snapshot(s)) => {
                assert_eq!(s.tool_call_count, Some(1));
                assert_eq!(s.side_activity_count, Some(1));
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Sub-agent transcripts become CHILD members with an
    /// Adapter-derived stable identity, while their text stays out of the
    /// Conversation.
    #[test]
    fn subagent_transcripts_are_child_members_with_derived_identity() {
        let dir = unique_dir("subagent");
        let parent = dir.join("-repo");
        let children = parent.join("s-main").join("subagents");
        std::fs::create_dir_all(&children).unwrap();
        std::fs::write(parent.join("s-main.jsonl"), main_lines()).unwrap();
        // No Qoder marker in the head, exactly like the real ones.
        std::fs::write(
            children.join("agent-aExplore-abc123.jsonl"),
            r#"{"type":"assistant","uuid":"c1","cwd":"/repo","sessionId":"s-main","message":{"role":"assistant","content":[{"type":"text","text":"子 Agent 的结论"}]}}
"#,
        )
        .unwrap();

        let found = QoderAdapter
            .discover_members_in(&[dir], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 2, "root + child: {found:#?}");
        let root = found
            .iter()
            .find(|m| m.kind == DiscoveredMemberKind::Root)
            .unwrap();
        assert_eq!(root.source_member_id, "s-main");
        let child = found
            .iter()
            .find(|m| m.kind == DiscoveredMemberKind::Child)
            .unwrap();
        assert_eq!(
            child.source_member_id,
            "s-main:subagent:agent-aExplore-abc123"
        );
        assert_eq!(child.parent_source_member_id.as_deref(), Some("s-main"));
        assert_eq!(
            child.first_user_text, None,
            "a child never carries a title source"
        );

        // The child's text is execution observation only.
        let member = SessionMember {
            id: "mem-child".into(),
            session_id: "sess-qoder".into(),
            agent: Agent::Qoder,
            source_member_id: child.source_member_id.clone(),
            relation: SessionMemberRelation::Child,
            parent_source_member_id: Some("s-main".into()),
            source_kind: "qoder_subagent_transcript".into(),
            source_path: child.source_path.to_string_lossy().to_string(),
            cwd: None,
            started_at: None,
            last_activity_at: None,
            metadata: serde_json::json!({}),
        };
        let delta: MemberReadDelta = QoderAdapter
            .read_member_delta(&member, &SessionMemberCursor::default())
            .unwrap();
        assert!(
            delta.messages.is_empty(),
            "child transcript text never becomes conversation"
        );
    }

    /// The app database still names the root session.
    #[test]
    fn discovery_reads_session_facts() {
        let dir = unique_dir("discover");
        let file = dir.join("s-main.jsonl");
        std::fs::write(&file, main_lines()).unwrap();

        let found = QoderAdapter
            .discover_members_in(&[dir], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 1);
        let m = &found[0];
        assert_eq!(m.kind, DiscoveredMemberKind::Root);
        assert_eq!(m.source_member_id, "s-main");
        assert_eq!(m.cwd.as_deref(), Some("/repo"));
        assert_eq!(
            m.started_at.as_deref(),
            Some("2026-09-22T15:01:29.551Z"),
            "epoch-millis bookkeeping timestamps must not win over ISO ones"
        );
        assert_eq!(m.first_user_text.as_deref(), Some("把详情页的消息改一下"));
    }

    /// A `chat_sessions` as Qoder lays it out — only the columns the reader
    /// touches, but with the real constraints, so the fixture could have been
    /// written by the app itself.
    #[test]
    fn only_a_settled_title_is_a_native_title() {
        assert!(is_resolved_title(Some("ai")));
        assert!(is_resolved_title(Some("custom")));
        assert!(!is_resolved_title(Some("provisional")));
        assert!(!is_resolved_title(None));
        let _ = SessionTitles::open_at(None);
    }

    /// The assistant row's own `message.model`
    /// attributes (this is NOT the runtime-config broadcast);
    /// locally synthesized error rows ("<synthetic>") are not generations and
    /// stay NULL. The runtime-config line itself contributes nothing.
    #[test]
    fn assistant_rows_carry_their_model_but_synthetic_stays_null() {
        let dir = unique_dir("prov");
        let file = dir.join("s.jsonl");
        let line = |idx: usize, vtype: &str, model: Option<&str>| {
            let mut v = serde_json::json!({
                "type": vtype,
                "uuid": format!("u{idx}"),
                "timestamp": format!("2026-09-20T15:01:{idx:02}.000Z"),
                "message": { "role": if vtype == "user" { "user" } else { "assistant" },
                             "content": [ { "type": "text", "text": if vtype == "user" { "问" } else { "答" } } ] },
            });
            if let Some(m) = model {
                v["message"]["model"] = serde_json::json!(m);
            }
            v.to_string()
        };
        std::fs::write(
            &file,
            [
                r#"{"type":"runtime-config","sessionId":"s","model":"dfmodel","timestamp":1790089288825}"#.to_string(),
                line(1, "user", None),
                line(2, "assistant", Some("dfmodel")),
                line(3, "assistant", Some("<synthetic>")),
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();
        let delta = QoderAdapter
            .read_member_delta(&root_member(&file), &SessionMemberCursor::default())
            .unwrap();
        let assistant: Vec<&crate::domain::ParsedSessionMessage> = delta
            .messages
            .iter()
            .filter(|m| m.role == SessionMessageRole::Assistant)
            .collect();
        assert_eq!(assistant.len(), 2);
        assert_eq!(assistant[0].model.as_deref(), Some("dfmodel"));
        assert_eq!(assistant[0].provider, None, "no provider in the source");
        assert_eq!(
            assistant[1].model, None,
            "a synthesized error notice is not a generation"
        );
    }
}
