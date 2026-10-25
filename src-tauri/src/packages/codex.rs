//! Codex plugins live in `config.toml`'s `plugins` table keyed by
//! `name@market`; the cache directory only shows where a single-version
//! install unpacked. Bundled runtime markets are Agent-owned: they can be
//! toggled but not removed. NoEnding manages the user-level scope only —
//! project-level `config.toml` files belong to the Agent's own configuration.

use super::{files, Capabilities, Origin, PackageEntry, Roots, Scope, ScopeState, ToggleState};
use crate::domain::Agent;

pub(super) fn discover(roots: &Roots, warnings: &mut Vec<String>) -> Vec<PackageEntry> {
    let config = roots.codex.join("config.toml");
    let raw = match files::read(&config) {
        Ok(raw) => raw,
        Err(e) => {
            warnings.push(format!("{}：{e}", config.display()));
            return Vec::new();
        }
    };
    if raw.is_empty() {
        return Vec::new();
    }
    let Ok(doc) = raw.parse::<toml_edit::Document>() else {
        warnings.push(format!("{}：config.toml 无法解析", config.display()));
        return Vec::new();
    };
    let Some(plugins) = doc.get("plugins").and_then(toml_edit::Item::as_table_like) else {
        return Vec::new();
    };
    plugins
        .iter()
        .filter(|(_, item)| item.as_table_like().is_some())
        .map(|(selector, item)| {
            let settings = item.as_table_like().expect("filtered above");
            let enabled = settings.get("enabled").and_then(toml_edit::Item::as_bool);
            let (name, marketplace) = match selector.rsplit_once('@') {
                Some(pair) => pair,
                None => (selector, ""),
            };
            let origin = if matches!(marketplace, "openai-bundled" | "openai-primary-runtime") {
                Origin::Bundled
            } else {
                Origin::Market
            };
            let capabilities = if origin == Origin::Bundled {
                Capabilities {
                    uninstall: false,
                    toggle: true,
                }
            } else {
                Capabilities::full()
            };
            PackageEntry {
                id: files::key(format!("package:codex:{selector}")),
                agent: Some(Agent::Codex),
                name: name.to_string(),
                version: String::new(),
                description: String::new(),
                origin,
                marketplace: (!marketplace.is_empty()).then(|| marketplace.to_string()),
                install_path: package_root(roots, name, marketplace),
                scopes: vec![ScopeState {
                    scope: Scope::User,
                    state: match enabled {
                        Some(true) => ToggleState::Enabled,
                        Some(false) => ToggleState::Disabled,
                        None => ToggleState::Unset,
                    },
                    config_path: config.to_string_lossy().into_owned(),
                    config_exists: config.is_file(),
                }],
                capabilities,
                revision: files::revision(&[&raw]),
                error: None,
            }
        })
        .collect()
}

/// The cache holds `<market>/<name>/<version>/`; with several versions present
/// there is no native evidence for the active one, so no path is reported.
fn package_root(roots: &Roots, name: &str, market: &str) -> Option<String> {
    if market.is_empty() {
        return None;
    }
    let dir = roots.codex.join("plugins/cache").join(market).join(name);
    let versions: Vec<_> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    (versions.len() == 1).then(|| versions[0].to_string_lossy().into_owned())
}
