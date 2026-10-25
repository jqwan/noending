//! ZCode hosts its CLI inside the desktop bundle (not on PATH), so packages
//! are read-only discovery. There is no install registry file: the runtime
//! data directory `plugins/data/<plugin>@<market>/` is the install evidence,
//! and the cache only locates a single-version unpack. Enablement lives in
//! the client config (`cli/config.json` → `plugins`): a master `enabled`
//! switch, an explicit `enabledPlugins` map keyed by `name@market`, and
//! `suppressedBuiltins` marking "uninstalled" built-ins. An explicit entry
//! wins; without one the plugin ships default-enabled. The schema was read
//! from the client's own bundle (readPluginConfigFromConfig).

use super::{
    files, zcode_dir, Capabilities, Origin, PackageEntry, Roots, Scope, ScopeState, ToggleState,
};
use crate::domain::Agent;
use serde_json::Value;
use std::path::Path;

pub(super) fn discover(roots: &Roots, warnings: &mut Vec<String>) -> Vec<PackageEntry> {
    let base = zcode_dir(roots);
    let data = base.join("plugins/data");
    let config_path = base.join("config.json");
    let config_raw = files::read(&config_path).unwrap_or_default();
    let config: Value = if config_raw.is_empty() {
        serde_json::json!({})
    } else {
        match serde_json::from_str(&config_raw) {
            Ok(value) => value,
            Err(e) => {
                warnings.push(format!("{}：{e}", config_path.display()));
                serde_json::json!({})
            }
        }
    };
    let plugins = config
        .get("plugins")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    // Master switch absent = on; every plugin reports disabled while off.
    let master_enabled = plugins
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let enabled_map = plugins.get("enabledPlugins");
    let suppressed = plugins.get("suppressedBuiltins");

    let state_for = |key: &str| {
        if !master_enabled {
            return ToggleState::Disabled;
        }
        match enabled_map.and_then(|map| map.get(key)) {
            Some(Value::Bool(true)) => ToggleState::Enabled,
            Some(Value::Bool(false)) => ToggleState::Disabled,
            // No explicit entry: the client applies its default-enabled set,
            // which is on for marketplace-installed plugins.
            _ => ToggleState::Enabled,
        }
    };
    let is_suppressed = |key: &str| {
        suppressed
            .and_then(Value::as_array)
            .map(|ids| ids.iter().any(|id| id.as_str() == Some(key)))
            .unwrap_or(false)
    };

    let Ok(dirs) = std::fs::read_dir(&data) else {
        return Vec::new();
    };
    dirs.flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let key = entry.file_name().into_string().ok()?;
            let (name, marketplace) = key.split_once('@')?;
            Some((
                key.clone(),
                name.to_string(),
                marketplace.to_string(),
                entry.path(),
            ))
        })
        // Suppression is the client's "uninstalled" marker for built-ins:
        // it hides them from discovery, and this read mirrors that.
        .filter(|(key, ..)| !is_suppressed(key))
        .map(|(key, name, marketplace, data_dir)| PackageEntry {
            id: files::key(format!("package:zcode:{key}")),
            agent: Some(Agent::ZCode),
            name,
            version: String::new(),
            description: String::new(),
            origin: Origin::Market,
            marketplace: (!marketplace.is_empty()).then_some(marketplace),
            install_path: package_root(&base.join("plugins/cache"), &key),
            scopes: vec![ScopeState {
                scope: Scope::User,
                state: state_for(&key),
                config_path: config_path.to_string_lossy().into_owned(),
                config_exists: config_path.is_file(),
            }],
            // Desktop-hosted: the app manages install, update and removal.
            capabilities: Capabilities::none(),
            revision: files::revision(&[&data_dir.to_string_lossy(), &config_raw]),
            error: None,
        })
        .collect()
}

/// `<market>/<name>/<version>/`; only a single version is reported.
fn package_root(cache_root: &Path, key: &str) -> Option<String> {
    let (name, market) = key.rsplit_once('@')?;
    let dir = cache_root.join(market).join(name);
    let versions: Vec<_> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    (versions.len() == 1).then(|| versions[0].to_string_lossy().into_owned())
}
