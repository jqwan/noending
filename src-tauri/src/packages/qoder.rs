//! Qoder is IDE/GUI hosted with no headless CLI, so its packages are
//! read-only discovery. The registry format matches Claude's
//! `installed_plugins.json` v2 (with an extra displayName/userVisible pair);
//! the `qoderapp-bundler` market ships with the app itself.

use super::{
    files, qoder_dir, Capabilities, Origin, PackageEntry, Roots, Scope, ScopeState, ToggleState,
};
use crate::domain::Agent;
use serde_json::Value;

pub(super) fn discover(roots: &Roots, warnings: &mut Vec<String>) -> Vec<PackageEntry> {
    let base = qoder_dir(roots);
    let registry_path = base.join("plugins/installed_plugins_v2.json");
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
    let mut entries = Vec::new();
    for (key, installs) in plugins {
        let (name, marketplace) = key.rsplit_once('@').unwrap_or((key, ""));
        for install in installs.as_array().into_iter().flatten() {
            // Project/local installs stay Agent-managed; user scope only.
            if install
                .get("scope")
                .and_then(Value::as_str)
                .is_some_and(|scope| scope != "user")
            {
                continue;
            }
            let bundled = marketplace == "qoderapp-bundler";
            entries.push(PackageEntry {
                id: files::key(format!("package:qoder:{key}")),
                agent: Some(Agent::Qoder),
                name: name.to_string(),
                version: install
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                // No description field in this registry; the displayName is the
                // closest user-facing label.
                description: install
                    .get("displayName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                origin: if bundled {
                    Origin::Bundled
                } else {
                    Origin::Market
                },
                marketplace: (!marketplace.is_empty()).then(|| marketplace.to_string()),
                install_path: install
                    .get("installPath")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                scopes: vec![ScopeState {
                    scope: Scope::User,
                    state: ToggleState::Unknown,
                    config_path: registry_path.to_string_lossy().into_owned(),
                    config_exists: registry_path.is_file(),
                }],
                // No headless CLI: discovery only, the app manages its packages.
                capabilities: Capabilities::none(),
                revision: files::revision(&[&raw]),
                error: None,
            });
        }
    }
    entries
}
