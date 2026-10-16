//! Tauri boundary: project IDs are checked against the path registry before any
//! filesystem operation. No client-supplied destination path is trusted.
use super::AppState;
use crate::{
    domain::Agent,
    error::{other, Result},
    plugins::{self, Scope},
    storage::Db,
};
use std::path::PathBuf;
use tauri::State;

fn cwd(db: &Db, scope: &Scope) -> Result<Option<PathBuf>> {
    match (&scope.project_id, &scope.workspace_path_id) {
        (None, None) => Ok(None),
        (Some(project), Some(path)) => {
            if db.get_project(project)?.is_none() {
                return Err(other("项目已不存在"));
            }
            let path = db
                .get_workspace_path(path)?
                .filter(|p| &p.project_id == project)
                .ok_or_else(|| other("工作目录不属于所选项目"))?;
            let path = PathBuf::from(path.canonical_path);
            if !path.is_dir() {
                return Err(other("所选工作目录不存在"));
            }
            Ok(Some(path))
        }
        _ => Err(other("项目级配置必须选择项目及其工作目录")),
    }
}

#[tauri::command]
pub async fn list_plugin_resources(
    state: State<'_, AppState>,
    scope: Scope,
) -> Result<plugins::Inventory> {
    let cwd = cwd(&state.db, &scope)?;
    super::run_on_blocking_worker(move || {
        Ok(plugins::list(&plugins::Roots::current()?, cwd.as_deref()))
    })
    .await?
}

#[tauri::command]
pub async fn get_plugin_resource(
    state: State<'_, AppState>,
    scope: Scope,
    id: String,
) -> Result<plugins::ResourceDetail> {
    let cwd = cwd(&state.db, &scope)?;
    super::run_on_blocking_worker(move || {
        plugins::detail(&plugins::Roots::current()?, cwd.as_deref(), &id)
    })
    .await?
}

#[tauri::command]
pub async fn plugin_location_revision(
    state: State<'_, AppState>,
    scope: Scope,
    location_id: String,
) -> Result<String> {
    let cwd = cwd(&state.db, &scope)?;
    super::run_on_blocking_worker(move || {
        plugins::location_revision(&plugins::Roots::current()?, cwd.as_deref(), &location_id)
    })
    .await?
}

#[tauri::command]
pub async fn install_plugin_skill(
    state: State<'_, AppState>,
    scope: Scope,
    source: String,
    location_id: String,
    mode: Option<String>,
) -> Result<plugins::OperationResult> {
    let cwd = cwd(&state.db, &scope)?;
    super::run_on_blocking_worker(move || {
        let install = match mode.as_deref().unwrap_or("copy") {
            "copy" => plugins::install_skill,
            "link" => plugins::link_skill,
            _ => return Err(other("安装方式无效")),
        };
        install(
            &plugins::Roots::current()?,
            cwd.as_deref(),
            std::path::Path::new(&source),
            &location_id,
        )
    })
    .await?
}

#[tauri::command]
pub async fn remove_plugin_skill(
    state: State<'_, AppState>,
    scope: Scope,
    id: String,
    entry_path: String,
    revision: String,
) -> Result<plugins::OperationResult> {
    let cwd = cwd(&state.db, &scope)?;
    super::run_on_blocking_worker(move || {
        plugins::remove_skill(
            &plugins::Roots::current()?,
            cwd.as_deref(),
            &id,
            &entry_path,
            &revision,
        )
    })
    .await?
}

#[tauri::command]
pub async fn set_plugin_skill_state(
    state: State<'_, AppState>,
    scope: Scope,
    id: String,
    config_scope: String,
    revision: String,
    enabled: Option<bool>,
) -> Result<plugins::OperationResult> {
    let cwd = cwd(&state.db, &scope)?;
    super::run_on_blocking_worker(move || {
        plugins::set_skill_state(
            &plugins::Roots::current()?,
            cwd.as_deref(),
            &id,
            &config_scope,
            &revision,
            enabled,
        )
    })
    .await?
}

#[tauri::command]
pub async fn save_plugin_mcp(
    state: State<'_, AppState>,
    scope: Scope,
    location_id: String,
    revision: String,
    name: String,
    config: serde_json::Value,
    create: bool,
) -> Result<plugins::OperationResult> {
    let cwd = cwd(&state.db, &scope)?;
    super::run_on_blocking_worker(move || {
        plugins::save_mcp(
            &plugins::Roots::current()?,
            cwd.as_deref(),
            &location_id,
            &revision,
            &name,
            config,
            create,
        )
    })
    .await?
}

#[tauri::command]
pub async fn remove_plugin_mcp(
    state: State<'_, AppState>,
    scope: Scope,
    id: String,
    revision: String,
) -> Result<plugins::OperationResult> {
    let cwd = cwd(&state.db, &scope)?;
    super::run_on_blocking_worker(move || {
        plugins::remove_mcp(&plugins::Roots::current()?, cwd.as_deref(), &id, &revision)
    })
    .await?
}

#[tauri::command]
pub async fn manage_plugin_package(
    state: State<'_, AppState>,
    scope: Scope,
    agent: Agent,
    action: String,
    target: String,
    id: Option<String>,
    revision: Option<String>,
) -> Result<plugins::OperationResult> {
    let cwd = cwd(&state.db, &scope)?;
    super::run_on_blocking_worker(move || {
        plugins::package_action(
            &plugins::Roots::current()?,
            cwd.as_deref(),
            agent,
            &action,
            &target,
            id.as_deref(),
            revision.as_deref(),
        )
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_configuration_rejects_unregistered_or_unrelated_paths() {
        let root =
            std::env::temp_dir().join(format!("noending-plugin-scope-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let db = Db::open(&root.join("scope.db")).unwrap();
        db.write().execute("INSERT INTO projects(id,name,name_customized,visibility,created_at,updated_at) VALUES('p','P',0,'normal','now','now')", []).unwrap();
        let workspace =
            crate::domain::WorkspacePath::new(root.to_string_lossy().into_owned(), "p".into());
        crate::storage::workspace::insert_workspace_path_conn(
            &db.write(),
            &workspace.canonical_path,
            &workspace.project_id,
        )
        .unwrap();
        let valid = Scope {
            project_id: Some("p".into()),
            workspace_path_id: Some(workspace.id.clone()),
        };
        assert_eq!(cwd(&db, &valid).unwrap(), Some(root.clone()));
        assert!(cwd(
            &db,
            &Scope {
                project_id: Some("other".into()),
                ..valid.clone()
            }
        )
        .is_err());
        assert!(cwd(
            &db,
            &Scope {
                project_id: Some("p".into()),
                workspace_path_id: None
            }
        )
        .is_err());
        assert!(cwd(
            &db,
            &Scope {
                project_id: None,
                workspace_path_id: Some(workspace.id)
            }
        )
        .is_err());
        assert!(cwd(
            &db,
            &Scope {
                project_id: Some("p".into()),
                workspace_path_id: Some("unregistered".into())
            }
        )
        .is_err());
        assert_eq!(cwd(&db, &Scope::default()).unwrap(), None);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}
