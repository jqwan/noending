//! Local-directory installs. The root manifests decide which Agents the
//! directory can install to; importing never registers anything — preview is
//! a pure read and `install_local` goes straight to the native CLIs, so no
//! "imported but installed nowhere" state can exist.

use crate::domain::Agent;
use crate::error::{other, Result};
use std::path::{Path, PathBuf};

/// Marketplace manifests: a directory carrying one is a plugin COLLECTION
/// consumed through the Agent's own marketplace machinery, not a single
/// package. Shapes verified against real registered marketplaces: Claude
/// `.claude-plugin/marketplace.json` and Codex `.agents/plugins/marketplace.json`.
const MARKETPLACES: [(&str, MarketKind); 2] = [
    (".claude-plugin/marketplace.json", MarketKind::Claude),
    (".agents/plugins/marketplace.json", MarketKind::Codex),
];

#[derive(Debug, Clone, Copy)]
pub(super) enum MarketKind {
    Claude,
    Codex,
}

impl MarketKind {
    pub(super) fn agent(self) -> Agent {
        match self {
            MarketKind::Claude => Agent::ClaudeCode,
            MarketKind::Codex => Agent::Codex,
        }
    }
}

/// Root manifests in detection priority order. The format determines the
/// Agent that can install the directory.
const MANIFESTS: [(&str, ManifestKind); 4] = [
    ("plugin.json", ManifestKind::Antigravity),
    (".claude-plugin/plugin.json", ManifestKind::Claude),
    (".codex-plugin/plugin.json", ManifestKind::Codex),
    ("package.json", ManifestKind::Pi),
];

#[derive(Debug, Clone, Copy)]
pub(super) enum ManifestKind {
    Antigravity,
    Pi,
    /// Recognized formats NoEnding cannot install from a local directory;
    /// the preview rejects them with the reason instead of a generic miss.
    Claude,
    Codex,
    /// A package.json carrying DSH's official bundle marker (`dsh.bundle`,
    /// per dsh-app-boot: "Bundles are npm packages whose manifest declares
    /// \"dsh\": { \"bundle\": { \"patch\": … } }"). Resolves through the same
    /// package.json as Pi, so the directory offers both targets.
    Dsh,
}

impl ManifestKind {
    pub(super) fn install_target(self) -> Result<Agent> {
        match self {
            ManifestKind::Antigravity => Ok(Agent::Antigravity),
            ManifestKind::Pi => Ok(Agent::Pi),
            ManifestKind::Dsh => Ok(Agent::Dsh),
            ManifestKind::Claude => Err(other(
                "这是 Claude 单插件目录；Claude 不支持直装，请通过本地市场目录（.claude-plugin/marketplace.json）导入",
            )),
            ManifestKind::Codex => Err(other(
                "这是 Codex 单插件目录；Codex 不支持直装，请通过本地市场目录（.agents/plugins/marketplace.json）导入",
            )),
        }
    }

    fn label(self) -> &'static str {
        match self {
            ManifestKind::Antigravity => "Antigravity",
            ManifestKind::Pi => "Pi",
            ManifestKind::Claude => "Claude",
            ManifestKind::Codex => "Codex",
            ManifestKind::Dsh => "DSH",
        }
    }
}

/// Every installable root manifest. A dual-format directory yields both
/// entries — the dialog offers them as multi-select; unsupported manifests
/// never compete, they can only be the reason for a rejection.
pub(super) fn installable_manifests(root: &Path) -> Result<Vec<(PathBuf, ManifestKind)>> {
    let hits: Vec<(&str, ManifestKind)> = MANIFESTS
        .iter()
        .filter(|(relative, _)| root.join(relative).is_file())
        .map(|(relative, kind)| (*relative, *kind))
        .collect();
    if hits.is_empty() {
        return Err(other(
            "该目录根目录没有插件清单（plugin.json / package.json），不是插件包",
        ));
    }
    let mut installable: Vec<(PathBuf, ManifestKind)> = Vec::new();
    for (relative, kind) in &hits {
        match kind {
            ManifestKind::Antigravity => installable.push((root.join(relative), *kind)),
            // Pi accepts any package.json directory, but installing a package
            // with no Pi resources only creates an empty entry — the option
            // requires Pi evidence (the `pi` manifest key or a conventional
            // resource directory).
            ManifestKind::Pi => {
                if has_pi_resources(root) {
                    installable.push((root.join(relative), *kind));
                }
            }
            ManifestKind::Claude | ManifestKind::Codex | ManifestKind::Dsh => {}
        }
    }
    // A package.json carrying the DSH bundle marker installs to DSH even
    // without Pi resources; both options coexist for genuinely dual-purpose
    // packages.
    let has_package_json = hits
        .iter()
        .any(|(relative, kind)| matches!(kind, ManifestKind::Pi) && *relative == "package.json");
    if has_package_json && is_dsh_bundle(&root.join("package.json")) {
        installable.push((root.join("package.json"), ManifestKind::Dsh));
    }
    if installable.is_empty() {
        if hits.iter().any(|(relative, _)| *relative == "package.json") {
            return Err(other(
                "该 package.json 目录没有 Pi 资源（pi 键或 extensions/skills/prompts/themes 目录），也不是 DSH bundle（缺少 dsh.bundle），无法安装",
            ));
        }
        let unsupported = hits
            .iter()
            .map(|(_, kind)| kind.label())
            .collect::<Vec<_>>()
            .join("、");
        return Err(other(format!(
            "这是 {unsupported} 插件包；本地目录安装仅支持 Pi、Antigravity 和 DSH"
        )));
    }
    Ok(installable)
}

/// Pi package evidence per Pi's own docs: conventional resource directories
/// or explicit paths declared under the `pi` key.
fn has_pi_resources(root: &Path) -> bool {
    let keyed = std::fs::read_to_string(root.join("package.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .map(|value| value.get("pi").is_some())
        .unwrap_or(false);
    keyed
        || ["extensions", "skills", "prompts", "themes"]
            .iter()
            .any(|dir| root.join(dir).is_dir())
}

pub(super) fn installable_marketplaces(root: &Path) -> Vec<(PathBuf, MarketKind)> {
    MARKETPLACES
        .iter()
        .filter(|(relative, _)| root.join(relative).is_file())
        .map(|(relative, kind)| (root.join(relative), *kind))
        .collect()
}

/// One plugin listed by a marketplace manifest.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MarketplacePlugin {
    pub name: String,
    pub description: String,
}

/// One installable target found at the directory root: a package format, or
/// a marketplace whose plugins are enumerated for multi-select.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ImportOption {
    pub agent: Agent,
    /// Relative manifest path backing this option.
    pub manifest: String,
    /// Package name, or the marketplace's own name.
    pub name: String,
    pub description: String,
    /// Empty for single packages; the marketplace's plugin list otherwise.
    pub plugins: Vec<MarketplacePlugin>,
}

/// What the import dialog shows before installing. Pure read, nothing is
/// registered or written, so the dialog can preview while the user types.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ImportPreview {
    pub path: String,
    pub kind: ImportKind,
    pub options: Vec<ImportOption>,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportKind {
    Package,
    Marketplace,
}

pub fn preview_import(raw_path: &str) -> Result<ImportPreview> {
    let canonical = canonical_dir(raw_path)?;
    let path = canonical.to_string_lossy().into_owned();
    // A marketplace root wins over single-package manifests: the directory's
    // identity is the collection, and both Agents install from collections
    // through their own marketplace machinery.
    let markets = installable_marketplaces(&canonical);
    if !markets.is_empty() {
        let options = markets
            .into_iter()
            .map(|(manifest, kind)| {
                let relative = manifest
                    .strip_prefix(&canonical)
                    .unwrap_or(&manifest)
                    .to_string_lossy()
                    .into_owned();
                let (name, description, plugins) = read_marketplace_meta(&manifest);
                ImportOption {
                    agent: kind.agent(),
                    manifest: relative,
                    name,
                    description,
                    plugins,
                }
            })
            .collect();
        return Ok(ImportPreview {
            path,
            kind: ImportKind::Marketplace,
            options,
        });
    }
    let options = installable_manifests(&canonical)?
        .into_iter()
        .map(|(manifest, kind)| {
            let agent = kind.install_target().expect("installable manifests only");
            let relative = manifest
                .strip_prefix(&canonical)
                .unwrap_or(&manifest)
                .to_string_lossy()
                .into_owned();
            let (name, description) = read_manifest_meta(&manifest);
            ImportOption {
                agent,
                manifest: relative,
                name,
                description,
                plugins: Vec::new(),
            }
        })
        .collect();
    Ok(ImportPreview {
        path,
        kind: ImportKind::Package,
        options,
    })
}

/// (marketplace name, its description, its listed plugins)
pub(super) fn read_marketplace_meta(manifest: &Path) -> (String, String, Vec<MarketplacePlugin>) {
    let parsed = std::fs::read_to_string(manifest)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok());
    let Some(value) = parsed else {
        return (String::new(), String::new(), Vec::new());
    };
    let plugins = value
        .get("plugins")
        .and_then(serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let name = entry.get("name")?.as_str()?.to_string();
                    let description = entry
                        .get("description")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    Some(MarketplacePlugin { name, description })
                })
                .collect()
        })
        .unwrap_or_default();
    let name = value
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let description = value
        .get("description")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    (name, description, plugins)
}

/// DSH's authoritative bundle marker: a `dsh.bundle` object in package.json
/// (its `patch` names the cordis patch file(s) of the bundle).
fn is_dsh_bundle(manifest: &Path) -> bool {
    std::fs::read_to_string(manifest)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|value| {
            value
                .get("dsh")
                .and_then(|dsh| dsh.get("bundle"))
                .map(serde_json::Value::is_object)
        })
        .unwrap_or(false)
}

pub(super) fn canonical_dir(raw_path: &str) -> Result<PathBuf> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return Err(other("请填写插件包目录"));
    }
    let root = Path::new(trimmed);
    if !root.is_dir() {
        return Err(other("目录不存在，请检查路径"));
    }
    std::fs::canonicalize(root).map_err(|e| other(format!("无法读取目录：{e}")))
}

pub(super) fn read_manifest_meta(manifest: &Path) -> (String, String) {
    std::fs::read_to_string(manifest)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .map(|manifest| {
            let name = manifest
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let description = manifest
                .get("description")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            (name, description)
        })
        .unwrap_or_default()
}
