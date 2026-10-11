//! Explicit task/project membership and session tracking. Directories belong to
//! projects; task associations never expand or follow a directory snapshot.
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
pub fn reindex_workstream_search_conn(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM search_index WHERE kind='workstream' AND ref_id=?1",
        [id],
    )?;
    conn.execute("INSERT INTO search_index(kind,ref_id,parent_id,title,body) SELECT 'workstream',w.id,'',w.title,w.description || char(10) || COALESCE(c.summary,'') FROM workstreams w LEFT JOIN workstream_context_state c ON c.workstream_id=w.id WHERE w.id=?1",[id])?;
    Ok(())
}
pub fn purge_workstream_data_conn(conn: &Connection, id: &str) -> Result<()> {
    let sessions = tracked_session_ids(conn, id)?;
    conn.execute("DELETE FROM workstreams WHERE id=?1", [id])?;
    conn.execute(
        "DELETE FROM search_index WHERE kind='workstream' AND ref_id=?1",
        [id],
    )?;
    for sid in sessions {
        super::index_session_conn(conn, &sid)?;
    }
    Ok(())
}
pub fn tracked_session_ids(conn: &Connection, id: &str) -> Result<Vec<String>> {
    let mut st = conn.prepare(
        "SELECT session_id FROM session_workstreams WHERE workstream_id=?1 ORDER BY session_id",
    )?;
    let rows = st.query_map([id], |r| r.get(0))?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}
pub fn set_tracking_conn(conn: &Connection, sid: &str, tid: &str, tracked: bool) -> Result<()> {
    let session = super::get_session_conn(conn, sid)?.ok_or_else(|| other("会话不存在"))?;
    let task = super::get_workstream_conn(conn, tid)?.ok_or_else(|| other("任务不存在"))?;
    let changed = if tracked {
        let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM session_workstreams WHERE session_id=?1 AND workstream_id=?2)",params![sid,tid],|r|r.get(0))?;
        if exists {
            return Ok(());
        }
        if task.visibility != "normal" {
            return Err(other("不能新增跟踪已归档任务"));
        }
        let eligible:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM workstream_projects WHERE workstream_id=?1 AND project_id=?2)",params![tid,session.project_id],|r|r.get(0))?;
        if !eligible {
            return Err(other("任务未关联会话所属项目"));
        }
        conn.execute(
            "INSERT INTO session_workstreams(session_id,workstream_id) VALUES (?1,?2)",
            params![sid, tid],
        )?
    } else {
        conn.execute(
            "DELETE FROM session_workstreams WHERE session_id=?1 AND workstream_id=?2",
            params![sid, tid],
        )?
    };
    if changed > 0 {
        super::bump_input_revision_conn(conn, tid)?;
        super::index_session_conn(conn, sid)?;
    }
    Ok(())
}
