//! Explicit Context update model call.
//!
//! ONE call produces both the affected Session Contexts and the Workstream
//! mutations; the backend derives authority, revisions, generations and
//! frontiers from the cited sources, never from the model's word.
//!
//! Strictness is the point: the response must be a single JSON object whose keys
//! and fields are exactly what was asked for. Anything else — prose, markdown
//! fences, an unknown field, an unknown id, an out-of-snapshot reference — fails
//! the whole update. No heuristic fallback, no "ignore the bad entry and keep
//! the rest".

use std::collections::{HashMap, HashSet};

use serde::Deserialize;

use crate::adapters::ExecOptions;
use crate::domain::{Agent, SessionContextFields, SessionMessage, SessionMessageRole};
use crate::error::{other, Result};
use crate::storage::Db;

use super::{ContextMutation, ContextUpdateOutput, SessionContextDraft};

/// Fixed single-request limits. Defined once here; the frontend never
/// recomputes them.
pub const INPUT_LIMIT_BYTES: usize = 48 * 1024;
pub const OUTPUT_LIMIT_BYTES: usize = 64 * 1024;
pub const WORKSTREAM_RESERVE_BYTES: usize = 8 * 1024;
pub const SESSION_CONTEXT_RESERVE_BYTES: usize = 4 * 1024;

/// Assistant configuration, persisted in settings.
///
/// ONLY the agent choice: which CLI the explicit Context update runs on. Model /
/// provider / effort are owned by Settings → Agents like every other runtime.
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

/// Runs the explicit Context update through the user's agent CLI (headless).
pub struct CliExtractor {
    pub agent: Agent,
    pub opts: ExecOptions,
    pub timeout_secs: u64,
}

impl CliExtractor {
    /// Strict lookup for the interactive path: `Ok(None)` means the user set
    /// Assistant to `none`; anything else that cannot be honoured is an `Err`.
    /// A missing or invalid CLI is surfaced, never degraded to a heuristic.
    pub fn try_from_settings(db: &Db) -> Result<Option<CliExtractor>> {
        Self::try_for_agent(db, &AssistantConfig::from_settings(db).agent)
    }

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

    pub fn new(agent: Agent, opts: ExecOptions) -> CliExtractor {
        CliExtractor {
            agent,
            opts,
            timeout_secs: 120,
        }
    }

    /// Runtime label recorded on written revisions.
    pub fn name(&self) -> String {
        let agent = self.agent.as_str();
        if self.opts.is_default() {
            format!("cli:{agent}:agent-default")
        } else {
            format!("cli:{agent}:override:{}", self.opts.override_summary())
        }
    }

    /// Run ONE headless model call and return cleaned stdout. Any failure
    /// (CLI missing, timeout, non-zero exit) is returned as Err — the caller
    /// must NOT fall back to a heuristic.
    pub fn run(&self, prompt: &str) -> Result<String> {
        let install = crate::platform::exec_resolver::resolve(self.agent)?;
        let adapter = crate::adapters::adapter_for(self.agent);
        let cmd = adapter.build_exec_command(&install, &self.opts, prompt)?;
        let out = crate::platform::exec_runner::run_headless(&cmd, self.timeout_secs)?;
        Ok(crate::platform::exec_runner::clean_exec_stdout(&out.stdout))
    }
}

// Prompt material

/// One entry of the prompt-local message reference map: "#N" as shown to the
/// model → the real, stable message identity behind it.
#[derive(Debug, Clone)]
pub struct PromptMessageRef {
    pub short_ref: String,
    pub message_id: String,
    pub role: SessionMessageRole,
}

/// Everything the model may cite, and the ids it is allowed to target. Built
/// while the snapshot was taken; parsing resolves through this, never through
/// prompt positions.
#[derive(Debug, Clone, Default)]
pub struct PromptRefs {
    pub messages: Vec<PromptMessageRef>,
    /// session_id → the revision number a `@<session_id>` citation resolves
    /// to (the revision this update will persist for a target, or the current
    /// revision for a read-only input).
    pub session_cite_revision: HashMap<String, i64>,
}

/// A Session fed into the prompt: its (optional) existing Context and the raw
/// messages that need processing. `is_target` sessions must be echoed in the
/// response; read-only sessions may only be cited.
#[derive(Debug, Clone)]
pub struct PromptSession {
    pub session_id: String,
    pub is_target: bool,
    /// Existing four-field Context, when one exists.
    pub existing: Option<SessionContextFields>,
    /// Current Session Context revision (0 when none exists).
    pub existing_revision: Option<i64>,
    /// For a target: the raw messages to (re)summarize; empty for a read-only
    /// input.
    pub messages: Vec<SessionMessage>,
}

/// Everything one explicit update needs from the model call.
#[derive(Debug, Clone)]
pub struct PromptInput {
    pub workstream_title: String,
    pub workstream_description: String,
    /// Lines describing the Workstream's current ContextItems (id | kind | title).
    pub item_lines: Vec<String>,
    pub sessions: Vec<PromptSession>,
    /// Messages for the Workstream call, in stable order.
    pub messages: Vec<SessionMessage>,
}

/// Assemble the prompt and the reference map. `refs` is filled so parsing can
/// resolve every short ref back to a real, stable identity.
pub fn build_update_prompt(input: &PromptInput) -> (String, PromptRefs) {
    let mut msg_lines = Vec::new();
    let mut refs = PromptRefs::default();
    for (i, m) in input.messages.iter().enumerate() {
        let short_ref = format!("#{}", i + 1);
        msg_lines.push(format!(
            "{} [{}] {}",
            short_ref,
            if m.role == SessionMessageRole::User {
                "user"
            } else {
                "agent"
            },
            crate::adapters::truncate_text(&m.content, 1200)
        ));
        refs.messages.push(PromptMessageRef {
            short_ref,
            message_id: m.id.clone(),
            role: m.role,
        });
    }

    let mut session_lines = Vec::new();
    for s in &input.sessions {
        let role = if s.is_target {
            "必须输出"
        } else {
            "只读引用"
        };
        let existing = match &s.existing {
            Some(f) => format!(
                "summary_current_state: {}\ndecisions: {}\nopen_questions: {}\nnext_steps: {}",
                f.summary_current_state,
                f.decisions.join(" | "),
                f.open_questions.join(" | "),
                f.next_steps.join(" | "),
            ),
            None => "(无，需首次生成)".to_string(),
        };
        session_lines.push(format!(
            "- session_id={} [{role}]\n  现有 Context: {}\n  本次消息数: {}",
            s.session_id,
            existing,
            s.messages.len()
        ));
        // citation target: current revision for read-only, next for target
        let rev = s
            .existing_revision
            .map(|r| if s.is_target { r + 1 } else { r })
            .unwrap_or(if s.is_target { 1 } else { 0 });
        if s.existing.is_some() || s.is_target {
            refs.session_cite_revision.insert(s.session_id.clone(), rev);
        }
    }

    let prompt = format!(
        r##"你是 NoEnding 的上下文更新器。只输出一个 JSON 对象，不要任何解释文字或 markdown 代码块。

Workstream：
- 标题：{ws_title}
- 描述：{ws_desc}

该 Workstream 现有的 active 条目（避免重复；若要实质更新某条，输出 op=update 并给出其 item_id）：
{items}

相关 Session（session_id 必须原样使用；标记「必须输出」的 Session 必须在 session_contexts 中给出完整四字段）：
{sessions}

需要处理的原始消息（行首 #编号 是引用编号）：
{messages}

输出 JSON 对象，且只能包含两个键：
{{
  "session_contexts": [
    {{"session_id":"<必须输出的 session_id>","summary_current_state":"...","decisions":["..."],"open_questions":["..."],"next_steps":["..."]}}
  ],
  "workstream_mutations": [
    {{"op":"add","item_kind":"decision|constraint|todo|open_question|goal|current_state|note|finding|risk|issue|reference","title":"不超过60字","content":"原文或简述","refs":["#1"]}},
    {{"op":"update","item_id":"<已有条目 id>","title":"...","content":"...","refs":["#2"]}},
    {{"op":"supersede","item_id":"<已有条目 id>","title":"...","content":"...","refs":["#2"]}},
    {{"op":"resolve","item_id":"<已有条目 id>","refs":["#3"]}}
  ]
}}

规则：
1. session_contexts 必须覆盖所有「必须输出」的 Session，且不得包含其他 Session；不得重复。
2. 四个字段必须齐全，空列表写 []。decisions/open_questions/next_steps 是字符串数组。
3. workstream_mutations 只允许 add/update/supersede/resolve；add 必须给 item_kind 与 title；update/supersede/resolve 必须给 item_id。
4. refs 只能引用上面的 #编号，或 @session_id（引用某个 Session 的现有摘要）；不得出现其他取值。
5. 没有可提出的变更时，workstream_mutations 写 []。
6. 不要输出 authority、revision、generation 或任何处理位置。除这一个 JSON 对象外不要输出任何内容。"##,
        ws_title = input.workstream_title,
        ws_desc = if input.workstream_description.is_empty() {
            "(无)"
        } else {
            &input.workstream_description
        },
        items = if input.item_lines.is_empty() {
            "(none)".to_string()
        } else {
            input.item_lines.join("\n")
        },
        sessions = session_lines.join("\n"),
        messages = if msg_lines.is_empty() {
            "(none)".to_string()
        } else {
            msg_lines.join("\n")
        },
    );
    (prompt, refs)
}

// Strict parsing

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOutput {
    session_contexts: Vec<RawSessionContext>,
    workstream_mutations: Vec<RawMutation>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSessionContext {
    session_id: String,
    summary_current_state: String,
    decisions: Vec<String>,
    open_questions: Vec<String>,
    next_steps: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMutation {
    op: String,
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

pub const ALLOWED_KINDS: [&str; 15] = [
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

/// Extract the first complete JSON object from model output. Models sometimes
/// wrap output in prose or code fences despite instructions; we still insist
/// the payload itself be a single object.
fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
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
            b'{' if !in_str => depth += 1,
            b'}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Resolve one source ref through the reference map. Only a known "#N" or
/// "@session_id" is legal; anything else fails.
fn resolve_ref(r: &str, refs: &PromptRefs) -> Result<String> {
    let r = r.trim();
    if let Some(key) = r.strip_prefix('#') {
        let short = format!("#{key}");
        let Some(m) = refs.messages.iter().find(|m| m.short_ref == short) else {
            return Err(other(format!("模型引用了不存在的消息引用 {r}")));
        };
        return Ok(format!("session-message:{}", m.message_id));
    }
    if let Some(sid) = r.strip_prefix('@') {
        let Some(rev) = refs.session_cite_revision.get(sid) else {
            return Err(other(format!("模型引用了不存在的 Session 引用 {r}")));
        };
        return Ok(format!("session-context:{sid}:{rev}"));
    }
    Err(other(format!("模型引用了非法来源 {r}")))
}

/// Deterministic authority derivation — authority is NEVER the model's call.
/// The model decides WHAT to extract; NoEnding decides whose word it is, from
/// the sources the mutation actually cites:
/// - any cited user message                    → `user_explicit`
/// - all-agent citations (messages / contexts) → `agent_statement`
/// - no resolvable citation                    → `agent_inferred`
pub fn derive_authority(source_refs: &[String], refs: &PromptRefs) -> String {
    if source_refs.is_empty() {
        return "agent_inferred".into();
    }
    let mut saw_user = false;
    let mut saw_agent = false;
    for r in source_refs {
        if let Some(id) = r.strip_prefix("session-message:") {
            match refs
                .messages
                .iter()
                .find(|m| m.message_id == id)
                .map(|m| m.role)
            {
                Some(SessionMessageRole::User) => saw_user = true,
                Some(SessionMessageRole::Assistant) => saw_agent = true,
                None => {}
            }
        } else if r.starts_with("session-context:") {
            saw_agent = true;
        }
    }
    if saw_user {
        "user_explicit".into()
    } else if saw_agent {
        "agent_statement".into()
    } else {
        "agent_inferred".into()
    }
}

fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse + STRICTLY validate model output. Any violation fails the whole
/// update; nothing partial is ever returned.
///
/// `expected_targets` is the exact set of session ids that must appear in
/// `session_contexts`. `allowed_item_ids` is the set of Workstream item ids
/// the model may name in update/supersede/resolve.
pub fn parse_update_output(
    text: &str,
    refs: &PromptRefs,
    owner_workstream_id: &str,
    expected_targets: &[String],
    allowed_item_ids: &HashSet<String>,
) -> Result<ContextUpdateOutput> {
    if text.len() > OUTPUT_LIMIT_BYTES {
        return Err(other("模型输出超出大小上限"));
    }
    let obj = extract_json_object(text).ok_or_else(|| other("模型输出中未找到 JSON 对象"))?;
    let raw: RawOutput =
        serde_json::from_str(obj).map_err(|e| other(format!("模型输出 JSON 解析失败: {e}")))?;

    // session_contexts: exactly the target set, no duplicates.
    let expected: HashSet<&str> = expected_targets.iter().map(|s| s.as_str()).collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut session_contexts = Vec::new();
    for sc in raw.session_contexts {
        if !expected.contains(sc.session_id.as_str()) {
            return Err(other(format!(
                "模型输出了非目标 Session: {}",
                sc.session_id
            )));
        }
        if !seen.insert(sc.session_id.clone()) {
            return Err(other(format!("模型重复输出 Session: {}", sc.session_id)));
        }
        session_contexts.push(SessionContextDraft {
            session_id: sc.session_id,
            fields: SessionContextFields {
                summary_current_state: normalize_ws(&sc.summary_current_state),
                decisions: normalize_list(sc.decisions),
                open_questions: normalize_list(sc.open_questions),
                next_steps: normalize_list(sc.next_steps),
            },
        });
    }
    for t in expected_targets {
        if !seen.contains(t) {
            return Err(other(format!("模型缺少目标 Session: {t}")));
        }
    }

    // workstream_mutations: allowed ops only, valid ids/refs.
    let mut workstream_mutations = Vec::new();
    for m in raw.workstream_mutations {
        let source_refs = m
            .refs
            .iter()
            .map(|r| resolve_ref(r, refs))
            .collect::<Result<Vec<String>>>()?;
        // dedup while preserving order
        let mut deduped: Vec<String> = Vec::new();
        for r in source_refs {
            if !deduped.contains(&r) {
                deduped.push(r);
            }
        }
        let title = normalize_ws(&m.title);
        let content = m.content.trim().to_string();
        match m.op.as_str() {
            "add" => {
                if !ALLOWED_KINDS.contains(&m.item_kind.as_str()) || title.is_empty() {
                    return Err(other("add mutation 缺少合法 item_kind 或 title"));
                }
                workstream_mutations.push(ContextMutation::Add {
                    workstream_id: owner_workstream_id.to_string(),
                    item_kind: m.item_kind,
                    title,
                    content,
                    authority: derive_authority(&deduped, refs),
                    source_refs: deduped,
                });
            }
            "update" | "supersede" => {
                if m.item_id.is_empty() || title.is_empty() {
                    return Err(other("update/supersede mutation 缺少 item_id 或 title"));
                }
                if !allowed_item_ids.contains(&m.item_id) {
                    return Err(other(format!("mutation 指向未知条目 {}", m.item_id)));
                }
                let authority = derive_authority(&deduped, refs);
                workstream_mutations.push(if m.op == "update" {
                    ContextMutation::Update {
                        item_id: m.item_id,
                        title,
                        content,
                        source_refs: deduped,
                        authority,
                    }
                } else {
                    ContextMutation::Supersede {
                        item_id: m.item_id,
                        title,
                        content,
                        source_refs: deduped,
                        authority,
                    }
                });
            }
            "resolve" => {
                if m.item_id.is_empty() {
                    return Err(other("resolve mutation 缺少 item_id"));
                }
                if !allowed_item_ids.contains(&m.item_id) {
                    return Err(other(format!("mutation 指向未知条目 {}", m.item_id)));
                }
                workstream_mutations.push(ContextMutation::Resolve {
                    item_id: m.item_id,
                    source_refs: deduped,
                });
            }
            other_op => return Err(other(format!("未知 mutation op: {other_op}"))),
        }
    }

    Ok(ContextUpdateOutput {
        session_contexts,
        workstream_mutations,
    })
}

fn normalize_list(items: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for it in items {
        let n = normalize_ws(&it);
        if !n.is_empty() && !out.contains(&n) {
            out.push(n);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs() -> PromptRefs {
        PromptRefs {
            messages: vec![
                PromptMessageRef {
                    short_ref: "#1".into(),
                    message_id: "m-aaa".into(),
                    role: SessionMessageRole::User,
                },
                PromptMessageRef {
                    short_ref: "#2".into(),
                    message_id: "m-bbb".into(),
                    role: SessionMessageRole::Assistant,
                },
            ],
            session_cite_revision: HashMap::from([("s1".to_string(), 3)]),
        }
    }

    fn allowed() -> HashSet<String> {
        HashSet::from(["item-1".to_string()])
    }

    #[test]
    fn parses_a_well_formed_object() {
        let text = r##"{"session_contexts":[{"session_id":"s1","summary_current_state":"推进重构","decisions":["采用 SQLite"],"open_questions":[],"next_steps":["补测试"]}],"workstream_mutations":[{"op":"add","item_kind":"decision","title":"采用 SQLite","content":"决定用 SQLite","refs":["#1"]},{"op":"update","item_id":"item-1","title":"x","content":"y","refs":["#2"]}]}"##;
        let out =
            parse_update_output(text, &refs(), "ws1", &["s1".to_string()], &allowed()).unwrap();
        assert_eq!(out.session_contexts.len(), 1);
        assert_eq!(
            out.session_contexts[0].fields.decisions,
            vec!["采用 SQLite"]
        );
        assert_eq!(out.workstream_mutations.len(), 2);
        match &out.workstream_mutations[0] {
            ContextMutation::Add {
                workstream_id,
                authority,
                source_refs,
                ..
            } => {
                assert_eq!(workstream_id, "ws1");
                assert_eq!(authority, "user_explicit");
                assert_eq!(source_refs, &vec!["session-message:m-aaa".to_string()]);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn unknown_field_is_rejected() {
        let text = r#"{"session_contexts":[],"workstream_mutations":[],"extra":1}"#;
        assert!(parse_update_output(text, &refs(), "ws1", &[], &allowed()).is_err());
    }

    #[test]
    fn missing_target_session_is_rejected() {
        let text = r#"{"session_contexts":[],"workstream_mutations":[]}"#;
        assert!(
            parse_update_output(text, &refs(), "ws1", &["s1".to_string()], &allowed()).is_err()
        );
    }

    #[test]
    fn non_target_session_is_rejected() {
        let text = r#"{"session_contexts":[{"session_id":"other","summary_current_state":"","decisions":[],"open_questions":[],"next_steps":[]}],"workstream_mutations":[]}"#;
        assert!(
            parse_update_output(text, &refs(), "ws1", &["s1".to_string()], &allowed()).is_err()
        );
    }

    #[test]
    fn unknown_item_id_is_rejected() {
        let text = r##"{"session_contexts":[],"workstream_mutations":[{"op":"resolve","item_id":"nope","refs":["#1"]}]}"##;
        assert!(parse_update_output(text, &refs(), "ws1", &[], &allowed()).is_err());
    }

    #[test]
    fn unknown_ref_is_rejected() {
        let text = r##"{"session_contexts":[],"workstream_mutations":[{"op":"add","item_kind":"note","title":"t","content":"c","refs":["#99"]}]}"##;
        assert!(parse_update_output(text, &refs(), "ws1", &[], &allowed()).is_err());
    }

    #[test]
    fn session_context_citation_resolves_to_revision() {
        let text = r#"{"session_contexts":[],"workstream_mutations":[{"op":"add","item_kind":"note","title":"t","content":"c","refs":["@s1"]}]}"#;
        let out = parse_update_output(text, &refs(), "ws1", &[], &allowed()).unwrap();
        match &out.workstream_mutations[0] {
            ContextMutation::Add {
                source_refs,
                authority,
                ..
            } => {
                assert_eq!(source_refs, &vec!["session-context:s1:3".to_string()]);
                assert_eq!(authority, "agent_statement");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn prose_wrapped_object_still_parses() {
        let text = "好的：\n```json\n{\"session_contexts\":[],\"workstream_mutations\":[]}\n```";
        assert!(parse_update_output(text, &refs(), "ws1", &[], &allowed()).is_ok());
    }
}
