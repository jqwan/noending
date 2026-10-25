//! Tauri boundary for plugin package management. Discovery is an offline read
//! of native registries; no command here launches an Agent CLI except the
//! Agent-owned uninstall and the local-directory install.
use crate::{domain, error::Result, packages, skills};

fn inventory() -> Result<packages::PackageInventory> {
    let roots = skills::Roots::current()?;
    Ok(packages::discover(&roots))
}

#[tauri::command]
pub async fn list_packages() -> Result<packages::PackageInventory> {
    super::run_on_blocking_worker(inventory).await?
}

#[tauri::command]
pub async fn get_package_detail(id: String) -> Result<Option<packages::PackageDetail>> {
    super::run_on_blocking_worker(move || packages::detail(&inventory()?, &id)).await?
}

#[tauri::command]
pub async fn preview_import_package(path: String) -> Result<packages::ImportPreview> {
    super::run_on_blocking_worker(move || packages::preview_import(&path)).await?
}

/// Installs a local package directory into the selected Agents. Nothing is
/// registered — on any failure the flow simply stops.
#[tauri::command]
pub async fn install_local_package(
    path: String,
    agents: Vec<domain::Agent>,
) -> Result<packages::PackageOperationResult> {
    super::run_on_blocking_worker(move || packages::install_local(&path, &agents)).await?
}

/// Installs selected plugins from a local marketplace directory; the
/// directory is registered as a marketplace in each selected Agent.
#[tauri::command]
pub async fn install_local_marketplace(
    path: String,
    selections: Vec<packages::MarketplaceSelection>,
) -> Result<packages::PackageOperationResult> {
    super::run_on_blocking_worker(move || {
        let selections: Vec<(domain::Agent, Vec<String>)> = selections
            .into_iter()
            .map(|selection| (selection.agent, selection.plugins))
            .collect();
        packages::install_local_marketplace(&path, &selections)
    })
    .await?
}

#[tauri::command]
pub async fn uninstall_package(id: String) -> Result<packages::PackageOperationResult> {
    super::run_on_blocking_worker(move || packages::uninstall(&inventory()?, &id)).await?
}

#[tauri::command]
pub async fn enable_package(
    id: String,
    revision: String,
) -> Result<packages::PackageOperationResult> {
    toggle_package(id, revision, true).await
}

#[tauri::command]
pub async fn disable_package(
    id: String,
    revision: String,
) -> Result<packages::PackageOperationResult> {
    toggle_package(id, revision, false).await
}

async fn toggle_package(
    id: String,
    revision: String,
    enable: bool,
) -> Result<packages::PackageOperationResult> {
    super::run_on_blocking_worker(move || {
        packages::set_enabled(&inventory()?, &id, &revision, enable)
    })
    .await?
}
