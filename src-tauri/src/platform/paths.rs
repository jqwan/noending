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

/// Extra data roots (relative to the user home) that hold conversations of
/// the SAME agent in a second official surface. Antigravity is the case: the
/// `agy` CLI keeps its own store beside the IDE's —
/// `~/.gemini/antigravity-cli`, identical conversation schema, its own
/// sibling `conversation_summaries.db` — so both directories are default
/// ingest sources and one enable covers both surfaces.
fn agent_extra_ingest_dirs(agent: Agent) -> Vec<PathBuf> {
    match agent {
        Agent::Antigravity => vec![[".gemini", "antigravity-cli"].iter().collect()],
        _ => vec![],
    }
}

/// Every root an Agent's conversations are known to live under: the resolved
/// data dir first, then any extra official surface. The default-ingest
/// seeding (`reconcile_runtime_defaults`) runs over this list.
pub fn agent_ingest_roots(agent: Agent) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = resolve_agent_data_dir(agent).into_iter().collect();
    let home = resolve_home();
    roots.extend(
        agent_extra_ingest_dirs(agent)
            .into_iter()
            .filter_map(|rel| home.as_ref().map(|h| h.join(rel))),
    );
    roots
}

/// Is a macOS application bundle with this name present? Probes the two
/// roots users actually install to. Other platforms return true — bundle
/// presence is not probed there, and the deep-link dispatch itself surfaces
/// an OS error when nothing is registered for the scheme.
pub fn app_bundle_present(name: &str) -> bool {
    if !cfg!(target_os = "macos") {
        return true;
    }
    let Some(home) = resolve_home() else {
        return false;
    };
    [PathBuf::from("/Applications"), home.join("Applications")]
        .into_iter()
        .any(|base| base.join(format!("{name}.app")).is_dir())
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

    run_file_manager(opener)
}

/// Reveal a source transcript in the file manager. Linux file managers do not
/// share a portable select-file interface, so open its containing directory.
pub fn reveal_file(path: &std::path::Path) -> crate::error::Result<()> {
    let metadata = std::fs::metadata(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            crate::error::other("源会话文件已不存在")
        } else {
            e.into()
        }
    })?;
    if !metadata.is_file() {
        return Err(crate::error::other("源会话路径不是文件"));
    }
    #[cfg(target_os = "macos")]
    let opener = file_revealer(path, DirectoryPlatform::MacOs);
    #[cfg(target_os = "windows")]
    let opener = file_revealer(path, DirectoryPlatform::Windows);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let opener = file_revealer(path, DirectoryPlatform::Other);
    run_file_manager(opener)
}

fn run_file_manager(opener: DirectoryOpener) -> crate::error::Result<()> {
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
                "系统文件管理器无法打开路径，请检查路径是否可访问",
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

fn file_revealer(path: &std::path::Path, platform: DirectoryPlatform) -> DirectoryOpener {
    let (program, args, wait_for_exit) = match platform {
        DirectoryPlatform::MacOs => ("open", vec!["-R".into(), path.as_os_str().to_owned()], true),
        DirectoryPlatform::Windows => (
            "explorer.exe",
            vec!["/select,".into(), path.as_os_str().to_owned()],
            false,
        ),
        DirectoryPlatform::Other => (
            "xdg-open",
            vec![path.parent().unwrap_or(path).as_os_str().to_owned()],
            true,
        ),
    };
    DirectoryOpener {
        program,
        args,
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

    #[test]
    fn file_revealers_select_files_or_open_their_directory() {
        let unix_path = PathBuf::from("/Users/Ada/No Ending/session.v2.jsonl.zstd");
        let mac = file_revealer(&unix_path, DirectoryPlatform::MacOs);
        assert_eq!(mac.program, "open");
        assert_eq!(
            mac.args,
            vec!["-R".into(), unix_path.as_os_str().to_owned()]
        );
        assert!(mac.wait_for_exit);

        let windows_path = PathBuf::from(r"C:\Users\Ada Lovelace\No Ending\session.v2.jsonl.zstd");
        let windows = file_revealer(&windows_path, DirectoryPlatform::Windows);
        assert_eq!(windows.program, "explorer.exe");
        assert_eq!(
            windows.args,
            vec!["/select,".into(), windows_path.as_os_str().to_owned()]
        );
        assert!(!windows.wait_for_exit);

        let linux = file_revealer(&unix_path, DirectoryPlatform::Other);
        assert_eq!(linux.program, "xdg-open");
        assert_eq!(
            linux.args,
            vec![unix_path.parent().unwrap().as_os_str().to_owned()]
        );
    }

    #[test]
    fn reveal_file_rejects_a_missing_source_before_launching_the_file_manager() {
        let missing = std::env::temp_dir()
            .join(format!("noending-missing-{}", crate::storage::new_id()))
            .join("session.v2.jsonl.zstd");
        let error = reveal_file(&missing).unwrap_err();
        assert!(error.to_string().contains("源会话文件已不存在"));
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

    /// Antigravity seeds BOTH official surfaces as default ingest sources:
    /// the IDE store and the `agy` CLI store, siblings under the same home.
    /// Agents without a second surface keep exactly one root.
    #[test]
    fn antigravity_ingest_roots_cover_the_ide_and_cli_stores() {
        let roots = agent_ingest_roots(Agent::Antigravity);
        assert_eq!(roots.len(), 2, "{roots:?}");
        assert!(
            roots.iter().any(|r| r.ends_with(".gemini/antigravity")),
            "{roots:?}"
        );
        assert!(
            roots.iter().any(|r| r.ends_with(".gemini/antigravity-cli")),
            "{roots:?}"
        );

        let single = agent_ingest_roots(Agent::Qoder);
        assert_eq!(single.len(), 1, "{single:?}");
    }
}
