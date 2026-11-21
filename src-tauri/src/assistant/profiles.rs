use super::*;
use crate::{
    domain::*,
    error::{other, Result},
    storage::{assistant_profiles as repo, new_id, Db},
    workspace::{project, wiring::WorkspaceLayer},
};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) fn lock(home: &NoEndingHome) -> Result<std::fs::File> {
    fs::create_dir_all(&home.runtime_dir)?;
    Ok(crate::platform::file_lock::exclusive_wait(
        &home.runtime_dir.join("assistant-profiles.lock"),
        std::time::Duration::from_secs(10),
    )?)
}
fn registered_path(db: &Db, project_id: &str) -> Result<WorkspacePath> {
    let conn = db.read();
    let path_id =
        repo::path_for_project(&conn, project_id)?.ok_or_else(|| other("项目没有智能体设置"))?;
    crate::storage::workspace::get_workspace_path_conn(&conn, &path_id)?
        .ok_or_else(|| other("智能体档案目录未登记"))
}
fn ensure_profile_parent(home: &NoEndingHome) -> Result<()> {
    fs::create_dir_all(directory(home))?;
    let actual = fs::canonicalize(directory(home))?;
    let expected = fs::canonicalize(&home.root)?.join("assistant");
    if crate::workspace::path_identity_of(&actual.to_string_lossy())
        != crate::workspace::path_identity_of(&expected.to_string_lossy())
    {
        return Err(other("智能体空间目录指向其他位置，未修改目录"));
    }
    Ok(())
}
fn expected_directory(home: &NoEndingHome, path: &WorkspacePath) -> Result<PathBuf> {
    let name = Path::new(&path.canonical_path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| uuid::Uuid::parse_str(name).is_ok())
        .ok_or_else(|| other("智能体档案目录名称无效"))?;
    let expected = directory(home).join(name);
    if crate::workspace::path_identity_of(&expected.to_string_lossy()).as_deref() != Some(&path.id)
    {
        return Err(other("智能体档案目录位置已变化"));
    }
    Ok(expected)
}

fn validate_directory(home: &NoEndingHome, path: &WorkspacePath) -> Result<PathBuf> {
    let expected = expected_directory(home, path)?;
    let metadata = fs::symlink_metadata(&expected)?;
    if !metadata.is_dir() {
        return Err(other("智能体档案目录已被替换"));
    }
    let canonical = fs::canonicalize(&expected)?;
    let parent = fs::canonicalize(directory(home))?;
    let home_parent = fs::canonicalize(&home.root)?.join("assistant");
    if crate::workspace::path_identity_of(&parent.to_string_lossy())
        != crate::workspace::path_identity_of(&home_parent.to_string_lossy())
        || canonical.parent() != Some(parent.as_path())
    {
        return Err(other("智能体档案目录位置已变化或不属于当前 Home"));
    }
    Ok(canonical)
}

pub(crate) struct StagedProfileDirectory {
    original: PathBuf,
    staged: PathBuf,
}

impl StagedProfileDirectory {
    pub(crate) fn restore(self) -> Result<()> {
        if fs::symlink_metadata(&self.original).is_ok() {
            return Err(other(format!(
                "原目录已被占用，智能体文件保留在 {}",
                self.staged.display()
            )));
        }
        fs::rename(&self.staged, &self.original).map_err(|error| {
            other(format!(
                "无法恢复智能体档案目录 {}：{error}",
                self.staged.display()
            ))
        })
    }

    pub(crate) fn delete(self) -> Result<()> {
        fs::remove_dir_all(&self.staged).map_err(|error| {
            other(format!(
                "智能体档案目录未能完全删除，请检查 {}：{error}",
                self.staged.display()
            ))
        })
    }
}

pub(crate) fn stage_profile_directory(
    home: &NoEndingHome,
    path: &WorkspacePath,
) -> Result<Option<StagedProfileDirectory>> {
    let expected = expected_directory(home, path)?;
    match fs::symlink_metadata(&expected) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(metadata) if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() => {
            return Err(other("智能体档案目录已被替换，无法删除目录"));
        }
        Ok(_) => {}
    }
    let original = validate_directory(home, path)?;
    let staged = original.with_file_name(format!(".deleting-{}", new_id()));
    fs::rename(&original, &staged)?;
    Ok(Some(StagedProfileDirectory { original, staged }))
}

fn prepare_default_skill(
    db: &Db,
    home: &NoEndingHome,
    roots: &crate::skills::Roots,
    cwd: &Path,
) -> Result<()> {
    crate::skills::prepare_noending_skill(db, home, &crate::cli::executable()?)?;
    let registry = crate::skills::load_skill_registry(db)?;
    let source = registry
        .sources
        .iter()
        .find(|s| s.name == "noending")
        .ok_or_else(|| other("NoEnding 技能未安装，请在技能管理中恢复"))?;
    crate::skills::prepare_project_skill_entries(
        roots,
        &[cwd.to_path_buf()],
        &source.id,
        Some(&source.path),
    )?;
    crate::skills::invalidate_catalog();
    Ok(())
}
pub fn create_profile(
    db: &Db,
    home: &NoEndingHome,
    layer: &WorkspaceLayer,
    input: AssistantCreate,
) -> Result<Project> {
    create_profile_with_roots(db, home, layer, input, &crate::skills::Roots::current()?)
}
/// Explicit roots keep native loading locations isolated in tests and local QA.
pub fn create_profile_with_roots(
    db: &Db,
    home: &NoEndingHome,
    layer: &WorkspaceLayer,
    input: AssistantCreate,
    roots: &crate::skills::Roots,
) -> Result<Project> {
    let _access = home.acquire_access()?;
    let _lock = lock(home)?;
    crate::storage::ensure_not_empty("智能体名称", &input.name)?;
    let id = new_id();
    ensure_profile_parent(home)?;
    let root = directory(home).join(&id);
    let staging = home.runtime_dir.join(format!("assistant-create-{id}"));
    fs::create_dir(&staging)?;
    let preparation = (|| {
        files::initialize(&staging, &input.agents_md)?;
        prepare_default_skill(db, home, roots, &staging)
    })();
    if let Err(error) = preparation {
        // This unpublished staging directory was allocated by this call in the
        // application's runtime area and contains only its generated entries.
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    if root.exists() {
        let _ = fs::remove_dir_all(&staging);
        return Err(other("智能体档案目录已存在，未覆盖原内容"));
    }
    if let Err(error) = fs::rename(&staging, &root) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error.into());
    }
    let projection = layer.projection();
    let observation = projection.observer().observe(&root.to_string_lossy());
    db.tx(|tx| {
        let path = project::ensure_workspace_path_conn(tx, &observation, projection.policy())?.path;
        let project =
            crate::storage::workspace::rename_project_conn(tx, &path.project_id, &input.name)?;
        repo::register(tx, &path)?;
        Ok(project)
    })
}

pub(crate) fn project_assistant(
    db: &Db,
    project_id: &str,
    paths: &[WorkspacePath],
) -> Result<Option<ProjectAssistantDetail>> {
    let path_id = repo::path_for_project(&db.read(), project_id)?;
    path_id
        .map(|id| {
            let path = paths
                .iter()
                .find(|path| path.id == id)
                .ok_or_else(|| other("智能体档案目录未登记"))?;
            Ok(ProjectAssistantDetail {
                file: files::inspect(Path::new(&path.canonical_path)),
                workspace_path_id: id,
            })
        })
        .transpose()
}

pub fn update_profile(
    db: &Db,
    home: &NoEndingHome,
    project_id: &str,
    revision: &str,
    patch: AssistantPatch,
) -> Result<ProjectAssistant> {
    let _lock = lock(home)?;
    let path = registered_path(db, project_id)?;
    let root = validate_directory(home, &path)?;
    files::save(&root, revision, &patch.agents_md)?;
    Ok(files::inspect(&root))
}

pub fn prepare_profile_files(
    db: &Db,
    home: &NoEndingHome,
    project_id: &str,
) -> Result<ProjectAssistant> {
    let _lock = lock(home)?;
    let path = registered_path(db, project_id)?;
    let expected = expected_directory(home, &path)?;
    if let Err(error) = fs::symlink_metadata(&expected) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(error.into());
        }
        ensure_profile_parent(home)?;
        fs::create_dir(&expected)?;
    }
    let root = validate_directory(home, &path)?;
    files::initialize(&root, "")?;
    Ok(files::inspect(&root))
}
