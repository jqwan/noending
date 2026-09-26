//! Session ↔ WorkspacePath, and the derived Project cache.
//!
//! Fact chain: `Session.cwd → sessions.workspace_path_id →
//! workspace_paths.project_id → Project`. `sessions.project_id` is only a
//! **derived cache** of that chain, never an independent fact — so the write
//! space is small and closed: three call sites that each compute the value from
//! the path in the same statement (discovery's session upsert,
//! [`attach_session_workspace_path_conn`],
//! [`refresh_sessions_project_for_path_conn`]), plus
//! [`clear_sessions_project_for_project_conn`] for a Project that is going away.
//! Anything else is a domain violation.
//!
//! Nothing here touches `owner_workstream_id`: physical Project membership and
//! semantic Workstream ownership are independent.

use rusqlite::{params, Connection};

use crate::error::Result;

use super::Db;

impl Db {
    /// Standalone Sessions belong to a Project too, straight from their own path,
    /// with no Workstream involved. Active only: the Project detail is a default
    /// projection and hides the recycle bin.
    pub fn list_sessions_for_workspace_path(
        &self,
        path_id: &str,
    ) -> Result<Vec<crate::domain::Session>> {
        let conn = self.read();
        let mut st = conn.prepare(
            "SELECT * FROM sessions WHERE workspace_path_id = ?1 AND trashed_at IS NULL
              ORDER BY COALESCE(last_activity_at, started_at) DESC",
        )?;
        let mapped = st.query_map(params![path_id], super::row_session)?;
        Ok(mapped.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}

/// Point a Session at a WorkspacePath and recompute its cached Project in the
/// same statement — passing a Project is deliberately not an option.
pub fn attach_session_workspace_path_conn(
    conn: &Connection,
    session_id: &str,
    workspace_path_id: Option<&str>,
) -> Result<bool> {
    Ok(conn.execute(
        "UPDATE sessions
            SET workspace_path_id = ?2,
                project_id = (SELECT project_id FROM workspace_paths WHERE id = ?2)
          WHERE id = ?1 AND trashed_at IS NULL",
        params![session_id, workspace_path_id],
    )? == 1)
}

/// The ONLY reason a Session's cached Project changes without the Session moving:
/// the path it hangs off changed owner (Git upgrade, Project merge, new Git
/// family). Batch, so the cache cannot be left half-updated.
pub fn refresh_sessions_project_for_path_conn(conn: &Connection, path_id: &str) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE sessions
            SET project_id = (SELECT project_id FROM workspace_paths WHERE id = ?1)
          WHERE workspace_path_id = ?1",
        params![path_id],
    )?)
}

/// A Project is going away: every Session that was only *projecting* onto it
/// through the derived cache must stop doing so.
///
/// Invalidation, not assignment — it writes `NULL` and nothing else, so it cannot
/// mint a membership the path chain does not imply.
pub fn clear_sessions_project_for_project_conn(
    conn: &Connection,
    project_id: &str,
) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE sessions SET project_id = NULL WHERE project_id = ?1",
        params![project_id],
    )?)
}
