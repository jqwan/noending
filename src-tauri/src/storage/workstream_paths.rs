//! WorkstreamPath persistence: the ordered working-path list (schema v12).
//!
//! The ordering IS the role — position 0 is the primary path, so there is no
//! `is_primary` column to drift out of agreement with the list (方案 §1.5). The
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
//! Policy that calls these — add/remove/reorder endpoints, the recycle bin, the
//! binding side-effects — is `workspace::workstream` (方案 §18). Two of them
//! live here rather than there because they are mechanical and total:
//! [`purge_workstream_data_conn`] (the whole §42.3-M6 delete order, in the
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
        source: r.get("source")?,
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

    /// §13 tier 2 — the launch directory for a New Session on this Workstream.
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

/// §1.8 — append at the end (an empty list yields position 0, which is why
/// "become the primary path" needs no special case).
///
/// Idempotent by `(workstream_id, workspace_path_id)`: re-adding a path that is
/// already in the list returns the existing row rather than failing or shifting
/// positions under the caller.
pub fn append_workstream_path_conn(
    conn: &Connection,
    workstream_id: &str,
    workspace_path_id: &str,
    source: &str,
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
        source: source.to_string(),
        created_at: now(),
    };
    conn.execute(
        "INSERT INTO workstream_paths
           (id, workstream_id, workspace_path_id, position, source, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            row.id,
            row.workstream_id,
            row.workspace_path_id,
            row.position,
            row.source,
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

/// §1.6 — delete the WorkstreamPath and renumber the survivors, in the caller's
/// transaction. Deleting position 0 therefore promotes position 1 automatically:
/// the user never has to pick a new primary path.
///
/// Also unbinds exactly the Sessions that came in through this path
/// (`session_workstream_bindings.workstream_path_id`), and only those:
///  * a binding with `workstream_path_id IS NULL` is a legacy or drifted Session
///    we cannot prove came from this path, so removing it would be a guess that
///    destroys user intent (§42.3-M1/M2);
///  * Sessions reached through another path of the same Workstream are untouched
///    (方案 §32, design §32 — this is the nested `/repo` vs `/repo/frontend` case,
///    resolved by exact identity rather than prefix matching);
///  * the Sessions themselves, their events, and their cursors are never
///    touched here.
///
/// Returns the number of bindings removed.
pub fn remove_workstream_path_conn(
    tx: &Transaction<'_>,
    workstream_id: &str,
    workstream_path_id: &str,
) -> Result<usize> {
    let removed: Option<String> = tx
        .query_row(
            "DELETE FROM workstream_paths
              WHERE workstream_id = ?1 AND id = ?2
              RETURNING id",
            params![workstream_id, workstream_path_id],
            |r| r.get::<_, String>(0),
        )
        .optional()?;
    if removed.is_none() {
        return Ok(0);
    }
    let unbound = tx.execute(
        "DELETE FROM session_workstream_bindings
          WHERE workstream_id = ?1 AND workstream_path_id = ?2",
        params![workstream_id, workstream_path_id],
    )?;
    recompact_workstream_positions_conn(tx, workstream_id)?;
    Ok(unbound)
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

/// §21 — "make this the primary path" is a reorder, not a role change.
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

/// §22 — how many Sessions came in through this WorkstreamPath, which is exactly
/// how many bindings removing it will take with it. Read-only, so the UI can
/// warn before asking, and the warning cannot be stale by the time it is
/// rendered.
pub fn count_bindings_for_workstream_path_conn(
    conn: &Connection,
    workstream_id: &str,
    workstream_path_id: &str,
) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM session_workstream_bindings
          WHERE workstream_id = ?1 AND workstream_path_id = ?2",
        params![workstream_id, workstream_path_id],
        |r| r.get(0),
    )?)
}

/// §12/§13 — the list as canonical path strings, **in position order**.
///
/// Ordering is the contract: this is the byte sequence the PreparedLaunch
/// fingerprint hashes, so a reorder that moves position 0 changes the launch
/// plan and a sorted copy of the list can never make two different orders look
/// equal (§42.3-M17).
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

/// §42.3-M18 — rewrite one Workstream's search row so its `parent_id` follows
/// the CURRENT position-0 path.
///
/// The generic [`super::workspace::refresh_workstream_search_parents_conn`]
/// finds affected Workstreams *through* `workstream_paths`, which cannot work
/// for a removal: by the time it runs, the row that connected the Workstream to
/// the path is gone. This one takes the Workstream id directly and is therefore
/// usable on both sides of a mutation.
///
/// A no-op when the bundled SQLite has no FTS5 (`search_index` absent), exactly
/// like the sibling helper.
pub fn reindex_workstream_search_conn(conn: &Connection, workstream_id: &str) -> Result<()> {
    if conn
        .query_row("SELECT 1 FROM search_index LIMIT 1", [], |_| Ok(()))
        .is_err()
    {
        return Ok(());
    }
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

/// §42.3-M6 — delete every row a Workstream owns, in FK order, inside the
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
/// NOT touched, on purpose: `sessions`, `session_events`, `session_cursors`,
/// `launch_intents`, `workspace_paths`, `projects`, `sync_runs` and the Agents'
/// raw transcript files. A Session survives the Workstream that referenced it
/// (§18-12); `launch_intents` keep their recorded `cwd` as historical evidence
/// even when it names a Workstream that is gone (§42.3-M24).
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
    search_delete_conn(
        tx,
        "DELETE FROM search_index WHERE kind = 'item' AND parent_id = ?1",
        workstream_id,
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
    // 7. bindings. The Sessions they point at are left exactly as they were.
    tx.execute(
        "DELETE FROM session_workstream_bindings WHERE workstream_id = ?1",
        params![workstream_id],
    )?;
    // 8. the ordered path list. `workspace_paths` rows survive: they are
    //    physical facts other Sessions and Workstreams may share, and Project /
    //    WorkspacePath GC is `workspace::project`'s job (§10).
    tx.execute(
        "DELETE FROM workstream_paths WHERE workstream_id = ?1",
        params![workstream_id],
    )?;
    // 9. negative decisions about this pair. No FK points here, so nothing
    //    else would ever remove them and they would outlive their subject.
    tx.execute(
        "DELETE FROM session_binding_removals WHERE workstream_id = ?1",
        params![workstream_id],
    )?;
    // 10. review state.
    tx.execute(
        "DELETE FROM workstream_review_state WHERE workstream_id = ?1",
        params![workstream_id],
    )?;
    search_delete_conn(
        tx,
        "DELETE FROM search_index WHERE kind = 'workstream' AND ref_id = ?1",
        workstream_id,
    )?;
    // 12. the Workstream itself.
    tx.execute(
        "DELETE FROM workstreams WHERE id = ?1",
        params![workstream_id],
    )?;
    Ok(())
}

/// `search_index` is optional (FTS5 may be absent from the bundled SQLite), so
/// every direct statement against it has to tolerate its absence.
fn search_delete_conn(conn: &Connection, sql: &str, arg: &str) -> Result<()> {
    if conn
        .query_row("SELECT 1 FROM search_index LIMIT 1", [], |_| Ok(()))
        .is_err()
    {
        return Ok(());
    }
    conn.execute(sql, params![arg])?;
    Ok(())
}

/// §1.12 — the Project projection for one Workstream: which Projects it appears
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

/// §36 — the Workstreams visible in one Project, with the primary/related
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
