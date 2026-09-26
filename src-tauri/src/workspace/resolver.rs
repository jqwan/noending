//! WorkspaceResolver — turns a path string into a [`WorkspaceObservation`].
//!
//! Observation only: this module must not create, reassign or delete anything.
//! Persistence and Project policy live in
//! `workspace::project`.
//!
//! ```text
//! WorkspaceObservation { canonical_path, path_id, exists, git }   // domain::WorkspaceObservation
//! ```
//!
//! * `canonical_path` / `path_id` come from [`crate::workspace::identity`] and
//!   nowhere else. `path_id` is `identity::path_identity(&canonical_path)`.
//! * `exists` is a plain `Path::is_dir()`. A missing directory is a legal
//!   observation — identity does not depend on it.
//! * `git` is one of `None | Detected{..} | Missing | Unavailable`. `Missing` is
//!   decided by the *caller's* prior state (a path that was `detected` and now
//!   reports no evidence), so the resolver reports the raw evidence and
//!   `workspace::project` maps it to `git_state`.
//!
//! ## Home-level Git is not evidence
//!
//! If `git root == user home` or `common dir == <home>/.git`, return
//! `GitDetection::None`. Without this a dotfiles repository swallows the entire
//! Home into one Project.
//!
//! ## Calling git
//!
//! * Resolve the binary through `platform::exec_resolver::resolve_executable`,
//!   never `Command::new("git")`.
//! * Run through `platform::exec_runner::run(..)` with the workspace path as an
//!   explicit `cwd`, plus `GIT_OPTIONAL_LOCKS=0` and `GIT_TERMINAL_PROMPT=0`.
//! * Only two commands are allowed:
//!   `git rev-parse --git-common-dir` (with `--show-toplevel` for the Home check)
//!   and `git worktree list --porcelain`.
//! * Every failure mode — no binary, timeout, non-zero exit, `dubious ownership`
//!   — collapses to `None` / `Unavailable`. It must never surface as a user
//!   visible error, because `AppError` serializes to a bare string and the UI
//!   cannot tell "not a repository" from "git is broken".
//! * `worktree list` output is discovery of *WorkspacePaths*; feeding it into any
//!   Workstream's path list is forbidden.
//!
//! ## What a caller must not read into this
//!
//! [`WorkspaceObserving::observe`] is total, so "this string is not a path at
//! all" has no `None` to return. It returns the sentinel built by
//! [`unobservable`] instead, and [`is_observable`] is the gate: an empty
//! `canonical_path` means reserved, un-normalizable, or the user Home
//! itself. `workspace::project::WorkspaceAttaching::ensure_path` must
//! therefore answer `Ok(None)` for it, never insert a row.

use std::path::{Path, PathBuf};

use crate::domain::{git_state, GitDetection, GitWorktreeKind, WorkspaceObservation};
use crate::platform::exec_resolver::resolve_executable;
use crate::platform::exec_runner;
use crate::workspace::identity::{self, NormalizeOpts, PathStyle};

// Re-exported because it appears in the public [`ResolverContext`] field, so a
// caller must be able to name the type through this module.
pub use crate::workspace::home::ReservedPaths;

/// The resolver seam. A `Db`-free trait so Project policy can be tested with a
/// scripted observation instead of a real repository.
pub trait WorkspaceObserving {
    fn observe(&self, raw_path: &str) -> WorkspaceObservation;

    /// The reason [`Self::observe`] answers the sentinel for `raw_path`. The
    /// default is the honest minimum for implementations that cannot name it;
    /// [`WorkspaceResolver`] refines it with [`classify_rejection`]. Read-only
    /// UI feedback — never consulted by acceptance policy.
    fn probe_rejection(&self, raw_path: &str) -> ProbeRejection {
        let _ = raw_path;
        ProbeRejection::Unresolvable
    }
}

/// The sentinel for "this string yields no WorkspacePath" — see the module docs
/// for why the trait cannot express it any other way.
pub fn unobservable() -> WorkspaceObservation {
    WorkspaceObservation {
        canonical_path: String::new(),
        path_id: String::new(),
        exists: false,
        git: GitDetection::None,
    }
}

/// `true` when `observe` produced a real observation. Every consumer of
/// [`WorkspaceObserving`] must check this before writing anything.
pub fn is_observable(obs: &WorkspaceObservation) -> bool {
    !obs.canonical_path.trim().is_empty() && !obs.path_id.trim().is_empty()
}

/// Why a raw string yields the "no WorkspacePath" sentinel: the three exact
/// faces of [`WorkspaceResolver::try_observe`]'s `None`, named for UI feedback.
/// This is a *report*, never policy — the attacher's `Ok(None)` stays the only
/// authority on whether a path is accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeRejection {
    /// Empty, un-normalizable, or a relative path with no base.
    Unresolvable,
    /// A reserved NoEnding Home path.
    Reserved,
    /// The user Home itself.
    Home,
}

/// Name the sentinel's cause for `raw`. Only meaningful when a fresh
/// `observe(raw)` actually produced the sentinel; the caller checks that first.
pub fn classify_rejection(ctx: &ResolverContext, raw: &str) -> ProbeRejection {
    let Some(canonical) = ctx.canonicalize(raw) else {
        return ProbeRejection::Unresolvable;
    };
    if ctx.reserved.contains_with(&canonical, ctx.style()) {
        return ProbeRejection::Reserved;
    }
    if ctx
        .user_home_canonical()
        .is_some_and(|home| ctx.same_location(&home, &canonical))
    {
        return ProbeRejection::Home;
    }
    // `observe` produced the sentinel without any of the three faces above
    // refusing it — say the honest minimum rather than inventing a fourth face.
    ProbeRejection::Unresolvable
}

/// Existence, observed in the one layer allowed to read the filesystem.
///
/// `workspace::project` never touches disk, but worktree adoption has to
/// record whether the directory it just learned about is actually present —
/// otherwise the Projects page says "目录不存在" about a directory that exists
/// until some later sweep corrects the row. It asks through
/// [`WorkspacePolicy::exists_on_disk`], and every real policy delegates here.
pub fn exists_on_disk(canonical_path: &str) -> bool {
    std::path::Path::new(canonical_path).is_dir()
}

/// How long a single `git` call may take. A hung credential prompt or a stalled
/// network mount must not stall Workspace Reconcile: the answer is
/// `Unavailable`, which is also what a missing binary produces.
pub const GIT_TIMEOUT_SECS: u64 = 10;

/// Environment for every `git` invocation.
///
/// `GIT_OPTIONAL_LOCKS=0` keeps us off the user's `index.lock`;
/// `GIT_TERMINAL_PROMPT=0` guarantees a credential prompt fails instead of
/// waiting forever; `LC_ALL=C` is what makes git's *stderr* readable by us at
/// all — a Chinese-locale git answers "fatal: 不是 Git 仓库…", and the
/// not-a-repository-vs-could-not-ask distinction below depends on that text.
/// `GIT_CONFIG_NOSYSTEM` is deliberately NOT set: the user's own configuration
/// is authoritative about their own repositories.
pub const GIT_ENV: [(&str, &str); 3] = [
    ("GIT_OPTIONAL_LOCKS", "0"),
    ("GIT_TERMINAL_PROMPT", "0"),
    ("LC_ALL", "C"),
];

/// How git answering (or not) actually went, before it is folded into a
/// [`GitDetection`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitProbeState {
    /// `rev-parse` found a repository.
    Repo,
    /// git ran and said there is nothing here (or the layout is unrecognised).
    NotARepository,
    /// git could not be consulted: no binary, timeout, unsafe directory,
    /// unexpected failure. Never presented as "not a repository".
    Unavailable,
}

/// One `worktree` block of `git worktree list --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorktreeEntry {
    /// `worktree <path>` — the working directory of this checkout.
    pub path: Option<String>,
    /// `gitdir <path>` — equals the common dir for the main checkout, which is
    /// how the main worktree is told apart from a linked one without guessing.
    pub gitdir: Option<String>,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub bare: bool,
    pub detached: bool,
}

/// Raw evidence from git. Strings are exactly as git printed them; canonicalizing
/// them is [`classify_git`]'s job (rule 6: `fs::canonicalize` may be
/// applied to git output, never to produce `canonical_path`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitProbe {
    pub state: GitProbeState,
    pub common_dir: Option<String>,
    pub toplevel: Option<String>,
    pub worktrees: Vec<WorktreeEntry>,
}

impl GitProbe {
    fn nothing(state: GitProbeState) -> Self {
        Self {
            state,
            common_dir: None,
            toplevel: None,
            worktrees: Vec::new(),
        }
    }
}

/// Where `git` is and how long it may take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitAccess {
    /// `None` means "we know there is no git" — every probe is then
    /// [`GitProbeState::Unavailable`], and no `Command::new` is ever reached.
    pub program: Option<PathBuf>,
    pub timeout_secs: u64,
}

impl Default for GitAccess {
    fn default() -> Self {
        Self::unavailable()
    }
}

impl GitAccess {
    /// Locate `git` through the same resolver the Agent CLIs use.
    pub fn auto() -> Self {
        Self {
            program: resolve_executable("git"),
            timeout_secs: GIT_TIMEOUT_SECS,
        }
    }

    /// A resolver that will report [`GitDetection::Unavailable`] for everything.
    /// Tests and `--offline` paths use this so no subprocess is spawned.
    pub fn unavailable() -> Self {
        Self {
            program: None,
            timeout_secs: GIT_TIMEOUT_SECS,
        }
    }

    pub fn is_available(&self) -> bool {
        self.program.is_some()
    }

    /// Ask git about `dir`. The two allowed read-only commands, no more.
    pub fn probe(&self, dir: &Path) -> GitProbe {
        let Some(program) = self.program.as_deref() else {
            return GitProbe::nothing(GitProbeState::Unavailable);
        };
        if !dir.is_dir() {
            // Nothing to ask: a missing directory has no repository either, and
            // running a child process to learn that is how a reconcile loop ends
            // up in a directory that is not there.
            return GitProbe::nothing(GitProbeState::NotARepository);
        }

        let rev = match exec_runner::run_with_env(
            program,
            &["rev-parse", "--git-common-dir", "--show-toplevel"],
            Some(dir),
            self.timeout_secs,
            &GIT_ENV,
        ) {
            Ok(out) => out,
            Err(_) => return GitProbe::nothing(GitProbeState::Unavailable),
        };
        // `rev-parse` prints in argument order. `--show-toplevel` fails on a bare
        // repository while `--git-common-dir` already answered, so a short output
        // is parsed rather than discarded.
        let lines: Vec<&str> = rev
            .stdout
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        if !rev.success && lines.is_empty() {
            return GitProbe::nothing(if is_not_a_repository(&rev.stderr) {
                GitProbeState::NotARepository
            } else {
                GitProbeState::Unavailable
            });
        }
        let Some(common_dir) = lines.first().map(|s| s.to_string()) else {
            return GitProbe::nothing(GitProbeState::NotARepository);
        };
        let toplevel = lines.get(1).map(|s| s.to_string());

        // worktree discovery is part of the same observation. A failure here
        // is not fatal — the repository is still detected with one worktree.
        let worktrees = match exec_runner::run_with_env(
            program,
            &["worktree", "list", "--porcelain"],
            Some(dir),
            self.timeout_secs,
            &GIT_ENV,
        ) {
            Ok(out) if out.success => parse_worktree_list(&out.stdout),
            _ => Vec::new(),
        };

        GitProbe {
            state: GitProbeState::Repo,
            common_dir: Some(common_dir),
            toplevel,
            worktrees,
        }
    }
}

/// Everything that makes observation deterministic and injectable. No ambient
/// state is read except through the explicit constructors, so the rule
/// ("tests never resolve the real user Home") holds by construction.
#[derive(Debug, Clone, Default)]
pub struct ResolverContext {
    /// Separator rules. `None` = the host's.
    pub style: Option<PathStyle>,
    /// The user's home directory: expands `~`, and needs it to recognize a
    /// dotfiles repository.
    pub user_home: Option<String>,
    /// Base for relative paths. `None` means a relative input is not observable
    /// rather than being guessed against the process cwd (rule 2).
    pub base: Option<String>,
    /// The reserved set; empty means "NoEnding Home is unknown", which
    /// reserves nothing rather than reserving everything.
    pub reserved: ReservedPaths,
    pub git: GitAccess,
}

impl ResolverContext {
    /// Nothing ambient, git unavailable: the shape tests start from.
    pub fn inert() -> Self {
        Self {
            style: None,
            user_home: None,
            base: None,
            reserved: ReservedPaths::default(),
            git: GitAccess::unavailable(),
        }
    }

    /// The production context: ambient user home plus a located `git`.
    pub fn from_environment(reserved: ReservedPaths) -> Self {
        Self {
            style: None,
            user_home: identity::home_dir().map(|h| h.to_string_lossy().to_string()),
            base: None,
            reserved,
            git: GitAccess::auto(),
        }
    }

    pub fn style(&self) -> PathStyle {
        self.style.unwrap_or_else(PathStyle::current)
    }

    fn opts(&self) -> NormalizeOpts<'_> {
        NormalizeOpts {
            style: Some(self.style()),
            base: self.base.as_deref(),
            home: self.user_home.as_deref(),
        }
    }

    /// `canonical_path` for a raw string, or `None` when it cannot be one.
    pub fn canonicalize(&self, raw: &str) -> Option<String> {
        tidy(identity::normalize_path_with(raw, self.opts()))
    }

    /// The user's Home in canonical form, used by the exclusions.
    pub fn user_home_canonical(&self) -> Option<String> {
        self.user_home.as_deref().and_then(|h| {
            tidy(identity::normalize_path_with(
                h,
                NormalizeOpts {
                    style: Some(self.style()),
                    base: None,
                    home: None,
                },
            ))
        })
    }

    /// Canonical form for a path git printed, resolved against `relative_to`.
    ///
    /// git answers `--git-common-dir` *relative to the directory it ran in*
    /// (`.git` at the root, `../../../.git` from a deep subdirectory, and the
    /// literal `.` for a bare repository). Those `..` are real hops out of
    /// `relative_to`, which is **not** what [`NormalizeOpts::base`] means: a base
    /// is a clamp (`../x` against base `/a/b` is `/a/b/x`, per
    /// `identity::tests::relative_path_needs_an_explicit_base`). So a relative
    /// answer is joined onto the directory first and only then normalized,
    /// because at that point it is an absolute path and `..` collapses against
    /// real segments.
    pub fn canonicalize_from(&self, raw: &str, relative_to: &str) -> Option<String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        if let Some(absolute) = self.normalize_rooted(raw, None) {
            return Some(absolute);
        }
        let sep = if self.style().is_windows() { '\\' } else { '/' };
        let base = relative_to.trim_end_matches(['/', '\\']);
        let base = if base.is_empty() { relative_to } else { base };
        self.normalize_rooted(&format!("{base}{sep}{raw}"), None)
    }

    /// Are these two `canonical_path`s the *same location*?
    ///
    /// Not `==`, and not `path_key`: on a Windows volume case is not part of
    /// location, and the two exclusions are gates rather than memberships. A
    /// case-sensitive read there lets a dotfiles repository whose `toplevel`
    /// git spelled as `C:\Users\ME` slip past, which then makes the entire user
    /// Home one Project — the exact outcome the exclusion exists to forbid.
    /// There is no later convergence to save it, because the Home is not in a
    /// Git family.
    pub fn same_location(&self, a: &str, b: &str) -> bool {
        let style = self.style();
        identity::identity_key(a, style) == identity::identity_key(b, style)
    }

    fn normalize_rooted(&self, raw: &str, base: Option<&str>) -> Option<String> {
        tidy(identity::normalize_path_with(
            raw,
            NormalizeOpts {
                style: Some(self.style()),
                base,
                home: self.user_home.as_deref(),
            },
        ))
    }
}

/// Drop the trailing separator that `identity::join` leaves on an input whose
/// segments all collapse away — `.` from a bare repository's
/// `--git-common-dir`, or a `/x/y/.` cwd — because otherwise the same directory
/// gets two different `canonical_path` values, and `canonical_path` is a UNIQUE
/// identity column.
///
/// Roots keep theirs: `C:\` is the drive root, and `identity` deliberately
/// returns it with the separator (`C:` alone means drive-*relative*).
fn tidy(canonical: Option<String>) -> Option<String> {
    let s = canonical?;
    if s.len() <= 1 {
        return Some(s);
    }
    let bytes = s.as_bytes();
    let last = bytes[bytes.len() - 1];
    if last != b'/' && last != b'\\' {
        return Some(s);
    }
    let before = bytes[bytes.len() - 2];
    if before == b':' {
        return Some(s); // `C:\`
    }
    Some(s[..bytes.len() - 1].to_string())
}

/// The filesystem-backed resolver.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceResolver {
    pub ctx: ResolverContext,
}

impl WorkspaceResolver {
    pub fn new(ctx: ResolverContext) -> Self {
        Self { ctx }
    }

    /// The fallible half of [`WorkspaceObserving::observe`]: `None` for a string
    /// that is not a WorkspacePath at all (un-normalizable, reserved, or
    /// the user Home itself).
    pub fn try_observe(&self, raw: &str) -> Option<WorkspaceObservation> {
        let canonical = self.ctx.canonicalize(raw)?;
        // The reservation question is asked in the context's own style, not the
        // host's: an injected `PathStyle::Windows` must fold exactly like a
        // Windows host does, or the seam only half-applies.
        if self
            .ctx
            .reserved
            .contains_with(&canonical, self.ctx.style())
        {
            return None;
        }
        // Second face: never let the user's Home become a Project. The
        // repositories *inside* it are unaffected — they are separate paths with
        // their own evidence.
        if self
            .ctx
            .user_home_canonical()
            .is_some_and(|home| self.ctx.same_location(&home, &canonical))
        {
            return None;
        }

        let path = PathBuf::from(&canonical);
        let exists = path.is_dir();
        let git = if exists {
            classify_git(&self.ctx, &canonical, &self.ctx.git.probe(&path))
        } else {
            // A missing directory is a legal observation; there is
            // simply nothing to detect in it, and no child process to ask.
            GitDetection::None
        };
        Some(WorkspaceObservation {
            path_id: identity::path_identity(&canonical),
            canonical_path: canonical,
            exists,
            git,
        })
    }
}

impl WorkspaceObserving for WorkspaceResolver {
    fn observe(&self, raw_path: &str) -> WorkspaceObservation {
        match self.try_observe(raw_path) {
            Some(obs) => obs,
            None => unobservable(),
        }
    }

    fn probe_rejection(&self, raw_path: &str) -> ProbeRejection {
        classify_rejection(&self.ctx, raw_path)
    }
}

/// Pure half of Git detection: raw git output in, [`GitDetection`] out, with the
/// Home exclusions applied. No filesystem, no process — this is what makes
/// the dotfiles-swallowing-the-Home case testable without a repository.
pub fn classify_git(ctx: &ResolverContext, observed: &str, probe: &GitProbe) -> GitDetection {
    match probe.state {
        GitProbeState::Unavailable => GitDetection::Unavailable,
        GitProbeState::NotARepository => GitDetection::None,
        GitProbeState::Repo => {
            let Some(common_raw) = probe.common_dir.as_deref() else {
                return GitDetection::None;
            };
            let Some(common_dir) = ctx.canonicalize_from(common_raw, observed) else {
                return GitDetection::Unavailable;
            };
            // `--show-toplevel` is absent for a bare repository; the parent of a
            // `<dir>/.git` common dir is the toplevel for every non-bare one, and
            // nothing is inferred for a bare repository (which has no checkout).
            let toplevel = probe
                .toplevel
                .as_deref()
                .and_then(|t| ctx.canonicalize_from(t, observed))
                .or_else(|| {
                    parent_of(&common_dir).and_then(|p| ctx.canonicalize_from(&p, observed))
                });

            // A repository rooted at the user's Home is not evidence.
            if let Some(home) = ctx.user_home_canonical() {
                let same = |a: &str, b: &str| ctx.same_location(a, b);
                if toplevel.as_deref().is_some_and(|t| same(&t, &home)) {
                    return GitDetection::None;
                }
                if let Some(home_git) = ctx.canonicalize_from(&home_git_of(&home), &home) {
                    if same(&common_dir, &home_git) {
                        return GitDetection::None;
                    }
                }
            }

            let worktrees: Vec<String> = probe
                .worktrees
                .iter()
                .filter(|e| !e.bare)
                .filter_map(|e| {
                    e.path
                        .as_deref()
                        .and_then(|p| ctx.canonicalize_from(p, observed))
                })
                .collect();
            let kind = worktree_kind(
                ctx,
                observed,
                toplevel.as_deref(),
                &common_dir,
                &probe.worktrees,
            );
            GitDetection::Detected {
                common_dir,
                toplevel,
                kind,
                worktrees,
            }
        }
    }
}

/// Which checkout the observed path belongs to.
///
/// `gitdir == common dir` identifies the main worktree, which is a fact of git's
/// own layout rather than a heuristic about path names or ordering. When
/// `worktree list` produced nothing, the same structural question is asked of the
/// common dir instead: `<toplevel>/.git` IS the main checkout's common dir, while
/// a linked worktree's common dir belongs to a *different* directory. Anything
/// that fits neither shape (a bare repository, a submodule) is `Unknown`.
pub fn worktree_kind(
    ctx: &ResolverContext,
    observed: &str,
    toplevel: Option<&str>,
    common_dir: &str,
    entries: &[WorktreeEntry],
) -> GitWorktreeKind {
    // A location comparison, not a spelling comparison: `git worktree list`
    // re-prints paths in its own casing, and on a Windows volume a case variant
    // of the observed root IS the observed root. Answering "no match" there
    // mislabels `git_kind`, which is an observation the user sees.
    let key = |s: &str| identity::identity_key(s, ctx.style());
    let root = toplevel.unwrap_or(observed);
    let matching_index = entries.iter().position(|e| {
        e.path
            .as_deref()
            .and_then(|p| ctx.canonicalize_from(p, observed))
            .is_some_and(|p| ctx.same_location(&p, root))
    });
    let matching = matching_index.and_then(|i| entries.get(i));
    if let Some(e) = matching {
        // A bare repository has no separate checkout to be "linked" from.
        if e.bare {
            return GitWorktreeKind::Main;
        }
        // `gitdir` is authoritative when git prints it: the main worktree's
        // gitdir IS the common dir.
        if let Some(gitdir) = e
            .gitdir
            .as_deref()
            .and_then(|g| ctx.canonicalize_from(g, observed))
        {
            return if key(&gitdir) == key(common_dir) {
                GitWorktreeKind::Main
            } else {
                GitWorktreeKind::Linked
            };
        }
        // git 2.54 does not print `gitdir` at all, so fall back on the listing
        // order: `worktree list` reports the main worktree first regardless of
        // which worktree the command ran in (verified against a linked one).
        return if matching_index == Some(0) {
            GitWorktreeKind::Main
        } else {
            GitWorktreeKind::Linked
        };
    }
    if !entries.is_empty() || toplevel.is_none() {
        // Either the listing did not include this checkout (a path inside a
        // submodule, an unusual git build) or there is no toplevel to compare
        // with. Say we do not know rather than guess `Main`.
        return GitWorktreeKind::Unknown;
    }
    // The only structure that *proves* a main checkout without a listing: git
    // keeps a repository's own metadata in `<toplevel>/.git`. Anything else
    // (`.git/worktrees/<name>`, `.git/modules/<name>`) stays `Unknown` —
    // `Linked` is only ever reported from real `worktree list` evidence.
    match parent_of(common_dir) {
        Some(main_root) if key(&main_root) == key(toplevel.unwrap_or(observed)) => {
            GitWorktreeKind::Main
        }
        _ => GitWorktreeKind::Unknown,
    }
}

/// `git worktree list --porcelain`.
///
/// Format: blocks starting with `worktree <path>`, followed by `HEAD`,
/// `branch`, optional `bare` / `detached`, and `gitdir`. A path may contain
/// spaces, so the value is everything after the single separating space — and a
/// block with no `worktree` line is ignored rather than guessed at.
pub fn parse_worktree_list(porcelain: &str) -> Vec<WorktreeEntry> {
    let mut out: Vec<WorktreeEntry> = Vec::new();
    let mut current: Option<WorktreeEntry> = None;
    for line in porcelain.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let Some(entry) = current.take() {
                out.push(entry);
            }
            let mut entry = WorktreeEntry::default();
            entry.path = Some(rest.to_string());
            current = Some(entry);
            continue;
        }
        let Some(entry) = current.as_mut() else {
            continue;
        };
        // `bare` and `detached` are valueless flags; the keyed lines carry one
        // space-separated value which may itself contain anything else.
        if line == "bare" {
            entry.bare = true;
            continue;
        }
        if line == "detached" {
            entry.detached = true;
            continue;
        }
        match line.split_once(' ') {
            Some(("HEAD", value)) => entry.head = Some(value.trim().to_string()),
            Some(("branch", value)) => entry.branch = Some(value.trim().to_string()),
            Some(("gitdir", value)) => entry.gitdir = Some(value.trim().to_string()),
            // Forward compatibility: an unknown key is skipped, never fatal.
            _ => {}
        }
    }
    if let Some(entry) = current.take() {
        out.push(entry);
    }
    out
}

/// Fold an observation into a stored `git_state`, for the caller that owns
/// persistence: losing `.git` is `detected → missing`, and nothing else.
/// `workspace::project` is the only place with the prior state this needs, so the
/// resolver reports `None` and stops.
///
/// [`GitProbeState::Unavailable`] keeps the stored value on purpose: "we could
/// not ask" must never be recorded as "this is not a repository", because losing
/// a Project's Git identity on a transient failure is forbidden.
pub fn git_state_after_observation(prior: &str, detection: &GitDetection) -> String {
    match detection {
        GitDetection::Detected { .. } => git_state::DETECTED.to_string(),
        GitDetection::Missing => git_state::MISSING.to_string(),
        // "No evidence right now" keeps a path that was once Git-backed in
        // `missing`: a lost `.git` must not detach the WorkspacePath or clear
        // anything, and must not forget that it was ever a repository. Demoting
        // `missing → none` would erase that fact.
        GitDetection::None if prior == git_state::DETECTED || prior == git_state::MISSING => {
            git_state::MISSING.to_string()
        }
        GitDetection::None => git_state::NONE.to_string(),
        GitDetection::Unavailable => prior.to_string(),
    }
}

/// The checkout a `<dir>/.git` common dir belongs to. Nothing is inferred for a
/// bare repository (`/srv/repo.git` has no working tree), because inventing a
/// toplevel there is how a wrong Project gets created.
fn parent_of(common_dir: &str) -> Option<String> {
    let trimmed = identity::path_key(common_dir)
        .trim_end_matches('/')
        .to_string();
    let parent = trimmed.strip_suffix("/.git")?;
    Some(if parent.is_empty() {
        "/".to_string()
    } else {
        parent.to_string()
    })
}

/// `<home>/.git`, the exact spelling.
fn home_git_of(home: &str) -> String {
    format!("{}.git", home.trim_end_matches(['/', '\\']))
}

/// `dubious ownership` (CVE-2022-24765 `safe.directory` refusal) and a plain
/// "not a repository" both make git exit non-zero, and only one of them is an
/// answer about the directory. Anything we cannot name stays `Unavailable`.
fn is_not_a_repository(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("not a git repository") || lower.contains("outside repository")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ResolverContext {
        ResolverContext {
            style: Some(PathStyle::Unix),
            user_home: Some("/Users/tester".to_string()),
            base: None,
            reserved: ReservedPaths::default(),
            git: GitAccess::unavailable(),
        }
    }

    fn detected_probe(common: &str, toplevel: Option<&str>) -> GitProbe {
        GitProbe {
            state: GitProbeState::Repo,
            common_dir: Some(common.to_string()),
            toplevel: toplevel.map(str::to_string),
            worktrees: Vec::new(),
        }
    }

    /// Home `.git` exclusion — both spellings, and the near-miss that must NOT
    /// be excluded.
    #[test]
    fn home_level_repository_is_not_evidence() {
        let c = ctx();
        // `git root == user home`: the dotfiles repository.
        let probe = detected_probe("/Users/tester/.git", Some("/Users/tester"));
        assert_eq!(
            classify_git(&c, "/Users/tester", &probe),
            GitDetection::None,
            "a Home-level repository must not become a Project"
        );
        // A directory *inside* the dotfiles repo: toplevel is still the Home, so
        // the evidence is dropped and the path stays ordinary.
        assert_eq!(
            classify_git(
                &c,
                "/Users/tester/code/x",
                &detected_probe("/Users/tester/.git", Some("/Users/tester"))
            ),
            GitDetection::None
        );
        // common dir spelled as the Home itself (bare-ish layout) is also out.
        assert_eq!(
            classify_git(
                &c,
                "/Users/tester",
                &detected_probe("/Users/tester/.git", None)
            ),
            GitDetection::None
        );
        // A repository whose toplevel merely *lives under* the Home is fine.
        match classify_git(
            &c,
            "/Users/tester/code/noending",
            &detected_probe(
                "/Users/tester/code/noending/.git",
                Some("/Users/tester/code/noending"),
            ),
        ) {
            GitDetection::Detected {
                common_dir,
                toplevel,
                kind,
                worktrees,
            } => {
                assert_eq!(common_dir, "/Users/tester/code/noending/.git");
                assert_eq!(toplevel.as_deref(), Some("/Users/tester/code/noending"));
                assert_eq!(kind, GitWorktreeKind::Main);
                assert!(worktrees.is_empty());
            }
            other => panic!("{other:?}"),
        }
        // `/Users/testerling/.git` is a different user, not the Home.
        assert!(matches!(
            classify_git(
                &c,
                "/Users/testerling",
                &detected_probe("/Users/testerling/.git", Some("/Users/testerling"))
            ),
            GitDetection::Detected { .. }
        ));
    }

    /// A normal git repo / a linked worktree, from real porcelain.
    #[test]
    fn worktree_porcelain_parsing_and_kinds() {
        let main_only = "\
worktree /Users/me/code/noending
HEAD 4a1b
branch refs/heads/main
gitdir /Users/me/code/noending/.git/worktrees/noending
"
        .to_string();
        // A main checkout's `gitdir` IS the common dir; the fixture above is the
        // linked spelling, so exercise both from one listing.
        let listing = "worktree /Users/me/code/noending
HEAD 4a1b
branch refs/heads/main
gitdir /Users/me/code/noending/.git

worktree /Users/me/worktrees/noending/windows
HEAD 9f2c
branch refs/heads/windows
detached
gitdir /Users/me/code/noending/.git/worktrees/windows

worktree /Volumes/nas/shared
HEAD 1111
bare
";
        let entries = parse_worktree_list(listing);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path.as_deref(), Some("/Users/me/code/noending"));
        assert_eq!(entries[1].branch.as_deref(), Some("refs/heads/windows"));
        assert!(entries[1].detached);
        assert!(entries[2].bare);
        assert_eq!(entries[2].path.as_deref(), Some("/Volumes/nas/shared"));
        assert_eq!(parse_worktree_list(&main_only).len(), 1);

        let c = ctx();
        let observed = "/Users/me/worktrees/noending/windows";
        let probe = GitProbe {
            state: GitProbeState::Repo,
            common_dir: Some("/Users/me/code/noending/.git".to_string()),
            toplevel: Some(observed.to_string()),
            worktrees: entries.clone(),
        };
        match classify_git(&c, observed, &probe) {
            GitDetection::Detected {
                kind,
                worktrees,
                common_dir,
                ..
            } => {
                assert_eq!(kind, GitWorktreeKind::Linked);
                assert_eq!(common_dir, "/Users/me/code/noending/.git");
                // Bare repositories are not working directories, so they are not
                // WorkspacePath candidates.
                assert_eq!(
                    worktrees,
                    vec!["/Users/me/code/noending".to_string(), observed.to_string()]
                );
            }
            other => panic!("{other:?}"),
        }
        // The same listing, observed at the main checkout.
        let probe_main = GitProbe {
            state: GitProbeState::Repo,
            common_dir: Some("/Users/me/code/noending/.git".to_string()),
            toplevel: Some("/Users/me/code/noending".to_string()),
            worktrees: entries,
        };
        match classify_git(&c, "/Users/me/code/noending", &probe_main) {
            GitDetection::Detected { kind, .. } => assert_eq!(kind, GitWorktreeKind::Main),
            other => panic!("{other:?}"),
        }
        // An unreadable listing (empty `worktree list`) still detects the repo,
        // and the layout of the common dir answers the main-vs-linked question.
        let probe_nolist = GitProbe {
            state: GitProbeState::Repo,
            common_dir: Some("/repo/.git".to_string()),
            toplevel: Some("/repo".to_string()),
            worktrees: Vec::new(),
        };
        match classify_git(&c, "/repo", &probe_nolist) {
            GitDetection::Detected { kind, .. } => assert_eq!(kind, GitWorktreeKind::Main),
            other => panic!("{other:?}"),
        }
        let probe_linked_nolist = GitProbe {
            state: GitProbeState::Repo,
            common_dir: Some("/repo/.git/worktrees/topic".to_string()),
            toplevel: Some("/worktrees/topic".to_string()),
            worktrees: Vec::new(),
        };
        match classify_git(&c, "/worktrees/topic", &probe_linked_nolist) {
            // Without a listing there is no proof this is a *linked* checkout, so
            // the answer is Unknown rather than a shape-guess. `Linked` is only
            // ever produced from real `worktree list` evidence.
            GitDetection::Detected { kind, .. } => assert_eq!(kind, GitWorktreeKind::Unknown),
            other => panic!("{other:?}"),
        }
        // A submodule's common dir matches neither shape: unknown, not guessed.
        let probe_submodule = GitProbe {
            state: GitProbeState::Repo,
            common_dir: Some("/parent/.git/modules/vendor/dep".to_string()),
            toplevel: Some("/parent/vendor/dep".to_string()),
            worktrees: Vec::new(),
        };
        match classify_git(&c, "/parent/vendor/dep", &probe_submodule) {
            GitDetection::Detected {
                kind, worktrees, ..
            } => {
                assert_eq!(kind, GitWorktreeKind::Unknown);
                assert!(worktrees.is_empty());
            }
            other => panic!("{other:?}"),
        }
    }

    /// Paths git hands back relative to the cwd must be resolved against the
    /// observed directory, not the process (rule 6).
    #[test]
    fn git_output_is_resolved_against_the_observed_path() {
        let c = ctx();
        let probe = detected_probe(".git", Some("/Users/me/repo"));
        match classify_git(&c, "/Users/me/repo/sub/dir", &probe) {
            GitDetection::Detected { common_dir, .. } => {
                assert_eq!(common_dir, "/Users/me/repo/sub/dir/.git")
            }
            other => panic!("{other:?}"),
        }
        // Mixed separators from git on a Windows volume normalize like any other
        // input: `/` becomes `\`.
        let w = ResolverContext {
            style: Some(PathStyle::Windows),
            user_home: Some("C:\\Users\\me".to_string()),
            ..ResolverContext::inert()
        };
        let probe = detected_probe("C:/Users/me/repo/.git", Some("C:/Users/me/repo"));
        match classify_git(&w, "C:\\Users\\me\\repo", &probe) {
            GitDetection::Detected {
                common_dir,
                toplevel,
                ..
            } => {
                assert_eq!(common_dir, "C:\\Users\\me\\repo\\.git");
                assert_eq!(toplevel.as_deref(), Some("C:\\Users\\me\\repo"));
            }
            other => panic!("{other:?}"),
        }
        // A repository rooted at the Windows Home is excluded just like on Unix.
        assert_eq!(
            classify_git(
                &w,
                "C:\\Users\\me\\doc",
                &detected_probe("C:\\Users\\me\\.git", Some("C:\\Users\\me"))
            ),
            GitDetection::None
        );
    }

    /// Missing `.git` — the resolver's raw answer is `None`, and only the
    /// caller's prior state turns it into `missing`.
    #[test]
    fn missing_git_is_the_callers_derivation_not_the_resolvers() {
        let c = ctx();
        let none = classify_git(
            &c,
            "/Users/me/repo",
            &GitProbe::nothing(GitProbeState::NotARepository),
        );
        assert_eq!(none, GitDetection::None);
        assert_eq!(
            git_state_after_observation(git_state::DETECTED, &none),
            git_state::MISSING
        );
        assert_eq!(
            git_state_after_observation(git_state::NONE, &none),
            git_state::NONE
        );
        assert_eq!(
            git_state_after_observation(git_state::MISSING, &none),
            git_state::MISSING,
            "already-missing stays missing"
        );
        // losing Git must not detach a path from its Project, so a value
        // that was never detected is never reported as `missing`.
        assert_eq!(
            git_state_after_observation(git_state::DETECTED, &GitDetection::None),
            git_state::MISSING
        );
        // "Could not ask" changes nothing at all — the whole point of keeping the
        // variant distinct.
        assert_eq!(
            git_state_after_observation(git_state::DETECTED, &GitDetection::Unavailable),
            git_state::DETECTED
        );
        assert_eq!(
            git_state_after_observation(git_state::NONE, &GitDetection::Unavailable),
            git_state::NONE
        );
        assert_eq!(
            git_state_after_observation(
                git_state::NONE,
                &GitDetection::Detected {
                    common_dir: "/r/.git".into(),
                    toplevel: None,
                    kind: GitWorktreeKind::Main,
                    worktrees: vec![],
                }
            ),
            git_state::DETECTED
        );
    }

    /// Every way git can fail collapses into an enum, never an error.
    #[test]
    fn git_failures_never_bubble_up() {
        let c = ctx();
        // No binary at all — observed on a directory that really exists, so the
        // answer is "could not ask" rather than "nothing there".
        let resolver = WorkspaceResolver::new(ResolverContext {
            git: GitAccess::unavailable(),
            ..c.clone()
        });
        let real_dir = identity::normalize_path(&std::env::temp_dir().to_string_lossy())
            .expect("temp dir normalizes");
        let obs = resolver.observe(&real_dir);
        assert_eq!(obs.git, GitDetection::Unavailable);
        assert!(is_observable(&obs));
        assert!(obs.exists);
        // A directory that does not exist is not even asked.
        let absent = format!("{real_dir}/noending-absent-9a7f2c");
        let missing = resolver.observe(&absent);
        assert!(!missing.exists);
        assert_eq!(missing.git, GitDetection::None);
        assert!(is_observable(&missing), "a missing directory is legal");
        // Not-a-repository stderr vs. everything else.
        assert!(is_not_a_repository(
            "fatal: not a git repository (or any of the parent directories): .git"
        ));
        assert!(!is_not_a_repository(
            "fatal: detected dubious ownership in repository at '/Users/me/repo'"
        ));
        assert!(!is_not_a_repository("error: unknown option"));
        // Un-normalizable input, the user Home, and reserved paths are all the
        // same sentinel.
        let sentinel = resolver.observe("relative/without/base");
        assert!(!is_observable(&sentinel));
        assert!(matches!(sentinel.git, GitDetection::None));
    }

    /// Relative → absolute (rejected without a base) and the Home / reserved
    /// exclusions, seen through the resolver rather than the predicate.
    ///
    /// The style is pinned, not inherited: a lexical test says which platform's
    /// spelling it means, so both are proven on every runner instead of one of
    /// them.
    #[test]
    fn normalization_and_exclusions_end_to_end() {
        let home = crate::workspace::home::NoEndingHome::new_with_style(
            "/Users/tester/.noending",
            Some("/Users/tester"),
            PathStyle::Unix,
        )
        .unwrap();
        let resolver = WorkspaceResolver::new(ResolverContext {
            style: Some(PathStyle::Unix),
            user_home: Some("/Users/tester".to_string()),
            reserved: home.reserved(),
            ..ResolverContext::inert()
        });

        assert!(resolver.observe("relative/path").is_none_sentinel());
        assert!(resolver
            .observe("/Users/tester/.noending/data")
            .is_none_sentinel());
        assert!(resolver
            .observe("/Users/tester/.noending")
            .is_none_sentinel());
        assert!(
            resolver.observe("/Users/tester").is_none_sentinel(),
            "sentinel"
        );
        assert!(!resolver
            .observe("/Users/tester/.noending/workspace")
            .is_none_sentinel());
        // Same directory, three spellings, one identity.
        let a = resolver.observe("/Users/tester/code/noending");
        let b = resolver.observe("/Users/tester/code/x/../noending/");
        let c = resolver.observe("~/code/noending");
        assert_eq!(a.canonical_path, "/Users/tester/code/noending");
        assert_eq!(a.canonical_path, b.canonical_path);
        assert_eq!(a.canonical_path, c.canonical_path);
        assert_eq!(a.path_id, b.path_id);
        assert_eq!(
            a.path_id,
            identity::path_identity("/Users/tester/code/noending")
        );

        // With a base, a relative path becomes an absolute one instead of a guess.
        let based = WorkspaceResolver::new(ResolverContext {
            base: Some("/Users/tester/projects".to_string()),
            ..resolver.ctx.clone()
        });
        assert_eq!(
            based.observe("noending/src").canonical_path,
            "/Users/tester/projects/noending/src"
        );

        // The same story in Windows spelling, with hand-written expectations —
        // "what a Windows host produces" is the half this test would otherwise
        // leave to whichever runner happened to execute it.
        let win_home = crate::workspace::home::NoEndingHome::new_with_style(
            "C:\\Users\\tester\\.noending",
            Some("C:\\Users\\tester"),
            PathStyle::Windows,
        )
        .unwrap();
        let win = WorkspaceResolver::new(ResolverContext {
            style: Some(PathStyle::Windows),
            user_home: Some("C:\\Users\\tester".to_string()),
            reserved: win_home.reserved(),
            ..ResolverContext::inert()
        });
        assert!(win.observe("relative/path").is_none_sentinel());
        assert!(win
            .observe("C:\\Users\\tester\\.noending\\data")
            .is_none_sentinel());
        assert!(
            win.observe("C:\\USERS\\tester\\.NoEnding")
                .is_none_sentinel(),
            "a case variant of the Home is still the Home"
        );
        assert!(
            win.observe("C:\\Users\\TESTER").is_none_sentinel(),
            "folds case too"
        );
        let repo = win.observe("c:/tester/code/../code/noending\\");
        assert_eq!(repo.canonical_path, "C:\\tester\\code\\noending");
        // The stored key is the host's rule, deliberately: a database belongs to
        // one machine, so an injected *spelling* must not decide what its ids
        // mean. That is why the Windows identity convergence itself is asserted
        // by the key tests and the `#[cfg(windows)]` registry tests, not here.
        assert_eq!(repo.path_id, identity::path_identity(&repo.canonical_path));
    }

    /// Small helper so the sentinel reads clearly in the assertions above.
    trait Sentinel {
        fn is_none_sentinel(&self) -> bool;
    }
    impl Sentinel for WorkspaceObservation {
        fn is_none_sentinel(&self) -> bool {
            !is_observable(self)
        }
    }

    #[test]
    fn git_env_is_the_locked_down_pair() {
        assert!(GIT_ENV.contains(&("GIT_OPTIONAL_LOCKS", "0")));
        assert!(GIT_ENV.contains(&("GIT_TERMINAL_PROMPT", "0")));
        // Messages are parsed, so they must be in a language we can parse.
        assert!(GIT_ENV.contains(&("LC_ALL", "C")));
        assert!(!GIT_ENV.iter().any(|(k, _)| *k == "GIT_CONFIG_NOSYSTEM"));
        // Only the two read-only commands exist in this module; assert the binary
        // is located, never hardcoded: `GitAccess::auto()` goes through
        // `resolve_executable`, and `unavailable()` proves the no-spawn path.
        assert!(!GitAccess::unavailable().is_available());
    }

    #[test]
    fn trailing_separators_cannot_produce_two_identities() {
        // The bare-repository spelling of `--git-common-dir` is exactly `.`, and
        // a transcript cwd may end in `/.` or `/`; both name the same directory
        // as the plain path, and `canonical_path` is a UNIQUE column.
        assert_eq!(tidy(Some("/repo/a/b".into())).as_deref(), Some("/repo/a/b"));
        assert_eq!(
            tidy(Some("/repo/a/b/".into())).as_deref(),
            Some("/repo/a/b")
        );
        assert_eq!(
            tidy(Some("C:\\repo\\a\\".into())).as_deref(),
            Some("C:\\repo\\a")
        );
        assert_eq!(tidy(Some("C:\\".into())).as_deref(), Some("C:\\"), "root");
        assert_eq!(tidy(Some("/".into())).as_deref(), Some("/"), "root");
        assert_eq!(tidy(Some("\\\\".into())).as_deref(), Some("\\"), "root");
        assert_eq!(tidy(Some("/数据/".into())).as_deref(), Some("/数据"));

        let c = ctx();
        assert_eq!(
            c.canonicalize_from(".", "/repo/x").as_deref(),
            Some("/repo/x"),
            "a bare repository's common dir is the directory itself"
        );
        assert_eq!(
            c.canonicalize_from("../../.git", "/repo/a/b").as_deref(),
            Some("/repo/.git"),
            "git prints the common dir relative to the cwd"
        );
        assert_eq!(
            c.canonicalize("/repo/x/").as_deref(),
            c.canonicalize("/repo/x").as_deref()
        );
    }
}
