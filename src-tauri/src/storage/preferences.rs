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
    if list(conn, project)?.iter().any(|p| p.content == text) {
        return Err(other("已有相同偏好"));
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
        return Err(other("已有相同偏好"));
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
