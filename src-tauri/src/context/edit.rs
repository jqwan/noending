//! Explicit summary edits change the text/revision, never extraction frontiers.
use crate::domain::Summary;
use crate::error::{other, Result};
use crate::storage::{context_repo as repo, project_context_repo, Db};
use rmcp::schemars;
use rusqlite::params;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskTodoContent {
    pub title: String,
    /// Optional goal/scope description. Omission preserves an existing item's description.
    pub description: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskTodoResult {
    /// Complete replacement body, including confirmed results and necessary context.
    pub description: String,
    /// Confirmed progress, outcomes and verification, not a checklist of child tasks.
    pub details: Vec<String>,
}

#[derive(serde::Serialize)]
pub struct SummaryEditResult {
    pub revision: i64,
    pub summary: Summary,
}
pub fn edit_summary(
    db: &Db,
    kind: &str,
    id: &str,
    expected_revision: i64,
    text: &str,
) -> Result<SummaryEditResult> {
    // CLI/MCP require a nonempty document; the desktop editor also supports
    // explicitly removing the last item without changing extraction progress.
    let summary: Summary = serde_json::from_str(text)?;
    save_summary(
        db,
        kind,
        id,
        expected_revision,
        summary.validated_content()?,
    )
}

pub fn save_summary(
    db: &Db,
    kind: &str,
    id: &str,
    expected_revision: i64,
    summary: Summary,
) -> Result<SummaryEditResult> {
    let summary = if summary.is_empty() {
        summary
    } else {
        summary.validated_content()?
    };
    mutate_summary(db, kind, id, expected_revision, |previous| {
        let summary = summary.resolve_ids()?;
        summary.validate_title_locks(&previous)?;
        Ok(summary)
    })
}

pub fn set_summary_title_lock(
    db: &Db,
    kind: &str,
    id: &str,
    expected_revision: i64,
    item_number: usize,
    locked: bool,
) -> Result<SummaryEditResult> {
    mutate_summary(db, kind, id, expected_revision, |mut summary| {
        let index = item_number
            .checked_sub(1)
            .ok_or_else(|| other("事项序号从 1 开始"))?;
        let item = summary
            .items
            .get_mut(index)
            .ok_or_else(|| other("事项不存在"))?;
        item.title_locked = locked;
        Ok(summary)
    })
}

/// Publish one explicit item without replacing the surrounding task summary.
pub fn publish_task_todo(
    db: &Db,
    id: &str,
    expected_revision: i64,
    item_number: Option<usize>,
    content: TaskTodoContent,
) -> Result<SummaryEditResult> {
    mutate_summary(db, "task", id, expected_revision, |mut summary| {
        let title = content.title.trim().to_string();
        if let Some(number) = item_number {
            let previous = task_item_mut(&mut summary, number)?;
            if previous.title_locked && previous.title != title {
                return Err(other("不能通过发布待办修改已锁定的事项标题"));
            }
            previous.title = title;
            if let Some(description) = content.description {
                previous.description = description;
            }
            previous.title_locked = true;
        } else {
            summary.items.push(crate::domain::SummaryItem {
                id: crate::storage::new_id(),
                title,
                description: content.description.unwrap_or_default(),
                details: Vec::new(),
                title_locked: true,
            });
        }
        summary.validated()
    })
}

/// Completion unlocks the title, optionally committing a replacement body together.
pub fn complete_task_todo(
    db: &Db,
    id: &str,
    expected_revision: i64,
    item_number: usize,
    result: Option<TaskTodoResult>,
) -> Result<SummaryEditResult> {
    mutate_summary(db, "task", id, expected_revision, |mut summary| {
        let item = task_item_mut(&mut summary, item_number)?;
        if !item.title_locked {
            return Err(other("完成待办要求目标事项标题已锁定"));
        }
        if let Some(result) = result {
            if result.description.trim().is_empty() && result.details.is_empty() {
                return Err(other("提供完成结果时内容不能为空"));
            }
            item.description = result.description;
            item.details = result.details;
        }
        item.title_locked = false;
        summary.validated()
    })
}

/// Discard is an explicit removal action, not a persisted item status.
pub fn discard_task_todo(
    db: &Db,
    id: &str,
    expected_revision: i64,
    item_number: usize,
) -> Result<SummaryEditResult> {
    mutate_summary(db, "task", id, expected_revision, |mut summary| {
        if !task_item_mut(&mut summary, item_number)?.title_locked {
            return Err(other("废弃待办要求目标事项标题已锁定"));
        }
        summary.items.remove(item_number - 1);
        Ok(summary)
    })
}

fn task_item_mut(summary: &mut Summary, number: usize) -> Result<&mut crate::domain::SummaryItem> {
    number
        .checked_sub(1)
        .and_then(|index| summary.items.get_mut(index))
        .ok_or_else(|| other("事项不存在，序号须从 1 开始并对应当前摘要版本"))
}

fn mutate_summary(
    db: &Db,
    kind: &str,
    id: &str,
    expected_revision: i64,
    change: impl FnOnce(Summary) -> Result<Summary>,
) -> Result<SummaryEditResult> {
    if expected_revision < 0 {
        return Err(other("需要当前摘要版本"));
    }
    db.tx(|tx| {
        let table=match kind {"session"=>"sessions","task"=>"workstreams","project"=>"projects",_=>return Err(other("摘要类型无效"))};
        let exists:bool=tx.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id=?1)"),[id],|r|r.get(0))?;
        if !exists {return Err(other("摘要目标不存在"));}
        let revision=match kind {
            "session"=>repo::get_session_context_conn(tx,id)?.map_or(0,|c|c.revision),
            "task"=>repo::get_workstream_context_state_conn(tx,id)?.context_revision,
            _=>project_context_repo::state(tx,id)?.revision,
        };
        if revision!=expected_revision {return Err(super::stale());}
        let previous = super::query::target_summary(tx,kind,id,revision)?.unwrap_or_default();
        let summary = change(previous)?;
        match kind {
            "session"=> {
                let existing=repo::get_session_context_conn(tx,id)?;
                let generation=repo::get_ingest_state_conn(tx,id)?.0;
                repo::commit_session_context_conn(tx,id,&crate::domain::SessionContextFields{summary:summary.clone()},revision,existing.as_ref().map_or(generation,|c|c.ingest_generation),existing.as_ref().map_or(0,|c|c.processed_through_seq))?;
            },
            "task"=> {
                repo::ensure_workstream_state_conn(tx,id)?;
                tx.execute("UPDATE workstream_context_state SET summary=?2,context_revision=context_revision+1,updated_at=?3 WHERE workstream_id=?1",params![id,summary,crate::storage::now()])?;

            },
            _=> {
                tx.execute("INSERT INTO project_context_state(project_id,summary,context_revision,updated_at) VALUES (?1,?2,1,?3) ON CONFLICT(project_id) DO UPDATE SET summary=?2,context_revision=context_revision+1,updated_at=?3",params![id,summary,crate::storage::now()])?;

            },
        }
        Ok(SummaryEditResult { revision:revision+1, summary })
    })
}
