#[cfg(not(windows))]
use super::shell_quote;
use super::{executable, CliError, CliResult};
use serde_json::{json, Value};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

#[cfg(not(windows))]
const MARKER: &str = "#!/bin/sh\n# NoEnding CLI launcher v1\n";
#[cfg(windows)]
const MARKER: &str = "@echo off\r\nrem NoEnding CLI launcher v1\r\n";

fn launchers(
    directory: &Path,
    connection: Option<&Path>,
    exe: Option<&Path>,
) -> CliResult<Vec<(PathBuf, String)>> {
    let exe = exe
        .map(Path::to_path_buf)
        .map(Ok)
        .unwrap_or_else(executable)?
        .canonicalize()?;
    Ok(["ne", "noending"]
        .iter()
        .map(|name| {
            #[cfg(not(windows))]
            {
                (
                    directory.join(name),
                    format!(
                        "{MARKER}exec {} --cli{} \"$@\"\n",
                        shell_quote(&exe),
                        connection
                            .map(|p| format!(" --connection {}", shell_quote(p)))
                            .unwrap_or_default()
                    ),
                )
            }
            #[cfg(windows)]
            {
                (
                    directory.join(format!("{name}.cmd")),
                    format!(
                        "{MARKER}\"{}\" --cli{} %*\r\n",
                        exe.to_string_lossy().replace('%', "%%"),
                        connection
                            .map(|p| format!(
                                " --connection \"{}\"",
                                p.to_string_lossy().replace('%', "%%")
                            ))
                            .unwrap_or_default()
                    ),
                )
            }
        })
        .collect())
}

pub(super) fn run(directory: &Path, action: &str) -> CliResult<Value> {
    run_connected(directory, action, None, None)
}

pub(super) fn run_connected(
    directory: &Path,
    action: &str,
    connection: Option<&Path>,
    exe: Option<&Path>,
) -> CliResult<Value> {
    let launchers = launchers(directory, connection, exe)?;
    let mut existing = Vec::new();
    let mut statuses = Vec::new();
    for (path, script) in &launchers {
        #[cfg(unix)]
        let executable = std::fs::symlink_metadata(path).ok().is_some_and(|m| {
            use std::os::unix::fs::PermissionsExt;
            m.file_type().is_file() && m.permissions().mode() & 0o111 != 0
        });
        #[cfg(not(unix))]
        let executable = std::fs::symlink_metadata(path)
            .ok()
            .is_some_and(|m| m.file_type().is_file());
        let old = match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_file() => Some(std::fs::read(path)?),
            Ok(_) => Some(Vec::new()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let managed = old
            .as_ref()
            .is_some_and(|s| s.starts_with(MARKER.as_bytes()));
        statuses.push(json!({"path":path,"exists":old.is_some(),"managed":managed,"executable":executable,"matches_current":old.as_ref().is_some_and(|s|s==script.as_bytes())}));
        if action != "status" {
            if action == "install" && old.is_some() {
                return Err(CliError::new(
                    "already_exists",
                    format!("{} 已存在", path.display()),
                ));
            }
            if action != "install" && old.is_some() && !managed {
                return Err(CliError::new(
                    "unmanaged_launcher",
                    format!("{} 不是受管理的 CLI 入口", path.display()),
                ));
            }
        }
        existing.push(old);
    }
    if action == "status" {
        return Ok(json!({"path_directory":directory,"commands":statuses}));
    }
    if action != "uninstall" {
        std::fs::create_dir_all(directory)?;
    }
    for ((path, script), old) in launchers.iter().zip(&existing) {
        // Check again before a mutation; never follow or replace a new symlink.
        if let Some(old) = old {
            if !std::fs::symlink_metadata(path)?.file_type().is_file()
                || std::fs::read(path)? != *old
            {
                return Err(CliError::new(
                    "installation_changed",
                    "CLI 入口已变化，请重新检查",
                ));
            }
        } else if std::fs::symlink_metadata(path).is_ok() {
            return Err(CliError::new(
                "installation_changed",
                "CLI 入口已变化，请重新检查",
            ));
        }
        if action == "uninstall" {
            if old.is_some() {
                std::fs::remove_file(path)?;
            }
            continue;
        }
        #[cfg(unix)]
        let current_executable = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(path)
                .ok()
                .is_some_and(|m| m.permissions().mode() & 0o111 != 0)
        };
        #[cfg(not(unix))]
        let current_executable = true;
        if old.as_deref() == Some(script.as_bytes()) && current_executable {
            continue;
        }
        let temp = path.with_extension(format!("{}.tmp", crate::storage::new_id()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o755);
        }
        let result = (|| -> std::io::Result<()> {
            options.open(&temp)?.write_all(script.as_bytes())?;
            #[cfg(windows)]
            if old.is_some() {
                std::fs::remove_file(path)?;
            }
            if let Err(error) = std::fs::rename(&temp, path) {
                #[cfg(windows)]
                if let Some(old) = old {
                    let _ = std::fs::write(path, old);
                }
                return Err(error);
            }
            Ok(())
        })();
        let _ = std::fs::remove_file(temp);
        result?;
    }
    Ok(
        json!({"commands":launchers.iter().map(|(p,_)|p).collect::<Vec<_>>(),"path_directory":directory,"action":action,"message":if action=="uninstall"{"CLI 入口已移除"}else{"将此目录加入 PATH 后可使用 ne 和 noending"}}),
    )
}
