//! Workspace settings commands — NoEnding Home, default workspace.
//!
//! This is the wiring surface between Home resolution and the UI. Two rules
//! make it safe to expose at all:
//!
//! * `get_workspace_settings` reports the Home the app is *actually* running on,
//!   read from managed state — never a re-resolution, because a re-resolution
//!   after a failed migration would report the new location while the database
//!   being written is still the old one.
//! * `set_noending_home` writes `pending_home` and nothing else. Moving data is a
//!   start-up operation (`home::prepare_home`), because switching a live process
//!   onto a copied database would leave the old process writing to a diverged
//!   file.

use tauri::{AppHandle, Manager};

use crate::error::{other, Result};
use crate::workspace::home::{self, BootstrapPointer, HomeSource, NoEndingHome, WorkspaceSettings};

/// `get_workspace_settings`.
///
/// Also the one place the UI learns which database file it is looking at, so
/// `get_app_info` retired into it (v0.2 had two commands reporting the same
/// Home, and only one of them was reading the live handle).
#[tauri::command]
pub fn get_workspace_settings(
    app: AppHandle,
    state: tauri::State<super::AppState>,
) -> Result<WorkspaceSettings> {
    let home = app
        .try_state::<NoEndingHome>()
        .ok_or_else(|| other("NoEnding Home 尚未初始化"))?
        .inner()
        .clone();
    let mut settings = read_settings(&home);
    // a failed relocation still starts the app, on the OLD Home, so
    // "where the Home thinks the database is" can differ from "which file writes
    // are landing in". The user gets the latter.
    if let Ok(Some(path)) = super::with_db(&state, |db| Ok(db.read().path().map(str::to_string))) {
        settings.db_path = path;
    }
    Ok(settings)
}

/// `set_noending_home`: request a relocation for the next launch.
///
/// Returns `restart_required = true` so the UI can say what it means instead of
/// implying the move already happened.
#[tauri::command]
pub fn set_noending_home(app: AppHandle, new_home: String) -> Result<WorkspaceSettings> {
    let trimmed = new_home.trim();
    if trimmed.is_empty() {
        return Err(other("NoEnding Home 路径不能为空"));
    }
    let pointer_path = pointer_path()?;
    let user_home = crate::workspace::home_dir().map(|h| h.to_string_lossy().to_string());
    home::request_relocation(&pointer_path, trimmed, user_home.as_deref())?;

    let home = app
        .try_state::<NoEndingHome>()
        .ok_or_else(|| other("NoEnding Home 尚未初始化"))?
        .inner()
        .clone();
    Ok(read_settings(&home))
}

/// The settings view of a running Home: the live location plus whatever the
/// bootstrap pointer is waiting for.
///
/// The managed `NoEndingHome` already *is* the effective Home — `prepare_home`
/// applied `$NOENDING_HOME` at startup and never persists it — so this reads
/// state instead of re-resolving it.
fn read_settings(home: &NoEndingHome) -> WorkspaceSettings {
    let (pointer, source) = match home::default_pointer_path().as_deref() {
        Some(path) => {
            let (pointer, _) = BootstrapPointer::load(path);
            let source = if home::resolve_explicit_override().is_some() {
                HomeSource::ExplicitEnv
            } else if pointer.current_home.is_some() {
                HomeSource::Bootstrap
            } else {
                HomeSource::DefaultHome
            };
            (pointer, source)
        }
        None => (BootstrapPointer::default(), HomeSource::DefaultHome),
    };
    WorkspaceSettings {
        noending_home: home.root_str(),
        default_workspace: home.default_workspace_str(),
        pending_home: pointer.pending_home.clone(),
        restart_required: pointer.pending_home.is_some(),
        db_path: home.db_path_str(),
        home_source: source.as_str().to_string(),
    }
}

fn pointer_path() -> Result<std::path::PathBuf> {
    home::default_pointer_path().ok_or_else(|| other("无法确定 bootstrap 指针的位置"))
}
