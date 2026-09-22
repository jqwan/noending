//! Executable resolution for agent CLIs.
//!
//! A GUI app launched from Finder / Explorer does not inherit the user's
//! interactive shell PATH, so `Command::new("claude")` is not reliable.
//! We resolve concrete executable paths once, cache them in
//! `agent_installations`, and re-verify on every app start.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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
                    let mut versions: Vec<_> =
                        rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
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
        let mut out = vec![
            format!("{}.exe", name),
            format!("{}.cmd", name),
            format!("{}.bat", name),
        ];
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

/// Probe the CLI version with a HARD timeout. Best effort — an agent
/// without `--version` support still resolves, just without a version
/// string, and a hanging probe can never stall app startup.
fn probe_version(exec: &PathBuf) -> Option<String> {
    const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
    let mut child = std::process::Command::new(exec)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break None,
        }
    };
    let status = status?;
    if !status.success() {
        return None;
    }
    // The child exited within the deadline; the pipe is complete, read it
    // (take() avoids blocking forever on a pathological descriptor).
    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        use std::io::Read;
        let _ = pipe.read_to_string(&mut stdout);
    }
    let text = stdout
        .trim()
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Locate an executable by bare name, outside the inherited PATH.
///
/// Extracted out of [`resolve`] because the candidate-dir logic exists for a
/// reason that has nothing to do with Agents: a GUI app started by Finder /
/// Explorer does not inherit the shell PATH, so `Command::new("git")` fails
/// even when git is installed (方案 §42.3-M10). The WorkspaceResolver needs
/// the same guarantee for `git`, so the locator is generic and `resolve`
/// becomes one of its callers.
///
/// A `name` that already contains a separator, or is absolute, is used as
/// given — a caller that knows where its binary lives must not be second-guessed.
/// Returns `None` when nothing executable is found; never an error, because
/// every caller treats "no binary" as an observation rather than a failure.
pub fn resolve_executable(name: &str) -> Option<PathBuf> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let explicit = Path::new(name);
    if explicit.is_absolute() || name.contains('/') || name.contains('\\') {
        return is_executable_file(explicit).then(|| explicit.to_path_buf());
    }
    for dir in candidate_dirs() {
        for file in candidate_file_names(name) {
            let candidate = dir.join(&file);
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// The directory a resolved executable came from, for `AgentInstallation::source`.
fn resolution_source(candidate: &Path) -> String {
    let dir = candidate.parent().unwrap_or(candidate);
    format!("resolved:{}", dir.display())
}

fn is_executable_file(p: &Path) -> bool {
    p.is_file() && (cfg!(target_os = "windows") || is_executable_unix(p))
}

/// Executable names to probe for an Agent, in order. **Empty means the Agent
/// has no headless CLI at all** (Qoder is IDE-hosted): its adapter still
/// reads history, but it can never be launched or resumed (方案 §37.3).
pub fn cli_names(agent: Agent) -> Vec<&'static str> {
    match agent {
        Agent::Codex => vec!["codex"],
        Agent::ClaudeCode => vec!["claude"],
        Agent::Pi => vec!["pi"],
        Agent::Qoder => vec![],
    }
}

pub fn resolve(agent: Agent) -> Result<AgentInstallation> {
    let names: Vec<&str> = cli_names(agent);
    if names.is_empty() {
        // Reading an Agent's transcripts never depends on its CLI; only
        // launching does. Saying so beats a PATH error the user cannot fix.
        return Err(other(format!(
            "{} 没有可启动的 CLI，NoEnding 只读取它的历史会话",
            agent.display_name()
        )));
    }
    let now = chrono::Utc::now().to_rfc3339();
    for name in names {
        if let Some(candidate) = resolve_executable(name) {
            return Ok(AgentInstallation {
                agent,
                executable_path: candidate.to_string_lossy().to_string(),
                version: probe_version(&candidate),
                source: resolution_source(&candidate),
                last_verified_at: now,
            });
        }
    }
    Err(other(format!(
        "未找到 {} CLI，请先安装或将其加入 PATH（尝试过 {} 个候选目录）",
        agent.display_name(),
        candidate_dirs().len()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `resolve_executable` is what lets `git` be found at all from a
    /// Finder-launched app (方案 §42.3-M10), so its own contract is pinned here
    /// without depending on what happens to be installed: an empty name and a
    /// missing explicit path resolve to nothing, and an explicit path that *is*
    /// executable is used verbatim rather than re-searched.
    #[test]
    fn explicit_paths_are_used_as_given() {
        assert!(resolve_executable("").is_none());
        assert!(resolve_executable("   ").is_none());
        let absent = std::env::temp_dir().join("noending-not-an-executable-9f3c1a");
        assert!(resolve_executable(&absent.to_string_lossy()).is_none());
        // A bare name that cannot exist anywhere: no candidate, no panic.
        assert!(resolve_executable("noending-definitely-not-installed-9f3c1a").is_none());

        let dir = std::env::temp_dir().join(format!("noending-exec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("noending-fake-git");
        std::fs::write(&fake, b"#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(
                resolve_executable(&fake.to_string_lossy()).as_deref(),
                Some(fake.as_path())
            );
            // Not executable ⇒ not a candidate.
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(resolve_executable(&fake.to_string_lossy()).is_none());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn candidate_names_cover_the_platform_suffixes() {
        let names = candidate_file_names("git");
        if cfg!(target_os = "windows") {
            // `.exe` must be spelled out: we hand the resolved path to
            // `Command::new` ourselves, and a bare `git` would not resolve.
            assert!(names.contains(&"git.exe".to_string()), "{names:?}");
            assert!(names.contains(&"git.cmd".to_string()), "{names:?}");
        } else {
            assert_eq!(names, vec!["git".to_string()]);
        }
    }
}
