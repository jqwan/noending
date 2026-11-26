//! List-specific projections: select only the fields displayed or filtered by
//! the UI. Full entities remain available through get_session for detail/runtime.
use super::{Db, SessionFilter};
use crate::domain::{Agent, SessionListItem, SessionListScope};
use crate::error::Result;
use rusqlite::{Connection, Row};
use std::collections::HashMap;

const LIST_SELECT: &str = "SELECT sessions.id,agent,root_agent_session_id,title,cwd,
    workspace_path_id,started_at,last_activity_at,last_conversation_at,archived_at,
    source_kind,latest_message_seq,
    (SELECT m.project_id FROM project_session_membership m WHERE m.session_id=sessions.id) AS project_id
    FROM sessions";

pub(super) fn query_sessions<T>(
    conn: &Connection,
    select: &str,
    filter: &SessionFilter,
    limit: Option<usize>,
    row: fn(&Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>> {
    let mut sql = format!("{select} WHERE 1=1");
    let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(project) = &filter.project_id {
        values.push(Box::new(project.clone()));
        sql.push_str(&format!(
            " AND sessions.id IN (SELECT session_id FROM project_session_membership WHERE project_id=?{})",
            values.len()
        ));
    }
    if let Some(agent) = filter.agent {
        values.push(Box::new(agent.as_str().to_string()));
        sql.push_str(&format!(" AND agent=?{}", values.len()));
    }
    match filter.scope {
        SessionListScope::Unarchived => sql.push_str(" AND archived_at IS NULL"),
        SessionListScope::Archived => sql.push_str(" AND archived_at IS NOT NULL"),
        SessionListScope::All => {}
    }
    sql.push_str(" ORDER BY COALESCE(last_conversation_at,last_activity_at,started_at) DESC,id");
    // A caller may request a bounded preview. Browsing/filtering consumers
    // receive all lightweight rows, never a silently truncated result.
    if let Some(limit) = limit {
        values.push(Box::new(limit as i64));
        sql.push_str(&format!(" LIMIT ?{}", values.len()));
    }
    let mut statement = conn.prepare(&sql)?;
    let refs: Vec<&dyn rusqlite::types::ToSql> = values.iter().map(|v| v.as_ref()).collect();
    let rows = statement.query_map(refs.as_slice(), row)?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

impl Db {
    pub(crate) fn project_session_items(&self, project_id: &str) -> Result<Vec<SessionListItem>> {
        let conn = self.read();
        let mut statement = conn.prepare(&format!(
            "{LIST_SELECT} WHERE sessions.id IN (SELECT session_id FROM project_session_membership WHERE project_id=?1)
             ORDER BY COALESCE(last_conversation_at,last_activity_at,started_at) DESC,id"
        ))?;
        let rows = statement.query_map([project_id], row_list_item)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn list_session_items(
        &self,
        filter: SessionFilter,
        limit: Option<usize>,
    ) -> Result<Vec<SessionListItem>> {
        query_sessions(&self.read(), LIST_SELECT, &filter, limit, row_list_item)
    }

    /// Counts used by Agent cards. No session bodies/metadata/associations are
    /// loaded, and counts have no preview limit.
    pub fn session_counts_by_agent(&self) -> Result<HashMap<String, i64>> {
        let conn = self.read();
        let mut statement = conn.prepare(
            "SELECT agent,COUNT(*) FROM sessions WHERE archived_at IS NULL GROUP BY agent",
        )?;
        let rows = statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<HashMap<_, _>, _>>()?)
    }

    /// Latest session activity timestamp per agent for sorting in the Agent board.
    pub fn agent_latest_activities(&self) -> Result<HashMap<String, String>> {
        let conn = self.read();
        let mut statement = conn.prepare(
            "SELECT agent, MAX(COALESCE(last_conversation_at, last_activity_at, started_at))
             FROM sessions
             WHERE archived_at IS NULL
             GROUP BY agent",
        )?;
        let rows = statement.query_map([], |r| {
            let agent: String = r.get(0)?;
            let max_time: Option<String> = r.get(1)?;
            Ok((agent, max_time))
        })?;
        let mut map = HashMap::new();
        for row in rows {
            let (agent, max_time) = row?;
            if let Some(t) = max_time {
                map.insert(agent, t);
            }
        }
        Ok(map)
    }
}

pub(super) fn row_list_item(r: &Row<'_>) -> rusqlite::Result<SessionListItem> {
    Ok(SessionListItem {
        id: r.get("id")?,
        agent: Agent::parse(&r.get::<_, String>("agent")?).unwrap_or(Agent::Codex),
        root_agent_session_id: r.get("root_agent_session_id")?,
        title: r.get("title")?,
        cwd: r.get("cwd")?,
        workspace_path_id: r.get("workspace_path_id")?,
        project_id: r.get("project_id")?,
        started_at: r.get("started_at")?,
        last_activity_at: r.get("last_activity_at")?,
        last_conversation_at: r.get("last_conversation_at")?,
        archived_at: r.get("archived_at")?,
        source_kind: r.get("source_kind")?,
        latest_message_seq: r.get("latest_message_seq")?,
    })
}
