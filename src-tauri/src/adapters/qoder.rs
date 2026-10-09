//! Qoder CN Adapter: `~/.qoder-cn/projects/<encoded-cwd>/<session>.jsonl`.
//!
//! Qoder's transcript is **Claude Code's format plus Qoder-only lines**
//! (`workspace-directories`, `runtime-config`, `worktree-state`, `active-leaf`,
//! `last-prompt`). That shared shape is why `fingerprint_line` must claim Qoder
//! *before* the Claude branch: read as Claude, every line would still parse and
//! nothing would look wrong.
//!
//! Member mapping:
//! - `<encoded-cwd>/<main>.jsonl` → the ROOT member;
//! - `<encoded-cwd>/<main>/subagents/agent-*.jsonl` → CHILD members. Every line
//!   repeats the ROOT's `sessionId`, so identity is Adapter-derived and stable:
//!   `<root-session-id>:subagent:<file-stem>`. Their text never becomes
//!   conversation — they contribute execution observations only.
//!
//! Qoder is an IDE with no headless CLI, so this adapter **ingests history
//! only**: `detect()` never succeeds, and there is no new/resume/exec command to
//! build. The raw transcripts are opened read-only, always.
//!
//! **One fact comes from a second store.** The transcript carries no title, so
//! discovery reads the app's own `chat_sessions` for it — read-only, looked up
//! by the session id the transcript reports, and silent when that store is
//! absent. Nothing else is taken from there.
//!
//! **The model field is a CODE, not a name.**
//! Each assistant row's `message.usage` identifies a model request. Plan-billed
//! accounts have structural zero token fields; BYOK/custom-model rows carry measured tokens,
//! and their `input_tokens` INCLUDES the cached subset (differs from the
//! Anthropic envelope), so the uncached input is what gets stored, with the
//! cached subset on its own axis. A subagent's own file is a member of its
//! own, so its usage rolls up through the session aggregate like every other
//! member's.
//!
//! `message.model` holds an internal model CODE (`qfmodel`, `dfmodel`, …),
//! not a name. The authoritative display names ship INSIDE the app as
//! `Resources/dynamic-text/qoder-cn.json` (`en.model.<code>.label`), a file
//! that updates with the app — the same code can even name a different model
//! across releases (`gmodel`: GLM-5 → GLM-5.3). That file is read once per
//! process and preferred; a built-in table (extracted from the 2026-09
//! build) backs the platforms/shapes where it cannot be read, and unmapped
//! codes pass through raw rather than being dropped. Locally synthesized
//! rows (`<synthetic>` error notices) are not generations and stay NULL. No
//! provider field exists → NULL.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::adapters::{
    detect_format, read_jsonl_delta, AgentCommand, DesktopResume, DiscoveredMember,
    DiscoveredMemberKind, ExecOptions, ParsedLine, ResumeRoute, SessionMessageRole,
};
use crate::domain::{Agent, Session, SourceAvailability, SourceCursor};
use crate::error::{other, Result};

pub struct QoderAdapter;

/// Qoder's bundle id, i.e. the name of the folder it keeps its app data in.
const QODER_BUNDLE: &str = "com.qodercn.app.stable";
/// The app database inside that folder; `chat_sessions` is the table we read.
const QODER_DB: &str = "main.sqlite";

/// The titles Qoder itself shows, read from the app's own database.
///
/// The transcript has none, which is why discovery reads a second source for
/// this one fact. `chat_sessions.session_id` is the id the transcript carries,
/// so the join is exact; `title` is the model's short name for the session —
/// except when `extra_json.titleSource` says otherwise.
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
/// It writes `ai` or `provisional` (the first user message, shown until the
/// model answers). The rule is a deny-list, not an allow-list, so a future
/// `custom` (the user renaming the session) is a title too. An absent field
/// predates the field and is treated as provisional.
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
        Err(other("Qoder 没有可启动的 CLI，无法新建会话"))
    }

    fn build_resume_command(
        &self,
        _install: &crate::platform::exec_resolver::AgentInstallation,
        _opts: &ExecOptions,
        _agent_session_id: &str,
        _cwd: Option<&Path>,
        _source_path: Option<&str>,
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

    /// The registered scheme `qoder-cn://` carries no session route (its
    /// deep links only cover reference/issue, settings, invite), so Continue
    /// can only activate the app. App absent → refuse (there is no CLI).
    fn desktop_app_name(&self) -> Option<&'static str> {
        Some("Qoder CN")
    }

    fn continue_route(&self, _session: &Session) -> ResumeRoute {
        if !crate::platform::paths::app_bundle_present("Qoder CN") {
            return ResumeRoute::Refused("未找到 Qoder 桌面应用，无法继续该会话".into());
        }
        ResumeRoute::Desktop(DesktopResume {
            uri: "qoder-cn://".into(),
            note: "将打开 Qoder 桌面应用（应用内不定位到该会话）".into(),
        })
    }
}

/// One transcript line's contribution. Root members produce conversation.
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
            // The compact-boundary summary row is the pruned context itself
            // (the source marks it `isCompactSummary`), never a turn.
            let compact_summary = v
                .get("isCompactSummary")
                .and_then(|s| s.as_bool())
                .unwrap_or(false);
            if compact_summary || sidechain {
                return None;
            }
            let msg = v.get("message").unwrap_or(&Value::Null);
            let text = content_text(msg.get("content").unwrap_or(&Value::Null));
            if text.trim().is_empty() {
                return None;
            }
            let role = if vtype == "user" {
                SessionMessageRole::User
            } else {
                SessionMessageRole::Assistant
            };
            // Injected context is never conversation.
            if role == SessionMessageRole::User && crate::adapters::is_injected_preamble(&text) {
                return None;
            }
            if !is_root {
                // A sub-agent's prose is never this Session's Conversation.
                return None;
            }
            Some(ParsedLine::message_only(crate::adapters::parsed_message(
                source_message_id,
                role,
                text,
            )))
        }
        // `attachment` lines are injected context (skill listings, system
        // reminders) — machine chatter, deliberately dropped.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::{AgentAdapter, MemberReadDelta};

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

    /// The compact-boundary summary row (`isCompactSummary`) is the pruned
    /// context itself — it must not surface as the user's voice.
    #[test]
    fn the_compact_summary_row_is_never_a_user_turn() {
        let dir = unique_dir("compact");
        let file = dir.join("s-main.jsonl");
        std::fs::write(
            &file,
            [
                r#"{"type":"user","uuid":"c1","timestamp":"2026-09-22T15:05:00.000Z","cwd":"/repo","sessionId":"s-main","isCompactSummary":true,"message":{"role":"user","content":"This session is being continued from a previous conversation…"}}"#,
                r#"{"type":"user","uuid":"u9","timestamp":"2026-09-22T15:06:00.000Z","cwd":"/repo","sessionId":"s-main","message":{"role":"user","content":[{"type":"text","text":"继续"}]}}"#,
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();
        let delta: MemberReadDelta = QoderAdapter
            .read_session_delta(
                &root_session(&file),
                &crate::domain::SourceCursor::default(),
            )
            .unwrap();
        assert_eq!(delta.messages.len(), 1);
        assert_eq!(delta.messages[0].content, "继续");
    }

    fn root_session(path: &Path) -> Session {
        Session {
            id: "sess-qoder".into(),
            agent: Agent::Qoder,
            root_agent_session_id: "s-main".into(),
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
            source_kind: "qoder_transcript".into(),
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

    /// Prose survives; attachments, tool results and bookkeeping lines
    /// never become messages.
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

    /// An absent title source is provisional too, and a missing store costs
    /// nothing but the title.
    #[test]
    fn only_a_settled_title_is_a_native_title() {
        assert!(is_resolved_title(Some("ai")));
        assert!(is_resolved_title(Some("custom")));
        assert!(!is_resolved_title(Some("provisional")));
        assert!(!is_resolved_title(None));
    }

    /// Assistant rows carrying a model code (including the synthesized
    /// `<synthetic>` notice) parse to prose since the provenance retirement,
    /// and the `runtime-config` line itself contributes nothing.
    #[test]
    fn assistant_rows_parse_regardless_of_model_codes() {
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
            .read_session_delta(
                &root_session(&file),
                &crate::domain::SourceCursor::default(),
            )
            .unwrap();
        let assistant: Vec<&crate::domain::ParsedSessionMessage> = delta
            .messages
            .iter()
            .filter(|m| m.role == SessionMessageRole::Assistant)
            .collect();
        assert_eq!(assistant.len(), 2);
    }
}
