//! Explicit work tracking, independent of generated summaries and their frontiers.
use crate::{
    error::{other, AppError, Result},
    storage::{new_id, now, Db},
};
use rmcp::schemars;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskItem {
    pub id: String,
    pub task_id: String,
    pub title: String,
    pub description: String,
    /// Confirmed progress, results and blockers, not subtasks.
    pub details: Vec<String>,
    pub completed: bool,
    pub revision: i64,
    pub created_at: String,
    pub updated_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateItem {
    pub title: String,
    pub description: Option<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ItemPatch {
    pub title: Option<String>,
    pub description: Option<String>,
    pub details: Option<Vec<String>>,
    pub completed: Option<bool>,
}
fn require_task(conn: &Connection, task: &str) -> Result<()> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM workstreams WHERE id=?1)",
        [task],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(other("任务不存在"));
    }
    Ok(())
}
fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<TaskItem> {
    let details: String = r.get(4)?;
    Ok(TaskItem {
        id: r.get(0)?,
        task_id: r.get(1)?,
        title: r.get(2)?,
        description: r.get(3)?,
        details: serde_json::from_str(&details).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e))
        })?,
        completed: r.get(5)?,
        revision: r.get(6)?,
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
    })
}
const SELECT: &str = "SELECT id,task_id,title,description,details,completed,revision,created_at,updated_at FROM task_items";
pub fn list(db: &Db, task: &str) -> Result<Vec<TaskItem>> {
    let conn = db.read();
    require_task(&conn, task)?;
    let mut stmt = conn.prepare(&format!("{SELECT} WHERE task_id=?1 ORDER BY created_at,id"))?;
    let items = stmt
        .query_map([task], row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(items)
}
fn validate(item: &mut TaskItem) -> Result<()> {
    item.title = item.title.trim().into();
    item.description = item.description.trim().into();
    if item.title.is_empty() || item.title.contains(['\r', '\n']) {
        return Err(other("任务事项标题必须为非空单行文字"));
    }
    for detail in &mut item.details {
        *detail = detail.trim().into();
        if detail.is_empty() {
            return Err(other("进展细则不能为空"));
        }
    }
    Ok(())
}
pub fn create(db: &Db, task: &str, content: CreateItem) -> Result<TaskItem> {
    let timestamp = now();
    let mut item = TaskItem {
        id: new_id(),
        task_id: task.into(),
        title: content.title,
        description: content.description.unwrap_or_default(),
        details: vec![],
        completed: false,
        revision: 1,
        created_at: timestamp.clone(),
        updated_at: timestamp,
    };
    validate(&mut item)?;
    db.tx(|tx| {
        require_task(tx,task)?;
        tx.execute("INSERT INTO task_items(id,task_id,title,description,details,completed,revision,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,0,1,?6,?6)",params![item.id,task,item.title,item.description,serde_json::to_string(&item.details)?,item.created_at])?;
        Ok(item)
    })
}
fn current(conn: &Connection, task: &str, id: &str, revision: i64) -> Result<TaskItem> {
    let item = conn
        .query_row(
            &format!("{SELECT} WHERE task_id=?1 AND id=?2"),
            params![task, id],
            row,
        )
        .optional()?
        .ok_or_else(|| other("该任务中不存在此任务事项"))?;
    if revision != item.revision {
        return Err(AppError::context(
            crate::domain::ContextUpdateError::ConcurrencyConflict(
                "任务事项已变化，请重新读取后编辑".into(),
            ),
        ));
    }
    Ok(item)
}
pub fn update(db: &Db, task: &str, id: &str, revision: i64, patch: ItemPatch) -> Result<TaskItem> {
    if patch.title.is_none()
        && patch.description.is_none()
        && patch.details.is_none()
        && patch.completed.is_none()
    {
        return Err(other("至少提供一个需要更新的字段"));
    }
    db.tx(|tx| {
        let mut item=current(tx,task,id,revision)?;
        if let Some(title)=patch.title {item.title=title;}
        if let Some(description)=patch.description {item.description=description;}
        if let Some(details)=patch.details {item.details=details;}
        if let Some(completed)=patch.completed {item.completed=completed;}
        validate(&mut item)?;
        item.revision+=1;
        item.updated_at=now();
        tx.execute("UPDATE task_items SET title=?3,description=?4,details=?5,completed=?6,revision=?7,updated_at=?8 WHERE task_id=?1 AND id=?2",params![task,id,item.title,item.description,serde_json::to_string(&item.details)?,item.completed,item.revision,item.updated_at])?;
        Ok(item)
    })
}

/// Append against the stored value so concurrent notes never replace each
/// other's progress. Full edits still require the latest item revision.
pub fn append_note(db: &Db, task: &str, id: &str, text: &str) -> Result<TaskItem> {
    let text = text.trim();
    if text.is_empty() {
        return Err(other("进展内容不能为空"));
    }
    db.tx(|tx| {
        let changed = tx.execute(
            "UPDATE task_items SET details=json_insert(details,'$[#]',?3),revision=revision+1,updated_at=?4 WHERE task_id=?1 AND id=?2",
            params![task, id, text, now()],
        )?;
        if changed == 0 {
            return Err(other("该任务中不存在此任务事项"));
        }
        Ok(tx.query_row(&format!("{SELECT} WHERE task_id=?1 AND id=?2"), params![task, id], row)?)
    })
}

pub fn delete(db: &Db, task: &str, id: &str, revision: i64) -> Result<()> {
    db.tx(|tx| {
        current(tx, task, id, revision)?;
        tx.execute(
            "DELETE FROM task_items WHERE task_id=?1 AND id=?2",
            params![task, id],
        )?;
        Ok(())
    })
}
