//! Built-in path-backed Assistant project. Native Agent sessions own conversation
//! history; NoEnding supplies directory instructions and CLI access.
use crate::{
    domain::Project,
    error::{other, Result},
    storage::Db,
    workspace::{home::NoEndingHome, wiring::WorkspaceLayer},
};
use serde::Serialize;
use std::path::{Path, PathBuf};

static FILE_PREPARATION: std::sync::Mutex<()> = std::sync::Mutex::new(());

const SKILL: &str = include_str!("../../skills/noending-assistant/SKILL.md");
const CLI_DOC: &str = include_str!("../../../docs/cli.md");
const ENTRY: &str = "<!-- NoEnding managed assistant entry -->\n# 助手工作目录\n\n此目录承载 NoEnding 内置助手的原生 Agent 会话。开始工作前读取 skills/noending-assistant/SKILL.md 和 USER.md；使用 bin/ne（Windows 为 bin/ne.cmd）查询或编辑工作区；临时产物放 tmp/，自行使用唯一文件名。操作对象由用户消息明确，必要时通过 CLI 查询确认。用户自定义要求写在 USER.md，应用更新会保留它。\n";

#[derive(Debug, Clone, Serialize)]
pub struct AssistantWorkspace {
    pub project: Project,
    pub directory: String,
}
pub fn directory(home: &NoEndingHome) -> PathBuf {
    home.root.join("assistant")
}

fn write_managed(path: &Path, text: &str) -> Result<()> {
    if std::fs::symlink_metadata(path).is_ok_and(|m| !m.file_type().is_file()) {
        return Err(other(format!("助手文件不是普通文件：{}", path.display())));
    }
    if std::fs::read_to_string(path).ok().as_deref() != Some(text) {
        std::fs::write(path, text)?;
    }
    Ok(())
}
/// Install only managed resources; preserve USER.md and arbitrary user files.
pub fn prepare_files(home: &NoEndingHome) -> Result<()> {
    let _files = FILE_PREPARATION
        .lock()
        .map_err(|_| other("助手资源准备被中断，请重试"))?;
    let root = directory(home);
    let references = root.join("skills/noending-assistant/references");
    std::fs::create_dir_all(&references)?;
    std::fs::create_dir_all(root.join("tmp"))?;
    write_managed(&root.join("AGENTS.md"), ENTRY)?;
    write_managed(&root.join("CLAUDE.md"), "<!-- NoEnding managed assistant entry -->\n读取 AGENTS.md，并按它加载助手说明和用户自定义要求。\n")?;
    write_managed(&root.join("skills/noending-assistant/SKILL.md"), SKILL)?;
    write_managed(&references.join("cli.md"), CLI_DOC)?;
    use std::io::Write;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join("USER.md"))
    {
        Ok(mut file) => file.write_all(b"# Assistant instructions\n\n")?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    crate::cli::install_workspace_entry(
        &root.join("bin"),
        &home.runtime_dir.join("cli.json"),
        &crate::cli::executable()?,
    )
    .map_err(|e| other(e.to_string()))?;
    Ok(())
}
pub fn ensure_workspace(
    db: &Db,
    home: &NoEndingHome,
    layer: &WorkspaceLayer,
) -> Result<AssistantWorkspace> {
    prepare_files(home)?;
    let root = directory(home).to_string_lossy().to_string();
    let projection = layer.projection();
    let observation = projection.observer().observe(&root);
    let path =
        crate::workspace::project::ensure_workspace_path(db, &observation, projection.policy())?;
    let project = db
        .get_project(&path.project_id)?
        .ok_or_else(|| other("助手项目未注册"))?;
    Ok(AssistantWorkspace {
        project,
        directory: root.clone(),
    })
}
/// Codex sandbox needs file IPC access outside the working directory. Ordinary
/// launches keep their existing permissions; no per-conversation state is created.
pub fn cli_writable_directories(runtime: &Path, cwd: Option<&str>) -> Vec<String> {
    let Some(parent) = runtime.parent() else {
        return Vec::new();
    };
    let Some(home) = NoEndingHome::new(&parent.to_string_lossy(), None) else {
        return Vec::new();
    };
    let root = directory(&home);
    if cwd
        .is_some_and(|cwd| crate::workspace::identity::same_location(cwd, &root.to_string_lossy()))
    {
        vec![runtime.join("cli").to_string_lossy().into_owned()]
    } else {
        Vec::new()
    }
}
