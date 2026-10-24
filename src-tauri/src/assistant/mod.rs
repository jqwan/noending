//! An empty, path-backed Assistant project. Agent sessions use the ordinary
//! launch and ingestion flow; users choose their own project files and Skills.
use crate::{
    domain::Project,
    error::{other, Result},
    storage::Db,
    workspace::{home::NoEndingHome, wiring::WorkspaceLayer},
};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
pub struct AssistantWorkspace {
    pub project: Project,
    pub directory: String,
}
pub fn directory(home: &NoEndingHome) -> PathBuf {
    home.root.join("assistant")
}

pub fn ensure_workspace(
    db: &Db,
    home: &NoEndingHome,
    layer: &WorkspaceLayer,
) -> Result<AssistantWorkspace> {
    let _access = home.acquire_access()?;
    let root = directory(home);
    std::fs::create_dir_all(&root)?;
    let root = root.to_string_lossy().to_string();
    let projection = layer.projection();
    let observation = projection.observer().observe(&root);
    let path =
        crate::workspace::project::ensure_workspace_path(db, &observation, projection.policy())?;
    let project = db
        .get_project(&path.project_id)?
        .ok_or_else(|| other("助手项目未注册"))?;
    Ok(AssistantWorkspace {
        project,
        directory: root,
    })
}
