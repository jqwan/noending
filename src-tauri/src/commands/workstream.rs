//! Workstream commands and the card projection.
//!
//! Explicit project memberships and archive state land here.
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
use crate::workspace::workstream::{self};

use super::{with_db, AppState};

// ---------------- Workstreams ----------------

#[tauri::command]
pub fn create_workstream(
    state: State<AppState>,
    title: String,
    description: String,
    project_ids: Vec<String>,
) -> Result<Workstream> {
    with_db(&state, |db| {
        workstream::create_workstream(db, &title, &description, &project_ids)
    })
}
#[tauri::command]
pub fn set_workstream_projects(
    state: State<AppState>,
    workstream_id: String,
    project_ids: Vec<String>,
) -> Result<()> {
    with_db(&state, |db| {
        workstream::set_workstream_projects(db, &workstream_id, &project_ids)
    })
}

/// Read-only probe for the path picker's feedback line: what would happen if
/// this string were attached as a working path, and which Project it would
/// project onto. Advisory only — the attacher decides at create time.
#[tauri::command]
pub async fn probe_workspace_path(
    state: State<'_, AppState>,
    layer: State<'_, std::sync::Arc<crate::workspace::wiring::WorkspaceLayer>>,
    path: String,
) -> Result<crate::workspace::probe::PathProbe> {
    let db = state.db.clone();
    let layer = layer.inner().clone();
    super::run_on_blocking_worker(move || {
        let projection = layer.projection();
        crate::workspace::probe::probe_workspace_path(
            &db,
            projection.observer(),
            projection.policy(),
            &path,
        )
    })
    .await?
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

// ---------------- archive state ----------------

/// Archive the task. Absolute: archiving an archived Workstream is a
/// no-op, never a restore (the old toggle lost that distinction).
#[tauri::command]
pub fn archive_workstream(state: State<AppState>, workstream_id: String) -> Result<Workstream> {
    with_db(&state, |db| {
        workstream::archive_workstream(db, &workstream_id)
    })
}

/// Move out of the recycle bin. Project memberships and Context are
/// exactly what the user left, so no state has to be remembered to restore it.
#[tauri::command]
pub fn restore_workstream(state: State<AppState>, workstream_id: String) -> Result<Workstream> {
    with_db(&state, |db| {
        workstream::restore_workstream(db, &workstream_id)
    })
}

/// The irreversible one, and only legal from the recycle bin. Deletes the
/// Workstream and everything only it owned; every Session it referenced
/// survives with its events.
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
    pub projects: Vec<workstream::WorkstreamProject>,
    pub summary: Option<String>,
    /// max(session activity, context edit, workstream update).
    pub last_activity_at: Option<String>,
    pub session_count: i64,
    /// Most recent bound session — the Resume button's agent + target.
    pub latest_session: Option<LatestSessionInfo>,
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
    let mut cards = Vec::new();
    for w in db.list_workstreams(None)? {
        let (session_count, latest, session_activity) = db.workstream_session_stats(&w.id)?;
        let context = db.get_workstream_context_state(&w.id)?;
        let summary_activity = context.updated_at;
        let projects = workstream::projects_for_workstream(db, &w.id)?;
        // "Last active" only follows real work signals (session activity,
        // context edits) — renames or metadata touches must not make a
        // Workstream look freshly active. Sorting still falls back to
        // updated_at so a brand-new Workstream surfaces at the top.
        let mut last_activity_at = None;
        for candidate in [&session_activity, &summary_activity] {
            last_activity_at = later_ts(&last_activity_at, candidate);
        }
        cards.push(WorkstreamCardView {
            projects,
            summary: (!context.summary.is_empty()).then_some(context.summary),
            last_activity_at,
            session_count,
            latest_session: latest.map(|(id, agent)| LatestSessionInfo { id, agent }),
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
