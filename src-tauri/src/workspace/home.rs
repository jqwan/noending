//! NoEnding Home — the on-disk data root, its bootstrap pointer, and the
//! default workspace.
//!
//! This module owns the Home layout and relocation contract.
//!
//! ## Layout (§2)
//!
//! ```text
//! ~/.noending/            (Windows: %USERPROFILE%\.noending\)
//! ├─ data/                noending.db            ┐
//! ├─ runtime/             context-bundles/       ├ reserved: never a WorkspacePath
//! ├─ logs/                                        ┘
//! └─ workspace/          ← the default working directory, a NORMAL path
//! ```
//!
//! ## Resolution order (§3)
//!
//! ```text
//! $NOENDING_HOME  →  bootstrap.current_home  →  ~/.noending
//! ```
//!
//! The pointer must live outside NoEnding Home, because the database inside the
//! Home is what we are trying to locate:
//!
//! ```text
//! <dirs::config_dir()>/app.noending.desktop/home.json
//! ```
//!
//! (`~/Library/Application Support/…` on macOS, `%APPDATA%\…` on Windows.)
//! Content: `{"current_home": "/Users/me/.noending", "pending_home": "…"}`,
//! written with the already-present `serde_json`; no new dependency (§42.3-M12).
//!
//! ## Changing the Home is a restart-then-migrate operation (§3/§4)
//!
//! ```text
//! set_noending_home(new)  →  write pending_home only  →  UI: “重启后迁移并生效”
//! next launch, BEFORE opening the DB:
//!   move data/ runtime/ logs/ to the new Home
//!   on success: current_home = pending_home, clear pending_home
//! ```
//!
//! Never switch the live process over to a copied database — that leaves the old
//! process writing to a diverged file. `workspace/` is NOT moved: the old
//! default workspace becomes an ordinary WorkspacePath and the new one is
//! `<new-home>/workspace` (§4). A failed move must leave `current_home` untouched.
//!
//! ## Required shape
//!
//! Every entry point takes the resolution inputs explicitly — `explicit`
//! (`$NOENDING_HOME`), `bootstrap` path, and `home` — because `std::env::set_var`
//! in one test poisons every other test in the same binary (§42.3-M13). Exactly
//! one function may read the environment.
//!
//! ```text
//! NoEndingHome { root, data_dir, runtime_dir, logs_dir, default_workspace, db_path }
//! NoEndingHome::resolve(req: &HomeRequest) -> Result<NoEndingHome>
//! NoEndingHome::ensure_dirs()              // create_dir_all, incl. workspace/ (§42.3-M21)
//! is_reserved_app_path(path) -> bool       // Home, data/, runtime/, logs/ — segment-wise
//! ```
//!
//! `is_reserved_app_path` must use `identity::is_within`, not `starts_with`
//! (`/Users/me/.noending/datax` is not reserved).
//!
//! Also owns (方案 §42.3-M10/M11/M23, all reuse-existing-code fixes):
//! * `platform::exec_resolver::resolve_executable(name)` — the generic locator
//!   `git` needs, extracted from the Agent-specific resolver; a Finder-launched
//!   macOS app does not inherit the shell PATH where Homebrew's git lives.
//! * `platform::exec_runner::run(program, args, cwd, timeout)` — extracted from
//!   `run_headless`, which today falls back to a hardcoded `/tmp` when `cwd` is
//!   `None` and would run `git` in the wrong directory.
//! * `platform::paths::expand_tilde` becomes the single expander, delegating to
//!   `identity::expand_tilde`; `launcher::expand_tilde` must not stay a third one.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{other, Result};
use crate::workspace::identity::{self, NormalizeOpts, PathStyle};

/// Default NoEnding Home directory name, under the user's home (§2).
pub const APP_DIR_NAME: &str = ".noending";
/// Reserved: the database and everything app-owned that is not a runtime file.
pub const DATA_DIR_NAME: &str = "data";
/// Reserved: ephemeral per-run artifacts (`context-bundles/`, §42.3-M14).
pub const RUNTIME_DIR_NAME: &str = "runtime";
/// Reserved: log output.
pub const LOGS_DIR_NAME: &str = "logs";
/// NOT reserved: the default working directory, an ordinary WorkspacePath (§2).
pub const WORKSPACE_DIR_NAME: &str = "workspace";
pub const DB_FILE_NAME: &str = "noending.db";
/// Bootstrap pointer file name (§42.3-M12).
pub const BOOTSTRAP_FILE_NAME: &str = "home.json";

/// What a Home relocation moves (§4). `workspace/` is absent on purpose: it
/// holds user files, and silently relocating them is data loss with extra steps.
pub const MIGRATABLE_DIRS: [&str; 3] = [DATA_DIR_NAME, RUNTIME_DIR_NAME, LOGS_DIR_NAME];

// ---------------------------------------------------------------------------
// resolution
// ---------------------------------------------------------------------------

/// Why the Home is where it is. Reported in logs and Settings so a surprising
/// location is never a mystery (§42.3-M12: failures must not be silent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HomeSource {
    /// `$NOENDING_HOME`.
    ExplicitEnv,
    /// `current_home` from the bootstrap pointer.
    Bootstrap,
    /// `<user home>/.noending`.
    DefaultHome,
}

impl HomeSource {
    pub fn as_str(self) -> &'static str {
        match self {
            HomeSource::ExplicitEnv => "explicit_env",
            HomeSource::Bootstrap => "bootstrap",
            HomeSource::DefaultHome => "default_home",
        }
    }
}

/// The three inputs of resolution, all explicit. Nothing here reads the
/// environment or the filesystem, so every branch is testable (§42.3-M13).
#[derive(Debug, Clone, Default)]
pub struct HomeRequest<'a> {
    /// `$NOENDING_HOME`, if set. Read by exactly one function:
    /// [`resolve_explicit_override`].
    pub explicit: Option<&'a Path>,
    /// `bootstrap.current_home`, if the pointer exists and parsed.
    pub bootstrap: Option<&'a str>,
    /// The user's home directory, used for the `~/.noending` fallback and for
    /// expanding `~` inside the other two inputs.
    pub user_home: Option<&'a Path>,
    /// Separator rules. `None` = the running platform's, which is why this is a
    /// field and not an ambient read: Windows layout behavior must be testable
    /// on a macOS runner (§16-7).
    pub style: Option<PathStyle>,
}

/// NoEnding Home and the paths derived from it. Pure values: constructing one
/// never touches the filesystem, so `resolve` cannot create anything by
/// accident and `is_reserved_app_path` stays cheap enough to call per path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoEndingHome {
    pub root: PathBuf,
    pub data_dir: PathBuf,
    pub runtime_dir: PathBuf,
    pub logs_dir: PathBuf,
    /// `<root>/workspace` — the default working directory for sessions with no
    /// Workstream path (§13). Must exist before it is ever used as a `cwd`
    /// (§42.3-M21): on macOS a failed `cd` in the launch script falls back to
    /// `$HOME`, which is exactly the dotfiles-repository directory §1.4 forbids
    /// ever becoming a Project.
    pub default_workspace: PathBuf,
    pub db_path: PathBuf,
}

impl NoEndingHome {
    /// Derive every child path from an absolute (or `~`-prefixed) root,
    /// lexically. `None` when the root cannot become an absolute path — we
    /// refuse to guess a data root against the process cwd.
    pub fn new(root: &str, user_home: Option<&str>) -> Option<Self> {
        Self::new_with_style(root, user_home, PathStyle::current())
    }

    /// Explicit-`style` form of [`Self::new`]: the only reason a Home's layout
    /// should depend on the machine running the test is the machine running the
    /// app, so Windows separators stay unit-testable on macOS (§16-7).
    pub fn new_with_style(root: &str, user_home: Option<&str>, style: PathStyle) -> Option<Self> {
        let canonical = identity::normalize_path_with(
            root,
            NormalizeOpts {
                style: Some(style),
                base: None,
                home: user_home,
            },
        )?;
        let child = |name: &str| {
            // Re-normalizing the join is not paranoia: it keeps `..` inside the
            // Home instead of escaping it, so a hostile `NOENDING_HOME=/a/../b`
            // cannot make `is_reserved_app_path` guard the wrong tree.
            identity::normalize_path_with(
                &join_native(&canonical, name, style),
                NormalizeOpts {
                    style: Some(style),
                    base: None,
                    home: None,
                },
            )
            .unwrap_or_else(|| join_native(&canonical, name, style))
        };
        let data = child(DATA_DIR_NAME);
        Some(Self {
            root: PathBuf::from(&canonical),
            data_dir: PathBuf::from(&data),
            runtime_dir: PathBuf::from(child(RUNTIME_DIR_NAME)),
            logs_dir: PathBuf::from(child(LOGS_DIR_NAME)),
            default_workspace: PathBuf::from(child(WORKSPACE_DIR_NAME)),
            db_path: PathBuf::from(data).join(DB_FILE_NAME),
        })
    }

    /// §3: `$NOENDING_HOME` → `bootstrap.current_home` → `~/.noending`.
    ///
    /// An input that cannot become an absolute path is skipped with a note
    /// rather than failing startup — a typo in an env var must not brick the
    /// app, and it must not be silent either.
    pub fn resolve(req: &HomeRequest) -> Result<NoEndingHome> {
        Self::resolve_reported(req).map(|r| r.home)
    }

    /// [`Self::resolve`] plus the reason and the notes.
    pub fn resolve_reported(req: &HomeRequest) -> Result<HomeResolution> {
        let user_home = req.user_home.map(|p| p.to_string_lossy().to_string());
        let style = req.style.unwrap_or_else(PathStyle::current);
        let mut notes = Vec::new();
        let mut candidates: Vec<(HomeSource, String)> = Vec::new();
        if let Some(explicit) = req.explicit {
            candidates.push((
                HomeSource::ExplicitEnv,
                explicit.to_string_lossy().to_string(),
            ));
        }
        if let Some(bootstrap) = req.bootstrap.filter(|b| !b.trim().is_empty()) {
            candidates.push((HomeSource::Bootstrap, bootstrap.to_string()));
        }
        if let Some(home) = user_home.as_deref() {
            candidates.push((
                HomeSource::DefaultHome,
                join_native(home, APP_DIR_NAME, style),
            ));
        }

        for (source, raw) in candidates {
            match Self::new_with_style(&raw, user_home.as_deref(), style) {
                Some(home) => {
                    return Ok(HomeResolution {
                        home,
                        source,
                        notes,
                    })
                }
                None => notes.push(format!(
                    "NoEnding Home 候选路径无法规范化为绝对路径，已忽略: {raw} (source={})",
                    source.as_str()
                )),
            }
        }
        Err(other(
            "无法确定 NoEnding Home：环境变量、bootstrap 指针与用户目录均不可用",
        ))
    }

    /// Create `<root>`, `data/`, `runtime/`, `logs/` and `workspace/`.
    ///
    /// `workspace/` is not optional (§42.3-M21): it is handed to Agents as a
    /// `cwd`, and a missing directory silently becomes `$HOME` on macOS.
    pub fn ensure_dirs(&self) -> Result<()> {
        for dir in [
            self.root.as_path(),
            self.data_dir.as_path(),
            self.runtime_dir.as_path(),
            self.logs_dir.as_path(),
            self.default_workspace.as_path(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    /// The §2 exclusion set for this Home.
    pub fn reserved(&self) -> ReservedPaths {
        ReservedPaths::new(self)
    }

    pub fn root_str(&self) -> String {
        self.root.to_string_lossy().to_string()
    }

    pub fn default_workspace_str(&self) -> String {
        self.default_workspace.to_string_lossy().to_string()
    }

    pub fn db_path_str(&self) -> String {
        self.db_path.to_string_lossy().to_string()
    }
}

/// Result of [`NoEndingHome::resolve_reported`].
#[derive(Debug, Clone)]
pub struct HomeResolution {
    pub home: NoEndingHome,
    pub source: HomeSource,
    /// Human/log-facing problems that must not be swallowed (§42.3-M12).
    pub notes: Vec<String>,
}

/// Where the bootstrap pointer lives, by default.
///
/// 方案 §42.3-M12 names the path (`~/Library/Application Support/…` on macOS,
/// `%APPDATA%\…` on Windows) and the call (`dirs::config_dir()`) — and on macOS
/// those two disagree, because `config_dir()` is `~/Library/Preferences`. The
/// **path** wins: it is the sentence the migration depends on (the pre-Home
/// database is already in that folder, §42.3-M14), and
/// `platform::paths::resolve_app_support_dir` encodes exactly that pair.
pub fn default_pointer_path() -> Option<PathBuf> {
    crate::platform::paths::resolve_app_support_dir().map(|d| d.join(BOOTSTRAP_FILE_NAME))
}

/// The ONLY reader of `$NOENDING_HOME` in the crate (方案 §42.3-M13).
///
/// Every other function takes its inputs explicitly, because Rust tests share
/// one process: a single `env::set_var` would leak into sibling tests (and is
/// `unsafe` in recent editions). Tests of Home behavior must construct a
/// [`HomeRequest`] instead of calling this.
pub fn resolve_explicit_override() -> Option<PathBuf> {
    std::env::var_os("NOENDING_HOME")
        .map(|v| v.to_string_lossy().to_string())
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
}

// ---------------------------------------------------------------------------
// reserved app paths (§2)
// ---------------------------------------------------------------------------

/// Paths that can never be a WorkspacePath: NoEnding Home itself and the three
/// app-owned directories under it (§2).
///
/// Two different relations, deliberately not collapsed into one list:
/// * the Home **root** is reserved by *equality* only — its non-reserved
///   children (`workspace/`) are ordinary paths, so treating the root as a
///   container would reserve the whole Home and contradict §2;
/// * `data/`, `runtime/`, `logs/` are reserved by *containment*, segment-wise via
///   [`identity::is_within`], so `~/.noending/datax` is an ordinary directory
///   and `~/.noending/data/noending.db` is not.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReservedPaths {
    home_root: Option<String>,
    app_dirs: Vec<String>,
}

impl ReservedPaths {
    pub fn new(home: &NoEndingHome) -> Self {
        Self {
            home_root: Some(home.root.to_string_lossy().to_string()),
            app_dirs: vec![
                home.data_dir.to_string_lossy().to_string(),
                home.runtime_dir.to_string_lossy().to_string(),
                home.logs_dir.to_string_lossy().to_string(),
            ],
        }
    }

    /// Empty when NoEnding Home is not installed at all, i.e. nothing is
    /// reserved. Resolution failures must not turn into "every path is reserved".
    pub fn is_empty(&self) -> bool {
        self.home_root.is_none() && self.app_dirs.is_empty()
    }

    /// `candidate` must already be a `canonical_path` (produced by
    /// [`identity::normalize_path`]); this does no normalization of its own.
    pub fn contains(&self, canonical: &str) -> bool {
        self.contains_with(canonical, identity::PathStyle::current())
    }

    /// [`Self::contains`] with the separator/case conventions injected, so
    /// Windows reservation can be proven from a macOS test (§42.3-M8).
    ///
    /// The comparison is [`identity::identity_key`], not [`identity::path_key`]:
    /// on a Windows volume `C:\Users\me\.noending\DATA` IS the `data/` directory
    /// this set exists to reserve. §2's reservation is a gate rather than a
    /// membership, so a case-variant miss does not converge later the way two
    /// spellings of a repository do — it lets the app's own database directory
    /// become a WorkspacePath and then a Project.
    pub fn contains_with(&self, canonical: &str, style: identity::PathStyle) -> bool {
        let canonical = canonical.trim();
        if canonical.is_empty() {
            return false;
        }
        let key = identity::identity_key(canonical, style);
        if self
            .home_root
            .as_deref()
            .is_some_and(|root| identity::identity_key(root.trim(), style) == key)
        {
            return true;
        }
        self.app_dirs
            .iter()
            .any(|dir| identity::is_within_with(canonical, dir, style))
    }

    /// Everything this set reserves, for logs and the Settings screen.
    pub fn roots(&self) -> Vec<String> {
        let mut out = self.app_dirs.clone();
        out.extend(self.home_root.clone());
        out
    }
}

/// §2 gate for callers holding only a raw string (a Session `cwd`, a path
/// picked in a file dialog). Normalizes first, then checks segment-wise.
pub fn is_reserved_app_path(raw: &str, home: &NoEndingHome) -> bool {
    let user_home = identity::home_dir().map(|h| h.to_string_lossy().to_string());
    is_reserved_app_path_with(
        raw,
        home,
        NormalizeOpts {
            style: None,
            base: None,
            home: user_home.as_deref(),
        },
    )
}

/// Fully injected form of [`is_reserved_app_path`]: no ambient home, explicit
/// separators, so Windows behavior is unit-testable on macOS (§16-7).
pub fn is_reserved_app_path_with(raw: &str, home: &NoEndingHome, opts: NormalizeOpts<'_>) -> bool {
    let Some(canonical) = identity::normalize_path_with(raw, opts) else {
        // Not normalizable ⇒ not a WorkspacePath ⇒ reservation is moot.
        return false;
    };
    let style = opts.style.unwrap_or_else(identity::PathStyle::current);
    home.reserved().contains_with(&canonical, style)
}

// ---------------------------------------------------------------------------
// bootstrap pointer (§3, §42.3-M12)
// ---------------------------------------------------------------------------

/// `<app support>/home.json`: the pointer that survives before the database —
/// and therefore before the Home — is known.
///
/// `current_home` is what this launch opened; `pending_home` is what the next
/// launch migrates to. Written only by [`request_relocation`] and
/// [`apply_relocation`] — never by the DB layer, and never inside the Home.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapPointer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_home: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_home: Option<String>,
}

impl BootstrapPointer {
    /// Read the pointer. A missing, unreadable or malformed file yields an empty
    /// pointer plus a note: resolution then falls back to `~/.noending`, which is
    /// the documented behavior, but the reason must reach the log (§42.3-M12).
    ///
    /// Unknown JSON fields are ignored on purpose — a newer app must not be
    /// unable to find the user's database.
    pub fn load(path: &Path) -> (Self, Option<String>) {
        match std::fs::read(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::default(), None),
            Err(e) => (
                Self::default(),
                Some(format!(
                    "无法读取 NoEnding Home 指针 {}: {e}",
                    path.display()
                )),
            ),
            Ok(bytes) => match serde_json::from_slice::<BootstrapPointer>(&bytes) {
                Ok(pointer) => (pointer, None),
                Err(e) => (
                    Self::default(),
                    Some(format!(
                        "NoEnding Home 指针 {} 解析失败，已回退到默认位置: {e}",
                        path.display()
                    )),
                ),
            },
        }
    }

    /// Atomic-enough write: temp file in the same directory, then rename. A
    /// half-written pointer would lose the user's Home location, which is worse
    /// than a stale one.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = sibling_tmp(path, "writing");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// The start-up migration handoff: everything needed to move the app-owned data
/// *before* the database is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingMigration {
    pub pointer_path: PathBuf,
    /// Home currently recorded as `current_home`.
    pub from: PathBuf,
    /// Home recorded as `pending_home`.
    pub to: PathBuf,
}

/// Write `pending_home` only — the request to move the Home at next start (§3).
///
/// `set_noending_home` must NOT swap the live database: the running process
/// would keep writing the old file while the new location diverges from it.
pub fn request_relocation(
    pointer_path: &Path,
    new_home: &str,
    user_home: Option<&str>,
) -> Result<BootstrapPointer> {
    request_relocation_with(pointer_path, new_home, user_home, PathStyle::current())
}

/// Explicit-`style` form of [`request_relocation`] (方案 §44.3-C1), so the
/// canonical spelling a relocation writes is proven on either runner instead of
/// meaning whatever platform happened to run the test. Only the target's
/// spelling is style-dependent: `pointer_path` is a real file on the host.
pub fn request_relocation_with(
    pointer_path: &Path,
    new_home: &str,
    user_home: Option<&str>,
    style: PathStyle,
) -> Result<BootstrapPointer> {
    let (mut pointer, _) = BootstrapPointer::load(pointer_path);
    let target = NoEndingHome::new_with_style(new_home, user_home, style)
        .ok_or_else(|| other(format!("无法规范化新的 NoEnding Home 路径: {new_home}")))?;
    if let Some(current) = pointer.current_home.as_deref() {
        // Same *location*, not same spelling: a case-variant on a Windows volume
        // would pass for a relocation and then move `data/` onto itself.
        if identity::same_location_with(&target.root_str(), current.trim(), style) {
            return Err(other("新位置与当前 NoEnding Home 相同，无需迁移"));
        }
    }
    pointer.pending_home = Some(target.root_str());
    pointer.save(pointer_path)?;
    Ok(pointer)
}

/// Clear `pending_home` and point `current_home` at the migration target.
///
/// Only ever called after every movable directory landed, which is what makes
/// "a failed move leaves `current_home` untouched" true.
pub fn apply_relocation(migration: &PendingMigration) -> Result<BootstrapPointer> {
    let (mut pointer, _) = BootstrapPointer::load(&migration.pointer_path);
    pointer.current_home = Some(migration.to.to_string_lossy().to_string());
    pointer.pending_home = None;
    pointer.save(&migration.pointer_path)?;
    Ok(pointer)
}

// ---------------------------------------------------------------------------
// data-root migration (§4) — runs BEFORE the database opens
// ---------------------------------------------------------------------------

/// What happened to one directory during a Home relocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DirOutcome {
    /// `rename`d in one step.
    Moved,
    /// The target already existed; kept as-is and the source was left alone.
    /// Never overwritten — losing `runtime/` of a Home the user already created
    /// is not a migration, it is a surprise.
    TargetExisted,
    /// Copied because `rename` failed (usually a different volume). The source
    /// stays in the old Home, which is inert but not deleted: history wins over
    /// disk space.
    Copied,
    /// Nothing on either side.
    NothingToMove,
}

/// Per-directory record; `Vec`-shaped so a UI can list it without guessing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MigrationReport {
    pub from: String,
    pub to: String,
    pub dirs: Vec<(String, DirOutcome)>,
    pub notes: Vec<String>,
    /// `workspace/` is reported, never moved (§4).
    pub workspace_left_behind: Option<String>,
}

impl MigrationReport {
    fn new(migration: &PendingMigration) -> Self {
        Self {
            from: migration.from.to_string_lossy().to_string(),
            to: migration.to.to_string_lossy().to_string(),
            dirs: Vec::new(),
            notes: Vec::new(),
            workspace_left_behind: None,
        }
    }

    pub fn is_noop(&self) -> bool {
        self.dirs.is_empty()
    }
}

/// Move `data/ runtime/ logs/` from the current Home to the pending one, then
/// flip the pointer. Never `workspace/`, never the pointer's `current_home`
/// before success (§3/§4).
///
/// Retry-safety, which matters because this runs on every start and a partial
/// first attempt is normal (power loss, full disk):
/// * each directory is staged as `<name>.migrating` on the *target* volume and
///   then renamed into place, so a re-run promotes the stage instead of copying
///   twice;
/// * an existing target is never overwritten;
/// * sources are never deleted on the copy fallback.
///
/// Any error returns before `apply_relocation`, so the next start retries.
pub fn migrate_data_root(migration: &PendingMigration) -> Result<MigrationReport> {
    let from = migration.from.to_string_lossy().to_string();
    let to = migration.to.to_string_lossy().to_string();
    let mut report = MigrationReport::new(migration);

    let from_home = NoEndingHome::new(&from, None).ok_or_else(|| {
        other(format!(
            "当前 NoEnding Home 路径无法规范化，迁移取消: {from}"
        ))
    })?;
    let to_home = NoEndingHome::new(&to, None)
        .ok_or_else(|| other(format!("目标 NoEnding Home 路径无法规范化，迁移取消: {to}")))?;

    if identity::same_location(&from, &to) {
        // Nothing to move, but the stale pointer must still be cleaned or every
        // later start would re-run this branch.
        apply_relocation(migration)?;
        report.notes.push(format!(
            "pending_home 与 current_home 相同，仅清理指针: {to}"
        ));
        return Ok(report);
    }
    // A target nested inside a directory we are about to move would recurse
    // into itself on the copy fallback, so refuse it up front.
    for dir in MIGRATABLE_DIRS {
        let src = child_of(&from_home, dir);
        if identity::is_within(&to, &src.to_string_lossy()) {
            return Err(other(format!(
                "新的 NoEnding Home 位于 {src} 内部，无法迁移；请先选择其它位置",
                src = src.display()
            )));
        }
    }

    std::fs::create_dir_all(&to_home.root)?;
    for dir in MIGRATABLE_DIRS {
        let (outcome, note) = relocate_dir(&from_home.root, &to_home.root, dir)?;
        report.dirs.push((dir.to_string(), outcome));
        if let Some(note) = note {
            report.notes.push(note);
        }
    }

    // §4: the old default workspace keeps its user files and becomes an
    // ordinary WorkspacePath. Say so in the report so nobody hunts for them.
    let old_workspace = from_home.default_workspace;
    if old_workspace.is_dir() {
        report.workspace_left_behind = Some(old_workspace.to_string_lossy().to_string());
        report.notes.push(format!(
            "旧默认工作目录未移动，已作为普通工作路径保留: {}",
            old_workspace.display()
        ));
    }

    apply_relocation(migration)?;
    Ok(report)
}

fn relocate_dir(
    from_root: &Path,
    to_root: &Path,
    name: &str,
) -> Result<(DirOutcome, Option<String>)> {
    let src = from_root.join(name);
    let dst = to_root.join(name);
    let staging = to_root.join(format!("{name}.migrating"));
    let staging_existed = staging.symlink_metadata().is_ok();

    if dst.symlink_metadata().is_ok() {
        let note = if src.symlink_metadata().is_ok() {
            Some(format!(
                "目标 {dst} 已存在，保留目标内容；源 {src} 未移动",
                dst = dst.display(),
                src = src.display()
            ))
        } else {
            None
        };
        return Ok((DirOutcome::TargetExisted, note));
    }
    if staging_existed {
        // A previous attempt finished copying but died before the rename.
        std::fs::rename(&staging, &dst)?;
        let note = if src.symlink_metadata().is_ok() {
            Some(format!(
                "沿用上次迁移的中转目录 {staging}；源 {src} 保持原样",
                staging = staging.display(),
                src = src.display()
            ))
        } else {
            None
        };
        return Ok((DirOutcome::Moved, note));
    }
    if src.symlink_metadata().is_err() {
        return Ok((DirOutcome::NothingToMove, None));
    }

    match std::fs::rename(&src, &staging) {
        Ok(()) => {
            std::fs::rename(&staging, &dst)?;
            Ok((DirOutcome::Moved, None))
        }
        Err(e) => {
            // Cross-device (EXDEV) or a denied rename: copy instead of failing,
            // but never delete the source, so the old Home stays readable.
            copy_tree(&src, &staging)?;
            std::fs::rename(&staging, &dst)?;
            Ok((
                DirOutcome::Copied,
                Some(format!(
                    "{name}/ 无法移动（{e}），已复制；旧位置 {src} 未删除，可自行清理",
                    name = name,
                    src = src.display()
                )),
            ))
        }
    }
}

/// Recursive, depth-first copy used only when `rename` cannot cross the volume.
/// Symlinks and special files are skipped and reported rather than followed —
/// following them could write through a link the user placed outside the Home.
fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    let entries = std::fs::read_dir(src)?;
    for entry in entries {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let meta = std::fs::symlink_metadata(&from)?;
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            copy_tree(&from, &to)?;
        } else if meta.is_file() {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// start-up orchestration
// ---------------------------------------------------------------------------

/// Everything [`prepare_home`] needs. Built by [`StartupInputs::from_environment`]
/// in `lib.rs` and by literal values in tests — the type is what keeps §42.3-M13
/// honest: there is no ambient read inside the resolution path.
#[derive(Debug, Clone, Default)]
pub struct StartupInputs {
    pub explicit: Option<PathBuf>,
    pub pointer_path: Option<PathBuf>,
    pub user_home: Option<PathBuf>,
}

impl StartupInputs {
    /// The only production constructor, and the only place besides
    /// [`resolve_explicit_override`] that consults the OS.
    pub fn from_environment() -> Self {
        Self {
            explicit: resolve_explicit_override(),
            pointer_path: default_pointer_path(),
            user_home: identity::home_dir(),
        }
    }
}

/// The outcome of Home resolution, ready to be handed to `Db::open` and
/// `app.manage()`.
#[derive(Debug, Clone)]
pub struct StartupOutcome {
    pub home: NoEndingHome,
    pub source: HomeSource,
    pub pointer_path: Option<PathBuf>,
    /// Set when a relocation was requested but could not be completed; the app
    /// keeps running on `current_home` (§3).
    pub pending_home: Option<String>,
    pub restart_required: bool,
    pub reports: Vec<MigrationReport>,
    pub notes: Vec<String>,
}

impl StartupOutcome {
    /// §11 `get_workspace_settings` payload (plus `home_source`, which exists so
    /// a wrong-looking location is explainable).
    pub fn settings(&self) -> WorkspaceSettings {
        WorkspaceSettings {
            noending_home: self.home.root_str(),
            default_workspace: self.home.default_workspace_str(),
            pending_home: self.pending_home.clone(),
            restart_required: self.restart_required,
            db_path: self.home.db_path_str(),
            home_source: self.source.as_str().to_string(),
        }
    }
}

/// §11 `get_workspace_settings`.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceSettings {
    pub noending_home: String,
    pub default_workspace: String,
    pub pending_home: Option<String>,
    pub restart_required: bool,
    pub db_path: String,
    pub home_source: String,
}

/// Resolve the Home, run the pending relocation, and create the directories —
/// in that order, all before the database is opened.
///
/// The `Result` covers exactly one case: no Home can be determined at all, so
/// there is nowhere to open a database. A *migration* failure is not an error
/// here — NoEnding still starts, on the old Home, with the reason in `notes`,
/// because "no database found" would look like data loss (§41).
pub fn prepare_home(inputs: &StartupInputs) -> Result<StartupOutcome> {
    let pointer = inputs.pointer_path.as_deref();
    let (loaded, load_note) = match pointer {
        Some(path) => BootstrapPointer::load(path),
        None => (BootstrapPointer::default(), None),
    };
    let user_home_str = inputs
        .user_home
        .as_deref()
        .map(|p| p.to_string_lossy().to_string());
    let mut notes = Vec::new();
    if let Some(note) = load_note {
        notes.push(note);
    }

    let request = HomeRequest {
        explicit: inputs.explicit.as_deref(),
        bootstrap: loaded.current_home.as_deref(),
        user_home: inputs.user_home.as_deref(),
        style: None,
    };
    let resolution = match NoEndingHome::resolve_reported(&request) {
        Ok(r) => r,
        Err(e) => {
            // There is genuinely nowhere to put the database. This is the one
            // unrecoverable case, and it is reported instead of being papered
            // over with an empty path that `Db::open` would mis-open elsewhere.
            notes.push(e.to_string());
            return Err(other(format!(
                "无法确定 NoEnding Home，已放弃启动: {}",
                notes.join("; ")
            )));
        }
    };
    notes.extend(resolution.notes);
    let mut home = resolution.home;

    // An explicit `$NOENDING_HOME` outranks the pointer for THIS launch only, and
    // is deliberately never persisted: a one-off env var must not silently become
    // the app's data root. That is a footgun worth a log line, because the user
    // sees a different (possibly empty) set of Workstreams for one session and
    // then loses it on restart.
    if resolution.source == HomeSource::ExplicitEnv {
        if let Some(recorded) = loaded.current_home.as_deref() {
            if !identity::same_location(&home.root_str(), recorded.trim()) {
                notes.push(format!(
                    "本次启动使用 $NOENDING_HOME={}，指针记录的 {recorded} 未改写；下次启动会回到指针位置（需要长期搬迁请在设置里修改 NoEnding Home）",
                    home.root.display()
                ));
            }
        }
    }

    // 1. The relocation the user asked for last run, if any.
    let mut reports = Vec::new();
    let mut pending_home = loaded.pending_home.clone();
    let mut restart_required = pending_home.is_some();
    if let (Some(pointer_path), Some(pending)) = (pointer, pending_home.clone()) {
        match NoEndingHome::new(&pending, user_home_str.as_deref()) {
            Some(target_home) => {
                let migration = PendingMigration {
                    pointer_path: pointer_path.to_path_buf(),
                    from: home.root.clone(),
                    to: target_home.root.clone(),
                };
                match migrate_data_root(&migration) {
                    Ok(report) => {
                        notes.extend(report.notes.clone());
                        // Only now does this process move over: the pointer and
                        // the directories agree, so nothing can diverge (§3).
                        home = target_home;
                        pending_home = None;
                        restart_required = false;
                        reports.push(report);
                    }
                    Err(e) => {
                        // §3: current_home was NOT flipped, so this run continues
                        // on the old Home and the next launch retries. The pointer
                        // is left exactly as the user wrote it.
                        notes.push(format!(
                            "NoEnding Home 迁移未完成，本次仍使用 {current}：{e}",
                            current = home.root.display()
                        ));
                    }
                }
            }
            None => notes.push(format!(
                "pending_home 无法规范化，已忽略并保持当前 Home: {pending}"
            )),
        }
    }

    // 2. §42.3-M21: `default_workspace` must exist before it is a `cwd`.
    if let Err(e) = home.ensure_dirs() {
        notes.push(format!(
            "无法创建 NoEnding Home 目录 {root}: {e}",
            root = home.root.display()
        ));
    }

    Ok(StartupOutcome {
        home,
        source: resolution.source,
        pointer_path: inputs.pointer_path.clone(),
        pending_home,
        restart_required,
        reports,
        notes,
    })
}

fn child_of(home: &NoEndingHome, name: &str) -> PathBuf {
    home.root.join(name)
}

fn join_native(base: &str, child: &str, style: PathStyle) -> String {
    let sep = if style.is_windows() { '\\' } else { '/' };
    format!("{}{}{}", base.trim_end_matches(['/', '\\']), sep, child)
}

/// `<dir>.<suffix>`, in the same directory, for atomic rewrite-then-rename.
fn sibling_tmp(dir: &Path, suffix: &str) -> PathBuf {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "home".to_string());
    dir.with_file_name(format!(".{name}.{suffix}"))
}

// ---------------------------------------------------------------------------
// unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up Unix Home must normalize the same way on a Windows runner, so
    /// every lexical test pins its style instead of inheriting the host's.
    fn home_at(root: &str) -> NoEndingHome {
        NoEndingHome::new_with_style(root, Some("/Users/tester"), PathStyle::Unix)
            .expect("test home")
    }

    /// 必须测试: `~` expansion, and the §3 precedence chain.
    #[test]
    fn tilde_and_precedence() {
        let explicit = PathBuf::from("~/custom-home");
        let req = HomeRequest {
            explicit: Some(&explicit),
            bootstrap: Some("/persisted/.noending"),
            user_home: Some(Path::new("/Users/tester")),
            style: Some(PathStyle::Unix),
        };
        let r = NoEndingHome::resolve_reported(&req).unwrap();
        assert_eq!(r.home.root, PathBuf::from("/Users/tester/custom-home"));
        assert_eq!(r.source, HomeSource::ExplicitEnv);
        assert!(r.notes.is_empty());

        // Bootstrap wins over the fallback when there is no env override.
        let req = HomeRequest {
            explicit: None,
            bootstrap: Some("/persisted/.noending"),
            user_home: Some(Path::new("/Users/tester")),
            style: Some(PathStyle::Unix),
        };
        let r = NoEndingHome::resolve_reported(&req).unwrap();
        assert_eq!(r.home.root, PathBuf::from("/persisted/.noending"));
        assert_eq!(r.source, HomeSource::Bootstrap);

        // §2 default: `~/.noending`, Windows spelled as `%USERPROFILE%`.
        let req = HomeRequest {
            explicit: None,
            bootstrap: None,
            user_home: Some(Path::new("/Users/tester")),
            style: Some(PathStyle::Unix),
        };
        let r = NoEndingHome::resolve_reported(&req).unwrap();
        assert_eq!(r.home.root, PathBuf::from("/Users/tester/.noending"));
        assert_eq!(
            identity::path_key(&r.home.db_path_str()),
            "/Users/tester/.noending/data/noending.db"
        );
        assert_eq!(r.source, HomeSource::DefaultHome);

        // A relative %USERPROFILE% spelling still stays on the drive it names.
        let req = HomeRequest {
            explicit: None,
            bootstrap: None,
            user_home: Some(Path::new("C:\\Users\\me")),
            style: Some(PathStyle::Windows),
        };
        let r = NoEndingHome::resolve_reported(&req).unwrap();
        assert_eq!(r.home.root, PathBuf::from("C:\\Users\\me\\.noending"));
        // `PathBuf::join` is platform-native, so compare the separator-insensitive
        // identity form rather than a spelling (`db_path` is used as a Path, not
        // as a key).
        assert_eq!(
            identity::path_key(&r.home.db_path_str()),
            "C:/Users/me/.noending/data/noending.db"
        );

        // A garbage env value is reported, not obeyed, and not fatal.
        let relative = PathBuf::from("not-absolute");
        let req = HomeRequest {
            explicit: Some(&relative),
            bootstrap: None,
            user_home: Some(Path::new("/Users/tester")),
            style: Some(PathStyle::Unix),
        };
        let r = NoEndingHome::resolve_reported(&req).unwrap();
        assert_eq!(r.home.root, PathBuf::from("/Users/tester/.noending"));
        assert_eq!(r.notes.len(), 1, "the skipped candidate must be reported");
    }

    /// 必须测试: reserved `~/.noending/data` exclusion, and
    /// `~/.noending/workspace` ALLOWED.
    #[test]
    fn reserved_paths_are_segment_wise() {
        let home = home_at("/Users/me/.noending");
        let reserved = home.reserved();
        assert!(reserved.contains("/Users/me/.noending"), "the Home itself");
        assert!(reserved.contains("/Users/me/.noending/data"));
        assert!(reserved.contains("/Users/me/.noending/data/noending.db"));
        assert!(reserved.contains("/Users/me/.noending/runtime/context-bundles"));
        assert!(reserved.contains("/Users/me/.noending/logs"));
        assert!(
            !reserved.contains("/Users/me/.noending/workspace"),
            "the default workspace is an ordinary path (§2)"
        );
        assert!(
            !reserved.contains("/Users/me/.noending/datax"),
            "not a prefix match"
        );
        assert!(!reserved.contains("/Users/me/other/data"));

        // The raw-string gate normalizes first, so `..` cannot smuggle a
        // reserved path past the gate (or a normal path behind it). An explicit
        // home keeps this off the real user Home (§42.3-M13) — the ambient
        // `is_reserved_app_path` exists for production callers only.
        let opts = NormalizeOpts {
            style: Some(PathStyle::Unix),
            base: None,
            home: Some("/Users/me"),
        };
        assert!(is_reserved_app_path_with(
            "~/.noending/../.noending/data",
            &home,
            opts
        ));
        assert!(!is_reserved_app_path_with(
            "~/.noending/data/../workspace",
            &home,
            opts
        ));
        assert!(!is_reserved_app_path_with("relative/data", &home, opts));
        assert!(!is_reserved_app_path_with("", &home, opts));
    }

    /// 必须测试: Windows separator/case behavior — driven by an explicit
    /// `PathStyle`, so it verifies Windows rules from a macOS runner.
    #[test]
    fn reserved_paths_windows_spelling() {
        let home = NoEndingHome::new_with_style(
            "c:\\Users\\me\\.noending",
            Some("C:\\Users\\me"),
            PathStyle::Windows,
        )
        .unwrap();
        // Drive letter is uppercased by `identity`; the rest keeps its case.
        assert_eq!(home.root, PathBuf::from("C:\\Users\\me\\.noending"));
        let reserved = home.reserved();
        assert!(reserved.contains("C:\\Users\\me\\.noending\\data"));
        // A backslash and a slash must not become two different trees.
        assert!(reserved.contains("C:/Users/me/.noending/data"));
        assert!(reserved.contains("C:\\Users\\me\\.noending\\data\\noending.db"));
        assert!(!reserved.contains("D:\\Users\\me\\.noending\\data"));
        assert!(!reserved.contains("C:\\Users\\me\\.noending\\workspace"));
        assert!(!reserved.contains("C:\\Users\\me\\.noending\\datax"));
        // Case: on a Windows volume the spelling is not the location, so a
        // case-variant of `data/` IS `data/` and §2 reserves it. Compared with
        // `path_key` this leaked — the app's own database directory could become
        // a WorkspacePath, and §2's reservation cannot self-heal afterwards the
        // way two spellings of one repository do (Git convergence).
        assert!(reserved.contains_with("c:\\Users\\me\\.noending\\data", PathStyle::Windows));
        assert!(reserved.contains_with("C:\\Users\\ME\\.noending\\DATA", PathStyle::Windows));
        assert!(reserved.contains_with("C:\\Users\\me\\.NOENDING", PathStyle::Windows));
        // Segment-wise even when folded: `datax` is a different directory.
        assert!(!reserved.contains_with("C:\\Users\\ME\\.noending\\DATAX", PathStyle::Windows));
        assert!(!reserved.contains_with("C:\\Users\\ME\\.noending\\workspace", PathStyle::Windows));
        let opts = NormalizeOpts {
            style: Some(PathStyle::Windows),
            base: None,
            home: Some("C:\\Users\\me"),
        };
        // The raw-string gate re-derives the canonical form, so both the tilde
        // form and the mixed-separator form are caught.
        assert!(is_reserved_app_path_with("~\\.noending\\logs", &home, opts));
        assert!(is_reserved_app_path_with(
            "C:/Users/me/.noending/logs",
            &home,
            opts
        ));
        assert!(!is_reserved_app_path_with(
            "C:/Users/me/.noending/workspace",
            &home,
            opts
        ));
        // The raw-string gate normalizes first and reserves on the *location*,
        // so a mid-path case variant of a reserved directory is still reserved
        // (`normalize_path` uppercases the drive and preserves the rest, which is
        // exactly why the comparison has to fold).
        assert!(is_reserved_app_path_with(
            "C:/Users/me/.NOENDING/Logs",
            &home,
            opts
        ));
        assert!(is_reserved_app_path_with("~\\.NOENDING\\data", &home, opts));
        // A case-variant input normalizes to the same canonical path the Home
        // was built from, so it *is* caught: identity folds the drive, not the
        // rest, and here the differing part is the drive.
        assert!(is_reserved_app_path_with(
            "c:\\Users\\me\\.noending\\logs",
            &home,
            opts
        ));
        assert!(!is_reserved_app_path_with(
            "~\\..\\..\\Windows\\System32",
            &home,
            opts
        ));
    }

    /// The counter-check for the fold above: on a Unix-style Home case is *not*
    /// folded, so a differently-spelled directory stays an ordinary directory.
    /// A real APFS volume may resolve both to one inode, but NoEnding's
    /// reservation is defined over `canonical_path`, and folding it there would
    /// make the gate disagree with the path stored beside it (§42.3-M8.4).
    #[test]
    fn reserved_paths_unix_keeps_case() {
        let home =
            NoEndingHome::new_with_style("/Users/me/.noending", Some("/Users/me"), PathStyle::Unix)
                .unwrap();
        let reserved = home.reserved();
        assert!(reserved.contains_with("/Users/me/.noending/data", PathStyle::Unix));
        assert!(reserved.contains_with("/Users/me/.noending", PathStyle::Unix));
        assert!(reserved.contains_with("/Users/me/.noending/data/noending.db", PathStyle::Unix));
        assert!(!reserved.contains_with("/Users/me/.noending/DATA", PathStyle::Unix));
        assert!(!reserved.contains_with("/Users/ME/.noending/data", PathStyle::Unix));
        assert!(!reserved.contains_with("/Users/me/.noending/workspace", PathStyle::Unix));
        assert!(!reserved.contains_with("/Users/me/.noending/datax", PathStyle::Unix));
        let unix = NormalizeOpts {
            style: Some(PathStyle::Unix),
            base: None,
            home: Some("/Users/me"),
        };
        assert!(is_reserved_app_path_with("~/.noending/logs", &home, unix));
        assert!(!is_reserved_app_path_with("~/.noending/LOGS", &home, unix));
        assert!(!is_reserved_app_path_with(
            "~/.noending/workspace",
            &home,
            unix
        ));
    }

    #[test]
    fn default_workspace_is_derived() {
        // 必须测试 (§16-5): `<home>/workspace`, and `ensure_dirs` is what makes
        // it real; the fs part is covered in tests/workspace_resolver_test.rs.
        let home = home_at("/Users/me/.noending");
        assert_eq!(
            home.default_workspace,
            PathBuf::from("/Users/me/.noending/workspace")
        );
        assert_eq!(home.logs_dir, PathBuf::from("/Users/me/.noending/logs"));
        assert_eq!(
            home.runtime_dir,
            PathBuf::from("/Users/me/.noending/runtime")
        );
        // A hostile-looking root still keeps its children inside it.
        let tricky = home_at("/Users/me/.noending");
        assert_eq!(tricky.data_dir, PathBuf::from("/Users/me/.noending/data"));
        assert!(!tricky.db_path.to_string_lossy().contains(".."));
    }

    /// The bootstrap pointer is the only file this module writes outside the
    /// Home, and it must round-trip through JSON (§42.3-M12).
    #[test]
    fn bootstrap_pointer_round_trip_and_bad_file() {
        let dir = unique_temp_dir("pointer");
        let path = dir.join(BOOTSTRAP_FILE_NAME);
        let pointer = BootstrapPointer {
            current_home: Some("/Users/me/.noending".into()),
            pending_home: Some("/Volumes/x/.noending".into()),
        };
        pointer.save(&path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"current_home\""), "{raw}");
        assert!(raw.contains("\"pending_home\""), "{raw}");
        let (loaded, note) = BootstrapPointer::load(&path);
        assert_eq!(loaded, pointer);
        assert!(note.is_none());

        // Unknown fields ignored: a newer app must not lose the user's Home.
        std::fs::write(&path, b"{\"current_home\":\"/a\",\"future\":true}").unwrap();
        let (loaded, note) = BootstrapPointer::load(&path);
        assert_eq!(loaded.current_home.as_deref(), Some("/a"));
        assert!(note.is_none());

        // Garbage falls back AND says so.
        std::fs::write(&path, b"{ not json").unwrap();
        let (loaded, note) = BootstrapPointer::load(&path);
        assert_eq!(loaded, BootstrapPointer::default());
        assert!(note.is_some(), "must never be silent");

        // Missing file is not an error at all.
        let (loaded, note) = BootstrapPointer::load(&dir.join("absent.json"));
        assert_eq!(loaded, BootstrapPointer::default());
        assert!(note.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// §44.3-C1 — both platforms' spelling in one run. The expected strings are
    /// written by hand: deriving them through this module would assert that
    /// `request_relocation` agrees with itself, which is not the claim.
    #[test]
    fn relocation_request_writes_pending_only() {
        // Unix Home, Unix target.
        let dir = unique_temp_dir("relocate");
        let path = dir.join(BOOTSTRAP_FILE_NAME);
        BootstrapPointer {
            current_home: Some("/Users/me/.noending".into()),
            pending_home: None,
        }
        .save(&path)
        .unwrap();

        let after =
            request_relocation_with(&path, "~/new-home", Some("/Users/me"), PathStyle::Unix)
                .unwrap();
        assert_eq!(after.current_home.as_deref(), Some("/Users/me/.noending"));
        assert_eq!(after.pending_home.as_deref(), Some("/Users/me/new-home"));
        // The live Home is unchanged on disk: `current_home` still points there.
        let (reread, _) = BootstrapPointer::load(&path);
        assert_eq!(reread, after);

        // Asking for the current location is refused rather than scheduled.
        assert!(request_relocation_with(
            &path,
            "/Users/me/.noending",
            Some("/Users/me"),
            PathStyle::Unix
        )
        .is_err());
        // On Unix a case difference is a different directory, so it is a real
        // relocation and must be accepted.
        assert!(request_relocation_with(
            &path,
            "/Users/me/.NoEnding",
            Some("/Users/me"),
            PathStyle::Unix
        )
        .is_ok());
        std::fs::remove_dir_all(&dir).ok();

        // Windows Home, Windows target — same runner, same three claims, and the
        // host's separators are not what the pointer gets.
        let dir = unique_temp_dir("relocate-win");
        let path = dir.join(BOOTSTRAP_FILE_NAME);
        BootstrapPointer {
            current_home: Some("C:\\Users\\me\\.noending".into()),
            pending_home: None,
        }
        .save(&path)
        .unwrap();

        let after = request_relocation_with(
            &path,
            "~\\new-home",
            Some("C:\\Users\\me"),
            PathStyle::Windows,
        )
        .unwrap();
        assert_eq!(
            after.current_home.as_deref(),
            Some("C:\\Users\\me\\.noending")
        );
        assert_eq!(
            after.pending_home.as_deref(),
            Some("C:\\Users\\me\\new-home")
        );
        // A case variant of the current Home is the same location, so it must be
        // refused: scheduled, it would move `data/` onto itself.
        assert!(request_relocation_with(
            &path,
            "c:\\users\\ME\\.NoEnding",
            Some("C:\\Users\\me"),
            PathStyle::Windows
        )
        .is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 必须测试-adjacent (§16-3): the move happens, `workspace/` does not, and
    /// a retry converges instead of double-applying.
    #[test]
    fn data_root_migration_moves_app_dirs_and_keeps_workspace() {
        let base = unique_temp_dir("migrate");
        let from = base.join("old-home");
        let to = base.join("new-home");
        for dir in MIGRATABLE_DIRS {
            std::fs::create_dir_all(from.join(dir)).unwrap();
            std::fs::write(from.join(dir).join("f.txt"), dir).unwrap();
        }
        std::fs::create_dir_all(from.join(WORKSPACE_DIR_NAME)).unwrap();
        std::fs::write(
            from.join(WORKSPACE_DIR_NAME).join("user-work.md"),
            b"keep me",
        )
        .unwrap();

        let pointer_path = base.join(BOOTSTRAP_FILE_NAME);
        BootstrapPointer {
            current_home: Some(from.to_string_lossy().to_string()),
            pending_home: Some(to.to_string_lossy().to_string()),
        }
        .save(&pointer_path)
        .unwrap();
        let migration = PendingMigration {
            pointer_path: pointer_path.clone(),
            from: from.clone(),
            to: to.clone(),
        };

        let report = migrate_data_root(&migration).unwrap();
        assert_eq!(report.dirs.len(), MIGRATABLE_DIRS.len());
        assert!(report.dirs.iter().all(|(_, o)| *o == DirOutcome::Moved));
        for dir in MIGRATABLE_DIRS {
            assert!(to.join(dir).join("f.txt").is_file(), "{dir} moved");
        }
        // §4: user files are never relocated without being asked.
        assert!(from.join(WORKSPACE_DIR_NAME).join("user-work.md").is_file());
        assert!(!to.join(WORKSPACE_DIR_NAME).exists());
        let left_behind = from.join(WORKSPACE_DIR_NAME).to_string_lossy().to_string();
        assert_eq!(
            report.workspace_left_behind.as_deref(),
            Some(left_behind.as_str())
        );
        // Pointer flipped, pending cleared: the next start has nothing to do.
        let (pointer, _) = BootstrapPointer::load(&pointer_path);
        assert_eq!(pointer.current_home, Some(to.to_string_lossy().to_string()));
        assert_eq!(pointer.pending_home, None);

        // Retry after a crash *between* staging and the final rename: the stage
        // is promoted, and the already-empty source is not re-created.
        let mid = base.join("mid-home");
        std::fs::create_dir_all(from.join(DATA_DIR_NAME)).unwrap();
        std::fs::write(from.join(DATA_DIR_NAME).join("g.txt"), b"g").unwrap();
        std::fs::create_dir_all(&mid).unwrap();
        let staging = mid.join(format!("{DATA_DIR_NAME}.migrating"));
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("staged.txt"), b"s").unwrap();
        let migration2 = PendingMigration {
            pointer_path: pointer_path.clone(),
            from: from.clone(),
            to: mid.clone(),
        };
        let report2 = migrate_data_root(&migration2).unwrap();
        assert!(
            mid.join(DATA_DIR_NAME).join("staged.txt").is_file(),
            "stage promoted instead of copied twice"
        );
        // `runtime/` had already moved in the first migration: a re-run must not
        // invent it in the new Home.
        assert!(!mid.join(RUNTIME_DIR_NAME).exists());
        assert!(report2
            .dirs
            .iter()
            .any(|(n, o)| n == RUNTIME_DIR_NAME && *o == DirOutcome::NothingToMove));
        assert_eq!(
            report2
                .dirs
                .iter()
                .find(|(n, _)| n == DATA_DIR_NAME)
                .map(|(_, o)| *o),
            Some(DirOutcome::Moved)
        );

        // An occupied target is never overwritten.
        let occupied = base.join("occupied");
        std::fs::create_dir_all(occupied.join(DATA_DIR_NAME)).unwrap();
        std::fs::write(occupied.join(DATA_DIR_NAME).join("mine.txt"), b"mine").unwrap();
        std::fs::create_dir_all(from.join(DATA_DIR_NAME)).unwrap();
        std::fs::write(from.join(DATA_DIR_NAME).join("theirs.txt"), b"t").unwrap();
        let report3 = migrate_data_root(&PendingMigration {
            pointer_path: pointer_path.clone(),
            from: from.clone(),
            to: occupied.clone(),
        })
        .unwrap();
        assert!(occupied.join(DATA_DIR_NAME).join("mine.txt").is_file());
        assert!(!occupied.join(DATA_DIR_NAME).join("theirs.txt").is_file());
        assert!(
            from.join(DATA_DIR_NAME).join("theirs.txt").is_file(),
            "source kept"
        );
        assert!(report3
            .dirs
            .iter()
            .any(|(n, o)| n == DATA_DIR_NAME && *o == DirOutcome::TargetExisted));
        // …and the pointer still advanced: keeping the target IS the resolution.
        let (pointer, _) = BootstrapPointer::load(&pointer_path);
        assert_eq!(
            pointer.current_home,
            Some(occupied.to_string_lossy().to_string())
        );

        // A target nested inside a migratable directory is refused, and the
        // pointer must not move.
        let before = BootstrapPointer::load(&pointer_path).0;
        let nested = from.join(DATA_DIR_NAME).join("inside");
        let err = migrate_data_root(&PendingMigration {
            pointer_path: pointer_path.clone(),
            from: from.clone(),
            to: nested.clone(),
        });
        assert!(err.is_err());
        assert_eq!(BootstrapPointer::load(&pointer_path).0, before);
        std::fs::remove_dir_all(&base).ok();
    }

    /// §16-3/§3 end to end, with real temp directories and NO env, NO real
    /// `~/.noending` (§42.3-M13).
    #[test]
    fn prepare_home_restarts_after_a_pending_relocation() {
        let base = unique_temp_dir("prepare");
        let old = base.join("old");
        let new = base.join("new");
        std::fs::create_dir_all(old.join(DATA_DIR_NAME)).unwrap();
        std::fs::write(old.join(DATA_DIR_NAME).join("payload"), b"p").unwrap();
        std::fs::create_dir_all(old.join(WORKSPACE_DIR_NAME)).unwrap();
        std::fs::write(old.join(WORKSPACE_DIR_NAME).join("work.txt"), b"w").unwrap();
        let pointer_path = base.join(BOOTSTRAP_FILE_NAME);
        BootstrapPointer {
            current_home: Some(old.to_string_lossy().to_string()),
            pending_home: Some(new.to_string_lossy().to_string()),
        }
        .save(&pointer_path)
        .unwrap();

        let outcome = prepare_home(&StartupInputs {
            explicit: None,
            pointer_path: Some(pointer_path.clone()),
            user_home: Some(base.clone()),
        })
        .unwrap();
        assert_eq!(outcome.home.root, new);
        assert_eq!(outcome.source, HomeSource::Bootstrap);
        assert!(!outcome.restart_required);
        assert!(outcome.pending_home.is_none());
        assert_eq!(outcome.reports.len(), 1);
        assert!(outcome.notes.iter().any(|n| n.contains("workspace")));
        // §42.3-M21: the new default workspace exists as a directory.
        assert!(outcome.home.default_workspace.is_dir());
        assert!(outcome.home.logs_dir.is_dir());
        assert!(new.join(DATA_DIR_NAME).join("payload").is_file());
        assert!(old.join(WORKSPACE_DIR_NAME).join("work.txt").is_file());
        // The pointer now says the new Home is current.
        assert_eq!(
            BootstrapPointer::load(&pointer_path).0.current_home,
            Some(new.to_string_lossy().to_string())
        );
        // §11 `get_workspace_settings` shape, straight off the same resolution.
        let settings = outcome.settings();
        assert_eq!(settings.noending_home, new.to_string_lossy());
        assert_eq!(settings.db_path, outcome.home.db_path_str());
        assert!(!settings.restart_required);
        assert!(settings.pending_home.is_none());

        // Now ask for an *impossible* relocation (the target sits inside the
        // `data/` directory that would have to move) and prove the invariant:
        // a failed move leaves `current_home` alone and the app keeps working
        // on the Home it has (§3).
        let nested = new.join(DATA_DIR_NAME).join("nested");
        BootstrapPointer {
            current_home: Some(new.to_string_lossy().to_string()),
            pending_home: Some(nested.to_string_lossy().to_string()),
        }
        .save(&pointer_path)
        .unwrap();
        let blocked = prepare_home(&StartupInputs {
            explicit: None,
            pointer_path: Some(pointer_path.clone()),
            user_home: Some(base.clone()),
        })
        .unwrap();
        assert_eq!(blocked.home.root, new, "still the working Home");
        assert!(blocked.restart_required, "and still asking for a retry");
        let nested_str = nested.to_string_lossy().to_string();
        assert_eq!(blocked.pending_home.as_deref(), Some(nested_str.as_str()));
        assert!(blocked.notes.iter().any(|n| n.contains("迁移未完成")));
        assert!(blocked.reports.is_empty());
        assert_eq!(
            BootstrapPointer::load(&pointer_path).0.current_home,
            Some(new.to_string_lossy().to_string()),
            "current_home untouched after a failed move"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn ensure_dirs_creates_the_default_workspace() {
        // §42.3-M21 — the plan wants a launchable directory, not a dangling string.
        let base = unique_temp_dir("ensure");
        let home = NoEndingHome::new(base.join("h").to_str().unwrap(), None).unwrap();
        assert!(!home.default_workspace.exists());
        home.ensure_dirs().unwrap();
        assert!(home.default_workspace.is_dir());
        assert!(home.data_dir.is_dir());
        assert!(home.runtime_dir.is_dir());
        assert!(home.logs_dir.is_dir());
        // Idempotent: called on every start.
        home.ensure_dirs().unwrap();
        std::fs::remove_dir_all(&base).ok();
    }

    /// §3: an explicit override outranks the pointer for one launch and must not
    /// be persisted — a one-off env var silently becoming the data root would
    /// look exactly like the user's History emptying out on the next start.
    #[test]
    fn an_explicit_override_is_not_persisted() {
        let base = unique_temp_dir("explicit");
        let pointer_path = base.join(BOOTSTRAP_FILE_NAME);
        let recorded = base.join("recorded").to_string_lossy().to_string();
        BootstrapPointer {
            current_home: Some(recorded.clone()),
            pending_home: None,
        }
        .save(&pointer_path)
        .unwrap();

        let outcome = prepare_home(&StartupInputs {
            explicit: Some(base.join("env-home")),
            pointer_path: Some(pointer_path.clone()),
            user_home: Some(base.clone()),
        })
        .unwrap();
        assert_eq!(outcome.source, HomeSource::ExplicitEnv);
        assert_eq!(outcome.home.root, base.join("env-home"));
        assert!(outcome
            .notes
            .iter()
            .any(|n| n.contains("$NOENDING_HOME") && n.contains("未改写")));
        assert!(outcome.home.default_workspace.is_dir(), "§42.3-M21");

        let (pointer, _) = BootstrapPointer::load(&pointer_path);
        assert_eq!(pointer.current_home.as_deref(), Some(recorded.as_str()));
        assert_eq!(pointer.pending_home, None);
        std::fs::remove_dir_all(&base).ok();
    }

    fn unique_temp_dir(tag: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("noending-home-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // §42.3-M8: never assert on a temp prefix without normalizing it —
        // `/tmp` is a symlink on macOS runners.
        identity::normalize_path(&dir.to_string_lossy()).unwrap();
        dir
    }
}
