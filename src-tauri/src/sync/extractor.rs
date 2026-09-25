//! ContextExtractor implementations.
//!
//! - HeuristicExtractor: deterministic rules, zero cost, always available.
//! - CliExtractor: runs the user's already-authenticated agent CLI headlessly
//!   (codex exec / claude -p / pi -p) and parses a strict JSON mutation list.
//!   Any failure (missing CLI, timeout, invalid JSON) returns Err so the
//!   SyncEngine can fall back to the heuristic — sync never breaks because
//!   the model had a bad day.
//!
//! SourceReference integrity (Issue #2): the prompt numbers events #1..#N
//! but those numbers are *prompt-local*. A `PromptEventRef` map is built
//! while composing the prompt and is the ONLY way a short ref resolves to a
//! real event; unknown refs are dropped with a diagnostic instead of being
//! misinterpreted as event sequences.

use serde::Deserialize;

use crate::adapters::ExecOptions;
use crate::domain::{Agent, Session, SessionEvent};
use crate::error::{other, Result};
use crate::storage::Db;

use super::{ContextExtractor, ContextMutation, ExtractOutput};

/// Prompt-building reads (workstream lines, existing item lines),
/// snapshotted while the DB lock is held so extraction can run lock-free.
#[derive(Debug, Clone, Default)]
pub struct PromptInputs {
    pub ws_lines: Vec<String>,
    pub item_lines: Vec<String>,
}

/// Snapshot the workstream / item context an extraction prompt needs.
/// Snapshot the prompt material for the ONE Workstream this extraction routes
/// to (方案 §22). A Session has a single Owner, so there is no candidate set and
/// no cross-workstream item stitching.
pub fn collect_prompt_inputs(db: &Db, workstream_id: &str) -> Result<PromptInputs> {
    let mut ws_lines = Vec::new();
    if let Some(w) = db.get_workstream(workstream_id)? {
        ws_lines.push(format!(
            "- id={} | {} | goal: {}",
            w.id,
            w.title,
            if w.description.is_empty() {
                "(none)"
            } else {
                &w.description
            }
        ));
    }
    let mut item_lines = Vec::new();
    for (item, rev) in db.items_for_workstream(workstream_id, false)? {
        item_lines.push(format!(
            "- item_id={} | {} | {}",
            item.id, item.kind, rev.title
        ));
    }
    Ok(PromptInputs {
        ws_lines,
        item_lines,
    })
}

const DECISION_HINTS: [&str; 8] = [
    "决定",
    "确定采用",
    "就用",
    "decided",
    "decision:",
    "we'll use",
    "选择",
    "定为",
];
const CONSTRAINT_HINTS: [&str; 8] = [
    "不能",
    "禁止",
    "不允许",
    "must not",
    "don't change",
    "不要动",
    "保持兼容",
    "constraint",
];
const TODO_HINTS: [&str; 7] = [
    "todo",
    "待办",
    "接下来要",
    "下一步",
    "next step",
    "待完成",
    "需要先",
];
const QUESTION_HINTS: [&str; 5] = ["？", "?", "为什么", "是否", "how to"];

pub struct HeuristicExtractor;

impl ContextExtractor for HeuristicExtractor {
    fn name(&self) -> String {
        "heuristic".into()
    }

    fn extract(
        &self,
        _session: &Session,
        events: &[&SessionEvent],
        workstream_id: &str,
        _inputs: &PromptInputs,
    ) -> Result<ExtractOutput> {
        let mut out = Vec::new();
        if workstream_id.is_empty() {
            return Ok(ExtractOutput::default());
        }
        let primary = workstream_id.to_string();

        for e in events {
            let text = match &e.text {
                Some(t) => t,
                None => continue,
            };
            // Stable source reference: the event's app-owned identity, not a
            // positional number.
            let source_refs = vec![format!("session-event:{}", e.id)];
            // Authority = whose word the content is. Text extracted from a
            // user message is the USER's statement, whoever wrote the row.
            // `created_by` (the extractor) is recorded separately.
            let authority = if e.kind == "user_message" {
                "user_explicit"
            } else {
                "agent_statement"
            };

            if e.kind == "user_message" && text.len() > 20 && !looks_like_command(text) {
                if contains_any(
                    text,
                    &[
                        "目标",
                        "要做",
                        "实现",
                        "完成",
                        "build",
                        "implement",
                        "规划",
                        "设计",
                    ],
                ) {
                    out.push(ContextMutation::Add {
                        workstream_id: primary.clone(),
                        item_kind: "current_state".into(),
                        title: crate::adapters::truncate_text(text.trim(), 80),
                        content: text.trim().to_string(),
                        source_refs: source_refs.clone(),
                        authority: authority.into(),
                    });
                }
            }

            for (kind, hints) in [
                ("decision", &DECISION_HINTS[..]),
                ("constraint", &CONSTRAINT_HINTS[..]),
                ("todo", &TODO_HINTS[..]),
                ("open_question", &QUESTION_HINTS[..]),
            ] {
                if let Some(line) = find_hint_line(text, hints) {
                    let title = crate::adapters::truncate_text(line.trim(), 80);
                    if title.len() < 8 {
                        continue;
                    }
                    out.push(ContextMutation::Add {
                        workstream_id: primary.clone(),
                        item_kind: kind.into(),
                        title,
                        content: line.trim().to_string(),
                        source_refs: source_refs.clone(),
                        authority: authority.into(),
                    });
                }
            }
        }

        out.truncate(8);
        Ok(ExtractOutput::mutations(out))
    }
}

// ---------------------------------------------------------------------------
// CLI extractor
// ---------------------------------------------------------------------------

/// Runs intelligence through the user's agent CLI (headless mode).
pub struct CliExtractor {
    pub agent: Agent,
    pub opts: ExecOptions,
    pub timeout_secs: u64,
}

/// Assistant configuration, persisted in settings.
///
/// This is ONLY the agent choice: which CLI the built-in Assistant runs on is
/// a product decision, while model / provider / effort are owned by
/// Settings → Agents like every other consumer's runtime (§14). There is no
/// second model configuration and no cross-Agent default left to leak.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AssistantConfig {
    pub agent: String, // codex | claude_code | pi | none
}

impl Default for AssistantConfig {
    fn default() -> Self {
        Self {
            agent: "codex".into(),
        }
    }
}

impl AssistantConfig {
    pub fn from_settings(db: &Db) -> AssistantConfig {
        AssistantConfig {
            agent: db
                .get_setting("assistant.agent")
                .ok()
                .flatten()
                .unwrap_or_else(|| AssistantConfig::default().agent),
        }
    }

    pub fn save(&self, db: &Db) -> Result<()> {
        db.set_setting("assistant.agent", &self.agent)
    }
}

impl CliExtractor {
    pub fn from_settings(db: &Db) -> Option<CliExtractor> {
        Self::for_agent(db, &AssistantConfig::from_settings(db).agent)
    }

    /// Interactive variant: an invalid stored override is reported instead of
    /// quietly downgrading to retrieval-only, because the user *did* configure
    /// an Agent. `Ok(None)` means Assistant runs with no model on purpose.
    pub fn try_from_settings(db: &Db) -> Result<Option<CliExtractor>> {
        Self::try_for_agent(db, &AssistantConfig::from_settings(db).agent)
    }

    pub fn for_agent(db: &Db, agent: &str) -> Option<CliExtractor> {
        match Self::try_for_agent(db, agent) {
            Ok(cli) => cli,
            Err(e) => {
                // An invalid stored row degrades to heuristic extraction;
                // it never becomes a launch with parameters NoEnding was not
                // allowed to pass.
                eprintln!("[extractor] {e}");
                None
            }
        }
    }

    /// `Ok(None)` has exactly one meaning: Assistant is set to `none`, i.e. no
    /// model on purpose. Anything else that cannot be honoured is an `Err` —
    /// a stored Agent name NoEnding cannot resolve is corruption, not a
    /// request for retrieval-only.
    pub fn try_for_agent(db: &Db, agent: &str) -> Result<Option<CliExtractor>> {
        if agent == "none" {
            return Ok(None);
        }
        let agent =
            Agent::parse(agent).ok_or_else(|| other(format!("未知的 Assistant Agent: {agent}")))?;
        Ok(Some(CliExtractor::new(
            agent,
            crate::agent_runtime::runtime_exec_options(db, agent)?,
        )))
    }

    /// Explicit runtime intent — nothing is read from Settings. `None` fields
    /// mean the Agent's own defaults are used verbatim.
    pub fn new(agent: Agent, opts: ExecOptions) -> CliExtractor {
        CliExtractor {
            agent,
            opts,
            timeout_secs: 120,
        }
    }
}

impl ContextExtractor for CliExtractor {
    /// Names what was actually passed: `agent-default` records that NoEnding
    /// sent no runtime flag at all, so a SyncRun can never be misread as
    /// "NoEnding chose this model".
    fn name(&self) -> String {
        let agent = self.agent.as_str();
        if self.opts.is_default() {
            format!("cli:{agent}:agent-default")
        } else {
            format!("cli:{agent}:override:{}", self.opts.override_summary())
        }
    }

    fn extract(
        &self,
        session: &Session,
        events: &[&SessionEvent],
        workstream_id: &str,
        inputs: &PromptInputs,
    ) -> Result<ExtractOutput> {
        let install = crate::platform::exec_resolver::resolve(self.agent)?;
        let adapter = crate::adapters::adapter_for(self.agent);
        let (prompt, ref_map) = build_extraction_prompt(session, events, inputs)?;
        let cmd = adapter.build_exec_command(&install, &self.opts, &prompt)?;
        let out = crate::platform::exec_runner::run_headless(&cmd, self.timeout_secs)?;
        let text = crate::platform::exec_runner::clean_exec_stdout(&out.stdout);
        parse_mutations(&text, &ref_map, workstream_id, session)
    }
}

/// One entry of the prompt-local reference map: "#N" as shown to the model
/// → the real, stable event identity behind it.
#[derive(Debug, Clone)]
pub struct PromptEventRef {
    pub short_ref: String,
    pub event_id: String,
    pub sequence: i64,
    /// The event's kind ("user_message" | "assistant_message" | …): the raw
    /// material for deterministic authority derivation after parsing.
    pub kind: String,
}

/// Raw mutation shape we ask the model for. Short refs ("#1") map back to
/// real session source refs after parsing, so the model cannot invent them.
#[derive(Debug, Deserialize)]
struct RawMutation {
    op: String,
    #[serde(default)]
    workstream_id: String,
    #[serde(default)]
    item_kind: String,
    #[serde(default)]
    item_id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    refs: Vec<String>,
}

const ALLOWED_KINDS: [&str; 15] = [
    "goal",
    "current_state",
    "constraint",
    "decision",
    "open_question",
    "todo",
    "finding",
    "issue",
    "risk",
    "note",
    "reference",
    "artifact",
    "requirement",
    "decision_detail",
    "research_note",
];

fn build_extraction_prompt(
    session: &Session,
    events: &[&SessionEvent],
    inputs: &PromptInputs,
) -> Result<(String, Vec<PromptEventRef>)> {
    let ws_lines = &inputs.ws_lines;
    let item_lines = &inputs.item_lines;

    let mut ev_lines = Vec::new();
    let mut ref_map = Vec::new();
    for (i, e) in events.iter().enumerate() {
        let short_ref = format!("#{}", i + 1);
        ev_lines.push(format!(
            "{} [{}] {}",
            short_ref,
            if e.kind == "user_message" {
                "user"
            } else {
                "agent"
            },
            crate::adapters::truncate_text(e.text.as_deref().unwrap_or(""), 600)
        ));
        ref_map.push(PromptEventRef {
            short_ref,
            event_id: e.id.clone(),
            sequence: e.sequence,
            kind: e.kind.clone(),
        });
    }

    let prompt = format!(
        r##"你是 NoEnding 的上下文提取器。任务：从 Agent 会话的新增消息中提取少量高价值的长期上下文变更。这不是对话，禁止自由发挥，只输出 JSON。

所属 Workstream（只允许使用这个 id）：
{ws}

该 Workstream 已有的 active 条目（避免重复；若新信息实质更新了某条，输出 op=update）：
{items}

新增消息（ref 编号见行首）：
{events}

只输出一个 JSON 数组，每个元素形如：
{{"op":"add","workstream_id":"<所属id>","item_kind":"decision|constraint|todo|open_question|goal|current_state|note|finding|risk|issue|reference","title":"不超过60字","content":"原文或简述","refs":["#1"]}}
可选 op："update"（需 item_id，取自已有条目列表）、"resolve"（需 item_id，表示已完成）、"conflict"（与用户约束冲突时保留双方，需 item_id）。

规则：
1. 最多 8 条；没有有价值信息就输出 []；
2. workstream_id 必须是上面给出的所属 id；item_kind 必须来自上面的枚举；
3. refs 只能引用消息行首的 #编号；寒暄、工具输出、纯过程性内容一律忽略；
4. 不要输出 JSON 以外的任何文字（不要 markdown 代码块标记）。

Session: {agent} / {sid}"##,
        ws = ws_lines.join("\n"),
        items = if item_lines.is_empty() {
            "(none)".to_string()
        } else {
            item_lines.join("\n")
        },
        events = ev_lines.join("\n"),
        agent = session.agent.display_name(),
        sid = session.agent_session_id,
    );
    Ok((prompt, ref_map))
}

/// Extract the first JSON array from model output (models sometimes wrap
/// output in prose or code fences despite instructions).
pub fn extract_json_array(text: &str) -> Option<String> {
    let start = text.find('[')?;
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escape = false;
    for (i, b) in bytes.iter().enumerate().skip(start) {
        if escape {
            escape = false;
            continue;
        }
        match b {
            b'\\' if in_str => escape = true,
            b'"' => in_str = !in_str,
            b'[' if !in_str => depth += 1,
            b']' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    return Some(text[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// Resolve short refs through the prompt's reference map. The map is the
/// only bridge between "#N" and a real event: an unknown ref is dropped
/// with a diagnostic, never misread as an event sequence.
fn resolve_refs(
    refs: &[String],
    ref_map: &[PromptEventRef],
    diagnostics: &mut Vec<String>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in refs {
        let key = r.trim().trim_start_matches('#');
        let short = format!("#{}", key);
        if let Some(m) = ref_map.iter().find(|m| m.short_ref == short) {
            let reference = format!("session-event:{}", m.event_id);
            if !out.contains(&reference) {
                out.push(reference);
            }
        } else {
            diagnostics.push(format!("模型引用了 prompt 中不存在的 {}，已忽略", r.trim()));
        }
    }
    out
}

/// Deterministic authority derivation (Issue: authority is NEVER the
/// model's call). The LLM decides WHAT to extract; NoEnding decides whose
/// word it is, from the events the mutation actually cites:
/// - all cited events are user messages        → `user_explicit`
/// - all cited events are assistant messages   → `agent_statement`
/// - mixed citations (user participated)       → `user_explicit` (conservative:
///   the item is protected from silent agent modification)
/// - no resolvable citation                    → `agent_inferred`
pub fn derive_authority(source_refs: &[String], ref_map: &[PromptEventRef]) -> String {
    if source_refs.is_empty() {
        return "agent_inferred".into();
    }
    let kinds: Vec<&str> = source_refs
        .iter()
        .filter_map(|r| {
            let id = r.strip_prefix("session-event:")?;
            ref_map
                .iter()
                .find(|m| m.event_id == id)
                .map(|m| m.kind.as_str())
        })
        .collect();
    if kinds.is_empty() {
        return "agent_inferred".into();
    }
    if kinds.iter().all(|k| *k == "assistant_message") {
        "agent_statement".into()
    } else {
        // all user, or mixed → user participates in the claim
        "user_explicit".into()
    }
}

/// Parse + validate model output into real mutations. Invalid entries are
/// dropped individually (with diagnostics); the rest still merges.
pub fn parse_mutations(
    text: &str,
    ref_map: &[PromptEventRef],
    workstream_id: &str,
    _session: &Session,
) -> Result<ExtractOutput> {
    let arr = extract_json_array(text).ok_or_else(|| other("模型输出中未找到 JSON 数组"))?;
    let raw: Vec<RawMutation> =
        serde_json::from_str(&arr).map_err(|e| other(format!("JSON 解析失败: {}", e)))?;

    let mut out = Vec::new();
    let mut diagnostics: Vec<String> = Vec::new();
    for r in raw.into_iter().take(12) {
        let source_refs = resolve_refs(&r.refs, ref_map, &mut diagnostics);

        match r.op.as_str() {
            "add" => {
                if r.workstream_id != workstream_id
                    || !ALLOWED_KINDS.contains(&r.item_kind.as_str())
                    || r.title.trim().is_empty()
                {
                    diagnostics
                        .push("模型输出 add 条目缺少合法 workstream/kind/title，已忽略".into());
                    continue;
                }
                out.push(ContextMutation::Add {
                    workstream_id: r.workstream_id,
                    item_kind: r.item_kind,
                    title: crate::adapters::truncate_text(r.title.trim(), 80),
                    content: r.content.trim().to_string(),
                    authority: derive_authority(&source_refs, ref_map),
                    source_refs,
                });
            }
            "update" | "supersede" => {
                if r.item_id.is_empty() || r.title.trim().is_empty() {
                    diagnostics.push("模型输出 update/supersede 缺少 item_id/title，已忽略".into());
                    continue;
                }
                let content = r.content.trim().to_string();
                let authority = derive_authority(&source_refs, ref_map);
                out.push(if r.op == "update" {
                    ContextMutation::Update {
                        item_id: r.item_id,
                        title: r.title.trim().to_string(),
                        content,
                        source_refs,
                        authority,
                    }
                } else {
                    ContextMutation::Supersede {
                        item_id: r.item_id,
                        title: r.title.trim().to_string(),
                        content,
                        source_refs,
                        authority,
                    }
                });
            }
            "resolve" => {
                if r.item_id.is_empty() {
                    diagnostics.push("模型输出 resolve 缺少 item_id，已忽略".into());
                    continue;
                }
                out.push(ContextMutation::Resolve {
                    item_id: r.item_id,
                    source_refs,
                });
            }
            "create_workstream" => {
                if r.title.trim().is_empty() {
                    continue;
                }
                out.push(ContextMutation::CreateWorkstream {
                    title: r.title.trim().to_string(),
                    reason: r.content.trim().to_string(),
                });
            }
            "conflict" => {
                if r.workstream_id != workstream_id {
                    continue;
                }
                out.push(ContextMutation::Conflict {
                    workstream_id: r.workstream_id,
                    item_id: r.item_id,
                    title: r.title.trim().to_string(),
                    content: r.content.trim().to_string(),
                    source_refs,
                    reason: "模型判定与用户约束可能冲突".into(),
                });
            }
            _ => continue,
        }
    }
    out.truncate(8);
    Ok(ExtractOutput {
        mutations: out,
        diagnostics,
    })
}

fn contains_any(text: &str, hints: &[&str]) -> bool {
    let lower = text.to_lowercase();
    hints.iter().any(|h| lower.contains(h))
}

fn looks_like_command(text: &str) -> bool {
    text.trim_start().starts_with('/') || text.trim_start().starts_with("!")
}

/// Extract the first line that carries a hint, so titles are anchored to
/// real transcript evidence.
fn find_hint_line<'a>(text: &'a str, hints: &[&str]) -> Option<String> {
    let lower = text.to_lowercase();
    for h in hints {
        if let Some(pos) = lower.find(h) {
            let bytes = text.as_bytes();
            let mut start = pos.min(text.len());
            while start > 0 && bytes[start] != b'\n' {
                start -= 1;
            }
            if bytes.get(start) == Some(&b'\n') {
                start += 1;
            }
            let mut end = pos + h.len();
            while end < text.len() && bytes[end] != b'\n' {
                end += 1;
            }
            let line = &text[start..end];
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::SessionEvent;

    fn session() -> Session {
        Session {
            id: "s1".into(),
            agent: Agent::Codex,
            agent_session_id: "a1".into(),
            title: None,
            cwd: None,
            workspace_path_id: None,
            project_id: None,
            owner_workstream_id: None,
            raw_path: "/tmp/x".into(),
            parent_agent_session_id: None,
            started_at: None,
            last_activity_at: None,
            trashed_at: None,
        }
    }

    /// Events with non-1-starting, non-contiguous sequences — the map must
    /// translate "#1" to the FIRST prompt event (sequence 101), never to
    /// "sequence 1".
    fn ref_map() -> Vec<PromptEventRef> {
        vec![
            PromptEventRef {
                short_ref: "#1".into(),
                event_id: "e-aaa".into(),
                sequence: 101,
                kind: "user_message".into(),
            },
            PromptEventRef {
                short_ref: "#2".into(),
                event_id: "e-bbb".into(),
                sequence: 105,
                kind: "assistant_message".into(),
            },
        ]
    }

    fn parse(text: &str) -> ExtractOutput {
        parse_mutations(text, &ref_map(), "ws1", &session()).unwrap()
    }

    #[test]
    fn parses_model_output_with_fences_and_prose() {
        let text = "好的，以下是提取结果：\n```json\n[{\"op\":\"add\",\"workstream_id\":\"ws1\",\"item_kind\":\"decision\",\"title\":\"采用 FTS5\",\"content\":\"决定使用 FTS5\",\"refs\":[\"#2\"]},\n{\"op\":\"add\",\"workstream_id\":\"bad\",\"item_kind\":\"decision\",\"title\":\"x\",\"content\":\"y\",\"refs\":[]}]\n```";
        let out = parse(text);
        assert_eq!(out.mutations.len(), 1, "invalid workstream filtered out");
        match &out.mutations[0] {
            ContextMutation::Add {
                workstream_id,
                source_refs,
                authority,
                ..
            } => {
                assert_eq!(workstream_id, "ws1");
                assert_eq!(source_refs, &vec!["session-event:e-bbb".to_string()]);
                // #2 is an assistant message: agent's own statement.
                assert_eq!(authority, "agent_statement");
            }
            other => panic!("unexpected mutation: {:?}", other),
        }
    }

    #[test]
    fn user_cited_extraction_keeps_user_authority() {
        let text = r##"[{"op":"add","workstream_id":"ws1","item_kind":"constraint","title":"双平台一等公民","content":"Windows 和 macOS 都必须作为一等平台。","refs":["#1"]}]"##;
        let out = parse(text);
        match &out.mutations[0] {
            ContextMutation::Add { authority, .. } => {
                assert_eq!(authority, "user_explicit", "user words keep user authority");
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn mixed_citation_is_conservatively_user_owned() {
        let text = r##"[{"op":"add","workstream_id":"ws1","item_kind":"note","title":"t","content":"c","refs":["#1","#2"]}]"##;
        let out = parse(text);
        match &out.mutations[0] {
            ContextMutation::Add { authority, .. } => {
                assert_eq!(
                    authority, "user_explicit",
                    "mixed refs protect the user's voice"
                );
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn uncited_extraction_is_agent_inferred() {
        let text = r##"[{"op":"add","workstream_id":"ws1","item_kind":"note","title":"t","content":"c","refs":[]}]"##;
        let out = parse(text);
        match &out.mutations[0] {
            ContextMutation::Add { authority, .. } => assert_eq!(authority, "agent_inferred"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn short_ref_maps_to_real_event_not_prompt_position() {
        let text = r##"[{"op":"add","workstream_id":"ws1","item_kind":"note","title":"第一条","content":"c","refs":["#1"]}]"##;
        let out = parse(text);
        match &out.mutations[0] {
            ContextMutation::Add { source_refs, .. } => {
                // "#1" is the first PROMPT event: id e-aaa / sequence 101.
                assert_eq!(source_refs, &vec!["session-event:e-aaa".to_string()]);
            }
            other => panic!("unexpected mutation: {:?}", other),
        }
    }

    #[test]
    fn unknown_ref_is_dropped_with_diagnostic() {
        let text = r##"[{"op":"add","workstream_id":"ws1","item_kind":"note","title":"t","content":"c","refs":["#999"]},
                       {"op":"add","workstream_id":"ws1","item_kind":"note","title":"u","content":"c","refs":["#2","#999"]}]"##;
        let out = parse(text);
        assert_eq!(out.mutations.len(), 2);
        match &out.mutations[0] {
            ContextMutation::Add { source_refs, .. } => assert!(source_refs.is_empty()),
            other => panic!("unexpected: {:?}", other),
        }
        match &out.mutations[1] {
            ContextMutation::Add { source_refs, .. } => {
                assert_eq!(source_refs.len(), 1);
                assert_eq!(source_refs[0], "session-event:e-bbb");
            }
            other => panic!("unexpected: {:?}", other),
        }
        assert!(
            out.diagnostics.iter().any(|d| d.contains("#999")),
            "diagnostics mention the bad ref"
        );
    }

    #[test]
    fn multi_and_duplicate_refs_are_deduped() {
        let text = r##"[{"op":"add","workstream_id":"ws1","item_kind":"note","title":"t","content":"c","refs":["#1","#2","#1"]}]"##;
        let out = parse(text);
        match &out.mutations[0] {
            ContextMutation::Add { source_refs, .. } => {
                assert_eq!(source_refs.len(), 2, "duplicates removed, both events kept");
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn heuristic_uses_user_authority_for_user_messages() {
        let db_dir = std::env::temp_dir().join(format!("noending-ext-{}", uuid::Uuid::new_v4()));
        let db = Db::open(&db_dir.join("t.db")).unwrap();
        let s = session();
        let ev = SessionEvent {
            id: "e-1".into(),
            session_id: s.id.clone(),
            sequence: 42,
            source_event_id: None,
            source_generation: 0,
            source_position: "line:42".into(),
            ts: None,
            kind: "user_message".into(),
            text: Some("我们决定采用 SQLite，不再引入向量数据库，这个方案就这么定了。".into()),
            raw_ref: "x#line:42".into(),
            metadata: serde_json::json!({}),
        };
        let inputs = collect_prompt_inputs(&db, "ws1").unwrap();
        let out = HeuristicExtractor
            .extract(&s, &[&ev], "ws1", &inputs)
            .unwrap();
        assert!(!out.mutations.is_empty());
        for m in &out.mutations {
            match m {
                ContextMutation::Add {
                    authority,
                    source_refs,
                    ..
                } => {
                    assert_eq!(authority, "user_explicit", "user words keep user authority");
                    assert_eq!(source_refs[0], "session-event:e-1");
                }
                other => panic!("unexpected: {:?}", other),
            }
        }
    }
}
