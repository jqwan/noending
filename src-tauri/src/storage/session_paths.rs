//! Session ↔ WorkspacePath. Project membership is derived at read time.
//! Physical membership and semantic Workstream ownership are independent.

use rusqlite::{params, Connection};

use crate::error::Result;

use super::Db;

impl Db {
    /// Standalone Sessions belong to a Project too, straight from their own path,
    /// with no Workstream involved. Archive state does not change membership.
    pub fn list_sessions_for_workspace_path(
        &self,
        path_id: &str,
    ) -> Result<Vec<crate::domain::Session>> {
        let conn = self.read();
        let mut st = conn.prepare(&format!(
            "{} WHERE workspace_path_id = ?1
              ORDER BY COALESCE(last_activity_at, started_at) DESC",
            super::SESSION_SELECT
        ))?;
        let mapped = st.query_map(params![path_id], super::row_session)?;
        Ok(mapped.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}

/// Point a Session at a WorkspacePath. Its Project follows the path at read time.
pub fn attach_session_workspace_path_conn(
    conn: &Connection,
    session_id: &str,
    workspace_path_id: Option<&str>,
) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE sessions SET workspace_path_id = ?2 WHERE id = ?1",
        params![session_id, workspace_path_id],
    )? == 1;
    if changed {
        super::index_session_conn(conn, session_id)?;
    }
    Ok(changed)
}
