//! Codex Adapter: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`.
//! `CODEX_HOME` overrides the root. Raw data is read-only during normal
//! operation; the one exception is the adapter-owned, user-confirmed
//! permanent source deletion below (方案 §39).

use std::collections::HashMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::{
    detect_format, json_str_field, read_first_json_line, read_jsonl_delta, truncate_text,
    AgentCommand, DiscoveredSession, ParsedLine, ReadDelta,
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

/// Codex opens every envelope with `Message Type: <X>`; the remaining lines
/// (Task name / Sender / Payload) are the body and are stored verbatim.
fn message_type_of(text: &str) -> Option<String> {
    text.lines()
        .next()?
        .strip_prefix("Message Type:")
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// The agent path one level up (`/root/foo` → `/root`; `/root` → `/`).
fn parent_path_of(path: &str) -> Option<String> {
    path.rsplit_once('/')
        .map(|(head, _)| if head.is_empty() { "/" } else { head }.to_string())
}

/// The other end of an inter-agent message, resolved as far as THIS file's own
/// records allow (方案 §37.13).
///
/// Codex logs a message only in the receiver's transcript — all 135 envelopes
/// on this machine carry `recipient` == the reading file's own agent path — so
/// the counterpart is always the `author` and the only missing piece is that
/// author's thread id. Two sources, both file-local: the `SubAgentActivity`
/// items this file wrote when it spawned a thread (100/135), and its own
/// `parent_thread_id` for a message from above (33/135). The remaining 2 are
/// peer/grandchild paths spawned by a shared ancestor, whose mapping lives in
/// that ancestor's file — they keep the path and get no id.
struct Peers {
    /// Whether this rollout is a thread Codex wrote itself (§37.12).
    internal: bool,
    /// This file's own agent path (`/root/foo`), from its session meta.
    own_path: Option<String>,
    parent_thread_id: Option<String>,
    spawned: HashMap<String, String>,
}

impl Peers {
    fn load(path: &Path) -> Self {
        let meta = match read_first_json_line(path) {
            Some(v) => match v.get("payload") {
                Some(p) => p.clone(),
                None => v,
            },
            None => Value::Null,
        };
        let spawn = meta.pointer("/source/subagent/thread_spawn");
        let mut spawned = HashMap::new();
        // The mapping lives wherever the spawn happened, which may sit earlier
        // in this file than the envelope that needs it, and may have been
        // consumed by an earlier incremental read — so it is collected from the
        // whole file rather than from the delta. Streaming keeps a long
        // transcript from being buffered twice.
        if let Ok(file) = std::fs::File::open(path) {
            for line in std::io::BufReader::new(file).lines() {
                let Ok(line) = line else { break };
                // Cheap gate: most rollout lines cannot carry this mapping, and
                // parsing the parent's whole transcript just to skip it is the
                // expensive half of a reconcile pass.
                if !line.contains("SubAgentActivity") {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let p = v.get("payload").unwrap_or(&v);
                let item = p.get("item").unwrap_or(&Value::Null);
                if item.get("type").and_then(|t| t.as_str()) != Some("SubAgentActivity") {
                    continue;
                }
                if let (Some(agent_path), Some(thread_id)) = (
                    json_str_field(item, "agent_path"),
                    json_str_field(item, "agent_thread_id"),
                ) {
                    spawned.insert(agent_path.to_string(), thread_id.to_string());
                }
            }
        }
        // A root rollout has no `source.subagent.thread_spawn`, so its own path
        // is nowhere in its meta — but its children's paths are, and they all
        // sit under it, so the deepest common one is its own. Without this a
        // root transcript could not tell a child's report from a peer's.
        let own_path = spawn
            .and_then(|s| json_str_field(s, "agent_path"))
            .map(str::to_string)
            .or_else(|| {
                let mut keys: Vec<&str> = spawned.keys().map(String::as_str).collect();
                keys.sort_unstable();
                let first = keys.first()?;
                let last = keys.last()?;
                let common = first
                    .char_indices()
                    .zip(last.chars())
                    .take_while(|((_, a), b)| a == b)
                    .map(|((i, _), _)| i)
                    .last()?;
                let cut = first[..common + 1].rfind('/')?;
                Some(first[..cut].to_string())
            });
        Self {
            internal: matches!(
                meta.get("thread_source").and_then(|t| t.as_str()),
                Some("subagent") | Some("guardian_review")
            ),
            own_path,
            parent_thread_id: json_str_field(&meta, "parent_thread_id").map(str::to_string),
            spawned,
        }
    }

    /// The counterpart's thread id, or `None` rather than a guess.
    fn thread_id_of(&self, author: &str) -> Option<String> {
        if let Some(id) = self.spawned.get(author) {
            return Some(id.clone());
        }
        // A message from above names the parent, whose path is my own path
        // minus its last segment (`/root/foo` → `/root`).
        let parent_path = self.own_path.as_deref().and_then(parent_path_of);
        if Some(author) == parent_path.as_deref() {
            return self.parent_thread_id.clone();
        }
        None
    }

    /// Which side of the thread tree the counterpart sits on — the question a
    /// reader of a subagent transcript actually has (`<` the task that started
    /// it, or a report handed back?).
    ///
    /// Derived from the two agent paths alone, so it also covers the envelopes
    /// whose thread id could not be resolved. It is a DIRECTION, not a
    /// generation: Codex nests deeper than two levels, and `counterpart_agent_path`
    /// always carries the exact path for anyone who needs the depth.
    fn role_of(&self, author: &str) -> Option<&'static str> {
        let own = self.own_path.as_deref()?;
        if author.starts_with(&format!("{own}/")) {
            return Some("child");
        }
        if own.starts_with(&format!("{author}/")) {
            return Some("parent");
        }
        if parent_path_of(author).as_deref() == parent_path_of(own).as_deref() {
            return Some("sibling");
        }
        None
    }
}

/// Codex's own name for each thread: `<root>/session_index.jsonl`, one
/// `{"id": <thread id>, "thread_name": <name>}` per line (方案 §37.16).
///
/// `[实测]` this file is a row-for-row mirror of `threads.name` in
/// `~/.codex/state_5.sqlite` (31 rows, same ids, same names, 0 differences), so
/// reading it needs neither the WAL nor the version number baked into that
/// file's name (`state_5` → the next release's `state_6`).
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
    /// thread: 4 of this machine's 31 names are their own transcript's first
    /// user text (3 verbatim, 1 cut at 60 chars with `…`). Comparing against
    /// `first_user_text` — which the parser already found — is exact where a
    /// length or ellipsis rule would be a guess.
    fn title_for(&self, thread_id: &str, first_user_text: Option<&str>) -> Option<String> {
        let name = self.0.get(thread_id)?;
        let placeholder =
            first_user_text.is_some_and(|t| t.starts_with(name.trim_end_matches('…')));
        (!placeholder).then(|| name.clone())
    }
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
        let mut first_agent_text = None;

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
            } else if vtype == Some("response_item") {
                let p = v.get("payload").unwrap_or(&v);
                if p.get("type").and_then(|t| t.as_str()) == Some("message") {
                    let role = p.get("role").and_then(|r| r.as_str()).unwrap_or("");
                    let text = extract_text(p.get("content").unwrap_or(&Value::Null));
                    // `<…>` environment_context wrappers and `#`-prefixed
                    // injections (AGENTS.md, attached-file headers) are not
                    // user text.
                    if role == "user"
                        && first_user_text.is_none()
                        && !internal_thread
                        && !text.is_empty()
                        && !crate::adapters::is_injected_preamble(&text)
                    {
                        first_user_text = Some(truncate_text(&text, 400));
                    }
                    // The last resort for a title: what the thread itself said
                    // first. Only consulted when there is no user text, so it
                    // costs nothing on the common path.
                    if role == "assistant" && first_agent_text.is_none() && !text.is_empty() {
                        first_agent_text = Some(text);
                    }
                }
            }
            // session_meta opens a rollout file, so stopping at the meta
            // line itself would leave first_user_text — and every title —
            // unset. Stop once the meta is parsed and there is a user text to
            // name the session after; a thread Codex wrote itself has no user
            // text, so it scans on until its own first reply (or EOF, for a
            // meta-only session).
            if meta_seen
                && (first_user_text.is_some() || (internal_thread && first_agent_text.is_some()))
            {
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
            // Codex writes no title of its own anywhere in a rollout. Its name
            // for the thread lives in the session index, and discovery fills
            // this in from there (§37.16).
            native_title: None,
            first_user_text,
            first_agent_text: first_agent_text.as_deref().map(|t| truncate_text(t, 400)),
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
                match Self::parse_rollout(&p) {
                    Ok(Some(mut s)) => {
                        s.native_title =
                            names.title_for(&s.agent_session_id, s.first_user_text.as_deref());
                        out.push(s)
                    }
                    Ok(None) => {}
                    Err(e) => eprintln!("[discover] skip {}: {}", p.display(), e),
                }
            }
        }
        Ok(out)
    }

    fn read_delta(&self, session: &Session, cursor: &SourceCursor) -> Result<ReadDelta> {
        let path = PathBuf::from(&session.raw_path);
        // Internal review threads contribute only their own replies: their user
        // turns are prompts Codex wrote and their developer turns are its
        // instructions — machine chatter in the user role, which is the same
        // call ZCode makes from `semantics` (方案 §36.11 / §37.12). Their
        // inter-agent envelopes ARE kept: that is the task they were handed
        // (§37.13).
        let peers = Peers::load(&path);
        let internal = peers.internal;
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
                        // A message that crossed between two threads: the
                        // counterpart lives in the source's own id space, and
                        // resolving it to a NoEnding session is the reader's
                        // job — adapters stay database-free (方案 §37.13).
                        "agent_message" => {
                            let text = extract_text(payload.get("content").unwrap_or(&Value::Null));
                            if text.trim().is_empty() {
                                return None;
                            }
                            ("agent_message", text)
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

            // An internal review thread contributes only its own replies and the
            // envelopes it received; every other line is a prompt Codex wrote, a
            // compaction marker, or the synthetic session-meta event.
            if internal && kind != "assistant_message" && kind != "agent_message" {
                return None;
            }

            let mut metadata = serde_json::Map::new();
            metadata.insert("agent".into(), Value::String("codex".into()));
            metadata.insert("type".into(), Value::String(vtype.into()));
            if kind == "agent_message" {
                let author = payload
                    .get("author")
                    .and_then(|a| a.as_str())
                    .unwrap_or("")
                    .to_string();
                if !author.is_empty() {
                    metadata.insert(
                        "counterpart_agent_path".into(),
                        Value::String(author.clone()),
                    );
                }
                if let Some(role) = peers.role_of(&author) {
                    metadata.insert("counterpart_role".into(), Value::String(role.into()));
                }
                if let Some(id) = peers.thread_id_of(&author) {
                    metadata.insert("counterpart_source_id".into(), Value::String(id));
                }
                if let Some(mt) = message_type_of(&text) {
                    metadata.insert("message_type".into(), Value::String(mt));
                }
            }

            Some(ParsedLine {
                kind: kind.into(),
                text: Some(text),
                source_event_id,
                metadata: Value::Object(metadata),
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
    use crate::adapters::AgentAdapter;

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
    /// machine text in the list (方案 §37.12). What the thread itself said
    /// first IS available, though, and is the last resort for a title (§37.15).
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
        assert_eq!(
            d.first_agent_text.as_deref(),
            Some("{\"outcome\":\"allow\"}"),
            "the thread's own first words are kept — they are what's left to name it by"
        );
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

    // ---- Inter-agent envelopes (方案 §37.13) ------------------------------

    const CHILD_THREAD: &str = "019fbbd3-47be-78f2-89fd-9deec2e4c6d9";

    /// `SubAgentActivity` is how a transcript records a thread it spawned, and
    /// it is the only place that maps an agent path to a thread id.
    fn spawn_line(ordinal: usize, agent_path: &str, thread_id: &str) -> String {
        serde_json::json!({
            "timestamp": "2026-09-20T13:01:48.000Z", "ordinal": ordinal,
            "type": "event_msg",
            "payload": {
                "type": "item_completed", "thread_id": SESSION_ID, "turn_id": "t1",
                "item": {
                    "type": "SubAgentActivity", "id": format!("call_{ordinal}"),
                    "kind": "started", "agent_thread_id": thread_id, "agent_path": agent_path
                }
            }
        })
        .to_string()
    }

    fn envelope_line(ordinal: usize, author: &str, recipient: &str, body: &str) -> String {
        serde_json::json!({
            "timestamp": "2026-09-20T13:01:49.000Z", "ordinal": ordinal,
            "type": "response_item",
            "payload": {
                "type": "agent_message", "id": format!("amsg_{ordinal}"),
                "author": author, "recipient": recipient,
                "content": [
                    { "type": "input_text", "text": format!(
                        "Message Type: FINAL_ANSWER\nTask name: {recipient}\nSender: {author}\nPayload:\n{body}") },
                    { "type": "encrypted_content", "encrypted_content": "zzz" }
                ]
            }
        })
        .to_string()
    }

    fn session_at(path: &Path) -> Session {
        Session {
            id: "sess-codex".into(),
            agent: Agent::Codex,
            agent_session_id: SESSION_ID.into(),
            title: None,
            cwd: None,
            workspace_path_id: None,
            project_id: None,
            raw_path: path.to_string_lossy().to_string(),
            parent_agent_session_id: None,
            started_at: None,
            last_activity_at: None,
            trashed_at: None,
        }
    }

    fn metadata_of(delta: &ReadDelta, kind: &str) -> serde_json::Value {
        delta
            .events
            .iter()
            .find(|e| e.kind == kind)
            .unwrap_or_else(|| panic!("no {kind} event in {:?}", delta.events.len()))
            .metadata
            .clone()
    }

    /// A message from a thread this transcript spawned: the mapping is in its
    /// own `SubAgentActivity` items, so the counterpart resolves to a thread id
    /// (not a guess at a path) — 100 of this machine's 135 envelopes.
    #[test]
    fn an_envelope_from_a_spawned_thread_names_its_thread_id() {
        let dir = temp_dir("envelope-spawned");
        let path = write_rollout(
            &dir,
            "rollout-parent.jsonl",
            &[
                meta_line(),
                spawn_line(3, "/root/design_prompt_review", CHILD_THREAD),
                envelope_line(9, "/root/design_prompt_review", "/root", "改完了"),
            ],
        );
        let delta = CodexAdapter
            .read_delta(&session_at(&path), &SourceCursor::default())
            .unwrap();
        let meta = metadata_of(&delta, "agent_message");
        assert_eq!(meta["counterpart_agent_path"], "/root/design_prompt_review");
        assert_eq!(meta["counterpart_source_id"], CHILD_THREAD);
        assert_eq!(meta["message_type"], "FINAL_ANSWER");
        assert_eq!(
            meta["counterpart_role"], "child",
            "a root rollout derives its own agent path from the threads it spawned"
        );
        let text = &delta
            .events
            .iter()
            .find(|e| e.kind == "agent_message")
            .unwrap()
            .text;
        assert!(text.as_deref().unwrap().contains("改完了"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A message from above, read in the child's own transcript: the author is
    /// the parent, whose path is this file's own path minus its last segment,
    /// and whose thread id is this file's `parent_thread_id` (33/135). The
    /// envelope also has to survive the internal-thread gate — it is the task
    /// the thread was handed, without which its timeline has no beginning.
    #[test]
    fn a_child_resolves_a_message_from_above_and_keeps_it() {
        let dir = temp_dir("envelope-from-parent");
        let parent = "019f135a-621c-76a1-a76c-7c71021847aa";
        let path = write_rollout(
            &dir,
            &format!("rollout-2026-09-20T21-01-47-{SESSION_ID}.jsonl"),
            &[
                meta_line_with(serde_json::json!({
                    "session_id": SESSION_ID, "id": SESSION_ID, "cwd": "/tmp/proj",
                    "thread_source": "subagent", "parent_thread_id": parent,
                    "source": { "subagent": { "thread_spawn": {
                        "parent_thread_id": parent, "agent_path": "/root/godot_prompt_review"
                    } } }
                })),
                message_line(2, "user", "The following is the Codex agent history…"),
                envelope_line(5, "/root", "/root/godot_prompt_review", "审一遍这份设计"),
                message_line(9, "assistant", "有阻断项，按严重度如下。"),
            ],
        );
        let delta = CodexAdapter
            .read_delta(&session_at(&path), &SourceCursor::default())
            .unwrap();
        let kinds: Vec<&str> = delta.events.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(
            kinds,
            vec!["agent_message", "assistant_message"],
            "the injected prompt is dropped, the envelope is not"
        );
        let meta = metadata_of(&delta, "agent_message");
        assert_eq!(meta["counterpart_source_id"], parent);
        assert_eq!(meta["counterpart_agent_path"], "/root");
        assert_eq!(meta["counterpart_role"], "parent");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A message from a sibling: the sibling's spawn was recorded by the shared
    /// ancestor, so this file cannot name its thread id. The path is still the
    /// truth and is kept; the id stays absent rather than guessed (2/135).
    #[test]
    fn an_envelope_from_a_sibling_keeps_the_path_and_invents_no_id() {
        let dir = temp_dir("envelope-sibling");
        let parent = "019f135a-621c-76a1-a76c-7c71021847aa";
        let path = write_rollout(
            &dir,
            &format!("rollout-2026-09-20T21-01-47-{SESSION_ID}.jsonl"),
            &[
                meta_line_with(serde_json::json!({
                    "session_id": SESSION_ID, "id": SESSION_ID, "cwd": "/tmp/proj",
                    "thread_source": "subagent", "parent_thread_id": parent,
                    "source": { "subagent": { "thread_spawn": {
                        "parent_thread_id": parent, "agent_path": "/root/binding_spec_audit"
                    } } }
                })),
                envelope_line(
                    5,
                    "/root/boss_runtime_rebuild",
                    "/root/binding_spec_audit",
                    "给你两条",
                ),
            ],
        );
        let delta = CodexAdapter
            .read_delta(&session_at(&path), &SourceCursor::default())
            .unwrap();
        let meta = metadata_of(&delta, "agent_message");
        assert_eq!(meta["counterpart_agent_path"], "/root/boss_runtime_rebuild");
        assert_eq!(
            meta["counterpart_role"], "sibling",
            "same parent, different leaf — a peer, not a parent or a child"
        );
        assert!(
            meta.get("counterpart_source_id").is_none(),
            "no thread id is derivable from this file, and a wrong one is worse than none"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- The session index as a title source (方案 §37.16) ----------------

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
            .discover_sessions_in(&[dir.clone()], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].native_title.as_deref(),
            Some("移除新建任务按钮加号")
        );
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
            .discover_sessions_in(&[dir.clone()], &|_| false)
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
}
