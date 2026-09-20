//! Project projection and the WorkspacePath registry (方案 §17).
//!
//! Every §17 "必须测试" case is exactly one named test here, plus the extra doors
//! the registry owns: the `WorkspaceAttaching` implementation, the reconcile
//! sweep, GC, automatic naming and the §11 detail shape.
//!
//! No test touches a real repository, a real NoEnding Home or `git`: observations
//! are constructed by script (§42.3-M13), which is the whole point of the
//! `WorkspaceObserving` / `WorkspacePolicy` seams. Paths are absolute strings that
//! need not exist — path identity is lexical (§42.3-M8) — and every assertion
//! compares against `canon(..)` rather than a spelled-out prefix, because
//! `canonical_path` is stored in platform-native form and CI runs both macOS and
//! Windows.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use noending::domain::{
    git_state, workstream_path_source, Agent, GitDetection, GitWorktreeKind, Project, Session,
    WorkspaceObservation, WorkspacePath, Workstream,
};
use noending::storage::workspace::{
    delete_zero_path_project_conn, insert_workspace_path_conn, rename_project_conn,
};
use noending::storage::workstream_paths::append_workstream_path_conn;
use noending::storage::Db;
use noending::workspace::project::{
    apply_projection_effect, ensure_workspace_path, ensure_workspace_path_conn,
    gc_gone_workspace_paths, merge_projects_conn, project_detail, reassign_workspace_path_conn,
    reconcile_workspace_paths, registry_is_consistent, ProjectProjection, ProjectionEffect,
    UnrestrictedWorkspace, WorkspacePolicy,
};
use noending::workspace::resolver::WorkspaceObserving;
use noending::workspace::{is_within, normalize_path, path_identity, WorkspaceAttaching};

// --------------------------------------------------------------------------
// fixtures
// --------------------------------------------------------------------------

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
/// assertion compares against this rather than a literal (§42.3-M8 rule 7).
fn canon(raw: &str) -> String {
    normalize_path(raw).expect("test paths are absolute")
}

fn path_id_of(raw: &str) -> String {
    path_identity(&canon(raw))
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

/// Register a path under an existing Project the way the v12 migration does, so a
/// test can build the multi-path Project shape §8.2 itself never produces.
fn insert_plain(db: &Db, raw: &str, project_id: &str) -> WorkspacePath {
    let id = db
        .tx(|tx| insert_workspace_path_conn(tx, &canon(raw), project_id))
        .unwrap();
    db.get_workspace_path(&id).unwrap().expect("inserted")
}

fn project_of(db: &Db, path: &WorkspacePath) -> Project {
    db.get_project(&path.project_id)
        .unwrap()
        .expect("a WorkspacePath always belongs to exactly one Project (§1.1)")
}

/// The Git family of the Project owning `path`.
fn family_of(db: &Db, path: &WorkspacePath) -> String {
    project_of(db, path).git_id.expect("git-backed Project")
}

fn paths_of(db: &Db, project_id: &str) -> Vec<WorkspacePath> {
    db.list_workspace_paths_for_project(project_id).unwrap()
}

fn session(db: &Db, id: &str, cwd: &str, path_id: &str) {
    let mut s = Session::new(
        id.into(),
        Agent::Codex,
        format!("src-{id}"),
        format!("/raw/{id}.jsonl"),
    );
    s.cwd = Some(canon(cwd));
    s.workspace_path_id = Some(path_id.into());
    db.upsert_session(&s).unwrap();
}

fn workstream(db: &Db, id: &str, title: &str) {
    db.upsert_workstream(&Workstream::new(id.into(), title))
        .unwrap();
}

fn add_ws_path(db: &Db, workstream_id: &str, workspace_path_id: &str) {
    db.tx(|tx| {
        append_workstream_path_conn(
            tx,
            workstream_id,
            workspace_path_id,
            workstream_path_source::USER,
        )
    })
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
/// default workspace. `workspace/` is deliberately NOT reserved (§2, §42.3-M21) —
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
        // silently admit every reserved path there (§42.3-M8).
        canonical_path == self.root || self.reserved.iter().any(|r| is_within(canonical_path, r))
    }

    fn default_workspace(&self) -> Option<String> {
        self.default_workspace.clone()
    }

    fn exists_on_disk(&self, canonical_path: &str) -> bool {
        // Same rule the production policy uses: existence is read from the real
        // host, never assumed. Tests that hand in a non-existent path therefore
        // get `false`, which is what §9 adoption must record.
        noending::workspace::resolver::exists_on_disk(canonical_path)
    }
}

// --------------------------------------------------------------------------
// §8.1-8.3 — the door
// --------------------------------------------------------------------------

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
    assert_eq!(
        projects.len(),
        1,
        "§8.2: one new plain directory, one Project"
    );
    assert_eq!(projects[0].id, path.project_id);
    assert_eq!(projects[0].name, "Alpha", "§37 automatic naming");
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
    // not become a second WorkspacePath (§42.3-M8).
    let second = ensure(&db, &plain("/work/alpha/./", true));

    assert_eq!(first.id, second.id);
    assert_eq!(first.project_id, second.project_id);
    assert_eq!(db.list_workspace_paths().unwrap().len(), 1);
    assert_eq!(db.list_projects().unwrap().len(), 1);

    // Replaying the whole run changes nothing a caller can observe except
    // `last_seen_at`, which is what makes a retried Sync or a replayed migration
    // converge instead of oscillate (§42.2-E1).
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
    let git_id = project
        .git_id
        .clone()
        .expect("§8.3: the family is recorded");

    // The family is a `git_identities` row keyed by the common dir (§5.2).
    let identity = db
        .find_git_identity_by_common_dir(&canon("/work/repo/.git"))
        .unwrap()
        .expect("git identity persisted");
    assert_eq!(identity.id, git_id);
    assert_ne!(
        identity.id,
        path_id_of("/work/repo"),
        "a Git id is app-assigned, not a second copy of the path hash (design §8)"
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

/// §9 + §1.3 — an adopted worktree's existence is observed, never assumed.
///
/// `git worktree list` reports registrations whether or not the directory is
/// still on this machine. Adoption used to hardcode `false`, so the Projects
/// page claimed "目录不存在" about worktrees that were sitting right there until
/// some later sweep corrected the row. Both answers must come from the disk.
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
    assert_eq!(family.len(), 3, "§9: the list becomes WorkspacePaths");

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
        !by_path(&ghost).exists,
        "a registration whose directory is gone stays a legal exists=false \
         observation (§42.3-M8), it is not silently dropped or asserted present"
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
        "§8.3: one common dir means one Project"
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

// --------------------------------------------------------------------------
// §8.4 / §8.5 / §1.3 — upgrades, moves, and evidence that disappears
// --------------------------------------------------------------------------

#[test]
fn path_project_upgrades_to_git_project_without_changing_project_id() {
    let (_d, db) = temp_db();
    let path = ensure(&db, &plain("/work/repo", true));
    let project = project_of(&db, &path);
    assert_eq!(project.git_id, None);
    session(&db, "s-up", "/work/repo", &path.id);

    // `git init` ran. The Project adopts the family; the row it owns does not move.
    let after = ensure(
        &db,
        &repo("/work/repo", "/work/repo/.git", GitWorktreeKind::Main, &[]),
    );
    let upgraded = project_of(&db, &after);

    assert_eq!(upgraded.id, project.id, "§8.4: Project.id is unchanged");
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
        db.get_session("s-up")
            .unwrap()
            .unwrap()
            .project_id
            .as_deref(),
        Some(project.id.as_str()),
        "the derived cache still agrees with the path"
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
    session(&db, "s-keep", "/work/repo", &path.id);

    // The `.git` directory vanished. §1.3 allows exactly one change: `git_state`.
    let missing = ensure(&db, &plain("/work/repo", true));
    assert_eq!(missing.git_state, git_state::MISSING, "§8.1");
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
    // `Unavailable` (no git binary, timeout, dubious ownership) likewise (§M9).
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
        db.get_session("s-keep")
            .unwrap()
            .unwrap()
            .project_id
            .as_deref(),
        Some(project.id.as_str())
    );
    registry_is_consistent(&db).expect("consistent");
}

#[test]
fn different_git_family_moves_the_path_and_old_project_dies_if_empty() {
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
    session(&db, "s-a", "/work/one", &a.id);
    session(&db, "s-b", "/work/two", &b.id);

    // `/work/one` now sits inside a DIFFERENT repository. §8.5: a strong identity
    // change — this path moves, its sibling keeps its own family.
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
        db.get_session("s-a")
            .unwrap()
            .unwrap()
            .project_id
            .as_deref(),
        Some(moved.project_id.as_str()),
        "§1.11: the derived cache followed its path in the same transaction"
    );
    assert_eq!(
        db.get_session("s-b")
            .unwrap()
            .unwrap()
            .project_id
            .as_deref(),
        Some(b.project_id.as_str()),
        "a Session whose path did not move is never dragged along"
    );

    // When the remaining path reports G2 too, the old Project loses its last path
    // and is deleted.
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
    assert!(db.get_project(&a.project_id).unwrap().is_none());
    assert_eq!(db.list_projects().unwrap().len(), 1);
    assert_eq!(paths_of(&db, &moved.project_id).len(), 2);
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM sessions WHERE project_id = ?1",
            &a.project_id
        ),
        0,
        "nothing still caches a Project that is gone"
    );
    registry_is_consistent(&db).expect("§1.2 holds at every committed moment");
}

#[test]
fn merge_deletes_the_zero_path_project() {
    let (_d, db) = temp_db();
    // A path-backed Project holding two plain directories. §8.2 gives every new
    // path its own Project, so this shape only arrives from the v12 migration (a
    // legacy Project that had several cwds) — which is exactly the case that has
    // to merge cleanly. `insert_workspace_path_conn` is what
    // `ensure_migration_workspace_path` uses for the same purpose.
    let legacy = "p-legacy".to_string();
    db.upsert_project(&Project::new(legacy.clone(), "Legacy"))
        .unwrap();
    let one = insert_plain(&db, "/legacy/one", &legacy);
    let two = insert_plain(&db, "/legacy/two", &legacy);
    assert_eq!(one.project_id, two.project_id);
    let merged_away = one.project_id.clone();
    session(&db, "s-1", "/legacy/one", &one.id);
    session(&db, "s-2", "/legacy/two", &two.id);
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
        "§8.4: the merged Project is deleted, not left empty"
    );
    assert_eq!(paths_of(&db, &survivor).len(), 3, "its paths came too");
    for id in ["s-1", "s-2"] {
        assert_eq!(
            db.get_session(id).unwrap().unwrap().project_id.as_deref(),
            Some(survivor.as_str()),
            "every Session behind a moved path follows it"
        );
    }
    assert!(
        db.get_workstream("w-1").unwrap().is_some(),
        "a Workstream survives its Project; membership is its path list (§1.12)"
    );
    assert_eq!(
        noending::storage::workstream_paths::project_roles_for_workstream(&db.read(), "w-1")
            .unwrap(),
        vec![(survivor.clone(), true)],
        "and its primary path now projects onto the survivor"
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
        "§17-15: a rename records that a person chose this name"
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

    // §17-17: the survivor ROW is the family owner, but the NAME follows
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
    db.upsert_project(&Project::new("p-mine".into(), "Mine"))
        .unwrap();
    db.upsert_project(&Project::new("p-other".into(), "Other"))
        .unwrap();

    let id = db
        .tx(|tx| insert_workspace_path_conn(tx, &canon("/shared/path"), "p-mine"))
        .unwrap();
    // The mechanical insert is idempotent on both unique keys and NEVER
    // re-projects an existing row: ownership changes go through the one door that
    // also repairs the derived caches.
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

    // Reassignment is the one legal move, and it is a move, not a copy.
    db.tx(|tx| reassign_workspace_path_conn(tx, &id, "p-other").map(|_| ()))
        .unwrap();
    let rows = db.list_workspace_paths().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].project_id, "p-other");
    registry_is_consistent(&db).expect("no path with two owners, none with zero");
}

#[test]
fn zero_path_project_cannot_survive() {
    let (_d, db) = temp_db();
    let path = ensure(&db, &plain("/doomed/path", true));
    let doomed = path.project_id.clone();

    // A Project that still owns a path is refused rather than quietly emptied:
    // deleting it would break the chain every other fact reads through.
    assert!(db
        .tx(|tx| delete_zero_path_project_conn(tx, &doomed).map(|_| ()))
        .is_err());
    assert!(db.get_project(&doomed).unwrap().is_some());
    assert_eq!(
        db.get_workspace_path(&path.id)
            .unwrap()
            .map(|p| p.project_id)
            .as_deref(),
        Some(doomed.as_str())
    );

    // Remove the path (nothing references it) and the Project goes in the same
    // transaction, not "eventually".
    let outcome = gc_gone_workspace_paths(&db, &[path.id.clone()]).unwrap();
    assert_eq!(outcome.deleted_paths, vec![path.id.clone()]);
    assert_eq!(outcome.deleted_projects, vec![doomed.clone()]);
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

// --------------------------------------------------------------------------
// §10 — GC
// --------------------------------------------------------------------------

#[test]
fn gc_only_removes_paths_nothing_references_and_that_we_observed_gone() {
    let (_d, db) = temp_db();
    let dead = ensure(&db, &plain("/gone/forever", true));
    let by_session = ensure(&db, &plain("/gone/session", true));
    let by_workstream = ensure(&db, &plain("/gone/workstream", true));
    let never_asked = ensure(&db, &plain("/still/here", true));
    session(&db, "s-ref", "/gone/session", &by_session.id);
    workstream(&db, "w-ref", "still listed");
    add_ws_path(&db, "w-ref", &by_workstream.id);

    let outcome = gc_gone_workspace_paths(
        &db,
        &[
            dead.id.clone(),
            by_session.id.clone(),
            by_workstream.id.clone(),
            // Not in the registry at all: a replay must not fail.
            "path-never-registered".to_string(),
        ],
    )
    .unwrap();

    assert_eq!(outcome.deleted_paths, vec![dead.id.clone()]);
    assert_eq!(outcome.deleted_projects.len(), 1, "§1.2 goes with it");
    assert!(db.get_workspace_path(&by_session.id).unwrap().is_some());
    assert!(db.get_workspace_path(&by_workstream.id).unwrap().is_some());
    assert!(db.get_workspace_path(&never_asked.id).unwrap().is_some());
    assert!(
        db.get_session("s-ref").unwrap().is_some(),
        "the Session survives"
    );
    assert_eq!(db.list_workstream_paths("w-ref").unwrap().len(), 1);
    assert!(db.get_project(&dead.project_id).unwrap().is_none());

    // A path that merely looks absent is not enough either: existence is never by
    // itself a reason to delete, only a candidate list.
    let temp_gone = ensure(&db, &plain("/away/temporarily", true));
    let seen_missing = ensure(&db, &plain("/away/temporarily", false));
    assert_eq!(seen_missing.id, temp_gone.id);
    assert!(db.get_workspace_path(&temp_gone.id).unwrap().is_some());
    gc_gone_workspace_paths(&db, &[temp_gone.id.clone()]).unwrap();
    assert!(db.get_workspace_path(&temp_gone.id).unwrap().is_none());
    registry_is_consistent(&db).expect("consistent");
}

// --------------------------------------------------------------------------
// §17-1 — the WorkspaceAttaching seam
// --------------------------------------------------------------------------

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
    // fabricated (§5.5, §7.2).
    for raw in ["", "   ", "relative/thing", "~user/other"] {
        let attached = db
            .tx(|tx| projection.ensure_path(tx, raw))
            .expect("no IO in these cases");
        assert_eq!(attached, None, "`{raw}` must not become a WorkspacePath");
    }
    // Reserved app paths never become workspaces (§2), and the default workspace
    // under the same Home does (§42.3-M21).
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
        "NoEnding Workspace",
        "§17-16 through the same door that created the row"
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
        "projects.git_id references git_identities.id, not a path (§5.2)"
    );
    registry_is_consistent(&db).expect("consistent");
}

#[test]
fn workspace_attaching_refuses_an_observation_whose_identity_disagrees() {
    // The one case where the door says "nothing" instead of writing a row it
    // cannot name: `workspace::identity` defines the key, so an observer with a
    // different idea must not become a second authority (§42.2-E1, §42.3-M8).
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

#[test]
fn the_string_door_reports_its_effect_to_the_transaction_owner() {
    // A caller that writes inside its own transaction cannot reach
    // `apply_projection_effect` by accident, so the seam has to hand the effect
    // back — otherwise a Project created during ingest stays invisible to search.
    let (_d, db) = temp_db();
    let observer = Scripted::new();
    observer.set(
        "/work/repo",
        repo("/work/repo", "/work/repo/.git", GitWorktreeKind::Main, &[]),
    );
    let projection = ProjectProjection::new(&observer);
    let outcome = db
        .tx(|tx| projection.ensure_path_outcome(tx, "/work/repo"))
        .unwrap()
        .expect("attached");
    assert!(!outcome.effect.projects_touched.is_empty());
    let project_id = outcome.path.project_id.clone();
    if db.fts_available() {
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM search_index WHERE kind = 'project' AND ref_id = ?1",
                &project_id
            ),
            0
        );
        apply_projection_effect(&db, &outcome.effect);
        assert_eq!(
            count_rows(
                &db,
                "SELECT COUNT(*) FROM search_index WHERE kind = 'project' AND ref_id = ?1",
                &project_id
            ),
            1
        );
    }
}

// --------------------------------------------------------------------------
// §26 / §42.3-M7 — reconcile
// --------------------------------------------------------------------------

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
    // Project may change without a user action (§26).
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
    assert_eq!(
        after.project_id, path.project_id,
        "§1.3 through reconcile too"
    );

    // An unknown id is an error, not a silent creation.
    assert!(projection
        .reconcile_workspace_path(&db, "path-never-registered")
        .is_err());
}

#[test]
fn reconcile_sweep_discovers_worktrees_then_gcs_the_one_that_is_really_gone() {
    let (_d, db) = temp_db_locked();
    let observer = Scripted::new();
    observer.set(
        "/work/repo",
        repo(
            "/work/repo",
            "/work/repo/.git",
            GitWorktreeKind::Main,
            &["/work/repo", "/work/repo-feature"],
        ),
    );
    observer.set("/work/repo-feature", plain("/work/repo-feature", true));
    let projection = ProjectProjection::new(&observer);

    // Seed through the ordinary door: a plain directory, no Git knowledge yet.
    {
        ensure(&db, &plain("/work/repo", true));
    }

    // Pass 1: the sweep re-observes the registry, finds the family (an in-place
    // upgrade of the seeded Project), and §9 pulls the sibling worktree in.
    let report = reconcile_workspace_paths(&db, &projection, 500).unwrap();
    assert_eq!(report.scanned, 1, "only /work/repo was registered yet");
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(
        report.discovered_paths,
        vec![path_id_of("/work/repo-feature")],
        "a discovered worktree becomes a WorkspacePath"
    );

    let family = {
        let row = db
            .get_workspace_path(&path_id_of("/work/repo"))
            .unwrap()
            .unwrap();
        assert_eq!(row.git_state, git_state::DETECTED);
        paths_of(&db, &row.project_id)
    };
    assert_eq!(family.len(), 2, "one Project, both worktrees (§9)");
    let feature = family
        .iter()
        .find(|p| p.id == path_id_of("/work/repo-feature"))
        .expect("the discovered worktree");
    assert_eq!(feature.git_state, git_state::DETECTED);
    assert_eq!(feature.git_kind.as_deref(), Some("unknown"));
    assert!(!feature.exists, "Git mentioned it; we never stood there");
    {
        assert!(
            db.list_workstream_paths("no-such-workstream")
                .unwrap()
                .is_empty(),
            "§9: discovering a WorkspacePath adds no WorkstreamPath"
        );
    }

    // Pass 2: the feature worktree's directory is removed, but Git still LISTS it
    // (a prunable entry). §10's third condition is "no longer a live worktree", so
    // the sweep must not delete a path another observation claims is live — that
    // is an endless delete/re-create churn on every repository that never ran
    // `git worktree prune`.
    observer.set("/work/repo-feature", plain("/work/repo-feature", false));
    let kept = reconcile_workspace_paths(&db, &projection, 500).unwrap();
    assert_eq!(kept.scanned, 2);
    assert!(
        kept.outcome.deleted_paths.is_empty(),
        "still listed by the family: {:?}",
        kept.outcome.deleted_paths
    );
    {
        assert!(db.get_workspace_path(&feature.id).unwrap().is_some());
    }

    // Pass 3: the family no longer mentions it. Absent + unreferenced + not a live
    // worktree ⇒ §10 removes the path, and the Project survives because it still
    // owns the main worktree.
    observer.set(
        "/work/repo",
        repo(
            "/work/repo",
            "/work/repo/.git",
            GitWorktreeKind::Main,
            &["/work/repo"],
        ),
    );
    let swept = reconcile_workspace_paths(&db, &projection, 500).unwrap();
    assert_eq!(swept.scanned, 2);
    assert_eq!(swept.outcome.deleted_paths, vec![feature.id.clone()]);
    assert!(
        swept.outcome.deleted_projects.is_empty(),
        "the family still owns /work/repo, so its Project survives"
    );
    {
        assert!(db.get_workspace_path(&feature.id).unwrap().is_none());
        assert!(db
            .get_workspace_path(&path_id_of("/work/repo"))
            .unwrap()
            .is_some());
        assert_eq!(db.list_projects().unwrap().len(), 1);
        registry_is_consistent(&db).expect("consistent after the sweep");
    }
}

#[test]
fn reconcile_sweep_keeps_every_path_it_can_still_see() {
    // §10: a sweep is not a cleanup. Two registered directories that nothing
    // references survive every reconcile for as long as they exist on disk — only
    // a path the sweep personally observed ABSENT is even a candidate, so a capped
    // or partial pass can never mistake "not scanned yet" for "gone".
    let (_d, db) = temp_db_locked();
    let observer = Scripted::new();
    observer.set("/aaa/first", plain("/aaa/first", true));
    observer.set("/bbb/second", plain("/bbb/second", true));
    let projection = ProjectProjection::new(&observer);
    let ids = {
        (
            ensure(&db, &plain("/aaa/first", true)).id,
            ensure(&db, &plain("/bbb/second", true)).id,
        )
    };

    let report = reconcile_workspace_paths(&db, &projection, 500).unwrap();
    assert_eq!(report.scanned, 2);
    assert!(report.outcome.deleted_paths.is_empty());
    assert!(report.outcome.deleted_projects.is_empty());
    {
        assert!(db.get_workspace_path(&ids.0).unwrap().is_some());
        assert!(db.get_workspace_path(&ids.1).unwrap().is_some());
        assert_eq!(db.list_projects().unwrap().len(), 2);
        registry_is_consistent(&db).expect("consistent");
    }
}

// --------------------------------------------------------------------------
// §11 — naming, rename, detail
// --------------------------------------------------------------------------

#[test]
fn default_workspace_project_is_named_noending_workspace() {
    let (_d, db) = temp_db();
    let home = Home::new(
        "/Users/me/.noending",
        &["/Users/me/.noending/data"],
        Some("/Users/me/.noending/workspace"),
    );
    let dw =
        ensure_workspace_path(&db, &plain("/Users/me/.noending/workspace", true), &home).unwrap();
    assert_eq!(project_of(&db, &dw).name, "NoEnding Workspace");

    // After a Home move the OLD workspace is an ordinary directory and must not
    // keep the special name (方案 §4; `auto_project_name`'s documented contract).
    let old = ensure_workspace_path(&db, &plain("/Users/other/.noending/workspace", true), &home)
        .unwrap();
    assert_eq!(project_of(&db, &old).name, "Workspace");
    assert_eq!(db.list_projects().unwrap().len(), 2);
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

    // The whole-object write the legacy command used must not rename a customized
    // row either — `upsert_project_conn`'s `CASE` db (§37).
    let mut lying = renamed.clone();
    lying.name = "Auto Renamed".into();
    lying.name_customized = false;
    db.upsert_project(&lying).unwrap();
    let stored = db.get_project(&project_id).unwrap().unwrap();
    assert_eq!(stored.name, "重要的项目");
    assert!(stored.name_customized, "name_customized is one-way");

    // And §1.3's other half: an automatic write that carries no family must never
    // CLEAR one. (The reverse direction — a caller-supplied non-null `git_id`
    // replacing a family — is why `update_project` leaves the command surface.)
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
    assert_eq!(after.git_id, with_family.git_id, "§1.3: never cleared");
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
    session(&db, "s-det", "/work/repo", &path.id);
    workstream(&db, "w-primary", "primary here");
    workstream(&db, "w-related", "related here");
    add_ws_path(&db, "w-primary", &path.id);
    add_ws_path(&db, "w-related", &sibling.id);
    add_ws_path(&db, "w-related", &path.id);

    let detail = project_detail(&db, &path.project_id)
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
        vec!["s-det".to_string()]
    );
    assert_eq!(
        detail
            .workstreams
            .iter()
            .map(|w| (w.workstream.id.clone(), w.is_primary))
            .collect::<Vec<_>>(),
        vec![
            ("w-primary".to_string(), true),
            ("w-related".to_string(), false)
        ],
        "§1.12/E4: membership is ANY path of the Project; position 0 is what makes a row 主关联"
    );
    assert_eq!(
        noending::storage::workstream_paths::workstreams_for_project(
            &db.read(),
            &sibling.project_id
        )
        .unwrap()
        .into_iter()
        .map(|(w, primary)| (w.id, primary))
        .collect::<Vec<_>>(),
        vec![("w-related".to_string(), true)]
    );

    // The §11 shape is a contract with the frontend bridge: these keys, nothing else.
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
        vec!["project", "sessions", "workspace_paths", "workstreams"]
    );
    let mut workstream_keys: Vec<&str> = value["workstreams"][0]
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.as_str())
        .collect();
    workstream_keys.sort_unstable();
    assert_eq!(workstream_keys, vec!["is_primary", "workstream"]);
    assert_eq!(
        value["workstreams"][0]["is_primary"],
        serde_json::json!(true)
    );

    // A Project that does not exist is `None`, and the command maps that to an
    // error rather than an empty object.
    assert!(project_detail(&db, "p-never").unwrap().is_none());
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

    // And a Project that loses its last path leaves the list by ceasing to exist.
    gc_gone_workspace_paths(&db, &[b.id.clone()]).unwrap();
    let rest: Vec<String> = db
        .list_projects()
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(rest, vec![a.project_id.clone()]);
}

#[test]
fn the_core_writes_no_index_rows_and_the_wrapper_does() {
    // §42.3-M4: the transaction-scoped core never touches `search_index`, so a
    // rolled-back transaction cannot leave search describing a Project that was
    // never committed. The effect list is the whole handover.
    let (_d, db) = temp_db();
    let observation = repo("/work/repo", "/work/repo/.git", GitWorktreeKind::Main, &[]);
    let outcome = db
        .tx(|tx| ensure_workspace_path_conn(tx, &observation, &UnrestrictedWorkspace))
        .unwrap();
    assert!(!outcome.effect.projects_touched.is_empty());
    assert!(outcome.effect.projects_deleted.is_empty());
    let project_id = outcome.path.project_id.clone();

    let indexed = |db: &Db| -> i64 {
        db.read()
            .query_row(
                "SELECT COUNT(*) FROM search_index WHERE kind = 'project' AND ref_id = ?1",
                [project_id.as_str()],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0)
    };
    assert_eq!(
        indexed(&db),
        0,
        "in-transaction: still nothing indexed (or FTS is unavailable, which is also 0)"
    );

    apply_projection_effect(&db, &outcome.effect);
    if db.fts_available() {
        assert_eq!(indexed(&db), 1, "after commit: exactly one row");
    }

    // A deleted id is never also "touched": the wrapper must not re-create the row
    // it was told to drop.
    let mut effect = ProjectionEffect::default();
    effect.touch(&project_id);
    effect.delete(&project_id);
    assert!(effect.projects_touched.is_empty());
    assert_eq!(effect.projects_deleted, vec![project_id.clone()]);
    apply_projection_effect(&db, &effect);
    if db.fts_available() {
        assert_eq!(indexed(&db), 0);
    }
}

#[test]
fn an_unregistered_home_policy_leaves_the_registry_open() {
    // The default policy is what the v12 migration and every test here use:
    // nothing reserved, no special name — so §8 is fully exercisable with no
    // filesystem and no Home at all (§42.3-M13).
    let (_d, db) = temp_db();
    let path = ensure(&db, &plain("/anything/at/all", true));
    assert_eq!(project_of(&db, &path).name, "All");
    let observer = Scripted::new();
    let projection = ProjectProjection::new(&observer);
    assert!(!projection.policy().is_reserved("/anything/at/all"));
    assert_eq!(projection.policy().default_workspace(), None);
}

// ------------------------------------------------ §44 Windows case aliases
//
// 方案 §10 settled this as 方案 A: `ensure_workspace_path_conn` re-derives the id
// through the host's rule (`project.rs:295`), so a macOS runner can prove the key
// — `identity.rs` and `workspace_identity_test.rs` do — but not the registry.
// Injecting a style into that door would mean bending production for the test, so
// the claim is pinned where it is actually true: CI's `windows-latest` job.
// 方案 §44.5 requires reading for these two names in that job's log, because on
// macOS they compile away to nothing — which also means Main could not falsify
// them by hand here. The falsifiable-on-any-host version is the one below.

/// §44.6 — `git_identities` is found by location, not by spelling. Two spellings
/// of one `common_dir` must return the id that was created first; before §44 the
/// second call inserted a fresh row, and §8.5 reads a second family as license to
/// move a WorkspacePath into a second Project.
///
/// Separator-only (rather than case) because the location relation it proves is
/// the one the host applies: on a Unix runner case is not part of it, and the
/// case half is what the `#[cfg(windows)]` tests below carry.
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

/// §44: two case spellings of one directory are one WorkspacePath, and the row
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

/// §44.6's harder half: one repository reported under two spellings is one
/// family. Without the location-keyed `ensure_git_identity_conn`, the second
/// observation mints a second `git_identities` row, §8.5 reads that as a family
/// change and moves the path into a second Project — splitting what §8.3 exists to
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

// ===========================================================================
// Projects Experience v0.2 §27 — workspace refresh contract.
//
// The two refresh entries (global / per-project) ride ONE rule set: the
// reconcile_workspace_path_ids primitive below is the same observe → ensure →
// GC pipeline the global sweep runs. These tests lock that contract without
// any real git or real Home (§42.3-M13).
// ===========================================================================

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
        // 一条引用，让 b 免于 GC——本测试观察的是"重观察"本身。
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
    assert_eq!(
        progress.lock().unwrap().last(),
        Some(&(2, 2)),
        "§12 progress"
    );
    let b = db
        .get_workspace_path(&path_id_of("/work/b"))
        .unwrap()
        .expect("b survives: it is only missing, GC is a separate decision");
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
fn refresh_marks_deleted_directory_missing() {
    let (_d, db) = temp_db_locked();
    let observer = Scripted::new();
    observer.set("/work/a", plain("/work/a", false));
    let projection = ProjectProjection::new(&observer);
    let wp = {
        let wp = ensure(&db, &plain("/work/a", true));
        // 引用让路径在 GC 判定下存活，好观察 missing 标记本身。
        session(&db, "s-a", "/work/a", &wp.id);
        wp
    };

    let report =
        reconcile_workspace_path_ids(&db, &projection, &[wp.id.clone()], &|_, _| {}).unwrap();
    assert_eq!(report.missing_paths, 1);
    let after = { db.get_workspace_path(&wp.id).unwrap() };
    let after = after.expect("row survives the observation");
    assert!(!after.exists);
    assert!(report.outcome.deleted_paths.is_empty());
}

#[test]
fn referenced_missing_path_survives_refresh() {
    let (_d, db) = temp_db_locked();
    let observer = Scripted::new();
    observer.set("/work/a", plain("/work/a", false));
    let projection = ProjectProjection::new(&observer);
    let wp = {
        let wp = ensure(&db, &plain("/work/a", true));
        // 一条 Session 引用让这条路径不可 GC（§15 的引用条件）。
        session(&db, "s-ref", "/work/a", &wp.id);
        wp
    };

    let report =
        reconcile_workspace_path_ids(&db, &projection, &[wp.id.clone()], &|_, _| {}).unwrap();
    assert_eq!(report.missing_paths, 1);
    assert!(
        report.outcome.deleted_paths.is_empty(),
        "referenced paths are never GC'd, got {:?}",
        report.outcome.deleted_paths
    );
    {
        assert!(db.get_workspace_path(&wp.id).unwrap().is_some());
    }
}

#[test]
fn unreferenced_missing_path_is_gc_d() {
    let (_d, db) = temp_db_locked();
    let observer = Scripted::new();
    observer.set("/work/a", plain("/work/a", false));
    let projection = ProjectProjection::new(&observer);
    let wp = { ensure(&db, &plain("/work/a", true)) };

    let report =
        reconcile_workspace_path_ids(&db, &projection, &[wp.id.clone()], &|_, _| {}).unwrap();
    assert_eq!(report.outcome.deleted_paths, vec![wp.id.clone()]);
    {
        assert!(db.get_workspace_path(&wp.id).unwrap().is_none());
    }
}

#[test]
fn last_path_gc_retires_project() {
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
    assert_eq!(report.outcome.deleted_projects, vec![project_id.clone()]);
    {
        assert!(db.get_project(&project_id).unwrap().is_none());
    }
    {
        registry_is_consistent(&db).expect("registry stays consistent");
    }
}

#[test]
fn git_worktree_registration_prevents_premature_gc() {
    let (_d, db) = temp_db_locked();
    let observer = Scripted::new();
    observer.set(
        "/work/repo",
        repo(
            "/work/repo",
            "/work/repo/.git",
            GitWorktreeKind::Main,
            &["/work/repo", "/work/repo-feature"],
        ),
    );
    let projection = ProjectProjection::new(&observer);
    {
        ensure(&db, &plain("/work/repo", true));
    }
    // 第一轮：feature 经由 `git worktree list` 被收养进注册表（目录尚不存在）。
    reconcile_workspace_paths(&db, &projection, 500).unwrap();

    // 定点刷新两个家族路径：feature 仍被家族列表提及 → 即使目录缺失也不 GC。
    let report = reconcile_workspace_path_ids(
        &db,
        &projection,
        &[path_id_of("/work/repo"), path_id_of("/work/repo-feature")],
        &|_, _| {},
    )
    .unwrap();
    assert_eq!(report.scanned, 2);
    assert!(
        report.outcome.deleted_paths.is_empty(),
        "a prunable worktree listing keeps the path alive, got {:?}",
        report.outcome.deleted_paths
    );
    {
        assert!(db
            .get_workspace_path(&path_id_of("/work/repo-feature"))
            .unwrap()
            .is_some());
    }
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
fn refresh_does_not_touch_workstream_paths() {
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
        "SELECT COUNT(*) FROM workstream_paths WHERE workspace_path_id = ?",
        &wp.id,
    );
    assert_eq!(before, 1);

    reconcile_workspace_path_ids(&db, &projection, &[wp.id.clone()], &|_, _| {}).unwrap();
    assert_eq!(
        count_rows(
            &db,
            "SELECT COUNT(*) FROM workstream_paths WHERE workspace_path_id = ?",
            &wp.id
        ),
        before,
        "refresh never writes workstream_paths (§9)"
    );
}

#[test]
fn refresh_does_not_touch_session_history() {
    let (_d, db) = temp_db_locked();
    let observer = Scripted::new();
    observer.set("/work/a", plain("/work/a", false));
    let projection = ProjectProjection::new(&observer);
    let wp = {
        let wp = ensure(&db, &plain("/work/a", true));
        session(&db, "s-hist", "/work/a", &wp.id);
        wp
    };
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
    let s = { db.get_session("s-hist").unwrap().expect("session intact") };
    assert!(s.trashed_at.is_none());
    assert_eq!(s.workspace_path_id.as_deref(), Some(wp.id.as_str()));
}

#[test]
fn refresh_does_not_ingest_sessions() {
    let (_d, db) = temp_db_locked();
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
