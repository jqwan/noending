use super::{files, location, Inventory, Location, ResourceDetail, Roots};
use crate::{
    domain::Agent,
    error::{other, Result},
};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
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
        // agy's own embedded docs name ~/.gemini/config/skills as THE
        // user-level skill root (workspace level rides the shared
        // <workspace>/.agents/skills); the desktop shares this config dir.
        // The old antigravity-cli/skills target had no reader and was
        // removed.
        location(
            Some(Agent::Antigravity),
            "user",
            roots.home.join(".gemini/config/skills"),
            "Antigravity Skills",
        ),
        // WorkBuddy does not read the shared ~/.agents root; its own user
        // skills directory is the only link target for it.
        location(
            Some(Agent::WorkBuddy),
            "user",
            roots.home.join(".workbuddy/skills"),
            "WorkBuddy Skills",
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
            (
                Some(Agent::WorkBuddy),
                ".workbuddy/skills",
                "WorkBuddy Skills",
            ),
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

/// Board and detail are separate routes, so entering a resource immediately
/// after the board scans the same directories again. Keep the merged scan for a
/// few seconds; every mutation below invalidates it explicitly.
const CATALOG_TTL: Duration = Duration::from_secs(10);

struct CachedCatalog {
    key: String,
    at: Instant,
    inventory: Inventory,
}

static CATALOG: OnceLock<Mutex<Option<CachedCatalog>>> = OnceLock::new();

fn catalog_cache() -> &'static Mutex<Option<CachedCatalog>> {
    CATALOG.get_or_init(|| Mutex::new(None))
}

pub fn invalidate_catalog() {
    *catalog_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

fn catalog_key(roots: &Roots, cwds: &[PathBuf]) -> String {
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

pub fn catalog(
    roots: &Roots,
    cwds: &[PathBuf],
    skill_registry: &super::SkillRegistry,
    managed_root: &Path,
) -> Inventory {
    let key = catalog_key(roots, cwds);
    let mut cache = catalog_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(cached) = cache.as_ref() {
        if cached.key == key && cached.at.elapsed() < CATALOG_TTL {
            return cached.inventory.clone();
        }
    }
    let mut inventory = list_catalog(roots, cwds);
    super::registry::merge(skill_registry, &mut inventory, true, managed_root);
    *cache = Some(CachedCatalog {
        key,
        at: Instant::now(),
        inventory: inventory.clone(),
    });
    inventory
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
