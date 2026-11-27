//! Project commands.
//!
//! Projects are derived from registered directories. Archive and permanent-delete
//! actions are explicit; task/session lifecycle remains independent.

use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tauri::{AppHandle, State};

use crate::domain::*;
use crate::error::{other, Result};
use crate::storage::Db;
use crate::workspace::project::{
    project_detail, project_kind, ProjectDeletion, ProjectDetail, WorkspacePolicy,
};
use crate::workspace::wiring::WorkspaceLayer;

use super::workstream::later_ts;
use super::{noending_home, run_on_blocking_worker, with_db, AppState};

// ---------------- Projects ----------------

/// One Project as the Projects Board needs it: identity, physical
/// availability, workstream/session reach and a recent-activity signal —
/// computed server-side so the Board is ONE query instead of 1 + N detail
/// reads. Diagnostic facts (uuid, git_id, raw identity) deliberately stay off
/// the card; they belong to Project Detail.
#[derive(Serialize)]
pub struct ProjectCardData {
    pub id: String,
    pub name: String,
    pub name_customized: bool,
    pub visibility: String,
    pub kind: ProjectKind,
    pub purpose: Option<ProjectPurpose>,
    pub is_agent: bool,
    pub path_count: i64,
    /// Paths last observed as gone from disk; the card's "有目录缺失" signal.
    pub missing_path_count: i64,
    /// Distinct tasks related through any working path.
    pub workstream_count: i64,
    /// Associated sessions including archived sessions.
    pub session_count: i64,
    /// Up to two representative paths for display.
    pub representative_paths: Vec<String>,
    /// All canonical paths for directory selection. Representative paths are
    /// only a display preview; object search reads authoritative workspace paths.
    pub search_paths: Vec<String>,
    /// max(session activity, workstream update) — the Board's default sort.
    pub last_activity_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Compose the whole Board in one call. Also the testable core of
/// `list_project_cards`, mirroring `workstream_cards`.
pub fn project_cards(db: &Db, policy: &dyn WorkspacePolicy) -> Result<Vec<ProjectCardData>> {
    let projects = db.list_projects()?;
    let conn = db.read();

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

    // Representative paths (first two, display) + all selectable directories
    // from one ordered scan.
    let mut representative: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut search_paths: BTreeMap<String, Vec<String>> = BTreeMap::new();
    {
        let mut st = conn.prepare(
            "SELECT project_id, canonical_path FROM workspace_paths
             ORDER BY project_id, CASE WHEN git_kind = 'main' THEN 0 ELSE 1 END, canonical_path",
        )?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (pid, path) = row?;
            search_paths
                .entry(pid.clone())
                .or_default()
                .push(path.clone());
            let slot = representative.entry(pid).or_default();
            if slot.len() < 2 {
                slot.push(path);
            }
        }
    }

    let mut workstream_count: BTreeMap<String, i64> = BTreeMap::new();
    {
        let mut st = conn
            .prepare("SELECT project_id,COUNT(*) FROM workstream_projects GROUP BY project_id")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (id, count) = row?;
            workstream_count.insert(id, count);
        }
    }

    let agent_projects = {
        let mut st = conn.prepare("SELECT project_id FROM project_assistants")?;
        let rows = st.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<BTreeSet<_>, _>>()?
    };

    // Session reach and activity, including archived sessions.
    let mut session_count: BTreeMap<String, i64> = BTreeMap::new();
    let mut session_activity: BTreeMap<String, Option<String>> = BTreeMap::new();
    {
        let mut st = conn.prepare(
            "SELECT m.project_id, COUNT(*), MAX(COALESCE(s.last_activity_at, s.started_at))
             FROM project_session_membership m JOIN sessions s ON s.id=m.session_id
             GROUP BY m.project_id",
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

    // The workstream half of the activity signal.
    let mut workstream_activity: BTreeMap<String, Option<String>> = BTreeMap::new();
    {
        let mut st = conn.prepare(
            "SELECT wsp.project_id,MAX(w.updated_at) FROM workstreams w JOIN workstream_projects wsp ON wsp.workstream_id=w.id GROUP BY wsp.project_id",
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
                purpose: crate::workspace::project::project_purpose(
                    search_paths.get(&p.id).map(Vec::as_slice).unwrap_or(&[]),
                    policy,
                ),
                is_agent: agent_projects.contains(&p.id),
                kind: project_kind(
                    &p,
                    search_paths.get(&p.id).map(Vec::as_slice).unwrap_or(&[]),
                    policy,
                ),
                path_count: *path_count.get(&p.id).unwrap_or(&0),
                missing_path_count: *missing_count.get(&p.id).unwrap_or(&0),
                workstream_count: *workstream_count.get(&p.id).unwrap_or(&0),
                session_count: *session_count.get(&p.id).unwrap_or(&0),
                representative_paths: representative.get(&p.id).cloned().unwrap_or_default(),
                search_paths: search_paths.get(&p.id).cloned().unwrap_or_default(),
                last_activity_at: later_ts(&session_activity, &workstream_activity),
                created_at: p.created_at,
                id: p.id,
                name: p.name,
                name_customized: p.name_customized,
                visibility: p.visibility,
                updated_at: p.updated_at,
            }
        })
        .collect();
    Ok(cards)
}

#[tauri::command]
pub fn list_project_cards(
    state: State<AppState>,
    layer: State<'_, Arc<WorkspaceLayer>>,
) -> Result<Vec<ProjectCardData>> {
    with_db(&state, |db| project_cards(db, layer.projection().policy()))
}

/// Every registered project, including archived projects.
#[tauri::command]
pub fn list_projects(state: State<AppState>) -> Result<Vec<Project>> {
    with_db(&state, |db| db.list_projects())
}

///  — the frozen detail shape
/// `{ project, kind, workspace_paths[], workstreams[{workstream}], sessions[], remote_url }`.
/// Nothing here is read from a cached membership column: paths, Workstreams and
/// Sessions all come through the registry.
#[tauri::command]
pub fn get_project_detail(
    state: State<AppState>,
    layer: State<'_, Arc<WorkspaceLayer>>,
    project_id: String,
) -> Result<ProjectDetail> {
    with_db(&state, |db| {
        project_detail(db, &project_id, layer.projection().policy())?
            .ok_or_else(|| other(format!("Project {project_id} 不存在")))
    })
}

/// rename, and nothing else. A Project's only user-editable fact.
///
/// This sets `name_customized`, which is the durable statement that automatic
/// naming (basename, `NoEnding Chat`, a merge's survivor rule) may never
/// overwrite this row's name again. No other column is touched: `git_id` in
/// particular has exactly one writer, `workspace::project`.
#[tauri::command]
pub async fn rename_project(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
    name: String,
) -> Result<Project> {
    let home = noending_home(&app).ok_or_else(|| other("NoEnding Home 未初始化"))?;
    let db = state.db.clone();
    run_on_blocking_worker(move || {
        crate::workspace::project::rename_project(&db, &home, &project_id, &name)
    })
    .await?
}

#[derive(Serialize)]
pub struct AddProjectPathResult {
    pub path: WorkspacePath,
    pub project_id: String,
    pub project_name: String,
}

/// 显式注册一个工作目录：若归属已有项目（如 Git 家族），则归属到该项目；
/// 若为新目录，则以其创建新项目。
#[tauri::command]
pub fn add_project_path(
    state: State<AppState>,
    layer: State<'_, Arc<WorkspaceLayer>>,
    path: String,
) -> Result<AddProjectPathResult> {
    let raw = path.trim();
    if raw.is_empty() {
        return Err(other("目录路径不能为空"));
    }
    let projection = layer.projection();
    let observation = projection.observer().observe(raw);
    let outcome = with_db(&state, |db| {
        crate::workspace::project::ensure_workspace_path_outcome(
            db,
            &observation,
            projection.policy(),
        )
    })?;
    let (project_id, project_name) = with_db(&state, |db| {
        let p = db.get_project(&outcome.path.project_id)?;
        let name = p
            .map(|proj| proj.name)
            .unwrap_or_else(|| "未命名项目".to_string());
        Ok((outcome.path.project_id.clone(), name))
    })?;
    // Project-level Skills follow the project, not the directory. A conflicting
    // physical Skill must not block registration; project status shows the
    // directory as partially covered instead.
    let new_cwds: Vec<std::path::PathBuf> = std::iter::once(&outcome.path)
        .chain(outcome.discovered.iter())
        .map(|path| std::path::PathBuf::from(&path.canonical_path))
        .collect();
    if let Ok(paths) = state
        .db
        .list_workspace_paths_for_project(&outcome.path.project_id)
    {
        let existing: Vec<_> = paths
            .into_iter()
            .map(|path| std::path::PathBuf::from(path.canonical_path))
            .filter(|path| path.is_dir() && !new_cwds.contains(path))
            .collect();
        if !existing.is_empty() {
            if let Ok(roots) = crate::skills::Roots::current() {
                let _ = crate::skills::inherit_project_skills(&roots, &existing, &new_cwds);
            }
        }
    }
    Ok(AddProjectPathResult {
        path: outcome.path,
        project_id,
        project_name,
    })
}

#[tauri::command]
pub fn archive_project(state: State<AppState>, project_id: String) -> Result<Project> {
    with_db(&state, |db| {
        crate::workspace::project::set_project_archived(db, &project_id, true)
    })
}

#[tauri::command]
pub async fn clean_missing_project_paths(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<usize> {
    let db = Arc::clone(&state.db);
    super::run_on_blocking_worker(move || {
        crate::workspace::project::clean_missing_project_paths(&db, &project_id)
    })
    .await?
}

#[tauri::command]
pub fn restore_project(state: State<AppState>, project_id: String) -> Result<Project> {
    with_db(&state, |db| {
        crate::workspace::project::set_project_archived(db, &project_id, false)
    })
}

#[tauri::command]
pub async fn delete_project_permanently(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    layer: State<'_, Arc<WorkspaceLayer>>,
    project_id: String,
) -> Result<ProjectDeletion> {
    let db = Arc::clone(&state.db);
    let layer = Arc::clone(&layer);
    let home = noending_home(&app);
    super::run_on_blocking_worker(move || {
        crate::workspace::project::delete_project_permanently(
            &db,
            &project_id,
            layer.projection().policy(),
            home.as_ref(),
        )
    })
    .await?
}
