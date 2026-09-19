//! Session ↔ WorkspacePath, and the derived Project cache (schema v12).
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
//! 1. `Db::upsert_session` (in `storage/mod.rs`) — discovery / launch matching;
//! 2. [`attach_session_workspace_path_conn`] — an explicit re-attach;
//! 3. [`refresh_sessions_project_for_path_conn`] — a WorkspacePath changed Project,
//!    so every Session behind it must follow in the same transaction.
//!
//! Plus one referential-cleanup case: `Db::delete_project_and_children` clearing
//! `project_id` for a Project that is going away. Anything else — notably the
//! retired `assign_session_project`, which was a raw
//! `UPDATE sessions SET project_id = ?` — is a domain violation, guarded by
//! 方案 §42.5-T2.

use rusqlite::{params, Connection};

use crate::error::Result;

use super::Db;

impl Db {
    /// §24 — standalone Sessions belong to a Project too, straight from their
    /// own path, with no Workstream involved.
    pub fn list_sessions_for_workspace_path(
        &self,
        path_id: &str,
    ) -> Result<Vec<crate::domain::Session>> {
        let mut st = self.0.prepare(
            "SELECT * FROM sessions WHERE workspace_path_id = ?1
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

/// §5.6 — record which WorkstreamPath brought a binding in.
///
/// `None` is a real state, not a reset: it means "legacy, or the Session's cwd
/// has since drifted elsewhere", and such a binding is deliberately out of reach
/// of a WorkstreamPath deletion (§42.3-M2). Callers must therefore only pass
/// `None` when they have actually looked and found no match.
pub fn set_binding_workstream_path_conn(
    conn: &Connection,
    session_id: &str,
    workstream_id: &str,
    workstream_path_id: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE session_workstream_bindings
            SET workstream_path_id = ?3
          WHERE session_id = ?1 AND workstream_id = ?2",
        params![session_id, workstream_id, workstream_path_id],
    )?;
    Ok(())
}

/// §42.3-M2 — after a Session's cwd moves, re-point each of its bindings at the
/// WorkstreamPath that now matches, or drop the path claim when none does.
/// Two statements rather than one clever predicate: "point at it" and "no longer
/// provably from it" are different decisions and read differently.
///
/// Never appends a path to a Workstream — the path list only grows from a user
/// action or an explicit bind.
pub fn reconcile_binding_paths_conn(conn: &Connection, session_id: &str) -> Result<usize> {
    let matched = conn.execute(
        "UPDATE session_workstream_bindings
            SET workstream_path_id = (
              SELECT wsp.id FROM workstream_paths wsp
                JOIN sessions s ON s.id = session_workstream_bindings.session_id
               WHERE wsp.workstream_id = session_workstream_bindings.workstream_id
                 AND wsp.workspace_path_id = s.workspace_path_id)
          WHERE session_id = ?1
            AND EXISTS (
              SELECT 1 FROM workstream_paths wsp
                JOIN sessions s ON s.id = session_workstream_bindings.session_id
               WHERE wsp.workstream_id = session_workstream_bindings.workstream_id
                 AND wsp.workspace_path_id = s.workspace_path_id)",
        params![session_id],
    )?;
    let cleared = conn.execute(
        "UPDATE session_workstream_bindings
            SET workstream_path_id = NULL
          WHERE session_id = ?1
            AND workstream_path_id IS NOT NULL
            AND NOT EXISTS (
              SELECT 1 FROM workstream_paths wsp
                JOIN sessions s ON s.id = session_workstream_bindings.session_id
               WHERE wsp.workstream_id = session_workstream_bindings.workstream_id
                 AND wsp.workspace_path_id = s.workspace_path_id)",
        params![session_id],
    )?;
    Ok(matched + cleared)
}
