use super::{files, Inventory, Location, OperationResult, Resource, Roots};
use crate::error::{other, Result};
use serde::Deserialize;
use std::{collections::HashSet, fs, path::Path};

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
        if file.is_file() {
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
                            enabled: None,
                            revision: super::key(content),
                            managed: !loc.writable || loc.package.is_some(),
                            error,
                            skill_controls: Vec::new(),
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

fn matches_skill(value: &str, path: &str) -> bool {
    let candidate = Path::new(value);
    let file = if candidate.is_dir() {
        candidate.join("SKILL.md")
    } else {
        candidate.to_path_buf()
    };
    fs::canonicalize(file)
        .ok()
        .is_some_and(|p| p == Path::new(path))
}

pub(super) fn controls(roots: &Roots, cwd: Option<&Path>, inventory: &mut Inventory) {
    for (scope, path) in std::iter::once(("user", roots.codex.join("config.toml")))
        .chain(cwd.map(|c| ("project", c.join(".codex/config.toml"))))
    {
        let raw = match files::read(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let value: serde_json::Value = if raw.trim().is_empty() {
            serde_json::json!({})
        } else {
            match toml_edit::de::from_str(&raw) {
                Ok(v) => v,
                Err(_) => continue,
            }
        };
        for resource in inventory
            .resources
            .iter_mut()
            .filter(|r| r.kind == "skill" && !r.managed)
        {
            if !resource.locations.iter().any(|id| {
                inventory.locations.iter().any(|l| {
                    &l.id == id
                        && (l.scope == scope || scope == "project" && l.scope == "user")
                        && (l.agent.is_none() || l.agent == Some(crate::domain::Agent::Codex))
                })
            }) {
                continue;
            }
            let enabled = value
                .get("skills")
                .and_then(|v| v.get("config"))
                .and_then(serde_json::Value::as_array)
                .and_then(|a| {
                    a.iter().rev().find(|v| {
                        v.get("path")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|p| matches_skill(p, &resource.path))
                    })
                })
                .and_then(|v| v.get("enabled"))
                .and_then(serde_json::Value::as_bool);
            resource.skill_controls.push(super::SkillControl {
                agent: crate::domain::Agent::Codex,
                scope: scope.into(),
                enabled,
                revision: super::key(&raw),
            });
        }
    }
}

/// Codex has a documented per-path enable setting. This edits configuration,
/// never SKILL.md, and does not claim to disable the skill for other Agents.
pub fn set_skill_state(
    roots: &Roots,
    cwd: Option<&Path>,
    id: &str,
    scope: &str,
    expected: &str,
    enabled: Option<bool>,
) -> Result<OperationResult> {
    let _guard = super::MUTATION
        .lock()
        .map_err(|_| other("配置操作锁不可用"))?;
    let inventory = super::list(roots, cwd);
    let row = inventory
        .resources
        .iter()
        .find(|r| r.id == id && r.kind == "skill" && !r.managed)
        .ok_or_else(|| other("此 Skill 不支持独立启停"))?;
    let control = row
        .skill_controls
        .iter()
        .find(|c| c.scope == scope)
        .ok_or_else(|| other("此作用域不支持独立启停"))?;
    super::check_revision(&control.revision, expected)?;
    let path = match scope {
        "user" => roots.codex.join("config.toml"),
        "project" => cwd
            .ok_or_else(|| other("缺少项目目录"))?
            .join(".codex/config.toml"),
        _ => return Err(other("作用域无效")),
    };
    let raw = files::read(&path)?;
    let mut doc = raw.parse::<toml_edit::Document>().map_err(other)?;
    if doc.get("skills").is_none() {
        doc["skills"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    if doc["skills"].get("config").is_none() {
        doc["skills"]["config"] = toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new());
    }
    let items = doc["skills"]["config"]
        .as_array_of_tables_mut()
        .ok_or_else(|| other("skills.config 不是表数组，无法安全修改"))?;
    let keep: Vec<_> = items
        .iter()
        .filter(|item| {
            !item
                .get("path")
                .and_then(toml_edit::Item::as_str)
                .is_some_and(|p| matches_skill(p, &row.path))
        })
        .cloned()
        .collect();
    items.clear();
    for item in keep {
        items.push(item);
    }
    if let Some(enabled) = enabled {
        let mut item = toml_edit::Table::new();
        item["path"] = toml_edit::value(&row.path);
        item["enabled"] = toml_edit::value(enabled);
        items.push(item);
    }
    if items.is_empty() {
        if let Some(skills) = doc["skills"].as_table_like_mut() {
            skills.remove("config");
        }
    }
    files::write(&path, expected, &doc.to_string())?;
    Ok(OperationResult {
        message: "Codex 的 Skill 配置已保存，重新打开 Codex 后生效；其他 Agent 不受此开关控制"
            .into(),
    })
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

pub fn install_skill(
    roots: &Roots,
    cwd: Option<&Path>,
    source: &Path,
    location_id: &str,
) -> Result<OperationResult> {
    install(roots, cwd, source, location_id, false)
}

pub fn link_skill(
    roots: &Roots,
    cwd: Option<&Path>,
    source: &Path,
    location_id: &str,
) -> Result<OperationResult> {
    install(roots, cwd, source, location_id, true)
}

fn install(
    roots: &Roots,
    cwd: Option<&Path>,
    source: &Path,
    location_id: &str,
    link: bool,
) -> Result<OperationResult> {
    let _guard = super::MUTATION
        .lock()
        .map_err(|_| other("配置操作锁不可用"))?;
    let loc = super::inventory::locations(roots, cwd)
        .into_iter()
        .find(|l| l.id == location_id && l.kind == "skill" && l.writable)
        .ok_or_else(|| other("Skill 安装位置无效"))?;
    let source = fs::canonicalize(source)?;
    let meta = metadata(&files::read(&source.join("SKILL.md"))?)?;
    if meta.name.len() > 64
        || !meta
            .name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || meta.name.starts_with('-')
        || meta.name.ends_with('-')
        || meta.name.contains("--")
    {
        return Err(other(
            "Skill 名称应为不超过 64 字符的小写字母、数字和单个连字符",
        ));
    }
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
    Ok(OperationResult {
        message: "Skill 已安装；已有终端请按 Agent 的方式重新加载或重新打开".into(),
    })
}

pub fn remove_skill(
    roots: &Roots,
    cwd: Option<&Path>,
    id: &str,
    entry_path: &str,
    expected: &str,
) -> Result<OperationResult> {
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
    if !loc.writable || loc.package.is_some() {
        return Err(other("内置或插件包提供的 Skill 不能单独删除"));
    }
    let dir = Path::new(entry_path)
        .parent()
        .ok_or_else(|| other("Skill 路径无效"))?;
    let root = Path::new(&loc.path);
    if dir == root || !dir.starts_with(root) {
        return Err(other("不能删除 Skill 根目录"));
    }
    // Refuse recursive source deletion through a link in an ancestor. Removing
    // the visible link itself is safe and leaves its target untouched.
    let mut cursor = dir.parent();
    while let Some(parent) = cursor {
        if is_link(parent) {
            return Err(other("该 Skill 位于链接目录内，请打开目录管理加载入口"));
        }
        if parent == root {
            break;
        }
        cursor = parent.parent();
    }
    if is_link(dir) {
        #[cfg(windows)]
        fs::remove_dir(dir)?;
        #[cfg(not(windows))]
        fs::remove_file(dir)?;
    } else {
        fs::remove_dir_all(dir)?;
    }
    Ok(OperationResult {
        message: "Skill 加载入口已移除".into(),
    })
}
