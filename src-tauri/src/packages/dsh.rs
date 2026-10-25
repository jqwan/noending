//! DSH is desktop-hosted with a Cordis-based package system: the desktop
//! profile (`~/.dsh/profiles/desktop/package.json`) declares its loaded
//! bundles in `dsh.profile.bundles`, and the bundle packages live under the
//! profile's `node_modules`. NoEnding manages the desktop profile only —
//! other profiles (web and friends) belong to their own surfaces — and
//! treats a bundle as one whole package: install/uninstall go through the
//! bundled `dsh plugin` command plus a declaration edit. Enablement lives in
//! the Cordis patch layer and stays untouched (state reports Unknown).

use super::{
    dsh_dir, files, Capabilities, Origin, PackageEntry, Roots, Scope, ScopeState, ToggleState,
};
use crate::domain::Agent;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The one profile NoEnding manages; DSH has no user-global scope.
pub const DESKTOP_PROFILE: &str = "desktop";

pub fn desktop_profile_dir(roots: &Roots) -> PathBuf {
    dsh_dir(roots).join("profiles").join(DESKTOP_PROFILE)
}

pub(super) fn discover(roots: &Roots, warnings: &mut Vec<String>) -> Vec<PackageEntry> {
    let profiles = dsh_dir(roots).join("profiles");
    let profile_root = desktop_profile_dir(roots);
    let manifest = profile_root.join("package.json");
    if !manifest.is_file() {
        return Vec::new();
    }
    let Ok(raw) = files::read(&manifest) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        warnings.push(format!("{}：无法解析", manifest.display()));
        return Vec::new();
    };
    let Some(bundles) = value
        .get("dsh")
        .and_then(|dsh| dsh.get("profile"))
        .and_then(|profile| profile.get("bundles"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    bundles
        .iter()
        .filter_map(|bundle| bundle.as_str())
        .map(|name| {
            let package_dir = bundle_package_dir(&profiles, &profile_root, name);
            let (version, description) = package_meta(package_dir.as_deref());
            PackageEntry {
                id: files::key(format!("package:dsh:{name}")),
                agent: Some(Agent::Dsh),
                name: name.to_string(),
                version,
                description,
                origin: Origin::Profile,
                marketplace: Some(DESKTOP_PROFILE.to_string()),
                install_path: package_dir.map(|dir| dir.to_string_lossy().into_owned()),
                scopes: vec![ScopeState {
                    scope: Scope::User,
                    state: ToggleState::Unknown,
                    config_path: manifest.to_string_lossy().into_owned(),
                    config_exists: manifest.is_file(),
                }],
                // Whole-bundle uninstall through the bundled CLI; enablement
                // stays with the Cordis patch layer.
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

/// Bundle packages resolve against the profile's own node_modules first, then
/// the shared `profiles/node_modules`.
fn bundle_package_dir(profiles: &Path, profile_root: &Path, name: &str) -> Option<PathBuf> {
    let candidates = [
        profile_root.join("node_modules").join(name),
        profiles.join("node_modules").join(name),
    ];
    candidates.into_iter().find(|dir| dir.is_dir())
}

fn package_meta(dir: Option<&Path>) -> (String, String) {
    let Some(dir) = dir else {
        return (String::new(), String::new());
    };
    let manifest = dir.join("package.json");
    let Ok(raw) = std::fs::read_to_string(manifest) else {
        return (String::new(), String::new());
    };
    match serde_json::from_str::<Value>(&raw) {
        Ok(value) => (
            value
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            value
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
        Err(_) => (String::new(), String::new()),
    }
}

/// Appends a bundle declaration to the desktop profile's `dsh.profile.bundles`.
/// Called after a successful dependency install, so a failure here leaves an
/// installed-but-undeclared package (benign, retry-safe) rather than a
/// declared-but-missing one (the loader would fail to boot the profile).
pub fn declare_bundle(roots: &Roots, name: &str) -> Result<()> {
    let manifest = desktop_profile_dir(roots).join("package.json");
    let mut value = read_profile_object(&manifest)?;
    let bundles = value
        .get_mut("dsh")
        .and_then(|dsh| dsh.get_mut("profile"))
        .and_then(|profile| profile.get_mut("bundles"))
        .and_then(|bundles| bundles.as_array_mut())
        .ok_or_else(|| other("desktop profile 缺少 dsh.profile.bundles，无法声明插件包"))?;
    if bundles.iter().any(|bundle| bundle.as_str() == Some(name)) {
        return Ok(());
    }
    bundles.push(Value::String(name.to_string()));
    write_profile(&manifest, &value)
}

/// Removes the declaration BEFORE the package is uninstalled: if the removal
/// then fails, the profile still boots (an undeclared installed package is
/// benign; a declared missing one is not).
pub fn undeclare_bundle(roots: &Roots, name: &str) -> Result<()> {
    let manifest = desktop_profile_dir(roots).join("package.json");
    let mut value = read_profile_object(&manifest)?;
    let Some(bundles) = value
        .get_mut("dsh")
        .and_then(|dsh| dsh.get_mut("profile"))
        .and_then(|profile| profile.get_mut("bundles"))
        .and_then(|bundles| bundles.as_array_mut())
    else {
        return Ok(());
    };
    bundles.retain(|bundle| bundle.as_str() != Some(name));
    write_profile(&manifest, &value)
}

fn read_profile_object(manifest: &Path) -> Result<Value> {
    let raw = files::read(manifest)?;
    serde_json::from_str::<Value>(&raw)
        .map_err(|e| other(format!("desktop profile package.json 无法解析：{e}")))
}

fn write_profile(manifest: &Path, value: &Value) -> Result<()> {
    let text = format!("{}\n", serde_json::to_string_pretty(value)?);
    files::write_atomic(manifest, &text)
}

fn other(message: impl Into<String>) -> crate::error::AppError {
    crate::error::other(message.into())
}

type Result<T> = std::result::Result<T, crate::error::AppError>;
