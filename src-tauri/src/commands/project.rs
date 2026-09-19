//! Project commands.
//!
//! v0.2 makes a Project entirely app-managed: it is derived from the physical
//! workspace registry, users may only rename it, and it disappears when its last
//! WorkspacePath goes. `Agent B` owns this file (方案 §17); the create / delete /
//! resource commands below are legacy surface that Wave 1 un-registers from
//! `lib.rs` while keeping the Rust helpers (方案 §11, §42.2-E10/E11).
//!
//! The v0.2 surface is exactly three reads and one write:
//!
//! ```text
//! list_projects                          → every derived Project
//! get_project_detail(project_id)         → §11 frozen shape
//! list_project_workstreams(project_id)   → 主关联 / 关联 (§42.2-E4)
//! rename_project(project_id, name)       → the only user-editable Project fact
//! ```
//!
//! `rename_project` is deliberately narrow. `update_project` was a whole-object
//! write, which under v0.2 means the UI could re-commit `git_id`, `name` and
//! `name_customized` it never observed — the same double-authority §42.2-E6 cut
//! out of `upsert_workstream_conn`. It stays compiled only for legacy callers and
//! must not be registered.

use serde::Serialize;
use std::collections::BTreeMap;
use tauri::State;

use crate::domain::*;
use crate::error::{other, Result};
use crate::storage::{new_id, now, Db};
use crate::workspace::project::{
    project_detail, project_workstreams, ProjectDetail, ProjectWorkstream,
};

use super::{with_db, AppState};
use super::workstream::later_ts;

// ---------------- Projects: the v0.2 surface ----------------

/// One Project as the Projects Board needs it (方案 §4/§8): identity, physical
/// availability, workstream/session reach and a recent-activity signal —
/// computed server-side so the Board is ONE query instead of 1 + N detail
/// reads. Diagnostic facts (uuid, git_id, raw identity) deliberately stay off
/// the card; they belong to Project Detail.
#[derive(Serialize)]
pub struct ProjectCardData {
    pub id: String,
    pub name: String,
    pub name_customized: bool,
    /// The Project's family badge (Git 家族). The raw `git_id` stays internal.
    pub has_git_identity: bool,
    pub path_count: i64,
    /// Paths last observed as gone from disk; the card's "有目录缺失" signal.
    pub missing_path_count: i64,
    /// Workstreams reaching this Project through their position-0 path (主关联).
    pub primary_workstream_count: i64,
    /// Workstreams with any path here but a position-0 elsewhere (关联).
    pub related_workstream_count: i64,
    /// Sessions through the authoritative
    /// `workspace_path_id → workspace_paths.project_id` chain (§1.10),
    /// trashed sessions excluded (v13 lifecycle).
    pub session_count: i64,
    /// Up to two canonical paths in canonical order (方案 §4: primary +
    /// "另有 N 个目录").
    pub representative_paths: Vec<String>,
    /// max(session activity, workstream update) — the Board's default sort.
    pub last_activity_at: Option<String>,
    pub updated_at: String,
}

/// Compose the whole Board in one call (方案 §8). Also the testable core of
/// `list_project_cards`, mirroring `workstream_cards`.
pub fn project_cards(db: &Db) -> Result<Vec<ProjectCardData>> {
    let projects = db.list_projects()?;
    let conn = db.conn();

    // Path totals + missing counts, one grouped query.
    let mut path_count: BTreeMap<String, i64> = BTreeMap::new();
    let mut missing_count: BTreeMap<String, i64> = BTreeMap::new();
    {
        let mut st = conn.prepare(
            "SELECT project_id, COUNT(*),
                    SUM(CASE WHEN exists_on_disk = 0 THEN 1 ELSE 0 END)
             FROM workspace_paths GROUP BY project_id",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (pid, total, missing) = row?;
            path_count.insert(pid.clone(), total);
            missing_count.insert(pid, missing);
        }
    }

    // Representative paths: first two canonical paths per project (方案 §4).
    let mut representative: BTreeMap<String, Vec<String>> = BTreeMap::new();
    {
        let mut st = conn.prepare(
            "SELECT project_id, canonical_path FROM workspace_paths
             ORDER BY project_id, canonical_path",
        )?;
        let rows = st.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (pid, path) = row?;
            let slot = representative.entry(pid).or_default();
            if slot.len() < 2 {
                slot.push(path);
            }
        }
    }

    // Workstream relation per project, folded in Rust from one ordered query:
    // a workstream's FIRST (position-0) path names its 主关联 Project; every
    // project it touches at all counts it as 关联 unless it is primary there.
    let mut primary_ws: BTreeMap<String, i64> = BTreeMap::new();
    let mut related_ws: BTreeMap<String, i64> = BTreeMap::new();
    {
        let mut st = conn.prepare(
            "SELECT wsp.workstream_id, wsp.position, wp.project_id
             FROM workstream_paths wsp
             JOIN workspace_paths wp ON wp.id = wsp.workspace_path_id
             ORDER BY wsp.workstream_id, wsp.position",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut current_ws: Option<String> = None;
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for row in rows {
            let (ws_id, _position, project_id) = row?;
            if current_ws.as_deref() != Some(ws_id.as_str()) {
                // position-0 row of this workstream: its 主关联 Project.
                *primary_ws.entry(project_id.clone()).or_insert(0) += 1;
                current_ws = Some(ws_id);
                seen.clear();
            }
            if seen.insert(project_id.clone()) {
                *related_ws.entry(project_id).or_insert(0) += 1;
            }
        }
    }

    // Session reach + activity through the authoritative chain (§1.10),
    // trashed sessions excluded (v13 lifecycle: §11).
    let mut session_count: BTreeMap<String, i64> = BTreeMap::new();
    let mut session_activity: BTreeMap<String, Option<String>> = BTreeMap::new();
    {
        let mut st = conn.prepare(
            "SELECT wp.project_id, COUNT(*), MAX(COALESCE(s.last_activity_at, s.started_at))
             FROM sessions s
             JOIN workspace_paths wp ON wp.id = s.workspace_path_id
             WHERE s.trashed_at IS NULL
             GROUP BY wp.project_id",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?;
        for row in rows {
            let (pid, count, activity) = row?;
            session_count.insert(pid.clone(), count);
            session_activity.insert(pid, activity);
        }
    }

    // The workstream half of the activity signal (§7).
    let mut workstream_activity: BTreeMap<String, Option<String>> = BTreeMap::new();
    {
        let mut st = conn.prepare(
            "SELECT wp.project_id, MAX(w.updated_at)
             FROM workstreams w
             JOIN workstream_paths wsp ON wsp.workstream_id = w.id
             JOIN workspace_paths wp ON wp.id = wsp.workspace_path_id
             GROUP BY wp.project_id",
        )?;
        let rows = st.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })?;
        for row in rows {
            let (pid, updated) = row?;
            workstream_activity.insert(pid, updated);
        }
    }

    let cards = projects
        .into_iter()
        .map(|p| {
            let session_activity = session_activity.get(&p.id).cloned().flatten();
            let workstream_activity = workstream_activity.get(&p.id).cloned().flatten();
            ProjectCardData {
                has_git_identity: p.git_id.is_some(),
                path_count: *path_count.get(&p.id).unwrap_or(&0),
                missing_path_count: *missing_count.get(&p.id).unwrap_or(&0),
                primary_workstream_count: *primary_ws.get(&p.id).unwrap_or(&0),
                related_workstream_count: {
                    let related = *related_ws.get(&p.id).unwrap_or(&0);
                    let primary = *primary_ws.get(&p.id).unwrap_or(&0);
                    (related - primary).max(0)
                },
                session_count: *session_count.get(&p.id).unwrap_or(&0),
                representative_paths: representative.get(&p.id).cloned().unwrap_or_default(),
                last_activity_at: later_ts(&session_activity, &workstream_activity),
                id: p.id,
                name: p.name,
                name_customized: p.name_customized,
                updated_at: p.updated_at,
            }
        })
        .collect();
    Ok(cards)
}

#[tauri::command]
pub fn list_project_cards(state: State<AppState>) -> Result<Vec<ProjectCardData>> {
    with_db(&state, project_cards)
}


/// Every Project the app derives. There is no filter and no lifecycle: a Project
/// exists exactly while it owns a WorkspacePath, so this list is the registry
/// grouped by owner (§1.2).
#[tauri::command]
pub fn list_projects(state: State<AppState>) -> Result<Vec<Project>> {
    with_db(&state, |db| db.list_projects())
}

/// §11/§42.2-E5 — the frozen detail shape
/// `{ project, workspace_paths[], workstreams[{workstream, is_primary}], sessions[] }`.
/// Nothing here is read from a cached membership column: paths, Workstreams and
/// Sessions all come through the registry.
#[tauri::command]
pub fn get_project_detail(state: State<AppState>, project_id: String) -> Result<ProjectDetail> {
    with_db(&state, |db| {
        project_detail(db, &project_id)?
            .ok_or_else(|| other(format!("Project {project_id} 不存在")))
    })
}

/// §42.2-E4 — the Workstreams of one Project with the primary/related
/// distinction Project Detail renders (position 0 is 主关联).
#[tauri::command]
pub fn list_project_workstreams(
    state: State<AppState>,
    project_id: String,
) -> Result<Vec<ProjectWorkstream>> {
    with_db(&state, |db| project_workstreams(db, &project_id))
}

/// §17-14/15 — rename, and nothing else. A Project's only user-editable fact.
///
/// This sets `name_customized`, which is the durable statement that automatic
/// naming (basename, `NoEnding Workspace`, a merge's survivor rule) may never
/// overwrite this row's name again. No other column is touched: `git_id` in
/// particular has exactly one writer, `workspace::project` (§8.4).
#[tauri::command]
pub fn rename_project(state: State<AppState>, project_id: String, name: String) -> Result<Project> {
    crate::storage::ensure_not_empty("项目名称", &name)?;
    let name = name.trim().to_string();
    with_db(&state, |db| {
        let project =
            db.tx(|tx| crate::storage::workspace::rename_project_conn(tx, &project_id, &name))?;
        // The search row carries the name, so it follows the rename (§42.3-M18).
        let _ = db.index_project(&project);
        Ok(project)
    })
}

// ---------------- Legacy surface: kept compiling, unregistered ----------------
//
// Everything below is v0.1 API. §11退出产品 API: a user may not create, delete or
// wholesale-edit a Project any more, and the resource commands are §7.5's retired
// "manage Project" UI. The Rust helpers stay because `storage::Db` and the v12
// migration still use them; `lib.rs` must not register any of them (方案 §42.5-T1).

#[tauri::command]
pub fn create_project(
    state: State<AppState>,
    name: String,
    description: String,
) -> Result<Project> {
    crate::storage::ensure_not_empty("项目名称", &name)?;
    let p = Project {
        id: new_id(),
        name: name.trim().to_string(),
        description: description.trim().to_string(),
        archived: false,
        // LEGACY: a user-created Project is treated as named by a person, so
        // automatic naming leaves it alone. No v0.2 code path can create a
        // Project this way — `workspace::project` owns creation (§8).
        git_id: None,
        name_customized: true,
        created_at: now(),
        updated_at: now(),
    };
    with_db(&state, |db| db.upsert_project(&p))?;
    Ok(p)
}

/// LEGACY whole-object write. Un-registered in v0.2: see the file header for why
/// `rename_project` replaced it.
#[tauri::command]
pub fn update_project(state: State<AppState>, project: Project) -> Result<()> {
    with_db(&state, |db| {
        let mut p = project;
        p.updated_at = now();
        db.upsert_project(&p)
    })
}

/// LEGACY. Under v0.2 the only legitimate reason a Project is deleted is that it
/// owns no WorkspacePath any more, and that is automatic (§1.2, §35) — a user
/// cannot ask for it. Deleting by id is kept as a migration/testing helper.
#[tauri::command]
pub fn delete_project(state: State<AppState>, project_id: String) -> Result<()> {
    with_db(&state, |db| db.delete_project(&project_id))
}

#[tauri::command]
pub fn add_project_resource(
    state: State<AppState>,
    project_id: String,
    kind: String,
    uri: Option<String>,
) -> Result<ProjectResource> {
    let r = ProjectResource {
        id: new_id(),
        project_id,
        kind,
        uri,
        metadata: serde_json::json!({}),
        created_at: now(),
    };
    with_db(&state, |db| db.add_resource(&r))?;
    Ok(r)
}

#[tauri::command]
pub fn list_project_resources(
    state: State<AppState>,
    project_id: String,
) -> Result<Vec<ProjectResource>> {
    with_db(&state, |db| db.list_resources(&project_id))
}

#[tauri::command]
pub fn remove_project_resource(state: State<AppState>, resource_id: String) -> Result<()> {
    with_db(&state, |db| db.remove_resource(&resource_id))
}
