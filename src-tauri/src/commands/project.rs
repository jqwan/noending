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

use tauri::State;

use crate::domain::*;
use crate::error::{other, Result};
use crate::storage::{new_id, now};
use crate::workspace::project::{
    project_detail, project_workstreams, ProjectDetail, ProjectWorkstream,
};

use super::{with_db, AppState};

// ---------------- Projects: the v0.2 surface ----------------

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
