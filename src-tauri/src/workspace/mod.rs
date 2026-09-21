//! Workspace Domain v0.2 — the physical side of the model.
//!
//! ```text
//! Filesystem path
//!     ↓ identity::normalize_path        (pure, deterministic)
//! WorkspacePath  ← identity::path_identity
//!     ↓ resolver observation (Git evidence)
//! Project        (app-managed; users may only rename)
//! ```
//!
//! and, on the user-intent side:
//!
//! ```text
//! Workstream → WorkstreamPath (ordered) → WorkspacePath → Project
//! Session    → workspace_path_id                          → Project
//! ```
//!
//! ## Layering
//!
//! * [`identity`]   — pure path/name functions. Main-owned; the only place a
//!   path string becomes an identity. No second implementation allowed.
//! * [`home`]       — NoEnding Home: bootstrap pointer, reserved paths, default
//!   workspace. Never touches Project.
//! * [`resolver`]   — observation only: canonicalize + Git detection + worktree
//!   listing. Produces `WorkspaceObservation`, never writes.
//! * [`project`]    — `ensure_workspace_path` and every Project-ownership rule
//!   (auto-create, upgrade, merge, reassign, zero-path deletion).
//! * [`workstream`] — the ordered WorkstreamPath list, lifecycle and recycle bin.
//! * [`session`]    — Session→WorkspacePath attach and the derived Project cache.
//!
//! ## Rules this layer must never break
//!
//! * Losing `.git` changes `WorkspacePath.git_state` and nothing else — not the
//!   path's Project, not the Project's `git_id` (§1.3).
//! * Discovering a worktree adds a `WorkspacePath`, never a `WorkstreamPath` (§9).
//! * Adding a `WorkstreamPath` never imports the Sessions under that path (§1.7).
//! * Removing a Session's binding never removes a `WorkstreamPath` (§1.9).
//! * A Home-level Git repository is not evidence (§1.4), and the reserved app
//!   paths under NoEnding Home never become a WorkspacePath (§2).
//! * Git detection runs *after* the database opens, in Workspace Reconcile —
//!   never inside schema initialization, and never while holding the DB mutex (§6, §42.3-M7).

pub mod home;
pub mod identity;
pub mod probe;
pub mod project;
pub mod resolver;
pub mod session;
pub mod wiring;
pub mod workstream;

pub use identity::{
    auto_project_name, basename, expand_tilde, home_dir, is_within, normalize_path,
    normalize_path_with, path_identity, path_identity_of, path_identity_with, path_key,
    NormalizeOpts, PathStyle,
};

/// The seam that turns a raw path string (a Session cwd, a user-chosen working
/// directory) into an existing [`domain::WorkspacePath`] id, creating the row —
/// and, through Project policy, its Project — when it is new.
///
/// It exists so the callers (Session discovery, Workstream path commands) can be
/// written and tested without depending on which of them owns Git detection.
/// `workspace::project` is the only permitted implementation; callers take
/// `&dyn` and stay independently testable with a scripted stand-in.
///
/// Contract:
/// * `Ok(None)` means "this string resolves to no path" (empty, or relative with
///   no base). Callers must then leave `workspace_path_id` NULL — never guess
///   (§5.5, §7.2).
/// * A reserved app path under NoEnding Home and a Home-level repository are not
///   WorkspacePaths (§2, §1.4); they also yield `Ok(None)`.
/// * Implementations write through the caller's connection so an atomic Sync run
///   commits or rolls back as one (§42.3-M3).
pub trait WorkspaceAttaching {
    fn ensure_path(
        &self,
        conn: &rusqlite::Connection,
        raw_path: &str,
    ) -> crate::error::Result<Option<String>>;
}
