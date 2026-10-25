//! Project projection and the WorkspacePath registry: the `WorkspaceAttaching`
//! implementation, the reconcile sweep, archive lifecycle, automatic naming and the detail shape.
//!
//! No test touches a real repository, a real NoEnding Home or `git`: observations
//! are constructed by script (the `WorkspaceObserving` / `WorkspacePolicy` seams).
//! Paths are absolute strings that need not exist — path identity is lexical — and
//! assertions compare against `canon(..)` rather than a spelled-out prefix,
//! because `canonical_path` is stored platform-native and CI runs macOS + Windows.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
mod support;

use noending::domain::{
    git_state, Agent, GitDetection, GitWorktreeKind, Project, ProjectKind, ProjectPurpose, Session,
    WorkspaceObservation, WorkspacePath,
};
use noending::storage::workspace::{insert_workspace_path_conn, rename_project_conn};
use noending::storage::Db;
use noending::workspace::project::{
    delete_project_permanently, ensure_workspace_path, ensure_workspace_path_conn,
    merge_projects_conn, project_detail, reconcile_workspace_paths, registry_is_consistent,
    set_project_archived, ProjectProjection, UnrestrictedWorkspace, WorkspacePolicy,
};
use noending::workspace::resolver::WorkspaceObserving;
use noending::workspace::{is_within, normalize_path, path_identity, WorkspaceAttaching};
use noending_lib as noending;
use support::associate_path_project as append_workstream_path_conn;

// fixtures

fn unique_dir(tag: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "noending-proj-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn temp_db() -> (PathBuf, Db) {
    let dir = unique_dir("db");
    let db = Db::open(&dir.join("noending.db")).expect("temp db");
    (dir, db)
}

fn temp_db_locked() -> (PathBuf, Db) {
    let dir = unique_dir("mutex");
    let db = Db::open(&dir.join("noending.db")).expect("temp db");
    (dir, db)
}

/// Platform-native canonical form of a test path — also its storage key, so every
/// assertion compares against this rather than a literal.
fn canon(raw: &str) -> String {
    normalize_path(raw).expect("test paths are absolute")
}

fn path_id_of(raw: &str) -> String {
    path_identity(&canon(raw))
}

/// Home-anchored helper: the chat-bucket roots are defined relative to the
/// user home, so tests build their fixtures the same way and stay
/// home-independent.
fn home_path(rel: &str) -> String {
    noending::platform::paths::resolve_home()
        .expect("tests run with a home directory")
        .join(rel)
        .to_string_lossy()
        .to_string()
}

/// Chat scratch directories under an Agent app's own root share ONE bucket
/// Project named after the Agent; lookalikes outside the roots keep their own
/// projects, and a different app means a different bucket.
#[test]
fn chat_directories_under_an_app_root_share_one_bucket_project() {
    let (_dir, db) = temp_db();
    let ensure =
        |raw: &str| ensure_workspace_path(&db, &plain(raw, true), &UnrestrictedWorkspace).unwrap();

    let a = ensure(&home_path("Documents/Qoder/2026-09-19/b8a25663"));
    let b = ensure(&home_path("Documents/Qoder/2026-09-19/2b6ac118"));
    assert_eq!(
        a.project_id, b.project_id,
        "one chat, one project — but all chats of the app share the bucket"
    );
    let (name, git_id, customized): (String, Option<String>, i64) = db
        .read()
        .query_row(
            "SELECT name, git_id, name_customized FROM projects WHERE id = ?1",
            [&a.project_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        name, "Qoder Chat",
        "the bucket is named after the Agent, not the first chat dir"
    );
    assert_eq!(git_id, None);
    assert_eq!(
        customized, 0,
        "still an automatic name the user may take over"
    );

    // A different app is a different bucket.
    let wb = ensure(&home_path("Workbuddy/2026-07-04-12-00-30"));
    assert_ne!(wb.project_id, a.project_id);
    let wb_name: String = db
        .read()
        .query_row(
            "SELECT name FROM projects WHERE id = ?1",
            [&wb.project_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(wb_name, "WorkBuddy Chat");

    // Lookalikes are NOT the bucket: component-wise matching.
    let lookalike = ensure(&home_path("Documents/QoderBackup/x"));
    assert_ne!(lookalike.project_id, a.project_id);
    // A repository literally named codex is a project, not a bucket.
    let real = ensure(&home_path("projects/codex"));
    assert_ne!(real.project_id, a.project_id);
}

/// A directory with no Git evidence at all.
fn plain(raw: &str, exists: bool) -> WorkspaceObservation {
    let canonical = canon(raw);
    WorkspaceObservation {
        path_id: path_identity(&canonical),
        canonical_path: canonical,
        exists,
        git: GitDetection::None,
    }
}

/// A working tree of the repository whose common dir is `common`.
fn repo(
    raw: &str,
    common: &str,
    kind: GitWorktreeKind,
    worktrees: &[&str],
) -> WorkspaceObservation {
    let canonical = canon(raw);
    let toplevel = canonical.clone();
    WorkspaceObservation {
        path_id: path_identity(&canonical),
        canonical_path: canonical,
        exists: true,
        git: GitDetection::Detected {
            common_dir: canon(common),
            toplevel: Some(toplevel),
            kind,
            worktrees: worktrees.iter().map(|w| canon(w)).collect(),
            remotes: Vec::new(),
        },
    }
}

/// A path that used to have a `.git` and now reports that it does not.
fn git_gone(raw: &str) -> WorkspaceObservation {
    WorkspaceObservation {
        git: GitDetection::Missing,
        ..plain(raw, true)
    }
}

fn ensure(db: &Db, observation: &WorkspaceObservation) -> WorkspacePath {
    ensure_workspace_path(db, observation, &UnrestrictedWorkspace).expect("ensure")
}

/// Register a path under an existing Project the way a bulk import does, so a
/// test can build the multi-path Project shape itself never produces.
fn insert_plain(db: &Db, raw: &str, project_id: &str) -> WorkspacePath {
    let id = db
        .tx(|tx| insert_workspace_path_conn(tx, &canon(raw), project_id))
        .unwrap();
    db.get_workspace_path(&id).unwrap().expect("inserted")
}

fn project_of(db: &Db, path: &WorkspacePath) -> Project {
    db.get_project(&path.project_id)
        .unwrap()
        .expect("a WorkspacePath always belongs to exactly one Project")
}

/// The Git family of the Project owning `path`.
fn family_of(db: &Db, path: &WorkspacePath) -> String {
    project_of(db, path).git_id.expect("git-backed Project")
}

fn paths_of(db: &Db, project_id: &str) -> Vec<WorkspacePath> {
    db.list_workspace_paths_for_project(project_id).unwrap()
}

/// The row id is store-assigned (identity is the ROOT member's Agent-side
/// id), so callers use the returned Session instead of a fixture id.
fn session(db: &Db, id: &str, cwd: &str, path_id: &str) -> Session {
    let (row_id, _) = db
        .upsert_logical_session_unchecked(
            Agent::Codex,
            &format!("src-{id}"),
            None,
            Some(&canon(cwd)),
            Some(path_id),
            None,
            None,
            None,
        )
        .unwrap();
    db.get_session(&row_id).unwrap().expect("session row")
}

fn workstream(db: &Db, id: &str, title: &str) {
    db.upsert_workstream(&support::workstream(id.into(), title))
        .unwrap();
}

fn add_ws_path(db: &Db, workstream_id: &str, workspace_path_id: &str) {
    db.tx(|tx| append_workstream_path_conn(tx, workstream_id, workspace_path_id))
        .unwrap();
}

fn count_rows(db: &Db, sql: &str, arg: &str) -> i64 {
    db.read()
        .query_row(sql, [arg], |r| r.get::<_, i64>(0))
        .unwrap()
}

/// A scripted resolver: whatever a test registers, and "an ordinary existing
/// directory" for anything else. `&self` only, so the reconcile sweep can share it
/// across lock boundaries like the real resolver does.
#[derive(Clone, Default)]
struct Scripted {
    answers: std::sync::Arc<Mutex<BTreeMap<String, WorkspaceObservation>>>,
}

impl Scripted {
    fn new() -> Self {
        Self::default()
    }

    fn set(&self, raw: &str, observation: WorkspaceObservation) {
        self.answers.lock().unwrap().insert(canon(raw), observation);
    }
}

impl WorkspaceObserving for Scripted {
    fn observe(&self, raw: &str) -> WorkspaceObservation {
        let key = normalize_path(raw);
        let store = self.answers.lock().unwrap();
        if let Some(o) = key.as_ref().and_then(|k| store.get(k)) {
            return o.clone();
        }
        match key {
            Some(canonical) => WorkspaceObservation {
                path_id: path_identity(&canonical),
                canonical_path: canonical,
                exists: true,
                git: GitDetection::None,
            },
            // Not a path we can name (empty, or relative with no base): the door
            // must see "nothing", never a guess.
            None => WorkspaceObservation {
                path_id: String::new(),
                canonical_path: String::new(),
                exists: false,
                git: GitDetection::None,
            },
        }
    }
}

/// The NoEnding Home stand-in: its root, its reserved app subtrees, and the
/// default workspace. `workspace/` is deliberately NOT reserved
/// a policy that reserved the whole tree would delete the default workspace.
struct Home {
    root: String,
    reserved: Vec<String>,
    default_workspace: Option<String>,
}

impl Home {
    fn new(root: &str, reserved: &[&str], default_workspace: Option<&str>) -> Self {
        Self {
            root: canon(root),
            reserved: reserved.iter().map(|r| canon(r)).collect(),
            default_workspace: default_workspace.map(canon),
        }
    }
}

impl WorkspacePolicy for Home {
    fn is_reserved(&self, canonical_path: &str) -> bool {
        // Segment-wise through the same helper the real Home must use: on Windows
        // both sides are `\`-separated, so a hand-rolled `{r}/` prefix test would
        // silently admit every reserved path there.
        canonical_path == self.root || self.reserved.iter().any(|r| is_within(canonical_path, r))
    }

    fn default_workspace(&self) -> Option<String> {
        self.default_workspace.clone()
    }

    fn assistant_workspace(&self) -> Option<String> {
        Some(canon(&format!("{}/assistant", self.root)))
    }

    fn exists_on_disk(&self, canonical_path: &str) -> bool {
        // Same rule the production policy uses: existence is read from the real
        // host, never assumed. Tests that hand in a non-existent path therefore
        // get `false`, which is what adoption must record.
        noending::workspace::resolver::exists_on_disk(canonical_path)
    }
}

// the door

#[test]
fn new_non_git_path_creates_exactly_one_project() {
    let (_d, db) = temp_db();
    let path = ensure(&db, &plain("/work/alpha", true));

    assert_eq!(path.canonical_path, canon("/work/alpha"));
    assert_eq!(
        path.id,
        path_id_of("/work/alpha"),
        "id IS the path identity"
    );
    assert!(path.exists);
    assert_eq!(path.git_state, git_state::NONE);
    assert_eq!(path.git_kind, None);

    let projects = db.list_projects().unwrap();
    assert_eq!(projects.len(), 1, "one new plain directory, one Project");
    assert_eq!(projects[0].id, path.project_id);
    assert_eq!(projects[0].name, "Alpha", "automatic naming");
    assert_eq!(projects[0].git_id, None);
    assert!(!projects[0].name_customized);
    assert_eq!(paths_of(&db, &projects[0].id).len(), 1);
    registry_is_consistent(&db).expect("consistent");
}

#[test]
fn ensuring_the_same_path_twice_is_idempotent() {
    let (_d, db) = temp_db();
    let first = ensure(&db, &plain("/work/alpha", true));
    // A different spelling of the same directory is the same identity, so it must
    // not become a second WorkspacePath.
    let second = ensure(&db, &plain("/work/alpha/./", true));

    assert_eq!(first.id, second.id);
    assert_eq!(first.project_id, second.project_id);
    assert_eq!(db.list_workspace_paths().unwrap().len(), 1);
    assert_eq!(db.list_projects().unwrap().len(), 1);

    // Replaying the same observation preserves the path and its project.
    ensure(&db, &plain("/work/alpha", true));
    assert_eq!(db.list_workspace_paths().unwrap().len(), 1);
    let projects = db.list_projects().unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "Alpha", "a re-ensure never renames");
    assert_eq!(projects[0].git_id, None, "nor invents an identity");
}

#[test]
fn new_git_path_creates_a_git_backed_project() {
    let (_d, db) = temp_db();
    let path = ensure(
        &db,
        &repo("/work/repo", "/work/repo/.git", GitWorktreeKind::Main, &[]),
    );

    assert_eq!(path.git_state, git_state::DETECTED);
    assert_eq!(path.git_kind.as_deref(), Some("main"));
    let project = project_of(&db, &path);
    let git_id = project.git_id.clone().expect("the family is recorded");

    // The family is a `git_identities` row keyed by the common dir.
    let identity = db
        .find_git_identity_by_common_dir(&canon("/work/repo/.git"))
        .unwrap()
        .expect("git identity persisted");
    assert_eq!(identity.id, git_id);
    assert_ne!(
        identity.id,
        path_id_of("/work/repo"),
        "a Git id is app-assigned, not a second copy of the path hash"
    );
    assert_eq!(db.list_projects().unwrap().len(), 1);
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM git_identities WHERE common_dir = ?1",
            &canon("/work/repo/.git")
        ),
        1
    );
    // Re-observing the same repository does not re-key it: one row, one Project.
    ensure(
        &db,
        &repo("/work/repo", "/work/repo/.git", GitWorktreeKind::Main, &[]),
    );
    assert_eq!(db.list_projects().unwrap().len(), 1);
    assert_eq!(project_of(&db, &path).git_id, Some(git_id));
}

/// + an adopted worktree's existence is observed, never assumed.
///
/// `git worktree list` reports registrations whether or not the directory is
/// still on this machine; adopted worktrees report observed existence, and
/// missing registrations are not adopted.
#[test]
fn adopted_worktrees_report_the_existence_that_was_observed() {
    let (_d, db) = temp_db();
    let root = unique_dir("wt-exists");
    let main = root.join("repo");
    let sibling = root.join("repo-feature");
    std::fs::create_dir_all(&main).unwrap();
    std::fs::create_dir_all(&sibling).unwrap();
    let ghost = root.join("removed-long-ago"); // never created

    let row = ensure(
        &db,
        &repo(
            &main.to_string_lossy(),
            &main.join(".git").to_string_lossy(),
            GitWorktreeKind::Main,
            &[
                &main.to_string_lossy(),
                &sibling.to_string_lossy(),
                &ghost.to_string_lossy(),
            ],
        ),
    );

    let family = paths_of(&db, &row.project_id);
    assert_eq!(
        family.len(),
        2,
        "only existing worktrees become WorkspacePaths"
    );

    let by_path = |p: &std::path::Path| {
        family
            .iter()
            .find(|w| w.canonical_path == canon(&p.to_string_lossy()))
            .unwrap_or_else(|| panic!("no row for {}", p.display()))
    };
    assert!(
        by_path(&sibling).exists,
        "a worktree that is on disk must be adopted as existing"
    );
    assert!(
        db.get_workspace_path(&path_id_of(&ghost.to_string_lossy()))
            .unwrap()
            .is_none(),
        "stale Git registrations must not recreate removed directories"
    );
}

#[test]
fn two_worktrees_of_one_repo_share_one_project() {
    let (_d, db) = temp_db();
    let main = ensure(
        &db,
        &repo("/work/repo", "/work/repo/.git", GitWorktreeKind::Main, &[]),
    );
    let linked = ensure(
        &db,
        &repo(
            "/work/repo-feature",
            "/work/repo/.git",
            GitWorktreeKind::Linked,
            &[],
        ),
    );

    assert_eq!(
        main.project_id, linked.project_id,
        "one common dir means one Project"
    );
    assert_eq!(db.list_projects().unwrap().len(), 1);
    assert_eq!(paths_of(&db, &main.project_id).len(), 2);
    assert_eq!(linked.git_kind.as_deref(), Some("linked"));
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM projects WHERE git_id = ?1",
            &family_of(&db, &main)
        ),
        1,
        "`projects.git_id` is UNIQUE when set: one family, one owner"
    );
}

// Project upgrades, moves, and evidence that disappears

#[test]
fn path_project_upgrades_to_git_project_without_changing_project_id() {
    let (_d, db) = temp_db();
    let path = ensure(&db, &plain("/work/repo", true));
    let project = project_of(&db, &path);
    assert_eq!(project.git_id, None);
    let s_up = session(&db, "s-up", "/work/repo", &path.id);

    // `git init` ran. The Project adopts the family; the row it owns does not move.
    let after = ensure(
        &db,
        &repo("/work/repo", "/work/repo/.git", GitWorktreeKind::Main, &[]),
    );
    let upgraded = project_of(&db, &after);

    assert_eq!(upgraded.id, project.id, "Project.id is unchanged");
    assert_eq!(after.id, path.id, "the WorkspacePath did not move either");
    assert_eq!(after.project_id, path.project_id);
    assert_eq!(after.git_state, git_state::DETECTED);
    assert!(db
        .find_git_identity_by_common_dir(&canon("/work/repo/.git"))
        .unwrap()
        .is_some());
    assert_eq!(
        db.list_projects().unwrap().len(),
        1,
        "an upgrade creates no second Project"
    );
    assert_eq!(
        db.get_session(&s_up.id)
            .unwrap()
            .unwrap()
            .project_id
            .as_deref(),
        Some(project.id.as_str()),
        "the read projection agrees with the path"
    );
    // One-way and idempotent: replaying the upgrade is a no-op.
    ensure(
        &db,
        &repo("/work/repo", "/work/repo/.git", GitWorktreeKind::Main, &[]),
    );
    assert_eq!(db.list_projects().unwrap().len(), 1);
    assert_eq!(project_of(&db, &after).git_id, upgraded.git_id);
    registry_is_consistent(&db).expect("consistent");
}

#[test]
fn git_missing_does_not_detach_the_path() {
    let (_d, db) = temp_db();
    let path = ensure(
        &db,
        &repo("/work/repo", "/work/repo/.git", GitWorktreeKind::Main, &[]),
    );
    let project = project_of(&db, &path);
    let s_keep = session(&db, "s-keep", "/work/repo", &path.id);

    // The `.git` directory vanished. The observation allows exactly one
    // change: `git_state`.
    let missing = ensure(&db, &plain("/work/repo", true));
    assert_eq!(missing.git_state, git_state::MISSING, "");
    assert_eq!(
        missing.git_kind, None,
        "kind describes evidence we no longer have"
    );
    assert_eq!(missing.project_id, project.id, "the path is NOT detached");
    let still = project_of(&db, &missing);
    assert_eq!(
        still.git_id, project.git_id,
        "Project.git_id is never cleared by a path that lost its evidence"
    );
    assert_eq!(
        db.list_projects().unwrap().len(),
        1,
        "and losing evidence creates no new Project either"
    );

    // `GitDetection::Missing` is the same decision for ownership, and `missing` is
    // remembered — it is never silently reclassified as "never was a repository".
    let again = ensure(&db, &git_gone("/work/repo"));
    assert_eq!(again.git_state, git_state::MISSING);
    assert_eq!(again.project_id, project.id);
    // `Unavailable` (no git binary, timeout, dubious ownership) likewise.
    let unavailable = WorkspaceObservation {
        git: GitDetection::Unavailable,
        ..plain("/work/repo", true)
    };
    let still_missing = ensure(&db, &unavailable);
    assert_eq!(still_missing.git_state, git_state::MISSING);
    assert_eq!(still_missing.project_id, project.id);

    // Evidence coming back restores `detected` on the same two rows.
    let restored = ensure(
        &db,
        &repo("/work/repo", "/work/repo/.git", GitWorktreeKind::Main, &[]),
    );
    assert_eq!(restored.git_state, git_state::DETECTED);
    assert_eq!(restored.project_id, project.id);
    assert_eq!(
        db.get_session(&s_keep.id)
            .unwrap()
            .unwrap()
            .project_id
            .as_deref(),
        Some(project.id.as_str())
    );
    registry_is_consistent(&db).expect("consistent");
}

#[test]
fn different_git_family_moves_the_path_and_preserves_the_empty_old_project() {
    let (_d, db) = temp_db();
    let a = ensure(
        &db,
        &repo("/work/one", "/work/common-git", GitWorktreeKind::Main, &[]),
    );
    let b = ensure(
        &db,
        &repo(
            "/work/two",
            "/work/common-git",
            GitWorktreeKind::Linked,
            &[],
        ),
    );
    assert_eq!(a.project_id, b.project_id);
    let g1 = family_of(&db, &a);
    let s_a = session(&db, "s-a", "/work/one", &a.id);
    let s_b = session(&db, "s-b", "/work/two", &b.id);

    // `/work/one` now sits inside a DIFFERENT repository. That is a strong
    // identity change — this path moves, its sibling keeps its own family.
    let moved = ensure(
        &db,
        &repo("/work/one", "/other/common-git", GitWorktreeKind::Main, &[]),
    );
    let g2 = family_of(&db, &moved);
    assert_ne!(g1, g2);
    assert_ne!(moved.project_id, a.project_id);
    assert_eq!(
        paths_of(&db, &b.project_id).len(),
        1,
        "evidence is per-path: the untouched worktree stays with G1"
    );
    assert_eq!(
        db.get_session(&s_a.id)
            .unwrap()
            .unwrap()
            .project_id
            .as_deref(),
        Some(moved.project_id.as_str()),
        "the read projection follows its path"
    );
    assert_eq!(
        db.get_session(&s_b.id)
            .unwrap()
            .unwrap()
            .project_id
            .as_deref(),
        Some(b.project_id.as_str()),
        "a Session whose path did not move is never dragged along"
    );

    // When the remaining path reports G2 too, the old Project loses its last path
    // and remains registered until explicitly deleted.
    let joined = ensure(
        &db,
        &repo(
            "/work/two",
            "/other/common-git",
            GitWorktreeKind::Linked,
            &[],
        ),
    );
    assert_eq!(joined.project_id, moved.project_id);
    assert!(db.get_project(&a.project_id).unwrap().is_some());
    assert_eq!(db.list_projects().unwrap().len(), 2);
    assert_eq!(paths_of(&db, &moved.project_id).len(), 2);
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM sessions s JOIN workspace_paths wp ON wp.id = s.workspace_path_id WHERE wp.project_id = ?1",
            &a.project_id
        ),
        0,
        "the empty old project has no remaining session association"
    );
    registry_is_consistent(&db).expect("holds at every committed moment");
}

#[test]
fn merge_deletes_the_zero_path_project() {
    let (_d, db) = temp_db();
    // A path-backed Project holding two plain directories. gives every new
    // path its own Project, so this shape only arrives when paths are registered
    // onto one Project in bulk — which is exactly the case that has to merge
    // cleanly. `insert_workspace_path_conn` registers a path directly.
    let legacy = "p-legacy".to_string();
    db.upsert_project(&support::project(legacy.clone(), "Legacy"))
        .unwrap();
    let one = insert_plain(&db, "/legacy/one", &legacy);
    let two = insert_plain(&db, "/legacy/two", &legacy);
    assert_eq!(one.project_id, two.project_id);
    let merged_away = one.project_id.clone();
    let s1 = session(&db, "s-1", "/legacy/one", &one.id);
    let s2 = session(&db, "s-2", "/legacy/two", &two.id);
    workstream(&db, "w-1", "keeps working");
    add_ws_path(&db, "w-1", &two.id);

    // One of them belongs to a family that already has a Project.
    let owner = ensure(
        &db,
        &repo(
            "/elsewhere/repo",
            "/elsewhere/repo/.git",
            GitWorktreeKind::Main,
            &[],
        ),
    );
    let survivor = owner.project_id.clone();
    assert_ne!(survivor, merged_away);
    let joined = ensure(
        &db,
        &repo(
            "/legacy/one",
            "/elsewhere/repo/.git",
            GitWorktreeKind::Linked,
            &[],
        ),
    );

    assert_eq!(joined.project_id, survivor);
    assert!(
        db.get_project(&merged_away).unwrap().is_none(),
        "the merged Project is deleted, not left empty"
    );
    assert_eq!(paths_of(&db, &survivor).len(), 3, "its paths came too");
    for s in [&s1, &s2] {
        assert_eq!(
            db.get_session(&s.id)
                .unwrap()
                .unwrap()
                .project_id
                .as_deref(),
            Some(survivor.as_str()),
            "every Session behind a moved path follows it"
        );
    }
    assert!(
        db.get_workstream("w-1").unwrap().is_some(),
        "a Workstream survives its Project; membership is its path list"
    );
    assert_eq!(
        noending::storage::workstream_projects::project_ids_for_workstream(&db.read(), "w-1")
            .unwrap(),
        vec![survivor.clone()],
        "all associated paths now reach the survivor"
    );
    registry_is_consistent(&db).expect("consistent");
}

#[test]
fn customized_project_name_survives_merge_and_reconcile() {
    let (_d, db) = temp_db();
    let one = ensure(&db, &plain("/legacy/one", true));
    ensure(&db, &plain("/legacy/two", true));
    db.tx(|tx| rename_project_conn(tx, &one.project_id, "我的旧项目").map(|_| ()))
        .unwrap();
    assert!(
        db.get_project(&one.project_id)
            .unwrap()
            .unwrap()
            .name_customized,
        "a rename records that a person chose this name"
    );

    let owner = ensure(
        &db,
        &repo(
            "/elsewhere/repo",
            "/elsewhere/repo/.git",
            GitWorktreeKind::Main,
            &[],
        ),
    );
    let survivor = owner.project_id.clone();
    ensure(
        &db,
        &repo(
            "/legacy/one",
            "/elsewhere/repo/.git",
            GitWorktreeKind::Linked,
            &[],
        ),
    );

    // the survivor ROW is the family owner, but the NAME follows
    // `name_customized`, so a user's title is never lost to an automatic one.
    let after = db.get_project(&survivor).unwrap().unwrap();
    assert_eq!(after.name, "我的旧项目");
    assert!(after.name_customized);

    // From then on nothing automatic renames it — not another merge, not a fresh
    // path that has to create its own Project.
    ensure(
        &db,
        &repo(
            "/legacy/two",
            "/elsewhere/repo/.git",
            GitWorktreeKind::Linked,
            &[],
        ),
    );
    assert_eq!(
        db.get_project(&survivor).unwrap().unwrap().name,
        "我的旧项目"
    );
    let separate = ensure(&db, &plain("/legacy/four", true));
    assert_ne!(separate.project_id, survivor);
    assert_eq!(
        project_of(&db, &separate).name,
        "Four",
        "an automatic name is minted for the new Project, never borrowed"
    );
    registry_is_consistent(&db).expect("consistent");
}

#[test]
fn one_workspace_path_never_belongs_to_two_projects() {
    let (_d, db) = temp_db();
    db.upsert_project(&support::project("p-mine".into(), "Mine"))
        .unwrap();
    db.upsert_project(&support::project("p-other".into(), "Other"))
        .unwrap();

    let id = db
        .tx(|tx| insert_workspace_path_conn(tx, &canon("/shared/path"), "p-mine"))
        .unwrap();
    // The mechanical insert is idempotent on both unique keys and NEVER
    // re-projects an existing row: ownership changes go through the one door that
    // also refreshes the search projections.
    let again = db
        .tx(|tx| insert_workspace_path_conn(tx, &canon("/shared/path/."), "p-other"))
        .unwrap();
    assert_eq!(id, again);
    assert_eq!(
        db.get_workspace_path(&id).unwrap().unwrap().project_id,
        "p-mine",
        "a second caller cannot steal a path by re-inserting it"
    );
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM workspace_paths WHERE canonical_path = ?1",
            &canon("/shared/path")
        ),
        1
    );

    // Reassignment is the one legal move, and it is a move, not a copy; the
    // move itself is covered at the real function in session_workspace_test
    // (`reassign_workspace_path_project_conn`).
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM workspace_paths WHERE canonical_path = ?1",
            &canon("/shared/path")
        ),
        1
    );
}

#[test]
fn project_deletion_requires_explicit_archive() {
    let (_d, db) = temp_db();
    let path = ensure(&db, &plain("/doomed/path", true));
    let doomed = path.project_id.clone();
    assert!(delete_project_permanently(&db, &doomed, &UnrestrictedWorkspace).is_err());
    assert!(db.get_workspace_path(&path.id).unwrap().is_some());
    set_project_archived(&db, &doomed, true).unwrap();
    delete_project_permanently(&db, &doomed, &UnrestrictedWorkspace).unwrap();
    assert!(db.get_workspace_path(&path.id).unwrap().is_none());
    assert!(db.get_project(&doomed).unwrap().is_none());

    // Two Git families are never merged: that is a contradiction in the evidence,
    // and the honest answer is to stop rather than drop one identity.
    let a = ensure(
        &db,
        &repo("/fam/a", "/fam/a/.git", GitWorktreeKind::Main, &[]),
    );
    let b = ensure(
        &db,
        &repo("/fam/b", "/fam/b/.git", GitWorktreeKind::Main, &[]),
    );
    assert!(db
        .tx(|tx| merge_projects_conn(tx, &a.project_id, &b.project_id).map(|_| ()))
        .is_err());
    assert!(db.get_project(&a.project_id).unwrap().is_some());
    assert!(db.get_project(&b.project_id).unwrap().is_some());
    registry_is_consistent(&db).expect("the refused merge changed nothing");

    // And a second owner for one family is refused by the schema, not by
    // convention.
    let taken = db.tx(|tx| {
        tx.execute(
            "UPDATE projects SET git_id = ?2 WHERE id = ?1",
            rusqlite::params![b.project_id, family_of(&db, &a)],
        )?;
        Ok(())
    });
    assert!(taken.is_err(), "one Git family is exactly one Project");
}

#[test]
fn project_archive_is_independent_and_all_sessions_block_permanent_deletion() {
    let (_d, db) = temp_db();
    let path = ensure(&db, &plain("/project/archive", true));
    let s = session(&db, "archive-ref", "/project/archive", &path.id);
    workstream(&db, "normal-task", "Normal");
    workstream(&db, "archived-task", "Archived");
    add_ws_path(&db, "normal-task", &path.id);
    add_ws_path(&db, "archived-task", &path.id);
    noending::workspace::workstream::archive_workstream(&db, "archived-task").unwrap();
    set_project_archived(&db, &path.project_id, true).unwrap();
    assert!(db
        .get_session(&s.id)
        .unwrap()
        .unwrap()
        .archived_at
        .is_none());
    assert_eq!(
        db.get_workstream("normal-task")
            .unwrap()
            .unwrap()
            .visibility,
        "normal"
    );
    assert_eq!(
        db.get_workstream("archived-task")
            .unwrap()
            .unwrap()
            .visibility,
        "archived"
    );
    assert!(delete_project_permanently(&db, &path.project_id, &UnrestrictedWorkspace).is_err());
    noending::lifecycle::archive_session(&db, &s.id).unwrap();
    assert!(delete_project_permanently(&db, &path.project_id, &UnrestrictedWorkspace).is_err());
    assert_eq!(
        noending::storage::workstream_projects::project_ids_for_workstream(
            &db.read(),
            "normal-task"
        )
        .unwrap()
        .len(),
        1,
        "rejection does not detach tasks"
    );
    // Re-observation/upserts must never undo a user's archive decision.
    ensure(&db, &plain("/project/archive", true));
    assert_eq!(
        db.get_project(&path.project_id)
            .unwrap()
            .unwrap()
            .visibility,
        "archived"
    );
    set_project_archived(&db, &path.project_id, false).unwrap();
    assert_eq!(
        db.get_project(&path.project_id)
            .unwrap()
            .unwrap()
            .visibility,
        "normal"
    );
    assert!(db
        .get_session(&s.id)
        .unwrap()
        .unwrap()
        .archived_at
        .is_some());
    assert_eq!(
        db.get_workstream("archived-task")
            .unwrap()
            .unwrap()
            .visibility,
        "archived"
    );
}

#[test]
fn permanent_project_deletion_unlinks_tasks_preserving_other_paths_and_disk() {
    let (_d, db) = temp_db();
    let real = unique_dir("keep-directory");
    let raw = real.to_str().unwrap();
    std::fs::write(real.join("keep.txt"), "keep").unwrap();
    let path = ensure(&db, &plain(raw, true));
    let other = ensure(&db, &plain("/project/other", true));
    for (id, archived) in [("task-active", false), ("task-archived", true)] {
        workstream(&db, id, "Keep task");
        add_ws_path(&db, id, &path.id);
        add_ws_path(&db, id, &other.id);
        if archived {
            noending::workspace::workstream::archive_workstream(&db, id).unwrap();
        }
    }
    db.tx(|tx| {
        tx.execute(
            "INSERT INTO project_context_state (project_id,summary) VALUES (?1,'{\"items\":[{\"id\":\"00000000-0000-4000-8000-000000000001\",\"title\":\"summary\",\"description\":\"\",\"details\":[]}]}')",
            [&path.project_id],
        )?;
        Ok(())
    })
    .unwrap();
    set_project_archived(&db, &path.project_id, true).unwrap();
    delete_project_permanently(&db, &path.project_id, &UnrestrictedWorkspace).unwrap();
    for (id, visibility) in [("task-active", "normal"), ("task-archived", "archived")] {
        assert_eq!(
            db.get_workstream(id).unwrap().unwrap().visibility,
            visibility
        );
        assert_eq!(
            noending::storage::workstream_projects::project_ids_for_workstream(&db.read(), id)
                .unwrap(),
            vec![other.project_id.clone()]
        );
    }
    assert!(db.get_project(&path.project_id).unwrap().is_none());
    assert!(db.get_workspace_path(&path.id).unwrap().is_none());
    assert!(db.get_project(&other.project_id).unwrap().is_some());
    for table in ["project_context_state"] {
        let count: i64 = db
            .read()
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE project_id=?1"),
                [&path.project_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
    assert_eq!(
        std::fs::read_to_string(real.join("keep.txt")).unwrap(),
        "keep"
    );
    registry_is_consistent(&db).unwrap();
    let foreign_keys: i64 = db
        .read()
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(foreign_keys, 0);
    std::fs::remove_dir_all(real).unwrap();
}

#[test]
fn chat_and_assistant_projects_cannot_be_deleted_even_after_git_conversion() {
    let (_d, db) = temp_db();
    let home = Home::new("/app/noending", &[], Some("/app/noending/workspace"));
    for raw in [
        "/app/noending/workspace",
        "/app/noending/assistant",
        &home_path("Documents/Codex/protected"),
    ] {
        let path = ensure_workspace_path(&db, &plain(raw, true), &home).unwrap();
        set_project_archived(&db, &path.project_id, true).unwrap();
        assert!(delete_project_permanently(&db, &path.project_id, &home).is_err());
        let upgraded = ensure_workspace_path(
            &db,
            &repo(raw, &format!("{raw}/.git"), GitWorktreeKind::Main, &[]),
            &home,
        )
        .unwrap();
        assert_eq!(upgraded.project_id, path.project_id);
        assert!(delete_project_permanently(&db, &path.project_id, &home).is_err());
        assert!(db.get_workspace_path(&path.id).unwrap().is_some());
        assert_eq!(
            project_of(&db, &upgraded).visibility,
            "archived",
            "in-place upgrade preserves state"
        );
        let external_raw = format!("/external/{}", path.project_id);
        let external = ensure_workspace_path(
            &db,
            &repo(
                &external_raw,
                &format!("{raw}/.git"),
                GitWorktreeKind::Linked,
                &[],
            ),
            &home,
        )
        .unwrap();
        assert_eq!(external.project_id, upgraded.project_id);
        let returned = ensure_workspace_path(&db, &plain(raw, true), &home).unwrap();
        assert_ne!(
            returned.project_id, upgraded.project_id,
            "other worktree stays in its Git family"
        );
        assert_eq!(
            project_of(&db, &returned).visibility,
            "archived",
            "new split destination inherits source state"
        );
        assert_eq!(project_of(&db, &external).visibility, "archived");
        assert!(delete_project_permanently(&db, &returned.project_id, &home).is_err());
        set_project_archived(&db, &returned.project_id, false).unwrap();
        assert_eq!(project_of(&db, &returned).visibility, "normal");
        assert_eq!(
            project_of(&db, &external).visibility,
            "archived",
            "restoring the split project does not restore its source"
        );
        set_project_archived(&db, &path.project_id, false).unwrap();
        assert_eq!(
            db.get_project(&path.project_id)
                .unwrap()
                .unwrap()
                .visibility,
            "normal"
        );
    }
}

// the WorkspaceAttaching seam

#[test]
fn workspace_attaching_returns_none_for_unresolvable_and_reserved_paths() {
    let (_d, db) = temp_db();
    let observer = Scripted::new();
    observer.set(
        "/work/repo",
        repo("/work/repo", "/work/repo/.git", GitWorktreeKind::Main, &[]),
    );
    let home = Home::new(
        "/Users/tester/.noending",
        &[
            "/Users/tester/.noending/data",
            "/Users/tester/.noending/runtime",
            "/Users/tester/.noending/logs",
        ],
        Some("/Users/tester/.noending/workspace"),
    );
    let projection = ProjectProjection::with_policy(&observer, &home);

    // Empty, whitespace and a relative string resolve to nothing, and nothing is
    // fabricated.
    for raw in ["", "   ", "relative/thing", "~user/other"] {
        let attached = db
            .tx(|tx| projection.ensure_path(tx, raw))
            .expect("no IO in these cases");
        assert_eq!(attached, None, "`{raw}` must not become a WorkspacePath");
    }
    // Reserved app paths never become workspaces, and the default workspace
    // under the same Home does.
    for raw in [
        "/Users/tester/.noending",
        "/Users/tester/.noending/data",
        "/Users/tester/.noending/data/noending.db",
        "/Users/tester/.noending/logs",
    ] {
        let attached = db
            .tx(|tx| projection.ensure_path(tx, raw))
            .unwrap_or_else(|e| panic!("reserved paths are Ok(None), not Err: {e}"));
        assert_eq!(attached, None, "{raw}");
    }
    let sibling = db
        .tx(|tx| projection.ensure_path(tx, "/Users/tester/.noending/datax"))
        .unwrap();
    assert_eq!(
        sibling.as_deref(),
        Some(path_id_of("/Users/tester/.noending/datax").as_str()),
        "a neighbour of a reserved subtree is an ordinary directory (segment-wise)"
    );
    let workspace = db
        .tx(|tx| projection.ensure_path(tx, "/Users/tester/.noending/workspace"))
        .unwrap()
        .expect("the default workspace is a normal path");
    assert_eq!(
        project_of(&db, &db.get_workspace_path(&workspace).unwrap().unwrap()).name,
        "NoEnding Chat",
        "through the same door that created the row"
    );

    // A real workspace through the string door, inside the CALLER's transaction.
    let attached = db
        .tx(|tx| projection.ensure_path(tx, "/work/repo"))
        .unwrap()
        .expect("attached");
    assert_eq!(attached, path_id_of("/work/repo"));
    let row = db.get_workspace_path(&attached).unwrap().unwrap();
    assert_eq!(row.git_state, git_state::DETECTED);
    assert_eq!(
        project_of(&db, &row).git_id,
        db.find_git_identity_by_common_dir(&canon("/work/repo/.git"))
            .unwrap()
            .map(|i| i.id),
        "projects.git_id references git_identities.id, not a path"
    );
    registry_is_consistent(&db).expect("consistent");
}

#[test]
fn workspace_attaching_refuses_an_observation_whose_identity_disagrees() {
    // The one case where the door says "nothing" instead of writing a row it
    // cannot name: `workspace::identity` defines the key, so an observer with a
    // different idea must not become a second authority.
    let (_d, db) = temp_db();
    struct Lying;
    impl WorkspaceObserving for Lying {
        fn observe(&self, raw: &str) -> WorkspaceObservation {
            WorkspaceObservation {
                canonical_path: normalize_path(raw).unwrap(),
                path_id: "path-not-derived-from-that-string".into(),
                exists: true,
                git: GitDetection::None,
            }
        }
    }
    let projection = ProjectProjection::new(&Lying);
    assert_eq!(
        db.tx(|tx| projection.ensure_path(tx, "/work/lying"))
            .unwrap(),
        None
    );
    assert!(db.list_workspace_paths().unwrap().is_empty());
    assert!(
        db.list_projects().unwrap().is_empty(),
        "and no Project either"
    );

    // The same contradiction through the observation-level door is loud, because
    // that caller hands us a fact rather than a string.
    let bad = WorkspaceObservation {
        canonical_path: canon("/work/lying"),
        path_id: "nope".into(),
        exists: true,
        git: GitDetection::None,
    };
    assert!(db
        .tx(|tx| ensure_workspace_path_conn(tx, &bad, &UnrestrictedWorkspace).map(|_| ()))
        .is_err());
    assert!(db.list_workspace_paths().unwrap().is_empty());
}

// Reconcile

#[test]
fn reconcile_a_single_path_reapplies_the_whole_decision_table() {
    let (_d, db) = temp_db();
    let observer = Scripted::new();
    observer.set(
        "/work/repo",
        repo("/work/repo", "/work/repo/.git", GitWorktreeKind::Main, &[]),
    );
    let projection = ProjectProjection::new(&observer);
    let path = ensure(&db, &plain("/work/repo", true));
    assert_eq!(path.git_state, git_state::NONE);

    // Same string, new evidence: reconcile is the only reason a stored path's
    // Project may change without a user action.
    let after = projection
        .reconcile_workspace_path(&db, &path.id)
        .expect("reconcile");
    assert_eq!(after.id, path.id);
    assert_eq!(after.git_state, git_state::DETECTED);
    assert_eq!(after.project_id, path.project_id, "upgrade, not a move");

    // Evidence gone.
    observer.set("/work/repo", plain("/work/repo", true));
    let after = projection.reconcile_workspace_path(&db, &path.id).unwrap();
    assert_eq!(after.git_state, git_state::MISSING);
    assert_eq!(after.project_id, path.project_id, "through reconcile too");

    // An unknown id is an error, not a silent creation.
    assert!(projection
        .reconcile_workspace_path(&db, "path-never-registered")
        .is_err());
}

#[test]
fn reconcile_sweep_keeps_existing_unreferenced_git_worktrees() {
    let (_d, db) = temp_db_locked();
    let root = unique_dir("retained-worktrees");
    let main = root.join("repo").to_string_lossy().into_owned();
    let sibling = root.join("repo-feature").to_string_lossy().into_owned();
    std::fs::create_dir_all(&main).unwrap();
    std::fs::create_dir_all(&sibling).unwrap();
    let observer = Scripted::new();
    observer.set(
        &main,
        repo(
            &main,
            &format!("{main}/.git"),
            GitWorktreeKind::Main,
            &[&main, &sibling],
        ),
    );
    observer.set(&sibling, plain(&sibling, true));
    let projection = ProjectProjection::new(&observer);

    // The seeded directory upgrades to a Git Project and discovers its sibling.
    let seeded = ensure(&db, &plain(&main, true));
    let report = reconcile_workspace_paths(&db, &projection, 500).unwrap();
    assert_eq!(report.scanned, 1);
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(report.discovered_paths, vec![path_id_of(&sibling)]);
    assert_eq!(paths_of(&db, &seeded.project_id).len(), 2);

    // Both paths remain without task/session references; no repeat discoveries.
    let repeated = reconcile_workspace_paths(&db, &projection, 500).unwrap();
    assert!(repeated.discovered_paths.is_empty());
    registry_is_consistent(&db).expect("consistent after the sweep");
}

#[test]
fn reconcile_sweep_preserves_unreferenced_registered_paths() {
    let (_d, db) = temp_db_locked();
    let default = "/app/noending/workspace";
    let agent = home_path("Documents/Codex/chat-gc");
    let home = Home::new("/app/noending", &[], Some(default));
    let observer = Scripted::new();
    let projection = ProjectProjection::with_policy(&observer, &home);
    let raws = ["/aaa/ordinary", default, agent.as_str()];
    let mut expected = Vec::new();
    for raw in raws {
        observer.set(raw, plain(raw, true));
        expected.push(
            ensure_workspace_path(&db, &plain(raw, true), &home)
                .unwrap()
                .id,
        );
    }

    let report = reconcile_workspace_paths(&db, &projection, 500).unwrap();
    assert_eq!(report.scanned, 3);
    let mut retained_ids: Vec<String> = db
        .list_workspace_paths()
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect();
    retained_ids.sort();
    expected.sort();
    assert_eq!(retained_ids, expected);
    assert_eq!(db.list_projects().unwrap().len(), 3);

    // A default-directory session protects its path just like any other session.
    let retained = ensure_workspace_path(&db, &plain(default, true), &home).unwrap();
    let observed_session = session(&db, "default-chat", default, &retained.id);
    let report = reconcile_workspace_paths(&db, &projection, 500).unwrap();
    assert_eq!(report.scanned, 3);
    assert!(db.get_workspace_path(&retained.id).unwrap().is_some());
    assert_eq!(
        db.get_session(&observed_session.id)
            .unwrap()
            .unwrap()
            .project_id,
        Some(retained.project_id)
    );
    registry_is_consistent(&db).expect("consistent");
}

// naming, rename, detail

#[test]
fn default_workspace_project_is_named_noending_chat() {
    let (_d, db) = temp_db();
    let home = Home::new(
        "/Users/me/.noending",
        &["/Users/me/.noending/data"],
        Some("/Users/me/.noending/workspace"),
    );
    let dw =
        ensure_workspace_path(&db, &plain("/Users/me/.noending/workspace", true), &home).unwrap();
    assert_eq!(project_of(&db, &dw).name, "NoEnding Chat");
    let git = ensure_workspace_path(
        &db,
        &repo(
            "/Users/me/.noending/workspace",
            "/Users/me/.noending/workspace/.git",
            GitWorktreeKind::Main,
            &[],
        ),
        &home,
    )
    .unwrap();
    assert_eq!(git.project_id, dw.project_id);
    let plain_again =
        ensure_workspace_path(&db, &plain("/Users/me/.noending/workspace", true), &home).unwrap();
    assert_eq!(plain_again.project_id, dw.project_id);
    assert!(project_of(&db, &plain_again).git_id.is_none());

    // After a Home move the OLD workspace is an ordinary directory and must not
    // keep the special name.
    let old = ensure_workspace_path(&db, &plain("/Users/other/.noending/workspace", true), &home)
        .unwrap();
    assert_eq!(project_of(&db, &old).name, "Workspace");
    assert_eq!(db.list_projects().unwrap().len(), 2);
}

#[test]
fn default_workspace_chat_directories_share_a_project_and_derive_kind_after_rename() {
    let (_d, db) = temp_db();
    let home = Home::new(
        "/app/noending",
        &["/app/noending/data"],
        Some("/app/noending/workspace"),
    );
    // The first observed path may be a conversation subdirectory, not the root.
    let a =
        ensure_workspace_path(&db, &plain("/app/noending/workspace/chat-a", true), &home).unwrap();
    let b =
        ensure_workspace_path(&db, &plain("/app/noending/workspace/chat-b", true), &home).unwrap();
    let root = ensure_workspace_path(&db, &plain("/app/noending/workspace", true), &home).unwrap();
    assert_eq!(a.project_id, b.project_id);
    assert_eq!(a.project_id, root.project_id);
    assert_eq!(project_of(&db, &a).name, "NoEnding Chat");
    let outside = ensure_workspace_path(
        &db,
        &plain("/app/noending/workspace-backup/chat-a", true),
        &home,
    )
    .unwrap();
    assert_ne!(a.project_id, outside.project_id);
    let ordinary = project_detail(&db, &outside.project_id, &home)
        .unwrap()
        .unwrap();
    assert_eq!(ordinary.kind, ProjectKind::SingleDirectory);

    db.tx(|tx| rename_project_conn(tx, &a.project_id, "我的默认聊天"))
        .unwrap();
    let detail = project_detail(&db, &a.project_id, &home).unwrap().unwrap();
    assert_eq!(detail.project.name, "我的默认聊天");
    assert_eq!(detail.kind, ProjectKind::DirectoryGroup);
    assert_eq!(detail.purpose, Some(ProjectPurpose::Chat));
    let c =
        ensure_workspace_path(&db, &plain("/app/noending/workspace/chat-c", true), &home).unwrap();
    assert_eq!(
        c.project_id, a.project_id,
        "renaming does not create a second directory group"
    );
    let cards = noending::commands::project::project_cards(&db, &home).unwrap();
    assert_eq!(
        cards.iter().find(|c| c.id == a.project_id).unwrap().kind,
        detail.kind
    );
    assert_eq!(detail.workspace_paths.len(), 3);
}

#[test]
fn git_in_one_chat_directory_does_not_absorb_other_chats() {
    for (existing_family, root_path) in [(false, false), (true, false), (false, true), (true, true)]
    {
        let (_d, db) = temp_db();
        let home = Home::new("/app/noending", &[], Some("/app/noending/workspace"));
        let path = if root_path {
            "/app/noending/workspace"
        } else {
            "/app/noending/workspace/a"
        };
        let a = ensure_workspace_path(&db, &plain(path, true), &home).unwrap();
        let observed_session = session(&db, "roundtrip", path, &a.id);
        workstream(&db, "roundtrip-task", "目录转换任务");
        add_ws_path(&db, "roundtrip-task", &a.id);
        let b =
            ensure_workspace_path(&db, &plain("/app/noending/workspace/b", true), &home).unwrap();
        assert_eq!(a.project_id, b.project_id);
        set_project_archived(&db, &a.project_id, true).unwrap();
        let existing = existing_family.then(|| {
            ensure_workspace_path(
                &db,
                &repo("/work/main", "/work/main/.git", GitWorktreeKind::Main, &[]),
                &home,
            )
            .unwrap()
        });
        let upgraded = ensure_workspace_path(
            &db,
            &repo(path, "/work/main/.git", GitWorktreeKind::Linked, &[]),
            &home,
        )
        .unwrap();
        assert_ne!(upgraded.project_id, b.project_id);
        if let Some(existing) = existing {
            assert_eq!(upgraded.project_id, existing.project_id);
        }
        assert_eq!(
            db.get_workspace_path(&b.id).unwrap().unwrap().project_id,
            b.project_id
        );
        let chat = project_detail(&db, &b.project_id, &home).unwrap().unwrap();
        let git = project_detail(&db, &upgraded.project_id, &home)
            .unwrap()
            .unwrap();
        assert_eq!(chat.kind, ProjectKind::DirectoryGroup);
        assert_eq!(chat.workspace_paths.len(), 1);
        assert_eq!(chat.project.visibility, "archived");
        assert_eq!(git.kind, ProjectKind::Git);
        assert_eq!(
            git.project.visibility,
            if existing_family {
                "normal"
            } else {
                "archived"
            }
        );
        assert_eq!(git.purpose, Some(ProjectPurpose::Chat));
        let mut unavailable = plain(path, true);
        unavailable.git = GitDetection::Unavailable;
        assert_eq!(
            ensure_workspace_path(&db, &unavailable, &home)
                .unwrap()
                .project_id,
            upgraded.project_id
        );
        assert_eq!(
            ensure_workspace_path(&db, &plain(path, false), &home)
                .unwrap()
                .project_id,
            upgraded.project_id
        );
        let returned = ensure_workspace_path(&db, &plain(path, true), &home).unwrap();
        let group = db.get_workspace_path(&b.id).unwrap().unwrap();
        assert_eq!(returned.id, a.id);
        assert_eq!(returned.project_id, group.project_id);
        assert_eq!(returned.git_state, git_state::NONE);
        let detail = project_detail(&db, &returned.project_id, &home)
            .unwrap()
            .unwrap();
        assert_eq!(detail.kind, ProjectKind::DirectoryGroup);
        assert_eq!(
            detail.project.visibility, "archived",
            "existing destination keeps its state"
        );
        assert_eq!(detail.purpose, Some(ProjectPurpose::Chat));
        assert!(detail.sessions.iter().any(|s| s.id == observed_session.id));
        assert!(detail
            .workstreams
            .iter()
            .any(|w| w.workstream.id == "roundtrip-task"));
        if existing_family {
            let external = db
                .get_workspace_path(&path_identity("/work/main"))
                .unwrap()
                .unwrap();
            assert_eq!(external.project_id, upgraded.project_id);
            assert!(project_of(&db, &external).git_id.is_some());
        }
        let again = ensure_workspace_path(
            &db,
            &repo(path, "/work/main/.git", GitWorktreeKind::Linked, &[]),
            &home,
        )
        .unwrap();
        assert_eq!(again.id, a.id);
        assert_ne!(
            again.project_id,
            db.get_workspace_path(&b.id).unwrap().unwrap().project_id
        );
        assert_eq!(
            db.get_session(&observed_session.id)
                .unwrap()
                .unwrap()
                .project_id,
            Some(again.project_id)
        );
        registry_is_consistent(&db).unwrap();
    }
}

#[test]
fn all_agent_chat_roots_are_classified_without_using_project_names() {
    let (_d, db) = temp_db();
    for rel in [
        "Documents/Qoder/chat-a",
        "Workbuddy/chat-b",
        "Documents/Codex/chat-c",
    ] {
        let path = ensure(&db, &plain(&home_path(rel), true));
        db.tx(|tx| rename_project_conn(tx, &path.project_id, "用户改过的名字"))
            .unwrap();
        let detail = project_detail(&db, &path.project_id, &UnrestrictedWorkspace)
            .unwrap()
            .unwrap();
        assert_eq!(detail.kind, ProjectKind::DirectoryGroup);
    }
}

#[test]
fn user_rename_is_customized_and_automatic_naming_stops() {
    let (_d, db) = temp_db();
    let path = ensure(&db, &plain("/work/alpha", true));
    let project_id = path.project_id.clone();

    let renamed = db
        .tx(|tx| rename_project_conn(tx, &project_id, "  重要的项目  "))
        .unwrap();
    assert_eq!(renamed.name, "重要的项目", "trimmed, not rewritten");
    assert!(renamed.name_customized);
    assert_eq!(
        renamed.git_id, None,
        "a rename cannot touch family identity"
    );

    // The whole-object write Project creation uses must not rename a customized
    // row either — `upsert_project_conn`'s `CASE` branch.
    let mut lying = renamed.clone();
    lying.name = "Auto Renamed".into();
    lying.name_customized = false;
    db.upsert_project(&lying).unwrap();
    let stored = db.get_project(&project_id).unwrap().unwrap();
    assert_eq!(stored.name, "重要的项目");
    assert!(stored.name_customized, "name_customized is one-way");

    // And the whole-object write's other half: an automatic write that carries
    // no family must never CLEAR one. (The reverse direction — a
    // caller-supplied non-null `git_id` replacing a family — is why
    // `update_project` leaves the command surface.)
    ensure(
        &db,
        &repo(
            "/work/alpha",
            "/work/alpha/.git",
            GitWorktreeKind::Main,
            &[],
        ),
    );
    let with_family = db.get_project(&project_id).unwrap().unwrap();
    assert!(with_family.git_id.is_some(), "the Git upgrade did land");
    let mut cleared = with_family.clone();
    cleared.git_id = None;
    cleared.name = "Auto Renamed".into();
    db.upsert_project(&cleared).unwrap();
    let after = db.get_project(&project_id).unwrap().unwrap();
    assert_eq!(after.git_id, with_family.git_id, "never cleared");
    assert_eq!(after.name, "重要的项目");

    // Reconciling the same path again is still not a rename.
    ensure(&db, &plain("/work/alpha", true));
    assert_eq!(
        db.get_project(&project_id).unwrap().unwrap().name,
        "重要的项目"
    );
}

#[test]
fn project_detail_has_the_frozen_shape() {
    let (_d, db) = temp_db();
    let path = ensure(
        &db,
        &repo("/work/repo", "/work/repo/.git", GitWorktreeKind::Main, &[]),
    );
    let sibling = ensure(&db, &plain("/work/repo/docs", true));
    assert_ne!(
        path.project_id, sibling.project_id,
        "a plain directory inside a repository is still its own Project until Git says otherwise"
    );
    let s_det = session(&db, "s-det", "/work/repo", &path.id);
    workstream(&db, "w-primary", "primary here");
    workstream(&db, "w-related", "related here");
    add_ws_path(&db, "w-primary", &path.id);
    add_ws_path(&db, "w-related", &sibling.id);
    add_ws_path(&db, "w-related", &path.id);

    let detail = project_detail(&db, &path.project_id, &UnrestrictedWorkspace)
        .unwrap()
        .expect("detail");
    assert_eq!(detail.project.id, path.project_id);
    assert_eq!(
        detail
            .workspace_paths
            .iter()
            .map(|p| p.id.clone())
            .collect::<Vec<_>>(),
        vec![path.id.clone()],
        "only the paths the Project actually owns"
    );
    assert_eq!(
        detail
            .sessions
            .iter()
            .map(|s| s.id.clone())
            .collect::<Vec<_>>(),
        vec![s_det.id.clone()]
    );
    assert_eq!(
        detail
            .workstreams
            .iter()
            .map(|w| w.workstream.id.clone())
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from(["w-primary".to_string(), "w-related".to_string()]),
        "every associated path counts equally"
    );
    assert_eq!(
        noending::storage::workstream_projects::workstreams_for_project(
            &db.read(),
            &sibling.project_id
        )
        .unwrap()
        .into_iter()
        .map(|w| w.id)
        .collect::<Vec<_>>(),
        vec!["w-related".to_string()]
    );

    // The shape is a contract with the frontend bridge: these keys, nothing else.
    // (`serde_json::Value` orders map keys, so this asserts the SET: the wire
    // object's field order is not part of the contract.)
    let value = serde_json::to_value(&detail).unwrap();
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.as_str())
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "kind",
            "project",
            "purpose",
            "remote_url",
            "sessions",
            "workspace_paths",
            "workstreams"
        ]
    );
    let mut workstream_keys: Vec<&str> = value["workstreams"][0]
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.as_str())
        .collect();
    workstream_keys.sort_unstable();
    assert_eq!(workstream_keys, vec!["workstream"]);

    // A Project that does not exist is `None`, and the command maps that to an
    // error rather than an empty object.
    assert!(project_detail(&db, "p-never", &UnrestrictedWorkspace)
        .unwrap()
        .is_none());
    registry_is_consistent(&db).expect("consistent");
}

#[test]
fn list_projects_reports_every_derived_project_and_no_ghost() {
    let (_d, db) = temp_db();
    let a = ensure(&db, &plain("/work/a", true));
    let b = ensure(&db, &plain("/work/b", true));
    let mut ids: Vec<String> = db
        .list_projects()
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect();
    ids.sort();
    let mut expected = vec![a.project_id.clone(), b.project_id.clone()];
    expected.sort();
    assert_eq!(ids, expected);

    // A Project remains listed while it still owns a WorkspacePath.
    assert_eq!(db.list_projects().unwrap().len(), 2);

    // Explicitly archived projects remain listed until permanent deletion.
    set_project_archived(&db, &b.project_id, true).unwrap();
    assert_eq!(db.list_projects().unwrap().len(), 2);
    delete_project_permanently(&db, &b.project_id, &UnrestrictedWorkspace).unwrap();
    let rest: Vec<String> = db
        .list_projects()
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(rest, vec![a.project_id.clone()]);
}

// The Windows case-alias claim can only be proven where the host's own rule
// applies: `ensure_workspace_path_conn` re-derives the id through the host, so
// these tests are `#[cfg(windows)]` and run on CI's windows-latest job only.

/// `git_identities` is found by location, not by spelling: two spellings of one
/// `common_dir` must return the id created first, or a fresh row is inserted and a
/// second family can move a WorkspacePath into a second Project.
///
/// Separator-only (not case) because on a Unix runner case is not part of the
/// location relation; that half is what the `#[cfg(windows)]` tests below carry.
#[test]
fn one_git_family_is_found_before_it_is_created() {
    let (_d, db) = temp_db();
    let first =
        noending::storage::workspace::ensure_git_identity_conn(&db.write(), "C:/Code/Repo/.git")
            .expect("first family");
    let again =
        noending::storage::workspace::ensure_git_identity_conn(&db.write(), "C:/Code/Repo/.git/")
            .expect("same family, trailing separator");

    assert_eq!(
        first, again,
        "the location relation has to answer before a row is created"
    );
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM git_identities WHERE id = ?1",
            &first
        ),
        1
    );
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM git_identities WHERE common_dir LIKE ?1",
            "C:%"
        ),
        1,
        "one row, not two spellings of it"
    );
}

/// two case spellings of one directory are one WorkspacePath, and the row
/// keeps the spelling that arrived first — the fold reaches the key, never the
/// display.
#[cfg(windows)]
#[test]
fn windows_case_aliases_cannot_create_two_workspace_paths() {
    let (_d, db) = temp_db();
    let upper = ensure(&db, &plain("C:\\Code\\NoEnding", true));
    let lower = ensure(&db, &plain("c:\\code\\noending", true));

    assert_eq!(upper.id, lower.id, "one directory, one key");
    assert_eq!(upper.project_id, lower.project_id);
    assert_eq!(paths_of(&db, &upper.project_id).len(), 1);
    assert_eq!(db.list_workspace_paths().unwrap().len(), 1);
    let stored = db
        .get_workspace_path(&upper.id)
        .unwrap()
        .expect("the row exists");
    assert_eq!(
        stored.canonical_path,
        canon("C:\\Code\\NoEnding"),
        "the first spelling survives; nothing lower-cased the stored row"
    );
    registry_is_consistent(&db).expect("every id derived from its own canonical_path");
}

/// 's harder half: one repository reported under two spellings is one
/// family. Without the location-keyed `ensure_git_identity_conn`, the second
/// observation mints a second `git_identities` row, reads that as a family
/// change and moves the path into a second Project — splitting what exists to
/// converge. Falsified by restoring the exact-string-only lookup.
#[cfg(windows)]
#[test]
fn windows_case_aliases_cannot_create_two_projects() {
    let (_d, db) = temp_db();
    let upper = ensure(
        &db,
        &repo(
            "C:\\Code\\Repo",
            "C:\\Code\\Repo\\.git",
            GitWorktreeKind::Main,
            &[],
        ),
    );
    let lower = ensure(
        &db,
        &repo(
            "c:\\code\\repo",
            "c:\\code\\repo\\.git",
            GitWorktreeKind::Main,
            &[],
        ),
    );

    assert_eq!(
        upper.project_id, lower.project_id,
        "one family, one Project"
    );
    assert_eq!(db.list_projects().unwrap().len(), 1);
    assert_eq!(db.list_workspace_paths().unwrap().len(), 1);
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM git_identities WHERE common_dir LIKE ?1",
            "%"
        ),
        1,
        "the git family keyed by location, not by spelling"
    );
    assert_eq!(project_of(&db, &lower).git_id, Some(family_of(&db, &upper)));
    registry_is_consistent(&db).expect("consistent after the case alias merged");
}

// Workspace refresh contract.
// The two refresh entries (global / per-project) ride ONE rule set: the
// reconcile_workspace_path_ids primitive below is the same observe → ensure →
// observation pipeline the global sweep runs. These tests lock that contract without
// any real git or real Home.

use noending::workspace::project::{
    reconcile_workspace_path_ids, reconcile_workspace_paths_with_progress,
};

#[test]
fn global_refresh_reobserves_registered_paths() {
    let (_d, db) = temp_db_locked();
    let observer = Scripted::new();
    observer.set("/work/a", plain("/work/a", true));
    observer.set("/work/b", plain("/work/b", true));
    let projection = ProjectProjection::new(&observer);
    {
        ensure(&db, &plain("/work/a", true));
        let b = ensure(&db, &plain("/work/b", true));
        // 会话关联应在目录重新观察后保留。
        session(&db, "s-b", "/work/b", &b.id);
    }
    // /work/b 在两次刷新之间从磁盘上消失了。
    observer.set("/work/b", plain("/work/b", false));

    let progress: std::sync::Arc<Mutex<Vec<(usize, usize)>>> =
        std::sync::Arc::new(Mutex::new(Vec::new()));
    let sink = progress.clone();
    let report =
        reconcile_workspace_paths_with_progress(&db, &projection, 500, &move |scanned, total| {
            sink.lock().unwrap().push((scanned, total));
        })
        .unwrap();

    assert_eq!(report.scanned, 2);
    assert_eq!(report.missing_paths, 1, "only /work/b is gone");
    assert_eq!(progress.lock().unwrap().last(), Some(&(2, 2)), "progress");
    let b = db
        .get_workspace_path(&path_id_of("/work/b"))
        .unwrap()
        .expect("missing registered paths are retained");
    assert!(!b.exists, "the observation is committed to the registry");
}

#[test]
fn project_refresh_does_not_scan_unrelated_projects() {
    let (_d, db) = temp_db_locked();
    let observer = Scripted::new();
    observer.set("/work/a", plain("/work/a", true));
    // Scripted 的默认答案是"存在"；若 B 被扫到，它的自定义答案才会生效。
    observer.set("/work/b", plain("/work/b", false));
    let projection = ProjectProjection::new(&observer);
    {
        ensure(&db, &plain("/work/a", true));
        ensure(&db, &plain("/work/b", true));
    }

    // 只定点刷新 A 的路径。
    let report =
        reconcile_workspace_path_ids(&db, &projection, &[path_id_of("/work/a")], &|_, _| {})
            .unwrap();
    assert_eq!(report.scanned, 1, "exactly one path was observed");

    let b = db
        .get_workspace_path(&path_id_of("/work/b"))
        .unwrap()
        .expect("b registered");
    assert!(b.exists, "unrelated project's path was never re-observed");
}

#[test]
fn missing_unreferenced_directory_and_project_survive_reconciliation() {
    let (_d, db) = temp_db_locked();
    let observer = Scripted::new();
    observer.set("/work/a", plain("/work/a", false));
    let projection = ProjectProjection::new(&observer);
    let (wp, project_id) = {
        let wp = ensure(&db, &plain("/work/a", true));
        let project_id = wp.project_id.clone();
        (wp, project_id)
    };

    let report =
        reconcile_workspace_path_ids(&db, &projection, &[wp.id.clone()], &|_, _| {}).unwrap();
    assert_eq!(report.missing_paths, 1);
    assert!(!db.get_workspace_path(&wp.id).unwrap().unwrap().exists);
    assert!(db.get_project(&project_id).unwrap().is_some());
    registry_is_consistent(&db).expect("registry stays consistent");
}

#[test]
fn restored_directory_becomes_present_again() {
    let (_d, db) = temp_db_locked();
    let observer = Scripted::new();
    observer.set("/work/a", plain("/work/a", false));
    let projection = ProjectProjection::new(&observer);
    let wp = {
        let wp = ensure(&db, &plain("/work/a", true));
        // 移动盘重新挂载的故事：路径有引用（比如 Session），所以两轮都存活。
        session(&db, "s-a", "/work/a", &wp.id);
        wp
    };

    let gone =
        reconcile_workspace_path_ids(&db, &projection, &[wp.id.clone()], &|_, _| {}).unwrap();
    assert_eq!(gone.missing_paths, 1);
    let still_there = { db.get_workspace_path(&wp.id).unwrap() };
    assert!(!still_there.expect("row survives").exists);

    // 目录回来了（从备份恢复、移动盘重新挂载……）：下一次刷新把它翻回来。
    observer.set("/work/a", plain("/work/a", true));
    let back =
        reconcile_workspace_path_ids(&db, &projection, &[wp.id.clone()], &|_, _| {}).unwrap();
    assert_eq!(back.missing_paths, 0);
    let present = { db.get_workspace_path(&wp.id).unwrap() };
    assert!(present.expect("row survives").exists);
}

#[test]
fn refresh_preserves_explicit_task_projects() {
    let (_d, db) = temp_db_locked();
    let observer = Scripted::new();
    observer.set("/work/a", plain("/work/a", true));
    let projection = ProjectProjection::new(&observer);
    let wp = {
        let wp = ensure(&db, &plain("/work/a", true));
        workstream(&db, "ws-1", "WS");
        add_ws_path(&db, "ws-1", &wp.id);
        wp
    };
    let before = count_rows(
        &db,
        "SELECT COUNT(*) FROM workstream_projects WHERE project_id = ?",
        &wp.project_id,
    );
    assert_eq!(before, 1);

    reconcile_workspace_path_ids(&db, &projection, &[wp.id.clone()], &|_, _| {}).unwrap();
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM workstream_projects WHERE project_id = ?",
            &wp.project_id
        ),
        before,
        "refresh never changes explicit task associations"
    );
}

#[test]
fn refresh_does_not_touch_session_history() {
    let (_d, db) = temp_db_locked();
    let observer = Scripted::new();
    observer.set("/work/a", plain("/work/a", false));
    let projection = ProjectProjection::new(&observer);
    let (wp, s_hist) = {
        let wp = ensure(&db, &plain("/work/a", true));
        let s_hist = session(&db, "s-hist", "/work/a", &wp.id);
        (wp, s_hist)
    };
    let history = db
        .commit_ingest(
            &s_hist.id,
            &[support::parsed_message(
                "history",
                noending::domain::SessionMessageRole::User,
                "preserved history",
            )],
            &support::seed_source(0),
        )
        .unwrap();
    let before = count_rows(
        &db,
        "SELECT COUNT(*) FROM sessions WHERE workspace_path_id = ?",
        &wp.id,
    );
    assert_eq!(before, 1);

    reconcile_workspace_path_ids(&db, &projection, &[wp.id.clone()], &|_, _| {}).unwrap();
    // 目录消失只会让路径标记 missing；Session 历史一行不动。
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM sessions WHERE workspace_path_id = ?",
            &wp.id
        ),
        before
    );
    let s = { db.get_session(&s_hist.id).unwrap().expect("session intact") };
    assert!(s.archived_at.is_none());
    assert_eq!(s.workspace_path_id.as_deref(), Some(wp.id.as_str()));
    let after = db.get_messages(&s_hist.id, None, 10).unwrap();
    assert_eq!(
        after
            .iter()
            .map(|m| (&m.id, &m.content))
            .collect::<Vec<_>>(),
        history
            .iter()
            .map(|m| (&m.id, &m.content))
            .collect::<Vec<_>>()
    );
}

#[test]
fn refresh_does_not_ingest_sessions() {
    let (dir, db) = temp_db_locked();
    let source = dir.join("external-source");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("rollout-2026-10-09-external.jsonl"),
        concat!(r#"{"ordinal":0,"type":"session_meta","payload":{"id":"external-root","cwd":"/work/a"}}"#, "\n",
        r#"{"ordinal":1,"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"external transcript"}]}}"#, "\n")).unwrap();
    db.add_ingest_source(Agent::Codex, &source.to_string_lossy())
        .unwrap();
    assert_eq!(
        noending::adapters::adapter_for(Agent::Codex)
            .discover_members_in(&[source], &|_| false)
            .unwrap()
            .len(),
        1,
        "fixture must be discoverable if ingestion were accidentally invoked"
    );
    let observer = Scripted::new();
    observer.set("/work/a", plain("/work/a", true));
    let projection = ProjectProjection::new(&observer);
    let wp = { ensure(&db, &plain("/work/a", true)) };
    let before = {
        db.read()
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get::<_, i64>(0))
            .unwrap()
    };

    reconcile_workspace_path_ids(&db, &projection, &[wp.id.clone()], &|_, _| {}).unwrap();
    let after = {
        db.read()
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get::<_, i64>(0))
            .unwrap()
    };
    assert_eq!(
        before, after,
        "workspace refresh never runs session ingestion"
    );
}

/// The remotes an observation carries land in the git identity's metadata and
/// surface on the Project detail — `origin` preferred over other remotes.
/// URLs arrive credential-stripped from the resolver; the fixture feeds
/// already-stripped ones.
#[test]
fn a_project_detail_surfaces_the_repository_remote() {
    let (_dir, db) = temp_db();
    let canonical = canon("/repo");
    let observation = WorkspaceObservation {
        path_id: path_identity(&canonical),
        exists: true,
        canonical_path: canonical.clone(),
        git: GitDetection::Detected {
            common_dir: canon("/repo/.git"),
            toplevel: Some(canonical),
            kind: GitWorktreeKind::Main,
            worktrees: vec![canon("/repo")],
            remotes: vec![
                (
                    "upstream".to_string(),
                    "https://example.com/up/repo.git".to_string(),
                ),
                (
                    "origin".to_string(),
                    "https://example.com/me/repo.git".to_string(),
                ),
            ],
        },
    };
    let path = ensure_workspace_path(&db, &observation, &UnrestrictedWorkspace).expect("ensure");
    let project = db.get_project(&path.project_id).unwrap().expect("project");

    let detail = project_detail(&db, &project.id, &UnrestrictedWorkspace)
        .unwrap()
        .expect("detail");
    assert_eq!(
        detail.remote_url.as_deref(),
        Some("https://example.com/me/repo.git"),
        "origin wins over the other remotes"
    );

    // A refresh where origin disappears: the fallback takes over, wholesale.
    let git = match &observation.git {
        GitDetection::Detected {
            common_dir,
            toplevel,
            kind,
            worktrees,
            ..
        } => GitDetection::Detected {
            common_dir: common_dir.clone(),
            toplevel: toplevel.clone(),
            kind: kind.clone(),
            worktrees: worktrees.clone(),
            remotes: vec![(
                "upstream".to_string(),
                "https://example.com/up/repo.git".to_string(),
            )],
        },
        other => panic!("{other:?}"),
    };
    let moved = WorkspaceObservation { git, ..observation };
    ensure_workspace_path(&db, &moved, &UnrestrictedWorkspace).expect("re-ensure");
    let detail = project_detail(&db, &project.id, &UnrestrictedWorkspace)
        .unwrap()
        .expect("detail");
    assert_eq!(
        detail.remote_url.as_deref(),
        Some("https://example.com/up/repo.git"),
        "a removed origin falls back to the first remaining remote"
    );
}

#[test]
fn project_workspace_paths_order_main_worktree_first() {
    let (_d, db) = temp_db();
    let main_obs = repo("/work/z_main", "/work/.git", GitWorktreeKind::Main, &[]);
    let linked_obs = repo("/work/a_linked", "/work/.git", GitWorktreeKind::Linked, &[]);

    let p_main = ensure(&db, &main_obs);
    let p_linked = ensure(&db, &linked_obs);
    assert_eq!(p_main.project_id, p_linked.project_id);

    let paths = db
        .list_workspace_paths_for_project(&p_main.project_id)
        .unwrap();
    assert_eq!(paths.len(), 2);
    // Despite "/work/a_linked" being lexicographically smaller, the main worktree is first
    assert_eq!(paths[0].git_kind.as_deref(), Some("main"));
    assert_eq!(paths[0].canonical_path, canon("/work/z_main"));
    assert_eq!(paths[1].git_kind.as_deref(), Some("linked"));
    assert_eq!(paths[1].canonical_path, canon("/work/a_linked"));
}
