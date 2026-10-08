//! Project commands.
//!
//! v0.2 makes a Project entirely app-managed: it is derived from the physical
//! workspace registry, users may only rename it, and it disappears when its last
//! WorkspacePath goes. Only the current read and rename surface remains here.
//!
//! The v0.2 surface is exactly three reads and one write:
//!
//! ```text
//! list_projects                          → every derived Project
//! get_project_detail(project_id)         → frozen shape
//! list_project_workstreams(project_id)   → 全部关联任务
//! rename_project(project_id, name)       → the only user-editable Project fact
//! ```
//!
//! `rename_project` is deliberately narrow: automatic identity fields cannot be
//! overwritten by a whole-object UI payload.

use rusqlite::OptionalExtension;
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::domain::*;
use crate::error::{other, Result};
use crate::storage::Db;
use crate::workspace::project::{
    project_detail, project_kind, project_workstreams, reconcile_workspace_path_ids, ProjectDetail,
    ProjectWorkstream, WorkspacePolicy,
};
use crate::workspace::wiring::WorkspaceLayer;

use super::workstream::later_ts;
use super::{with_db, AppState};

// ---------------- Projects: the v0.2 surface ----------------

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
    pub kind: ProjectKind,
    pub path_count: i64,
    /// Paths last observed as gone from disk; the card's "有目录缺失" signal.
    pub missing_path_count: i64,
    /// Distinct tasks related through any working path.
    pub workstream_count: i64,
    /// Sessions through the authoritative
    /// `workspace_path_id → workspace_paths.project_id` chain,
    /// Archived sessions are included.
    pub session_count: i64,
    /// Up to two representative paths for display.
    pub representative_paths: Vec<String>,
    /// Every canonical path of the project, canonical order. Review P2-1:
    /// the board's path search covers ALL paths, while
    /// `representative_paths` stays a display-only truncation.
    pub search_paths: Vec<String>,
    /// max(session activity, workstream update) — the Board's default sort.
    pub last_activity_at: Option<String>,
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

    // Representative paths (first two, display) + the full search list
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
        let mut st = conn.prepare(
            "SELECT wp.project_id, COUNT(DISTINCT wsp.workstream_id)
             FROM workstream_paths wsp JOIN workspace_paths wp ON wp.id = wsp.workspace_path_id
             GROUP BY wp.project_id",
        )?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (id, count) = row?;
            workstream_count.insert(id, count);
        }
    }

    // Session reach + activity through the authoritative chain,
    // Archived sessions are included.
    let mut session_count: BTreeMap<String, i64> = BTreeMap::new();
    let mut session_activity: BTreeMap<String, Option<String>> = BTreeMap::new();
    {
        let mut st = conn.prepare(
            "SELECT wp.project_id, COUNT(*), MAX(COALESCE(s.last_activity_at, s.started_at))
             FROM sessions s
             JOIN workspace_paths wp ON wp.id = s.workspace_path_id
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

    // The workstream half of the activity signal.
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
pub fn list_project_cards(
    state: State<AppState>,
    layer: State<'_, Arc<WorkspaceLayer>>,
) -> Result<Vec<ProjectCardData>> {
    with_db(&state, |db| project_cards(db, layer.projection().policy()))
}

// ---------------- Workspace refresh ----------------
//
// 「刷新工作区状态」不是编辑 Project，而是重新观察外部物理世界，然后让现有
// domain rules 重新投影。Reconcile 里会跑 Path::is_dir 和 git 子进程
// （单次 probe 最长 10s），因此绝不在 command 线程上同步执行：后台线程 +
// 事件回报，全局与定点共用同一套规则。

fn spawn_workspace_reconcile<F>(
    app: &AppHandle,
    state: &State<AppState>,
    job: F,
) -> Result<serde_json::Value>
where
    F: FnOnce(
            &AppState,
            &dyn Fn(usize, usize),
        ) -> Result<crate::workspace::project::ReconcileReport>
        + Send
        + 'static,
{
    use crate::workspace::project::ReconcileReport;

    if state
        .workspace_refresh_in_progress
        .swap(true, Ordering::SeqCst)
    {
        return Err(other("已有工作区刷新在进行中，请等待完成"));
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        fn finish(handle: &AppHandle, result: Result<ReconcileReport>) {
            match result {
                Ok(report) => {
                    let _ = handle.emit(
                        "workspace-reconcile-completed",
                        serde_json::json!({
                            "scanned": report.scanned,
                            "missing": report.missing_paths,
                            "discovered": report.discovered_paths.len(),
                            "moved": report.moved_paths,
                            "deleted_paths": report.outcome.deleted_paths.len(),
                            "deleted_projects": report.outcome.deleted_projects.len(),
                            "failed": report.failed.len(),
                        }),
                    );
                }
                Err(e) => {
                    let _ = handle.emit(
                        "workspace-reconcile-failed",
                        serde_json::json!({ "error": e.to_string() }),
                    );
                }
            }
        }
        let _ = handle.emit("workspace-reconcile-started", serde_json::json!({}));
        let state = handle.state::<AppState>();
        let notify = |scanned: usize, total: usize| {
            let _ = handle.emit(
                "workspace-reconcile-progress",
                serde_json::json!({ "scanned": scanned, "total": total }),
            );
        };
        let result = job(&state, &notify);
        state
            .workspace_refresh_in_progress
            .store(false, Ordering::SeqCst);
        finish(&handle, result);
    });
    Ok(serde_json::json!({ "started": true }))
}

/// 重新观察全部已注册 WorkspacePath：存在性、Git 状态、Git family /
/// worktrees、已有的 Project merge / retire、以及既有 GC 规则。它不是
/// Session Sync，也不做 Context 提取。长时间运行（2N 个 git 子进程可能各等
/// 10s），因此在后台线程执行，UI 通过事件跟进。
#[tauri::command]
pub fn refresh_workspace_projects(
    app: AppHandle,
    state: State<AppState>,
    layer: State<'_, Arc<WorkspaceLayer>>,
) -> Result<serde_json::Value> {
    let layer = layer.inner().clone();
    spawn_workspace_reconcile(&app, &state, move |state, progress| {
        crate::workspace::project::reconcile_workspace_paths_with_progress(
            &state.db,
            &layer.projection(),
            usize::MAX,
            progress,
        )
    })
}

///  — 定点刷新：只扫当前 Project 自己拥有的 WorkspacePaths，与全局
/// 刷新走同一个 `reconcile_workspace_path_ids` 规则集。刷新后 Project 可能
/// 因最后一条路径 GC 而消失——那是正常生命周期，前端以 gone 视图响应。
#[tauri::command]
pub fn refresh_project_workspace(
    app: AppHandle,
    state: State<AppState>,
    layer: State<'_, Arc<WorkspaceLayer>>,
    project_id: String,
) -> Result<serde_json::Value> {
    let path_ids = {
        let conn = state.db.read();
        let exists: Option<String> = conn
            .query_row(
                "SELECT id FROM projects WHERE id = ?1",
                rusqlite::params![project_id],
                |r| r.get(0),
            )
            .optional()?;
        exists.ok_or_else(|| other(format!("Project {project_id} 不存在")))?;
        let ids: Vec<String> = conn
            .prepare(
                "SELECT id FROM workspace_paths WHERE project_id = ?1 ORDER BY canonical_path",
            )?
            .query_map(rusqlite::params![project_id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ids
    };
    let layer = layer.inner().clone();
    spawn_workspace_reconcile(&app, &state, move |state, progress| {
        reconcile_workspace_path_ids(&state.db, &layer.projection(), &path_ids, progress)
    })
}

/// Every Project the app derives. There is no filter and no lifecycle: a Project
/// exists exactly while it owns a WorkspacePath, so this list is the registry
/// grouped by owner.
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

/// All tasks associated with this Project through any working path.
#[tauri::command]
pub fn list_project_workstreams(
    state: State<AppState>,
    project_id: String,
) -> Result<Vec<ProjectWorkstream>> {
    with_db(&state, |db| project_workstreams(db, &project_id))
}

/// rename, and nothing else. A Project's only user-editable fact.
///
/// This sets `name_customized`, which is the durable statement that automatic
/// naming (basename, `NoEnding Workspace`, a merge's survivor rule) may never
/// overwrite this row's name again. No other column is touched: `git_id` in
/// particular has exactly one writer, `workspace::project`.
#[tauri::command]
pub fn rename_project(state: State<AppState>, project_id: String, name: String) -> Result<Project> {
    crate::storage::ensure_not_empty("项目名称", &name)?;
    let name = name.trim().to_string();
    with_db(&state, |db| {
        let project =
            db.tx(|tx| crate::storage::workspace::rename_project_conn(tx, &project_id, &name))?;
        // The search row carries the name, so it follows the rename.
        let _ = db.index_project(&project);
        Ok(project)
    })
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
    Ok(AddProjectPathResult {
        path: outcome.path,
        project_id,
        project_name,
    })
}
