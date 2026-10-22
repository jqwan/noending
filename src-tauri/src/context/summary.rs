//! Scoped lazy database queries and result protocol for session, task and project summaries.
use super::prompts::{PromptKind, PromptTemplates};
use super::query::Input;
use super::SessionSnapshot;
use crate::domain::SessionContextFields;
use crate::error::{other, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionReply {
    Completed {},
    Skipped { reason: String },
    Failed { error: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionFailure {
    pub session_id: String,
    pub error: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum BatchReply {
    Completed {
        skipped_session_ids: Vec<String>,
    },
    Partial {
        failed_sessions: Vec<SessionFailure>,
        skipped_session_ids: Vec<String>,
    },
    Skipped {
        reason: String,
        skipped_session_ids: Vec<String>,
    },
    Failed {
        error: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SummaryResult {
    summary: GeneratedSummary,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GeneratedSummary {
    items: Vec<GeneratedItem>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GeneratedItem {
    title: String,
    description: String,
    details: Vec<String>,
}

pub fn parse_session_reply(raw: &str) -> Result<SessionReply> {
    let reply: SessionReply = serde_json::from_str(raw.trim())?;
    match &reply {
        SessionReply::Skipped { reason, .. } if reason != "no_summary_content" => {
            return Err(other("未知跳过原因"))
        }
        SessionReply::Failed { error } if error.trim().is_empty() => {
            return Err(other("失败原因不能为空"))
        }
        _ => {}
    }
    Ok(reply)
}

pub fn parse_batch_reply(raw: &str) -> Result<BatchReply> {
    let reply: BatchReply = serde_json::from_str(raw.trim())?;
    match &reply {
        BatchReply::Partial {
            failed_sessions, ..
        } if failed_sessions.is_empty() => return Err(other("partial 必须列出失败会话")),
        BatchReply::Failed { error } if error.trim().is_empty() => {
            return Err(other("失败原因不能为空"))
        }
        BatchReply::Skipped { reason, .. } if reason != "no_summary_content" => {
            return Err(other("未知跳过原因"))
        }
        _ => {}
    }
    Ok(reply)
}

/// A single invocation owns its directory and all involved session identities.
/// Unique directories isolate abandoned children; OS locks cover desktop and CLI calls.
pub struct ExtractionFiles {
    pub dir: PathBuf,
    pub(super) target: String,
    _locks: Vec<std::fs::File>,
    pub(super) inputs: Mutex<HashMap<String, Input>>,
}
impl ExtractionFiles {
    pub fn create(root: &Path, target: &str, session_ids: &[String]) -> Result<Self> {
        let mut keys = vec![root.join(target)];
        keys.extend(
            session_ids
                .iter()
                .map(|id| root.join(format!("session-{id}"))),
        );
        keys.sort();
        keys.dedup();
        std::fs::create_dir_all(root.join(".locks"))?;
        let mut locks = Vec::new();
        for key in &keys {
            use sha2::{Digest, Sha256};
            let name = format!(
                "{:x}.lock",
                Sha256::digest(key.file_name().unwrap().to_string_lossy().as_bytes())
            );
            locks.push(crate::platform::file_lock::exclusive(
                &root.join(".locks").join(name),
            )?);
        }
        let files = Self {
            dir: root.join(format!("{target}-{}", crate::storage::new_id())),
            _locks: locks,
            target: target.into(),
            inputs: Mutex::new(HashMap::new()),
        };
        // Never reuse an interrupted invocation's result paths.
        std::fs::create_dir_all(&files.dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&files.dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(files)
    }
    pub fn session_input(&self, id: &str) -> PathBuf {
        self.dir.join(format!("session-{id}.jsonl"))
    }
    pub fn session_result(&self, id: &str) -> PathBuf {
        self.dir.join(format!("session-{id}.result.json"))
    }
    pub fn batch_input(&self, kind: &str, id: &str) -> PathBuf {
        self.dir.join(format!("{kind}-{id}.jsonl"))
    }
    pub fn batch_result(&self, kind: &str, id: &str) -> PathBuf {
        self.dir.join(format!("{kind}-{id}.result.json"))
    }

    pub(super) fn add_session(
        &self,
        snapshot: &SessionSnapshot,
        include_summary: bool,
        full_history: bool,
    ) {
        let name = if full_history {
            self.history_session_input(&snapshot.session.id)
        } else {
            self.session_input(&snapshot.session.id)
        };
        self.inputs.lock().unwrap().insert(
            name.file_name().unwrap().to_string_lossy().into_owned(),
            Input::Session {
                snapshot: snapshot.clone(),
                include_summary,
                full_history,
            },
        );
    }
    pub fn history_session_input(&self, id: &str) -> PathBuf {
        self.dir.join(format!("history-session-{id}.jsonl"))
    }
    pub(super) fn add_batch(
        &self,
        kind: &str,
        id: &str,
        header: serde_json::Value,
        revision: i64,
        regenerate: bool,
        sessions: Vec<SessionSnapshot>,
    ) {
        self.inputs.lock().unwrap().insert(
            self.batch_input(kind, id)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            Input::Batch {
                kind: kind.into(),
                id: id.into(),
                header,
                revision,
                regenerate,
                sessions,
            },
        );
    }
    pub fn read_result(&self, path: &Path) -> Result<SessionContextFields> {
        let result: SummaryResult = serde_json::from_str(&self.read_text_result(path)?)?;
        let summary = crate::domain::Summary {
            items: result
                .summary
                .items
                .into_iter()
                .map(|item| crate::domain::SummaryItem {
                    title: item.title,
                    description: item.description,
                    details: item.details,
                    title_locked: false,
                })
                .collect(),
        }
        .validated()?;
        Ok(SessionContextFields { summary })
    }
    pub(super) fn read_text_result(&self, path: &Path) -> Result<String> {
        if !std::fs::symlink_metadata(path)?.file_type().is_file() {
            return Err(other("结果必须是普通文件"));
        }
        Ok(std::fs::read_to_string(path)?)
    }
    pub fn require_absent(&self, path: &Path) -> Result<()> {
        match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
            Ok(_) => Err(other("失败或跳过的目标不应有结果文件")),
        }
    }
    pub fn session_prompt(&self, id: &str, templates: &PromptTemplates) -> String {
        session_prompt(&self.session_input(id), &self.session_result(id), templates)
    }
    pub fn batch_prompt(&self, kind: &str, id: &str, templates: &PromptTemplates) -> String {
        batch_prompt(
            kind,
            &self.batch_input(kind, id),
            &self.batch_result(kind, id),
            templates,
        )
    }
}
impl Drop for ExtractionFiles {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn quoted(path: &Path) -> String {
    serde_json::to_string(&path.to_string_lossy()).unwrap()
}
pub fn session_prompt(input: &Path, result: &Path, templates: &PromptTemplates) -> String {
    format!(
        "根据本次会话输入生成完整、独立可读的摘要。\n输入查询名称：{}\n结果文件：{}\n{}\n## 会话摘要要求\n{}",
        quoted(input), quoted(result), TIME_RULES, templates.session
    )
}
const TIME_RULES: &str = "started_at 是会话起始时间，last_message_at 是本次材料最后消息时间，summary_through_at 是旧摘要覆盖的最后消息时间；均可能为空。判断事实新旧以具体消息 timestamp、内容和明确修正关系为依据，不将会话最后消息时间当作其中所有事实的发生时间。已有摘要与消息共同构成材料，输出完整摘要而非增量补丁。输入 locked_titles 中的每个标题必须原样保留，不能修改、删除或合并，描述和细则仍需根据材料更新；重新生成同样遵守。锁定标题优先于项数精简目标，锁定状态由程序管理，结果不输出 title_locked。";
pub fn batch_prompt(
    kind: &str,
    input: &Path,
    result: &Path,
    templates: &PromptTemplates,
) -> String {
    let (label, requirements, background) = if kind == "project" {
        ("项目", templates.project.as_str(), "项目材料包含名称、类型、目录和经由目录关联的会话（包括已归档会话）。相关任务标题和描述仅供背景，不提取任务摘要，不引入其他项目会话。")
    } else {
        (
            "任务",
            templates.task.as_str(),
            "任务材料包含标题、描述、目录及归属于当前任务的会话。",
        )
    };
    format!(
        "先更新会话摘要，再综合生成{label}摘要。\n输入查询名称：{}\n结果文件：{}\n{background}\n清单按 last_message_at 从早到晚排列，未知时间在前，顺序不代表结论优先级。{}\n逐个完整读取 need_update=true 的会话输入，成功摘要写到 result_file；无需更新的会话复用 current_summary。每份会话摘要只依据自身材料。失败会话有旧摘要就复用，否则暂不纳入；继续处理其他会话。\n综合时核对重要历史变更：提供的 history_input_file 也可用于核对原始事实；检索后完整读取命中消息及前后讨论，结合 timestamp 综合。不仅比较摘要名称，没有原始证据时保留必要不确定性，不扫描工作目录。\n## 会话摘要要求\n{}\n## {label}摘要要求\n{requirements}",
        quoted(input), quoted(result), TIME_RULES, templates.session
    )
}
/// Preview the skill, protocol and the same requirements used at runtime.
pub fn preview_prompt(kind: PromptKind, templates: &PromptTemplates) -> String {
    let input = Path::new("{{输入查询名称}}");
    let result = Path::new("{{结果文件路径}}");
    let requirements = match kind {
        PromptKind::Session => session_prompt(input, result, templates),
        PromptKind::Task => batch_prompt("task", input, result, templates),
        PromptKind::Project => batch_prompt("project", input, result, templates),
    };
    super::tools::preview(&requirements)
}
