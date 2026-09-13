//! Platform-independent path resolution.
//!
//! Domain and Adapter code must never hard-code `~/.codex`, `%USERPROFILE%\.claude`
//! or any other absolute user path. Everything goes through this layer so that
//! macOS / Windows differences (and per-agent env overrides) live in one place.

use std::path::PathBuf;

use crate::domain::Agent;

/// Resolve the current user's home directory on any supported platform:
/// macOS `$HOME`, Windows `%USERPROFILE%` / Known Folders.
pub fn resolve_home() -> Option<PathBuf> {
    dirs::home_dir()
}

/// Resolve the OS-specific application data directory used by NoEnding itself
/// (macOS `~/Library/Application Support/...`, Windows `%APPDATA%\...`).
/// When running inside Tauri we prefer `app_data_dir()`; this fallback covers
/// unit tests and CLI usage.
pub fn resolve_app_data() -> Option<PathBuf> {
    dirs::data_dir()
}

/// Environment variable that overrides the data root for each agent,
/// mirroring what the agents themselves honour where applicable.
pub fn agent_env_override(agent: Agent) -> &'static str {
    match agent {
        Agent::Codex => "CODEX_HOME",
        Agent::ClaudeCode => "CLAUDE_CONFIG_DIR",
        Agent::Pi => "PI_HOME",
    }
}

/// Default data root relative to the user home for each agent.
pub fn agent_default_dir(agent: Agent) -> PathBuf {
    match agent {
        Agent::Codex => [".codex"].iter().collect(),
        Agent::ClaudeCode => [".claude"].iter().collect(),
        Agent::Pi => [".pi"].iter().collect(),
    }
}

/// Resolve an agent's data root:
///   `$CODEX_HOME`            ?? `~/.codex`
///   `%CODEX_HOME%`           ?? `%USERPROFILE%\.codex`
/// (same pattern for CLAUDE_CONFIG_DIR / PI_HOME).
pub fn resolve_agent_data_dir(agent: Agent) -> Option<PathBuf> {
    if let Ok(v) = std::env::var(agent_env_override(agent)) {
        let p = PathBuf::from(v);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    resolve_home().map(|home| home.join(agent_default_dir(agent)))
}

/// Normalize a directory name like `-Users-jqk-projects-noending`
/// (Claude Code / Pi encode cwd with `-` separators) back into
/// `/Users/jqk/projects/noending`. Returns None when the name does not
/// look like an encoded absolute path on this platform.
pub fn decode_cwd_dir_name(name: &str) -> Option<String> {
    let name = name.trim_matches('-');
    if name.is_empty() {
        return None;
    }
    let decoded = if cfg!(target_os = "windows") {
        // `-C--Users-jqk-...` style: drive letter survives as `C-`
        let mut out = String::new();
        for (i, part) in name.split('-').enumerate() {
            if i == 0 {
                out.push_str(part);
                out.push_str(":\\");
            } else {
                out.push_str(part);
                if i > 0 {
                    out.push('\\');
                }
            }
        }
        out.trim_end_matches('\\').to_string()
    } else {
        format!("/{}", name.replace('-', "/"))
    };
    if decoded.is_empty() {
        None
    } else {
        Some(decoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_override_env() {
        assert_eq!(agent_env_override(Agent::Codex), "CODEX_HOME");
        assert_eq!(agent_env_override(Agent::ClaudeCode), "CLAUDE_CONFIG_DIR");
    }

    #[test]
    fn decode_cwd() {
        assert_eq!(
            decode_cwd_dir_name("-Users-jqk-projects-noending"),
            Some("/Users/jqk/projects/noending".to_string())
        );
    }
}
