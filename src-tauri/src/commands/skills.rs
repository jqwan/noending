//! Tauri boundary: project IDs are checked against the path registry before any
//! filesystem operation. No client-supplied destination path is trusted.
use super::AppState;
use crate::{
    error::{other, Result},
    skills::{self, catalog, invalidate_catalog, Scope},
    storage::Db,
};
use std::path::{Path, PathBuf};
use tauri::State;

/// Clears the catalog again when a mutation finishes so a read that ran while
/// files were changing cannot leave its pre-mutation snapshot cached.
struct CatalogReset;

impl Drop for CatalogReset {
    fn drop(&mut self) {
        invalidate_catalog();
    }
}

/// The NoEnding library is NoEnding-owned, so a Skill dropped into it is
/// registered by location; discovery keeps its metadata and lists sources that
/// currently have no Agent entry. Agent directories remain the only Locations.
fn sync_library_sources(db: &Db, managed_root: &Path) -> Result<()> {
    let mut registry = skills::load_skill_registry(db)?;
    if skills::discover_library_sources(managed_root, &mut registry)? {
        skills::save_skill_registry(db, &registry)?;
        invalidate_catalog();
    }
    Ok(())
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

fn projects_with_paths(db: &Db) -> Result<skills::ProjectPaths> {
    let mut rows = Vec::new();
    for project in db.list_projects()? {
        let paths = db.list_workspace_paths_for_project(&project.id)?;
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
/// scope for a single working directory.
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
pub async fn list_skills(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    scope: Scope,
) -> Result<skills::Inventory> {
    let managed_root = managed_skills_dir(&app)?;
    sync_library_sources(&state.db, &managed_root)?;
    let registry = skills::load_skill_registry(&state.db)?;
    let (cwd, project_cwds) = scope_paths(&state.db, &scope)?;
    super::run_on_blocking_worker(move || {
        let roots = skills::Roots::current()?;
        let mut inventory = {
            if let Some(paths) = project_cwds {
                let inventories = paths.iter().map(|path| skills::list(&roots, Some(path)));
                skills::merge_inventories(inventories.collect())
            } else {
                skills::list(&roots, cwd.as_deref())
            }
        };
        skills::merge_skill_sources(&registry, &mut inventory, true, &managed_root);
        Ok(inventory)
    })
    .await?
}

#[tauri::command]
pub async fn get_skill_detail(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<Option<skills::DetailBundle>> {
    let managed_root = managed_skills_dir(&app)?;
    sync_library_sources(&state.db, &managed_root)?;
    let skill_registry = skills::load_skill_registry(&state.db)?;
    let cwds = workspace_cwds(&state.db)?;
    let projects = projects_with_paths(&state.db)?;
    super::run_on_blocking_worker(move || {
        let roots = skills::Roots::current()?;
        let inventory = catalog(&roots, &cwds, &skill_registry, &managed_root);
        let Some(detail) = skills::detail_in_catalog(&inventory, &id)? else {
            return Ok(None);
        };
        let scopes = skills::project_scopes_from_inventory(&roots, &inventory, &projects, &id);
        let source_kind = Some(skills::skill_source_kind(&detail.resource, &managed_root));
        Ok(Some(skills::DetailBundle {
            detail,
            locations: inventory.locations,
            scopes,
            source_kind,
        }))
    })
    .await?
}

#[tauri::command]
pub async fn list_skill_catalog(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<skills::Inventory> {
    let managed_root = managed_skills_dir(&app)?;
    sync_library_sources(&state.db, &managed_root)?;
    let skill_registry = skills::load_skill_registry(&state.db)?;
    let cwds = workspace_cwds(&state.db)?;
    super::run_on_blocking_worker(move || {
        let roots = skills::Roots::current()?;
        Ok(catalog(&roots, &cwds, &skill_registry, &managed_root))
    })
    .await?
}

#[tauri::command]
pub async fn register_skill_source(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    source: String,
    migrate_source: bool,
) -> Result<skills::OperationResult> {
    invalidate_catalog();
    let _catalog_reset = CatalogReset;
    let managed_root = managed_skills_dir(&app)?;
    let cwds = workspace_cwds(&state.db)?;
    let outcome = super::run_on_blocking_worker(move || {
        let roots = skills::Roots::current()?;
        skills::prepare_source(
            &roots,
            &cwds,
            &managed_root,
            Path::new(&source),
            migrate_source,
        )
    })
    .await??;
    save_skill_source(
        &state,
        &outcome.source_path,
        &outcome.name,
        &outcome.description,
    )?;
    Ok(skills::OperationResult {
        message: outcome.message,
    })
}

#[tauri::command]
pub async fn unregister_skill(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
    remove_links: bool,
) -> Result<skills::OperationResult> {
    invalidate_catalog();
    let _catalog_reset = CatalogReset;
    let managed_root = managed_skills_dir(&app)?;
    let cwds = workspace_cwds(&state.db)?;
    let mut registry = skills::load_skill_registry(&state.db)?;
    let remove_id = id.clone();
    let result = super::run_on_blocking_worker(move || {
        skills::unregister_skill(
            &skills::Roots::current()?,
            &cwds,
            &managed_root,
            &mut registry,
            &id,
            remove_links,
        )
    })
    .await??;
    let mut registry = skills::load_skill_registry(&state.db)?;
    skills::remove_source(&mut registry, &remove_id);
    skills::save_skill_registry(&state.db, &registry)?;
    Ok(result)
}

#[tauri::command]
pub async fn enable_skill_all(
    state: State<'_, AppState>,
    id: String,
) -> Result<skills::OperationResult> {
    invalidate_catalog();
    let _catalog_reset = CatalogReset;
    let registry = skills::load_skill_registry(&state.db)?;
    let source = skills::find_source(&registry, &id).map(|source| source.path.clone());
    let cwds = workspace_cwds(&state.db)?;
    let outcome = super::run_on_blocking_worker(move || {
        let roots = skills::Roots::current()?;
        let source = source.or_else(|| {
            skills::find_skill_source(&roots, &cwds, &id)
                .map(|path| path.to_string_lossy().into_owned())
        });
        skills::enable_skill_all(&roots, &cwds, &id, source.as_deref())
    })
    .await??;
    record_skill_source(&state, &outcome)?;
    Ok(skills::OperationResult {
        message: outcome.message,
    })
}

#[tauri::command]
pub async fn disable_skill_all(
    state: State<'_, AppState>,
    id: String,
) -> Result<skills::OperationResult> {
    invalidate_catalog();
    let _catalog_reset = CatalogReset;
    let outcome = super::run_on_blocking_worker(move || {
        skills::disable_skill_all(&skills::Roots::current()?, &id)
    })
    .await??;
    save_skill_source(
        &state,
        &outcome.source_path,
        &outcome.name,
        &outcome.description,
    )?;
    Ok(skills::OperationResult {
        message: outcome.message,
    })
}

#[tauri::command]
pub async fn skill_registration_preview(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<skills::SkillMigrationPreview> {
    let managed_root = managed_skills_dir(&app)?;
    let cwds = workspace_cwds(&state.db)?;
    let registry = skills::load_skill_registry(&state.db)?;
    let source = skills::find_source(&registry, &id).map(|source| source.path.clone());
    super::run_on_blocking_worker(move || {
        let roots = skills::Roots::current()?;
        skills::preview_skill_registration(&roots, &cwds, &managed_root, &id, source.as_deref())
    })
    .await?
}

#[tauri::command]
pub async fn register_discovered_skill(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
    migrate_source: bool,
) -> Result<skills::SkillMigrationResult> {
    invalidate_catalog();
    let _catalog_reset = CatalogReset;
    let managed_root = managed_skills_dir(&app)?;
    let cwds = workspace_cwds(&state.db)?;
    let registry = skills::load_skill_registry(&state.db)?;
    let source = skills::find_source(&registry, &id).map(|source| source.path.clone());
    let old_id = id.clone();
    let outcome = super::run_on_blocking_worker(move || -> Result<skills::SkillMigrationResult> {
        let roots = skills::Roots::current()?;
        let source = source
            .map(PathBuf::from)
            .or_else(|| skills::find_skill_source(&roots, &cwds, &id))
            .ok_or_else(|| other("Skill 已不存在"))?;
        let outcome =
            skills::prepare_source(&roots, &cwds, &managed_root, &source, migrate_source)?;
        Ok(skills::SkillMigrationResult {
            resource_id: skills::source_id(&outcome.source_path),
            source_path: outcome.source_path,
            message: outcome.message,
            name: outcome.name,
            description: outcome.description,
        })
    })
    .await??;
    let mut registry = skills::load_skill_registry(&state.db)?;
    skills::remove_source(&mut registry, &old_id);
    skills::register_source(
        &mut registry,
        &outcome.source_path,
        &outcome.name,
        &outcome.description,
    );
    skills::save_skill_registry(&state.db, &registry)?;
    Ok(outcome)
}

#[tauri::command]
pub async fn enable_skill_projects(
    state: State<'_, AppState>,
    project_ids: Vec<String>,
    id: String,
) -> Result<skills::OperationResult> {
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
    let registry = skills::load_skill_registry(&state.db)?;
    let source = skills::find_source(&registry, &id).map(|source| source.path.clone());
    let all_cwds = workspace_cwds(&state.db)?;
    let (enabled, failures, first_source) = super::run_on_blocking_worker(
        move || -> Result<(usize, Vec<String>, Option<skills::SkillInstallResult>)> {
            let roots = skills::Roots::current()?;
            let source = source.or_else(|| {
                skills::find_skill_source(&roots, &all_cwds, &id)
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
                match skills::enable_skill_project(&roots, &cwds, &id, source.as_deref()) {
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
    Ok(skills::OperationResult { message })
}

#[tauri::command]
pub async fn disable_skill_projects(
    state: State<'_, AppState>,
    project_ids: Vec<String>,
    id: String,
) -> Result<skills::OperationResult> {
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
            let roots = skills::Roots::current()?;
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
                match skills::disable_skill_project(&roots, &cwds, &id) {
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
    Ok(skills::OperationResult { message })
}

fn record_skill_source(
    state: &State<'_, AppState>,
    outcome: &skills::SkillInstallResult,
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
    let mut registry = skills::load_skill_registry(&state.db)?;
    skills::register_source(&mut registry, source_path, name, description);
    skills::save_skill_registry(&state.db, &registry)
}

#[tauri::command]
pub async fn skill_uninstall_preview(
    state: State<'_, AppState>,
    id: String,
) -> Result<skills::UninstallPreview> {
    let cwds = workspace_cwds(&state.db)?;
    let registry = skills::load_skill_registry(&state.db)?;
    let source = skills::find_source(&registry, &id).cloned();
    super::run_on_blocking_worker(move || {
        let roots = skills::Roots::current()?;
        match source {
            Some(source) => skills::registered_uninstall_preview(&roots, &cwds, &source),
            None => skills::uninstall_preview(&roots, &cwds, &id),
        }
    })
    .await?
}

#[tauri::command]
pub async fn uninstall_skill(
    state: State<'_, AppState>,
    id: String,
    revision: String,
    delete_source: bool,
) -> Result<skills::OperationResult> {
    invalidate_catalog();
    let _catalog_reset = CatalogReset;
    let cwds = workspace_cwds(&state.db)?;
    let registry = skills::load_skill_registry(&state.db)?;
    let source = skills::find_source(&registry, &id).cloned();
    let registered = source.is_some();
    let remove_id = id.clone();
    let result = super::run_on_blocking_worker(move || {
        let roots = skills::Roots::current()?;
        match source {
            Some(source) => {
                skills::uninstall_registered_skill(&roots, &cwds, &source, &revision, delete_source)
            }
            None => skills::uninstall_skill(&roots, &cwds, &id, &revision, delete_source),
        }
    })
    .await??;
    if registered {
        let mut registry = skills::load_skill_registry(&state.db)?;
        skills::remove_source(&mut registry, &remove_id);
        skills::save_skill_registry(&state.db, &registry)?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_configuration_rejects_unregistered_or_unrelated_paths() {
        let root =
            std::env::temp_dir().join(format!("noending-skill-scope-{}", uuid::Uuid::new_v4()));
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
            std::env::temp_dir().join(format!("noending-skill-cache-{}", uuid::Uuid::new_v4()));
        let home = root.join("home");
        let roots = skills::Roots {
            codex: home.join(".codex"),
            claude: home.join(".claude"),
            pi: home.join(".pi/agent"),
            home: home.clone(),
        };
        let registry = skills::SkillRegistry::default();
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
        assert_eq!(catalog(&roots, &[], &registry, &managed).resources.len(), 1);
        write("two");
        assert_eq!(catalog(&roots, &[], &registry, &managed).resources.len(), 1);
        invalidate_catalog();
        assert_eq!(catalog(&roots, &[], &registry, &managed).resources.len(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }
}
