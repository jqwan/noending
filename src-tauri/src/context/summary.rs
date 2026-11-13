//! Validation and locking shared by both summary submission entry points.
use super::{invalid, query};
use crate::domain::SessionContextFields;
use crate::error::{other, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Submission {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub reply: Value,
    pub results: HashMap<String, Value>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionReply {
    Completed {},
    Skipped { reason: String },
    Failed { error: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionFailure {
    pub session_id: String,
    pub error: String,
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
    #[serde(default)]
    id: String,
    title: String,
    description: String,
    details: Vec<String>,
}

pub fn parse_session_reply(raw: &str) -> Result<SessionReply> {
    let reply: SessionReply = serde_json::from_str(raw.trim())?;
    match &reply {
        SessionReply::Skipped { reason } if reason != "no_summary_content" => {
            return Err(other("未知跳过原因"))
        }
        SessionReply::Failed { error } if error.trim().is_empty() => {
            return Err(other("失败原因不能为空"))
        }
        _ => {}
    }
    Ok(reply)
}

pub(super) fn fields(
    conn: &Connection,
    kind: &str,
    id: &str,
    revision: i64,
    reply: &SessionReply,
    mut results: HashMap<String, Value>,
) -> Result<SessionContextFields> {
    let previous = query::target_summary(conn, kind, id, revision)?.unwrap_or_default();
    let summary = match reply {
        SessionReply::Completed {} => {
            let value = results
                .remove(&format!("{kind}-{id}"))
                .ok_or_else(|| invalid("缺少目标摘要"))?;
            let result: SummaryResult = serde_json::from_value(value).map_err(invalid)?;
            crate::domain::Summary {
                items: result
                    .summary
                    .items
                    .into_iter()
                    .map(|item| crate::domain::SummaryItem {
                        id: item.id,
                        title: item.title,
                        description: item.description,
                        details: item.details,
                    })
                    .collect(),
            }
            .validated_content()
            .map_err(invalid)?
            .resolve_extracted_ids(&previous)
            .map_err(invalid)?
        }
        SessionReply::Skipped { .. } => previous,
        SessionReply::Failed { .. } => return Err(invalid("失败的提取不能保存摘要")),
    };
    if !results.is_empty() {
        return Err(invalid("提交只能包含当前目标的结果"));
    }
    Ok(SessionContextFields { summary })
}

/// Locks coordinate autonomous and background submits, even across processes.
pub(super) fn lock_targets(
    root: &Path,
    target: &str,
    session_ids: &[String],
) -> Result<Vec<std::fs::File>> {
    let mut keys = vec![target.to_owned()];
    keys.extend(session_ids.iter().map(|id| format!("session-{id}")));
    keys.sort();
    keys.dedup();
    let directory = root.join(".locks");
    std::fs::create_dir_all(&directory)?;
    keys.into_iter()
        .map(|key| {
            use sha2::{Digest, Sha256};
            let name = format!("{:x}.lock", Sha256::digest(key.as_bytes()));
            Ok(crate::platform::file_lock::exclusive(
                &directory.join(name),
            )?)
        })
        .collect()
}

pub(super) struct RunDirectory(pub PathBuf);

impl RunDirectory {
    pub fn create(root: &Path, target: &str) -> Result<Self> {
        let path = root.join(format!("{target}-{}", crate::storage::new_id()));
        crate::cli::transport::private_dir(&path).map_err(other)?;
        Ok(Self(path))
    }
}
impl Drop for RunDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(super) const TIME_RULES: &str = "started_at 是会话起始时间，last_message_at 是本次材料最后消息时间，summary_through_at 是旧摘要覆盖的最后消息时间；均可能为空。以具体消息时间和明确修正判断事实新旧。输出完整摘要，延续事项保留原 ID，合并保留一个原 ID，拆分只有一个沿用原 ID，新增事项省略 ID。";
