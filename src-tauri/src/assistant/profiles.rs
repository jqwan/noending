use super::*;
use crate::{
    domain::*,
    error::{other, AppError, Result},
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
    registered_path_conn(&db.read(), project_id)?.ok_or_else(|| other("项目没有智能体设置"))
}

fn registered_path_conn(
    conn: &rusqlite::Connection,
    project_id: &str,
) -> Result<Option<WorkspacePath>> {
    let Some(path_id) = repo::path_for_project(conn, project_id)? else {
        return Ok(None);
    };
    crate::storage::workspace::get_workspace_path_conn(conn, &path_id)?
        .map(Some)
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
fn archive_identity(path: &WorkspacePath) -> Result<(&str, uuid::Uuid)> {
    let name = Path::new(&path.canonical_path)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| other("智能体档案目录名称无效"))?;
    let id = uuid::Uuid::parse_str(name).map_err(|_| other("智能体档案目录名称无效"))?;
    Ok((name, id))
}

pub(crate) fn archive_id(path: &str) -> Option<uuid::Uuid> {
    uuid::Uuid::parse_str(Path::new(path).file_name()?.to_str()?).ok()
}

pub(crate) fn archive_path_id(home: &NoEndingHome, id: uuid::Uuid) -> Result<String> {
    crate::workspace::path_identity_of(&directory(home).join(id.to_string()).to_string_lossy())
        .ok_or_else(|| other("智能体档案目录无效"))
}

fn session_archive(conn: &rusqlite::Connection, session: &str) -> Result<Option<WorkspacePath>> {
    use rusqlite::OptionalExtension;
    let path = conn.query_row(
        "SELECT a.workspace_path_id FROM sessions s JOIN project_assistants a ON a.workspace_path_id=s.workspace_path_id WHERE s.id=?1",
        [session], |row| row.get::<_, String>(0),
    ).optional()?;
    match path {
        Some(path) => crate::storage::workspace::get_workspace_path_conn(conn, &path),
        None => Ok(None),
    }
}

pub(crate) fn session_assistant_id(db: &Db, session: &str) -> Result<Option<uuid::Uuid>> {
    session_archive(&db.read(), session)?
        .map(|path| archive_identity(&path).map(|(_, id)| id))
        .transpose()
}

pub(crate) fn list_public_assistants(
    db: &Db,
    name: Option<&str>,
) -> Result<Vec<AssistantDirectoryEntry>> {
    let filter = name.unwrap_or_default().trim().to_lowercase();
    let rows = repo::active_names_and_paths(&db.read())?;
    rows.into_iter()
        .filter(|(name, _)| filter.is_empty() || name.to_lowercase().contains(&filter))
        .map(|(name, path)| {
            let id = archive_id(&path).ok_or_else(|| other("智能体档案目录名称无效"))?;
            Ok(AssistantDirectoryEntry { id, name })
        })
        .collect()
}

pub(crate) fn public_card(
    db: &Db,
    home: &NoEndingHome,
    id: uuid::Uuid,
) -> Result<Option<AssistantCard>> {
    let (name, path) = {
        let conn = db.read();
        let path_id = archive_path_id(home, id)?;
        let Some(project_id) = repo::project_for_path(&conn, &path_id)? else {
            return Ok(None);
        };
        let project = crate::storage::workspace::get_project_conn(&conn, &project_id)?
            .ok_or_else(|| other("智能体项目不存在"))?;
        let path = crate::storage::workspace::get_workspace_path_conn(&conn, &path_id)?
            .ok_or_else(|| other("智能体档案目录未登记"))?;
        (project.name, path)
    };
    let intro = validate_directory(home, &path)
        .ok()
        .map(|root| files::read_intro(&root))
        .unwrap_or_default();
    Ok(Some(AssistantCard { id, name, intro }))
}

pub(crate) fn private_profile(
    db: &Db,
    home: &NoEndingHome,
    session: &str,
) -> Result<Option<AssistantPrivateProfile>> {
    let Some(path) = session_archive(&db.read(), session)? else {
        return Ok(None);
    };
    let id = archive_identity(&path)?.1;
    let root = validate_directory(home, &path)?;
    Ok(Some(AssistantPrivateProfile {
        id,
        files: files::inspect(&root),
    }))
}

fn expected_directory(home: &NoEndingHome, path: &WorkspacePath) -> Result<PathBuf> {
    let (name, _) = archive_identity(path)?;
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
        files::initialize(&staging, input.soul_md.as_deref(), input.role_md.as_deref())?;
        files::sync_identity(&staging, &id, input.name.trim())?;
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
                files: files::inspect(Path::new(&path.canonical_path)),
                id: archive_identity(path)?.1,
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
    files::save(&root, revision, patch)?;
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
    let project = db
        .get_project(project_id)?
        .ok_or_else(|| other("智能体项目不存在"))?;
    files::sync_identity(
        &root,
        &archive_identity(&path)?.1.to_string(),
        &project.name,
    )?;
    files::initialize(&root, None, None)?;
    Ok(files::inspect(&root))
}

/// Synchronize a generated identity while the caller holds the archive lock.
pub(crate) fn sync_project_identity(
    conn: &rusqlite::Connection,
    home: &NoEndingHome,
    project: &Project,
) -> Result<bool> {
    let Some(path) = registered_path_conn(conn, &project.id)? else {
        return Ok(false);
    };
    let root = match validate_directory(home, &path) {
        Ok(root) => root,
        Err(AppError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(false)
        }
        Err(error) => return Err(error),
    };
    files::sync_identity(
        &root,
        &archive_identity(&path)?.1.to_string(),
        &project.name,
    )?;
    Ok(true)
}

/// Repair generated identities in existing archives without resetting their guides.
pub(crate) fn refresh_identities(db: &Db, home: &NoEndingHome) -> Result<()> {
    let _lock = lock(home)?;
    let conn = db.read();
    let project_ids = {
        let mut stmt = conn.prepare("SELECT project_id FROM project_assistants")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for project_id in project_ids {
        if let Some(project) = crate::storage::workspace::get_project_conn(&conn, &project_id)? {
            if let Err(error) = sync_project_identity(&conn, home, &project) {
                eprintln!(
                    "[noending] could not update assistant identity for {project_id}: {error}"
                );
            }
        }
    }
    Ok(())
}

/// Resolve the identity of the registered archive directory in this Home.
pub(crate) fn resolve_archive(
    db: &Db,
    home: &NoEndingHome,
    project_id: &str,
    path: &str,
) -> Result<Option<uuid::Uuid>> {
    let Some(path_id) = crate::workspace::path_identity_of(path) else {
        return Ok(None);
    };
    let conn = db.read();
    if repo::project_for_path(&conn, &path_id)?.as_deref() != Some(project_id) {
        return Ok(None);
    }
    let Some(path) = crate::storage::workspace::get_workspace_path_conn(&conn, &path_id)? else {
        return Ok(None);
    };
    drop(conn);
    if validate_directory(home, &path).is_err() {
        return Ok(None);
    }
    archive_identity(&path).map(|(_, id)| Some(id))
}
