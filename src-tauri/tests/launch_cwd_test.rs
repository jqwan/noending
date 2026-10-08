//! Launch directory selection uses explicit input or NoEnding's default workspace.
use noending::launcher::{resolve_new_cwd, CwdSource, LaunchWorkspace};
use noending::storage::new_id;
use std::path::PathBuf;

fn workspace() -> LaunchWorkspace {
    LaunchWorkspace {
        default_workspace: Some(
            std::env::temp_dir()
                .join(format!("noending-cwd-{}", new_id()))
                .to_string_lossy()
                .into_owned(),
        ),
    }
}

#[test]
fn explicit_cwd_wins_over_the_default_workspace() {
    let resolution = resolve_new_cwd(Some("/explicit/dir"), &workspace()).unwrap();
    assert_eq!(resolution.cwd.as_deref(), Some("/explicit/dir"));
    assert_eq!(resolution.source, CwdSource::Explicit);
    assert!(!resolution.fallback);
}

#[test]
fn unspecified_directory_uses_and_creates_the_default_workspace() {
    let workspace = workspace();
    let resolution = resolve_new_cwd(None, &workspace).unwrap();
    assert_eq!(resolution.cwd, workspace.default_workspace);
    assert_eq!(resolution.source, CwdSource::DefaultWorkspace);
    assert!(!resolution.fallback);
    assert!(PathBuf::from(resolution.cwd.unwrap()).is_dir());
}

#[test]
fn no_default_workspace_reports_an_unresolved_directory() {
    let resolution = resolve_new_cwd(None, &LaunchWorkspace::default()).unwrap();
    assert_eq!(resolution.cwd, None);
    assert_eq!(resolution.source, CwdSource::Unresolved);
    assert!(resolution.note.is_some());
}

#[test]
fn explicit_tilde_cwd_expands_to_home_directory() {
    let home = dirs::home_dir().unwrap();
    let resolution =
        resolve_new_cwd(Some("~/projects/noending"), &LaunchWorkspace::default()).unwrap();
    assert_eq!(
        resolution.cwd.as_deref(),
        Some(home.join("projects/noending").to_string_lossy().as_ref())
    );
    assert_eq!(resolution.source, CwdSource::Explicit);
}

#[test]
fn bare_tilde_expands_to_home_root() {
    let home = dirs::home_dir().unwrap();
    let resolution = resolve_new_cwd(Some("~"), &LaunchWorkspace::default()).unwrap();
    assert_eq!(
        resolution.cwd.as_deref(),
        Some(home.to_string_lossy().as_ref())
    );
}

#[test]
fn tilde_expands_only_at_leading_position() {
    let resolution = resolve_new_cwd(Some("/opt/a~b/dir"), &LaunchWorkspace::default()).unwrap();
    assert_eq!(resolution.cwd.as_deref(), Some("/opt/a~b/dir"));
}

#[test]
fn an_unusable_explicit_directory_is_honored_and_annotated() {
    let missing = std::env::temp_dir()
        .join(format!("noending-missing-{}", new_id()))
        .to_string_lossy()
        .into_owned();
    let resolution = resolve_new_cwd(Some(&missing), &workspace()).unwrap();
    assert_eq!(resolution.cwd.as_deref(), Some(missing.as_str()));
    assert_eq!(resolution.source, CwdSource::Explicit);
    assert!(resolution.note.is_some());
}
