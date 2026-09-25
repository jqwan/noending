//! Session ↔ WorkspacePath, and the derived Project cache.
//!
//! The fact chain is
//!
//! ```text
//! Session.cwd → sessions.workspace_path_id → workspace_paths.project_id → Project
//! ```
//!
//! `sessions.project_id` survives as a **derived cache** so the Project pages do
//! not have to re-join on every read, and 方案 §29 forbids it ever being an
//! independent fact. That prohibition is enforced by making the write space
//! small and closed — exactly three call sites, and each one computes the value
//! from the path in the same statement instead of taking a Project from the
//! caller:
//!
//! 1. `Db::upsert_session` (in `storage/mod.rs`) — discovery / launch matching,
//!    called by `ingestion::ensure_session_row`;
//! 2. [`attach_session_workspace_path_conn`] — an explicit re-attach, called when
//!    discovery sees the cwd move (`workspace::session`);
//! 3. [`refresh_sessions_project_for_path_conn`] — a WorkspacePath changed Project,
//!    so every Session behind it must follow in the same transaction
//!    (`storage::workspace::reassign_workspace_path_project_conn`).
//!
//! Plus one referential-cleanup case: [`clear_sessions_project_for_project_conn`]
//! invalidates the cache for a Project that is going away. The workspace layer's
//! zero-path auto-delete funnels through it, so the cleanup stays one statement
//! in one file. Anything else is a domain violation, guarded by
//! 方案 §42.5-T2 (and executably by
//! `tests/session_workspace_test::project_id_writers_are_confined_to_the_derived_doors`).
//!
//! Nothing here reads or writes `owner_workstream_id`: physical Project
//! membership and semantic Workstream ownership are independent (方案 §3.1,
//! §4, §5.2).

use rusqlite::{params, Connection};

use crate::error::Result;

use super::Db;

impl Db {
    /// §24 — standalone Sessions belong to a Project too, straight from their
    /// own path, with no Workstream involved. Active sessions only: the
    /// Project detail is a default projection and hides the recycle bin
    /// (方案 §11).
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

/// §1.11 — point a Session at a WorkspacePath and recompute its cached Project
/// in the same statement. Passing a Project is not an option here by design.
pub fn attach_session_workspace_path_conn(
    conn: &Connection,
    session_id: &str,
    workspace_path_id: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE sessions
            SET workspace_path_id = ?2,
                project_id = (SELECT project_id FROM workspace_paths WHERE id = ?2)
          WHERE id = ?1",
        params![session_id, workspace_path_id],
    )?;
    Ok(())
}

/// §1.11 — the ONLY reason a Session's cached Project changes without the Session
/// moving: the path it hangs off changed owner (Git upgrade, Project merge, new
/// Git family). Batch, so the cache cannot be left half-updated if one row
/// fails.
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
/// This is invalidation, not assignment — it writes `NULL` and nothing else, so
/// it cannot mint a membership the path chain does not imply. It lives here so
/// the question "who may write `sessions.project_id`?" has one answer per file:
/// `grep 'UPDATE sessions SET project_id'` finds this module and the
/// authoritative project-cache derivation statements in `storage/mod.rs`,
/// nowhere else.
pub fn clear_sessions_project_for_project_conn(
    conn: &Connection,
    project_id: &str,
) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE sessions SET project_id = NULL WHERE project_id = ?1",
        params![project_id],
    )?)
}
