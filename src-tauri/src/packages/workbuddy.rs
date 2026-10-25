//! WorkBuddy has no confirmed install CLI, so its packages are read-only
//! discovery: the registry format matches Claude's `installed_plugins.json`
//! and the built-in marketplace names come from `configured-plugins.json`.

use super::{
    files, workbuddy_dir, Capabilities, Origin, PackageEntry, Roots, Scope, ScopeState, ToggleState,
};
use crate::domain::Agent;
use serde_json::Value;

pub(super) fn discover(roots: &Roots, warnings: &mut Vec<String>) -> Vec<PackageEntry> {
    let base = workbuddy_dir(roots);
    let registry_path = base.join("plugins/installed_plugins.json");
    let Some(registry) = files::json(&registry_path, warnings) else {
        return Vec::new();
    };
    let configured_raw =
        files::read(&base.join("plugins/configured-plugins.json")).unwrap_or_default();
    let configured: Value = serde_json::from_str(&configured_raw).unwrap_or(Value::Null);
    let builtin_names: Vec<String> = configured
        .get("builtInMarketPlugins")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|plugin| plugin.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    let Some(plugins) = registry.get("plugins").and_then(Value::as_object) else {
        return Vec::new();
    };
    plugins
        .iter()
        .filter_map(|(key, installs)| {
            let install = installs.as_array()?.first()?;
            let (name, marketplace) = key.rsplit_once('@').unwrap_or((key, ""));
            let bundled = marketplace == "workbuddy-builtin"
                || builtin_names.iter().any(|builtin| builtin == name);
            Some(PackageEntry {
                id: files::key(format!("package:workbuddy:{key}")),
                agent: Some(Agent::WorkBuddy),
                name: name.to_string(),
                version: install
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                description: String::new(),
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
                capabilities: Capabilities::none(),
                revision: files::revision(&[&configured_raw]),
                error: None,
            })
        })
        .collect()
}
