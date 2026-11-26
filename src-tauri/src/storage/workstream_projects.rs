//! Project membership for tasks.
use crate::{
    domain::*,
    error::{other, Result},
};
use rusqlite::{params, Connection};

pub fn project_ids_for_workstream(conn: &Connection, id: &str) -> Result<Vec<String>> {
    let mut st = conn.prepare(
        "SELECT project_id FROM workstream_projects WHERE workstream_id=?1 ORDER BY project_id",
    )?;
    let rows = st.query_map([id], |r| r.get(0))?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}
pub fn set_projects_conn(conn: &Connection, id: &str, projects: &[String]) -> Result<()> {
    if super::get_workstream_conn(conn, id)?.is_none() {
        return Err(other("任务不存在"));
    }
    for project in projects {
        if super::workspace::get_project_conn(conn, project)?.is_none() {
            return Err(other("项目不存在"));
        }
    }
    let mut target = projects.to_vec();
    target.sort();
    target.dedup();
    if project_ids_for_workstream(conn, id)? == target {
        return Ok(());
    }
    conn.execute(
        "DELETE FROM workstream_projects WHERE workstream_id=?1",
        [id],
    )?;
    for project in projects {
        conn.execute(
            "INSERT OR IGNORE INTO workstream_projects(workstream_id,project_id) VALUES (?1,?2)",
            params![id, project],
        )?;
    }
    super::bump_input_revision_conn(conn, id)?;
    Ok(())
}
pub fn workstreams_for_project(conn: &Connection, id: &str) -> Result<Vec<Workstream>> {
    let mut st=conn.prepare("SELECT w.* FROM workstreams w JOIN workstream_projects p ON p.workstream_id=w.id WHERE p.project_id=?1 ORDER BY w.updated_at DESC,w.id")?;
    let rows = st.query_map([id], super::row_workstream)?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub fn purge_workstream_data_conn(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM workstreams WHERE id=?1", [id])?;
    Ok(())
}
