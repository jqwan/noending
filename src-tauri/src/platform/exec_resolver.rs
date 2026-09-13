//! Executable resolution for agent CLIs.
//!
//! A GUI app launched from Finder / Explorer does not inherit the user's
//! interactive shell PATH, so `Command::new("claude")` is not reliable.
//! We resolve concrete executable paths once, cache them in
//! `agent_installations`, and re-verify on every app start.

use std::path::PathBuf;
use std::time::Instant;

use serde::Serialize;

use crate::domain::Agent;
use crate::error::{other, Result};

#[derive(Debug, Clone, Serialize)]
pub struct AgentInstallation {
    pub agent: Agent,
    pub executable_path: String,
    pub version: Option<String>,
    pub source: String,
    pub last_verified_at: String,
}

/// Extra directories to probe beyond the inherited PATH, because GUI apps
/// on macOS commonly miss homebrew / nvm / cargo locations.
fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    if let Some(home) = crate::platform::paths::resolve_home() {
        if cfg!(target_os = "macos") {
            dirs.push(home.join(".local/bin"));
            dirs.push(home.join(".cargo/bin"));
            // nvm
            if let Ok(nvm_dir) = std::env::var("NVM_DIR") {
                let nvm = PathBuf::from(nvm_dir);
                if let Ok(rd) = std::fs::read_dir(&nvm.join("versions/node")) {
                    let mut versions: Vec<_> = rd
                        .filter_map(|e| e.ok())
                        .map(|e| e.path())
                        .collect();
                    versions.sort();
                    if let Some(latest) = versions.pop() {
                        dirs.push(latest.join("bin"));
                    }
                }
            }
            // fnm / volta style default node
            dirs.push(home.join(".volta/bin"));
        } else if cfg!(target_os = "windows") {
            dirs.push(home.join("AppData/Roaming/npm"));
            dirs.push(home.join(".cargo/bin"));
        } else {
            dirs.push(home.join(".local/bin"));
            dirs.push(home.join(".cargo/bin"));
        }
    }
    if cfg!(target_os = "macos") {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
    }
    dirs
}

fn candidate_file_names(name: &str) -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        let exts = std::env::var("PATHEXT")
            .map(|v| {
                v.split(';')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|_| vec![".EXE".into(), ".CMD".into(), ".BAT".into()]);
        let mut out = vec![format!("{}.exe", name), format!("{}.cmd", name), format!("{}.bat", name)];
        for e in exts {
            out.push(format!("{}{}", name, e.to_lowercase()));
        }
        out
    }
    #[cfg(not(target_os = "windows"))]
    {
        vec![name.to_string()]
    }
}

/// Probe the CLI version. Best effort — an agent without `--version`
/// support still resolves, just without a version string.
fn probe_version(exec: &PathBuf) -> Option<String> {
    let out = std::process::Command::new(exec)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let text = text.trim().lines().next().unwrap_or("").trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

pub fn resolve(agent: Agent) -> Result<AgentInstallation> {
    let names: Vec<&str> = match agent {
        Agent::Codex => vec!["codex"],
        Agent::ClaudeCode => vec!["claude"],
        Agent::Pi => vec!["pi"],
    };
    let now = chrono::Utc::now().to_rfc3339();
    let mut tried = Vec::new();
    for name in names {
        for dir in candidate_dirs() {
            for file in candidate_file_names(name) {
                let candidate = dir.join(&file);
                let is_exec = candidate.is_file()
                    && (cfg!(target_os = "windows") || is_executable_unix(&candidate));
                if is_exec {
                    return Ok(AgentInstallation {
                        agent,
                        executable_path: candidate.to_string_lossy().to_string(),
                        version: probe_version(&candidate),
                        source: format!("resolved:{}", dir.display()),
                        last_verified_at: now,
                    });
                }
                tried.push(candidate.display().to_string());
            }
        }
    }
    Err(other(format!(
        "未找到 {} CLI，请先安装或将其加入 PATH（尝试过 {} 个候选路径）",
        agent.display_name(),
        tried.len()
    )))
}

#[cfg(unix)]
fn is_executable_unix(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_unix(_p: &std::path::Path) -> bool {
    true
}

/// Resolve with a time budget per agent so a hanging `--version` probe
/// cannot stall startup (versions are optional metadata).
pub fn resolve_quiet(agent: Agent) -> Option<AgentInstallation> {
    let t = Instant::now();
    let r = resolve(agent).ok();
    if r.is_some() {
        tracing_ok(agent, t.elapsed());
    }
    r
}

fn tracing_ok(agent: Agent, elapsed: std::time::Duration) {
    eprintln!(
        "[exec-resolver] {} resolved in {}ms",
        agent.display_name(),
        elapsed.as_millis()
    );
}
