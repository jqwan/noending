use super::{
    files, registry::source_is_managed, Inventory, Location, OperationResult, Resource, Roots,
};
use crate::{
    domain::Agent,
    error::{other, Result},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
struct Metadata {
    name: String,
    description: String,
}

fn is_link(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        fs::symlink_metadata(path).is_ok_and(|m| m.file_attributes() & 0x400 != 0)
    }
    #[cfg(not(windows))]
    {
        path.is_symlink()
    }
}

/// A link entry can be removed without touching its target. A real directory
/// contains the files the Agent reads, even when an ancestor is a link.
pub(super) fn entry_is_link(entry: &Path) -> bool {
    entry.parent().is_some_and(is_link)
}

/// Whether an entry can be removed without deleting Skill content: a link, or
/// a legacy wrapper directory that only exposes links into the source.
pub(super) fn entry_is_removable(entry: &Path) -> bool {
    entry_is_link(entry) || entry_is_wrapper(entry)
}

/// Some installers expose a Skill as a real directory whose only content links
/// back into the source. Replacing that directory with a single link keeps the
/// entry working and loses nothing.
fn entry_is_wrapper(entry: &Path) -> bool {
    let Some(dir) = entry.parent() else {
        return false;
    };
    if is_link(dir) || !is_link(entry) {
        return false;
    }
    let (Ok(target), Ok(real_dir)) = (fs::canonicalize(entry), fs::canonicalize(dir)) else {
        return false;
    };
    let Some(source_dir) = target.parent() else {
        return false;
    };
    real_dir != source_dir && links_inside(dir, source_dir)
}

fn links_inside(dir: &Path, source_dir: &Path) -> bool {
    let Ok(items) = fs::read_dir(dir) else {
        return false;
    };
    for item in items.flatten() {
        let path = item.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            return false;
        };
        if meta.file_type().is_symlink() {
            match fs::canonicalize(&path) {
                Ok(target) if target.starts_with(source_dir) => {}
                _ => return false,
            }
        } else if !meta.is_dir() || !links_inside(&path, source_dir) {
            return false;
        }
    }
    true
}

/// Extension adapters exposed by the extension board. `Agent::all()` also
/// contains history-only adapters that never scan skill directories.
const ENABLE_AGENTS: [Agent; 4] = [
    Agent::Codex,
    Agent::ClaudeCode,
    Agent::Pi,
    Agent::Antigravity,
];

fn supports_user_agent(loc: &Location, agent: Agent) -> bool {
    loc.agent == Some(agent) || (loc.agent.is_none() && matches!(agent, Agent::Codex | Agent::Pi))
}

fn supports_project_agent(loc: &Location, agent: Agent) -> bool {
    loc.agent == Some(agent)
        || (loc.agent.is_none() && matches!(agent, Agent::Codex | Agent::Pi | Agent::Antigravity))
}

fn supports_scope_agent(loc: &Location, scope: &str, agent: Agent) -> bool {
    if scope == "project" {
        supports_project_agent(loc, agent)
    } else {
        supports_user_agent(loc, agent)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillScopeEntry {
    pub path: String,
    pub link: bool,
    pub label: String,
    pub agents: Vec<Agent>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectWorkspaceScope {
    pub workspace_path_id: String,
    pub path: String,
    pub entries: Vec<SkillScopeEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectSkillScope {
    pub project_id: String,
    pub project_name: String,
    pub workspaces: Vec<ProjectWorkspaceScope>,
    pub enabled: bool,
    pub full: bool,
}

/// Group a catalog row's project entries by project and working directory.
/// This reuses the catalog scan, so the detail page never rescans every path.
pub fn project_scopes_from_inventory(
    roots: &Roots,
    inventory: &Inventory,
    projects: &[(String, String, Vec<(String, PathBuf)>)],
    id: &str,
) -> Vec<ProjectSkillScope> {
    let row = inventory.resources.iter().find(|row| row.id == id);
    let kind = row.map(|row| row.kind.as_str()).unwrap_or("skill");
    projects
        .iter()
        .map(|(project_id, project_name, paths)| {
            let workspaces: Vec<ProjectWorkspaceScope> = paths
                .iter()
                .map(|(workspace_path_id, path)| {
                    let project_locations: Vec<String> =
                        super::inventory::locations(roots, Some(path))
                            .into_iter()
                            .filter(|loc| loc.kind == kind && loc.scope == "project")
                            .map(|loc| loc.id)
                            .collect();
                    let entries = row
                        .map(|row| {
                            row.entry_paths
                                .iter()
                                .enumerate()
                                .filter_map(|(index, entry)| {
                                    let location_id = row.locations.get(index)?;
                                    if !project_locations.contains(location_id) {
                                        return None;
                                    }
                                    let loc = inventory
                                        .locations
                                        .iter()
                                        .find(|loc| &loc.id == location_id)?;
                                    Some(SkillScopeEntry {
                                        path: entry.clone(),
                                        link: row.entry_links.get(index).copied().unwrap_or(false),
                                        label: loc.label.clone(),
                                        agents: ENABLE_AGENTS
                                            .iter()
                                            .copied()
                                            .filter(|agent| {
                                                supports_scope_agent(loc, "project", *agent)
                                            })
                                            .collect(),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    ProjectWorkspaceScope {
                        workspace_path_id: workspace_path_id.clone(),
                        path: path.to_string_lossy().into_owned(),
                        entries,
                    }
                })
                .collect();
            let enabled = workspaces
                .iter()
                .any(|workspace| !workspace.entries.is_empty());
            let full = !workspaces.is_empty()
                && workspaces
                    .iter()
                    .all(|workspace| !workspace.entries.is_empty());
            ProjectSkillScope {
                project_id: project_id.clone(),
                project_name: project_name.clone(),
                workspaces,
                enabled,
                full,
            }
        })
        .collect()
}

fn metadata(content: &str) -> Result<Metadata> {
    let content = content.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let body = content
        .strip_prefix("---\n")
        .ok_or_else(|| other("缺少 YAML frontmatter"))?;
    let yaml = body
        .split_once("\n---")
        .ok_or_else(|| other("frontmatter 未闭合"))?
        .0;
    let meta: Metadata = serde_yaml::from_str(yaml).map_err(other)?;
    if meta.name.trim().is_empty() || meta.description.trim().is_empty() {
        return Err(other("Skill 必须包含 name 和 description"));
    }
    Ok(meta)
}

fn validate_skill_name(name: &str) -> Result<()> {
    if name.len() > 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || name.starts_with('-')
        || name.ends_with('-')
        || name.contains("--")
    {
        return Err(other(
            "Skill 名称应为不超过 64 字符的小写字母、数字和单个连字符",
        ));
    }
    Ok(())
}

pub(super) fn scan(loc: &Location, seen: &mut HashSet<String>, inventory: &mut Inventory) {
    let mut stack = vec![(Path::new(&loc.path).to_path_buf(), 0)];
    let mut visited = HashSet::new();
    while let Some((dir, depth)) = stack.pop() {
        let real = match fs::canonicalize(&dir) {
            Ok(p) => p,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !dir.is_symlink() => continue,
            Err(e) => {
                inventory.warnings.push(format!("{}：{e}", dir.display()));
                continue;
            }
        };
        if !visited.insert(real) {
            continue;
        }
        let file = dir.join("SKILL.md");
        // A Location root is a container: its own SKILL.md is ignored so it
        // cannot shadow or merge with the child Skills it holds.
        if depth > 0 && file.is_file() {
            match files::read(&file) {
                Err(e) => inventory.warnings.push(format!("{}：{e}", file.display())),
                Ok(content) => {
                    let canonical = match fs::canonicalize(&file) {
                        Ok(p) => p.to_string_lossy().into_owned(),
                        Err(e) => {
                            inventory.warnings.push(format!("{}：{e}", file.display()));
                            continue;
                        }
                    };
                    let id = super::key(format!("skill:{canonical}"));
                    let entry = file.to_string_lossy().into_owned();
                    let physical = !entry_is_removable(&file);
                    if !seen.insert(id.clone()) {
                        if let Some(row) = inventory.resources.iter_mut().find(|r| r.id == id) {
                            if !row
                                .locations
                                .iter()
                                .zip(&row.entry_paths)
                                .any(|(l, p)| l == &loc.id && p == &entry)
                            {
                                row.locations.push(loc.id.clone());
                                row.entry_paths.push(entry);
                                row.entry_links.push(!physical);
                            }
                        }
                    } else {
                        let parsed = metadata(&content).or_else(|error| {
                            if loc.agent != Some(crate::domain::Agent::ClaudeCode) {
                                return Err(error);
                            }
                            // Claude allows omitted frontmatter fields and derives
                            // the command name from the containing directory.
                            let normalized = content.replace("\r\n", "\n");
                            let yaml = normalized
                                .strip_prefix("---\n")
                                .and_then(|s| s.split_once("\n---"))
                                .map(|p| p.0);
                            let value: serde_yaml::Value = match yaml {
                                Some(y) => serde_yaml::from_str(y).map_err(other)?,
                                None => serde_yaml::Value::Null,
                            };
                            Ok(Metadata {
                                name: value
                                    .get("name")
                                    .and_then(serde_yaml::Value::as_str)
                                    .map(str::to_string)
                                    .unwrap_or_else(|| {
                                        dir.file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy()
                                            .into_owned()
                                    }),
                                description: value
                                    .get("description")
                                    .and_then(serde_yaml::Value::as_str)
                                    .unwrap_or("")
                                    .into(),
                            })
                        });
                        let (name, description, error) = match parsed {
                            Ok(meta) => (meta.name, meta.description, None),
                            Err(e) => (
                                dir.file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .into_owned(),
                                String::new(),
                                Some(e.to_string()),
                            ),
                        };
                        inventory.resources.push(Resource {
                            id,
                            kind: "skill".into(),
                            name,
                            description,
                            path: canonical,
                            locations: vec![loc.id.clone()],
                            entry_paths: vec![entry],
                            entry_links: vec![!physical],
                            revision: super::key(content),
                            registered: false,
                            error,
                        });
                    }
                }
            }
            // Supporting files may contain examples named SKILL.md. A skill is
            // a unit; do not recursively discover its references as more skills.
            continue;
        }
        if depth >= 12 {
            inventory
                .warnings
                .push(format!("{}：目录嵌套过深，未继续扫描", dir.display()));
            continue;
        }
        match fs::read_dir(&dir) {
            Ok(entries) => {
                let mut children = Vec::new();
                for entry in entries {
                    match entry {
                        Ok(e)
                            if !e.file_name().to_string_lossy().starts_with('.')
                                && e.path().is_dir() =>
                        {
                            children.push(e.path())
                        }
                        Ok(_) => {}
                        Err(e) => inventory.warnings.push(format!("{}：{e}", dir.display())),
                    }
                }
                children.sort();
                stack.extend(children.into_iter().rev().map(|p| (p, depth + 1)));
            }
            Err(e) => inventory.warnings.push(format!("{}：{e}", dir.display())),
        }
    }
}

fn copy_tree(source: &Path, dest: &Path, depth: usize) -> Result<()> {
    if depth > 32 {
        return Err(other("Skill 目录嵌套过深"));
    }
    fs::create_dir(dest)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if is_link(&entry.path()) {
            return Err(other("Skill 包含符号链接，请先准备不含外部链接的安装目录"));
        }
        if kind.is_dir() {
            if entry.file_name() == ".git" {
                continue;
            }
            copy_tree(&entry.path(), &dest.join(entry.file_name()), depth + 1)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), dest.join(entry.file_name()))?;
        } else {
            return Err(other("Skill 包含不支持的特殊文件"));
        }
    }
    Ok(())
}

pub struct SkillInstallResult {
    pub message: String,
    /// Canonical source `SKILL.md` that this entry was installed from.
    pub source_path: String,
    pub name: String,
    pub description: String,
}

pub struct SkillDisableResult {
    pub message: String,
    pub source_path: String,
    pub name: String,
    pub description: String,
    pub changed: bool,
}

/// Register an existing Skill directory as a source without writing any Agent
/// entry. Enabling later decides which scopes receive links.
pub fn register_source_dir(path: &Path) -> Result<SkillInstallResult> {
    let file = if path.is_dir() {
        path.join("SKILL.md")
    } else {
        path.to_path_buf()
    };
    let source = fs::canonicalize(&file)?;
    let meta = metadata(&files::read(&source)?)?;
    Ok(SkillInstallResult {
        message: format!("已转移 Skill 源：{}", meta.name),
        source_path: source.to_string_lossy().into_owned(),
        name: meta.name,
        description: meta.description,
    })
}

/// Register a chosen Skill directory as a NoEnding source. Registration adopts
/// outside sources into the library first, so a registered Skill is always
/// NoEnding-managed; directories already in the library are registered in place.
pub fn adopt_source(
    roots: &Roots,
    cwds: &[PathBuf],
    managed_root: &Path,
    path: &Path,
) -> Result<SkillInstallResult> {
    let install = register_source_dir(path)?;
    let source = Path::new(&install.source_path);
    let source_dir = source.parent().ok_or_else(|| other("Skill 源路径无效"))?;
    if source_is_managed(managed_root, source_dir) {
        let managed = fs::canonicalize(managed_root).unwrap_or_else(|_| managed_root.to_path_buf());
        let source_dir = fs::canonicalize(source_dir).unwrap_or_else(|_| source_dir.to_path_buf());
        if source_dir == managed {
            return Err(other(
                "NoEnding 源库根目录不是 Skill，请选择其中的具体 Skill 目录",
            ));
        }
        return Ok(install);
    }
    let id = super::source_id(&install.source_path);
    let outcome = migrate_skill(
        roots,
        cwds,
        managed_root,
        &id,
        Some(&install.source_path),
        true,
    )?;
    Ok(SkillInstallResult {
        message: outcome.message,
        source_path: outcome.source_path,
        name: outcome.name,
        description: outcome.description,
    })
}

/// Register valid Skills placed directly in the NoEnding library. The library
/// is NoEnding-owned, so discovery is implicit registration; these sources are
/// still never Locations and have no loading entries until enabled.
pub fn discover_library_sources(
    managed_root: &Path,
    registry: &mut super::SkillRegistry,
) -> Result<bool> {
    let entries = match fs::read_dir(managed_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let root = fs::canonicalize(managed_root)?;
    let mut changed = false;
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let file = entry.path().join("SKILL.md");
        if !file.is_file() {
            continue;
        }
        let Ok(source) = fs::canonicalize(&file) else {
            continue;
        };
        if !source.starts_with(&root) {
            continue;
        }
        let Ok(content) = files::read(&source) else {
            continue;
        };
        let Ok(meta) = metadata(&content) else {
            continue;
        };
        let path = source.to_string_lossy();
        let unchanged = registry
            .sources
            .iter()
            .find(|existing| existing.path == path)
            .is_some_and(|existing| {
                existing.name == meta.name && existing.description == meta.description
            });
        if unchanged {
            continue;
        }
        super::register_source(registry, &path, &meta.name, &meta.description);
        changed = true;
    }
    Ok(changed)
}

pub fn install_skill(
    roots: &Roots,
    cwd: Option<&Path>,
    source: &Path,
    location_id: &str,
) -> Result<SkillInstallResult> {
    install(roots, cwd, source, location_id, false)
}

pub fn link_skill(
    roots: &Roots,
    cwd: Option<&Path>,
    source: &Path,
    location_id: &str,
) -> Result<SkillInstallResult> {
    install(roots, cwd, source, location_id, true)
}

/// Re-enable a registered (or still-discovered) source with a link entry.
pub fn enable_skill(
    roots: &Roots,
    cwd: Option<&Path>,
    id: &str,
    source: Option<&str>,
    location_id: &str,
) -> Result<SkillInstallResult> {
    let source = match source {
        Some(path) => PathBuf::from(path),
        None => super::list(roots, cwd)
            .resources
            .iter()
            .find(|row| row.id == id && row.kind == "skill")
            .map(|row| PathBuf::from(&row.path))
            .ok_or_else(|| other("Skill 已不存在"))?,
    };
    if !source.is_file() {
        return Err(other("Skill 源目录已不存在"));
    }
    let dir = source
        .parent()
        .ok_or_else(|| other("Skill 源路径无效"))?
        .to_path_buf();
    install(roots, cwd, &dir, location_id, true)
}

fn user_entry_exists(roots: &Roots, id: &str) -> bool {
    let inventory = super::list(roots, None);
    inventory.resources.iter().any(|row| {
        row.id == id
            && row.kind == "skill"
            && row.locations.iter().any(|location| {
                inventory
                    .locations
                    .iter()
                    .any(|loc| &loc.id == location && loc.scope == "user")
            })
    })
}

/// Remove project-level links to a source that user-level enablement now
/// covers. Physical entries are the source itself and stay in place.
fn remove_project_links(
    roots: &Roots,
    cwds: &[PathBuf],
    id: &str,
    source: &str,
) -> (usize, Vec<String>) {
    let mut removed = 0;
    let mut failures = Vec::new();
    for cwd in cwds {
        let inventory = super::inventory::list_project_skills(roots, cwd);
        let Some(row) = inventory
            .resources
            .iter()
            .find(|row| row.id == id && row.kind == "skill" && row.path == source)
        else {
            continue;
        };
        for (index, entry) in row.entry_paths.iter().enumerate() {
            let Some(loc) = row
                .locations
                .get(index)
                .and_then(|location| inventory.locations.iter().find(|loc| &loc.id == location))
            else {
                continue;
            };
            if loc.scope != "project" {
                continue;
            }
            let path = Path::new(entry);
            if !entry_is_removable(path) {
                continue;
            }
            let root = Path::new(&loc.path);
            let Some(dir) = path.parent() else {
                continue;
            };
            if dir == root || !dir.starts_with(root) {
                failures.push(format!("{}：入口不在加载目录内", path.display()));
                continue;
            }
            match remove_entry_dir(dir) {
                Ok(()) => removed += 1,
                Err(error) => failures.push(format!("{}：{error}", dir.display())),
            }
        }
    }
    (removed, failures)
}

/// Enable one source for every user-level Agent with a single action. Agents
/// already covered by an existing entry are left alone. A complete user-level
/// enable supersedes project-level links to the same source, so those are
/// removed to avoid duplicate entries; physical project entries are untouched.
pub fn enable_skill_all(
    roots: &Roots,
    cwds: &[PathBuf],
    id: &str,
    source: Option<&str>,
) -> Result<SkillInstallResult> {
    let inventory = super::list(roots, None);
    let row = inventory
        .resources
        .iter()
        .find(|row| row.id == id && row.kind == "skill");
    let source = match source {
        Some(path) => PathBuf::from(path),
        None => row
            .map(|row| PathBuf::from(&row.path))
            .ok_or_else(|| other("Skill 已不存在"))?,
    };
    if !source.is_file() {
        return Err(other("Skill 源目录已不存在"));
    }
    let source = fs::canonicalize(source)?;
    let meta = metadata(&files::read(&source)?)?;
    let source_path = source.to_string_lossy().into_owned();
    let mut remaining = [true; ENABLE_AGENTS.len()];
    if let Some(row) = row {
        for (index, agent) in ENABLE_AGENTS.iter().enumerate() {
            remaining[index] = !row.locations.iter().any(|id| {
                inventory
                    .locations
                    .iter()
                    .any(|loc| &loc.id == id && supports_user_agent(loc, *agent))
            });
        }
    }
    // Pick the smallest set of link locations that covers uncovered Agents
    // without adding redundant entries for Agents that are already covered.
    let mut targets = Vec::new();
    while remaining.iter().any(|value| *value) {
        let mut best: Option<(&Location, Vec<usize>, bool)> = None;
        for loc in inventory
            .locations
            .iter()
            .filter(|loc| loc.kind == "skill" && loc.scope == "user")
        {
            let mut supported = Vec::new();
            for (index, agent) in ENABLE_AGENTS.iter().enumerate() {
                if supports_user_agent(loc, *agent) {
                    supported.push(index);
                }
            }
            let hits: Vec<usize> = supported
                .iter()
                .copied()
                .filter(|index| remaining[*index])
                .collect();
            if hits.is_empty() {
                continue;
            }
            let redundant = supported.iter().any(|index| !remaining[*index]);
            let better = best
                .as_ref()
                .map_or(true, |(_, best_hits, best_redundant)| {
                    (usize::from(!redundant), hits.len())
                        > (usize::from(!*best_redundant), best_hits.len())
                });
            if better {
                best = Some((loc, hits, redundant));
            }
        }
        let Some((loc, hits, _)) = best else {
            break;
        };
        targets.push(loc.id.clone());
        for index in hits {
            remaining[index] = false;
        }
    }
    let unplaced = remaining.iter().any(|value| *value);
    let mut installed = 0;
    let mut failures = Vec::new();
    for location in &targets {
        if let Err(error) = enable_skill(roots, None, id, Some(&source_path), location) {
            failures.push(error.to_string());
        } else {
            installed += 1;
        }
    }
    if installed == 0 && !failures.is_empty() {
        return Err(other(format!("未能启用任何位置：{}", failures.join("；"))));
    }
    let complete = failures.is_empty() && !unplaced;
    let (cleaned, cleanup_failures) = if complete {
        remove_project_links(roots, cwds, id, &source_path)
    } else {
        (0, Vec::new())
    };
    let mut message = if complete {
        if targets.is_empty() {
            "所有 Agent 已启用".into()
        } else {
            format!("已为所有 Agent 启用（新增 {installed} 个链接）")
        }
    } else if failures.is_empty() {
        format!("已启用 {installed} 个位置，但部分 Agent 没有可用的加载位置")
    } else {
        format!("已启用 {installed} 个位置；失败：{}", failures.join("；"))
    };
    if cleaned > 0 {
        message.push_str(&format!("；已清理 {cleaned} 个项目级链接入口"));
    }
    if !cleanup_failures.is_empty() {
        message.push_str(&format!(
            "；项目级链接清理失败：{}",
            cleanup_failures.join("；")
        ));
    }
    Ok(SkillInstallResult {
        message,
        source_path,
        name: meta.name,
        description: meta.description,
    })
}

fn project_targets(inventory: &Inventory, row: Option<&Resource>) -> (Vec<String>, bool) {
    let mut remaining = [true; ENABLE_AGENTS.len()];
    if let Some(row) = row {
        for (index, agent) in ENABLE_AGENTS.iter().enumerate() {
            remaining[index] = !row.locations.iter().any(|location_id| {
                inventory.locations.iter().any(|loc| {
                    &loc.id == location_id
                        && loc.scope == "project"
                        && supports_project_agent(loc, *agent)
                })
            });
        }
    }
    let mut targets = Vec::new();
    while remaining.iter().any(|value| *value) {
        let mut best: Option<(&Location, Vec<usize>, bool)> = None;
        for loc in inventory
            .locations
            .iter()
            .filter(|loc| loc.kind == "skill" && loc.scope == "project")
        {
            let supported: Vec<usize> = ENABLE_AGENTS
                .iter()
                .enumerate()
                .filter(|(_, agent)| supports_project_agent(loc, **agent))
                .map(|(index, _)| index)
                .collect();
            let hits: Vec<usize> = supported
                .iter()
                .copied()
                .filter(|index| remaining[*index])
                .collect();
            if hits.is_empty() {
                continue;
            }
            let redundant = supported.iter().any(|index| !remaining[*index]);
            let better = best
                .as_ref()
                .map_or(true, |(_, best_hits, best_redundant)| {
                    (usize::from(!redundant), hits.len())
                        > (usize::from(!*best_redundant), best_hits.len())
                });
            if better {
                best = Some((loc, hits, redundant));
            }
        }
        let Some((loc, hits, _)) = best else {
            break;
        };
        targets.push(loc.id.clone());
        for index in hits {
            remaining[index] = false;
        }
    }
    (targets, remaining.iter().any(|value| *value))
}

pub fn find_source_path(roots: &Roots, cwds: &[PathBuf], id: &str) -> Option<PathBuf> {
    if let Some(row) = super::list(roots, None)
        .resources
        .into_iter()
        .find(|row| row.id == id && row.kind == "skill")
    {
        return Some(PathBuf::from(row.path));
    }
    cwds.iter().find_map(|cwd| {
        super::inventory::list_project_skills(roots, cwd)
            .resources
            .into_iter()
            .find(|row| row.id == id && row.kind == "skill")
            .map(|row| PathBuf::from(row.path))
    })
}

/// Enable one source in every working directory of a project. Each directory
/// gets the smallest set of project-level links that covers all Agents;
/// read-only locations and existing physical entries are never written.
pub fn enable_skill_project(
    roots: &Roots,
    cwds: &[PathBuf],
    id: &str,
    source: Option<&str>,
) -> Result<SkillInstallResult> {
    if cwds.is_empty() {
        return Err(other("项目没有可用的工作目录"));
    }
    if user_entry_exists(roots, id) {
        return Err(other("该 Skill 已由用户级启用，不能再配置项目级入口"));
    }
    let source = match source {
        Some(path) => PathBuf::from(path),
        None => find_source_path(roots, cwds, id).ok_or_else(|| other("Skill 已不存在"))?,
    };
    if !source.is_file() {
        return Err(other("Skill 源目录已不存在"));
    }
    let source = fs::canonicalize(source)?;
    let meta = metadata(&files::read(&source)?)?;
    let source_path = source.to_string_lossy().into_owned();
    let mut installed = 0;
    let mut failures = Vec::new();
    let mut unplaced = Vec::new();
    for cwd in cwds {
        let inventory = super::inventory::list_project_skills(roots, cwd);
        let row = inventory
            .resources
            .iter()
            .find(|row| row.id == id && row.kind == "skill");
        let (targets, partial) = project_targets(&inventory, row);
        if partial {
            unplaced.push(cwd.display().to_string());
        }
        for location in targets {
            match enable_skill(roots, Some(cwd), id, Some(&source_path), &location) {
                Ok(_) => installed += 1,
                Err(error) => failures.push(format!("{}：{error}", cwd.display())),
            }
        }
    }
    if installed == 0 && !failures.is_empty() {
        return Err(other(format!("未能启用任何位置：{}", failures.join("；"))));
    }
    let message = if failures.is_empty() && unplaced.is_empty() {
        if installed == 0 {
            "项目已为所有 Agent 启用".into()
        } else {
            format!(
                "已在 {} 个工作目录为所有 Agent 启用（新增 {installed} 个链接）",
                cwds.len()
            )
        }
    } else if failures.is_empty() {
        format!(
            "已启用 {installed} 个位置，但以下目录没有可用的加载位置：{}",
            unplaced.join("、")
        )
    } else {
        format!("已启用 {installed} 个位置；失败：{}", failures.join("；"))
    };
    Ok(SkillInstallResult {
        message,
        source_path,
        name: meta.name,
        description: meta.description,
    })
}

/// Remove every project-level link entry in the project's working directories.
/// Physical entries are reported before anything is removed, because disabling
/// them would delete the files the Agent reads.
pub fn disable_skill_project(
    roots: &Roots,
    cwds: &[PathBuf],
    id: &str,
) -> Result<SkillDisableResult> {
    let _guard = super::MUTATION
        .lock()
        .map_err(|_| other("配置操作锁不可用"))?;
    let mut entries: Vec<(PathBuf, PathBuf, String)> = Vec::new();
    let mut metadata: Option<(String, String, String)> = None;
    for cwd in cwds {
        let inventory = super::inventory::list_project_skills(roots, cwd);
        let Some(row) = inventory
            .resources
            .iter()
            .find(|row| row.id == id && row.kind == "skill")
        else {
            continue;
        };
        if metadata.is_none() {
            metadata = Some((row.path.clone(), row.name.clone(), row.description.clone()));
        }
        for (index, entry) in row.entry_paths.iter().enumerate() {
            let Some(loc) = row
                .locations
                .get(index)
                .and_then(|id| inventory.locations.iter().find(|loc| &loc.id == id))
            else {
                continue;
            };
            if loc.scope != "project" {
                continue;
            }
            entries.push((
                PathBuf::from(entry),
                PathBuf::from(&loc.path),
                cwd.display().to_string(),
            ));
        }
    }
    let (source_path, name, description) = metadata.unwrap_or_default();
    if entries.is_empty() {
        return Ok(SkillDisableResult {
            message: "当前项目未启用".into(),
            source_path,
            name,
            description,
            changed: false,
        });
    }
    let removable: Vec<(PathBuf, PathBuf, String)> = entries
        .into_iter()
        .filter(|(entry, _, _)| entry_is_removable(entry))
        .collect();
    if removable.is_empty() {
        return Ok(SkillDisableResult {
            message: "源目录本身位于项目加载位置，没有可停用的链接入口".into(),
            source_path,
            name,
            description,
            changed: false,
        });
    }
    let mut removed = BTreeSet::new();
    let mut directories = BTreeSet::new();
    for (entry, root, cwd) in &removable {
        let dir = entry.parent().ok_or_else(|| other("Skill 入口无效"))?;
        if dir == root || !dir.starts_with(root) {
            return Err(other("不能移除 Skill 根目录"));
        }
        if removed.insert(dir.to_path_buf()) {
            remove_entry_dir(dir)?;
            directories.insert(cwd.as_str());
        }
    }
    Ok(SkillDisableResult {
        message: format!("已在 {} 个工作目录停用项目级链接入口", directories.len()),
        source_path,
        name,
        description,
        changed: true,
    })
}

/// A newly registered working directory inherits the project-level Skills that
/// are already enabled in any sibling directory. Best-effort: a path with a
/// conflicting physical Skill remains visible as partial coverage.
pub fn inherit_project_skills(
    roots: &Roots,
    existing_cwds: &[PathBuf],
    new_cwds: &[PathBuf],
) -> Result<()> {
    let mut sources = BTreeSet::<(String, String)>::new();
    for cwd in existing_cwds {
        let inventory = super::list(roots, Some(cwd));
        for row in inventory.resources.iter().filter(|row| row.kind == "skill") {
            let has_project_entry = row.entry_paths.iter().enumerate().any(|(index, _)| {
                row.locations
                    .get(index)
                    .and_then(|id| inventory.locations.iter().find(|loc| &loc.id == id))
                    .is_some_and(|loc| loc.scope == "project")
            });
            if has_project_entry && Path::new(&row.path).is_file() {
                sources.insert((row.id.clone(), row.path.clone()));
            }
        }
    }
    for cwd in new_cwds {
        for (id, path) in &sources {
            let _ = enable_skill_project(roots, std::slice::from_ref(cwd), id, Some(path));
        }
    }
    Ok(())
}

fn install(
    roots: &Roots,
    cwd: Option<&Path>,
    source: &Path,
    location_id: &str,
    link: bool,
) -> Result<SkillInstallResult> {
    let _guard = super::MUTATION
        .lock()
        .map_err(|_| other("配置操作锁不可用"))?;
    let loc = super::inventory::locations(roots, cwd)
        .into_iter()
        .find(|l| l.id == location_id && l.kind == "skill")
        .ok_or_else(|| other("Skill 安装位置无效"))?;
    let source = fs::canonicalize(source)?;
    let meta = metadata(&files::read(&source.join("SKILL.md"))?)?;
    validate_skill_name(&meta.name)?;
    let root = Path::new(&loc.path);
    fs::create_dir_all(root)?;
    let root = fs::canonicalize(root)?;
    if root.starts_with(&source) {
        return Err(other("安装位置不能位于源 Skill 内"));
    }
    let destination = root.join(&meta.name);
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(other("目标目录已存在，不会覆盖已有 Skill"));
    }
    let temp = root.join(format!(".noending-{}", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        if link {
            #[cfg(unix)]
            std::os::unix::fs::symlink(&source, &temp)?;
            #[cfg(windows)]
            std::os::windows::fs::symlink_dir(&source, &temp).map_err(|e| {
                other(format!(
                    "创建目录链接失败：{e}；可改用复制安装，或检查 Windows 开发者模式/链接权限"
                ))
            })?;
        } else {
            copy_tree(&source, &temp, 0)?;
        }
        // Recheck after copying; a native installer may have created the target.
        if fs::symlink_metadata(&destination).is_ok() {
            return Err(other("目标目录已被创建，请刷新后重试"));
        }
        fs::rename(&temp, &destination)?;
        Ok(())
    })();
    if fs::symlink_metadata(&temp).is_ok() {
        if is_link(&temp) {
            #[cfg(windows)]
            let _ = fs::remove_dir(temp);
            #[cfg(not(windows))]
            let _ = fs::remove_file(temp);
        } else {
            let _ = fs::remove_dir_all(temp);
        }
    }
    result?;
    Ok(SkillInstallResult {
        message: "Skill 已安装；已有终端请按 Agent 的方式重新加载或重新打开".into(),
        source_path: fs::canonicalize(source.join("SKILL.md"))?
            .to_string_lossy()
            .into_owned(),
        name: meta.name,
        description: meta.description,
    })
}

fn remove_link(path: &Path) -> Result<()> {
    #[cfg(windows)]
    fs::remove_dir(path)?;
    #[cfg(not(windows))]
    fs::remove_file(path)?;
    Ok(())
}

/// Remove an entry directory that holds no Skill content of its own. Wrapper
/// directories contain only links, so deleting the tree never follows them.
fn remove_entry_dir(dir: &Path) -> Result<()> {
    if is_link(dir) {
        remove_link(dir)
    } else {
        fs::remove_dir_all(dir)?;
        Ok(())
    }
}

/// Disable one link entry. Real directories are rejected: removing them would
/// delete the files the Agent reads, which is uninstall, not disable.
pub fn disable_skill(
    roots: &Roots,
    cwd: Option<&Path>,
    id: &str,
    entry_path: &str,
    expected: &str,
) -> Result<SkillDisableResult> {
    let _guard = super::MUTATION
        .lock()
        .map_err(|_| other("配置操作锁不可用"))?;
    let inventory = super::list(roots, cwd);
    let resource = inventory
        .resources
        .iter()
        .find(|r| r.id == id && r.kind == "skill")
        .ok_or_else(|| other("Skill 已不存在"))?;
    super::check_revision(&resource.revision, expected)?;
    let index = resource
        .entry_paths
        .iter()
        .position(|p| p == entry_path)
        .ok_or_else(|| other("加载入口已不存在"))?;
    let loc = inventory
        .locations
        .iter()
        .find(|l| l.id == resource.locations[index])
        .ok_or_else(|| other("来源已不存在"))?;
    let dir = Path::new(entry_path)
        .parent()
        .ok_or_else(|| other("Skill 路径无效"))?;
    let root = Path::new(&loc.path);
    if dir == root || !dir.starts_with(root) {
        return Err(other("不能移除 Skill 根目录"));
    }
    if !entry_is_removable(Path::new(entry_path)) {
        return Err(other("源目录位于 Agent 生效目录，不能停用；请卸载"));
    }
    remove_entry_dir(dir)?;
    Ok(SkillDisableResult {
        message: "Skill 加载入口已停用".into(),
        source_path: resource.path.clone(),
        name: resource.name.clone(),
        description: resource.description.clone(),
        changed: true,
    })
}

/// Disable every user-level link entry for a source in one action. Project
/// entries are untouched; physical entries belong to their Agent directory and
/// are left in place.
pub fn disable_skill_all(roots: &Roots, id: &str) -> Result<SkillDisableResult> {
    let _guard = super::MUTATION
        .lock()
        .map_err(|_| other("配置操作锁不可用"))?;
    let inventory = super::list(roots, None);
    let resource = inventory
        .resources
        .iter()
        .find(|row| row.id == id && row.kind == "skill")
        .ok_or_else(|| other("Skill 已不存在"))?;
    let entries: Vec<&String> = resource
        .entry_paths
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            resource
                .locations
                .get(*index)
                .and_then(|id| inventory.locations.iter().find(|loc| &loc.id == id))
                .is_some_and(|loc| loc.scope == "user")
        })
        .map(|(_, entry)| entry)
        .collect();
    if entries.is_empty() {
        return Ok(SkillDisableResult {
            message: "当前未启用".into(),
            source_path: resource.path.clone(),
            name: resource.name.clone(),
            description: resource.description.clone(),
            changed: false,
        });
    }
    let removable: Vec<&String> = entries
        .iter()
        .copied()
        .filter(|entry| entry_is_removable(Path::new(entry)))
        .collect();
    if removable.is_empty() {
        return Ok(SkillDisableResult {
            message: "源目录本身位于加载位置，没有可停用的链接入口".into(),
            source_path: resource.path.clone(),
            name: resource.name.clone(),
            description: resource.description.clone(),
            changed: false,
        });
    }
    for entry in &removable {
        let dir = Path::new(entry)
            .parent()
            .ok_or_else(|| other("Skill 入口无效"))?;
        remove_entry_dir(dir)?;
    }
    let physical = entries.len() - removable.len();
    Ok(SkillDisableResult {
        message: if physical > 0 {
            format!(
                "已停用 {} 个用户级链接入口；{physical} 个源目录入口保持启用",
                removable.len()
            )
        } else {
            "已在用户级停用所有 Agent 的加载入口".into()
        },
        source_path: resource.path.clone(),
        name: resource.name.clone(),
        description: resource.description.clone(),
        changed: true,
    })
}

fn collect_entries(
    roots: &Roots,
    cwds: &[PathBuf],
    id: &str,
) -> (BTreeSet<PathBuf>, BTreeSet<String>, Vec<Resource>) {
    let mut inventories = vec![super::list(roots, None)];
    for cwd in cwds {
        inventories.push(super::list(roots, Some(cwd)));
    }
    let mut skill_roots = BTreeSet::new();
    let mut entries = BTreeSet::new();
    let mut rows = Vec::new();
    for inventory in inventories {
        for loc in inventory.locations {
            if loc.kind == "skill" {
                skill_roots.insert(PathBuf::from(loc.path));
            }
        }
        for row in inventory.resources {
            if row.kind == "skill" && row.id == id {
                entries.extend(row.entry_paths.iter().cloned());
                rows.push(row);
            }
        }
    }
    (skill_roots, entries, rows)
}

fn collect_skill_uninstall(
    roots: &Roots,
    cwds: &[PathBuf],
    id: &str,
) -> Result<(Resource, BTreeSet<String>, BTreeSet<PathBuf>)> {
    let (skill_roots, entries, rows) = collect_entries(roots, cwds, id);
    let mut resource: Option<Resource> = None;
    for row in rows {
        match &resource {
            Some(existing) if existing.path != row.path => {
                return Err(other("Skill 来源已变化，请刷新"));
            }
            None => resource = Some(row),
            _ => {}
        }
    }
    let resource = resource.ok_or_else(|| other("Skill 已不存在"))?;
    Ok((resource, entries, skill_roots))
}

pub fn uninstall_preview(
    roots: &Roots,
    cwds: &[PathBuf],
    id: &str,
) -> Result<super::UninstallPreview> {
    let (row, entries, _) = collect_skill_uninstall(roots, cwds, id)?;
    Ok(super::UninstallPreview {
        delete_source_required: entries
            .iter()
            .any(|entry| !entry_is_removable(Path::new(entry))),
        entry_count: entries.len(),
        source_path: row.path,
    })
}

fn scan_registered(
    roots: &Roots,
    cwds: &[PathBuf],
    id: &str,
) -> Result<(BTreeSet<String>, BTreeSet<PathBuf>, Option<String>)> {
    let (skill_roots, entries, rows) = collect_entries(roots, cwds, id);
    let revision = rows.first().map(|row| row.revision.clone());
    Ok((entries, skill_roots, revision))
}

pub fn detail_registered(
    roots: &Roots,
    cwd: Option<&Path>,
    managed_root: &Path,
    registry: &super::SkillRegistry,
    source: &super::SkillSource,
) -> Result<super::ResourceDetail> {
    let mut inventory = super::list(roots, cwd);
    super::registry::merge(registry, &mut inventory, true, managed_root);
    let row = inventory
        .resources
        .into_iter()
        .find(|row| row.id == source.id)
        .ok_or_else(|| other("Skill 已不存在"))?;
    let path = Path::new(&row.path);
    let content = if path.is_file() {
        files::read(path)?
    } else {
        String::new()
    };
    Ok(super::ResourceDetail {
        resource: row,
        content,
        config: None,
    })
}

pub fn detail_project(roots: &Roots, cwds: &[PathBuf], id: &str) -> Result<super::ResourceDetail> {
    let inventory = super::merge_inventories(
        cwds.iter()
            .map(|cwd| super::list(roots, Some(cwd)))
            .collect(),
    );
    let row = inventory
        .resources
        .into_iter()
        .find(|row| row.id == id)
        .ok_or_else(|| other("Skill 已不存在"))?;
    if row.kind != "skill" {
        return Err(other("项目级 MCP 配置需要选择工作目录"));
    }
    let path = Path::new(&row.path);
    let content = if path.is_file() {
        files::read(path)?
    } else {
        String::new()
    };
    Ok(super::ResourceDetail {
        resource: row,
        content,
        config: None,
    })
}

pub fn registered_uninstall_preview(
    roots: &Roots,
    cwds: &[PathBuf],
    source: &super::SkillSource,
) -> Result<super::UninstallPreview> {
    let (entries, _, _) = scan_registered(roots, cwds, &source.id)?;
    Ok(super::UninstallPreview {
        delete_source_required: entries
            .iter()
            .any(|entry| !entry_is_removable(Path::new(entry))),
        entry_count: entries.len(),
        source_path: source.path.clone(),
    })
}

/// Uninstall a registered source. Link entries are removed and the source is
/// deleted only when requested. A source that is itself a physical entry cannot
/// be kept, because the Agent would keep discovering it.
pub fn uninstall_registered_skill(
    roots: &Roots,
    cwds: &[PathBuf],
    source: &super::SkillSource,
    expected: &str,
    delete_source: bool,
) -> Result<OperationResult> {
    let _guard = super::MUTATION
        .lock()
        .map_err(|_| other("配置操作锁不可用"))?;
    let (entries, skill_roots, scanned_revision) = scan_registered(roots, cwds, &source.id)?;
    let revision = scanned_revision.unwrap_or_else(|| super::registry::source_revision(source));
    super::check_revision(&revision, expected)?;
    if entries
        .iter()
        .any(|entry| !entry_is_removable(Path::new(entry)))
        && !delete_source
    {
        return Err(other("源目录位于 Agent 生效目录，必须同时删除源文件"));
    }
    let source_path = Path::new(&source.path);
    let source_dir = source_path
        .parent()
        .ok_or_else(|| other("Skill 源路径无效"))?;
    let source_canonical = source_path
        .is_file()
        .then(|| fs::canonicalize(source_dir))
        .transpose()?;
    for entry in entries
        .iter()
        .filter(|entry| !entry_is_removable(Path::new(entry)))
    {
        let dir = Path::new(entry)
            .parent()
            .ok_or_else(|| other("Skill 入口无效"))?;
        match &source_canonical {
            Some(source) if fs::canonicalize(dir).ok().as_ref() == Some(source) => {}
            _ => return Err(other("Skill 入口与源目录不一致，请刷新")),
        }
    }
    if delete_source {
        if let Some(source) = &source_canonical {
            let contains_root = skill_roots
                .iter()
                .filter_map(|root| fs::canonicalize(root).ok())
                .any(|root| root == *source || root.starts_with(source));
            if contains_root {
                return Err(other("源目录包含 Agent 配置目录，拒绝删除"));
            }
        }
    }
    for entry in entries
        .iter()
        .filter(|entry| entry_is_removable(Path::new(entry)))
    {
        let dir = Path::new(entry)
            .parent()
            .ok_or_else(|| other("Skill 入口无效"))?;
        remove_entry_dir(dir)?;
    }
    if delete_source {
        if let Some(source) = source_canonical {
            fs::remove_dir_all(source)?;
        }
    }
    Ok(OperationResult {
        message: if delete_source {
            "Skill 及源文件已卸载".into()
        } else {
            "Skill 已从 NoEnding 移除，源文件已保留".into()
        },
    })
}

/// Remove a Skill's entries across every registered project. Keeping source
/// files is impossible when an entry is a real directory: the Agent would
/// discover it again. Such resources must be deleted together with the source.
pub fn uninstall_skill(
    roots: &Roots,
    cwds: &[PathBuf],
    id: &str,
    expected: &str,
    delete_source: bool,
) -> Result<OperationResult> {
    let _guard = super::MUTATION
        .lock()
        .map_err(|_| other("配置操作锁不可用"))?;
    let (row, entries, skill_roots) = collect_skill_uninstall(roots, cwds, id)?;
    super::check_revision(&row.revision, expected)?;
    let physical = entries
        .iter()
        .any(|entry| !entry_is_removable(Path::new(entry)));
    if physical && !delete_source {
        return Err(other("源目录位于 Agent 生效目录，必须同时删除源文件"));
    }
    let source = Path::new(&row.path)
        .parent()
        .ok_or_else(|| other("Skill 源路径无效"))?;
    let source = if delete_source {
        let source = fs::canonicalize(source)?;
        if fs::canonicalize(source.join("SKILL.md"))? != PathBuf::from(&row.path) {
            return Err(other("Skill 源目录已变化，请刷新"));
        }
        // Deleting a directory that contains an Agent scan root would remove
        // configuration unrelated to this Skill.
        if skill_roots
            .iter()
            .any(|root| root == &source || root.starts_with(&source))
        {
            return Err(other("源目录包含 Agent 配置目录，拒绝删除"));
        }
        Some(source)
    } else {
        None
    };
    if let Some(source) = &source {
        for entry in entries
            .iter()
            .filter(|entry| !entry_is_removable(Path::new(entry)))
        {
            let dir = Path::new(entry)
                .parent()
                .ok_or_else(|| other("Skill 入口无效"))?;
            if fs::canonicalize(dir)? != *source {
                return Err(other("Skill 入口与源目录不一致，请刷新"));
            }
        }
    }
    for entry in &entries {
        let path = Path::new(entry);
        if entry_is_removable(path) {
            let dir = path.parent().ok_or_else(|| other("Skill 入口无效"))?;
            remove_entry_dir(dir)?;
        }
    }
    if let Some(source) = source {
        fs::remove_dir_all(source)?;
        Ok(OperationResult {
            message: "Skill 及源文件已卸载".into(),
        })
    } else {
        Ok(OperationResult {
            message: "Skill 已从 NoEnding 移除，源文件已保留".into(),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillMigrationPreview {
    pub source_path: String,
    pub target_path: String,
    pub entry_count: usize,
    pub physical_entries: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillMigrationResult {
    pub message: String,
    pub source_path: String,
    pub resource_id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSourceKind {
    Library,
    Agent,
    External,
}

/// Where the Skill's content lives. Configuration entries are separate and do
/// not determine this classification.
pub fn skill_source_kind(resource: &Resource, managed_root: &Path) -> SkillSourceKind {
    let source = Path::new(&resource.path);
    let source_dir = source.parent().unwrap_or(source);
    if source_is_managed(managed_root, source_dir) {
        SkillSourceKind::Library
    } else if resource.entry_links.iter().any(|link| !link) {
        SkillSourceKind::Agent
    } else {
        SkillSourceKind::External
    }
}

struct MigrationPlan {
    name: String,
    description: String,
    entries: BTreeSet<String>,
    source_dir: PathBuf,
    target: PathBuf,
}

/// Validate a source outside the library and compute where it would live.
/// Sources with no entries (registered but disabled) are valid.
fn migration_plan(
    roots: &Roots,
    cwds: &[PathBuf],
    managed_root: &Path,
    id: &str,
    source: Option<&str>,
) -> Result<MigrationPlan> {
    let source_file = match source {
        Some(path) => PathBuf::from(path),
        None => find_source_path(roots, cwds, id).ok_or_else(|| other("Skill 已不存在"))?,
    };
    if !source_file.is_file() {
        return Err(other("Skill 源目录已不存在"));
    }
    let source = fs::canonicalize(&source_file)?;
    let source_dir = source
        .parent()
        .ok_or_else(|| other("Skill 源路径无效"))?
        .to_path_buf();
    let meta = metadata(&files::read(&source)?)?;
    validate_skill_name(&meta.name)?;
    if source_is_managed(managed_root, &source_dir) {
        return Err(other("源目录已在 NoEnding 源库中"));
    }
    let (skill_roots, entries, _) = collect_entries(roots, cwds, id);
    // Moving a Location root would take every sibling Skill with it, so a
    // SKILL.md living directly in an Agent Skills root cannot be registered.
    if skill_roots
        .iter()
        .filter_map(|root| fs::canonicalize(root).ok())
        .any(|root| root == source_dir)
    {
        return Err(other(
            "Agent Skills 根目录不能作为 Skill 源，请选择其中的具体 Skill 目录",
        ));
    }
    for entry in &entries {
        let path = Path::new(entry);
        let dir = path.parent().ok_or_else(|| other("Skill 入口无效"))?;
        let is_source = fs::canonicalize(dir).ok().as_ref() == Some(&source_dir);
        if !is_source && !entry_is_removable(path) {
            return Err(other(format!(
                "加载入口既不是源目录也不是链接，无法转移：{}",
                dir.display()
            )));
        }
    }
    let target = managed_root.join(&meta.name);
    if target == source_dir || target.starts_with(&source_dir) {
        return Err(other("源目录包含 NoEnding 源库，无法迁移"));
    }
    if fs::symlink_metadata(&target).is_ok() {
        return Err(other(format!(
            "NoEnding 源库中已存在同名 Skill：{}",
            target.display()
        )));
    }
    Ok(MigrationPlan {
        name: meta.name,
        description: meta.description,
        entries,
        source_dir,
        target,
    })
}

pub fn preview_skill_migration(
    roots: &Roots,
    cwds: &[PathBuf],
    managed_root: &Path,
    id: &str,
    source: Option<&str>,
) -> Result<SkillMigrationPreview> {
    let plan = migration_plan(roots, cwds, managed_root, id, source)?;
    Ok(SkillMigrationPreview {
        source_path: plan
            .source_dir
            .join("SKILL.md")
            .to_string_lossy()
            .into_owned(),
        target_path: plan.target.to_string_lossy().into_owned(),
        entry_count: plan.entries.len(),
        physical_entries: plan
            .entries
            .iter()
            .filter(|entry| !entry_is_removable(Path::new(entry)))
            .count(),
    })
}

/// Move or copy a Skill source outside the library into NoEnding-managed
/// storage and replace every loading entry with a link to the new location.
/// Only the source directory moves; file contents are never rewritten.
pub fn migrate_skill(
    roots: &Roots,
    cwds: &[PathBuf],
    managed_root: &Path,
    id: &str,
    source: Option<&str>,
    delete_source: bool,
) -> Result<SkillMigrationResult> {
    let _guard = super::MUTATION
        .lock()
        .map_err(|_| other("配置操作锁不可用"))?;
    let plan = migration_plan(roots, cwds, managed_root, id, source)?;
    fs::create_dir_all(managed_root)?;
    if fs::symlink_metadata(&plan.target).is_ok() {
        return Err(other(format!(
            "NoEnding 源库中已存在同名 Skill：{}",
            plan.target.display()
        )));
    }
    let mut physical_dirs = BTreeSet::new();
    let mut wrapper_dirs = BTreeSet::new();
    let mut link_dirs = BTreeSet::new();
    for entry in &plan.entries {
        let path = Path::new(entry);
        let dir = path
            .parent()
            .ok_or_else(|| other("Skill 入口无效"))?
            .to_path_buf();
        if entry_is_link(path) {
            link_dirs.insert(dir);
        } else if entry_is_wrapper(path) {
            wrapper_dirs.insert(dir);
        } else {
            physical_dirs.insert(dir);
        }
    }
    if !delete_source && !physical_dirs.is_empty() {
        return Err(other("源目录位于 Agent 配置目录，转移后必须删除源目录"));
    }
    if delete_source {
        move_dir(&plan.source_dir, &plan.target)?;
    } else {
        copy_migration_tree(&plan.source_dir, &plan.target, 0)?;
    }
    let target = fs::canonicalize(&plan.target)?;
    // Physical entries are the Agent-visible source itself. Creating their
    // replacement link is the critical step; any failure rolls the move back.
    let mut created: Vec<PathBuf> = Vec::new();
    for dir in &physical_dirs {
        let result = (|| -> Result<()> {
            if fs::symlink_metadata(dir).is_ok() {
                return Err(other("加载位置仍被占用，请刷新后重试"));
            }
            create_dir_link(&target, dir)
        })();
        if let Err(error) = result {
            for path in &created {
                let _ = remove_link(path);
            }
            return Err(match move_dir(&target, &plan.source_dir) {
                Ok(()) => error,
                Err(rollback) => other(format!("{error}；回滚迁移也失败：{rollback}")),
            });
        }
        created.push(dir.clone());
    }
    let mut failures = Vec::new();
    let mut replaced_entries = 0;
    for dir in &wrapper_dirs {
        match replace_wrapper_dir(dir, &target) {
            Ok(()) => replaced_entries += 1,
            Err(error) => failures.push(format!("{}：{error}", dir.display())),
        }
    }
    for dir in &link_dirs {
        match replace_dir_link(dir, &target) {
            Ok(()) => replaced_entries += 1,
            Err(error) => failures.push(format!("{}：{error}", dir.display())),
        }
    }
    if physical_dirs.is_empty()
        && replaced_entries == 0
        && !(link_dirs.is_empty() && wrapper_dirs.is_empty())
    {
        let rollback = if delete_source {
            move_dir(&target, &plan.source_dir)
        } else {
            for dir in &link_dirs {
                let _ = replace_dir_link(dir, &plan.source_dir);
            }
            fs::remove_dir_all(&target).map_err(|error| other(format!("删除转移副本失败：{error}")))
        };
        if rollback.is_ok() && delete_source {
            for dir in &link_dirs {
                if fs::symlink_metadata(dir).is_err() {
                    let _ = create_dir_link(&plan.source_dir, dir);
                }
            }
        }
        return Err(match rollback {
            Ok(()) => other(format!("未能更新任何加载入口：{}", failures.join("；"))),
            Err(rollback) => other(format!(
                "未能更新任何加载入口：{}；回滚转移也失败：{rollback}",
                failures.join("；")
            )),
        });
    }
    let source_path = fs::canonicalize(target.join("SKILL.md"))?;
    let resource_id = super::source_id(&source_path.to_string_lossy());
    let updated = physical_dirs.len() + replaced_entries;
    let done = if delete_source {
        "已将 Skill 源迁移到 NoEnding 源库"
    } else {
        "已将 Skill 源复制到 NoEnding 源库，原源目录保留"
    };
    let mut message = if plan.entries.is_empty() {
        format!("{done}（{}）", target.display())
    } else {
        format!(
            "{done}（{}），并更新 {updated} 个加载入口",
            target.display()
        )
    };
    if !failures.is_empty() {
        message.push_str(&format!("；以下入口未能更新：{}", failures.join("；")));
    }
    Ok(SkillMigrationResult {
        message,
        source_path: source_path.to_string_lossy().into_owned(),
        resource_id,
        name: plan.name,
        description: plan.description,
    })
}

fn is_cross_device(error: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        error.raw_os_error() == Some(18)
    }
    #[cfg(windows)]
    {
        error.raw_os_error() == Some(17)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = error;
        false
    }
}

fn move_dir(source: &Path, target: &Path) -> Result<()> {
    match fs::rename(source, target) {
        Ok(()) => Ok(()),
        Err(error) if is_cross_device(&error) => {
            let parent = target.parent().ok_or_else(|| other("目标路径无效"))?;
            let temp = parent.join(format!(".noending-{}", uuid::Uuid::new_v4()));
            if let Err(error) = copy_migration_tree(source, &temp, 0) {
                let _ = fs::remove_dir_all(&temp);
                return Err(error);
            }
            if let Err(error) = fs::rename(&temp, target) {
                let _ = fs::remove_dir_all(&temp);
                return Err(error.into());
            }
            if let Err(error) = fs::remove_dir_all(source) {
                let _ = fs::remove_dir_all(target);
                return Err(error.into());
            }
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn copy_migration_tree(source: &Path, dest: &Path, depth: usize) -> Result<()> {
    if depth > 64 {
        return Err(other("Skill 目录嵌套过深"));
    }
    let info = fs::symlink_metadata(source)?;
    fs::create_dir(dest)?;
    #[cfg(unix)]
    fs::set_permissions(dest, info.permissions())?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = dest.join(entry.file_name());
        if kind.is_symlink() {
            let value = fs::read_link(entry.path())?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&value, &target)?;
            #[cfg(windows)]
            {
                let is_dir = fs::metadata(entry.path())
                    .map(|meta| meta.is_dir())
                    .unwrap_or(false);
                if is_dir {
                    std::os::windows::fs::symlink_dir(&value, &target)?;
                } else {
                    std::os::windows::fs::symlink_file(&value, &target)?;
                }
            }
        } else if kind.is_dir() {
            copy_migration_tree(&entry.path(), &target, depth + 1)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &target)?;
        } else {
            return Err(other("Skill 包含不支持的特殊文件"));
        }
    }
    Ok(())
}

fn create_dir_link(source: &Path, dest: &Path) -> Result<()> {
    #[cfg(unix)]
    std::os::unix::fs::symlink(source, dest)?;
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(source, dest).map_err(|e| {
        other(format!(
            "创建目录链接失败：{e}；可检查 Windows 开发者模式/链接权限"
        ))
    })?;
    Ok(())
}

fn replace_dir_link(dir: &Path, target: &Path) -> Result<()> {
    let parent = dir.parent().ok_or_else(|| other("Skill 入口无效"))?;
    let temp = parent.join(format!(".noending-link-{}", uuid::Uuid::new_v4()));
    create_dir_link(target, &temp)?;
    #[cfg(unix)]
    {
        if let Err(error) = fs::rename(&temp, dir) {
            let _ = remove_link(&temp);
            return Err(error.into());
        }
    }
    #[cfg(windows)]
    {
        if let Err(error) = remove_link(dir) {
            let _ = remove_link(&temp);
            return Err(error);
        }
        if let Err(error) = fs::rename(&temp, dir) {
            let _ = remove_link(&temp);
            return Err(error.into());
        }
    }
    Ok(())
}

/// Swap a wrapper directory for a single link. The wrapper is moved aside
/// first so a failed link creation can restore it unchanged.
fn replace_wrapper_dir(dir: &Path, target: &Path) -> Result<()> {
    let parent = dir.parent().ok_or_else(|| other("Skill 入口无效"))?;
    let backup = parent.join(format!(".noending-old-{}", uuid::Uuid::new_v4()));
    fs::rename(dir, &backup)?;
    if let Err(error) = create_dir_link(target, dir) {
        let _ = fs::rename(&backup, dir);
        return Err(error);
    }
    if let Err(error) = fs::remove_dir_all(&backup) {
        let _ = remove_link(dir);
        let _ = fs::rename(&backup, dir);
        return Err(error.into());
    }
    Ok(())
}
