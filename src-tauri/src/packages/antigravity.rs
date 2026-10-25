//! Antigravity installs plugins as directories with a root `plugin.json`
//! manifest; enablement lives in `~/.gemini/config/config.json`. NoEnding's
//! own terminal-identity plugin is marked and kept read-only.

use super::{
    files, gemini_dir, Capabilities, Origin, PackageEntry, Roots, Scope, ScopeState, ToggleState,
};
use crate::domain::Agent;
use serde_json::Value;

pub(super) fn discover(roots: &Roots, warnings: &mut Vec<String>) -> Vec<PackageEntry> {
    let gemini = gemini_dir(roots);
    let config_raw = files::read(&gemini.join("config/config.json")).unwrap_or_default();
    let config: Value = if config_raw.is_empty() {
        serde_json::json!({})
    } else {
        match serde_json::from_str(&config_raw) {
            Ok(value) => value,
            Err(e) => {
                warnings.push(format!(
                    "{}：{e}",
                    gemini.join("config/config.json").display()
                ));
                serde_json::json!({})
            }
        }
    };
    let mut entries = Vec::new();
    for base in [
        gemini.join("config/plugins"),
        gemini.join("antigravity-cli/plugins"),
    ] {
        let Ok(dirs) = std::fs::read_dir(&base) else {
            continue;
        };
        for dir in dirs.flatten() {
            let root = dir.path();
            if !root.is_dir() {
                continue;
            }
            let manifest_path = root.join("plugin.json");
            let Ok(manifest_raw) = files::read(&manifest_path) else {
                continue;
            };
            if manifest_raw.is_empty() {
                continue;
            }
            let Ok(manifest) = serde_json::from_str::<Value>(&manifest_raw) else {
                warnings.push(format!("{}：无法解析", manifest_path.display()));
                continue;
            };
            let Some(name) = manifest.get("name").and_then(Value::as_str) else {
                continue;
            };
            let noending_managed =
                root.join(".noending-managed").is_file() || name.starts_with("noending-");
            let enabled = config
                .get("plugins")
                .and_then(|plugins| plugins.get(name))
                .and_then(|plugin| plugin.get("enabled"))
                .and_then(Value::as_bool);
            // The native config records install provenance per plugin
            // (`installedFrom.marketplace`); without it the install came from
            // a local directory or an untracked source.
            let installed_from = config
                .get("plugins")
                .and_then(|plugins| plugins.get(name))
                .and_then(|plugin| plugin.get("installedFrom"));
            let marketplace = installed_from
                .and_then(|from| from.get("marketplace"))
                .and_then(Value::as_str);
            let version = manifest
                .get("version")
                .and_then(Value::as_str)
                .or_else(|| {
                    installed_from
                        .and_then(|from| from.get("version"))
                        .and_then(Value::as_str)
                })
                .unwrap_or_default();
            entries.push(PackageEntry {
                id: files::key(format!("package:antigravity:{}", root.display())),
                agent: Some(Agent::Antigravity),
                name: name.to_string(),
                version: version.to_string(),
                description: manifest
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                origin: if noending_managed {
                    Origin::NoEndingManaged
                } else if marketplace.is_some() {
                    Origin::Market
                } else {
                    Origin::Local
                },
                marketplace: marketplace.map(str::to_string),
                install_path: Some(root.to_string_lossy().into_owned()),
                scopes: vec![ScopeState {
                    scope: Scope::User,
                    state: match enabled {
                        Some(true) => ToggleState::Enabled,
                        Some(false) => ToggleState::Disabled,
                        // NoEnding ships its built-in enabled and never
                        // writes the switch; an absent key is the default-on
                        // state, not an unknown one. Only an explicit false
                        // (written outside NoEnding) reads as disabled.
                        None if noending_managed => ToggleState::Enabled,
                        None => ToggleState::Unset,
                    },
                    config_path: gemini
                        .join("config/config.json")
                        .to_string_lossy()
                        .into_owned(),
                    config_exists: gemini.join("config/config.json").is_file(),
                }],
                capabilities: if noending_managed {
                    Capabilities::none()
                } else {
                    Capabilities::full()
                },
                revision: files::revision(&[&manifest_raw, &config_raw]),
                error: None,
            });
        }
    }
    entries
}
