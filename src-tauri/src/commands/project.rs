//! Project commands.
//!
//! v0.2 makes a Project entirely app-managed: it is derived from the physical
//! workspace registry, users may only rename it, and it disappears when its last
//! WorkspacePath goes. `Agent B` owns this file (方案 §17); the create / delete /
//! resource commands below are legacy surface that Wave 1 un-registers from
//! `lib.rs` while keeping the Rust helpers (方案 §11, §42.2-E10/E11).

use tauri::State;

use crate::domain::*;
use crate::error::Result;
use crate::storage::{new_id, now};

use super::{with_db, AppState};

// ---------------- Projects ----------------

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
        // A user-created Project is treated as named by a person, so automatic
        // naming leaves it alone. (This command exits the product API in
        // Wave 1 — Projects are app-managed from then on.)
        git_id: None,
        name_customized: true,
        created_at: now(),
        updated_at: now(),
    };
    with_db(&state, |db| db.upsert_project(&p))?;
    Ok(p)
}

#[tauri::command]
pub fn update_project(state: State<AppState>, project: Project) -> Result<()> {
    with_db(&state, |db| {
        let mut p = project;
        p.updated_at = now();
        db.upsert_project(&p)
    })
}

#[tauri::command]
pub fn list_projects(state: State<AppState>) -> Result<Vec<Project>> {
    with_db(&state, |db| db.list_projects())
}

/// Deleting a Project only detaches: Workstreams and Sessions survive with
/// `project_id = NULL`. A Project is an optional organization layer, never
/// the lifecycle owner of a Workstream — nothing is archived or deleted.
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
