//! Workstream commands and the card projection.
//!
//! The ordered path list, lifecycle and recycle bin land here.
//! `workstream_cards` is the Home/Sidebar projection, and
//! `commands.rs` re-exports it so existing tests keep their path.
//!
//! Every command here is a thin door: it takes the arguments, then calls
//! `workspace::workstream`, which holds the policy and is testable against a
//! temp DB with a scripted [`crate::workspace::WorkspaceAttaching`]. No
//! invariant is implemented twice.

use serde::Serialize;
use tauri::State;

use crate::domain::*;
use crate::error::Result;
use crate::storage::Db;
use crate::workspace::workstream::{self, PathService};

use super::{with_db, AppState};

// ---------------- Workstreams ----------------

/// §11 — `create_workstream(title, description, initial_paths?)`.
///
/// Project is derived through the paths, and the launch anchor is the ordered
/// path list. Each entry of `initial_paths` is a raw string, resolved by the
/// Workspace attacher in submission order; a string that does not resolve is
/// reported back instead of guessed into a path, and the Workstream is still
/// created with however many paths did land (possibly zero).
#[tauri::command]
pub fn create_workstream(
    state: State<AppState>,
    paths: State<'_, PathService>,
    title: String,
    description: String,
    initial_paths: Vec<String>,
) -> Result<workstream::CreateWorkstreamReport> {
    with_db(&state, |db| {
        workstream::create_workstream(db, paths.attaching(), &title, &description, &initial_paths)
    })
}

/// Read-only probe for the path picker's feedback line: what would happen if
/// this string were attached as a working path, and which Project it would
/// project onto. Advisory only — the attacher decides at create time.
#[tauri::command]
pub fn probe_workspace_path(
    state: State<AppState>,
    layer: State<'_, std::sync::Arc<crate::workspace::wiring::WorkspaceLayer>>,
    path: String,
) -> Result<crate::workspace::probe::PathProbe> {
    let projection = layer.projection();
    with_db(&state, |db| {
        crate::workspace::probe::probe_workspace_path(
            db,
            projection.observer(),
            projection.policy(),
            &path,
        )
    })
}

/// The path picker's "recent / known directories" candidates, ranked by recent
/// session activity. Pure reads; nothing here creates a WorkspacePath.
#[tauri::command]
pub fn list_recent_workspace_paths(
    state: State<AppState>,
    layer: State<'_, std::sync::Arc<crate::workspace::wiring::WorkspaceLayer>>,
    limit: Option<usize>,
) -> Result<Vec<crate::workspace::probe::RecentWorkspacePath>> {
    let projection = layer.projection();
    with_db(&state, |db| {
        crate::workspace::probe::list_recent_workspace_paths(
            db,
            projection.policy(),
            limit.unwrap_or(8).clamp(1, 50),
        )
    })
}

/// Whole-object write kept for the existing UI. Which fields may not travel
/// through it is `workspace::workstream::apply_whole_object_edit`.
#[tauri::command]
pub fn update_workstream(state: State<AppState>, workstream: Workstream) -> Result<()> {
    with_db(&state, |db| {
        let w = workstream::apply_whole_object_edit(db, &workstream)?;
        db.upsert_workstream(&w)
    })
}

#[tauri::command]
pub fn list_workstreams(
    state: State<AppState>,
    project_id: Option<String>,
) -> Result<Vec<Workstream>> {
    with_db(&state, |db| db.list_workstreams(project_id.as_deref()))
}

// ---------------- ordered workstream paths (§1.5) ----------------

/// The ordered list with the physical facts behind each entry. Position 0 is the
/// primary path; there is no separate role to read.
#[tauri::command]
pub fn list_workstream_paths(
    state: State<AppState>,
    workstream_id: String,
) -> Result<Vec<workstream::WorkstreamPathView>> {
    with_db(&state, |db| {
        workstream::list_workstream_path_views(db, &workstream_id)
    })
}

/// §1.7 — add a working directory. Appends last, or becomes position 0 when the
/// list is empty. Never imports the Sessions under that path.
#[tauri::command]
pub fn add_workstream_path(
    state: State<AppState>,
    paths: State<'_, PathService>,
    workstream_id: String,
    path: String,
) -> Result<WorkstreamPath> {
    with_db(&state, |db| {
        workstream::add_workstream_path(db, paths.attaching(), &workstream_id, &path)
    })
}

/// §1.6 — remove one entry. Sessions and their ownership are untouched.
#[tauri::command]
pub fn remove_workstream_path(
    state: State<AppState>,
    workstream_id: String,
    workstream_path_id: String,
) -> Result<()> {
    with_db(&state, |db| {
        workstream::remove_workstream_path(db, &workstream_id, &workstream_path_id)
    })
}

/// §1.5 — full-list reorder. "设为主路径" is this with the chosen id first.
#[tauri::command]
pub fn reorder_workstream_paths(
    state: State<AppState>,
    workstream_id: String,
    ordered_workspace_path_ids: Vec<String>,
) -> Result<Vec<WorkstreamPath>> {
    with_db(&state, |db| {
        workstream::reorder_workstream_paths(db, &workstream_id, &ordered_workspace_path_ids)
    })
}

// ---------------- lifecycle and the recycle bin (§1.13) ----------------

#[tauri::command]
pub fn set_workstream_lifecycle(
    state: State<AppState>,
    workstream_id: String,
    lifecycle: String,
) -> Result<Workstream> {
    with_db(&state, |db| {
        workstream::set_workstream_lifecycle(db, &workstream_id, &lifecycle)
    })
}

/// Move into the recycle bin. Absolute: archiving an archived Workstream is a
/// no-op, never a restore (the old toggle lost that distinction).
#[tauri::command]
pub fn archive_workstream(state: State<AppState>, workstream_id: String) -> Result<Workstream> {
    with_db(&state, |db| {
        workstream::archive_workstream(db, &workstream_id)
    })
}

/// Move out of the recycle bin. Lifecycle, paths and Context are
/// exactly what the user left, so no state has to be remembered to restore it.
#[tauri::command]
pub fn restore_workstream(state: State<AppState>, workstream_id: String) -> Result<Workstream> {
    with_db(&state, |db| {
        workstream::restore_workstream(db, &workstream_id)
    })
}

/// The irreversible one, and only legal from the recycle bin. Deletes the
/// Workstream and everything only it owned; every Session it referenced
/// survives with its events (§18-12).
#[tauri::command]
pub fn delete_workstream_permanently(state: State<AppState>, workstream_id: String) -> Result<()> {
    with_db(&state, |db| {
        workstream::delete_workstream_permanently(db, &workstream_id)
    })
}

/// Card view for Home / Workstreams pages: everything a "continue working"
/// card needs, computed server-side so the UI stays a thin projection.
#[derive(Serialize)]
pub struct LatestSessionInfo {
    pub id: String,
    pub agent: String,
}

#[derive(Serialize)]
pub struct WorkstreamCardView {
    #[serde(flatten)]
    pub workstream: Workstream,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    /// L1 Current State text (content, falling back to its title).
    pub current_state: Option<String>,
    /// L1 Goal text — the card's last content fallback.
    pub goal: Option<String>,
    /// max(session activity, context edit, workstream update).
    pub last_activity_at: Option<String>,
    pub session_count: i64,
    /// Most recent bound session — the Resume button's agent + target.
    pub latest_session: Option<LatestSessionInfo>,
    /// How many working paths the Workstream has. `0` is a normal state; the UI
    /// distinguishes "no path" from "a path in a Project we cannot name".
    pub path_count: i64,
    /// The position-0 path's canonical spelling, so pickers and Session launch
    /// dialogs can show WHERE a Workstream works without a second read.
    pub primary_path: Option<String>,
}

pub(crate) fn later_ts(a: &Option<String>, b: &Option<String>) -> Option<String> {
    let key = |s: &String| {
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|t| t.with_timezone(&chrono::Utc))
            .ok()
    };
    match (a, b) {
        (None, x) | (x, None) => x.clone(),
        (Some(x), Some(y)) => {
            let (xt, yt) = (key(x), key(y));
            match (xt, yt) {
                (Some(xt), Some(yt)) => {
                    if yt > xt {
                        Some(y.clone())
                    } else {
                        Some(x.clone())
                    }
                }
                // unparseable timestamps fall back to string order
                _ => {
                    if y.as_str() > x.as_str() {
                        Some(y.clone())
                    } else {
                        Some(x.clone())
                    }
                }
            }
        }
    }
}

#[tauri::command]
pub fn list_workstream_cards(state: State<AppState>) -> Result<Vec<WorkstreamCardView>> {
    with_db(&state, workstream_cards)
}

/// Compose card views from the real sources (sessions, items, workstreams).
/// Also the testable core of `list_workstream_cards`.
pub fn workstream_cards(db: &Db) -> Result<Vec<WorkstreamCardView>> {
    let project_names: std::collections::HashMap<String, String> = db
        .list_projects()?
        .into_iter()
        .map(|p| (p.id, p.name))
        .collect();
    let mut cards = Vec::new();
    for w in db.list_workstreams(None)? {
        let (session_count, latest, session_activity) = db.workstream_session_stats(&w.id)?;
        let items_activity = db.workstream_items_last_update(&w.id)?;
        let paths = db.list_workstream_paths(&w.id)?;
        let primary = paths
            .first()
            .and_then(|p| db.get_workspace_path(&p.workspace_path_id).ok().flatten());
        let projected_project = primary.as_ref().map(|wp| wp.project_id.clone());
        // "Last active" only follows real work signals (session activity,
        // context edits) — renames or metadata touches must not make a
        // Workstream look freshly active. Sorting still falls back to
        // updated_at so a brand-new Workstream surfaces at the top.
        let mut last_activity_at = None;
        for candidate in [&session_activity, &items_activity] {
            last_activity_at = later_ts(&last_activity_at, candidate);
        }
        cards.push(WorkstreamCardView {
            project_id: projected_project.clone(),
            project_name: projected_project.and_then(|pid| project_names.get(&pid).cloned()),
            current_state: db.workstream_state_text(&w.id, "current_state")?,
            goal: db.workstream_state_text(&w.id, "goal")?,
            last_activity_at,
            session_count,
            latest_session: latest.map(|(id, agent)| LatestSessionInfo { id, agent }),
            path_count: paths.len() as i64,
            primary_path: primary.map(|wp| wp.canonical_path),
            workstream: w,
        });
    }
    cards.sort_by(|a, b| {
        let key = |c: &WorkstreamCardView| {
            c.last_activity_at
                .as_deref()
                .or(Some(c.workstream.updated_at.as_str()))
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        };
        key(b).cmp(&key(a))
    });
    Ok(cards)
}
