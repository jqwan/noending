//! Session → WorkspacePath, and the derived Project cache.
//!
//! This module owns Session-to-WorkspacePath policy and Owner assignment.
//!
//! ## The fact chain
//!
//! ```text
//! Session.cwd  →  workspace_path_id  →  workspace_paths.project_id  →  Project
//! ```
//!
//! `sessions.project_id` stays in the schema as a **derived cache** and is
//! written in exactly two places:
//!
//! 1. inside `upsert_session`, as `project_id = (SELECT project_id FROM
//!    workspace_paths WHERE id = <workspace_path_id>)` — same statement, so the
//!    cache and its source cannot be set apart;
//! 2. the batch refresh when a WorkspacePath changes Project, from
//!    `workspace::project`.
//!
//! Any other `UPDATE sessions SET project_id` is a domain violation.
//!
//! ## Rules
//!
//! * A Session with no cwd keeps `workspace_path_id = NULL`. Never backfill it
//!   with the default workspace — that would fabricate a physical fact the
//!   transcript does not contain.
//! * Ordinary event ingestion must not re-resolve a Project. Project work
//!   happens when `workspace_path_id` changes or a WorkspacePath is reassigned
//!; the per-event hot path stays as it is.
//! * Discovery is authoritative for cwd, so `workspace_path_id` follows it: add
//!   it to the `ON CONFLICT` update set alongside `cwd`.
//! * Setting a Session's Owner writes `owner_workstream_id` and nothing else —
//!   no WorkstreamPath is added, and cwd / `workspace_path_id` / `project_id`
//!   stay put.
//! * `launch_intents` keep their existing `cwd` and gain no workspace_path_id:
//!   a fourth place to store a path is a fourth chance to be wrong.
//!
//! ## Wiring
//!
//! The WorkspacePath creator is [`WorkspaceAttaching`], and it is *injected*:
//! every policy entry point here takes `&dyn WorkspaceAttaching` (or is reached
//! from one), so these rules are testable with a scripted stand-in while
//! `workspace::project` remains the only real implementation. Because Session
//! discovery runs on a background thread that has nowhere to carry one,
//! [`register_workspace_attacher`] holds the app-wide seam that ingestion's
//! logical-session resolution uses; [`UnattachedWorkspacePaths`] is what it
//! falls back to, and it answers "no path" to everything — never a guess.
//!
//! The doors, in the order the fact chain is walked:
//!
//! * [`resolve_session_path`] — cwd → WorkspacePath id (discovery's only
//!   workspace call);
//! * [`attach_session_conn`] — an existing row moves;
//! * [`attach_sessions_to_registered_paths`] — the same move for every Session
//!   discovery skipped, so a late-registered path still reaches them;
//! * [`set_session_owner`] — the one write door for semantic ownership.

use std::sync::{Arc, OnceLock};

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{other, Result};
use crate::storage::session_paths;
use crate::storage::Db;
use crate::workspace::WorkspaceAttaching;

// ------------------------------------------------------------ the seam

/// The seam when no workspace layer is wired: every path answers `Ok(None)`.
///
/// Discovery then leaves `workspace_path_id` NULL, which is the required
/// behaviour rather than a degradation — an absent WorkspacePath is a fact we do
/// not know, a fabricated one is a fact that is wrong.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnattachedWorkspacePaths;

impl WorkspaceAttaching for UnattachedWorkspacePaths {
    fn ensure_path(&self, _conn: &Connection, _raw_path: &str) -> Result<Option<String>> {
        Ok(None)
    }
}

/// The seam shared by every thread: discovery runs on a background reconcile
/// worker, user binds run on the command thread, so the registered creator has
/// to cross both.
pub type SharedAttacher = Arc<dyn WorkspaceAttaching + Send + Sync>;

static ATTACHER: OnceLock<SharedAttacher> = OnceLock::new();

/// Register the app-wide WorkspacePath creator — `workspace::project` is the
/// only permitted implementation.
///
/// Callers that already hold a seam must pass it explicitly (every policy
/// function here takes `&dyn WorkspaceAttaching`); the registry exists because
/// the background discovery thread has nowhere to carry one. Registering twice
/// is refused rather than silently swapping the authority mid-run.
pub fn register_workspace_attacher(
    attacher: SharedAttacher,
) -> std::result::Result<(), SharedAttacher> {
    ATTACHER.set(attacher)
}

/// The registered seam, or [`UnattachedWorkspacePaths`] if none is wired yet.
pub fn workspace_attacher() -> SharedAttacher {
    ATTACHER
        .get_or_init(|| Arc::new(UnattachedWorkspacePaths) as SharedAttacher)
        .clone()
}

// -------------------------------------------------------- path attaching

/// the WorkspacePath a Session's cwd names, resolved through the seam.
///
/// A missing or blank cwd is answered here instead of being handed to the seam:
/// "no path was observed" and "the observed path resolves to nothing" are
/// different statements, and the second one is Project policy's to make.
pub fn resolve_session_path(
    conn: &Connection,
    attacher: &dyn WorkspaceAttaching,
    cwd: Option<&str>,
) -> Result<Option<String>> {
    let Some(raw) = cwd.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    attacher.ensure_path(conn, raw)
}

/// Move an existing Session row onto a WorkspacePath: the path and its derived
/// Project cache are written in one statement. The Session's Owner Workstream is
/// not involved.
pub fn move_session_to_path_conn(
    conn: &Connection,
    session_id: &str,
    workspace_path_id: &str,
) -> Result<bool> {
    session_paths::attach_session_workspace_path_conn(conn, session_id, Some(workspace_path_id))
}

/// move an existing Session row onto its cwd's WorkspacePath and
/// recompute its cached Project in the same statement.
///
/// Returns whether the row actually moved. `Ok(false)` when the seam resolves to
/// nothing: an attachment is only ever replaced by a better fact, never deleted
/// because a resolver is unavailable (, change-discipline 1).
pub fn attach_session_conn(
    conn: &Connection,
    attacher: &dyn WorkspaceAttaching,
    session_id: &str,
    cwd: Option<&str>,
) -> Result<bool> {
    let Some(path_id) = resolve_session_path(conn, attacher, cwd)? else {
        return Ok(false);
    };
    // The column is NULL for a Session that has never been attached, so it has
    // to be read as an Option — and a missing row is read as an outer None.
    let current: Option<Option<String>> = conn
        .query_row(
            "SELECT workspace_path_id FROM sessions WHERE id = ?1",
            params![session_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?;
    let Some(current) = current else {
        return Ok(false);
    };
    if current.as_deref() == Some(path_id.as_str()) {
        return Ok(false);
    }
    move_session_to_path_conn(conn, session_id, &path_id)
}

/// attach the Sessions that owe a WorkspacePath but were never given
/// one, because discovery skipped their file.
///
/// `ingestion::ensure_logical_session` resolves the cwd while a row is created
/// or re-read, and discovery skips sources the stored cursors call unchanged —
/// so a Session ingested before its directory was registered (or before the
/// workspace layer was wired at all) keeps `workspace_path_id = NULL` forever.
/// Runs once per reconcile pass, after discovery.
///
/// Deliberately a JOIN and not the seam: the seam's job is to *establish* a
/// path's identity (canonicalisation, reserved verdict, git family), while here
/// that identity is already stored — `workspace_paths.canonical_path` equal to
/// the Session's cwd IS the resolution. The seam would re-observe every row,
/// `git` included. Sessions whose cwd is not registered are left alone:
/// registering a new directory is a decision, not a repair, so the
/// set this walks only ever shrinks.
pub fn attach_sessions_to_registered_paths(db: &Db) -> Result<usize> {
    let pairs: Vec<(String, String)> = {
        let conn = db.read();
        let mut stmt = conn.prepare(
            "SELECT s.id, wp.id
               FROM sessions s JOIN workspace_paths wp ON wp.canonical_path = s.cwd
              WHERE s.workspace_path_id IS NULL AND s.trashed_at IS NULL",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    if pairs.is_empty() {
        return Ok(0);
    }
    db.tx(|tx| {
        let mut attached = 0;
        for (session_id, path_id) in &pairs {
            attached += usize::from(move_session_to_path_conn(tx, session_id, path_id)?);
        }
        Ok(attached)
    })
}

// ------------------------------------------------------- owner assignment

/// Set (or clear) a Session's single Owner Workstream.
///
/// This is the only door for semantic ownership, and it writes exactly one
/// column. It never touches WorkstreamPath rows, the Session's cwd,
/// `workspace_path_id` or `project_id`: physical Project
/// membership and semantic ownership are independent. Returns the Session
/// after the change.
pub fn set_session_owner(
    db: &Db,
    session_id: &str,
    owner_workstream_id: Option<&str>,
) -> Result<crate::domain::Session> {
    db.set_session_owner(session_id, owner_workstream_id)?;
    db.get_session(session_id)?
        .ok_or_else(|| other(format!("未知 Session: {session_id}")))
}
