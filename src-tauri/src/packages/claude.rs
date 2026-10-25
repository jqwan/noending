//! Claude Code keeps an install registry at `plugins/installed_plugins.json`;
//! cached versions alone are not evidence of an install. NoEnding manages the
//! user-level scope only — project/local installs belong to the Agent.

use super::{files, Capabilities, Origin, PackageEntry, Roots, Scope, ScopeState, ToggleState};
use crate::domain::Agent;
use serde_json::Value;
use std::path::PathBuf;

pub(super) fn discover(roots: &Roots, warnings: &mut Vec<String>) -> Vec<PackageEntry> {
    let registry_path = roots.claude.join("plugins/installed_plugins.json");
    let raw = match files::read(&registry_path) {
        Ok(raw) => raw,
        Err(e) => {
            warnings.push(format!("{}：{e}", registry_path.display()));
            return Vec::new();
        }
    };
    if raw.is_empty() {
        return Vec::new();
    }
    let Ok(registry) = serde_json::from_str::<Value>(&raw) else {
        warnings.push(format!("{}：注册表无法解析", registry_path.display()));
        return Vec::new();
    };
    let Some(plugins) = registry.get("plugins").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut settings_cache: Vec<(PathBuf, Option<Value>)> = Vec::new();
    let mut entries = Vec::new();
    for (key, installs) in plugins {
        let (name, marketplace) = key.rsplit_once('@').unwrap_or((key, ""));
        for install in installs.as_array().into_iter().flatten() {
            // Project and local installs stay Agent-managed; NoEnding lists
            // the user scope only.
            if install
                .get("scope")
                .and_then(Value::as_str)
                .is_some_and(|scope| scope != "user")
            {
                continue;
            }
            let config_path = roots.claude.join("settings.json");
            let enabled = settings_cache
                .iter()
                .find(|(path, _)| *path == config_path)
                .map(|(_, value)| value.clone())
                .unwrap_or_else(|| {
                    let value = files::json(&config_path, warnings);
                    settings_cache.push((config_path.clone(), value.clone()));
                    value
                })
                .and_then(|settings| {
                    settings
                        .get("enabledPlugins")
                        .and_then(|plugins| plugins.get(key))
                        .and_then(Value::as_bool)
                });
            let capabilities = Capabilities::full();
            entries.push(PackageEntry {
                id: files::key(format!("package:claude_code:{key}")),
                agent: Some(Agent::ClaudeCode),
                name: name.to_string(),
                version: install
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                description: String::new(),
                origin: Origin::Market,
                marketplace: (!marketplace.is_empty()).then(|| marketplace.to_string()),
                install_path: install
                    .get("installPath")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                scopes: vec![ScopeState {
                    scope: Scope::User,
                    state: match enabled {
                        Some(true) => ToggleState::Enabled,
                        Some(false) => ToggleState::Disabled,
                        None => ToggleState::Unset,
                    },
                    config_path: config_path.to_string_lossy().into_owned(),
                    config_exists: config_path.is_file(),
                }],
                capabilities,
                revision: files::revision(&[&raw]),
                error: None,
            });
        }
    }
    entries
}
