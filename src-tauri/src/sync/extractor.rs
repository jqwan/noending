//! ContextExtractor implementations.
//!
//! - HeuristicExtractor: deterministic rules, zero cost, always available.
//! - CliExtractor: runs the user's already-authenticated agent CLI headlessly
//!   (codex exec / claude -p / pi -p) and parses a strict JSON mutation list.
//!   Any failure (missing CLI, timeout, invalid JSON) returns Err so the
//!   SyncEngine can fall back to the heuristic — sync never breaks because
//!   the model had a bad day.

use serde::Deserialize;

use crate::adapters::ExecOptions;
use crate::domain::{Agent, Session, SessionEvent};
use crate::error::{other, Result};
use crate::storage::Db;

use super::{ContextExtractor, ContextMutation};

const DECISION_HINTS: [&str; 8] = ["决定", "确定采用", "就用", "decided", "decision:", "we'll use", "选择", "定为"];
const CONSTRAINT_HINTS: [&str; 8] = ["不能", "禁止", "不允许", "must not", "don't change", "不要动", "保持兼容", "constraint"];
const TODO_HINTS: [&str; 7] = ["todo", "待办", "接下来要", "下一步", "next step", "待完成", "需要先"];
const QUESTION_HINTS: [&str; 5] = ["？", "?", "为什么", "是否", "how to"];

pub struct HeuristicExtractor;

impl ContextExtractor for HeuristicExtractor {
    fn name(&self) -> String {
        "heuristic".into()
    }

    fn extract(
        &self,
        _db: &Db,
        session: &Session,
        events: &[&SessionEvent],
        candidate_workstream_ids: &[String],
    ) -> Result<Vec<ContextMutation>> {
        let mut out = Vec::new();
        let primary = candidate_workstream_ids.first().cloned().unwrap_or_default();
        if primary.is_empty() {
            return Ok(out);
        }

        for e in events {
            let text = match &e.text {
                Some(t) => t,
                None => continue,
            };
            let source_ref = format!("session:{}#{}", session.id, e.sequence);
            let authority = if e.kind == "user_message" {
                "agent_statement"
            } else {
                "agent_inferred"
            };

            if e.kind == "user_message" && text.len() > 20 && !looks_like_command(text) {
                if contains_any(text, &["目标", "要做", "实现", "完成", "build", "implement", "规划", "设计"]) {
                    out.push(ContextMutation::Add {
                        workstream_id: primary.clone(),
                        item_kind: "current_state".into(),
                        title: crate::adapters::truncate_text(text.trim(), 80),
                        content: text.trim().to_string(),
                        source_ref: source_ref.clone(),
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
                        source_ref: source_ref.clone(),
                        authority: authority.into(),
                    });
                }
            }
        }

        out.truncate(8);
        Ok(out)
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

/// Assistant runtime configuration, persisted in settings.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AssistantConfig {
    pub agent: String,    // codex | claude_code | pi | none
    pub model: String,
    pub provider: String, // pi only (openai-codex | lmstudio | ...)
    pub effort: String,   // codex reasoning effort / pi thinking level
}

impl Default for AssistantConfig {
    fn default() -> Self {
        // user-facing default: codex + gpt-5.6-luna @ low effort
        Self {
            agent: "codex".into(),
            model: "gpt-5.6-luna".into(),
            provider: "openai-codex".into(),
            effort: "low".into(),
        }
    }
}

impl AssistantConfig {
    pub fn from_settings(db: &Db) -> AssistantConfig {
        let mut cfg = Self::default();
        if let Ok(Some(v)) = db.get_setting("assistant.agent") {
            cfg.agent = v;
        }
        if let Ok(Some(v)) = db.get_setting("assistant.model") {
            cfg.model = v;
        }
        if let Ok(Some(v)) = db.get_setting("assistant.provider") {
            cfg.provider = v;
        }
        if let Ok(Some(v)) = db.get_setting("assistant.effort") {
            cfg.effort = v;
        }
        cfg
    }

    pub fn save(&self, db: &Db) -> Result<()> {
        db.set_setting("assistant.agent", &self.agent)?;
        db.set_setting("assistant.model", &self.model)?;
        db.set_setting("assistant.provider", &self.provider)?;
        db.set_setting("assistant.effort", &self.effort)?;
        Ok(())
    }

    pub fn exec_options(&self) -> ExecOptions {
        ExecOptions {
            model: if self.model.is_empty() { None } else { Some(self.model.clone()) },
            provider: if self.provider.is_empty() { None } else { Some(self.provider.clone()) },
            effort: if self.effort.is_empty() { None } else { Some(self.effort.clone()) },
        }
    }
}

impl CliExtractor {
    pub fn from_settings(db: &Db) -> Option<CliExtractor> {
        Self::from_config(&AssistantConfig::from_settings(db))
    }

    pub fn from_config(cfg: &AssistantConfig) -> Option<CliExtractor> {
        if cfg.agent == "none" {
            return None;
        }
        let agent = Agent::parse(&cfg.agent)?;
        Some(CliExtractor {
            agent,
            opts: cfg.exec_options(),
            timeout_secs: 120,
        })
    }
}

impl ContextExtractor for CliExtractor {
    fn name(&self) -> String {
        format!(
            "cli:{}:{}",
            self.agent.as_str(),
            self.opts.model.as_deref().unwrap_or("default")
        )
    }

    fn extract(
        &self,
        db: &Db,
        session: &Session,
        events: &[&SessionEvent],
        candidate_workstream_ids: &[String],
    ) -> Result<Vec<ContextMutation>> {
        let install = crate::platform::exec_resolver::resolve(self.agent)?;
        let adapter = crate::adapters::adapter_for(self.agent);
        let prompt = build_extraction_prompt(db, session, events, candidate_workstream_ids)?;
        let cmd = adapter.build_exec_command(&install, &self.opts, &prompt)?;
        let out = crate::platform::exec_runner::run_headless(&cmd, self.timeout_secs)?;
        let text = crate::platform::exec_runner::clean_exec_stdout(&out.stdout);
        parse_mutations(&text, candidate_workstream_ids, session)
    }
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
    "goal", "current_state", "constraint", "decision", "open_question",
    "todo", "finding", "issue", "risk", "note", "reference", "artifact",
    "requirement", "decision_detail", "research_note",
];

fn build_extraction_prompt(
    db: &Db,
    session: &Session,
    events: &[&SessionEvent],
    candidates: &[String],
) -> Result<String> {
    let mut ws_lines = Vec::new();
    for id in candidates {
        if let Some(w) = db.get_workstream(id)? {
            ws_lines.push(format!(
                "- id={} | {} | goal: {}",
                w.id,
                w.title,
                if w.description.is_empty() { "(none)" } else { &w.description }
            ));
        }
    }
    let mut item_lines = Vec::new();
    for id in candidates {
        for (item, rev) in db.items_for_workstream(id, false)? {
            item_lines.push(format!("- item_id={} | {} | {}", item.id, item.kind, rev.title));
        }
    }

    let mut ev_lines = Vec::new();
    for (i, e) in events.iter().enumerate() {
        ev_lines.push(format!(
            "#{} [{}] {}",
            i + 1,
            if e.kind == "user_message" { "user" } else { "agent" },
            crate::adapters::truncate_text(e.text.as_deref().unwrap_or(""), 600)
        ));
    }

    Ok(format!(
        r##"你是 NoEnding 的上下文提取器。任务：从 Agent 会话的新增消息中提取少量高价值的长期上下文变更。这不是对话，禁止自由发挥，只输出 JSON。

候选 Workstream（只允许使用这些 id）：
{ws}

这些 Workstream 已有的 active 条目（避免重复；若新信息实质更新了某条，输出 op=update）：
{items}

新增消息（ref 编号见行首）：
{events}

只输出一个 JSON 数组，每个元素形如：
{{"op":"add","workstream_id":"<候选id>","item_kind":"decision|constraint|todo|open_question|goal|current_state|note|finding|risk|issue|reference","title":"不超过60字","content":"原文或简述","refs":["#1"]}}
可选 op："update"（需 item_id，取自已有条目列表）、"resolve"（需 item_id，表示已完成）、"conflict"（与用户约束冲突时保留双方，需 item_id）。

规则：
1. 最多 8 条；没有有价值信息就输出 []；
2. workstream_id 必须来自候选列表；item_kind 必须来自上面的枚举；
3. refs 只能引用消息行首的 #编号；寒暄、工具输出、纯过程性内容一律忽略；
4. 不要输出 JSON 以外的任何文字（不要 markdown 代码块标记）。

Session: {agent} / {sid}"##,
        ws = ws_lines.join("\n"),
        items = if item_lines.is_empty() { "(none)".to_string() } else { item_lines.join("\n") },
        events = ev_lines.join("\n"),
        agent = session.agent.display_name(),
        sid = session.agent_session_id,
    ))
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

/// Parse + validate model output into real mutations. Invalid entries are
/// dropped individually; the rest still merges.
pub fn parse_mutations(
    text: &str,
    candidates: &[String],
    session: &Session,
) -> Result<Vec<ContextMutation>> {
    let arr = extract_json_array(text).ok_or_else(|| other("模型输出中未找到 JSON 数组"))?;
    let raw: Vec<RawMutation> =
        serde_json::from_str(&arr).map_err(|e| other(format!("JSON 解析失败: {}", e)))?;

    let mut out = Vec::new();
    for r in raw.into_iter().take(12) {
        let source_ref = r
            .refs
            .first()
            .and_then(|s| parse_short_ref(s))
            .map(|seq| format!("session:{}#{}", session.id, seq))
            .unwrap_or_else(|| format!("session:{}#llm", session.id));

        match r.op.as_str() {
            "add" => {
                if !candidates.contains(&r.workstream_id)
                    || !ALLOWED_KINDS.contains(&r.item_kind.as_str())
                    || r.title.trim().is_empty()
                {
                    continue;
                }
                out.push(ContextMutation::Add {
                    workstream_id: r.workstream_id,
                    item_kind: r.item_kind,
                    title: crate::adapters::truncate_text(r.title.trim(), 80),
                    content: r.content.trim().to_string(),
                    source_ref,
                    authority: "agent_inferred".into(),
                });
            }
            "update" | "supersede" => {
                if r.item_id.is_empty() || r.title.trim().is_empty() {
                    continue;
                }
                let content = r.content.trim().to_string();
                out.push(if r.op == "update" {
                    ContextMutation::Update {
                        item_id: r.item_id,
                        title: r.title.trim().to_string(),
                        content,
                        source_ref,
                        authority: "agent_inferred".into(),
                    }
                } else {
                    ContextMutation::Supersede {
                        item_id: r.item_id,
                        title: r.title.trim().to_string(),
                        content,
                        source_ref,
                        authority: "agent_inferred".into(),
                    }
                });
            }
            "resolve" => {
                if r.item_id.is_empty() {
                    continue;
                }
                out.push(ContextMutation::Resolve {
                    item_id: r.item_id,
                    source_ref,
                });
            }
            "conflict" => {
                if !candidates.contains(&r.workstream_id) {
                    continue;
                }
                out.push(ContextMutation::Conflict {
                    workstream_id: r.workstream_id,
                    item_id: r.item_id,
                    title: r.title.trim().to_string(),
                    content: r.content.trim().to_string(),
                    source_ref,
                    reason: "模型判定与用户约束可能冲突".into(),
                });
            }
            _ => continue,
        }
    }
    out.truncate(8);
    Ok(out)
}

fn parse_short_ref(s: &str) -> Option<i64> {
    s.trim().trim_start_matches('#').parse().ok()
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

    #[test]
    fn parses_model_output_with_fences_and_prose() {
        let text = "好的，以下是提取结果：\n```json\n[{\"op\":\"add\",\"workstream_id\":\"ws1\",\"item_kind\":\"decision\",\"title\":\"采用 FTS5\",\"content\":\"决定使用 FTS5\",\"refs\":[\"#2\"]},\n{\"op\":\"add\",\"workstream_id\":\"bad\",\"item_kind\":\"decision\",\"title\":\"x\",\"content\":\"y\",\"refs\":[]}]\n```";
        let session = Session {
            id: "s1".into(),
            agent: Agent::Codex,
            agent_session_id: "a1".into(),
            title: None,
            cwd: None,
            project_id: None,
            raw_path: "/tmp/x".into(),
            parent_agent_session_id: None,
            started_at: None,
            last_activity_at: None,
        };
        let m = parse_mutations(text, &["ws1".to_string()], &session).unwrap();
        assert_eq!(m.len(), 1, "invalid workstream filtered out");
        match &m[0] {
            ContextMutation::Add { workstream_id, source_ref, authority, .. } => {
                assert_eq!(workstream_id, "ws1");
                assert_eq!(source_ref, "session:s1#2");
                assert_eq!(authority, "agent_inferred");
            }
            other => panic!("unexpected mutation: {:?}", other),
        }
    }
}
