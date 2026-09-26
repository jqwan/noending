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
///
/// The empty string means "this Agent documents no override": the IDE-hosted
/// ones (Qoder) hard-code their home, and inventing a variable name here would
/// be a guess that silently points at nothing.
pub fn agent_env_override(agent: Agent) -> &'static str {
    match agent {
        Agent::Codex => "CODEX_HOME",
        Agent::ClaudeCode => "CLAUDE_CONFIG_DIR",
        Agent::Pi => "PI_HOME",
        Agent::Qoder => "",
        // Electron app; no documented override.
        Agent::WorkBuddy => "",
        // Documented by dsh's own settings loader: `$DSH_HOME` ?? `~/.dsh`.
        Agent::Dsh => "DSH_HOME",
        // Desktop app with no documented override; its data root is `~/.zcode`.
        Agent::ZCode => "",
        // Desktop IDE, no documented override; conversations under
        // `~/.gemini/antigravity`.
        Agent::Antigravity => "",
    }
}

/// Default data root relative to the user home for each agent.
pub fn agent_default_dir(agent: Agent) -> PathBuf {
    match agent {
        Agent::Codex => [".codex"].iter().collect(),
        Agent::ClaudeCode => [".claude"].iter().collect(),
        Agent::Pi => [".pi"].iter().collect(),
        Agent::Qoder => [".qoder-cn"].iter().collect(),
        Agent::WorkBuddy => [".workbuddy"].iter().collect(),
        Agent::Dsh => [".dsh"].iter().collect(),
        Agent::ZCode => [".zcode"].iter().collect(),
        Agent::Antigravity => [".gemini", "antigravity"].iter().collect(),
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

/// Expand a leading `~` / `~\` / `~/…` to the user's home directory (UI input is
/// plain text; no shell is involved anywhere else).
///
/// This is the platform-facing wrapper over
/// [`crate::workspace::identity::expand_tilde`], which is the repository's only
/// expander: the previous copy here ignored `~\`, so a Windows
/// user typing `~\.noending` got a literal `~` directory, and `launcher` had a
/// *third* expander. One semantics, two spellings (Unix `PathBuf`, domain
/// `String`), because that is all the type systems allow.
pub fn expand_tilde(path: &str) -> PathBuf {
    PathBuf::from(crate::workspace::identity::expand_tilde(path))
}

/// Ask the desktop shell to show a directory in the platform's file manager.
pub fn open_directory(path: &std::path::Path) -> crate::error::Result<()> {
    #[cfg(target_os = "macos")]
    let opener = directory_opener(path, DirectoryPlatform::MacOs);
    #[cfg(target_os = "windows")]
    let opener = directory_opener(path, DirectoryPlatform::Windows);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let opener = directory_opener(path, DirectoryPlatform::Other);

    let mut child = std::process::Command::new(opener.program)
        .args(&opener.args)
        .spawn()
        .map_err(|_| crate::error::other("无法启动系统文件管理器，请检查系统配置"))?;

    // macOS `open` and `xdg-open` are short-lived launch helpers. Wait for
    // them so Unix reaps the child and reports a failed handoff. Explorer is
    // different: it may keep the launched process alive, so Windows returns
    // after a successful spawn and lets the OS own that process handle.
    if opener.wait_for_exit {
        let status = match child.wait() {
            Ok(status) => status,
            Err(_) => {
                // A rare wait error should still not leave a short-lived Unix
                // child unreaped. Try once more from a background reaper so a
                // transient interruption cannot leave a zombie behind.
                let _ = std::thread::spawn(move || child.wait());
                return Err(crate::error::other("无法确认系统文件管理器是否已打开目录"));
            }
        };
        if !status.success() {
            return Err(crate::error::other(
                "系统文件管理器无法打开目录，请检查目录是否可访问",
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // Non-host platforms are exercised by path/argv unit tests.
enum DirectoryPlatform {
    MacOs,
    Windows,
    Other,
}

struct DirectoryOpener {
    program: &'static str,
    args: Vec<std::ffi::OsString>,
    wait_for_exit: bool,
}

fn directory_opener(path: &std::path::Path, platform: DirectoryPlatform) -> DirectoryOpener {
    let (program, wait_for_exit) = match platform {
        DirectoryPlatform::MacOs => ("open", true),
        DirectoryPlatform::Windows => ("explorer.exe", false),
        DirectoryPlatform::Other => ("xdg-open", true),
    };
    DirectoryOpener {
        program,
        args: vec![path.as_os_str().to_owned()],
        wait_for_exit,
    }
}

/// Identifier used for both the Tauri bundle and the OS-native app folder.
pub const APP_IDENTIFIER: &str = "app.noending.desktop";

/// The OS-native per-app folder that lives OUTSIDE NoEnding Home:
/// `~/Library/Application Support/app.noending.desktop` on macOS,
/// `%APPDATA%\app.noending.desktop` on Windows,
/// `~/.config/app.noending.desktop` on Linux.
///
/// Two things live here and nothing else may:
/// * `home.json`, the bootstrap pointer that tells us where NoEnding Home is
/// — it must be outside the Home, since the database inside
///   the Home is precisely what it locates;
/// macOS/Windows differ because `dirs` maps the two concepts onto different
/// known folders: we want *Application Support* on macOS and *Roaming AppData*
/// on Windows, which is what Tauri's own `app_data_dir()` resolves to today.
/// Keeping that branch here is the point of the layer — `workspace/` must stay
/// filesystem- and OS-free.
pub fn resolve_app_support_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let base = dirs::data_dir();
    #[cfg(target_os = "windows")]
    let base = dirs::config_dir();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let base = dirs::config_dir();
    base.map(|d| d.join(APP_IDENTIFIER))
}

/// The OS-native folder a THIRD-PARTY app keeps its data in, by bundle id:
/// macOS `~/Library/Application Support/<bundle>`, Windows
/// `%APPDATA%\<bundle>`. Same resolution as [`resolve_app_support_dir`] with
/// the identifier parameterised.
///
/// This is for reading another vendor's store as a title sidecar,
/// and it is a *lookup*, not a dependency: a wrong or missing path yields no
/// titles rather than an error. Verified on macOS: `com.qodercn.app.stable`
/// holds Qoder's `main.sqlite`; the Windows spelling follows Electron's own
/// `app.getPath('userData')` rule (inferred).
pub fn resolve_external_app_support(bundle_id: &str) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let base = dirs::data_dir();
    #[cfg(target_os = "windows")]
    let base = dirs::config_dir();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let base = dirs::config_dir();
    base.map(|d| d.join(bundle_id))
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
    fn directory_openers_preserve_paths_as_single_arguments() {
        let unix_path = PathBuf::from("/Users/Ada/No Ending/logs/context extraction");
        let mac = directory_opener(&unix_path, DirectoryPlatform::MacOs);
        assert_eq!(mac.program, "open");
        assert_eq!(mac.args, vec![unix_path.as_os_str().to_owned()]);
        assert!(mac.wait_for_exit);

        let windows_path =
            PathBuf::from(r"C:\Users\Ada Lovelace\No Ending\logs\context extraction");
        let windows = directory_opener(&windows_path, DirectoryPlatform::Windows);
        assert_eq!(windows.program, "explorer.exe");
        assert_eq!(windows.args, vec![windows_path.as_os_str().to_owned()]);
        assert!(!windows.wait_for_exit);
    }

    /// 必须测试-adjacent: the expander merged here must keep every
    /// spelling the two predecessors handled, and the one only `launcher` handled.
    #[test]
    fn tilde_expansion_covers_both_separators() {
        // No shell is involved, so these are pure string operations except for
        // the ambient home — hence only the shapes, never the user's real paths.
        assert_eq!(
            expand_tilde("/already/absolute"),
            PathBuf::from("/already/absolute")
        );
        assert_eq!(expand_tilde("relative/x"), PathBuf::from("relative/x"));
        assert_eq!(expand_tilde(""), PathBuf::from(""));
        assert_eq!(
            expand_tilde("~user/x"),
            PathBuf::from("~user/x"),
            "~user needs a passwd lookup, i.e. ambient state"
        );
        // A `~` that is not leading is data, not an instruction.
        assert_eq!(expand_tilde("/opt/a~b"), PathBuf::from("/opt/a~b"));
        if cfg!(windows) {
            assert_ne!(expand_tilde("~"), PathBuf::from("~"));
            assert!(expand_tilde("~\\.noending")
                .to_string_lossy()
                .ends_with(".noending"));
        }
    }

    #[test]
    fn app_support_dir_is_outside_noending_home() {
        // the bootstrap pointer's folder must be resolvable without
        // knowing the NoEnding Home, so it can never be inside it.
        let Some(dir) = resolve_app_support_dir() else {
            return; // no known folder: the caller falls back to `~/.noending`
        };
        assert_eq!(dir.file_name().unwrap(), APP_IDENTIFIER);
        if let Some(home) = resolve_home() {
            let noending_home = home.join(crate::workspace::home::APP_DIR_NAME);
            assert!(
                !crate::workspace::identity::is_within(
                    &dir.to_string_lossy(),
                    &noending_home.to_string_lossy()
                ),
                "{dir:?} must not live inside {noending_home:?}"
            );
        }
    }
}
