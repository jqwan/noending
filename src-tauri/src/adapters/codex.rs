//! Codex Adapter: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`.
//! `CODEX_HOME` overrides the root. Raw data is read-only, always — NoEnding
//! never deletes an Agent-owned source.
//!
//! Member mapping: normal root rollout → `Root`; `thread_source=subagent` →
//! `Child` (a task thread Codex spawned); `thread_source=guardian_review` →
//! `Side` (a review lane beside the main conversation);
//! `thread_source=memory_consolidation` → `Child` (Codex's background memory
//! pass, never a conversation); forked page → `ForkRoot` (its own Logical
//! Session, the fork base being provenance only). `agent_created_thread` and
//! `chatgpt_handoff` are NOT internal — verified as ordinary interactive
//! threads — so they stay `Root`.
//!
//! Conversation is ONLY the root member's `response_item.message` turns with
//! role user / assistant. Everything else — `agent_message` envelopes, tool
//! traffic, reasoning, compaction markers, session meta, and the per-turn
//! model state events (`turn_context`, `thread_settings_applied`) — is not
//! conversation and contributes nothing since the provenance retirement.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::{
    detect_format, json_str_field, read_jsonl_delta, truncate_text, AgentCommand, DesktopResume,
    DiscoveredMember, DiscoveredMemberKind, ExecOptions, MemberReadDelta, ParsedLine, ResumeRoute,
    SessionMessageRole,
};
use crate::domain::{Agent, Session, SourceAvailability, SourceCursor};
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

/// The uuid Codex wrote into a rollout's name: the file's OWN thread id,
/// which differs from `payload.id` only on a forked page.
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
/// being it.
///
/// NOT to be confused with `session_meta.forked_from_id`, which means the
/// thread inherited another thread's HISTORY, which every forked-history
/// subagent has (over the local corpus all 13 occurrences equal the thread's own
/// `parent_thread_id`). Treating it as this marker would promote those subagents
/// from `Child` to `ForkRoot` — one phantom Logical Session each.
fn is_forked_page_name(path: &Path) -> bool {
    path.file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_prefix("rollout-"))
        .map(|rest| rest.contains('_'))
        .unwrap_or(false)
}

/// Threads Codex runs for itself, whose "user" turns are prompts Codex wrote
/// rather than anything a person typed: a spawned subagent, its review thread,
/// and the background pass that updates its memories. They never name a session
/// and are never conversation.
fn is_internal_source(thread_source: Option<&str>) -> bool {
    matches!(
        thread_source,
        Some("subagent") | Some("guardian_review") | Some("memory_consolidation")
    )
}

/// Which side of the execution graph one rollout sits on.
///
/// Verified over the local corpus (75 rollouts): `subagent` (37) and
/// `guardian_review` (4) each carry a `parent_thread_id` and are internal;
/// `user` (32) is a normal root. `agent_created_thread` and `chatgpt_handoff`
/// are NOT internal — the sampled ones are ordinary interactive threads (one
/// waiting for the user's task, one a conversation handed over from ChatGPT) —
/// so they stay Root. `memory_consolidation` is Codex updating its memories in
/// the background, never a conversation, so it is internal too.
fn member_kind_of(thread_source: Option<&str>, forked: bool) -> DiscoveredMemberKind {
    if forked {
        // A forked page is independently continuable — the fork wins over any
        // internal marking a continued prefix may carry.
        return DiscoveredMemberKind::ForkRoot;
    }
    match thread_source {
        Some("guardian_review") => DiscoveredMemberKind::Side,
        Some("subagent") | Some("memory_consolidation") => DiscoveredMemberKind::Child,
        _ => DiscoveredMemberKind::Root,
    }
}

impl CodexAdapter {
    /// Parse one rollout into its execution member facts.
    fn parse_member(path: &Path) -> Result<Option<DiscoveredMember>> {
        let lines = crate::adapters::read_jsonl_lines(path)?;
        let mut meta_seen = false;
        let mut meta_id = None;
        let mut legacy_session_id = None;
        let mut cwd = None;
        let mut started_at = None;
        let mut parent = None;
        let mut fork_base = None;
        let mut thread_source: Option<String> = None;
        // Codex's own review subagents: their user turns are prompts Codex wrote
        // (the parent transcript re-embedded) and their developer turns are its
        // instructions, so there is no user text to name them after.
        let mut first_user_text = None;
        let mut first_agent_text = None;

        for (_, line) in &lines {
            let v: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let vtype = v.get("type").and_then(|t| t.as_str());
            if vtype == Some("session_meta") {
                // The FIRST meta is this rollout's OWN declaration (codex
                // writes it at file creation). A continuation file embeds the
                // RESUMED thread's meta inside its replayed history — letting
                // a later meta override identity re-keys the file to a session
                // whose usage lives in a different rollout, and two rollouts
                // then fight over one member: source_path flips per discovery
                // pass and the member row is replaced with each flip (a
                // real store showed generation 21). First meta wins, always.
                if !meta_seen {
                    meta_seen = true;
                    let p = v.get("payload").unwrap_or(&v);
                    meta_id = p.get("id").and_then(|s| s.as_str()).map(|s| s.to_string());
                    // Legacy last resort only: `session_id` is the conversation
                    // the writer was joined to, not this thread, so it never
                    // decides identity while a real thread id is available.
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
                    thread_source = p
                        .get("thread_source")
                        .and_then(|t| t.as_str())
                        .map(|t| t.to_string());
                    // A forked page records where the prefix it inherited lives.
                    fork_base = p
                        .get("history_base")
                        .and_then(|h| h.get("thread_id"))
                        .and_then(|t| t.as_str())
                        .map(|t| t.to_string());
                }
            } else if vtype == Some("response_item") {
                let p = v.get("payload").unwrap_or(&v);
                if p.get("type").and_then(|t| t.as_str()) == Some("message") {
                    let role = p.get("role").and_then(|r| r.as_str()).unwrap_or("");
                    let text = extract_text(p.get("content").unwrap_or(&Value::Null));
                    // `<…>` environment_context wrappers and `#`-prefixed
                    // injections (AGENTS.md, attached-file headers) are not
                    // user text; an envelope that wraps the real request
                    // under `## My request:` contributes the wrapped words.
                    let internal = is_internal_source(thread_source.as_deref());
                    if role == "user" && first_user_text.is_none() && !internal && !text.is_empty()
                    {
                        if let Some(user_text) = crate::adapters::user_request_text(&text) {
                            first_user_text = Some(truncate_text(&user_text, 400));
                        }
                    }
                    // The last resort for a title: what the thread itself said
                    // first. Only consulted when there is no user text, so it
                    // costs nothing on the common path.
                    if role == "assistant" && first_agent_text.is_none() && !text.is_empty() {
                        first_agent_text = Some(text);
                    }
                }
            }
            // session_meta opens a rollout file, so stopping at the meta line
            // itself would leave every title unset. Stop once the meta is parsed
            // and there is a user text to name the session after; a thread Codex
            // wrote itself has no user text, so it scans on to its own first
            // reply (or EOF, for a meta-only session).
            let internal = is_internal_source(thread_source.as_deref());
            if meta_seen && (first_user_text.is_some() || (internal && first_agent_text.is_some()))
            {
                break;
            }
        }

        // Identity is the file's OWN thread id. A forked page's meta still
        // names the thread it forked FROM, so there the name has to decide;
        // everywhere else the meta leads. Keying on `session_id` instead would
        // collapse several rollouts into one session.
        let by_name = || thread_id_from_filename(path);
        let forked = is_forked_page_name(path);
        let session_id = if forked {
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

        let kind = member_kind_of(thread_source.as_deref(), forked);
        let is_logical_root = kind.is_logical_root();

        let meta = std::fs::metadata(path)?;
        let last_activity = meta
            .modified()
            .ok()
            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());

        Ok(Some(DiscoveredMember {
            agent: Agent::Codex,
            source_member_id: session_id,
            kind,
            // Only fork provenance / topology hints; a root rollout has none.
            parent_source_member_id: parent.clone(),
            root_hint: parent,
            source_kind: "codex_rollout".into(),
            source_path: path.to_path_buf(),
            cwd,
            started_at,
            last_activity_at: last_activity,
            // Codex writes no title of its own anywhere in a rollout. Its name
            // for the thread lives in the session index, and discovery fills
            // this in from there.
            native_title: None,
            first_user_text: is_logical_root.then_some(first_user_text).flatten(),
            first_agent_text: is_logical_root
                .then_some(first_agent_text.map(|t| truncate_text(&t, 400)))
                .flatten(),
            metadata: serde_json::json!({ "thread_source": thread_source }),
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

    fn discover_members_in(
        &self,
        roots: &[PathBuf],
        unchanged: &dyn Fn(&Path) -> bool,
    ) -> Result<Vec<DiscoveredMember>> {
        let mut out = Vec::new();
        let names = ThreadNames::load(roots);
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
                match Self::parse_member(&p) {
                    Ok(Some(mut m)) => {
                        if m.kind.is_logical_root() {
                            m.native_title =
                                names.title_for(&m.source_member_id, m.first_user_text.as_deref());
                        }
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
    ) -> Result<MemberReadDelta> {
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
        install: &AgentInstallation,
        opts: &ExecOptions,
        cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        let mut args = runtime_args(opts);
        if let Some(message) = opts.initial_message() {
            // Stop option/subcommand parsing even when the message starts
            // with a dash or matches a CLI command such as `resume`.
            args.extend(["--".into(), message.into()]);
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
        opts: &ExecOptions,
        agent_session_id: &str,
        cwd: Option<&Path>,
        _source_path: Option<&str>,
    ) -> Result<AgentCommand> {
        // codex resume [OPTIONS] [SESSION_ID]
        let mut args = vec!["resume".into()];
        args.extend(runtime_args(opts));
        args.push(agent_session_id.into());
        Ok(AgentCommand {
            program: install.executable_path.clone(),
            args,
            cwd: cwd.map(|p| p.to_path_buf()),
        })
    }

    fn build_exec_command(
        &self,
        install: &AgentInstallation,
        opts: &ExecOptions,
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

    fn build_context_extraction_command(
        &self,
        install: &AgentInstallation,
        opts: &ExecOptions,
        prompt: &str,
        runtime_dir: &Path,
    ) -> Result<AgentCommand> {
        let mut args: Vec<String> = vec![
            "exec".into(),
            "--ephemeral".into(),
            "-s".into(),
            "read-only".into(),
            "--skip-git-repo-check".into(),
        ];
        args.extend(runtime_args(opts));
        args.push(prompt.to_string());
        Ok(AgentCommand {
            program: install.executable_path.clone(),
            args,
            cwd: Some(runtime_dir.to_path_buf()),
        })
    }

    /// Codex 的桌面端是 ChatGPT（com.openai.codex），与 CLI 共用 `~/.codex`
    /// 的会话存储——终端恢复对桌面端创建的 thread 同样有效，所以 continue
    /// 仍走终端；这里只是让设置页把 ChatGPT 列为 Codex 的桌面端。
    fn desktop_app_name(&self) -> Option<&'static str> {
        Some("ChatGPT")
    }

    /// 桌面端打开方式：注册的深链 `codex://threads/{id}` 按 thread id 定位，
    /// 与终端恢复指向同一个存储。应用缺席 → 拒绝；终端始终是 Codex 的默认
    /// 打开方式，这条路只在保存的偏好要求桌面端时被换入。
    fn desktop_resume_route(&self, session: &Session) -> ResumeRoute {
        if !crate::platform::paths::app_bundle_present("ChatGPT") {
            return ResumeRoute::Refused(
                "未找到 ChatGPT（Codex 桌面端），无法在桌面端打开该会话".into(),
            );
        }
        ResumeRoute::Desktop(DesktopResume {
            uri: format!("codex://threads/{}", session.root_agent_session_id),
            note: "将在 ChatGPT（Codex 桌面端）中打开该会话".into(),
        })
    }
}

/// One rollout line's contribution to the member read. Model-provenance state
/// events (`turn_context`, `thread_settings_applied`) contribute nothing since
/// the provenance retirement — only conversation prose does.
fn parse_line(v: &Value, is_root: bool) -> Option<ParsedLine> {
    let vtype = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let payload = v.get("payload").cloned().unwrap_or(Value::Null);
    let source_message_id = json_str_field(v, "id")
        .or_else(|| json_str_field(&payload, "id"))
        .map(|s| s.to_string());

    match vtype {
        // Provenance state events: not conversation, not tracked.
        "turn_context" => None,
        "event_msg"
            if payload.get("type").and_then(|t| t.as_str()) == Some("thread_settings_applied") =>
        {
            None
        }
        "response_item" => {
            let ptype = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match ptype {
                "message" => {
                    let role = payload.get("role").and_then(|r| r.as_str()).unwrap_or("");
                    let mut text = extract_text(payload.get("content").unwrap_or(&Value::Null));
                    if text.trim().is_empty() {
                        return None;
                    }
                    // Conversation is the ROOT member's user/assistant prose
                    // only; every member's own turns are counted. Injected
                    // context (`<…>` blocks, machine `#` headers) is neither;
                    // an envelope wrapping the real request contributes the
                    // wrapped words.
                    let role = match role {
                        "user" => match crate::adapters::user_request_text(&text) {
                            Some(user_text) => {
                                text = user_text;
                                SessionMessageRole::User
                            }
                            None => return None,
                        },
                        "assistant" => SessionMessageRole::Assistant,
                        // developer/system prompts: not conversation, not counted.
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
                // A message that crossed between two threads: execution
                // traffic — topology lives in discovery, the text is not
                // conversation.
                "agent_message" => None,
                // Tool traffic is deliberately not ingested:
                // machine chatter whose payload shape also drifts across
                // Codex versions (function_call vs custom_tool_call).
                "function_call" | "custom_tool_call" => None,
                "function_call_output" | "custom_tool_call_output" => None,
                "reasoning" => None, // internal model reasoning: not conversation
                _ => None,
            }
        }
        "event_msg" => {
            // token_count (usage), compact, task bookkeeping: none of it is
            // conversation, and usage is no longer recorded.
            None
        }
        _ => None, // session_meta and unrecognized payloads carry nothing
    }
}

/// Codex's own name for each thread: `<root>/session_index.jsonl`, one
/// `{"id": <thread id>, "thread_name": <name>}` per line.
///
/// This file mirrors `threads.name` in `~/.codex/state_5.sqlite`, so reading it
/// needs neither the WAL nor the version number baked into that file's name.
struct ThreadNames(HashMap<String, String>);

impl ThreadNames {
    const FILE: &'static str = "session_index.jsonl";

    fn load(roots: &[PathBuf]) -> Self {
        let mut map = HashMap::new();
        for root in roots {
            let Ok(lines) = crate::adapters::read_jsonl_lines(&root.join(Self::FILE)) else {
                continue;
            };
            for (_, line) in &lines {
                let Ok(v) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                let (Some(id), Some(name)) =
                    (json_str_field(&v, "id"), json_str_field(&v, "thread_name"))
                else {
                    continue;
                };
                if !name.trim().is_empty() {
                    map.insert(id.to_string(), name.to_string());
                }
            }
        }
        Self(map)
    }

    /// The name, unless it is merely the first user message again.
    ///
    /// The index holds the runtime's placeholder until the model names the
    /// thread (the first user text, verbatim or cut at 60 chars with `…`).
    /// Comparing against `first_user_text` — which the parser already found — is
    /// exact where a length or ellipsis rule would be a guess.
    fn title_for(&self, thread_id: &str, first_user_text: Option<&str>) -> Option<String> {
        let name = self.0.get(thread_id)?;
        let placeholder =
            first_user_text.is_some_and(|t| t.starts_with(name.trim_end_matches('…')));
        (!placeholder).then(|| name.clone())
    }
}

#[cfg(test)]
mod rollout_tests {
    use super::*;
    use crate::adapters::{AgentAdapter, DiscoveredMemberKind, MemberReadDelta};

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

    fn meta_line_with(payload: serde_json::Value) -> String {
        serde_json::json!({
            "timestamp": "2026-09-20T13:01:47.832Z", "ordinal": 0,
            "type": "session_meta", "payload": payload
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

    fn tool_line(ordinal: usize) -> String {
        serde_json::json!({
            "timestamp": "2026-09-20T13:01:48.000Z", "ordinal": ordinal,
            "type": "response_item",
            "payload": { "type": "function_call", "id": format!("c{ordinal}"), "name": "shell" }
        })
        .to_string()
    }

    fn reasoning_line(ordinal: usize) -> String {
        serde_json::json!({
            "timestamp": "2026-09-20T13:01:48.000Z", "ordinal": ordinal,
            "type": "response_item",
            "payload": { "type": "reasoning", "id": format!("r{ordinal}"), "summary": [] }
        })
        .to_string()
    }

    fn compact_line(ordinal: usize) -> String {
        serde_json::json!({
            "timestamp": "2026-09-20T13:01:49.000Z", "ordinal": ordinal,
            "type": "event_msg", "payload": { "type": "compact" }
        })
        .to_string()
    }

    fn root_session(path: &Path) -> Session {
        Session {
            id: "sess-codex".into(),
            agent: Agent::Codex,
            root_agent_session_id: SESSION_ID.into(),
            title: None,
            cwd: None,
            workspace_path_id: None,
            project_id: None,
            owner_workstream_id: None,
            forked_from_session_id: None,
            started_at: None,
            last_activity_at: None,
            last_conversation_at: None,
            trashed_at: None,
            source_kind: "codex_rollout".into(),
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

    /// ChatGPT（Codex 桌面端）是 codex 格式的第二种打开方式：应用在场时按
    /// thread 深链定位（与终端恢复同一存储），缺席时拒绝——终端默认不受影响。
    #[test]
    fn the_desktop_route_deep_links_the_thread_when_chatgpt_is_present() {
        let session = root_session(Path::new("/tmp/rollout.jsonl"));
        match CodexAdapter.desktop_resume_route(&session) {
            ResumeRoute::Desktop(open) => {
                assert!(
                    open.uri.starts_with("codex://threads/01a0bee7"),
                    "the deep link must carry the thread id: {open:?}"
                );
            }
            ResumeRoute::Refused(reason) => {
                assert!(
                    !crate::platform::paths::app_bundle_present("ChatGPT"),
                    "ChatGPT is installed, so the route must not refuse: {reason}"
                );
            }
            other => panic!("unexpected route: {other:?}"),
        }
    }

    /// Regression: session_meta opens every rollout file, and stopping at the
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
        let m = CodexAdapter::parse_member(&path).unwrap().unwrap();
        assert_eq!(m.source_member_id, SESSION_ID);
        assert_eq!(m.kind, DiscoveredMemberKind::Root);
        assert_eq!(m.cwd.as_deref(), Some("/tmp/proj"));
        assert_eq!(
            m.first_user_text.as_deref(),
            Some("我想对整体工程进行代码瘦身，请给出优化方案")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A forked page carries `_<own uuid>` in its name yet still names the
    /// thread it forked FROM in `payload.id`: identity must follow the name, or
    /// the fork collapses back into the thread it came from.
    #[test]
    fn a_forked_page_is_its_own_logical_root_with_the_fork_as_parent() {
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
        let m = CodexAdapter::parse_member(&path).unwrap().unwrap();
        assert_eq!(m.source_member_id, OWN, "the file's name owns the identity");
        assert_eq!(m.kind, DiscoveredMemberKind::ForkRoot);
        assert_eq!(m.parent_source_member_id.as_deref(), Some(BASE));
        assert_eq!(m.first_user_text.as_deref(), Some("接着上面继续"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `session_meta.forked_from_id` is NOT the forked-PAGE marker: it means the
    /// thread inherited another thread's history, which is what every
    /// forked-history subagent does. Read as a page marker it would turn each of
    /// them into its own Logical Session.
    #[test]
    fn an_inherited_history_does_not_make_a_subagent_a_fork_root() {
        const PARENT: &str = "019f135a-621c-76a1-a76c-7c71021847aa";
        let dir = temp_dir("forked-history-subagent");
        let path = write_rollout(
            &dir,
            &format!("rollout-2026-09-24T10-00-00-{SESSION_ID}.jsonl"),
            &[meta_line_with(serde_json::json!({
                "session_id": PARENT, "id": SESSION_ID, "cwd": "/tmp/proj",
                "thread_source": "subagent", "parent_thread_id": PARENT,
                "forked_from_id": PARENT, "history_mode": "paginated"
            }))],
        );
        let m = CodexAdapter::parse_member(&path).unwrap().unwrap();
        assert_eq!(m.kind, DiscoveredMemberKind::Child);
        assert_eq!(m.source_member_id, SESSION_ID, "the meta still owns it");
        assert_eq!(m.parent_source_member_id.as_deref(), Some(PARENT));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `thread_source=subagent` → Child, `guardian_review` → Side;
    /// both carry the parent as the topology hint and neither carries any
    /// title source.
    #[test]
    fn internal_thread_sources_map_to_child_and_side() {
        let dir = temp_dir("internal-kinds");
        let parent = "019f135a-621c-76a1-a76c-7c71021847aa";
        let sub = write_rollout(
            &dir,
            &format!("rollout-2026-09-20T21-01-47-{SESSION_ID}.jsonl"),
            &[meta_line_with(serde_json::json!({
                "session_id": SESSION_ID, "id": SESSION_ID, "cwd": "/tmp/proj",
                "thread_source": "subagent", "parent_thread_id": parent
            }))],
        );
        let m = CodexAdapter::parse_member(&sub).unwrap().unwrap();
        assert_eq!(m.kind, DiscoveredMemberKind::Child);
        assert_eq!(m.parent_source_member_id.as_deref(), Some(parent));
        assert_eq!(m.root_hint.as_deref(), Some(parent));
        assert_eq!(m.native_title, None);
        assert_eq!(m.first_user_text, None);

        const GID: &str = "01b0bee7-6afb-7622-afcd-e26c61dd545d";
        let review = write_rollout(
            &dir,
            &format!("rollout-2026-09-20T22-01-47-{GID}.jsonl"),
            &[meta_line_with(serde_json::json!({
                "session_id": GID, "id": GID, "cwd": "/tmp/proj",
                "thread_source": "guardian_review", "parent_thread_id": parent
            }))],
        );
        let m = CodexAdapter::parse_member(&review).unwrap().unwrap();
        assert_eq!(m.kind, DiscoveredMemberKind::Side);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Codex's background memory pass is internal too, and its machine-written
    /// "user" turns must not name anything.
    #[test]
    fn a_memory_consolidation_thread_is_internal() {
        let dir = temp_dir("memory-consolidation");
        const MID: &str = "01b0bee7-6afb-7622-afcd-e26c61dd545e";
        let path = write_rollout(
            &dir,
            &format!("rollout-2026-09-21T09-01-47-{MID}.jsonl"),
            &[
                meta_line_with(serde_json::json!({
                    "session_id": MID, "id": MID, "cwd": "/tmp/proj",
                    "thread_source": "memory_consolidation"
                })),
                serde_json::json!({
                    "type": "response_item",
                    "payload": {
                        "type": "message", "role": "user",
                        "content": [{"type": "input_text", "text": "Summarize what to remember."}]
                    }
                })
                .to_string(),
            ],
        );
        let m = CodexAdapter::parse_member(&path).unwrap().unwrap();
        assert_eq!(m.kind, DiscoveredMemberKind::Child);
        assert_eq!(
            m.first_user_text, None,
            "its user turns are prompts Codex wrote"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A just-started session has a meta line but no user turn yet: parse
    /// still succeeds (scanning to EOF) and simply yields no user text.
    #[test]
    fn meta_only_file_parses_without_user_text() {
        let dir = temp_dir("meta-only");
        let path = write_rollout(&dir, "rollout-meta-only.jsonl", &[meta_line()]);
        let m = CodexAdapter::parse_member(&path).unwrap().unwrap();
        assert_eq!(m.source_member_id, SESSION_ID);
        assert_eq!(m.cwd.as_deref(), Some("/tmp/proj"));
        assert_eq!(m.first_user_text, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Root conversation vs execution observations

    /// Only root user/assistant prose becomes messages; tool traffic contributes
    /// activity counts, while thinking and compression markers are filtered.
    /// Two visible assistant prose segments around a tool call are BOTH kept.
    #[test]
    fn the_root_read_keeps_prose_and_counts_the_machine_traffic() {
        let dir = temp_dir("root-read");
        let path = write_rollout(
            &dir,
            "rollout-parent.jsonl",
            &[
                meta_line(),
                message_line(1, "user", "先检查一下代码"),
                message_line(2, "assistant", "我先看现状。"),
                reasoning_line(3),
                tool_line(4),
                message_line(5, "assistant", "问题在这里，已经修改完成。"),
                // `total_token_usage` is cumulative for the thread and must NOT
                // be the number counted; `last_token_usage` is this turn's own.
                r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":99999,"output_tokens":88888},"last_token_usage":{"input_tokens":1234,"cached_input_tokens":1000,"output_tokens":56,"reasoning_output_tokens":7}}}}"#.to_string(),
                compact_line(6),
                message_line(7, "developer", "<permissions>"),
            ],
        );
        let delta = CodexAdapter
            .read_session_delta(
                &root_session(&path),
                &crate::domain::SourceCursor::default(),
            )
            .unwrap();
        let msgs = &delta.messages;
        assert_eq!(msgs.len(), 3, "both visible prose segments survive");
        assert_eq!(msgs[0].role, SessionMessageRole::User);
        assert_eq!(msgs[0].content, "先检查一下代码");
        assert_eq!(msgs[1].role, SessionMessageRole::Assistant);
        assert_eq!(
            msgs[1].content, "我先看现状。",
            "prose before the tool call"
        );
        assert_eq!(
            msgs[2].content, "问题在这里，已经修改完成。",
            "prose after the tool call"
        );
        assert_eq!(msgs[0].source_message_id.as_deref(), Some("m1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An injected preamble in the user role is not the user's words.
    #[test]
    fn an_injected_user_block_is_not_conversation() {
        let dir = temp_dir("injected");
        let path = write_rollout(
            &dir,
            "rollout-injected.jsonl",
            &[
                meta_line(),
                message_line(
                    1,
                    "user",
                    "<environment_context>\ncwd: /tmp/proj\n</environment_context>",
                ),
                message_line(2, "user", "# AGENTS.md instructions"),
                message_line(3, "user", "真正的问题"),
            ],
        );
        let delta = CodexAdapter
            .read_session_delta(
                &root_session(&path),
                &crate::domain::SourceCursor::default(),
            )
            .unwrap();
        assert_eq!(delta.messages.len(), 1);
        assert_eq!(delta.messages[0].content, "真正的问题");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Inter-agent envelopes are execution observations, never conversation
    /// — on the root and on internal members alike.
    #[test]
    fn an_agent_message_envelope_is_observed_not_stored() {
        let dir = temp_dir("envelope");
        let envelope = serde_json::json!({
            "timestamp": "2026-09-20T13:01:49.000Z", "ordinal": 9,
            "type": "response_item",
            "payload": {
                "type": "agent_message", "id": "amsg_9",
                "author": "/root/design_review", "recipient": "/root",
                "content": [{ "type": "input_text", "text": "Message Type: FINAL_ANSWER\nPayload:\n改完了" }]
            }
        })
        .to_string();
        let path = write_rollout(
            &dir,
            "rollout-parent.jsonl",
            &[
                meta_line(),
                envelope,
                message_line(10, "assistant", "收到，已合并。"),
            ],
        );
        let delta: MemberReadDelta = CodexAdapter
            .read_session_delta(
                &root_session(&path),
                &crate::domain::SourceCursor::default(),
            )
            .unwrap();
        assert_eq!(delta.messages.len(), 1, "the envelope is not conversation");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // The session index as a title source

    fn index_line(id: &str, name: &str) -> String {
        serde_json::json!({ "id": id, "thread_name": name, "updated_at": "2026-09-20T13:00:00Z" })
            .to_string()
    }

    /// The name Codex gives a thread is its title, and the transcript has no
    /// copy of it — discovery has to go to the index for it.
    #[test]
    fn the_index_names_a_thread_that_its_transcript_cannot() {
        let dir = temp_dir("index-named");
        write_rollout(
            &dir,
            &format!("rollout-2026-09-20T21-01-47-{SESSION_ID}.jsonl"),
            &[
                meta_line(),
                message_line(
                    3,
                    "user",
                    "任务看板界面右上角的那个新建任务按钮 去掉 前边的“+”",
                ),
            ],
        );
        std::fs::write(
            dir.join("session_index.jsonl"),
            format!("{}\n", index_line(SESSION_ID, "移除新建任务按钮加号")),
        )
        .unwrap();
        let found = CodexAdapter
            .discover_members_in(&[dir.clone()], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].native_title.as_deref(),
            Some("移除新建任务按钮加号")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A continuation rollout embeds the RESUMED thread's meta in its replayed
    /// history. Identity must come from the rollout's OWN (first) meta, and
    /// the replayed meta must not re-key the file to the old session — two
    /// rollouts resolving to one identity fight over the member, and the
    /// row churns on every discovery pass (observed: generation 21).
    #[test]
    fn a_continuation_file_keeps_its_own_identity() {
        let dir = temp_dir("continuation-identity");
        const OLD: &str = "01a0aa00-0000-0000-0000-000000000001";
        const NEW: &str = "01a0bb00-0000-0000-0000-000000000002";

        // The resumed thread's own rollout: carries its usage.
        write_rollout(
            &dir,
            &format!("rollout-2026-09-26T20-57-43-{OLD}.jsonl"),
            &[
                serde_json::json!({
                    "timestamp": "2026-09-26T20:57:43.000Z", "ordinal": 0,
                    "type": "session_meta",
                    "payload": {"session_id": OLD, "id": OLD, "cwd": "/repo"}
                })
                .to_string(),
                serde_json::json!({
                    "timestamp": "2026-09-26T20:58:00.000Z", "ordinal": 1,
                    "type": "event_msg",
                    "payload": {"type": "token_count", "info": {
                        "total_token_usage": {"input_tokens": 1000, "output_tokens": 100, "cached_input_tokens": 900, "reasoning_output_tokens": 10},
                        "last_token_usage": {"input_tokens": 1000, "output_tokens": 100, "cached_input_tokens": 900, "reasoning_output_tokens": 10}
                    }}
                })
                .to_string(),
            ],
        );
        // The continuation: first meta names ITSELF (NEW, resuming OLD), the
        // replayed history re-declares OLD. Last-meta-wins would key this file
        // to OLD and both rollouts would fight over one member.
        write_rollout(
            &dir,
            &format!("rollout-2026-09-26T21-39-40-{NEW}.jsonl"),
            &[
                serde_json::json!({
                    "timestamp": "2026-09-26T21:39:40.000Z", "ordinal": 0,
                    "type": "session_meta",
                    "payload": {"session_id": OLD, "id": NEW, "cwd": "/repo"}
                })
                .to_string(),
                serde_json::json!({
                    "timestamp": "2026-09-26T21:39:40.000Z", "ordinal": 1,
                    "type": "session_meta",
                    "payload": {"session_id": OLD, "id": OLD, "cwd": "/repo"}
                })
                .to_string(),
                message_line(2, "user", "继续这个任务"),
            ],
        );

        let found = CodexAdapter
            .discover_members_in(&[dir.clone()], &|_| false)
            .unwrap();
        let mut ids: Vec<&str> = found
            .iter()
            .map(|m| m.source_member_id.as_str())
            .collect::<Vec<_>>();
        ids.sort();
        assert_eq!(
            ids,
            vec![OLD, NEW],
            "each rollout keys to its OWN first meta: {found:?}"
        );

        // And the resolved identity is stable across repeated discovery —
        // the old bug re-keyed the continuation file to OLD on some passes.
        let again = CodexAdapter
            .discover_members_in(&[dir.clone()], &|_| false)
            .unwrap();
        let mut ids_again: Vec<&str> = again
            .iter()
            .map(|m| m.source_member_id.as_str())
            .collect::<Vec<_>>();
        ids_again.sort();
        assert_eq!(ids, ids_again);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Until the model names the thread, the index holds the first user message
    /// — cut at 60 chars with `…`, or verbatim when it is shorter. That is the
    /// same string NoEnding derives, so it must not be promoted over it.
    #[test]
    fn a_placeholder_name_is_not_a_native_title() {
        let dir = temp_dir("index-placeholder");
        let long = "这是一个初始godot引擎工程，我该如何在该工程里使用godot mcp：/Users/jqk/projects/r/godot-mcp";
        let cut: String = long.chars().take(59).collect();
        // Shorter than the cut: the index stores the message verbatim.
        let short = "如何让你能够接管收发微信消息？";
        write_rollout(
            &dir,
            &format!("rollout-2026-09-20T21-01-47-{SESSION_ID}.jsonl"),
            &[meta_line(), message_line(3, "user", long)],
        );
        std::fs::write(
            dir.join("session_index.jsonl"),
            format!(
                "{}\n{}\n",
                index_line(SESSION_ID, &format!("{cut}…")),
                index_line("019ff12a-3a13-7553-b629-9c7403deb658", short)
            ),
        )
        .unwrap();
        let found = CodexAdapter
            .discover_members_in(&[dir.clone()], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].native_title, None,
            "a cut first message is not a name"
        );
        assert_eq!(found[0].first_user_text.as_deref(), Some(long));
        let names = ThreadNames::load(&[dir.clone()]);
        assert_eq!(
            names.title_for("019ff12a-3a13-7553-b629-9c7403deb658", Some(short)),
            None,
            "verbatim-equal to the first message is the same placeholder"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn turn_context_line(ordinal: usize, model: &str) -> String {
        serde_json::json!({
            "timestamp": "2026-09-20T13:02:00.000Z", "ordinal": ordinal,
            "type": "turn_context",
            "payload": { "turn_id": format!("t{ordinal}"), "model": model }
        })
        .to_string()
    }

    fn thread_settings_line(ordinal: usize, provider: &str, model: &str) -> String {
        serde_json::json!({
            "timestamp": "2026-09-20T13:01:59.000Z", "ordinal": ordinal,
            "type": "event_msg",
            "payload": {
                "type": "thread_settings_applied",
                "thread_settings": { "model": model, "model_provider_id": provider }
            }
        })
        .to_string()
    }

    /// Provenance state events (`turn_context`, `thread_settings_applied`)
    /// are consumed silently since the provenance retirement: they neither
    /// break the read nor carry model facts onto messages.
    #[test]
    fn state_events_are_consumed_without_attributing_anything() {
        let dir = temp_dir("prov-consumed");
        let path = write_rollout(
            &dir,
            "rollout-prov.jsonl",
            &[
                meta_line(),
                thread_settings_line(1, "openai", "gpt-5.6-terra"),
                turn_context_line(2, "gpt-5.6-terra"),
                message_line(3, "user", "第一问"),
                message_line(4, "assistant", "回答一"),
                thread_settings_line(10, "openai", "gpt-5.6-luna"),
                turn_context_line(11, "gpt-5.6-luna"),
                message_line(12, "assistant", "回答二"),
            ],
        );
        let delta = CodexAdapter
            .read_session_delta(
                &root_session(&path),
                &crate::domain::SourceCursor::default(),
            )
            .unwrap();
        let roles: Vec<SessionMessageRole> = delta.messages.iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            vec![
                SessionMessageRole::User,
                SessionMessageRole::Assistant,
                SessionMessageRole::Assistant
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
