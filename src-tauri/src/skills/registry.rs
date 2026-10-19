//! Registered Skill sources may live in the NoEnding library or remain in an
//! external directory. Native Agent directories determine enablement; registry
//! membership keeps referenced sources managed after their links are removed.
use super::{files, Inventory, Resource};
use crate::{
    error::{other, Result},
    storage::Db,
};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

// Keep the existing storage key so registered Skill sources retain their identities.
pub(super) const KEY: &str = "extensions.skill_sources";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SkillRegistry {
    #[serde(default)]
    pub sources: Vec<SkillSource>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillSource {
    pub id: String,
    /// Canonical `SKILL.md` path of the source.
    pub path: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
}

pub fn load(db: &Db) -> Result<SkillRegistry> {
    match db.get_setting(KEY)? {
        Some(raw) => {
            serde_json::from_str(&raw).map_err(|e| other(format!("Skill 源清单无法读取：{e}")))
        }
        None => Ok(SkillRegistry::default()),
    }
}

pub fn save(db: &Db, registry: &SkillRegistry) -> Result<()> {
    db.set_setting(KEY, &serde_json::to_string(registry)?)
}

pub fn source_id(path: &str) -> String {
    super::key(format!("skill:{path}"))
}

pub fn find_source<'a>(registry: &'a SkillRegistry, id: &str) -> Option<&'a SkillSource> {
    registry.sources.iter().find(|source| source.id == id)
}

pub fn register_source<'a>(
    registry: &'a mut SkillRegistry,
    path: &str,
    name: &str,
    description: &str,
) -> &'a mut SkillSource {
    if let Some(index) = registry
        .sources
        .iter()
        .position(|source| source.path == path)
    {
        let source = &mut registry.sources[index];
        source.name = name.into();
        source.description = description.into();
        return source;
    }
    registry.sources.push(SkillSource {
        id: source_id(path),
        path: path.into(),
        name: name.into(),
        description: description.into(),
    });
    registry.sources.last_mut().expect("source was just pushed")
}

pub fn remove_source(registry: &mut SkillRegistry, id: &str) {
    registry.sources.retain(|source| source.id != id);
}

pub fn source_revision(source: &SkillSource) -> String {
    let path = Path::new(&source.path);
    if path.is_file() {
        files::revision(path).unwrap_or_else(|_| missing_revision(&source.path))
    } else {
        missing_revision(&source.path)
    }
}

fn missing_revision(path: &str) -> String {
    super::key(format!("missing:{path}"))
}

/// Explicit registration and placement in the NoEnding library both constitute
/// management. Storage location is independent of registration and enablement.
pub fn merge(
    registry: &SkillRegistry,
    inventory: &mut Inventory,
    include_inactive: bool,
    managed_root: &Path,
) {
    let managed = fs::canonicalize(managed_root).unwrap_or_else(|_| managed_root.to_path_buf());
    for row in inventory.resources.iter_mut() {
        let source = Path::new(&row.path);
        row.registered = find_source(registry, &row.id).is_some()
            || resolves_inside(&managed, source.parent().unwrap_or(source));
        let Some(source) = find_source(registry, &row.id) else {
            continue;
        };
        if row.name.is_empty() {
            row.name = source.name.clone();
        }
        if row.description.is_empty() {
            row.description = source.description.clone();
        }
        if Path::new(&source.path).is_file() {
            row.path = source.path.clone();
        }
    }
    if include_inactive {
        for source in &registry.sources {
            if !inventory.resources.iter().any(|row| row.id == source.id) {
                inventory.resources.push(inactive(source));
            }
        }
    }
}

/// True when the Skill source directory resolves inside the NoEnding library.
pub(crate) fn source_is_managed(managed_root: &Path, source_dir: &Path) -> bool {
    let managed = fs::canonicalize(managed_root).unwrap_or_else(|_| managed_root.to_path_buf());
    resolves_inside(&managed, source_dir)
}

fn resolves_inside(root: &Path, path: &Path) -> bool {
    fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .starts_with(root)
}

fn inactive(source: &SkillSource) -> Resource {
    let present = Path::new(&source.path).is_file();
    Resource {
        id: source.id.clone(),

        name: source.name.clone(),
        description: source.description.clone(),
        path: source.path.clone(),
        locations: Vec::new(),
        entry_paths: Vec::new(),
        entry_links: Vec::new(),
        registered: true,
        revision: source_revision(source),
        error: if present {
            None
        } else {
            Some("源目录不存在或缺少 SKILL.md。".into())
        },
    }
}
