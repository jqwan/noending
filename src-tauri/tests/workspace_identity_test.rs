//! Workspace identity integrity.
//!
//! `workspace_paths.id` is the join key for Sessions, WorkstreamPaths and — one
//! hop further — every Project fact. Nothing else can notice a row whose id is
//! not the one its own `canonical_path` derives to, because every join agrees
//! with the stored id. So these tests do not exercise a behaviour: they check
//! that the *key itself* is the derivation, on both platforms. Temp dirs only;
//! no user Home and no real transcript.

use noending::domain::{GitDetection, GitWorktreeKind, WorkspaceObservation};
use noending::storage::workspace::insert_workspace_path_conn;
use noending::storage::{new_id, now, Db};
use noending::workspace::home::NoEndingHome;
use noending::workspace::project::{
    ensure_workspace_path, registry_is_consistent, UnrestrictedWorkspace,
};
use noending::workspace::{
    normalize_path, normalize_path_with, path_identity, path_identity_of, path_identity_with,
    NormalizeOpts, PathStyle,
};

mod support;

fn canon(raw: &str) -> String {
    normalize_path(raw).expect("test paths are absolute")
}

fn win(raw: &str) -> String {
    normalize_path_with(
        raw,
        NormalizeOpts {
            style: Some(PathStyle::Windows),
            base: None,
            home: Some("C:\\Users\\me"),
        },
    )
    .expect("windows test paths are absolute")
}

fn temp_db() -> (std::path::PathBuf, Db) {
    let dir = std::env::temp_dir().join(format!("noending-wsid-{}", new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("noending.db")).expect("temp db");
    (dir, db)
}

fn plain(raw: &str) -> WorkspaceObservation {
    let canonical = canon(raw);
    WorkspaceObservation {
        path_id: path_identity(&canonical),
        canonical_path: canonical,
        exists: true,
        git: GitDetection::None,
    }
}

fn repo(raw: &str, common: &str, kind: GitWorktreeKind) -> WorkspaceObservation {
    WorkspaceObservation {
        git: GitDetection::Detected {
            common_dir: canon(common),
            toplevel: Some(canon(raw)),
            kind,
            worktrees: vec![canon(raw)],
        },
        ..plain(raw)
    }
}

// 1. the key

/// the check `registry_is_consistent` actually performs.
///
/// Its predecessor — `GROUP BY id HAVING COUNT(DISTINCT project_id) > 1` — could
/// never fail, because `id` is the PRIMARY KEY. A row whose id is *not*
/// `path_identity(its own canonical_path)` is reachable (a hand-written INSERT, a
/// second normalizer — "禁止第二份实现") and every join agrees with it anyway, so
/// the corruption stays silent.
#[test]
fn registry_rejects_a_workspace_path_whose_id_is_not_its_own_derivation() {
    let (_d, db) = temp_db();
    db.upsert_project(&support::project("p1".to_string(), "One"))
        .unwrap();
    let canonical = canon("/identity-integrity/repo");
    let good = db
        .tx(|tx| insert_workspace_path_conn(tx, &canonical, "p1"))
        .unwrap();
    assert_eq!(good, path_identity(&canonical), "the key IS the derivation");
    registry_is_consistent(&db).expect("a derived id is consistent");

    // The same shape of row, under an id that is not its own derivation.
    db.write()
        .execute(
            "INSERT INTO workspace_paths
               (id, canonical_path, project_id, git_state, git_kind, exists_on_disk,
                first_seen_at, last_seen_at)
             VALUES ('path-not-derived-from-this-path', ?1, 'p1', 'none', NULL, 1, ?2, ?2)",
            rusqlite::params![canon("/identity-integrity/other"), now()],
        )
        .unwrap();
    let err = registry_is_consistent(&db).expect_err("a lying id must be reported");
    assert!(err.contains("not derived from its canonical_path"), "{err}");
}

/// one Git family, one Project, structurally rather than by convention.
#[test]
fn one_git_family_cannot_be_claimed_by_two_projects() {
    let (_d, db) = temp_db();
    db.upsert_project(&support::project("fa".to_string(), "A"))
        .unwrap();
    db.upsert_project(&support::project("fb".to_string(), "B"))
        .unwrap();
    let err = db
        .write()
        .execute(
            "UPDATE projects SET git_id = 'git-shared' WHERE id IN ('fa','fb')",
            [],
        )
        .expect_err("idx_projects_git_id is UNIQUE: two rows cannot share a family");
    assert!(
        err.to_string().contains("UNIQUE"),
        "expected the partial UNIQUE index to refuse it, got {err}"
    );
}

// 2. cross-platform

/// The Unix half of the derivation, pinned as data rather than as a rule.
///
/// Stored `workspace_paths.id` values are keyed by these strings, so changing the
/// derivation would re-key every existing row — an identity change, not a fix.
/// The literals came from an independent implementation of
/// `sha256("noending:workspace-path:v1:" + path_key)`, not from running this
/// module, so the test stays a check even if the module is wrong.
#[test]
fn unix_path_identity_vectors_are_stable() {
    const VECTORS: [(&str, &str); 6] = [
        (
            "/Users/example/code/noending",
            "path-68ce5f80501db78b93ed64f2c82229e3",
        ),
        ("/tmp/project", "path-b2d50482a612276bf0fd044f0001f741"),
        ("/home/user/repo", "path-17728608906fe02ea7bf2ea7bf3a2e25"),
        (
            "/Users/me/.noending/workspace",
            "path-efc8a6c0f1e0ce8676d372f4c0f91295",
        ),
        (
            "/Users/dev/projects/noending",
            "path-f89dfc8b3a07fb0b6083631898a33217",
        ),
        // The case pair that 's Test 4 keeps apart, pinned as data too.
        ("/var/data/Repo", "path-ee90d2b156a11b39705abc879aebcde4"),
    ];
    for (canonical, expected) in VECTORS {
        assert_eq!(
            path_identity_with(canonical, PathStyle::Unix),
            expected,
            "{canonical}"
        );
    }
    // The host door has to agree with the Unix door exactly when the host is
    // Unix — and on a Windows runner it must NOT, which is the other half of the
    // proof that these literals were not quietly re-derived per platform.
    if PathStyle::current() == PathStyle::Unix {
        assert_eq!(
            path_identity_of(VECTORS[0].0).as_deref(),
            Some(VECTORS[0].1),
            "a Unix host keeps storing these ids"
        );
    } else {
        assert_ne!(
            path_identity_with(VECTORS[0].0, PathStyle::Windows),
            VECTORS[0].1,
            "the fold must actually change Windows ids, or fixed nothing"
        );
    }
    // A trailing separator is trimmed by the key itself, because an
    // externally-supplied spelling must not become a second identity.
    assert_eq!(
        path_identity_with("/Users/dev/projects/noending/", PathStyle::Unix),
        "path-f89dfc8b3a07fb0b6083631898a33217"
    );
    // And a case difference is still a different Unix directory.
    assert_ne!(
        path_identity_with("/Users/dev/projects/repo", PathStyle::Unix),
        "path-ee90d2b156a11b39705abc879aebcde4"
    );
}

/// + every Windows spelling of one directory carries one
/// identity, including the case variants folded into the key, and the
/// *display* form keeps the case the user typed.
#[test]
fn windows_spellings_of_one_directory_carry_one_identity() {
    let canonical = win("C:\\Users\\me\\.noending\\workspace");
    assert_eq!(canonical, "C:\\Users\\me\\.noending\\workspace");
    let w = PathStyle::Windows;
    let expected = path_identity_with(&canonical, w);
    // Separator, trailing-separator, verbatim-prefix, `.`-hop and *case*
    // spellings. Case joins them because on a Windows volume it names the same
    // directory, and  made the stored key agree with that comparison
    // rather than leaving `same_location` and `path_identity` apart.
    for raw in [
        "C:\\Users\\me\\.noending\\workspace",
        "C:/Users/me/.noending/workspace",
        "C:\\Users\\me\\.noending\\workspace\\",
        "C:\\Users\\me\\.noending\\.\\workspace",
        "\\\\?\\C:\\Users\\me\\.noending\\workspace",
        "C:\\Users\\ME\\.noending\\WORKSPACE",
        "c:\\Users\\me\\.NoEnding\\Workspace",
    ] {
        assert_eq!(
            path_identity_with(&win(raw), w),
            expected,
            "{raw} is one directory with {canonical}"
        );
    }
    // The fold reaches the key only. Two case variants remain two *strings*,
    // and the row keeps the spelling that got there first.
    assert_ne!(
        win("C:\\Users\\me\\.noending\\WORKSPACE"),
        canonical,
        "identity folded, display did not"
    );
    // The drive letter was already upper-cased by normalize for both platforms,
    // so that alias was never a split to begin with.
    assert_eq!(
        path_identity_with(&win("c:\\Users\\me\\.noending\\workspace"), w),
        expected
    );
    // And Unix still does not fold: the same pair of spellings is two ids.
    assert_ne!(
        path_identity_with("/Repo", PathStyle::Unix),
        path_identity_with("/repo", PathStyle::Unix)
    );
}

/// The premise under the launcher's path-list gate (M30).
///
/// `apply_match` decides whether to grow a Workstream's ordered path list by
/// comparing `path_identity_of(default_workspace)` against
/// `sessions.workspace_path_id`. Both sides come from the *host* platform, so
/// Home's default workspace string must be exactly what the identity door
/// re-derives — otherwise the gate stops recognising the fallback and an
/// app-invented directory re-enters the user's path list.
#[test]
fn the_default_workspace_the_launcher_hands_round_trips_through_the_identity_door() {
    let base = std::env::temp_dir().join(format!("noending-wsid-home-{}", new_id()));
    std::fs::create_dir_all(&base).unwrap();
    let home = NoEndingHome::new(base.to_string_lossy().as_ref(), None).unwrap();
    let stored = home.default_workspace_str();
    assert_eq!(
        path_identity_of(&stored).as_deref(),
        Some(path_identity(&canon(&stored)).as_str()),
        "the string Home reports is already canonical, so the door adds nothing"
    );
    assert!(
        !home.reserved().contains(&stored),
        "workspace/ is the one path under Home that is a normal WorkspacePath"
    );
    assert!(
        home.reserved().contains(&home.db_path_str()),
        "data/ is not"
    );
    std::fs::remove_dir_all(&base).ok();
}

/// on a Windows volume a case variant of a reserved app directory is still
/// that directory, so it is still reserved. Proven with an injected
/// [`PathStyle`], because CI's Windows behavior cannot otherwise be reached from
/// a macOS runner.
#[test]
fn a_case_variant_of_a_reserved_windows_directory_is_still_reserved() {
    let home = NoEndingHome::new_with_style(
        "C:\\Users\\me\\.noending",
        Some("C:\\Users\\me"),
        PathStyle::Windows,
    )
    .unwrap();
    for spelling in [
        "C:\\Users\\me\\.noending\\data",
        "c:\\users\\me\\.NOENDING\\Data",
        "C:/Users/me/.noending/RUNTIME",
        "\\\\?\\C:\\Users\\me\\.noending\\logs\\archive",
    ] {
        let canonical = win(spelling);
        assert!(
            home.reserved()
                .contains_with(&canonical, PathStyle::Windows),
            "{spelling} → {canonical} is a reserved app directory on a case-insensitive volume"
        );
    }
    assert!(
        !home.reserved().contains_with(
            &win("C:\\Users\\me\\.noending\\workspace"),
            PathStyle::Windows
        ),
        "folding must not reserve the Home wholesale"
    );
    assert!(
        !home.reserved().contains_with(
            &win("C:\\Users\\me\\.noending\\database"),
            PathStyle::Windows
        ),
        "containment stays segment-wise: `database` is a sibling of `data`"
    );
}

/// rule 5 — the aliasing cost is only acceptable because Git
/// convergence actually runs. Without this, "two spellings, one Project" would
/// be an unverified claim rather than a mechanism.
#[test]
fn git_convergence_reunites_two_paths_that_report_one_family() {
    let (_d, db) = temp_db();
    // Two directories, each reporting the same common dir: the shape a real
    // repository and a copy of it (or two case spellings) reach the door with.
    let main = ensure_workspace_path(
        &db,
        &repo(
            "/converge/repo",
            "/converge/repo/.git",
            GitWorktreeKind::Main,
        ),
        &UnrestrictedWorkspace,
    )
    .unwrap();
    let alias = ensure_workspace_path(
        &db,
        &repo(
            "/converge/repo-copy",
            "/converge/repo/.git",
            GitWorktreeKind::Main,
        ),
        &UnrestrictedWorkspace,
    )
    .unwrap();
    assert_ne!(main.id, alias.id, "two directories, two rows");
    assert_eq!(
        main.project_id, alias.project_id,
        "one Git family, so one Project"
    );
    registry_is_consistent(&db).expect("consistent after convergence");
}
