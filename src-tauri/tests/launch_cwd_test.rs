//! New Session launch directory resolution (方案 §13, Workspace Domain v0.2).
//!
//! ```text
//! explicit cwd
//!     → the selected Workstreams' ordered WorkstreamPaths
//!       (first usable one, in the user's selection order and then list order)
//!     → NoEnding Home's default workspace
//! ```
//!
//! Two retired authorities are asserted on the way: `workstreams.default_cwd`
//! (§42.2-E6 — a frozen compatibility column, now only a v12 migration input)
//! and "the most recent Session cwd across these Workstreams" (the ordered path
//! list is what records where a Workstream's work happens).
//!
//! Every directory a test expects to be *chosen* is a real temp directory,
//! because §42.3-M21 forbids handing the terminal a directory that is not there
//! (macOS would print a hint and quietly cd to `$HOME`).

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use noending::domain::{binding_source, Agent, Project};
use noending::launcher::{record_binding, resolve_new_cwd, CwdSource, LaunchWorkspace};
use noending::storage::workspace::insert_workspace_path_conn;
use noending::storage::{new_id, now, Db};
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
    create_workstream(database, &LexicalPaths, title, "", None)
        .unwrap()
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
    workstream_ids: &[String],
    explicit: Option<&str>,
    default_workspace: Option<&str>,
) -> Option<String> {
    resolve_new_cwd(
        database,
        workstream_ids,
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
        new_cwd(&database, &[w], Some("/explicit/dir"), None).as_deref(),
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
        &[w.clone()],
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

/// The selection order is still the user's, not the database's: the first
/// selected Workstream that has a usable path wins, and reversing the
/// selection reverses the answer.
#[test]
fn first_selected_workstream_with_a_usable_path_wins() {
    let database = db("selection-order");
    let a = workstream(&database, "A");
    let b_paths = real_dir("selection-order", "b");
    let b = workstream_with_paths(&database, "B", &[b_paths.clone()]);
    let c_paths = real_dir("selection-order", "c");
    let c = workstream_with_paths(&database, "C", &[c_paths.clone()]);

    assert_eq!(
        new_cwd(&database, &[a.clone(), b.clone(), c.clone()], None, None).as_deref(),
        Some(b_paths.as_str()),
        "selection order decides which Workstream answers"
    );
    assert_eq!(
        new_cwd(&database, &[c.clone(), b], None, None).as_deref(),
        Some(c_paths.as_str())
    );
    let _ = a;
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

    let resolution = resolve_new_cwd(&database, &[w], None, &workspace(Some(&default))).unwrap();
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

    let resolution = resolve_new_cwd(&database, &[], None, &workspace(Some(&default))).unwrap();
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
        &[w],
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

    let resolution = resolve_new_cwd(&database, &[w], None, &LaunchWorkspace::default()).unwrap();
    assert_eq!(resolution.cwd, None);
    assert_eq!(resolution.source, CwdSource::Unresolved);
    assert!(
        resolution.note.is_some(),
        "the terminal default must be announced"
    );
}

// ------------------------------------------------------- retired authorities

/// §42.2-E6: `default_cwd` is frozen at creation and is NOT a launch authority
/// any more. A Workstream whose retired column points somewhere real must
/// still launch from its path list — otherwise the column is a second source of
/// the same fact, which is exactly what §29 forbids.
#[test]
fn the_frozen_default_cwd_column_is_not_a_launch_authority() {
    let database = db("frozen-column");
    let retired = real_dir("frozen-column", "retired");
    let w = {
        let row = noending::domain::Workstream {
            id: new_id(),
            project_id: None,
            title: "legacy".into(),
            description: String::new(),
            lifecycle: "active".into(),
            visibility: "normal".into(),
            default_cwd: Some(retired.clone()),
            created_at: now(),
            updated_at: now(),
        };
        database.upsert_workstream(&row).unwrap();
        row.id
    };

    // No paths and no default workspace: the retired column is not consulted.
    assert_eq!(
        resolve_new_cwd(&database, &[w.clone()], None, &LaunchWorkspace::default())
            .unwrap()
            .cwd,
        None
    );
    // With a path, the path answers.
    let live = real_dir("frozen-column", "live");
    add_paths(&database, &w, &[live.clone()]);
    assert_eq!(
        new_cwd(&database, &[w], None, None).as_deref(),
        Some(live.as_str())
    );
}

/// The pre-v0.2 "continue where you left off" tier is gone: a bound Session's
/// cwd is that Session's own fact and never a suggestion for a new one.
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
            raw_path: "/tmp/fake/session-cwd.jsonl".into(),
            parent_agent_session_id: None,
            started_at: Some("2026-09-01T08:00:00Z".into()),
            last_activity_at: Some("2026-09-12T09:00:00Z".into()),
            trashed_at: None,
        })
        .unwrap();
    record_binding(
        &database,
        &sid,
        &w,
        "related",
        binding_source::USER_ASSIGNED,
        1.0,
    )
    .unwrap();

    assert_eq!(
        resolve_new_cwd(&database, &[w], None, &LaunchWorkspace::default())
            .unwrap()
            .cwd,
        None,
        "the bound Session's directory ({elsewhere}) must not leak into a New Session"
    );
}

/// `default_cwd` is written at creation and then frozen (方案 §42.2-E6): under
/// Workspace Domain v0.2 the launch directory comes from the ordered
/// `workstream_paths` list, and this column is only a v12 migration input plus a
/// compatibility read. Because `update_workstream` is a whole-object write,
/// leaving it in the DO UPDATE set would keep re-committing it from unrelated
/// title edits — so neither setting nor clearing is possible after creation.
#[test]
fn default_cwd_is_frozen_after_creation() {
    let database = db("roundtrip");
    let w = {
        let row = noending::domain::Workstream {
            id: new_id(),
            project_id: None,
            title: "W".into(),
            description: String::new(),
            lifecycle: "active".into(),
            visibility: "normal".into(),
            default_cwd: Some("/some/dir".into()),
            created_at: now(),
            updated_at: now(),
        };
        database.upsert_workstream(&row).unwrap();
        row.id
    };

    let stored = database.get_workstream(&w).unwrap().unwrap();
    assert_eq!(stored.default_cwd.as_deref(), Some("/some/dir"));

    let mut changed = stored.clone();
    changed.default_cwd = Some("/somewhere/else".into());
    database.upsert_workstream(&changed).unwrap();
    assert_eq!(
        database
            .get_workstream(&w)
            .unwrap()
            .unwrap()
            .default_cwd
            .as_deref(),
        Some("/some/dir"),
        "an after-the-fact retarget must not stick"
    );

    let mut cleared = database.get_workstream(&w).unwrap().unwrap();
    cleared.default_cwd = None;
    cleared.title = "改名".into();
    database.upsert_workstream(&cleared).unwrap();
    let after = database.get_workstream(&w).unwrap().unwrap();
    assert_eq!(after.title, "改名", "the intended edit still goes through");
    assert_eq!(
        after.default_cwd.as_deref(),
        Some("/some/dir"),
        "and clearing the retired column through an edit is not possible either"
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
        &[],
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
        resolve_new_cwd(&database, &[], Some("~"), &LaunchWorkspace::default())
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
            &[],
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
        &[w],
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
