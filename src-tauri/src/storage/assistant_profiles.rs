//! Persistent role identity and explicit management scope. Project ownership
//! remains the projection of the profile's registered WorkspacePath.
use crate::{
    assistant::AssistantProfile,
    domain::*,
    error::{other, Result},
};
use rusqlite::{params, Connection, OptionalExtension, Row};

fn row(r: &Row<'_>) -> rusqlite::Result<AssistantProfile> {
    let agent: String = r.get("agent")?;
    let runtime: Option<String> = r.get("runtime_override")?;
    let invalid = |text: String| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(other(text)),
        )
    };
    Ok(AssistantProfile {
        id: r.get("id")?,
        name: r.get("name")?,
        avatar: r.get("avatar")?,
        description: r.get("description")?,
        instructions: r.get("instructions")?,
        agent: Agent::parse(&agent).ok_or_else(|| invalid("未知运行工具".into()))?,
        runtime_override: runtime
            .map(|v| serde_json::from_str(&v).map_err(|e| invalid(e.to_string())))
            .transpose()?,
        workspace_path_id: r.get("workspace_path_id")?,
        visibility: r.get("visibility")?,
        revision: r.get("revision")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
    })
}
pub fn get(conn: &Connection, id: &str) -> Result<AssistantProfile> {
    conn.query_row("SELECT * FROM assistant_profiles WHERE id=?1", [id], row)
        .optional()?
        .ok_or_else(|| other("智能体档案不存在"))
}
pub fn save(conn: &Connection, p: &AssistantProfile, expected: Option<i64>) -> Result<()> {
    if let Some(expected) = expected {
        let changed = conn.execute("UPDATE assistant_profiles SET name=?2,avatar=?3,description=?4,instructions=?5,agent=?6,runtime_override=?7,visibility=?8,revision=?9,updated_at=?10 WHERE id=?1 AND revision=?11",
            params![p.id,p.name,p.avatar,p.description,p.instructions,p.agent.as_str(),p.runtime_override.as_ref().map(serde_json::to_string).transpose()?,p.visibility,p.revision,p.updated_at,expected])?;
        if changed != 1 {
            return Err(other("智能体档案版本冲突，请重新读取"));
        }
    } else {
        conn.execute("INSERT INTO assistant_profiles(id,name,avatar,description,instructions,agent,runtime_override,workspace_path_id,visibility,revision,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![p.id,p.name,p.avatar,p.description,p.instructions,p.agent.as_str(),p.runtime_override.as_ref().map(serde_json::to_string).transpose()?,p.workspace_path_id,p.visibility,p.revision,p.created_at,p.updated_at])?;
    }
    Ok(())
}
pub fn check_revision(p: &AssistantProfile, revision: i64) -> Result<()> {
    if p.revision != revision {
        return Err(other("智能体档案版本冲突，请重新读取"));
    }
    Ok(())
}
pub fn profile_for_session(conn: &Connection, session: &str) -> Result<Option<AssistantProfile>> {
    Ok(conn.query_row("SELECT p.* FROM assistant_profiles p JOIN assistant_sessions a ON a.assistant_id=p.id WHERE a.session_id=?1", [session], row).optional()?)
}
pub fn profile_for_path(conn: &Connection, path: &str) -> Result<Option<AssistantProfile>> {
    Ok(conn
        .query_row(
            "SELECT * FROM assistant_profiles WHERE workspace_path_id=?1",
            [path],
            row,
        )
        .optional()?)
}
/// Exact registered main-directory membership only; no prefix, title or age inference.
/// Called in the ingestion transaction, after the Session has a real path.
pub fn bind_session(conn: &Connection, session: &str) -> Result<()> {
    conn.execute("INSERT OR IGNORE INTO assistant_sessions(session_id,assistant_id) SELECT s.id,p.id FROM sessions s JOIN assistant_profiles p ON p.workspace_path_id=s.workspace_path_id WHERE s.id=?1", [session])?;
    Ok(())
}
pub fn management_eligible(conn: &Connection, session: &str, task: &str) -> Result<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM assistant_sessions s JOIN assistant_profiles p ON p.id=s.assistant_id JOIN assistant_tasks t ON t.assistant_id=p.id WHERE s.session_id=?1 AND t.task_id=?2 AND p.visibility='normal')",params![session,task],|r|r.get(0))?)
}
pub fn management_path_eligible(conn: &Connection, path: &str, task: &str) -> Result<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM assistant_profiles p JOIN assistant_tasks t ON t.assistant_id=p.id WHERE p.workspace_path_id=?1 AND t.task_id=?2 AND p.visibility='normal')", params![path,task],|r|r.get(0))?)
}
pub fn project_ids(conn: &Connection, id: &str) -> Result<Vec<String>> {
    ids(
        conn,
        "SELECT project_id FROM assistant_projects WHERE assistant_id=?1 ORDER BY project_id",
        id,
    )
}
pub fn task_ids(conn: &Connection, id: &str) -> Result<Vec<String>> {
    ids(
        conn,
        "SELECT task_id FROM assistant_tasks WHERE assistant_id=?1 ORDER BY task_id",
        id,
    )
}
fn ids(conn: &Connection, sql: &str, id: &str) -> Result<Vec<String>> {
    let mut st = conn.prepare(sql)?;
    let rows = st.query_map([id], |r| r.get(0))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}
pub fn counts(conn: &Connection, id: &str) -> Result<(i64, Option<String>)> {
    Ok(conn.query_row("SELECT COUNT(*),MAX(COALESCE(s.last_conversation_at,s.last_activity_at,s.started_at)) FROM sessions s JOIN assistant_sessions a ON a.session_id=s.id WHERE a.assistant_id=?1 AND s.archived_at IS NULL",[id],|r|Ok((r.get(0)?,r.get(1)?)))?)
}
pub fn set_responsibilities(
    conn: &Connection,
    id: &str,
    projects: &[String],
    tasks: &[String],
) -> Result<()> {
    let previous_projects = project_ids(conn, id)?;
    let previous_tasks = task_ids(conn, id)?;
    for pid in projects {
        let project =
            super::workspace::get_project_conn(conn, pid)?.ok_or_else(|| other("项目不存在"))?;
        if project.visibility != "normal" && !previous_projects.contains(pid) {
            return Err(other("不能新增负责已归档项目"));
        }
    }
    for tid in tasks {
        let task = super::get_workstream_conn(conn, tid)?.ok_or_else(|| other("任务不存在"))?;
        if task.visibility != "normal" && !previous_tasks.contains(tid) {
            return Err(other("不能新增负责已归档任务"));
        }
    }
    for tid in previous_tasks.iter().filter(|t| !tasks.contains(t)) {
        let active: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM collaboration_rooms room JOIN collaboration_runs run ON run.room_id=room.id JOIN collaboration_run_members member ON member.run_id=run.id JOIN assistant_sessions a ON a.session_id=member.session_id WHERE room.task_id=?2 AND a.assistant_id=?1 AND run.status IN ('starting','active'))",params![id,tid],|r|r.get(0))?;
        if active {
            return Err(other("智能体管理会话正在协作，不能解除任务责任"));
        }
    }
    conn.execute("DELETE FROM assistant_projects WHERE assistant_id=?1", [id])?;
    conn.execute("DELETE FROM assistant_tasks WHERE assistant_id=?1", [id])?;
    for pid in projects {
        conn.execute(
            "INSERT OR IGNORE INTO assistant_projects(assistant_id,project_id) VALUES(?1,?2)",
            params![id, pid],
        )?;
    }
    for tid in tasks {
        conn.execute(
            "INSERT OR IGNORE INTO assistant_tasks(assistant_id,task_id) VALUES(?1,?2)",
            params![id, tid],
        )?;
    }
    Ok(())
}

pub fn session_page(
    conn: &Connection,
    id: &str,
    scope: &str,
    after: Option<(&str, &str)>,
    limit: usize,
) -> Result<Vec<SessionListItem>> {
    let (stamp, key) = after
        .map(|(a, b)| (Some(a), Some(b)))
        .unwrap_or((None, None));
    let sql=format!("{} JOIN assistant_sessions a ON a.session_id=sessions.id WHERE a.assistant_id=?1 AND (?2='all' OR (?2='unarchived' AND archived_at IS NULL) OR (?2='archived' AND archived_at IS NOT NULL)) AND (?3 IS NULL OR COALESCE(last_conversation_at,last_activity_at,started_at,'')<?3 OR (COALESCE(last_conversation_at,last_activity_at,started_at,'')=?3 AND sessions.id>?4)) ORDER BY COALESCE(last_conversation_at,last_activity_at,started_at,'') DESC,sessions.id LIMIT ?5",super::SESSION_SELECT);
    let mut st = conn.prepare(&sql)?;
    let rows = st.query_map(
        params![id, scope, stamp, key, limit as i64],
        super::session_queries::row_list_item,
    )?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Existing ordinary task members retain their historical eligibility. A
/// cross-project management member additionally needs its current assignment.
pub fn member_predicate(session_alias: &str, task_parameter: &str) -> String {
    format!("(NOT EXISTS(SELECT 1 FROM assistant_sessions a WHERE a.session_id={session_alias}.id) OR EXISTS(SELECT 1 FROM workstream_projects p JOIN workspace_paths wp ON wp.project_id=p.project_id WHERE p.workstream_id={task_parameter} AND wp.id={session_alias}.workspace_path_id) OR EXISTS(SELECT 1 FROM assistant_sessions a JOIN assistant_profiles p ON p.id=a.assistant_id JOIN assistant_tasks t ON t.assistant_id=p.id WHERE a.session_id={session_alias}.id AND p.visibility='normal' AND t.task_id={task_parameter}))")
}
pub fn management_member_eligible(conn: &Connection, session: &str, task: &str) -> Result<bool> {
    Ok(conn.query_row(
        &format!(
            "SELECT EXISTS(SELECT 1 FROM sessions s WHERE s.id=?1 AND {})",
            member_predicate("s", "?2")
        ),
        params![session, task],
        |r| r.get(0),
    )?)
}

pub fn card_page(
    conn: &Connection,
    scope: &str,
    query: &str,
    after: Option<(&str, &str)>,
    limit: usize,
) -> Result<Vec<(String, String, crate::assistant::AssistantCard)>> {
    let (stamp, key) = after
        .map(|(a, b)| (Some(a), Some(b)))
        .unwrap_or((None, None));
    let sql="SELECT p.*,wp.canonical_path AS directory,wp.project_id,
      (SELECT COUNT(*) FROM assistant_sessions a JOIN sessions s ON s.id=a.session_id WHERE a.assistant_id=p.id AND s.archived_at IS NULL) AS session_count,
      (SELECT COUNT(*) FROM assistant_projects a WHERE a.assistant_id=p.id) AS project_count,
      (SELECT COUNT(*) FROM assistant_tasks a WHERE a.assistant_id=p.id) AS task_count,
      (SELECT MAX(COALESCE(s.last_conversation_at,s.last_activity_at,s.started_at)) FROM assistant_sessions a JOIN sessions s ON s.id=a.session_id WHERE a.assistant_id=p.id AND s.archived_at IS NULL) AS activity
      FROM assistant_profiles p JOIN workspace_paths wp ON wp.id=p.workspace_path_id
      WHERE (?1='all' OR (?1='unarchived' AND p.visibility='normal') OR (?1='archived' AND p.visibility='archived'))
      AND instr(lower(p.name || ' ' || p.description),lower(?2))>0
      AND (?3 IS NULL OR COALESCE(activity,p.updated_at)<?3 OR (COALESCE(activity,p.updated_at)=?3 AND p.id>?4))
      ORDER BY COALESCE(activity,p.updated_at) DESC,p.id LIMIT ?5";
    let mut st = conn.prepare(sql)?;
    let rows = st.query_map(
        params![scope, query.trim(), stamp, key, limit as i64],
        |r| {
            let p = row(r)?;
            let directory: String = r.get("directory")?;
            let activity: Option<String> = r.get("activity")?;
            let sort = activity.clone().unwrap_or_else(|| p.updated_at.clone());
            let card = crate::assistant::AssistantCard {
                id: p.id.clone(),
                name: p.name,
                avatar: p.avatar,
                description: p.description,
                agent: p.agent,
                runtime_override: p.runtime_override,
                visibility: p.visibility,
                revision: p.revision,
                directory_exists: std::path::Path::new(&directory).is_dir(),
                directory,
                project_id: r.get("project_id")?,
                session_count: r.get("session_count")?,
                project_count: r.get("project_count")?,
                task_count: r.get("task_count")?,
                last_activity_at: activity,
                created_at: p.created_at,
                updated_at: p.updated_at,
            };
            Ok((sort, p.id, card))
        },
    )?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}
