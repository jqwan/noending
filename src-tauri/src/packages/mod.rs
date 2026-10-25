//! Plugin package discovery and import registration. Native Agent registries
//! and configuration files remain the only source of truth; discovery is an
//! offline read and never launches an Agent CLI.
//!
//! A package belongs to one Agent. Entries are never merged across Agents by
//! name; only an imported local source and an Agent install of that same
//! canonical directory collapse into one entry.

mod antigravity;
mod claude;
mod codex;
mod contents;
pub mod dsh;
mod files;
mod imported;
mod operations;
mod pi;
mod qoder;
mod workbuddy;
mod zcode;

use crate::{domain::Agent, error::Result};
use serde::Serialize;
use std::path::PathBuf;

pub use imported::{preview_import, ImportKind, ImportOption, ImportPreview, MarketplacePlugin};
/// One agent's plugin selection inside a local marketplace install.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct MarketplaceSelection {
    pub agent: Agent,
    pub plugins: Vec<String>,
}
pub use operations::{
    cli_args, install_local, install_local_marketplace, marketplace_add_args,
    marketplace_install_args, set_enabled, uninstall, PackageAction, PackageOperationResult,
};

/// Reuses the Skill roots so environment overrides (`CODEX_HOME`,
/// `CLAUDE_CONFIG_DIR`, `PI_CODING_AGENT_DIR`) stay consistent across features.
pub use crate::skills::Roots;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    User,
    Project,
    Local,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Scope::User => "用户级",
            Scope::Project => "项目级",
            Scope::Local => "本机项目",
        }
    }
}

/// Native enablement as the owning Agent stores it. `Unset` means the Agent
/// applies its default rule; it is not the same as disabled. `Unknown` means
/// NoEnding found no enablement evidence at all — the Agent owns the state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToggleState {
    Enabled,
    Disabled,
    Unset,
    Unknown,
}

/// Where an installed package came from, derived from native evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// Installed from a marketplace (`name@market`).
    Market,
    /// Shipped by the Agent application itself.
    Bundled,
    /// Shipped by NoEnding (terminal identity hooks and similar). Pinned
    /// against snake_case's hump-splitting, which yields `no_ending_managed`.
    #[serde(rename = "noending_managed")]
    NoEndingManaged,
    /// Installed from a local directory or fetched by the Agent from a URL.
    Local,
    /// Loaded into one DSH profile (the profile is DSH's isolation dimension).
    Profile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Capabilities {
    pub uninstall: bool,
    pub toggle: bool,
}

impl Capabilities {
    pub fn none() -> Self {
        Self {
            uninstall: false,
            toggle: false,
        }
    }

    pub fn full() -> Self {
        Self {
            uninstall: true,
            toggle: true,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ScopeState {
    pub scope: Scope,
    pub state: ToggleState,
    /// Native file that holds this scope's enablement. Derived from the
    /// registry for scopes the Agent has never written, so the file may not
    /// exist yet — `config_exists` distinguishes that from a broken path.
    pub config_path: String,
    pub config_exists: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackageEntry {
    pub id: String,
    pub agent: Option<Agent>,
    pub name: String,
    pub version: String,
    pub description: String,
    pub origin: Origin,
    pub marketplace: Option<String>,
    pub install_path: Option<String>,
    pub scopes: Vec<ScopeState>,
    pub capabilities: Capabilities,
    /// Hash of the native registry/config content this entry was read from.
    pub revision: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PackageContents {
    /// Skill directories inside the package (relative to the package root).
    pub skills: Vec<String>,
    /// MCP configuration files inside the package (relative paths).
    pub mcp_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackageDetail {
    pub entry: PackageEntry,
    pub contents: PackageContents,
    /// Absolute package root used for the contents scan.
    pub root: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PackageInventory {
    pub packages: Vec<PackageEntry>,
    pub warnings: Vec<String>,
}

/// Antigravity/WorkBuddy roots are fixed under the user's home; only Codex,
/// Claude and Pi offer environment overrides.
pub(crate) fn gemini_dir(roots: &Roots) -> PathBuf {
    roots.home.join(".gemini")
}

pub(crate) fn workbuddy_dir(roots: &Roots) -> PathBuf {
    roots.home.join(".workbuddy")
}

pub(crate) fn qoder_dir(roots: &Roots) -> PathBuf {
    roots.home.join(".qoder-cn")
}

pub(crate) fn zcode_dir(roots: &Roots) -> PathBuf {
    roots.home.join(".zcode/cli")
}

pub(crate) fn dsh_dir(roots: &Roots) -> PathBuf {
    roots.home.join(".dsh")
}

/// Offline discovery across every integrated provider. Each provider appends
/// its own warnings and keeps going, so one broken registry never hides the
/// others. User-level scope only: project-level plugin configuration belongs
/// to each Agent.
pub fn discover(roots: &Roots) -> PackageInventory {
    let mut inventory = PackageInventory::default();
    inventory
        .packages
        .extend(codex::discover(roots, &mut inventory.warnings));
    inventory
        .packages
        .extend(claude::discover(roots, &mut inventory.warnings));
    inventory
        .packages
        .extend(pi::discover(roots, &mut inventory.warnings));
    inventory
        .packages
        .extend(antigravity::discover(roots, &mut inventory.warnings));
    inventory
        .packages
        .extend(workbuddy::discover(roots, &mut inventory.warnings));
    inventory
        .packages
        .extend(qoder::discover(roots, &mut inventory.warnings));
    inventory
        .packages
        .extend(zcode::discover(roots, &mut inventory.warnings));
    inventory
        .packages
        .extend(dsh::discover(roots, &mut inventory.warnings));
    inventory
}

/// Detail read for one entry. The inventory is passed in so the caller controls
/// how fresh the discovery is; contents are read from the package root.
pub fn detail(inventory: &PackageInventory, id: &str) -> Result<Option<PackageDetail>> {
    let Some(entry) = inventory.packages.iter().find(|entry| entry.id == id) else {
        return Ok(None);
    };
    let root = entry
        .install_path
        .as_deref()
        .filter(|path| std::path::Path::new(path).is_dir());
    let contents = root
        .map(|root| {
            // Contents parsing warnings are view-level noise; the scan still
            // reports whatever resolved.
            let mut scratch = Vec::new();
            contents::scan(std::path::Path::new(root), &mut scratch)
        })
        .unwrap_or_default();
    Ok(Some(PackageDetail {
        entry: entry.clone(),
        contents,
        root: root.map(str::to_string),
    }))
}
