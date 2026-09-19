//! WorkstreamPaths, lifecycle and the recycle bin.
//!
//! Owned by Agent C (方案 §18).
//!
//! ## The ordered list is the whole model (§1.5)
//!
//! ```text
//! workstream_paths(workstream_id, workspace_path_id, position, source)
//!   UNIQUE(workstream_id, workspace_path_id)
//!   UNIQUE(workstream_id, position)
//! ```
//!
//! `position = 0` IS the primary path. There is no `is_primary` column and no
//! primary/secondary pair — two authorities for one fact is precisely how
//! `default_cwd` and `workstreams.project_id` came to disagree with reality.
//! Therefore: `paths.is_empty() || paths[0]` exists by construction, and
//! "secondary without primary" is not representable.
//!
//! * remove at index k → recompact positions, so the next entry becomes 0
//!   without asking the user (§1.6).
//! * `reorder_workstream_paths(ordered_workspace_path_ids)` takes the FULL list;
//!   "make this the primary path" is a reorder to index 0 (§21).
//! * `create_workstream(title, description, initial_path?)` — no `project_id`,
//!   no `default_cwd`. `default_cwd` survives only as a v12 migration input.
//! * Workstream→Project is a projection through the paths. New code must not
//!   write `workstreams.project_id` (`upsert_workstream_conn` no longer accepts
//!   it — §42.2-E6).
//!
//! ## Lifecycle (§1.13 / §5.7)
//!
//! ```text
//! lifecycle   active | completed     classification only, no behavior, free to switch
//! visibility  normal | archived       archived IS the recycle bin
//! ```
//!
//! `archive` flips visibility and nothing else; `restore` flips back, so
//! lifecycle / paths / bindings / configuration are all still there and the
//! previous state returns naturally. Permanent deletion is only reachable from
//! `archived`.
//!
//! ## Permanent deletion table order (§42.3-M6)
//!
//! `context_conflict_events` → `context_conflicts` → `context_item_revisions` →
//! `context_items` → `context_deliveries` → `session_workstream_bindings` →
//! `workstream_paths` → `session_binding_removals` (no FK, so it never cascades
//! and would leak) → `workstream_review_state` (cascades) → `workstreams`.
//!
//! Never touched: `sessions`, `session_events`, `session_cursors`,
//! `launch_intents`, `workspace_paths`, and the Agents' raw transcript files.
//! A Session survives the Workstream that referenced it.
//!
//! ## Reindex
//!
//! Any path-list mutation changes the primary-path Project projection, so it
//! must re-index the Workstream's search row (§42.3-M18).
//!
//! ## Binding side-effects: no tombstone
//!
//! Removing a WorkstreamPath deletes the bindings it brought in (§1.6). Those
//! deletions deliberately do NOT write `session_binding_removals`. A tombstone
//! means "the user refuses THIS session in THIS workstream, forever"; a path
//! removal is a decision about a directory, and keeping the tombstone would
//! mean that re-adding the same path next week silently loses Sessions the user
//! has since bound — an unrecoverable inference from an unrelated action.
//! Bindings deleted here were mostly automatic, and AGENTS.md is explicit that
//! automatic classification may re-propose what it retracts; only a user's own
//! `unbind_session_workstream` is a permanent negative decision.
//!
//! ## Add vs. create: why unresolvable paths differ
//!
//! `create_workstream(…, initial_path?)` treats an unresolvable path as "no
//! path yet" (the field is optional, so the Workstream stays valid with zero
//! paths). `add_workstream_path` reports an error instead: there the user asked
//! for exactly one named directory and a silent no-op would be a lie.

use serde::Serialize;

use crate::domain::*;
use crate::error::{other, Result};
use crate::storage::workstream_paths::{
    append_workstream_path_conn, count_bindings_for_workstream_path_conn,
    ordered_canonical_paths_for_workstream, primary_workspace_path, purge_workstream_data_conn,
    reindex_workstream_search_conn, remove_workstream_path_conn, reorder_workstream_paths_conn,
    workstream_path_by_id_conn,
};
use crate::storage::{new_id, now, Db};

use super::WorkspaceAttaching;

/// Managed Tauri state carrying the one runtime implementation of
/// [`WorkspaceAttaching`].
///
/// The concrete type is `workspace::wiring::WorkspaceLayer` (Agent B's Project
/// policy over Agent A's resolver) — which is why every policy function below
/// takes `&dyn WorkspaceAttaching` explicitly and stays testable with a scripted
/// stand-in. Main wires it in `lib.rs` setup:
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

// ------------------------------------------------------------- creation (§11)

/// `create_workstream(title, description, initial_path?)`.
///
/// No `project_id` and no `default_cwd`: both left the signature with v0.2
/// (§42.2-E6). A Workstream is created with zero paths or exactly one, and that
/// one is position 0 by construction — becoming primary needs no special case.
///
/// Atomic: the row and its first path commit together, so a failed path attach
/// cannot leave a half-created Workstream.
pub fn create_workstream(
    db: &Db,
    attaching: &dyn WorkspaceAttaching,
    title: &str,
    description: &str,
    initial_path: Option<&str>,
) -> Result<Workstream> {
    crate::storage::ensure_not_empty("Workstream 标题", title)?;
    let w = Workstream {
        id: new_id(),
        project_id: None,
        title: title.trim().to_string(),
        description: description.trim().to_string(),
        lifecycle: workstream_lifecycle::ACTIVE.into(),
        visibility: workstream_visibility::NORMAL.into(),
        default_cwd: None,
        created_at: now(),
        updated_at: now(),
    };
    let initial = initial_path.map(str::trim).filter(|s| !s.is_empty());
    db.tx(|tx| {
        crate::storage::upsert_workstream_conn(tx, &w)?;
        if let Some(raw) = initial {
            // `Ok(None)` — empty, relative, a reserved app path, a Home-level
            // repository — leaves the Workstream with zero paths. Creating it is
            // still what the user asked for (§42.3: never guess a path).
            if let Some(path_id) = attaching.ensure_path(tx, raw)? {
                append_workstream_path_conn(tx, &w.id, &path_id, workstream_path_source::USER)?;
            }
        }
        reindex_workstream_search_conn(tx, &w.id)?;
        Ok(())
    })?;
    Ok(w)
}

// ---------------------------------------------------------- the path list (§1.5)

/// One entry of the list as the UI shows it: the row plus the physical facts
/// behind it, and how many Sessions removing it would take with it (§22's
/// warning must be computable before the user commits).
#[derive(Serialize)]
pub struct WorkstreamPathView {
    #[serde(flatten)]
    pub path: WorkstreamPath,
    pub canonical_path: String,
    pub project_id: Id,
    pub project_name: Option<String>,
    /// Observation, not identity: a path can exist and still be a valid entry.
    pub exists: bool,
    pub bound_session_count: i64,
}

/// §1.7 — a user action that ONLY adds a path. Nothing under the path is
/// scanned or imported, and an already-present path keeps its original
/// position and `source` (a later session can never launder `user` provenance).
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
        let row =
            append_workstream_path_conn(tx, workstream_id, &path_id, workstream_path_source::USER)?;
        reindex_workstream_search_conn(tx, workstream_id)?;
        Ok(row)
    })?;
    Ok(row)
}

/// §1.6 — remove one entry and let the next move up to primary.
///
/// Returns how many bindings went with it, so the caller can tell the user what
/// happened without a second read.
pub fn remove_workstream_path(
    db: &Db,
    workstream_id: &str,
    workstream_path_id: &str,
) -> Result<usize> {
    require_workstream(db, workstream_id)?;
    let unbound = db.tx(|tx| {
        if workstream_path_by_id_conn(tx, workstream_id, workstream_path_id)?.is_none() {
            return Err(other("该工作路径不属于此 Workstream"));
        }
        let unbound = remove_workstream_path_conn(tx, workstream_id, workstream_path_id)?;
        reindex_workstream_search_conn(tx, workstream_id)?;
        Ok(unbound)
    })?;
    Ok(unbound)
}

/// §1.5 — rewrite the order. Takes the COMPLETE list; "make this the primary
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
            bound_session_count: count_bindings_for_workstream_path_conn(
                &db.0,
                workstream_id,
                &path.id,
            )?,
            project_id: wp.project_id,
            project_name,
            canonical_path: wp.canonical_path,
            exists: wp.exists,
            path,
        });
    }
    Ok(views)
}

// ------------------------------------------------------- lifecycle & trash (§1.13)

/// `active | completed` and nothing else. The old vocabulary (`open`,
/// `abandoned`) is not silently normalized here: v12 already folded it, and a
/// caller still writing it is a bug worth surfacing.
///
/// Both values are a label only — no behavior differs, and switching must not
/// touch paths, bindings, visibility or Context.
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

/// Move to the recycle bin. `archived` IS the trash (§1.13): paths, bindings,
/// lifecycle, Context and configuration all survive, and `updated_at` is the
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

/// The rules for the legacy whole-object write (`update_workstream`), which the
/// detail page still uses to save a title or a description.
///
/// `lifecycle` and `visibility` may not travel through it: they have commands of
/// their own, and a stale object from another screen silently reverting an
/// archive is exactly the double-authority failure v0.2 exists to remove.
/// `project_id` and `default_cwd` are restored from the stored row instead of
/// refused, because every client echoes them back — they are frozen reads
/// (§42.2-E6), and rejecting the whole write for a field nobody can set would
/// break renaming a Workstream.
///
/// Returns the payload with those columns taken from the row, so nothing the
/// client holds can reach the INSERT branch of the upsert either.
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
        project_id: current.project_id,
        default_cwd: current.default_cwd,
        created_at: current.created_at,
        updated_at: now(),
        ..payload.clone()
    })
}

/// The only irreversible action in this module, and the reason it is gated on
/// `archived`: the user must have already moved the Workstream out of the way,
/// and the recycle bin is where "delete for real" is offered (§1.13, §22).
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

/// §42.3-M19 — the card / detail `project_id` + `project_name`, read through the
/// position-0 path instead of the frozen `workstreams.project_id` column.
///
/// Both are `None` for a Workstream with no paths, which is a normal state and
/// not an error.
pub fn primary_project_for_workstream(
    db: &Db,
    workstream_id: &str,
) -> Result<(Option<Id>, Option<String>)> {
    let Some(wp) = primary_workspace_path(&db.0, workstream_id)? else {
        return Ok((None, None));
    };
    let name = db.get_project(&wp.project_id)?.map(|p| p.name);
    Ok((Some(wp.project_id), name))
}

/// §12 — the Workstream-side input to the PreparedLaunch state fingerprint.
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
/// in separately (§42.3-M17 note 4).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkstreamLaunchPaths {
    /// Canonical paths in list order. Order is semantics: never sort this.
    pub ordered_paths: Vec<String>,
}

impl WorkstreamLaunchPaths {
    /// §13 tier 2 — the launch directory, when this Workstream has one.
    pub fn primary(&self) -> Option<&str> {
        self.ordered_paths.first().map(String::as_str)
    }

    /// Tagged, delimited hash bytes (§42.3-M17).
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
        ordered_paths: ordered_canonical_paths_for_workstream(&db.0, workstream_id)?,
    })
}

// ------------------------------------------------------------------- helpers

fn require_workstream(db: &Db, workstream_id: &str) -> Result<Workstream> {
    db.get_workstream(workstream_id)?
        .ok_or_else(|| other("Workstream 不存在"))
}
