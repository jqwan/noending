//! Application-supplied optional capabilities use ordinary Skill sources and
//! native loading entries. Preparing content never enables a scope.
use super::{load_skill_registry, register_source, register_source_dir};
use crate::{
    error::{other, Result},
    storage::Db,
    workspace::{home::NoEndingHome, identity},
};
use std::{
    fs,
    path::{Path, PathBuf},
};

const NAME: &str = "noending";
const INITIALIZED: &str = "skills.noending_initialized";

pub(crate) const FILES: &[(&str, &str)] = &[
    (
        "references/access.json",
        include_str!("../../skills/noending/references/access.json"),
    ),
    ("SKILL.md", include_str!("../../skills/noending/SKILL.md")),
    (
        "references/access.md",
        include_str!("../../skills/noending/references/access.md"),
    ),
    (
        "references/identity.md",
        include_str!("../../skills/noending/references/identity.md"),
    ),
    (
        "references/memory.md",
        include_str!("../../skills/noending/references/memory.md"),
    ),
    (
        "references/task.md",
        include_str!("../../skills/noending/references/task.md"),
    ),
    (
        "references/mailbox.md",
        include_str!("../../skills/noending/references/mailbox.md"),
    ),
    (
        "references/extraction.md",
        include_str!("../../skills/noending/references/extraction.md"),
    ),
    (
        "references/extraction-protocol.md",
        include_str!("../../skills/noending/references/extraction-protocol.md"),
    ),
];

fn owned(path: &Path) -> bool {
    let marker = path.join(format!(".{NAME}"));
    fs::symlink_metadata(&marker).is_ok_and(|m| m.file_type().is_file())
        && fs::read_to_string(marker).ok().as_deref() == Some(NAME)
}

fn registered_directory(registry: &super::SkillRegistry) -> Option<PathBuf> {
    registry
        .sources
        .iter()
        .filter(|s| s.name == NAME)
        .filter_map(|s| Path::new(&s.path).parent())
        .find(|path| owned(path))
        .map(Path::to_path_buf)
}

fn directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_dir() => Ok(()),
        Ok(_) => Err(other(format!(
            "内置技能路径不是普通目录：{}",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path)?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn write(path: &Path, text: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.file_type().is_file() => {
            return Err(other(format!(
                "内置技能文件不是普通文件：{}",
                path.display()
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if fs::read_to_string(path).ok().as_deref() != Some(text) {
        fs::write(path, text)?;
    }
    Ok(())
}

/// Update only application-owned files. A removed installed skill stays removed;
/// a same-named user source is never adopted or overwritten automatically.
pub fn prepare_noending_skill(db: &Db, home: &NoEndingHome, executable: &Path) -> Result<()> {
    let _access = home.acquire_access()?;
    let _mutation = super::MUTATION
        .lock()
        .map_err(|_| other("技能配置操作锁不可用"))?;
    let initialized = db.get_setting(INITIALIZED)?.is_some();
    let mut registry = load_skill_registry(db)?;
    let library = home.root.join("skills");
    // Home relocation leaves Skill sources and native project links in place.
    // Refresh the installed source's binding to the current database Home.
    let target = registered_directory(&registry).unwrap_or_else(|| library.join(NAME));
    if initialized && !target.join("SKILL.md").is_file() {
        return Ok(());
    }
    directory(&library)?;
    if fs::symlink_metadata(&target).is_ok() {
        directory(&target)?;
        if !owned(&target) {
            return Err(other(format!(
                "同名技能目录已存在，保留原内容：{}",
                target.display()
            )));
        }
    } else {
        directory(&target)?;
        write(&target.join(format!(".{NAME}")), NAME)?;
    }
    let references = target.join("references");
    let bin = target.join("bin");
    directory(&references)?;
    directory(&bin)?;
    crate::cli::install_workspace_entry(&bin, &home.root, executable)
        .map_err(|e| other(e.to_string()))?;
    #[cfg(windows)]
    let entry = bin.join("ne.cmd");
    #[cfg(not(windows))]
    let entry = bin.join("ne");
    let display = |path: &Path| {
        identity::normalize_path(&path.to_string_lossy())
            .ok_or_else(|| other("技能入口路径必须是绝对路径"))
    };
    let access = serde_json::json!({"home":display(&home.root)?,"cli":display(&entry)?});
    write(
        &references.join("access.json"),
        &serde_json::to_string_pretty(&access)?,
    )?;
    for (relative, content) in FILES {
        if *relative != "references/access.json" {
            write(&target.join(relative), content)?;
        }
    }
    let source = register_source_dir(&target)?;
    let before = registry.clone();
    register_source(
        &mut registry,
        &source.source_path,
        &source.name,
        &source.description,
    );
    if registry != before {
        super::save_skill_registry(db, &registry)?;
    }
    if !initialized {
        db.set_setting(INITIALIZED, "true")?;
    }
    super::invalidate_catalog();
    Ok(())
}
