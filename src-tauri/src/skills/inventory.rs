use super::{files, location, Inventory, Location, ResourceDetail, Roots};
use crate::{
    domain::Agent,
    error::{other, Result},
};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

pub(super) fn locations(roots: &Roots, cwd: Option<&Path>) -> Vec<Location> {
    let mut rows = vec![
        location(
            None,
            "user",
            roots.home.join(".agents/skills"),
            "共享 Skills",
        ),
        location(
            Some(Agent::Codex),
            "user",
            roots.codex.join("skills"),
            "Codex Skills",
        ),
        location(
            Some(Agent::ClaudeCode),
            "user",
            roots.claude.join("skills"),
            "Claude Code Skills",
        ),
        location(
            Some(Agent::Pi),
            "user",
            roots.pi.join("skills"),
            "Pi Skills",
        ),
        location(
            Some(Agent::Antigravity),
            "user",
            roots.home.join(".gemini/antigravity-cli/skills"),
            "Antigravity Skills",
        ),
        location(
            Some(Agent::Antigravity),
            "user",
            roots.home.join(".gemini/config/skills"),
            "Antigravity 兼容目录",
        ),
    ];
    if let Some(cwd) = cwd {
        // These are explicit configuration destinations, not an invented
        // NoEnding inheritance system. Native CLI trust/ancestor rules still apply.
        for (agent, rel, label) in [
            (None, ".agents/skills", "共享 Skills"),
            (
                Some(Agent::ClaudeCode),
                ".claude/skills",
                "Claude Code Skills",
            ),
            (Some(Agent::Pi), ".pi/skills", "Pi Skills"),
        ] {
            rows.push(location(agent, "project", cwd.join(rel), label));
        }
    }
    rows
}

/// Scan only a project's Skill load directories.
pub(super) fn list_project_skills(roots: &Roots, cwd: &Path) -> Inventory {
    scan(
        locations(roots, Some(cwd))
            .into_iter()
            .filter(|loc| loc.scope == "project")
            .collect(),
    )
}

pub fn list_catalog(roots: &Roots, cwds: &[PathBuf]) -> Inventory {
    let mut inventories = vec![list(roots, None)];
    inventories.extend(cwds.iter().map(|cwd| list_project_skills(roots, cwd)));
    merge_inventories(inventories)
}

pub fn list(roots: &Roots, cwd: Option<&Path>) -> Inventory {
    scan(locations(roots, cwd))
}

fn scan(locations: Vec<Location>) -> Inventory {
    let mut inventory = Inventory {
        locations,
        ..Default::default()
    };
    let mut seen = HashSet::new();
    for location in inventory.locations.clone() {
        super::operations::scan(&location, &mut seen, &mut inventory);
    }
    inventory
        .resources
        .sort_by_key(|row| row.name.to_lowercase());
    inventory
}

pub fn merge_inventories(inventories: Vec<Inventory>) -> Inventory {
    let mut result = Inventory::default();
    for inventory in inventories {
        for loc in inventory.locations {
            if !result
                .locations
                .iter()
                .any(|existing| existing.id == loc.id)
            {
                result.locations.push(loc);
            }
        }
        for warning in inventory.warnings {
            if !result.warnings.contains(&warning) {
                result.warnings.push(warning);
            }
        }
        for row in inventory.resources {
            let Some(existing) = result.resources.iter_mut().find(|r| r.id == row.id) else {
                result.resources.push(row);
                continue;
            };
            existing.registered |= row.registered;
            if existing.error.is_none() {
                existing.error = row.error;
            }
            for (index, location) in row.locations.iter().enumerate() {
                if existing.locations.contains(location) {
                    continue;
                }
                let Some(entry) = row.entry_paths.get(index) else {
                    continue;
                };
                existing.locations.push(location.clone());
                existing.entry_paths.push(entry.clone());
                existing
                    .entry_links
                    .push(row.entry_links.get(index).copied().unwrap_or(false));
            }
        }
    }
    result
        .resources
        .sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    result
}

pub fn detail(roots: &Roots, cwd: Option<&Path>, id: &str) -> Result<ResourceDetail> {
    detail_in_catalog(&list(roots, cwd), id)?.ok_or_else(|| other("技能已不存在，请刷新"))
}

/// Reuse the catalog scan for detail and scope projections.
pub fn detail_in_catalog(inventory: &Inventory, id: &str) -> Result<Option<ResourceDetail>> {
    let Some(resource) = inventory.resources.iter().find(|row| row.id == id).cloned() else {
        return Ok(None);
    };
    Ok(Some(ResourceDetail {
        content: files::read(Path::new(&resource.path))?,
        resource,
    }))
}
