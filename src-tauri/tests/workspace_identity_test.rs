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
    normalize_path, normalize_path_with, path_identity, path_identity_of, NormalizeOpts, PathStyle,
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

/// §42.3-M8 — every Windows spelling §2 permits collapses to one identity, and
/// the *display* form keeps the case the user typed.
#[test]
fn windows_spellings_of_one_directory_carry_one_identity() {
    let canonical = win("C:\\Users\\me\\.noending\\workspace");
    assert_eq!(canonical, "C:\\Users\\me\\.noending\\workspace");
    let expected = path_identity(&canonical);
    // Separator, trailing-separator, verbatim-prefix and `.`-hop spellings only.
    // A *case* difference below the drive is deliberately not in this list: see
    // the assertion after it.
    for raw in [
        "C:\\Users\\me\\.noending\\workspace",
        "C:/Users/me/.noending/workspace",
        "C:\\Users\\me\\.noending\\workspace\\",
        "C:\\Users\\me\\.noending\\.\\workspace",
        "\\\\?\\C:\\Users\\me\\.noending\\workspace",
    ] {
        assert_eq!(
            path_identity(&win(raw)),
            expected,
            "{raw} is one directory with {canonical}"
        );
    }
    // Case below the drive is NOT folded into the identity. `path_identity`
    // hashes `path_key`, which converges separators only, because
    // `canonical_path` is a display string as well as a key and
    // `identity_key` (the folding comparison) is reserved for the gates that
    // cannot self-heal. This is §42.3-M8's accepted alias — and the boundary is
    // worth pinning: `C:\\A\\b` and `C:\\a\\b` are two WorkspacePaths, which is
    // precisely the trade Main should re-read before any Windows release.
    //
    // The drive letter itself IS folded, because `normalize_path` upper-cases it
    // for both platforms, so that one is not an alias.
    assert_ne!(
        path_identity(&win("C:\\Users\\me\\.noending\\WORKSPACE")),
        expected
    );
    assert_eq!(
        path_identity(&win("c:\\Users\\me\\.noending\\workspace")),
        expected,
        "the drive is upper-cased by normalize, so that one alias is impossible"
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
