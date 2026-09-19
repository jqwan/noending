//! Workspace identity integrity (方案 §1.1, §2, §42.3-M8).
//!
//! `workspace_paths.id` is the join key for Sessions, WorkstreamPaths and — one
//! hop further — every Project fact. Nothing else in the app can notice a row
//! whose id is not the one its own `canonical_path` derives to, because every
//! join agrees with the stored id. So these tests do not exercise a behaviour:
//! they check that the *key itself* is the derivation, on both platforms.
//!
//! Temp directories only; no user Home and no real transcript (§42.3-M13).

use noending::domain::{GitDetection, GitWorktreeKind, Project, WorkspaceObservation};
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

// ------------------------------------------------------------------ 1. the key

/// §1.1 — the check `registry_is_consistent` actually performs.
///
/// The assertion this replaced was `GROUP BY id HAVING COUNT(DISTINCT
/// project_id) > 1`, which can never be non-zero because `id` is the PRIMARY
/// KEY. An assertion that cannot fail is worse than none, because it reads as
/// coverage. A row whose id is *not* `path_identity(its own canonical_path)` is
/// reachable — a hand-written INSERT, a migration that derives the key
/// differently, or a second normalizer (§42.3-M8: "禁止第二份实现") — and every
/// join keeps agreeing with it, so the corruption stays silent.
#[test]
fn registry_rejects_a_workspace_path_whose_id_is_not_its_own_derivation() {
    let (_d, db) = temp_db();
    db.upsert_project(&Project::new("p1".to_string(), "One"))
        .unwrap();
    let canonical = canon("/identity-integrity/repo");
    let good = db
        .tx(|tx| insert_workspace_path_conn(tx, &canonical, "p1"))
        .unwrap();
    assert_eq!(good, path_identity(&canonical), "the key IS the derivation");
    registry_is_consistent(&db).expect("a derived id is consistent");

    // The same shape of row, under an id that is not its own derivation.
    db.conn()
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

/// §8.3 — one Git family, one Project, structurally rather than by convention.
#[test]
fn one_git_family_cannot_be_claimed_by_two_projects() {
    let (_d, db) = temp_db();
    db.upsert_project(&Project::new("fa".to_string(), "A"))
        .unwrap();
    db.upsert_project(&Project::new("fb".to_string(), "B"))
        .unwrap();
    let err = db
        .conn()
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

// --------------------------------------------------------- 2. cross-platform

/// The Unix half of 方案 §42.3-M8.4, frozen as data rather than as a rule.
///
/// macOS ships v12 rows keyed by these values, so an identity change here is a
/// data migration and not a fix. The literals were produced by an independent
/// implementation of `sha256("noending:workspace-path:v1:" + path_key)` — not
/// by running this module and copying what came out — so the test stays a check
/// even if the module is wrong.
#[test]
fn unix_v12_path_identity_vectors_are_stable() {
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
        // The case pair that §8's Test 4 keeps apart, pinned as data too.
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
            "the fold must actually change Windows ids, or §44 fixed nothing"
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

/// §42.3-M8 + §44 — every Windows spelling of one directory carries one
/// identity, including the case variants §44 folded into the key, and the
/// *display* form keeps the case the user typed.
#[test]
fn windows_spellings_of_one_directory_carry_one_identity() {
    let canonical = win("C:\\Users\\me\\.noending\\workspace");
    assert_eq!(canonical, "C:\\Users\\me\\.noending\\workspace");
    let w = PathStyle::Windows;
    let expected = path_identity_with(&canonical, w);
    // Separator, trailing-separator, verbatim-prefix, `.`-hop and *case*
    // spellings. Case joins them because on a Windows volume it names the same
    // directory, and 方案 §44 made the stored key agree with that comparison
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
    // and the row keeps the spelling that got there first (§44.2).
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

/// §42.3-M8 + M30 — the premise under the launcher's path-list gate.
///
/// `apply_match` decides whether to grow a Workstream's ordered path list by
/// comparing `path_identity_of(default_workspace)` against
/// `sessions.workspace_path_id`. Both sides are produced on the *host* platform,
/// so the host form of NoEnding Home's default workspace must be exactly the
/// string the identity door re-derives. If it were not, the gate would stop
/// recognising the fallback and an app-invented directory would re-enter the
/// user's path list — the failure M30 exists to prevent.
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
        "workspace/ is the one path under Home that is a normal WorkspacePath (§2)"
    );
    assert!(
        home.reserved().contains(&home.db_path_str()),
        "data/ is not (§2)"
    );
    std::fs::remove_dir_all(&base).ok();
}

/// §2 — on a Windows volume a case variant of a reserved app directory is still
/// that directory, so it is still reserved. Proven with an injected
/// [`PathStyle`], because CI's Windows behavior cannot otherwise be reached from
/// a macOS runner (§42.3-M8).
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
        "folding must not reserve the Home wholesale (§2)"
    );
    assert!(
        !home.reserved().contains_with(
            &win("C:\\Users\\me\\.noending\\database"),
            PathStyle::Windows
        ),
        "containment stays segment-wise: `database` is a sibling of `data`"
    );
}

/// §42.3-M8 rule 5 — the aliasing cost is only acceptable because Git
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
        "one Git family, so one Project (§8.3)"
    );
    registry_is_consistent(&db).expect("consistent after convergence");
}
