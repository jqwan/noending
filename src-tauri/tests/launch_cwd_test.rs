//! New Session launch directory resolution (方案 §13, Workspace Domain v0.2).
//!
//! ```text
//! explicit cwd
//!     → the single Owner Workstream's ordered WorkstreamPaths
//!       (first usable one, in list order)
//!     → NoEnding Home's default workspace
//! ```
//!
//! The ordered path list is the only Workstream workspace authority; an owned
//! Session's cwd is never a suggestion for a new launch (方案 §15).
//!
//! Every directory a test expects to be *chosen* is a real temp directory,
//! because §42.3-M21 forbids handing the terminal a directory that is not there
//! (macOS would print a hint and quietly cd to `$HOME`).

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use noending::domain::{Agent, Project};
use noending::launcher::{resolve_new_cwd, CwdSource, LaunchWorkspace};
use noending::storage::workspace::insert_workspace_path_conn;
use noending::storage::{new_id, Db};
use noending::workspace::workstream::{add_workstream_path, create_workstream};
use noending::workspace::{normalize_path, WorkspaceAttaching};

const PROJECT: &str = "p-cwd-test";

fn db(tag: &str) -> Db {
    let dir = temp_root(tag);
    let database = Db::open(&dir.join("test.db")).unwrap();
    database
        .upsert_project(&Project::new(PROJECT.into(), "P"))
        .unwrap();
    database
}

/// The launcher is a *reader* of WorkspacePaths, so a test fixture needs a real
/// one. This is the stand-in for `workspace::project`: pure lexical resolution
/// into a WorkspacePath row, no Git (§42.3-M8 keeps identity filesystem-free).
#[derive(Default)]
struct LexicalPaths;

impl WorkspaceAttaching for LexicalPaths {
    fn ensure_path(&self, conn: &Connection, raw: &str) -> noending::error::Result<Option<String>> {
        Ok(match normalize_path(raw) {
            Some(canonical) => Some(insert_workspace_path_conn(conn, &canonical, PROJECT)?),
            None => None,
        })
    }
}

fn temp_root(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("noending-launch-cwd-{}-{}", tag, new_id()))
}

/// A directory that really exists — "usable" in §13's sense.
///
/// §42.3-M8 note 7: never assert a stored canonical path against a literal. The
/// temp dir is handed to the same normalizer production uses, so this file says
/// the same thing on macOS, Linux and a Windows `C:\…` temp path.
fn real_dir(tag: &str, name: &str) -> String {
    let dir = temp_root(tag).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    canonical(&dir)
}

fn canonical(path: impl AsRef<Path>) -> String {
    normalize_path(&path.as_ref().to_string_lossy()).expect("fixture path is normalizable")
}

/// A default workspace that does NOT exist yet: §42.3-M21 requires the launcher
/// to treat Home's default workspace as creatable rather than skip it.
fn missing_dir(tag: &str, name: &str) -> String {
    canonical(temp_root(tag).join(name))
}

fn workstream(database: &Db, title: &str) -> String {
    create_workstream(database, &LexicalPaths, title, "", &[])
        .unwrap()
        .workstream
        .id
}

fn workstream_with_paths(database: &Db, title: &str, dirs: &[String]) -> String {
    let id = workstream(database, title);
    add_paths(database, &id, dirs);
    id
}

fn add_paths(database: &Db, workstream_id: &str, dirs: &[String]) {
    for dir in dirs {
        add_workstream_path(database, &LexicalPaths, workstream_id, dir).unwrap();
    }
}

fn workspace(default_workspace: Option<&str>) -> LaunchWorkspace {
    LaunchWorkspace {
        default_workspace: default_workspace.map(|s| s.to_string()),
    }
}

fn new_cwd(
    database: &Db,
    owner_workstream_id: Option<&str>,
    explicit: Option<&str>,
    default_workspace: Option<&str>,
) -> Option<String> {
    resolve_new_cwd(
        database,
        owner_workstream_id,
        explicit,
        &workspace(default_workspace),
    )
    .unwrap()
    .cwd
}

// ------------------------------------------------------------- the tiers

/// §21 "explicit cwd wins": a directory the user named outranks everything,
/// including a Workstream whose primary path is a perfectly good directory.
#[test]
fn explicit_cwd_wins_over_everything() {
    let database = db("explicit");
    let w = workstream_with_paths(&database, "W", &[real_dir("explicit", "primary")]);

    assert_eq!(
        new_cwd(&database, Some(&w), Some("/explicit/dir"), None).as_deref(),
        Some("/explicit/dir"),
    );
}

/// §21 "primary path wins": position 0 of the ordered list is the launch
/// directory, ahead of every other entry and ahead of the default workspace.
#[test]
fn primary_workstream_path_wins() {
    let database = db("primary");
    let primary = real_dir("primary", "zero");
    let second = real_dir("primary", "one");
    let w = workstream_with_paths(&database, "W", &[primary.clone(), second]);

    let resolution = resolve_new_cwd(
        &database,
        Some(&w.clone()),
        None,
        &workspace(Some(&real_dir("primary", "default"))),
    )
    .unwrap();
    assert_eq!(resolution.cwd.as_deref(), Some(primary.as_str()));
    assert_eq!(resolution.source, CwdSource::WorkstreamPath);
    assert_eq!(resolution.path_position, Some(0));
    assert!(!resolution.fallback, "position 0 is the expected answer");
    assert_eq!(resolution.workstream_id.as_deref(), Some(w.as_str()));
}

/// §21 "no Workstream path → default workspace": a Workstream with zero paths
/// is fully valid (§1.5), and the launch falls to Home's default workspace —
/// marked as a fallback, because the user picked a Workstream expecting to work
/// in *its* directory.
#[test]
fn workstream_without_paths_falls_back_to_the_default_workspace() {
    let database = db("no-paths");
    let w = workstream(&database, "W");
    let default = missing_dir("no-paths", "workspace");

    let resolution =
        resolve_new_cwd(&database, Some(&w), None, &workspace(Some(&default))).unwrap();
    assert_eq!(resolution.source, CwdSource::DefaultWorkspace);
    assert_eq!(resolution.cwd.as_deref(), Some(default.as_str()));
    assert!(
        resolution.fallback,
        "a selected Workstream with no path is a downgrade the UI must show"
    );
    assert!(
        std::fs::metadata(&default)
            .map(|m| m.is_dir())
            .unwrap_or(false),
        "§42.3-M21 — the default workspace is created, never handed over missing"
    );
}

/// §21 "standalone → default workspace": with no Workstream at all the default
/// workspace IS the documented answer, so it is not a fallback.
#[test]
fn standalone_new_session_uses_the_default_workspace() {
    let database = db("standalone");
    let default = real_dir("standalone", "workspace");

    let resolution = resolve_new_cwd(&database, None, None, &workspace(Some(&default))).unwrap();
    assert_eq!(resolution.cwd.as_deref(), Some(default.as_str()));
    assert_eq!(resolution.source, CwdSource::DefaultWorkspace);
    assert!(
        !resolution.fallback,
        "a standalone launch has nothing to fall back from"
    );
}

/// A Workstream path that is gone (unmounted volume, deleted directory) is
/// skipped, and the entry it skipped to is reported with its position.
#[test]
fn an_unusable_primary_falls_to_the_next_path_and_says_so() {
    let database = db("unusable-primary");
    let lost = missing_dir("unusable-primary", "gone");
    let live = real_dir("unusable-primary", "live");
    let w = workstream_with_paths(&database, "W", &[lost.clone(), live.clone()]);

    let resolution = resolve_new_cwd(
        &database,
        Some(&w),
        None,
        &workspace(Some(&real_dir("unusable-primary", "default"))),
    )
    .unwrap();
    assert_eq!(
        resolution.cwd.as_deref(),
        Some(live.as_str()),
        "position 0 wins while it can; a missing primary yields to the next entry"
    );
    assert_eq!(resolution.path_position, Some(1));
    assert!(resolution.fallback);
    assert!(resolution.note.is_some());
}

/// Nothing usable anywhere and no Home default: the honest answer is "no
/// directory", not an implied one.
#[test]
fn nothing_resolves_to_no_directory_at_all() {
    let database = db("nothing");
    let w = workstream(&database, "W");

    let resolution =
        resolve_new_cwd(&database, Some(&w), None, &LaunchWorkspace::default()).unwrap();
    assert_eq!(resolution.cwd, None);
    assert_eq!(resolution.source, CwdSource::Unresolved);
    assert!(
        resolution.note.is_some(),
        "the terminal default must be announced"
    );
}

/// An owned Session's cwd is its own fact and never a suggestion for a new one.
#[test]
fn another_sessions_cwd_is_not_a_launch_authority() {
    let database = db("session-cwd");
    let w = workstream(&database, "W");
    let elsewhere = canonical(temp_root("session-cwd").join("elsewhere"));
    let sid = new_id();
    database
        .upsert_session(&noending::domain::Session {
            id: sid.clone(),
            agent: Agent::Codex,
            agent_session_id: sid.clone(),
            title: None,
            cwd: Some(elsewhere.clone()),
            workspace_path_id: None,
            project_id: None,
            owner_workstream_id: Some(w.clone()),
            raw_path: "/tmp/fake/session-cwd.jsonl".into(),
            parent_agent_session_id: None,
            started_at: Some("2026-09-01T08:00:00Z".into()),
            last_activity_at: Some("2026-09-12T09:00:00Z".into()),
            trashed_at: None,
        })
        .unwrap();

    assert_eq!(
        resolve_new_cwd(&database, Some(&w), None, &LaunchWorkspace::default())
            .unwrap()
            .cwd,
        None,
        "the owned Session's directory ({elsewhere}) must not leak into a New Session"
    );
}

// ----------------------------------------------------------------- tilde

/// Users type `~/projects/x` — the terminal renderers quote the path, and a
/// literal `~` inside quotes never expands (the launch would silently land
/// in $HOME). Resolution must hand the renderers an absolute path.
#[test]
fn explicit_tilde_cwd_expands_to_home_directory() {
    let database = db("tilde");
    let home = dirs::home_dir().expect("home dir available in test env");

    let resolution = resolve_new_cwd(
        &database,
        None,
        Some("~/projects/noending"),
        &LaunchWorkspace::default(),
    )
    .unwrap();
    assert_eq!(
        resolution.cwd.as_deref(),
        Some(home.join("projects/noending").to_string_lossy().as_ref())
    );
    assert_eq!(resolution.source, CwdSource::Explicit);
}

#[test]
fn bare_tilde_expands_to_home_root() {
    let database = db("tilde-bare");
    let home = dirs::home_dir().unwrap();

    assert_eq!(
        resolve_new_cwd(&database, None, Some("~"), &LaunchWorkspace::default())
            .unwrap()
            .cwd
            .as_deref(),
        Some(home.to_string_lossy().as_ref())
    );
}

/// `~` only means home at the leading position; an absolute path containing
/// it is not touched.
#[test]
fn tilde_expands_only_at_leading_position() {
    let database = db("tilde-mid");

    assert_eq!(
        resolve_new_cwd(
            &database,
            None,
            Some("/opt/a~b/dir"),
            &LaunchWorkspace::default(),
        )
        .unwrap()
        .cwd
        .as_deref(),
        Some("/opt/a~b/dir")
    );
}

/// A directory the user *typed* is honored even when it is not there — the
/// launcher must not quietly substitute another tier, because that would
/// override an explicit choice. It does, however, say what will happen.
#[test]
fn an_unusable_explicit_directory_is_honored_and_annotated() {
    let database = db("explicit-missing");
    let w = workstream_with_paths(&database, "W", &[real_dir("explicit-missing", "live")]);

    let resolution = resolve_new_cwd(
        &database,
        Some(&w),
        Some("/definitely/not/here"),
        &workspace(Some(&real_dir("explicit-missing", "default"))),
    )
    .unwrap();
    assert_eq!(resolution.cwd.as_deref(), Some("/definitely/not/here"));
    assert_eq!(resolution.source, CwdSource::Explicit);
    assert!(
        resolution.note.is_some(),
        "a `cd` that cannot succeed prints a hint and lands in $HOME on macOS, \
         so the preview must not pretend otherwise (§42.3-M21)"
    );
}
