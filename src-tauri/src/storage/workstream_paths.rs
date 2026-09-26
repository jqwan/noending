//! WorkstreamPath persistence: the ordered working-path list.
//!
//! The ordering IS the role — position 0 is the primary path, so there is no
//! `is_primary` column to drift out of agreement with the list. The
//! invariant these helpers must keep at all times:
//!
//! ```text
//! paths.is_empty()  OR  a row at position 0 exists
//! ```
//!
//! which is only true if every removal recompacts. `remove_workstream_path_conn`
//! is therefore the only legal delete, and it recompacts in the same statement.
//!
//! The UNIQUE constraints do the rest of the enforcing at the storage level:
//! `(workstream_id, workspace_path_id)` stops duplicates, `(workstream_id,
//! position)` stops two paths claiming the same slot.
//!
//! Policy that calls these — add/remove/reorder endpoints, the recycle bin —
//! is `workspace::workstream`. Two of them
//! live here rather than there because they are mechanical and total:
//! [`purge_workstream_data_conn`] (the full delete order, in the
//! caller's transaction) and [`reindex_workstream_search_conn`] (one Workstream's
//! search row, re-derived from position 0).
//!
//! [`reorder_workstream_paths_conn`] is the one helper here a caller must not run
//! on a `&Connection` it cannot roll back: it stages through negative positions,
//! so a mid-way failure would leave the list half-renumbered. Always pass a
//! transaction.

use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};

use crate::domain::*;
use crate::error::{other, Result};

use super::{new_id, now, Db};

fn row_workstream_path(r: &Row) -> rusqlite::Result<WorkstreamPath> {
    Ok(WorkstreamPath {
        id: r.get("id")?,
        workstream_id: r.get("workstream_id")?,
        workspace_path_id: r.get("workspace_path_id")?,
        position: r.get("position")?,
        created_at: r.get("created_at")?,
    })
}

impl Db {
    /// The ordered list, always. Every caller that shows or reasons about "the
    /// Workstream's paths" reads it here, so ordering can not be forgotten at a
    /// call site.
    pub fn list_workstream_paths(&self, workstream_id: &str) -> Result<Vec<WorkstreamPath>> {
        let conn = self.read();
        let mut st = conn
            .prepare("SELECT * FROM workstream_paths WHERE workstream_id = ?1 ORDER BY position")?;
        let mapped = st.query_map(params![workstream_id], row_workstream_path)?;
        Ok(mapped.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// The launch directory for a New Session on this Workstream.
    pub fn primary_workspace_path_id(&self, workstream_id: &str) -> Result<Option<String>> {
        let conn = self.read();
        Ok(conn
            .query_row(
                "SELECT workspace_path_id FROM workstream_paths
                  WHERE workstream_id = ?1 AND position = 0",
                params![workstream_id],
                |r| r.get(0),
            )
            .optional()?)
    }
}

/// Full canonical row behind a Workstream's primary path, when it has one.
pub fn primary_workspace_path(
    conn: &Connection,
    workstream_id: &str,
) -> Result<Option<WorkspacePath>> {
    Ok(conn
        .query_row(
            "SELECT wp.* FROM workstream_paths wsp
               JOIN workspace_paths wp ON wp.id = wsp.workspace_path_id
              WHERE wsp.workstream_id = ?1 AND wsp.position = 0",
            params![workstream_id],
            |r| {
                Ok(WorkspacePath {
                    id: r.get("id")?,
                    canonical_path: r.get("canonical_path")?,
                    project_id: r.get("project_id")?,
                    git_state: r.get("git_state")?,
                    git_kind: r.get("git_kind")?,
                    exists: r.get::<_, i64>("exists_on_disk")? != 0,
                    first_seen_at: r.get("first_seen_at")?,
                    last_seen_at: r.get("last_seen_at")?,
                })
            },
        )
        .optional()?)
}

/// Append at the end (an empty list yields position 0, which is why
/// "become the primary path" needs no special case).
///
/// Idempotent by `(workstream_id, workspace_path_id)`: re-adding a path that is
/// already in the list returns the existing row rather than failing or shifting
/// positions under the caller.
pub fn append_workstream_path_conn(
    conn: &Connection,
    workstream_id: &str,
    workspace_path_id: &str,
) -> Result<WorkstreamPath> {
    if let Some(existing) = find_workstream_path_conn(conn, workstream_id, workspace_path_id)? {
        return Ok(existing);
    }
    let next: i64 = conn.query_row(
        "SELECT COALESCE(MAX(position) + 1, 0) FROM workstream_paths WHERE workstream_id = ?1",
        params![workstream_id],
        |r| r.get(0),
    )?;
    let row = WorkstreamPath {
        id: new_id(),
        workstream_id: workstream_id.to_string(),
        workspace_path_id: workspace_path_id.to_string(),
        position: next,
        created_at: now(),
    };
    conn.execute(
        "INSERT INTO workstream_paths
           (id, workstream_id, workspace_path_id, position, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            row.id,
            row.workstream_id,
            row.workspace_path_id,
            row.position,
            row.created_at
        ],
    )?;
    Ok(row)
}

pub fn find_workstream_path_conn(
    conn: &Connection,
    workstream_id: &str,
    workspace_path_id: &str,
) -> Result<Option<WorkstreamPath>> {
    Ok(conn
        .query_row(
            "SELECT * FROM workstream_paths WHERE workstream_id = ?1 AND workspace_path_id = ?2",
            params![workstream_id, workspace_path_id],
            row_workstream_path,
        )
        .optional()?)
}

/// Delete the WorkstreamPath and renumber the survivors, in the caller's
/// transaction. Deleting position 0 therefore promotes position 1 automatically:
/// the user never has to pick a new primary path.
///
/// This touches the path list only. It never changes a Session — not its cwd,
/// not its workspace path, and not its Owner Workstream. A
/// Session's cwd is historical execution fact; the Workstream's path list is
/// current configuration; the owner is semantic assignment. They are
/// independent.
pub fn remove_workstream_path_conn(
    tx: &Transaction<'_>,
    workstream_id: &str,
    workstream_path_id: &str,
) -> Result<()> {
    tx.execute(
        "DELETE FROM workstream_paths
          WHERE workstream_id = ?1 AND id = ?2",
        params![workstream_id, workstream_path_id],
    )?;
    recompact_workstream_positions_conn(tx, workstream_id)?;
    Ok(())
}

/// Renumber positions to `0..len` preserving the current order. Cheap and
/// total: it is also the repair path if an aborted sequence ever left a gap.
pub fn recompact_workstream_positions_conn(conn: &Connection, workstream_id: &str) -> Result<()> {
    let ids: Vec<String> = {
        let mut st = conn.prepare(
            "SELECT id FROM workstream_paths WHERE workstream_id = ?1 ORDER BY position, created_at",
        )?;
        let mapped = st.query_map(params![workstream_id], |r| r.get::<_, String>(0))?;
        mapped.collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (i, id) in ids.iter().enumerate() {
        conn.execute(
            "UPDATE workstream_paths SET position = ?2 WHERE id = ?1",
            params![id, i as i64],
        )?;
    }
    Ok(())
}

/// "Make this the primary path" is a reorder, not a role change.
///
/// Takes the COMPLETE ordered list of workspace path ids and rewrites positions
/// to match. Anything not listed is not silently appended: an incomplete list is
/// a bug in the caller, and quietly keeping the old tail would let a UI race drop
/// a user's path.
pub fn reorder_workstream_paths_conn(
    conn: &Connection,
    workstream_id: &str,
    ordered_workspace_path_ids: &[String],
) -> Result<()> {
    let current: Vec<String> = {
        let mut st = conn.prepare(
            "SELECT workspace_path_id FROM workstream_paths WHERE workstream_id = ?1 ORDER BY position",
        )?;
        let mapped = st.query_map(params![workstream_id], |r| r.get::<_, String>(0))?;
        mapped.collect::<std::result::Result<Vec<_>, _>>()?
    };
    if current.len() != ordered_workspace_path_ids.len()
        || !ordered_workspace_path_ids
            .iter()
            .all(|p| current.contains(p))
    {
        return Err(other(
            "reorder 必须给出完整且相同的工作路径集合，不会替你补上或保留路径",
        ));
    }
    // Two paths cannot hold the same position, so stage into negative slots
    // first and the final UNIQUE(workstream_id, position) is never violated
    // mid-way.
    for (i, path_id) in ordered_workspace_path_ids.iter().enumerate() {
        conn.execute(
            "UPDATE workstream_paths SET position = ?3
              WHERE workstream_id = ?1 AND workspace_path_id = ?2",
            params![workstream_id, path_id, -(i as i64) - 1],
        )?;
    }
    for (i, path_id) in ordered_workspace_path_ids.iter().enumerate() {
        conn.execute(
            "UPDATE workstream_paths SET position = ?3
              WHERE workstream_id = ?1 AND workspace_path_id = ?2",
            params![workstream_id, path_id, i as i64],
        )?;
    }
    Ok(())
}

/// One entry of the list by its own id, scoped to the Workstream that owns it.
///
/// The `workstream_id` predicate is not decoration: a `workstream_paths.id`
/// copied from another Workstream (a stale UI row, a race between two lists)
/// must resolve to `None` rather than mutate someone else's path list.
pub fn workstream_path_by_id_conn(
    conn: &Connection,
    workstream_id: &str,
    id: &str,
) -> Result<Option<WorkstreamPath>> {
    Ok(conn
        .query_row(
            "SELECT * FROM workstream_paths WHERE workstream_id = ?1 AND id = ?2",
            params![workstream_id, id],
            row_workstream_path,
        )
        .optional()?)
}

/// The list as canonical path strings, **in position order**.
///
/// Ordering is the contract: this is the byte sequence the PreparedLaunch
/// fingerprint hashes, so a reorder that moves position 0 changes the launch
/// plan and a sorted copy of the list can never make two different orders look
/// equal.
pub fn ordered_canonical_paths_for_workstream(
    conn: &Connection,
    workstream_id: &str,
) -> Result<Vec<String>> {
    let mut st = conn.prepare(
        "SELECT wp.canonical_path FROM workstream_paths wsp
           JOIN workspace_paths wp ON wp.id = wsp.workspace_path_id
          WHERE wsp.workstream_id = ?1 ORDER BY wsp.position",
    )?;
    let mapped = st.query_map(params![workstream_id], |r| r.get::<_, String>(0))?;
    Ok(mapped.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// Rewrite one Workstream's search row so its `parent_id` follows
/// the CURRENT position-0 path.
///
/// The generic [`super::workspace::refresh_workstream_search_parents_conn`]
/// finds affected Workstreams *through* `workstream_paths`, which cannot work
/// for a removal: by the time it runs, the row that connected the Workstream to
/// the path is gone. This one takes the Workstream id directly and is therefore
/// usable on both sides of a mutation.
pub fn reindex_workstream_search_conn(conn: &Connection, workstream_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM search_index WHERE kind = 'workstream' AND ref_id = ?1",
        params![workstream_id],
    )?;
    conn.execute(
        "INSERT INTO search_index (kind, ref_id, parent_id, title, body)
         SELECT 'workstream', w.id,
                (SELECT wp.project_id FROM workstream_paths wsp
                   JOIN workspace_paths wp ON wp.id = wsp.workspace_path_id
                  WHERE wsp.workstream_id = w.id AND wsp.position = 0),
                w.title, w.description
         FROM workstreams w WHERE w.id = ?1",
        params![workstream_id],
    )?;
    Ok(())
}

/// Delete every row a Workstream owns, in FK order, inside the
/// caller's transaction. A permanent delete is all-or-nothing: a half-purged
/// Workstream would keep Context rows whose owner no longer exists.
///
/// The search index is an FTS5 table with no FK. Its `kind='item'` rows
///   carry `parent_id = workstream_id`, so deleting the items without deleting
///   their index rows leaves search returning facts that no longer exist.
///
/// `workstream_review_state` cascades; it is deleted explicitly anyway so the
/// order is written down rather than inferred from the schema.
///
/// NOT touched, on purpose: `sessions` (their `owner_workstream_id` is cleared
/// by the FK in step 9, the rows themselves survive), `session_members`,
/// `session_messages`, `launch_intents`, `workspace_paths`, `projects`,
/// `sync_runs` and the Agents' raw source data. A Session survives the
/// Workstream that referenced it; `launch_intents` keep their
/// recorded `cwd` as historical evidence even when it names a Workstream that
/// is gone.
pub fn purge_workstream_data_conn(tx: &Transaction<'_>, workstream_id: &str) -> Result<()> {
    // 1. conflict audit trail — references context_conflicts.
    tx.execute(
        "DELETE FROM context_conflict_events
          WHERE conflict_id IN (SELECT id FROM context_conflicts WHERE workstream_id = ?1)",
        params![workstream_id],
    )?;
    // 2. conflicts — they also reference context_items, so they must go first.
    tx.execute(
        "DELETE FROM context_conflicts WHERE workstream_id = ?1",
        params![workstream_id],
    )?;
    // 3. revision history — references context_items.
    tx.execute(
        "DELETE FROM context_item_revisions
          WHERE item_id IN (SELECT id FROM context_items WHERE workstream_id = ?1)",
        params![workstream_id],
    )?;
    // 4. the item search rows, while we can still see which items they were.
    tx.execute(
        "DELETE FROM search_index WHERE kind = 'item' AND parent_id = ?1",
        params![workstream_id],
    )?;
    // 5. items.
    tx.execute(
        "DELETE FROM context_items WHERE workstream_id = ?1",
        params![workstream_id],
    )?;
    // 6. delivery snapshots — provenance of what an Agent was told.
    tx.execute(
        "DELETE FROM context_deliveries WHERE workstream_id = ?1",
        params![workstream_id],
    )?;
    // 7. the ordered path list. `workspace_paths` rows survive: they are
    //    physical facts other Sessions and Workstreams may share, and Project /
    //    WorkspacePath GC is `workspace::project`'s job. Sessions keep
    //    their `owner_workstream_id` until the Workstream row itself is removed
    //    in step 9, where the FK's ON DELETE SET NULL clears it.
    tx.execute(
        "DELETE FROM workstream_paths WHERE workstream_id = ?1",
        params![workstream_id],
    )?;
    // 8. review state.
    tx.execute(
        "DELETE FROM workstream_review_state WHERE workstream_id = ?1",
        params![workstream_id],
    )?;
    tx.execute(
        "DELETE FROM search_index WHERE kind = 'workstream' AND ref_id = ?1",
        params![workstream_id],
    )?;
    // 9. the Workstream itself. Sessions survive; their `owner_workstream_id`
    //    is cleared by the FK's ON DELETE SET NULL in the same statement.
    //
    //    Their search documents are rebuilt right after, in this same
    //    transaction: a Session document embeds its Owner's title, so
    //    leaving it would keep the deleted Workstream's name findable through
    //    the Sessions that used to own it. The ids are collected BEFORE the
    //    delete — afterwards the owner column no longer names them.
    let mut owned_sessions: Vec<String> = Vec::new();
    {
        let mut st = tx.prepare("SELECT id FROM sessions WHERE owner_workstream_id = ?1")?;
        for row in st.query_map(params![workstream_id], |r| r.get(0))? {
            owned_sessions.push(row?);
        }
    }
    tx.execute(
        "DELETE FROM workstreams WHERE id = ?1",
        params![workstream_id],
    )?;
    for session_id in &owned_sessions {
        crate::storage::index_session_conn(tx, session_id)?;
    }
    Ok(())
}

/// The Project projection for one Workstream: which Projects it appears
/// in, and whether it appears there through its primary path.
pub fn project_roles_for_workstream(
    conn: &Connection,
    workstream_id: &str,
) -> Result<Vec<(String, bool)>> {
    let mut st = conn.prepare(
        "SELECT wp.project_id,
                MAX(CASE WHEN wsp.position = 0 THEN 1 ELSE 0 END) AS is_primary
           FROM workstream_paths wsp
           JOIN workspace_paths wp ON wp.id = wsp.workspace_path_id
          WHERE wsp.workstream_id = ?1
          GROUP BY wp.project_id
          ORDER BY is_primary DESC, wp.project_id",
    )?;
    let mapped = st.query_map(params![workstream_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0))
    })?;
    Ok(mapped.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// The Workstreams visible in one Project, with the primary/related
/// distinction. Any position counts as membership; position 0 makes it 主关联.
pub fn workstreams_for_project(
    conn: &Connection,
    project_id: &str,
) -> Result<Vec<(Workstream, bool)>> {
    let mut st = conn.prepare(
        "SELECT w.*, MAX(CASE WHEN wsp.position = 0 THEN 1 ELSE 0 END) AS is_primary
           FROM workstream_paths wsp
           JOIN workspace_paths wp ON wp.id = wsp.workspace_path_id
           JOIN workstreams w ON w.id = wsp.workstream_id
          WHERE wp.project_id = ?1
          GROUP BY w.id
          ORDER BY is_primary DESC, w.updated_at DESC",
    )?;
    let mapped = st.query_map(params![project_id], |r| {
        Ok((
            Workstream {
                id: r.get("id")?,
                title: r.get("title")?,
                description: r.get("description")?,
                lifecycle: r.get("lifecycle")?,
                visibility: r.get("visibility")?,
                created_at: r.get("created_at")?,
                updated_at: r.get("updated_at")?,
            },
            r.get::<_, i64>("is_primary")? != 0,
        ))
    })?;
    Ok(mapped.collect::<std::result::Result<Vec<_>, _>>()?)
}
