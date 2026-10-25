//! Skill discovery and management. Agent load directories remain authoritative.
mod bundled;
mod files;
mod inventory;
mod operations;
mod registry;

use crate::{
    domain::Agent,
    error::{other, Result},
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub use bundled::prepare_noending_skill;
pub(crate) use bundled::FILES as BUNDLED_FILES;
pub use inventory::{
    catalog, detail, detail_in_catalog, invalidate_catalog, list, list_catalog, merge_inventories,
};
pub use operations::{
    adopt_source, detail_project, detail_registered, disable_skill, disable_skill_all,
    disable_skill_project, discover_library_sources, enable_skill, enable_skill_all,
    enable_skill_project, find_source_path as find_skill_source, inherit_project_skills,
    install_skill, link_skill, migrate_skill, prepare_source, preview_skill_migration,
    preview_skill_registration, project_scopes_from_inventory, register_source_dir,
    registered_uninstall_preview, skill_source_kind, uninstall_preview, uninstall_registered_skill,
    uninstall_skill, unregister_skill, ProjectSkillScope, ProjectWorkspaceScope,
    SkillDisableResult, SkillInstallResult, SkillMigrationPreview, SkillMigrationResult,
    SkillScopeEntry, SkillSourceKind,
};
pub use registry::{
    find_source, load as load_skill_registry, merge as merge_skill_sources, register_source,
    remove_source, save as save_skill_registry, source_id, source_revision, SkillRegistry,
    SkillSource,
};

pub(crate) static MUTATION: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Scope {
    pub project_id: Option<String>,
    pub workspace_path_id: Option<String>,
}

/// Environment is captured once, never changed for an operation or a test.
#[derive(Clone)]
pub struct Roots {
    pub home: PathBuf,
    pub codex: PathBuf,
    pub claude: PathBuf,
    pub pi: PathBuf,
}

impl Roots {
    pub fn current() -> Result<Self> {
        let home =
            crate::platform::paths::resolve_home().ok_or_else(|| other("无法确定用户目录"))?;
        let override_or = |key: &str, default: PathBuf| {
            std::env::var_os(key)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or(default)
        };
        Ok(Self {
            codex: override_or("CODEX_HOME", home.join(".codex")),
            claude: override_or("CLAUDE_CONFIG_DIR", home.join(".claude")),
            pi: override_or("PI_CODING_AGENT_DIR", home.join(".pi/agent")),
            home,
        })
    }
}

/// An Agent load directory. Never a Skill source; NoEnding's source library is not a Location.
#[derive(Debug, Clone, Serialize)]
pub struct Location {
    pub id: String,
    pub agent: Option<Agent>,
    pub scope: String,
    pub path: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Resource {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Canonical source `SKILL.md` path. Configuration entries live in
    /// `entry_paths`; this never points at a link inside an Agent directory
    /// unless that physical directory is the source itself.
    pub path: String,
    /// Configuration locations aligned with `entry_paths`, never source storage.
    pub locations: Vec<String>,
    /// Configuration entries (links or physical directories) aligned with
    /// `locations`. A physical entry is the source directory itself.
    pub entry_paths: Vec<String>,
    /// Aligned with `entry_paths`: false when the entry is a real directory
    /// whose removal would delete the Skill source instead of just unlinking it.
    pub entry_links: Vec<bool>,
    pub revision: String,
    /// True for an explicitly registered source or one in the NoEnding library.
    pub registered: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Inventory {
    pub resources: Vec<Resource>,
    pub locations: Vec<Location>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResourceDetail {
    pub resource: Resource,
    pub content: String,
}

pub type ProjectPaths = Vec<(String, String, Vec<(String, PathBuf)>)>;

/// One catalog scan answers everything the detail page renders.
#[derive(Debug, Clone, Serialize)]
pub struct DetailBundle {
    pub detail: ResourceDetail,
    pub locations: Vec<Location>,
    pub scopes: Vec<ProjectSkillScope>,
    pub source_kind: Option<SkillSourceKind>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OperationResult {
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UninstallPreview {
    pub delete_source_required: bool,
    pub entry_count: usize,
    pub source_path: String,
}

pub(crate) fn key(value: impl AsRef<[u8]>) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(value.as_ref()))
}

pub(crate) fn location(agent: Option<Agent>, scope: &str, path: PathBuf, label: &str) -> Location {
    let path = path.to_string_lossy().into_owned();
    Location {
        id: key(format!("skill:{agent:?}:{scope}:{path}")),
        agent,
        scope: scope.into(),
        path,
        label: label.into(),
    }
}

pub(crate) fn check_revision(actual: &str, expected: &str) -> Result<()> {
    if actual != expected {
        return Err(other("配置已被修改，请刷新后重试"));
    }
    Ok(())
}
