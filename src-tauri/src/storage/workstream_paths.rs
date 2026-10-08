//! WorkstreamPath persistence: the ordered working-path list.
//!
//! Positions record display order only. Invariant: `paths.is_empty() OR a row at
//! position 0 exists`; it holds only if every removal recompacts, so
//! [`remove_workstream_path_conn`] is the only legal delete.
//!
//! The UNIQUE keys enforce the rest: `(workstream_id, workspace_path_id)` stops
//! duplicates, `(workstream_id, position)` stops two paths claiming one slot.
//! Policy that calls these (add/remove/reorder endpoints, the recycle bin) is
//! `workspace::workstream`; only the mechanical, total helpers live here:
//! [`purge_workstream_data_conn`] and [`reindex_workstream_search_conn`].
//!
//! [`reorder_workstream_paths_conn`] stages through negative positions, so it
//! must only ever run inside a transaction.

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
    /// The ordered list. Every caller that shows or reasons about "the
    /// Workstream's paths" reads it here, so ordering cannot be forgotten.
    pub fn list_workstream_paths(&self, workstream_id: &str) -> Result<Vec<WorkstreamPath>> {
        let conn = self.read();
        let mut st = conn
            .prepare("SELECT * FROM workstream_paths WHERE workstream_id = ?1 ORDER BY position")?;
        let mapped = st.query_map(params![workstream_id], row_workstream_path)?;
        Ok(mapped.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}

/// Append at the end; an empty list starts at position 0. Idempotent by
/// `(workstream_id, workspace_path_id)`: re-adding a path returns the existing
/// row rather than failing or shifting positions under the caller.
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
/// transaction. Every removal recompacts the display positions.
///
/// Touches the path list only — never a Session's cwd, workspace path or Owner.
/// Those are independent facts: cwd is historical execution, the path list is
/// current configuration, the owner is semantic assignment.
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

/// Reorder the display list. Takes the
/// COMPLETE ordered list of workspace path ids and rewrites positions to match;
/// an incomplete list is a caller bug, and silently keeping the old tail would let
/// a UI race drop a user's path.
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
/// The `workstream_id` predicate is not decoration: an id copied from another
/// Workstream (a stale UI row, a race between two lists) must resolve to `None`
/// rather than mutate someone else's path list.
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

/// Task search results have no single parent Project.
pub fn reindex_workstream_search_conn(conn: &Connection, workstream_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM search_index WHERE kind = 'workstream' AND ref_id = ?1",
        params![workstream_id],
    )?;
    conn.execute(
        "INSERT INTO search_index (kind, ref_id, parent_id, title, body)
         SELECT 'workstream', w.id, '', w.title, w.description
         FROM workstreams w WHERE w.id = ?1",
        params![workstream_id],
    )?;
    Ok(())
}

/// Delete every row a Workstream owns, in FK order, inside the caller's
/// transaction. A permanent delete is all-or-nothing: a half-purged Workstream
/// would keep Context rows whose owner no longer exists.
///
/// The search index is an FTS5 table with no FK, so its `kind='item'` rows
/// (`parent_id = workstream_id`) must go with the items. `workstream_review_state`
/// cascades; it is deleted explicitly anyway so the order is written down rather
/// than inferred from the schema.
///
/// NOT touched, on purpose: `sessions` (only their `owner_workstream_id` is
/// cleared by the FK in step 9; the rows survive), `session_members`,
/// `session_messages`, `launch_intents`, `workspace_paths`, `projects`, the Session
/// Contexts themselves and the Agents' raw source data.
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
    // 6. the Workstream's own revision state and per-Session frontiers.
    tx.execute(
        "DELETE FROM workstream_session_frontiers WHERE workstream_id = ?1",
        params![workstream_id],
    )?;
    tx.execute(
        "DELETE FROM workstream_context_state WHERE workstream_id = ?1",
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

/// Every Project reached through the task's paths, once per project.
pub fn project_ids_for_workstream(conn: &Connection, workstream_id: &str) -> Result<Vec<String>> {
    let mut st = conn.prepare(
        "SELECT DISTINCT wp.project_id FROM workstream_paths wsp
           JOIN workspace_paths wp ON wp.id = wsp.workspace_path_id
          WHERE wsp.workstream_id = ?1 ORDER BY wp.project_id",
    )?;
    let rows = st.query_map(params![workstream_id], |r| r.get(0))?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// A task is related to a project through any of its paths.
pub fn workstreams_for_project(conn: &Connection, project_id: &str) -> Result<Vec<Workstream>> {
    let mut st = conn.prepare(
        "SELECT w.* FROM workstreams w
          WHERE EXISTS (SELECT 1 FROM workstream_paths wsp
            JOIN workspace_paths wp ON wp.id = wsp.workspace_path_id
            WHERE wsp.workstream_id = w.id AND wp.project_id = ?1)
          ORDER BY w.updated_at DESC, w.id",
    )?;
    let rows = st.query_map(params![project_id], |r| {
        Ok(Workstream {
            id: r.get("id")?,
            title: r.get("title")?,
            description: r.get("description")?,
            visibility: r.get("visibility")?,
            created_at: r.get("created_at")?,
            updated_at: r.get("updated_at")?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}
