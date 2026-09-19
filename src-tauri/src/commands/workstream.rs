//! Workstream commands and the card projection.
//!
//! Owned by `Agent C` (方案 §18): the ordered path list, lifecycle and the
//! recycle bin land here. `workstream_cards` is the Home/Sidebar projection, and
//! `commands.rs` re-exports it so existing tests keep their path.

use serde::Serialize;
use tauri::State;

use crate::domain::*;
use crate::error::{other, Result};
use crate::storage::{new_id, now, Db};

use super::{with_db, AppState};

// ---------------- Workstreams ----------------

#[tauri::command]
pub fn create_workstream(
    state: State<AppState>,
    project_id: Option<String>,
    title: String,
    description: String,
    default_cwd: Option<String>,
) -> Result<Workstream> {
    crate::storage::ensure_not_empty("Workstream 标题", &title)?;
    let default_cwd = default_cwd
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let w = Workstream {
        id: new_id(),
        project_id,
        title: title.trim().to_string(),
        description: description.trim().to_string(),
        lifecycle: workstream_lifecycle::ACTIVE.into(),
        visibility: "normal".into(),
        default_cwd,
        created_at: now(),
        updated_at: now(),
    };
    with_db(&state, |db| {
        db.upsert_workstream(&w)?;
        if let Some(pid) = &w.project_id {
            db.touch_project(pid)?;
        }
        Ok(())
    })?;
    Ok(w)
}

#[tauri::command]
pub fn update_workstream(state: State<AppState>, workstream: Workstream) -> Result<()> {
    with_db(&state, |db| {
        let mut w = workstream;
        w.updated_at = now();
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
}

fn later_ts(a: &Option<String>, b: &Option<String>) -> Option<String> {
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

/// Compose card views from the real sources (bindings, items, workstreams).
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
        // "Last active" only follows real work signals (session activity,
        // context edits) — renames or metadata touches must not make a
        // Workstream look freshly active. Sorting still falls back to
        // updated_at so a brand-new Workstream surfaces at the top.
        let mut last_activity_at = None;
        for candidate in [&session_activity, &items_activity] {
            last_activity_at = later_ts(&last_activity_at, candidate);
        }
        cards.push(WorkstreamCardView {
            project_name: w
                .project_id
                .as_ref()
                .and_then(|pid| project_names.get(pid).cloned()),
            current_state: db.workstream_state_text(&w.id, "current_state")?,
            goal: db.workstream_state_text(&w.id, "goal")?,
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

#[tauri::command]
pub fn archive_workstream(state: State<AppState>, workstream_id: String) -> Result<()> {
    with_db(&state, |db| {
        let mut w = db
            .get_workstream(&workstream_id)?
            .ok_or_else(|| other("Workstream 不存在"))?;
        w.visibility = if w.visibility == "archived" {
            "normal"
        } else {
            "archived"
        }
        .into();
        w.updated_at = now();
        db.upsert_workstream(&w)
    })
}

#[tauri::command]
pub fn merge_workstreams(
    state: State<AppState>,
    source_id: String,
    target_id: String,
) -> Result<()> {
    with_db(&state, |db| {
        let source = db
            .get_workstream(&source_id)?
            .ok_or_else(|| other("源 Workstream 不存在"))?;
        let _target = db
            .get_workstream(&target_id)?
            .ok_or_else(|| other("目标 Workstream 不存在"))?;
        // move items + bindings, mark source abandoned
        let items = db.items_for_workstream(&source_id, true)?;
        for (mut item, rev) in items {
            item.workstream_id = target_id.clone();
            item.updated_at = now();
            db.index_item(&item, &rev)?;
            db.0.execute(
                "UPDATE context_items SET workstream_id = ?2, updated_at = ?3 WHERE id = ?1",
                rusqlite::params![item.id, target_id, now()],
            )?;
        }
        let bindings = db.bindings_for_workstream(&source_id)?;
        for b in bindings {
            db.0.execute(
                "INSERT OR IGNORE INTO session_workstream_bindings
                 (session_id, workstream_id, role, source, confidence, last_seen_revision, last_sync_cursor, created_at, last_used_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![b.session_id, target_id, b.role, b.source, b.confidence, b.last_seen_revision, b.last_sync_cursor, b.created_at, b.last_used_at],
            )?;
        }
        let mut src = source;
        src.lifecycle = workstream_lifecycle::COMPLETED.into();
        src.visibility = "archived".into();
        src.updated_at = now();
        db.upsert_workstream(&src)?;
        Ok(())
    })
}
