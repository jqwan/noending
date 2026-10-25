//! Pi tracks packages as `npm:`/`git:`/local-path sources in `settings.json`.
//! There is no native per-package toggle: presence is the only state, so the
//! entry reports Enabled and no toggle capability.

use super::{files, Capabilities, Origin, PackageEntry, Roots, Scope, ScopeState, ToggleState};
use crate::domain::Agent;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(super) fn discover(roots: &Roots, warnings: &mut Vec<String>) -> Vec<PackageEntry> {
    let settings = roots.pi.join("settings.json");
    let Some(value) = files::json(&settings, warnings) else {
        return Vec::new();
    };
    let Some(raw) = value.get("packages").and_then(Value::as_array) else {
        return Vec::new();
    };
    let raw = serde_json::to_string(raw).unwrap_or_default();
    raw_array_sources(&value)
        .map(|source| {
            let origin = if source.starts_with("npm:") || source.starts_with("git:") {
                Origin::Market
            } else {
                Origin::Local
            };
            PackageEntry {
                id: files::key(format!("package:pi:{source}")),
                agent: Some(Agent::Pi),
                name: source.clone(),
                version: String::new(),
                description: String::new(),
                origin,
                marketplace: None,
                install_path: local_path(&settings, &source),
                scopes: vec![ScopeState {
                    scope: Scope::User,
                    state: ToggleState::Enabled,
                    config_path: settings.to_string_lossy().into_owned(),
                    config_exists: settings.is_file(),
                }],
                capabilities: Capabilities {
                    uninstall: true,
                    toggle: false,
                },
                revision: files::revision(&[&raw]),
                error: None,
            }
        })
        .collect()
}

fn raw_array_sources(value: &Value) -> impl Iterator<Item = String> + '_ {
    value
        .get("packages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|package| {
            package.as_str().map(str::to_string).or_else(|| {
                package
                    .get("source")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
        })
}

/// Relative package sources resolve against the settings file's directory;
/// remote sources stay unresolved because resolving them is Pi's job.
fn local_path(settings: &Path, source: &str) -> Option<String> {
    let path = Path::new(source.trim_start_matches("./"));
    let candidate: PathBuf = if path.is_absolute() {
        path.to_path_buf()
    } else if source.starts_with("./") || source.starts_with("../") {
        settings.parent()?.join(path)
    } else {
        return None;
    };
    candidate.to_string_lossy().into_owned().into()
}
