//! Current project summary and consumed inputs; no historical summary copies.
use crate::error::Result;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;

#[derive(Default)]
pub struct ProjectContextState {
    pub revision: i64,
    pub input_fingerprint: String,
    pub summary: String,
    pub updated_at: Option<String>,
}
pub fn state(conn: &Connection, id: &str) -> Result<ProjectContextState> {
    Ok(conn.query_row("SELECT context_revision,input_fingerprint,summary,updated_at FROM project_context_state WHERE project_id=?1", [id], |r| Ok(ProjectContextState { revision:r.get(0)?,input_fingerprint:r.get(1)?,summary:r.get(2)?,updated_at:r.get(3)? })).optional()?.unwrap_or_default())
}
pub fn frontiers(conn: &Connection, id: &str) -> Result<HashMap<String, i64>> {
    let mut st=conn.prepare("SELECT session_id,session_context_revision FROM project_session_frontiers WHERE project_id=?1")?;
    let rows = st
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    Ok(rows)
}
pub fn commit(
    conn: &Connection,
    id: &str,
    fingerprint: &str,
    summary: &str,
    consumed: &[(String, i64)],
) -> Result<i64> {
    conn.execute("INSERT INTO project_context_state(project_id,context_revision,input_fingerprint,summary,updated_at) VALUES (?1,1,?2,?3,?4) ON CONFLICT(project_id) DO UPDATE SET context_revision=context_revision+1,input_fingerprint=?2,summary=?3,updated_at=?4",params![id,fingerprint,summary,super::now()])?;
    for (sid, revision) in consumed {
        conn.execute("INSERT INTO project_session_frontiers(project_id,session_id,session_context_revision) VALUES (?1,?2,?3) ON CONFLICT(project_id,session_id) DO UPDATE SET session_context_revision=?3", params![id,sid,revision])?;
    }
    conn.execute("DELETE FROM project_session_frontiers WHERE project_id=?1 AND NOT EXISTS(SELECT 1 FROM sessions s JOIN workspace_paths wp ON wp.id=s.workspace_path_id WHERE s.id=project_session_frontiers.session_id AND wp.project_id=?1)", [id])?;
    reindex(conn, id)?;
    Ok(state(conn, id)?.revision)
}
/// A rename or registry refresh must retain the current summary in search too.
pub fn reindex(conn: &Connection, id: &str) -> Result<()> {
    super::unindex_conn(conn, "project", id);
    conn.execute("INSERT INTO search_index(kind,ref_id,parent_id,title,body) SELECT 'project',p.id,'',p.name,COALESCE(c.summary,'') FROM projects p LEFT JOIN project_context_state c ON c.project_id=p.id WHERE p.id=?1", [id])?;
    Ok(())
}
