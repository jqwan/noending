//! WorkspaceResolver regression tests (方案 §16 “必须测试”).
//!
//! The pure, lexical half of the resolver is unit-tested inside
//! `workspace/resolver.rs` with injected fixtures. This file covers what a
//! fixture cannot prove: that a real `git` binary, run through
//! `platform::exec_resolver` + `platform::exec_runner`, produces the
//! observation the domain claims.
//!
//! Environment rules (§42.3-M13):
//! * Nothing here reads or writes the process environment: the Home, the user
//!   home and the reserved set are all passed in explicitly.
//! * Nothing here resolves the real `~/.noending`.
//! * Every repository is created under a real temp directory whose path has been
//!   `fs::canonicalize`d first — because on macOS `std::env::temp_dir()` is
//!   `/tmp`, a symlink, and an assertion against an unresolved prefix would
//!   compare two different spellings of the same directory (§42.3-M8 rule 7).
//! * `git init` / `git worktree add` appear ONLY in these fixtures. The product
//!   path runs two read-only commands and nothing else (§42.3-M9).
//! * Every test returns early when the `git` binary cannot be located, so a
//!   machine without git reports green rather than lying.

use noending::domain::{GitDetection, GitWorktreeKind};
use noending::platform::exec_resolver::resolve_executable;
use noending::platform::exec_runner;
use noending::workspace::home::NoEndingHome;
use noending::workspace::identity::{self, PathStyle};
use noending::workspace::resolver::{
    is_observable, GitAccess, GitProbe, GitProbeState, ReservedPaths, ResolverContext,
    WorkspaceObserving, WorkspaceResolver,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// A real, symlink-free working directory for one fixture.
fn scratch(tag: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let raw = std::env::temp_dir().join(format!(
        "noending-resolver-{tag}-{}-{n}",
        std::process::id()
    ));
    std::fs::create_dir_all(&raw).unwrap();
    let real = std::fs::canonicalize(&raw).unwrap_or(raw);
    // §42.3-M8 rule 7: never assert on a temp prefix without normalizing it.
    // The call below used to compute the normalized string and discard it, so
    // every fixture handed the resolver — and compared against — the raw
    // `fs::canonicalize` answer. On macOS that is harmless (`/private/var/…` is
    // already lexical); on Windows it is `\\?\C:\Users\…`, a verbatim API
    // spelling no NoEnding row ever carries, which is why five of these tests
    // were red on the Windows runner and none on the macOS one.
    let normalized =
        identity::normalize_path(&real.to_string_lossy()).expect("temp path normalizes");
    PathBuf::from(normalized)
}

fn git() -> Option<PathBuf> {
    resolve_executable("git")
}

/// `git` with the fixture-only identity overrides, so a machine with
/// `commit.gpgsign=true` or no global identity still builds a repository.
fn git_in(program: &Path, args: &[&str], cwd: &Path) -> Result<String, String> {
    let id = [
        "-c",
        "user.email=resolver@test.local",
        "-c",
        "user.name=Resolver Test",
        "-c",
        "commit.gpgsign=false",
        "-c",
        "core.autocrlf=false",
    ];
    let mut full: Vec<&str> = id.to_vec();
    full.extend_from_slice(args);
    let out = exec_runner::run_with_env(
        program,
        &full,
        Some(cwd),
        30,
        &noending::workspace::resolver::GIT_ENV,
    )
    .map_err(|e| e.to_string())?;
    if !out.success {
        return Err(format!("git {} failed: {}", args.join(" "), out.stderr));
    }
    Ok(out.stdout)
}

fn init_repo(program: &Path, dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git_in(program, &["init", "--quiet", "."], dir).expect("git init");
    git_in(
        program,
        &["commit", "--quiet", "--allow-empty", "-m", "init"],
        dir,
    )
    .expect("initial commit");
}

/// A resolver that never spawns anything but the fixture's own `git`.
fn resolver_for(user_home: Option<&Path>, reserved: ReservedPaths) -> WorkspaceResolver {
    WorkspaceResolver::new(ResolverContext {
        style: Some(PathStyle::current()),
        user_home: user_home.map(|p| p.to_string_lossy().to_string()),
        base: None,
        reserved,
        git: GitAccess::auto(),
    })
}

fn path_key_of(p: &Path) -> String {
    identity::path_key(&p.to_string_lossy())
}

/// 必须测试: a normal git repo.
#[test]
fn a_plain_repository_is_detected() {
    let Some(program) = git() else {
        eprintln!("skip: git not found");
        return;
    };
    let root = scratch("plain");
    let repo = root.join("noending");
    init_repo(&program, &repo);

    let resolver = resolver_for(None, ReservedPaths::default());
    let obs = resolver.observe(&repo.to_string_lossy());
    assert!(is_observable(&obs));
    assert!(obs.exists);
    assert_eq!(obs.path_id, identity::path_identity(&obs.canonical_path));
    let GitDetection::Detected {
        common_dir,
        toplevel,
        kind,
        worktrees,
    } = obs.git
    else {
        panic!("expected detection, got {:?}", obs.git);
    };
    assert_eq!(path_key_of(&repo), obs.canonical_path.replace('\\', "/"));
    assert_eq!(
        common_dir.replace('\\', "/"),
        format!("{}/.git", obs.canonical_path.replace('\\', "/"))
    );
    assert_eq!(toplevel.as_deref(), Some(obs.canonical_path.as_str()));
    assert_eq!(kind, GitWorktreeKind::Main);
    assert_eq!(worktrees.len(), 1, "one checkout");

    // A subdirectory reports the repository it is inside, and is not itself a
    // worktree — `Unknown` rather than a guessed `Main`.
    let subdir = repo.join("src-tauri").join("src");
    std::fs::create_dir_all(&subdir).unwrap();
    let sub = resolver.observe(&subdir.to_string_lossy());
    let GitDetection::Detected {
        common_dir: sub_common,
        toplevel: sub_toplevel,
        kind: sub_kind,
        ..
    } = sub.git
    else {
        panic!("expected detection in a subdir, got {:?}", sub.git);
    };
    assert_eq!(sub_common, common_dir, "same Git family");
    assert_eq!(sub_toplevel.as_deref(), toplevel.as_deref());
    assert_eq!(sub_kind, GitWorktreeKind::Main);

    std::fs::remove_dir_all(&root).ok();
}

/// 必须测试: a linked worktree.
#[test]
fn a_linked_worktree_is_detected_as_linked() {
    let Some(program) = git() else {
        eprintln!("skip: git not found");
        return;
    };
    let root = scratch("worktree");
    let main = root.join("main-checkout");
    init_repo(&program, &main);
    let linked = root.join("wt-windows");
    git_in(
        &program,
        &[
            "worktree",
            "add",
            "--detach",
            linked.to_str().unwrap(),
            "HEAD",
        ],
        &main,
    )
    .expect("git worktree add");

    let resolver = resolver_for(None, ReservedPaths::default());
    let obs = resolver.observe(&linked.to_string_lossy());
    let GitDetection::Detected {
        common_dir,
        kind,
        worktrees,
        ..
    } = &obs.git
    else {
        panic!("expected detection, got {:?}", obs.git);
    };
    assert_eq!(*kind, GitWorktreeKind::Linked);
    // The family identity is the MAIN checkout's `.git`, from which the linked
    // worktree is only a `.git` file away (§8.3 converges them this way).
    assert_eq!(
        path_key_of(&main.join(".git")),
        common_dir.replace('\\', "/"),
        "linked worktree shares the common dir"
    );
    let keys: Vec<String> = worktrees.iter().map(|w| identity::path_key(w)).collect();
    assert!(
        keys.contains(&path_key_of(&main)),
        "main worktree discovered: {keys:?}"
    );
    assert!(
        keys.contains(&path_key_of(&linked)),
        "linked worktree discovered: {keys:?}"
    );

    // Observing the main checkout of the same family says `Main`.
    let main_obs = resolver.observe(&main.to_string_lossy());
    match &main_obs.git {
        GitDetection::Detected { kind, .. } => assert_eq!(*kind, GitWorktreeKind::Main),
        other => panic!("{other:?}"),
    }

    std::fs::remove_dir_all(&root).ok();
}

/// 必须测试: `.git` missing. The resolver's answer is the raw evidence, `None`;
/// `git_state::MISSING` is derived by the caller from prior state (§1.3, §16-11).
#[test]
fn a_removed_git_directory_leaves_the_path_but_not_the_evidence() {
    let Some(program) = git() else {
        eprintln!("skip: git not found");
        return;
    };
    let root = scratch("missing");
    let repo = root.join("repo");
    init_repo(&program, &repo);

    let resolver = resolver_for(None, ReservedPaths::default());
    assert!(matches!(
        resolver.observe(&repo.to_string_lossy()).git,
        GitDetection::Detected { .. }
    ));

    // The whole point of §1.3: the path survives, the evidence does not.
    std::fs::remove_dir_all(repo.join(".git")).unwrap();
    let after = resolver.observe(&repo.to_string_lossy());
    assert!(is_observable(&after), "the WorkspacePath identity survives");
    assert_eq!(
        after.canonical_path,
        repo.to_string_lossy().replace('\\', "/")
    );
    assert_eq!(
        after.path_id,
        identity::path_identity(&repo.to_string_lossy()),
        "path_id is not a function of Git state"
    );
    assert_eq!(after.git, GitDetection::None);
    assert_eq!(
        noending::workspace::resolver::git_state_after_observation(
            noending::domain::git_state::DETECTED,
            &after.git
        ),
        noending::domain::git_state::MISSING
    );

    std::fs::remove_dir_all(&root).ok();
}

/// 必须测试: home `.git` exclusion, end to end against a real repository whose
/// toplevel IS the (fake) user Home.
#[test]
fn a_home_level_dotfiles_repository_is_not_evidence() {
    let Some(program) = git() else {
        eprintln!("skip: git not found");
        return;
    };
    let root = scratch("homegit");
    let fake_home = root.join("home");
    init_repo(&program, &fake_home);
    let nested = fake_home.join("code").join("noending");
    std::fs::create_dir_all(&nested).unwrap();

    let resolver = resolver_for(Some(&fake_home), ReservedPaths::default());

    // Without the §1.4 rule the dotfiles repo would swallow the whole Home.
    let obs = resolver.observe(&nested.to_string_lossy());
    assert!(
        is_observable(&obs),
        "the path itself is still a WorkspacePath"
    );
    assert_eq!(
        obs.git,
        GitDetection::None,
        "a repository rooted at the user Home is ignored (§1.4)"
    );
    // Including the Home directory itself, which is not a WorkspacePath at all.
    assert!(!is_observable(
        &resolver.observe(&fake_home.to_string_lossy())
    ));

    // A real repository INSIDE the dotfiles Home is still evidence: the rule
    // excludes the Home-level repo, not every repo under the Home.
    init_repo(&program, &nested);
    let nested_obs = resolver.observe(&nested.to_string_lossy());
    match &nested_obs.git {
        GitDetection::Detected { common_dir, .. } => {
            assert_eq!(
                path_key_of(&nested.join(".git")),
                common_dir.replace('\\', "/"),
                "own .git, not the Home's"
            );
        }
        other => panic!("nested repo must still be detected: {other:?}"),
    }

    std::fs::remove_dir_all(&root).ok();
}

/// 必须测试: reserved `~/.noending/data` exclusion and `~/.noending/workspace`
/// ALLOWED, on a real Home created by `ensure_dirs` (§42.3-M21).
#[test]
fn a_real_noending_home_reserves_app_paths_and_allows_workspace() {
    let root = scratch("home");
    let home = NoEndingHome::new(root.join(".noending").to_str().unwrap(), None).unwrap();
    home.ensure_dirs().unwrap();
    assert!(home.default_workspace.is_dir(), "must be launchable");
    assert!(home.data_dir.is_dir());

    let resolver = resolver_for(Some(&root), ReservedPaths::new(&home));
    let cases = [
        (".noending", false),
        (".noending/data", false),
        (".noending/data/noending.db", false),
        (".noending/runtime", false),
        (".noending/runtime/context-bundles", false),
        (".noending/logs", false),
        (".noending/workspace", true),
        (".noending/workspace/sub/project", true),
        (".noending/datax", true),
        (".noendingskin", true),
    ];
    for (rel, allowed) in cases {
        let raw = root.join(rel).to_string_lossy().to_string();
        let observed = is_observable(&resolver.observe(&raw));
        assert_eq!(observed, allowed, "{rel}: expected observable={allowed}");
    }

    // The allowed default workspace is a normal path with a normal identity,
    // and it is the directory `identity::auto_project_name` calls special.
    let ws = resolver.observe(home.default_workspace.to_str().unwrap());
    assert!(is_observable(&ws));
    assert!(ws.exists);
    assert_eq!(
        noending::workspace::identity::auto_project_name(
            &ws.canonical_path,
            Some(home.default_workspace.to_str().unwrap())
        ),
        "NoEnding Workspace"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// §42.3-M11: git must run IN the observed directory. The old `/tmp` fallback
/// would answer a question about the wrong directory — and on Windows `/tmp`
/// does not exist at all.
#[test]
fn the_child_process_runs_in_the_requested_directory() {
    let Some(program) = git() else {
        eprintln!("skip: git not found");
        return;
    };
    let root = scratch("cwd");
    let repo = root.join("r");
    init_repo(&program, &repo);

    let out = exec_runner::run(
        &program,
        &["rev-parse", "--show-toplevel"],
        Some(repo.as_path()),
        30,
    )
    .unwrap();
    assert!(out.success);
    assert_eq!(
        identity::path_key(out.stdout.trim()),
        identity::path_key(&repo.to_string_lossy()),
        "the answer is about the directory we named"
    );

    // A directory that is not there is an error from `run`, and an *observation*
    // from the resolver — never both, and never a silent /tmp fallback.
    let absent = root.join("does-not-exist");
    assert!(exec_runner::run(
        &program,
        &["rev-parse", "--git-common-dir"],
        Some(absent.as_path()),
        30
    )
    .is_err());
    let obs = resolver_for(None, ReservedPaths::default()).observe(&absent.to_string_lossy());
    assert!(!obs.exists);
    assert_eq!(obs.git, GitDetection::None);

    // `GitProbe` is the injected seam: an unavailable git must not be reached
    // through a process at all.
    let probe = GitAccess::unavailable().probe(&repo);
    assert_eq!(probe.state, GitProbeState::Unavailable);
    let scripted = GitProbe {
        state: GitProbeState::Repo,
        common_dir: Some(repo.join(".git").to_string_lossy().to_string()),
        toplevel: Some(repo.to_string_lossy().to_string()),
        worktrees: vec![],
    };
    assert!(matches!(
        noending::workspace::resolver::classify_git(
            &ResolverContext {
                user_home: Some(root.to_string_lossy().to_string()),
                ..ResolverContext::inert()
            },
            &identity::normalize_path(&repo.to_string_lossy()).unwrap(),
            &scripted
        ),
        GitDetection::Detected { .. }
    ));

    std::fs::remove_dir_all(&root).ok();
}

/// §16-12: observation only. Nothing a resolver run touches may change the
/// repository, and nothing it returns may be a Project.
#[test]
fn observing_is_not_mutation() {
    let Some(program) = git() else {
        eprintln!("skip: git not found");
        return;
    };
    let root = scratch("readonly");
    let repo = root.join("r");
    init_repo(&program, &repo);
    let before = read_tree(&repo).unwrap();

    let resolver = resolver_for(Some(&root), ReservedPaths::default());
    for _ in 0..3 {
        let _ = resolver.observe(&repo.to_string_lossy());
        let _ = resolver.observe(&repo.join("sub").to_string_lossy());
    }
    assert_eq!(before, read_tree(&repo).unwrap(), "no byte changed");
    std::fs::remove_dir_all(&root).ok();
}

/// A cheap fingerprint of a directory tree: names, sizes and file count.
fn read_tree(dir: &Path) -> std::io::Result<Vec<(String, u64)>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current)? {
            let entry = entry?;
            let path = entry.path();
            let meta = entry.file_type()?;
            if meta.is_dir() {
                stack.push(path);
            } else {
                let name = path
                    .strip_prefix(dir)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((name, entry.metadata()?.len()));
            }
        }
    }
    out.sort();
    Ok(out)
}
