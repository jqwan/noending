//! Workstream paths and archive state.
//!
//! Positions record display order only. Removing an entry
//! recompacts positions; reordering takes the FULL list. Every path contributes
//! equally to the task’s Project associations.
//!
//! Visibility (`normal | archived`) is the only task state.
//! `archive` / `restore` only flip visibility, and permanent deletion is
//! reachable only from `archived`.
//!
//! Permanent deletion order: `context_conflict_events` → `context_conflicts` →
//! `context_item_revisions` → `context_items` → `context_deliveries` →
//! `workstream_paths` → `workstream_review_state` (cascades) → `workstreams`.
//! Never touched: `sessions`, `session_members`, `session_messages`,
//! `launch_intents`, `workspace_paths`, and the Agents' raw source data. A
//! Session survives via its `owner_workstream_id` `ON DELETE SET NULL`.
//!
//! Removing a WorkstreamPath touches the
//! path list only — Session Owner, cwd, `workspace_path_id` and `project_id` are
//! independent facts and stay put.
//!
//! `create_workstream` treats an unresolvable path as "not this one" and reports
//! it per entry; `add_workstream_path` errors instead, because there the user
//! asked for exactly one named directory and a silent no-op would be a lie.

use serde::Serialize;

use crate::domain::*;
use crate::error::{other, Result};
use crate::storage::workspace::{get_project_conn, get_workspace_path_conn};
use crate::storage::workstream_paths::{
    append_workstream_path_conn, project_ids_for_workstream, purge_workstream_data_conn,
    reindex_workstream_search_conn, remove_workstream_path_conn, reorder_workstream_paths_conn,
    workstream_path_by_id_conn,
};
use crate::storage::{new_id, now, Db};

use super::WorkspaceAttaching;

/// Managed Tauri state carrying the one runtime implementation of
/// [`WorkspaceAttaching`] (`workspace::wiring::WorkspaceLayer`).
///
/// Every policy function here takes `&dyn WorkspaceAttaching` explicitly so it
/// stays testable with a scripted stand-in; nothing else may implement or
/// construct this.
pub struct PathService {
    attaching: std::sync::Arc<dyn WorkspaceAttaching + Send + Sync>,
}

impl PathService {
    pub fn new(attaching: std::sync::Arc<dyn WorkspaceAttaching + Send + Sync>) -> Self {
        Self { attaching }
    }

    pub fn attaching(&self) -> &dyn WorkspaceAttaching {
        &*self.attaching
    }
}

// Creation

/// One entry of [`CreateWorkstreamReport::paths`]: what became of each raw
/// string the user submitted. An entry is either accepted (with the position it
/// took) or rejected (with the reason); the Workstream itself is always created.
#[derive(Debug, Clone, Serialize)]
pub struct CreatedPath {
    pub raw: String,
    pub accepted: bool,
    /// The canonical spelling the path was attached under (accepted only).
    pub canonical_path: Option<String>,
    /// Display position in the Workstream's ordered list (accepted only).
    pub position: Option<i64>,
    /// Project the path projects onto (accepted only; derived, not chosen).
    pub project_name: Option<String>,
    /// Why the string was not attached (rejected only).
    pub reason: Option<String>,
}

/// The Workstream plus the per-path outcome of creation. The report exists so
/// the UI can say "these three landed, this one didn't and here's why" instead
/// of re-reading the list and silently dropping what it cannot find.
#[derive(Debug, Clone, Serialize)]
pub struct CreateWorkstreamReport {
    pub workstream: Workstream,
    pub paths: Vec<CreatedPath>,
}

/// `create_workstream(title, description, initial_paths?)`.
///
/// Accepted paths take consecutive display positions in submission order.
/// A string the attacher refuses (`Ok(None)`:
/// unresolvable, reserved, the Home itself) is reported, never
/// guessed into a path (不能确定就不猜), and a Workstream whose strings
/// all bounce is still created with zero paths.
///
/// Duplicates inside one call are refused without a second attach: the same raw
/// string twice, or two spellings that resolve to one canonical directory.
///
/// Atomic: the row and every accepted path commit together, so a failed attach
/// (`Err` from the attacher) cannot leave a half-created Workstream.
pub fn create_workstream(
    db: &Db,
    attaching: &dyn WorkspaceAttaching,
    title: &str,
    description: &str,
    initial_paths: &[String],
) -> Result<CreateWorkstreamReport> {
    crate::storage::ensure_not_empty("Workstream 标题", title)?;
    let w = Workstream {
        id: new_id(),
        title: title.trim().to_string(),
        description: description.trim().to_string(),
        visibility: workstream_visibility::NORMAL.into(),
        created_at: now(),
        updated_at: now(),
    };
    let mut paths: Vec<CreatedPath> = Vec::new();
    let mut attached_ids: Vec<String> = Vec::new();
    db.tx(|tx| {
        crate::storage::upsert_workstream_conn(tx, &w)?;
        for raw in initial_paths {
            let raw = raw.trim();
            if raw.is_empty() {
                continue;
            }
            if let Some(earlier) = paths.iter().position(|p| p.raw == raw) {
                paths.push(CreatedPath {
                    raw: raw.to_string(),
                    accepted: false,
                    canonical_path: None,
                    position: None,
                    project_name: None,
                    reason: Some(format!(
                        "与第 {} 条重复",
                        earlier + 1
                    )),
                });
                continue;
            }
            // `Ok(None)` leaves the Workstream without this path; creating it is
            // still what the user asked for, and the report says what did not
            // land. An `Err` aborts the whole creation (atomicity case).
            let Some(path_id) = attaching.ensure_path(tx, raw)? else {
                paths.push(CreatedPath {
                    raw: raw.to_string(),
                    accepted: false,
                    canonical_path: None,
                    position: None,
                    project_name: None,
                    // The attacher reports no reason, so the message stays true of
                    // every `Ok(None)` case rather than guessing at one.
                    reason: Some(
                        "该目录不能作为工作路径：需要一个可解析的绝对路径，且不能是 NoEnding 自留目录"
                            .to_string(),
                    ),
                });
                continue;
            };
            // Two spellings, one directory: the registry already holds it from
            // an earlier entry of this call.
            if attached_ids.iter().any(|known| *known == path_id) {
                paths.push(CreatedPath {
                    raw: raw.to_string(),
                    accepted: false,
                    canonical_path: None,
                    position: None,
                    project_name: None,
                    reason: Some("与前面一条指向同一目录".to_string()),
                });
                continue;
            }
            let wp = get_workspace_path_conn(tx, &path_id)?
                .ok_or_else(|| other("WorkspacePath 写入后消失"))?;
            let project_name = get_project_conn(tx, &wp.project_id)?.map(|p| p.name);
            let row = append_workstream_path_conn(tx, &w.id, &path_id)?;
            attached_ids.push(path_id);
            paths.push(CreatedPath {
                raw: raw.to_string(),
                accepted: true,
                canonical_path: Some(wp.canonical_path),
                position: Some(row.position),
                project_name,
                reason: None,
            });
        }
        reindex_workstream_search_conn(tx, &w.id)?;
        Ok(())
    })?;
    Ok(CreateWorkstreamReport {
        workstream: w,
        paths,
    })
}

// The path list

/// One entry of the list as the UI shows it: the row plus the physical facts
/// behind it.
#[derive(Serialize)]
pub struct WorkstreamPathView {
    #[serde(flatten)]
    pub path: WorkstreamPath,
    pub canonical_path: String,
    pub project_id: Id,
    pub project_name: Option<String>,
    /// Observation, not identity: a path can exist and still be a valid entry.
    pub exists: bool,
}

/// A user action that ONLY adds a path. Nothing under the path is
/// scanned or imported, and an already-present path keeps its original
/// position.
pub fn add_workstream_path(
    db: &Db,
    attaching: &dyn WorkspaceAttaching,
    workstream_id: &str,
    raw_path: &str,
) -> Result<WorkstreamPath> {
    require_workstream(db, workstream_id)?;
    let row = db.tx(|tx| {
        let path_id = attaching.ensure_path(tx, raw_path)?.ok_or_else(|| {
            // The attacher's `Ok(None)` covers several refusals (relative with no
            // base, a reserved path under NoEnding Home, a Home-level repository)
            // and reports no reason, so the message stays true of all of them
            // rather than guessing at one.
            other("该目录不能作为工作路径：需要一个可解析的绝对路径，且不能是 NoEnding 自留目录")
        })?;
        let row = append_workstream_path_conn(tx, workstream_id, &path_id)?;
        Ok(row)
    })?;
    Ok(row)
}

/// Remove one entry and recompact display positions. Sessions are
/// not affected: this changes the Workstream's path list, never a Session's
/// ownership.
pub fn remove_workstream_path(
    db: &Db,
    workstream_id: &str,
    workstream_path_id: &str,
) -> Result<()> {
    require_workstream(db, workstream_id)?;
    db.tx(|tx| {
        if workstream_path_by_id_conn(tx, workstream_id, workstream_path_id)?.is_none() {
            return Err(other("该工作路径不属于此 Workstream"));
        }
        remove_workstream_path_conn(tx, workstream_id, workstream_path_id)?;
        Ok(())
    })
}

/// Rewrite the display order. Takes the COMPLETE list.
pub fn reorder_workstream_paths(
    db: &Db,
    workstream_id: &str,
    ordered_workspace_path_ids: &[String],
) -> Result<Vec<WorkstreamPath>> {
    require_workstream(db, workstream_id)?;
    db.tx(|tx| {
        reorder_workstream_paths_conn(tx, workstream_id, ordered_workspace_path_ids)?;
        Ok(())
    })?;
    db.list_workstream_paths(workstream_id)
}

/// The list with the physical facts behind each entry, in position order.
pub fn list_workstream_path_views(db: &Db, workstream_id: &str) -> Result<Vec<WorkstreamPathView>> {
    require_workstream(db, workstream_id)?;
    let rows = db.list_workstream_paths(workstream_id)?;
    let mut views = Vec::with_capacity(rows.len());
    for path in rows {
        let wp = db.get_workspace_path(&path.workspace_path_id)?;
        // A WorkspacePath that vanished under us is a storage inconsistency, not
        // a reason to hide an entry the user can still remove.
        let Some(wp) = wp else { continue };
        let project_name = db.get_project(&wp.project_id)?.map(|p| p.name);
        views.push(WorkstreamPathView {
            project_id: wp.project_id,
            project_name,
            canonical_path: wp.canonical_path,
            exists: wp.exists,
            path,
        });
    }
    Ok(views)
}

// Archive state

/// Archive preserves paths, Context, configuration and owned sessions.
/// Archived tasks cannot create sessions.
///
/// Absolute, not a toggle: a retry or double click must not quietly
/// un-archive the Workstream.
pub fn archive_workstream(db: &Db, workstream_id: &str) -> Result<Workstream> {
    set_visibility(db, workstream_id, workstream_visibility::ARCHIVED)
}

/// Unarchive without changing task data or associations.
pub fn restore_workstream(db: &Db, workstream_id: &str) -> Result<Workstream> {
    set_visibility(db, workstream_id, workstream_visibility::NORMAL)
}

fn set_visibility(db: &Db, workstream_id: &str, visibility: &str) -> Result<Workstream> {
    let mut w = require_workstream(db, workstream_id)?;
    w.visibility = visibility.into();
    w.updated_at = now();
    db.upsert_workstream(&w)?;
    Ok(w)
}

/// The rules for the whole-object write (`update_workstream`), which the
/// detail page still uses to save a title or a description.
///
/// `visibility` may not travel through it: it has commands of
/// their own, and a stale object from another screen silently reverting an
/// archive is exactly the double-authority failure this rule exists to remove.
pub fn apply_whole_object_edit(db: &Db, payload: &Workstream) -> Result<Workstream> {
    let current = require_workstream(db, &payload.id)?;
    if payload.visibility != current.visibility {
        return Err(other(
            "归档与恢复请用 archive_workstream / restore_workstream",
        ));
    }
    Ok(Workstream {
        created_at: current.created_at,
        updated_at: now(),
        ..payload.clone()
    })
}

/// The only irreversible action in this module, and the reason it is gated on
/// `archived`: the user must have already moved the Workstream out of the way,
/// and the recycle bin is where "delete for real" is offered.
///
/// Deleting the Workstream ends its Context. That is the point of the action,
/// and the only part that is not recoverable from a backup — which is why
/// archive is a prerequisite rather than a courtesy.
pub fn delete_workstream_permanently(db: &Db, workstream_id: &str) -> Result<()> {
    let w = require_workstream(db, workstream_id)?;
    if w.visibility != workstream_visibility::ARCHIVED {
        return Err(other("只能永久删除已归档（已归档）中的 Workstream"));
    }
    db.tx(|tx| purge_workstream_data_conn(tx, workstream_id))?;
    // The FTS rows are gone with the transaction; `unindex` is the same write
    // through the door every other caller uses, and harmless twice.
    db.unindex("workstream", workstream_id);
    Ok(())
}

// Projections

/// A task's projects are derived from every associated path, without roles.
#[derive(Debug, Clone, Serialize)]
pub struct WorkstreamProject {
    pub id: Id,
    pub name: String,
}

pub fn projects_for_workstream(db: &Db, workstream_id: &str) -> Result<Vec<WorkstreamProject>> {
    let ids = project_ids_for_workstream(&db.read(), workstream_id)?;
    let mut projects = Vec::new();
    for id in ids {
        if let Some(project) = db.get_project(&id)? {
            projects.push(WorkstreamProject {
                id,
                name: project.name,
            });
        }
    }
    projects.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    Ok(projects)
}

// Helpers

fn require_workstream(db: &Db, workstream_id: &str) -> Result<Workstream> {
    db.get_workstream(workstream_id)?
        .ok_or_else(|| other("Workstream 不存在"))
}
