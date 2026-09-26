//! WorkstreamPaths, lifecycle and the recycle bin.
//!
//! This module owns the ordered path list, lifecycle and visibility policy.
//!
//! ## The ordered list is the whole model
//!
//! ```text
//! workstream_paths(workstream_id, workspace_path_id, position)
//!   UNIQUE(workstream_id, workspace_path_id)
//!   UNIQUE(workstream_id, position)
//! ```
//!
//! `position = 0` IS the primary path. There is no second workspace authority.
//! Therefore: `paths.is_empty() || paths[0]` exists by construction, and
//! "secondary without primary" is not representable.
//!
//! * remove at index k → recompact positions, so the next entry becomes 0
//!   without asking the user.
//! * `reorder_workstream_paths(ordered_workspace_path_ids)` takes the FULL list;
//!   "make this the primary path" is a reorder to index 0.
//! * `create_workstream(title, description, initial_paths?)` creates the
//!   Workstream plus any accepted initial paths, in submission order.
//! * Workstream→Project is a projection through the paths.
//!
//! ## Lifecycle
//!
//! ```text
//! lifecycle   active | completed     classification only, no behavior, free to switch
//! visibility  normal | archived       archived IS the recycle bin
//! ```
//!
//! `archive` flips visibility and nothing else; `restore` flips back, so
//! lifecycle / paths / configuration are all still there and the previous state
//! returns naturally. Permanent deletion is only reachable from `archived`.
//!
//! ## Permanent deletion table order
//!
//! `context_conflict_events` → `context_conflicts` → `context_item_revisions` →
//! `context_items` → `context_deliveries` → `workstream_paths` →
//! `workstream_review_state` (cascades) → `workstreams`.
//!
//! Never touched: `sessions`, `session_members`, `session_messages`,
//! `launch_intents`, `workspace_paths`, and the Agents' raw source data.
//! A Session survives the Workstream that referenced it; its
//! `owner_workstream_id` is cleared by the `ON DELETE SET NULL` FK when the
//! `workstreams` row goes.
//!
//! ## Reindex
//!
//! Any path-list mutation changes the primary-path Project projection, so it
//! must re-index the Workstream's search row.
//!
//! ## Path mutation vs. Session ownership
//!
//! Removing a WorkstreamPath touches the path list only. It never changes a
//! Session's Owner Workstream, cwd, `workspace_path_id` or `project_id`
//!. A Session's cwd is historical execution fact, the path list is
//! current configuration, ownership is semantic assignment — the three are
//! independent.
//!
//! ## Add vs. create: why unresolvable paths differ
//!
//! `create_workstream(…, initial_paths?)` treats an unresolvable path as "not
//! this one" and reports it per entry (the field is optional, so the Workstream
//! stays valid with zero paths). `add_workstream_path` reports an error instead:
//! there the user asked for exactly one named directory and a silent no-op would
//! be a lie.

use serde::Serialize;

use crate::domain::*;
use crate::error::{other, Result};
use crate::storage::workspace::{get_project_conn, get_workspace_path_conn};
use crate::storage::workstream_paths::{
    append_workstream_path_conn, ordered_canonical_paths_for_workstream, primary_workspace_path,
    purge_workstream_data_conn, reindex_workstream_search_conn, remove_workstream_path_conn,
    reorder_workstream_paths_conn, workstream_path_by_id_conn,
};
use crate::storage::{new_id, now, Db};

use super::WorkspaceAttaching;

/// Managed Tauri state carrying the one runtime implementation of
/// [`WorkspaceAttaching`].
///
/// The concrete type is `workspace::wiring::WorkspaceLayer`, which is why every
/// policy function below takes `&dyn WorkspaceAttaching` explicitly and stays
/// testable with a scripted stand-in. Main wires it in `lib.rs` setup:
///
/// ```text
/// let layer = Arc::new(workspace::wiring::WorkspaceLayer::new(&home));
/// app.manage(workspace::workstream::PathService::new(layer));
/// ```
///
/// and registers the commands in `generate_handler!`. Nothing else may
/// implement or construct it.
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

// ------------------------------------------------------------- creation

/// One entry of [`CreateWorkstreamReport::paths`]: what became of each raw
/// string the user submitted. An entry is either accepted (with the position it
/// took) or rejected (with the reason); the Workstream itself is always created.
#[derive(Debug, Clone, Serialize)]
pub struct CreatedPath {
    pub raw: String,
    pub accepted: bool,
    /// The canonical spelling the path was attached under (accepted only).
    pub canonical_path: Option<String>,
    /// Position in the Workstream's ordered list, 0 = primary (accepted only).
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
/// Accepted paths take consecutive positions in submission order — position 0
/// IS the primary path by construction, so "first accepted wins the primary
/// seat" needs no special case. A string the attacher refuses (`Ok(None)`:
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
        lifecycle: workstream_lifecycle::ACTIVE.into(),
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
            // `Ok(None)` — empty, relative, a reserved app path, a Home-level
            // repository — leaves the Workstream without this path. Creating it
            // is still what the user asked for; the report says what did not
            // land. An `Err` aborts the whole creation (atomicity case).
            let Some(path_id) = attaching.ensure_path(tx, raw)? else {
                paths.push(CreatedPath {
                    raw: raw.to_string(),
                    accepted: false,
                    canonical_path: None,
                    position: None,
                    project_name: None,
                    // The attacher's `Ok(None)` covers several refusals
                    // (relative with no base, a reserved path under NoEnding
                    // Home, a Home-level repository) and reports no reason, so
                    // the message stays true of all of them rather than
                    // guessing at one — the same wording `add_workstream_path`
                    // reports.
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

// ---------------------------------------------------------- the path list

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

/// a user action that ONLY adds a path. Nothing under the path is
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
        reindex_workstream_search_conn(tx, workstream_id)?;
        Ok(row)
    })?;
    Ok(row)
}

/// remove one entry and let the next move up to primary. Sessions are
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
        reindex_workstream_search_conn(tx, workstream_id)?;
        Ok(())
    })
}

/// rewrite the order. Takes the COMPLETE list; "make this the primary
/// path" is a reorder to index 0, never a role flag.
pub fn reorder_workstream_paths(
    db: &Db,
    workstream_id: &str,
    ordered_workspace_path_ids: &[String],
) -> Result<Vec<WorkstreamPath>> {
    require_workstream(db, workstream_id)?;
    db.tx(|tx| {
        reorder_workstream_paths_conn(tx, workstream_id, ordered_workspace_path_ids)?;
        reindex_workstream_search_conn(tx, workstream_id)?;
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

// ------------------------------------------------------- lifecycle & trash

/// `active | completed` and nothing else. Other vocabularies (`open`,
/// `abandoned`) are not normalized here: silently accepting one would leave the
/// stored value outside the closed set, so a caller still writing it is a bug
/// worth surfacing.
///
/// Both values are a label only — no behavior differs, and switching must not
/// touch paths, visibility or Context.
pub fn set_workstream_lifecycle(
    db: &Db,
    workstream_id: &str,
    lifecycle: &str,
) -> Result<Workstream> {
    if lifecycle != workstream_lifecycle::ACTIVE && lifecycle != workstream_lifecycle::COMPLETED {
        return Err(other("Workstream 生命周期只有 active 与 completed"));
    }
    let mut w = require_workstream(db, workstream_id)?;
    w.lifecycle = lifecycle.into();
    w.updated_at = now();
    db.upsert_workstream(&w)?;
    Ok(w)
}

/// Move to the recycle bin. `archived` IS the trash: paths, lifecycle,
/// Context and configuration all survive, and `updated_at` is the
/// only other thing that moves.
///
/// Absolute, not a toggle: the previous `archive_workstream` flipped
/// visibility, so a retry or a double click quietly un-archived the Workstream.
pub fn archive_workstream(db: &Db, workstream_id: &str) -> Result<Workstream> {
    set_visibility(db, workstream_id, workstream_visibility::ARCHIVED)
}

/// Leave the recycle bin. Only visibility returns, and the lifecycle the user
/// had before archiving is therefore still there — "restore the previous state"
/// needs no stored snapshot.
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
/// `lifecycle` and `visibility` may not travel through it: they have commands of
/// their own, and a stale object from another screen silently reverting an
/// archive is exactly the double-authority failure this rule exists to remove.
pub fn apply_whole_object_edit(db: &Db, payload: &Workstream) -> Result<Workstream> {
    let current = require_workstream(db, &payload.id)?;
    if payload.lifecycle != current.lifecycle {
        return Err(other("生命周期请用 set_workstream_lifecycle 修改"));
    }
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
        return Err(other("只能永久删除回收站（已归档）中的 Workstream"));
    }
    db.tx(|tx| purge_workstream_data_conn(tx, workstream_id))?;
    // The FTS rows are gone with the transaction; `unindex` is the same write
    // through the door every other caller uses, and harmless twice.
    db.unindex("workstream", workstream_id);
    Ok(())
}

// --------------------------------------------------------------- projections

/// the card / detail `project_id` + `project_name`, read through the
/// position-0 path.
///
/// Both are `None` for a Workstream with no paths, which is a normal state and
/// not an error.
pub fn primary_project_for_workstream(
    db: &Db,
    workstream_id: &str,
) -> Result<(Option<Id>, Option<String>)> {
    let Some(wp) = primary_workspace_path(&db.read(), workstream_id)? else {
        return Ok((None, None));
    };
    let name = db.get_project(&wp.project_id)?.map(|p| p.name);
    Ok((Some(wp.project_id), name))
}

/// the Workstream-side input to the PreparedLaunch state fingerprint.
///
/// Agent E owns `launcher::compute_state_fingerprint`; this is the shape it
/// reads, defined here so the stale rule and the data live together:
///
/// ```text
/// hasher.update(workstream_launch_paths(db, ws_id)?.fingerprint_input());
/// ```
///
/// Called once per workstream id in E's existing loop, after the metadata it
/// already hashes. `default_ws:` is NOT part of this type — the default
/// workspace comes from `NoEndingHome`, which is not in the DB, so E must pass it
/// in separately (note 4).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkstreamLaunchPaths {
    /// Canonical paths in list order. Order is semantics: never sort this.
    pub ordered_paths: Vec<String>,
}

impl WorkstreamLaunchPaths {
    /// tier 2 — the launch directory, when this Workstream has one.
    pub fn primary(&self) -> Option<&str> {
        self.ordered_paths.first().map(String::as_str)
    }

    /// Tagged, delimited hash bytes.
    ///
    /// Every value is preceded by its label and followed by `|`, so no two
    /// different states can produce the same stream by shifting a boundary, and
    /// the list is written in list order because position 0 IS the fact the
    /// launch depends on. A Workstream with zero paths contributes
    /// `ws_paths:primary:|` — distinct from any non-empty list.
    pub fn fingerprint_input(&self) -> Vec<u8> {
        let mut out = b"ws_paths:".to_vec();
        for p in &self.ordered_paths {
            out.extend_from_slice(p.as_bytes());
            out.push(b'|');
        }
        out.extend_from_slice(b"primary:");
        if let Some(p) = self.primary() {
            out.extend_from_slice(p.as_bytes());
        }
        out.push(b'|');
        out
    }
}

pub fn workstream_launch_paths(db: &Db, workstream_id: &str) -> Result<WorkstreamLaunchPaths> {
    Ok(WorkstreamLaunchPaths {
        ordered_paths: ordered_canonical_paths_for_workstream(&db.read(), workstream_id)?,
    })
}

// ------------------------------------------------------------------- helpers

fn require_workstream(db: &Db, workstream_id: &str) -> Result<Workstream> {
    db.get_workstream(workstream_id)?
        .ok_or_else(|| other("Workstream 不存在"))
}
