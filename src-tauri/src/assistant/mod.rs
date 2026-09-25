//! Workspace Assistant — Interactive Mode.
//!
//! The assistant's intelligence runs through the user's own agent CLI in
//! headless mode (codex exec / claude -p / pi -p), fed with a retrieval
//! snapshot from the same Domain API the background sync uses. Proposed
//! write-actions come back as a fenced `noending-action` JSON block that
//! the UI must confirm before execution (policy: 技术实现方案 §25/§26).

use serde::Serialize;

use crate::error::{other, Result};
use crate::storage::{AssistantMessageRow, Db};
use crate::sync::extractor::CliExtractor;

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ActionProposal {
    pub action: String, // launch_new_session | resume_session
    #[serde(default)]
    pub agent: Option<String>,
    /// The single Owner Workstream for a new Session, or none.
    #[serde(default)]
    pub owner_workstream_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantReply {
    pub session_id: String,
    pub content: String,
    pub action: Option<ActionProposal>,
    pub runtime: String,
}

const ACTION_FENCE_BEGIN: &str = "```noending-action";
const ACTION_FENCE_END: &str = "```";

fn split_action(text: &str) -> (String, Option<ActionProposal>) {
    if let Some(start) = text.find(ACTION_FENCE_BEGIN) {
        let after = &text[start + ACTION_FENCE_BEGIN.len()..];
        if let Some(end) = after.find(ACTION_FENCE_END) {
            let json = after[..end].trim();
            let content = format!(
                "{}{}",
                &text[..start],
                &after[end + ACTION_FENCE_END.len()..]
            )
            .trim()
            .to_string();
            match serde_json::from_str::<ActionProposal>(json) {
                Ok(a) => return (content, Some(a)),
                Err(e) => {
                    eprintln!("[assistant] action parse failed: {}", e);
                    return (text.trim().to_string(), None);
                }
            }
        }
    }
    (text.trim().to_string(), None)
}

const SYSTEM_INSTRUCTIONS: &str = r#"你是 NoEnding 的 Workspace Assistant。NoEnding 是一个以 Workstream Context 为核心的本地多 Agent 工作空间：Project 是 WorkspacePath 的物理聚合（描述这次执行在哪个目录 / 仓库），Workstream 是跨目录、跨 Project 的持续工作目标，保存持续演进的语义上下文（Goal / Current State / Constraints / Decisions / Open Questions + 扩展条目），Session 是 Codex / Claude Code / Pi 的一次实际执行，最多属于一个 Workstream（Owner）。

规则：
1. 基于下方「领域快照」回答用户问题，语言跟随用户；找不到就说找不到，不要编造；
2. 只做只读分析。如果用户的意图需要执行写操作（启动会话 / 恢复会话），不要描述执行过程，而是在回复末尾输出一个动作块：
```noending-action
{"action":"launch_new_session","agent":"claude_code","owner_workstream_id":"<id>","cwd":"/可选"}
```
或
{"action":"resume_session","session_id":"<id>"}
3. action/agent 取值只能用快照里给出的 id 和 codex/claude_code/pi；
4. 保持简洁克制，不使用夸张营销语言。"#;

fn build_domain_snapshot(db: &Db, query: &str) -> Result<String> {
    let mut snap = String::new();

    snap.push_str("== Workstreams ==\n");
    for w in db.list_workstreams(None)?.into_iter().take(20) {
        snap.push_str(&format!(
            "- id={} | {} | lifecycle={} | {}\n",
            w.id,
            w.title,
            w.lifecycle,
            if w.description.is_empty() {
                ""
            } else {
                &w.description
            }
        ));
    }

    snap.push_str("\n== 相关检索命中 ==\n");
    let hits = crate::search::search(db, query, 8)?;
    if hits.is_empty() {
        snap.push_str("(none)\n");
    }
    for h in &hits {
        snap.push_str(&format!(
            "- [{}] {} | parent={}\n",
            h.kind, h.title, h.parent_id
        ));
    }

    snap.push_str("\n== 最近同步 ==\n");
    for r in db.list_sync_runs(5)? {
        snap.push_str(&format!(
            "- [{}] {} ({})\n",
            r.created_at, r.summary, r.runtime
        ));
    }

    Ok(snap)
}

fn build_conversation_history(msgs: &[AssistantMessageRow]) -> String {
    let start = msgs.len().saturating_sub(6);
    let mut out = String::new();
    for m in &msgs[start..] {
        out.push_str(&format!(
            "{}: {}\n",
            if m.role == "user" {
                "用户"
            } else {
                "Assistant"
            },
            crate::adapters::truncate_text(&m.content, 500)
        ));
    }
    out
}

pub struct AssistantService;

impl AssistantService {
    /// One interactive turn: retrieve → CLI headless call → parse action.
    pub fn chat(db: &Db, session_id: Option<&str>, user_text: &str) -> Result<AssistantReply> {
        let user_text = user_text.trim();
        if user_text.is_empty() {
            return Err(other("消息不能为空"));
        }
        let sid = db.ensure_assistant_session(session_id)?;
        db.insert_assistant_message(&sid, "user", user_text, None, None)?;

        let history = build_conversation_history(&db.list_assistant_messages(&sid, 50)?);
        let snapshot = build_domain_snapshot(db, user_text)?;
        let prompt = format!(
            "{sys}\n\n== 领域快照 ==\n{snap}\n\n== 对话历史 ==\n{hist}\n\n用户: {q}",
            sys = SYSTEM_INSTRUCTIONS,
            snap = snapshot,
            hist = history,
            q = user_text
        );

        let (content, runtime) = match CliExtractor::try_from_settings(db)? {
            Some(cli) if crate::adapters::adapter_for(cli.agent).detect().is_some() => {
                let install = crate::platform::exec_resolver::resolve(cli.agent)?;
                let adapter = crate::adapters::adapter_for(cli.agent);
                let cmd = adapter.build_exec_command(&install, &cli.opts, &prompt)?;
                let out = crate::platform::exec_runner::run_headless(&cmd, 180)?;
                let text = crate::platform::exec_runner::clean_exec_stdout(&out.stdout);
                if text.is_empty() {
                    (
                        "（模型返回为空，请重试，或到 Settings → Agents 调整该 Agent 的 Runtime。）".to_string(),
                        "empty".to_string(),
                    )
                } else {
                    (text, crate::sync::ContextExtractor::name(&cli))
                }
            }
            _ => {
                // no LLM configured: fall back to pure retrieval answer
                let hits = crate::search::search(db, user_text, 8)?;
                let text = if hits.is_empty() {
                    "尚未选择 Assistant 的 Agent，或该 Agent CLI 未安装，且检索没有命中。请在 Settings → Agents 配置。".to_string()
                } else {
                    format!(
                        "尚未选择 Assistant 的 Agent（或该 Agent CLI 未安装），以下是通过检索找到的相关内容：\n{}",
                        hits.iter()
                            .map(|h| format!(
                                "- [{}] {}",
                                h.kind,
                                crate::adapters::truncate_text(&h.title, 80)
                            ))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                };
                (text, "retrieval-only".to_string())
            }
        };

        let (content, action) = split_action(&content);
        let action_json = action
            .as_ref()
            .map(|a| serde_json::to_string(a).unwrap_or_default());
        db.insert_assistant_message(
            &sid,
            "assistant",
            &content,
            action_json.as_deref(),
            Some(&runtime),
        )?;

        Ok(AssistantReply {
            session_id: sid,
            content,
            action,
            runtime,
        })
    }

    /// Execute a user-confirmed action proposal via the Session Launcher.
    ///
    /// The launcher and the §13 tier-3 [`crate::launcher::LaunchWorkspace`] are
    /// injected rather than built here: an Assistant launch must resolve its
    /// directory through the same Home the New Session modal previewed, and
    /// `NoEnding Home` is a fact only the command layer holds.
    pub fn execute_action(
        db: &Db,
        action: &ActionProposal,
        launcher: &crate::launcher::SessionLauncher,
        workspace: &crate::launcher::LaunchWorkspace,
    ) -> Result<serde_json::Value> {
        match action.action.as_str() {
            "launch_new_session" => {
                let agent = action
                    .agent
                    .as_deref()
                    .and_then(crate::domain::Agent::parse)
                    .ok_or_else(|| other("动作缺少有效 agent"))?;
                let r = launcher.new_session_in(
                    db,
                    agent,
                    action.owner_workstream_id.as_deref(),
                    action.cwd.as_deref(),
                    workspace,
                )?;
                Ok(
                    serde_json::json!({ "ok": true, "kind": "launch", "launched_via": r.launched_via, "note": r.note }),
                )
            }
            "resume_session" => {
                let sid = action
                    .session_id
                    .as_deref()
                    .ok_or_else(|| other("动作缺少 session_id"))?;
                let r = launcher.resume_session_in(db, sid, workspace)?;
                Ok(
                    serde_json::json!({ "ok": true, "kind": "resume", "launched_via": r.launched_via, "note": r.note }),
                )
            }
            other_action => Err(other(format!("未知动作: {}", other_action))),
        }
    }
}

/// Convenience for tests / debugging: run a bare prompt through the
/// configured assistant CLI and return cleaned stdout.
#[allow(dead_code)]
pub fn raw_prompt(db: &Db, prompt: &str) -> Result<String> {
    let cli = CliExtractor::try_from_settings(db)?
        .ok_or_else(|| other("Assistant 未选择 Agent（Settings → Agents 配置 Runtime）"))?;
    let install = crate::platform::exec_resolver::resolve(cli.agent)?;
    let adapter = crate::adapters::adapter_for(cli.agent);
    let cmd = adapter.build_exec_command(&install, &cli.opts, prompt)?;
    let out = crate::platform::exec_runner::run_headless(&cmd, 180)?;
    Ok(crate::platform::exec_runner::clean_exec_stdout(&out.stdout))
}
