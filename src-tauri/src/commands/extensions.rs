//! Tauri boundary: project IDs are checked against the path registry before any
//! filesystem operation. No client-supplied destination path is trusted.
use super::AppState;
use crate::{
    error::{other, Result},
    extensions::{self, Scope},
    storage::Db,
};
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use tauri::State;

/// Board and detail are separate routes, so entering a resource immediately
/// after the board scans the same directories again. Keep the merged scan for a
/// few seconds; every mutation below invalidates it explicitly.
const CATALOG_TTL: Duration = Duration::from_secs(10);

struct CachedCatalog {
    key: String,
    at: Instant,
    inventory: extensions::Inventory,
}

static CATALOG: OnceLock<Mutex<Option<CachedCatalog>>> = OnceLock::new();

fn catalog_cache() -> &'static Mutex<Option<CachedCatalog>> {
    CATALOG.get_or_init(|| Mutex::new(None))
}

fn invalidate_catalog() {
    *catalog_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// Clears the catalog again when a mutation finishes so a read that ran while
/// files were changing cannot leave its pre-mutation snapshot cached.
struct CatalogReset;

impl Drop for CatalogReset {
    fn drop(&mut self) {
        invalidate_catalog();
    }
}

fn catalog_key(roots: &extensions::Roots, cwds: &[PathBuf]) -> String {
    let mut paths: Vec<String> = cwds
        .iter()
        .map(|path| path.to_string_lossy().into())
        .collect();
    paths.sort();
    format!(
        "{}\0{}\0{}\0{}\0{}",
        roots.home.display(),
        roots.codex.display(),
        roots.claude.display(),
        roots.pi.display(),
        paths.join("\0")
    )
}

/// The NoEnding library is NoEnding-owned, so a Skill dropped into it is
/// registered by location; discovery keeps its metadata and lists sources that
/// currently have no Agent entry. Agent directories remain the only Locations.
fn sync_library_sources(db: &Db, managed_root: &Path) -> Result<()> {
    let mut registry = extensions::load_skill_registry(db)?;
    if extensions::discover_library_sources(managed_root, &mut registry)? {
        extensions::save_skill_registry(db, &registry)?;
        invalidate_catalog();
    }
    Ok(())
}

fn catalog(
    roots: &extensions::Roots,
    cwds: &[PathBuf],
    skill_registry: &extensions::SkillRegistry,
    mcp_registry: &extensions::McpRegistry,
    managed_root: &Path,
) -> extensions::Inventory {
    let key = catalog_key(roots, cwds);
    let mut cache = catalog_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(cached) = cache.as_ref() {
        if cached.key == key && cached.at.elapsed() < CATALOG_TTL {
            return cached.inventory.clone();
        }
    }
    let mut inventory = extensions::list_catalog(roots, cwds);
    extensions::merge_skill_sources(skill_registry, &mut inventory, true, managed_root);
    extensions::merge_mcp_definitions(mcp_registry, &mut inventory, true);
    *cache = Some(CachedCatalog {
        key,
        at: Instant::now(),
        inventory: inventory.clone(),
    });
    inventory
}

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

fn project_paths(db: &Db, project_id: &str) -> Result<Vec<crate::domain::WorkspacePath>> {
    if db.get_project(project_id)?.is_none() {
        return Err(other("项目已不存在"));
    }
    Ok(db
        .list_workspace_paths_for_project(project_id)?
        .into_iter()
        .filter(|path| std::path::Path::new(&path.canonical_path).is_dir())
        .collect())
}

fn projects_with_paths(db: &Db) -> Result<Vec<(String, String, Vec<(String, PathBuf)>)>> {
    let mut rows = Vec::new();
    for project in db.list_projects()? {
        let paths = project_paths(db, &project.id)?;
        if paths.is_empty() {
            continue;
        }
        rows.push((
            project.id,
            project.name,
            paths
                .into_iter()
                .map(|path| (path.id, PathBuf::from(path.canonical_path)))
                .collect(),
        ));
    }
    Ok(rows)
}

/// A project without a workspace path is a project-wide scope: every registered
/// working directory participates. A project with one is a single-directory
/// scope used by MCP configuration.
fn scope_paths(db: &Db, scope: &Scope) -> Result<(Option<PathBuf>, Option<Vec<PathBuf>>)> {
    match (&scope.project_id, &scope.workspace_path_id) {
        (None, None) => Ok((None, None)),
        (Some(_), Some(_)) => {
            let cwd = cwd(db, scope)?.expect("project scope has a cwd");
            Ok((Some(cwd.clone()), Some(vec![cwd])))
        }
        (Some(project), None) => {
            let paths = project_paths(db, project)?
                .into_iter()
                .map(|path| PathBuf::from(path.canonical_path))
                .collect();
            Ok((None, Some(paths)))
        }
        _ => Err(other("项目级配置必须选择项目")),
    }
}

/// Uninstall is Skill-scoped, not scope-scoped: cover every registered project
/// path so one action removes the Skill from all inventories.
fn workspace_cwds(db: &Db) -> Result<Vec<PathBuf>> {
    Ok(db
        .list_workspace_paths()?
        .into_iter()
        .map(|p| PathBuf::from(p.canonical_path))
        .filter(|p| p.is_dir())
        .collect())
}

/// NoEnding-owned Skill sources live beside the Home's other app data.
fn managed_skills_dir(app: &tauri::AppHandle) -> Result<PathBuf> {
    super::noending_home(app)
        .map(|home| home.root.join("skills"))
        .ok_or_else(|| other("无法确定 NoEnding 数据目录"))
}

#[tauri::command]
pub async fn list_extension_resources(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    scope: Scope,
) -> Result<extensions::Inventory> {
    let managed_root = managed_skills_dir(&app)?;
    sync_library_sources(&state.db, &managed_root)?;
    let registry = extensions::load_skill_registry(&state.db)?;
    let mcp_registry = extensions::load_mcp_registry(&state.db)?;
    let (cwd, project_cwds) = scope_paths(&state.db, &scope)?;
    super::run_on_blocking_worker(move || {
        let roots = extensions::Roots::current()?;
        let mut inventory = if let Some(paths) = project_cwds {
            let inventories = paths
                .iter()
                .map(|path| extensions::list(&roots, Some(path)));
            extensions::merge_inventories(inventories.collect())
        } else {
            extensions::list(&roots, cwd.as_deref())
        };
        extensions::merge_skill_sources(&registry, &mut inventory, true, &managed_root);
        extensions::merge_mcp_definitions(&mcp_registry, &mut inventory, true);
        Ok(inventory)
    })
    .await?
}

#[tauri::command]
pub async fn get_extension_detail(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<Option<extensions::DetailBundle>> {
    let managed_root = managed_skills_dir(&app)?;
    sync_library_sources(&state.db, &managed_root)?;
    let skill_registry = extensions::load_skill_registry(&state.db)?;
    let mcp_registry = extensions::load_mcp_registry(&state.db)?;
    let cwds = workspace_cwds(&state.db)?;
    let projects = projects_with_paths(&state.db)?;
    super::run_on_blocking_worker(move || {
        let roots = extensions::Roots::current()?;
        let inventory = catalog(&roots, &cwds, &skill_registry, &mcp_registry, &managed_root);
        let Some(detail) = extensions::detail_in_catalog(&inventory, &mcp_registry, &id)? else {
            return Ok(None);
        };
        let scopes = extensions::project_scopes_from_inventory(&roots, &inventory, &projects, &id);
        let source_kind = (detail.resource.kind == "skill")
            .then(|| extensions::skill_source_kind(&detail.resource, &managed_root));
        Ok(Some(extensions::DetailBundle {
            detail,
            locations: inventory.locations,
            scopes,
            source_kind,
        }))
    })
    .await?
}

#[tauri::command]
pub async fn list_extension_catalog(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<extensions::Inventory> {
    let managed_root = managed_skills_dir(&app)?;
    sync_library_sources(&state.db, &managed_root)?;
    let skill_registry = extensions::load_skill_registry(&state.db)?;
    let mcp_registry = extensions::load_mcp_registry(&state.db)?;
    let cwds = workspace_cwds(&state.db)?;
    super::run_on_blocking_worker(move || {
        let roots = extensions::Roots::current()?;
        Ok(catalog(
            &roots,
            &cwds,
            &skill_registry,
            &mcp_registry,
            &managed_root,
        ))
    })
    .await?
}

#[tauri::command]
pub async fn register_extension_skill_source(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    source: String,
) -> Result<extensions::OperationResult> {
    invalidate_catalog();
    let _catalog_reset = CatalogReset;
    let managed_root = managed_skills_dir(&app)?;
    let cwds = workspace_cwds(&state.db)?;
    let outcome = super::run_on_blocking_worker(move || {
        let roots = extensions::Roots::current()?;
        extensions::adopt_source(&roots, &cwds, &managed_root, Path::new(&source))
    })
    .await??;
    save_skill_source(
        &state,
        &outcome.source_path,
        &outcome.name,
        &outcome.description,
    )?;
    Ok(extensions::OperationResult {
        message: outcome.message,
    })
}

#[tauri::command]
pub async fn enable_extension_skill_all(
    state: State<'_, AppState>,
    id: String,
) -> Result<extensions::OperationResult> {
    invalidate_catalog();
    let _catalog_reset = CatalogReset;
    let registry = extensions::load_skill_registry(&state.db)?;
    let source = extensions::find_source(&registry, &id).map(|source| source.path.clone());
    let cwds = workspace_cwds(&state.db)?;
    let outcome = super::run_on_blocking_worker(move || {
        let roots = extensions::Roots::current()?;
        let source = source.or_else(|| {
            extensions::find_skill_source(&roots, &cwds, &id)
                .map(|path| path.to_string_lossy().into_owned())
        });
        extensions::enable_skill_all(&roots, &cwds, &id, source.as_deref())
    })
    .await??;
    record_skill_source(&state, &outcome)?;
    Ok(extensions::OperationResult {
        message: outcome.message,
    })
}

#[tauri::command]
pub async fn disable_extension_skill_all(
    state: State<'_, AppState>,
    id: String,
) -> Result<extensions::OperationResult> {
    invalidate_catalog();
    let _catalog_reset = CatalogReset;
    let outcome = super::run_on_blocking_worker(move || {
        extensions::disable_skill_all(&extensions::Roots::current()?, &id)
    })
    .await??;
    save_skill_source(
        &state,
        &outcome.source_path,
        &outcome.name,
        &outcome.description,
    )?;
    Ok(extensions::OperationResult {
        message: outcome.message,
    })
}

#[tauri::command]
pub async fn extension_skill_migration_preview(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<extensions::SkillMigrationPreview> {
    let managed_root = managed_skills_dir(&app)?;
    let cwds = workspace_cwds(&state.db)?;
    let registry = extensions::load_skill_registry(&state.db)?;
    let source = extensions::find_source(&registry, &id).map(|source| source.path.clone());
    super::run_on_blocking_worker(move || {
        let roots = extensions::Roots::current()?;
        extensions::preview_skill_migration(&roots, &cwds, &managed_root, &id, source.as_deref())
    })
    .await?
}

#[tauri::command]
pub async fn migrate_extension_skill(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
    delete_source: bool,
) -> Result<extensions::SkillMigrationResult> {
    invalidate_catalog();
    let _catalog_reset = CatalogReset;
    let managed_root = managed_skills_dir(&app)?;
    let cwds = workspace_cwds(&state.db)?;
    let registry = extensions::load_skill_registry(&state.db)?;
    let source = extensions::find_source(&registry, &id).map(|source| source.path.clone());
    let old_id = id.clone();
    let outcome = super::run_on_blocking_worker(move || {
        let roots = extensions::Roots::current()?;
        extensions::migrate_skill(
            &roots,
            &cwds,
            &managed_root,
            &id,
            source.as_deref(),
            delete_source,
        )
    })
    .await??;
    let mut registry = extensions::load_skill_registry(&state.db)?;
    extensions::remove_source(&mut registry, &old_id);
    extensions::register_source(
        &mut registry,
        &outcome.source_path,
        &outcome.name,
        &outcome.description,
    );
    extensions::save_skill_registry(&state.db, &registry)?;
    Ok(outcome)
}

#[tauri::command]
pub async fn enable_extension_skill_projects(
    state: State<'_, AppState>,
    project_ids: Vec<String>,
    id: String,
) -> Result<extensions::OperationResult> {
    invalidate_catalog();
    let _catalog_reset = CatalogReset;
    let mut projects = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for project in project_ids {
        if !seen.insert(project.clone()) {
            continue;
        }
        let row = state
            .db
            .get_project(&project)?
            .ok_or_else(|| other("项目已不存在"))?;
        projects.push((row.id, row.name, project_paths(&state.db, &project)?));
    }
    if projects.is_empty() {
        return Err(other("请选择项目"));
    }
    let registry = extensions::load_skill_registry(&state.db)?;
    let source = extensions::find_source(&registry, &id).map(|source| source.path.clone());
    let all_cwds = workspace_cwds(&state.db)?;
    let (enabled, failures, first_source) = super::run_on_blocking_worker(
        move || -> Result<(usize, Vec<String>, Option<extensions::SkillInstallResult>)> {
            let roots = extensions::Roots::current()?;
            let source = source.or_else(|| {
                extensions::find_skill_source(&roots, &all_cwds, &id)
                    .map(|path| path.to_string_lossy().into_owned())
            });
            let mut enabled = 0;
            let mut failures = Vec::new();
            let mut first_source = None;
            for (_, project_name, paths) in projects {
                let cwds: Vec<PathBuf> = paths
                    .into_iter()
                    .map(|path| PathBuf::from(path.canonical_path))
                    .collect();
                if cwds.is_empty() {
                    failures.push(format!("{project_name}：没有可用的工作目录"));
                    continue;
                }
                match extensions::enable_skill_project(&roots, &cwds, &id, source.as_deref()) {
                    Ok(outcome) => {
                        enabled += 1;
                        if first_source.is_none() {
                            first_source = Some(outcome);
                        }
                    }
                    Err(error) => failures.push(format!("{project_name}：{error}")),
                }
            }
            Ok((enabled, failures, first_source))
        },
    )
    .await??;
    if let Some(outcome) = &first_source {
        record_skill_source(&state, outcome)?;
    }
    if enabled == 0 {
        return Err(other(format!("未能启用任何项目：{}", failures.join("；"))));
    }
    let mut message = format!("已为 {enabled} 个项目启用");
    if !failures.is_empty() {
        message.push_str(&format!("；失败：{}", failures.join("；")));
    }
    Ok(extensions::OperationResult { message })
}

#[tauri::command]
pub async fn disable_extension_skill_projects(
    state: State<'_, AppState>,
    project_ids: Vec<String>,
    id: String,
) -> Result<extensions::OperationResult> {
    invalidate_catalog();
    let _catalog_reset = CatalogReset;
    let mut projects = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for project in project_ids {
        if !seen.insert(project.clone()) {
            continue;
        }
        let row = state
            .db
            .get_project(&project)?
            .ok_or_else(|| other("项目已不存在"))?;
        projects.push((row.name, project_paths(&state.db, &project)?));
    }
    if projects.is_empty() {
        return Err(other("请选择项目"));
    }
    let (changed, failures) =
        super::run_on_blocking_worker(move || -> Result<(usize, Vec<String>)> {
            let roots = extensions::Roots::current()?;
            let mut changed = 0;
            let mut failures = Vec::new();
            for (project_name, paths) in projects {
                let cwds: Vec<PathBuf> = paths
                    .into_iter()
                    .map(|path| PathBuf::from(path.canonical_path))
                    .collect();
                if cwds.is_empty() {
                    failures.push(format!("{project_name}：没有可用的工作目录"));
                    continue;
                }
                match extensions::disable_skill_project(&roots, &cwds, &id) {
                    Ok(outcome) if outcome.changed => changed += 1,
                    Ok(_) => {}
                    Err(error) => failures.push(format!("{project_name}：{error}")),
                }
            }
            Ok((changed, failures))
        })
        .await??;
    if changed == 0 && !failures.is_empty() {
        return Err(other(format!("未能停用任何项目：{}", failures.join("；"))));
    }
    let mut message = if changed == 0 {
        "所选项目没有可停用的链接入口".into()
    } else {
        format!("已在 {changed} 个项目停用")
    };
    if !failures.is_empty() {
        message.push_str(&format!("；失败：{}", failures.join("；")));
    }
    Ok(extensions::OperationResult { message })
}

fn record_skill_source(
    state: &State<'_, AppState>,
    outcome: &extensions::SkillInstallResult,
) -> Result<()> {
    save_skill_source(
        state,
        &outcome.source_path,
        &outcome.name,
        &outcome.description,
    )
}

fn save_skill_source(
    state: &State<'_, AppState>,
    source_path: &str,
    name: &str,
    description: &str,
) -> Result<()> {
    let mut registry = extensions::load_skill_registry(&state.db)?;
    extensions::register_source(&mut registry, source_path, name, description);
    extensions::save_skill_registry(&state.db, &registry)
}

#[tauri::command]
pub async fn extension_skill_uninstall_preview(
    state: State<'_, AppState>,
    id: String,
) -> Result<extensions::UninstallPreview> {
    let cwds = workspace_cwds(&state.db)?;
    let registry = extensions::load_skill_registry(&state.db)?;
    let source = extensions::find_source(&registry, &id).cloned();
    super::run_on_blocking_worker(move || {
        let roots = extensions::Roots::current()?;
        match source {
            Some(source) => extensions::registered_uninstall_preview(&roots, &cwds, &source),
            None => extensions::uninstall_preview(&roots, &cwds, &id),
        }
    })
    .await?
}

#[tauri::command]
pub async fn uninstall_extension_skill(
    state: State<'_, AppState>,
    id: String,
    revision: String,
    delete_source: bool,
) -> Result<extensions::OperationResult> {
    invalidate_catalog();
    let _catalog_reset = CatalogReset;
    let cwds = workspace_cwds(&state.db)?;
    let registry = extensions::load_skill_registry(&state.db)?;
    let source = extensions::find_source(&registry, &id).cloned();
    let registered = source.is_some();
    let remove_id = id.clone();
    let result = super::run_on_blocking_worker(move || {
        let roots = extensions::Roots::current()?;
        match source {
            Some(source) => extensions::uninstall_registered_skill(
                &roots,
                &cwds,
                &source,
                &revision,
                delete_source,
            ),
            None => extensions::uninstall_skill(&roots, &cwds, &id, &revision, delete_source),
        }
    })
    .await??;
    if registered {
        let mut registry = extensions::load_skill_registry(&state.db)?;
        extensions::remove_source(&mut registry, &remove_id);
        extensions::save_skill_registry(&state.db, &registry)?;
    }
    Ok(result)
}

#[tauri::command]
pub async fn save_mcp_definition(
    state: State<'_, AppState>,
    name: String,
    description: Option<String>,
    config: serde_json::Value,
) -> Result<extensions::OperationResult> {
    invalidate_catalog();
    let _catalog_reset = CatalogReset;
    let id = extensions::store_mcp_definition(
        &state.db,
        &name,
        description.as_deref().unwrap_or_default(),
        config,
    )?;
    Ok(extensions::OperationResult {
        message: format!("MCP 定义已创建：{name}（{id}）"),
    })
}

#[tauri::command]
pub async fn enable_mcp_definition_user(
    state: State<'_, AppState>,
    id: String,
) -> Result<extensions::OperationResult> {
    invalidate_catalog();
    let _catalog_reset = CatalogReset;
    let registry = extensions::load_mcp_registry(&state.db)?;
    let cwds = workspace_cwds(&state.db)?;
    super::run_on_blocking_worker(move || {
        extensions::enable_mcp_user(&extensions::Roots::current()?, &registry, &id, &cwds)
    })
    .await?
}

#[tauri::command]
pub async fn disable_mcp_definition_user(
    state: State<'_, AppState>,
    id: String,
) -> Result<extensions::OperationResult> {
    invalidate_catalog();
    let _catalog_reset = CatalogReset;
    let registry = extensions::load_mcp_registry(&state.db)?;
    super::run_on_blocking_worker(move || {
        extensions::disable_mcp_user(&extensions::Roots::current()?, &registry, &id)
    })
    .await?
}

#[tauri::command]
pub async fn enable_mcp_definition_projects(
    state: State<'_, AppState>,
    project_ids: Vec<String>,
    id: String,
) -> Result<extensions::OperationResult> {
    invalidate_catalog();
    let _catalog_reset = CatalogReset;
    let cwds = selected_cwds(&state.db, &project_ids)?;
    let registry = extensions::load_mcp_registry(&state.db)?;
    super::run_on_blocking_worker(move || {
        extensions::enable_mcp_projects(&extensions::Roots::current()?, &registry, &id, &cwds)
    })
    .await?
}

#[tauri::command]
pub async fn disable_mcp_definition_projects(
    state: State<'_, AppState>,
    project_ids: Vec<String>,
    id: String,
) -> Result<extensions::OperationResult> {
    invalidate_catalog();
    let _catalog_reset = CatalogReset;
    let cwds = selected_cwds(&state.db, &project_ids)?;
    let registry = extensions::load_mcp_registry(&state.db)?;
    super::run_on_blocking_worker(move || {
        extensions::disable_mcp_projects(&extensions::Roots::current()?, &registry, &id, &cwds)
    })
    .await?
}

#[tauri::command]
pub async fn uninstall_mcp_definition(
    state: State<'_, AppState>,
    id: String,
) -> Result<extensions::OperationResult> {
    invalidate_catalog();
    let _catalog_reset = CatalogReset;
    let cwds = workspace_cwds(&state.db)?;
    let mut registry = extensions::load_mcp_registry(&state.db)?;
    let remove_id = id.clone();
    let result = super::run_on_blocking_worker(move || -> Result<extensions::OperationResult> {
        let roots = extensions::Roots::current()?;
        extensions::uninstall_mcp_definition(&mut registry, &roots, &id, &cwds)
    })
    .await??;
    extensions::forget_mcp_definition(&state.db, &remove_id)?;
    Ok(result)
}

fn selected_cwds(db: &Db, project_ids: &[String]) -> Result<Vec<PathBuf>> {
    let mut seen = std::collections::HashSet::new();
    let mut cwds = Vec::new();
    for project in project_ids {
        for path in project_paths(db, project)? {
            let path = PathBuf::from(path.canonical_path);
            if seen.insert(path.clone()) {
                cwds.push(path);
            }
        }
    }
    if cwds.is_empty() {
        return Err(other("项目没有可用的工作目录"));
    }
    Ok(cwds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_configuration_rejects_unregistered_or_unrelated_paths() {
        let root =
            std::env::temp_dir().join(format!("noending-extension-scope-{}", uuid::Uuid::new_v4()));
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

    #[test]
    fn catalog_cache_reuses_the_scan_until_invalidated() {
        let root =
            std::env::temp_dir().join(format!("noending-extension-cache-{}", uuid::Uuid::new_v4()));
        let home = root.join("home");
        let roots = extensions::Roots {
            codex: home.join(".codex"),
            claude: home.join(".claude"),
            pi: home.join(".pi/agent"),
            home: home.clone(),
        };
        let registry = extensions::SkillRegistry::default();
        let mcp = extensions::McpRegistry::default();
        let write = |name: &str| {
            let dir = home.join(".agents/skills").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: {name}\n---\n"),
            )
            .unwrap();
        };
        let managed = home.join("skills");
        write("one");
        assert_eq!(
            catalog(&roots, &[], &registry, &mcp, &managed)
                .resources
                .len(),
            1
        );
        write("two");
        assert_eq!(
            catalog(&roots, &[], &registry, &mcp, &managed)
                .resources
                .len(),
            1
        );
        invalidate_catalog();
        assert_eq!(
            catalog(&roots, &[], &registry, &mcp, &managed)
                .resources
                .len(),
            2
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
