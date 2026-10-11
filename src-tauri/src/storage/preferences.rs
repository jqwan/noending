//! Current preference items only; no source history or deletion tombstones.
use super::{new_id, Db};
use crate::error::{other, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Preference {
    pub id: String,
    pub project_id: Option<String>,
    pub content: String,
    pub locked: bool,
    // A concurrency token, not a retained history/version list.
    #[serde(skip)]
    pub(crate) revision: i64,
}
fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Preference> {
    Ok(Preference {
        id: r.get(0)?,
        project_id: r.get(1)?,
        content: r.get(2)?,
        locked: r.get(3)?,
        revision: r.get(4)?,
    })
}
pub fn list(conn: &Connection, project: Option<&str>) -> Result<Vec<Preference>> {
    let mut st = conn.prepare("SELECT id,project_id,content,locked,revision FROM preferences WHERE project_id IS ?1 ORDER BY id")?;
    let items = st
        .query_map([project], row)?
        .collect::<std::result::Result<_, _>>()?;
    Ok(items)
}
pub(crate) fn get(conn: &Connection, id: &str) -> Result<Option<Preference>> {
    Ok(conn
        .query_row(
            "SELECT id,project_id,content,locked,revision FROM preferences WHERE id=?1",
            [id],
            row,
        )
        .optional()?)
}
fn content(text: &str) -> Result<&str> {
    let text = text.trim();
    if text.is_empty() {
        return Err(other("偏好正文不能为空"));
    }
    Ok(text)
}
pub(crate) fn insert(
    conn: &Connection,
    project: Option<&str>,
    text: &str,
    locked: bool,
) -> Result<Preference> {
    let text = content(text)?;
    if list(conn, project)?.iter().any(|p| p.content == text)
        || project.is_some() && list(conn, None)?.iter().any(|p| p.content == text)
    {
        return Err(other("已有相同的用户或项目偏好"));
    }
    let id = new_id();
    conn.execute(
        "INSERT INTO preferences(id,project_id,content,locked) VALUES (?1,?2,?3,?4)",
        params![id, project, text, locked],
    )?;
    Ok(get(conn, &id)?.unwrap())
}
pub(crate) fn update(conn: &Connection, id: &str, text: &str, locked: bool) -> Result<Preference> {
    let existing = get(conn, id)?.ok_or_else(|| other("偏好不存在"))?;
    let text = content(text)?;
    if text != existing.content
        && list(conn, existing.project_id.as_deref())?
            .iter()
            .any(|p| p.id != id && p.content == text)
    {
        return Err(other("已有相同的用户或项目偏好"));
    }
    if text != existing.content
        && existing.project_id.is_some()
        && list(conn, None)?.iter().any(|p| p.content == text)
    {
        return Err(other("已有相同的用户偏好"));
    }
    conn.execute(
        "UPDATE preferences SET content=?1,locked=?2,revision=revision+1 WHERE id=?3",
        params![text, locked, id],
    )?;
    Ok(get(conn, id)?.unwrap())
}
pub fn list_items(db: &Db, project: Option<&str>) -> Result<Vec<Preference>> {
    list(&db.read(), project)
}
pub fn save_item(
    db: &Db,
    id: Option<&str>,
    project: Option<&str>,
    text: &str,
    locked: bool,
) -> Result<Preference> {
    db.tx(|tx| match id {
        Some(id) => {
            let current = get(tx, id)?.ok_or_else(|| other("偏好不存在"))?;
            if current.project_id.as_deref() != project {
                return Err(other("不能改变偏好所属范围"));
            }
            update(tx, id, text, locked)
        }
        None => insert(tx, project, text, locked),
    })
}
pub fn delete_item(db: &Db, id: &str) -> Result<()> {
    db.tx(|tx| {
        if tx.execute("DELETE FROM preferences WHERE id=?1", [id])? == 0 {
            return Err(other("偏好不存在"));
        }
        Ok(())
    })
}

/// Explicit user operation; locked items may be promoted and keep their lock.
pub fn promote_item(db: &Db, id: &str) -> Result<Preference> {
    db.tx(|tx| {
        let item = get(tx, id)?.ok_or_else(|| other("偏好不存在"))?;
        if item.project_id.is_none() {
            return Ok(item);
        }
        if let Some(existing) = list(tx, None)?
            .into_iter()
            .find(|p| p.content == item.content)
        {
            tx.execute(
                "UPDATE preferences SET locked=?2,revision=revision+1 WHERE id=?1",
                params![existing.id, existing.locked || item.locked],
            )?;
            tx.execute("DELETE FROM preferences WHERE id=?1", [id])?;
            return Ok(get(tx, &existing.id)?.unwrap());
        }
        tx.execute(
            "UPDATE preferences SET project_id=NULL,revision=revision+1 WHERE id=?1",
            [id],
        )?;
        Ok(get(tx, id)?.unwrap())
    })
}
pub(crate) fn merge_project_items(conn: &Connection, from: &str, to: &str) -> Result<()> {
    for item in list(conn, Some(from))? {
        let global = list(conn, None)?
            .into_iter()
            .find(|p| p.content == item.content);
        let duplicate = global.or(list(conn, Some(to))?
            .into_iter()
            .find(|p| p.content == item.content));
        if let Some(existing) = duplicate {
            conn.execute(
                "UPDATE preferences SET locked=?2,revision=revision+1 WHERE id=?1",
                params![existing.id, existing.locked || item.locked],
            )?;
            conn.execute("DELETE FROM preferences WHERE id=?1", [item.id])?;
        } else {
            conn.execute(
                "UPDATE preferences SET project_id=?2,revision=revision+1 WHERE id=?1",
                params![item.id, to],
            )?;
        }
    }
    // Existing destination items can also duplicate a user item added later.
    for item in list(conn, Some(to))? {
        if let Some(global) = list(conn, None)?
            .into_iter()
            .find(|p| p.content == item.content)
        {
            conn.execute(
                "UPDATE preferences SET locked=?2,revision=revision+1 WHERE id=?1",
                params![global.id, global.locked || item.locked],
            )?;
            conn.execute("DELETE FROM preferences WHERE id=?1", [item.id])?;
        }
    }
    Ok(())
}
