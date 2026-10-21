//! Item-level summary retrieval. Item numbers belong to one document revision.
use crate::{
    domain::SummaryItem,
    error::{other, Result},
    storage::Db,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryFilter {
    pub kind: Option<String>,
    pub project: Option<String>,
    pub scope: String,
}
impl SummaryFilter {
    pub fn validate(&self) -> Result<()> {
        if self
            .kind
            .as_deref()
            .is_some_and(|kind| !matches!(kind, "session" | "task" | "project"))
            || !matches!(self.scope.as_str(), "unarchived" | "archived" | "all")
        {
            return Err(other("摘要类型或归档范围无效"));
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub struct SummaryHit {
    pub kind: String,
    pub id: String,
    pub title: String,
    pub revision: i64,
    pub item_number: usize,
    pub item: SummaryItem,
}

// The same scoped document set feeds the page and its version fingerprint.
// No derived Markdown or redundant summary search index is persisted.
const DOCUMENTS: &str = r#"
WITH documents AS (
 SELECT 'session' kind,s.id,COALESCE(s.title,'') title,CASE WHEN s.archived_at IS NULL THEN 'normal' ELSE 'archived' END visibility,c.revision,c.summary
 FROM session_contexts c JOIN sessions s ON s.id=c.session_id
 WHERE (?2 IS NULL OR EXISTS(SELECT 1 FROM workspace_paths p WHERE p.id=s.workspace_path_id AND p.project_id=?2))
 UNION ALL
 SELECT 'task',w.id,w.title,w.visibility,c.context_revision,c.summary
 FROM workstream_context_state c JOIN workstreams w ON w.id=c.workstream_id
 WHERE (?2 IS NULL OR EXISTS(SELECT 1 FROM workstream_projects p WHERE p.workstream_id=w.id AND p.project_id=?2))
 UNION ALL
 SELECT 'project',p.id,p.name,p.visibility,c.context_revision,c.summary
 FROM project_context_state c JOIN projects p ON p.id=c.project_id WHERE (?2 IS NULL OR p.id=?2)
), scoped AS (
 SELECT * FROM documents WHERE (?1 IS NULL OR kind=?1)
 AND (?3='all' OR (?3='archived' AND visibility='archived') OR (?3='unarchived' AND visibility='normal'))
)
"#;

pub fn fingerprint(conn: &Connection, filter: &SummaryFilter) -> Result<String> {
    filter.validate()?;
    let mut stmt = conn.prepare(&format!(
        "{DOCUMENTS} SELECT kind,id,revision FROM scoped ORDER BY kind,id"
    ))?;
    let rows = stmt
        .query_map(params![filter.kind, filter.project, filter.scope], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&rows)?)))
}

pub fn search_page(
    conn: &Connection,
    query: &str,
    filter: &SummaryFilter,
    offset: usize,
    limit: usize,
) -> Result<Vec<SummaryHit>> {
    filter.validate()?;
    if query.trim().is_empty() || !(1..=51).contains(&limit) || offset > i64::MAX as usize - 51 {
        return Err(other("摘要检索参数无效"));
    }
    let sql = format!(
        r#"{DOCUMENTS}
 SELECT d.kind,d.id,d.title,d.revision,CAST(i.key AS INTEGER)+1,i.value
 FROM scoped d,json_each(d.summary,'$.items') i
 WHERE instr(lower(json_extract(i.value,'$.title')),lower(?4))>0
    OR instr(lower(json_extract(i.value,'$.description')),lower(?4))>0
    OR EXISTS(SELECT 1 FROM json_each(i.value,'$.details') detail WHERE instr(lower(detail.value),lower(?4))>0)
 ORDER BY d.kind,d.id,CAST(i.key AS INTEGER) LIMIT ?5 OFFSET ?6"#
    );
    let mut stmt = conn.prepare(&sql)?;
    let raw = stmt
        .query_map(
            params![
                filter.kind,
                filter.project,
                filter.scope,
                query.trim(),
                limit as i64,
                offset as i64
            ],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, usize>(4)?,
                    r.get::<_, String>(5)?,
                ))
            },
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    raw.into_iter()
        .map(|(kind, id, title, revision, item_number, raw)| {
            Ok(SummaryHit {
                kind,
                id,
                title,
                revision,
                item_number,
                item: serde_json::from_str(&raw)?,
            })
        })
        .collect()
}

pub fn read_item(
    db: &Db,
    kind: &str,
    id: &str,
    revision: i64,
    number: usize,
) -> Result<SummaryItem> {
    let index = number
        .checked_sub(1)
        .filter(|index| *index <= i64::MAX as usize)
        .ok_or_else(|| other("事项序号无效"))?;
    let (table, key, version) = match kind {
        "session" => ("session_contexts", "session_id", "revision"),
        "task" => (
            "workstream_context_state",
            "workstream_id",
            "context_revision",
        ),
        "project" => ("project_context_state", "project_id", "context_revision"),
        _ => return Err(other("摘要类型无效")),
    };
    // Select only the requested item's JSON, not the entire summary body.
    let sql = format!("SELECT {version}, json_extract(summary,'$.items[' || ?2 || ']') FROM {table} WHERE {key}=?1");
    let row: Option<(i64, Option<String>)> = db
        .read()
        .query_row(&sql, params![id, index as i64], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?;
    if row.as_ref().map_or(0, |row| row.0) != revision {
        return Err(super::stale());
    }
    let raw = row
        .and_then(|row| row.1)
        .ok_or_else(|| other("摘要事项不存在"))?;
    Ok(serde_json::from_str(&raw)?)
}
