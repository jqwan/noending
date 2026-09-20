//! Session → WorkspacePath, and the derived Project cache.
//!
//! This module owns Session-to-WorkspacePath and binding policy.
//!
//! ## The fact chain
//!
//! ```text
//! Session.cwd  →  workspace_path_id  →  workspace_paths.project_id  →  Project
//! ```
//!
//! `sessions.project_id` stays in the schema as a **derived cache** and is
//! written in exactly two places (§42.3-M3):
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
//!   transcript does not contain (§5.5, §7.2).
//! * Ordinary event ingestion must not re-resolve a Project. Project work
//!   happens when `workspace_path_id` changes or a WorkspacePath is reassigned
//!   (§1.11); the per-event hot path stays as it is.
//! * Discovery is authoritative for cwd, so `workspace_path_id` follows it: add
//!   it to the `ON CONFLICT` update set alongside `cwd`.
//! * Binding a Session into a Workstream ensures the WorkstreamPath exists
//!   (append: empty list → position 0, otherwise last) and records
//!   `workstream_path_id` = the Session's own WorkspacePath, matched EXACTLY —
//!   no prefix or longest-match logic (§42.3-M1).
//! * Unbinding removes only the binding, never a WorkstreamPath (§1.9).
//! * If a Session's cwd drifts out of the Workstream's list, set the binding's
//!   `workstream_path_id` to NULL; do NOT append a path the user never chose
//!   (§42.3-M2). A NULL `workstream_path_id` is also not removed by a path
//!   deletion — we may not destroy a binding we cannot prove came from that path.
//! * `launch_intents` keep their existing `cwd` and gain no workspace_path_id:
//!   a fourth place to store a path is a fourth chance to be wrong (§42.3-M24).
//!
//! ## Wiring
//!
//! The WorkspacePath creator is [`WorkspaceAttaching`], and it is *injected*:
//! every policy entry point here takes `&dyn WorkspaceAttaching` (or is reached
//! from one), so these rules are testable with a scripted stand-in while
//! `workspace::project` remains the only real implementation. Because Session
//! discovery runs on a background thread that has nowhere to carry one,
//! [`register_workspace_attacher`] holds the app-wide seam that
//! `ingestion::ensure_session_row` uses; [`UnattachedWorkspacePaths`] is what it
//! falls back to, and it answers "no path" to everything — never a guess.
//!
//! The doors, in the order the fact chain is walked:
//!
//! * [`resolve_session_path`] — cwd → WorkspacePath id (discovery's only
//!   workspace call);
//! * [`attach_session_conn`] — an existing row moves, and its binding claims are
//!   reconciled in the same transaction;
//! * [`resolve_binding_path_conn`] — which WorkstreamPath brings a binding in,
//!   appending one only when a user action says the list should grow;
//! * [`record_user_binding`] / [`replace_session_bindings`] — the two write
//!   transactions the command layer calls.

use std::sync::{Arc, OnceLock};

use rusqlite::{params, Connection, OptionalExtension};

use crate::domain::{
    binding_source, workstream_path_source, Id, SessionWorkstreamBinding, WorkstreamPath,
};
use crate::error::{other, Result};
use crate::storage::session_paths;
use crate::storage::workstream_paths::{append_workstream_path_conn, find_workstream_path_conn};
use crate::storage::{now, Db};
use crate::workspace::WorkspaceAttaching;

/// A Session binding plus the WorkstreamPath that produced it, as consumed by
/// `replace_session_bindings`. Frozen in Wave 0 so the storage layer and the
/// command layer agree on the shape; `workstream_path_id` is optional because
/// legacy bindings and drifted Sessions legitimately have none.
#[derive(Debug, Clone)]
pub struct DesiredBinding {
    pub workstream_id: Id,
    pub role: String,
    pub workstream_path_id: Option<Id>,
}

// ------------------------------------------------------------ the seam

/// The seam when no workspace layer is wired: every path answers `Ok(None)`.
///
/// Discovery then leaves `workspace_path_id` NULL, which is the required
/// behaviour rather than a degradation — an absent WorkspacePath is a fact we do
/// not know, a fabricated one is a fact that is wrong (§5.5, §7.2).
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
/// only permitted implementation (方案 §42.4).
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

/// §19-1 — the WorkspacePath a Session's cwd names, resolved through the seam.
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

/// §42.3-M2 — the Session moved and its binding claims must follow: the
/// attachment and the reconciliation are one transaction, so an interrupted pass
/// can never leave a binding claiming a path the Session no longer has.
pub fn move_session_to_path_conn(
    conn: &Connection,
    session_id: &str,
    workspace_path_id: &str,
) -> Result<()> {
    session_paths::attach_session_workspace_path_conn(conn, session_id, Some(workspace_path_id))?;
    session_paths::reconcile_binding_paths_conn(conn, session_id)?;
    Ok(())
}

/// §19-2/3 — move an existing Session row onto its cwd's WorkspacePath and
/// recompute its cached Project in the same statement.
///
/// Returns whether the row actually moved. `Ok(false)` when the seam resolves to
/// nothing: an attachment is only ever replaced by a better fact, never deleted
/// because a resolver is unavailable (§42.3-M2, change-discipline 1).
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
    move_session_to_path_conn(conn, session_id, &path_id)?;
    Ok(true)
}

// ------------------------------------------------------- binding semantics

/// §1.8 + §42.3-M1 — which WorkstreamPath brings this Session into this
/// Workstream, creating it when a user action says the list should grow.
///
/// * `explicit_claim` — a caller that already names a WorkstreamPath row. It is
///   checked against the Workstream's list AND against the Session's own path;
///   naming anything else is a caller bug and is refused, not repaired.
/// * otherwise the Session's own `workspace_path_id` is matched against the list
///   by EXACT equality. No prefix logic, no longest-match (§42.3-M1).
/// * `allow_append` — only a user action (Binding Modal, explicit launch) may
///   grow the list. Absent paths go through `append_workstream_path_conn`, which
///   is position 0 for an empty list and last otherwise, and returns the existing
///   row untouched when the path is already there (§19-7). An automatic
///   classification never grows it (§42.3-M2).
///
/// `Ok(None)` means "not provably brought in by a path", which is a real state,
/// not a failure: callers must then leave any existing claim alone, because NULL
/// keeps a binding out of a path deletion's reach.
pub fn resolve_binding_path_conn(
    conn: &Connection,
    session_id: &str,
    workstream_id: &str,
    explicit_claim: Option<&str>,
    allow_append: bool,
) -> Result<Option<String>> {
    let session_path: Option<String> = conn
        .query_row(
            "SELECT workspace_path_id FROM sessions WHERE id = ?1",
            params![session_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    if let Some(claim) = explicit_claim {
        let row = find_workstream_path_conn_by_id(conn, workstream_id, claim)?;
        return match row {
            Some(row) if Some(row.workspace_path_id.as_str()) == session_path.as_deref() => {
                Ok(Some(row.id))
            }
            Some(_) => Err(other(
                "绑定的工作路径必须是该 Session 自身的工作路径，不接受其它路径",
            )),
            None => Err(other("该 Workstream 的路径列表中没有这条工作路径")),
        };
    }
    let Some(session_path) = session_path else {
        // A Session with no cwd has no path to bring in — the binding stays
        // legitimate with a NULL claim (§5.6, §19-11).
        return Ok(None);
    };
    if let Some(existing) = find_workstream_path_conn(conn, workstream_id, &session_path)? {
        return Ok(Some(existing.id));
    }
    if !allow_append {
        return Ok(None);
    }
    let appended = append_workstream_path_conn(
        conn,
        workstream_id,
        &session_path,
        workstream_path_source::SESSION,
    )?;
    Ok(Some(appended.id))
}

/// The one WorkstreamPath of `workstream_id` with this row id, if any.
fn find_workstream_path_conn_by_id(
    conn: &Connection,
    workstream_id: &str,
    workstream_path_id: &str,
) -> Result<Option<WorkstreamPath>> {
    Ok(conn
        .query_row(
            "SELECT * FROM workstream_paths WHERE workstream_id = ?1 AND id = ?2",
            params![workstream_id, workstream_path_id],
            |r| {
                Ok(WorkstreamPath {
                    id: r.get("id")?,
                    workstream_id: r.get("workstream_id")?,
                    workspace_path_id: r.get("workspace_path_id")?,
                    position: r.get("position")?,
                    source: r.get("source")?,
                    created_at: r.get("created_at")?,
                })
            },
        )
        .optional()?)
}

/// Write one binding row with its path claim. `bind_conn` keeps the provenance
/// precedence, lifts a removal tombstone for a strong write, and never lets a
/// NULL claim erase a provable one — so this is the only legal insert door here.
fn insert_binding_conn(
    conn: &Connection,
    session_id: &str,
    workstream_id: &str,
    role: &str,
    source: &str,
    confidence: f64,
    workstream_path_id: Option<&str>,
) -> Result<()> {
    crate::storage::bind_conn(
        conn,
        &SessionWorkstreamBinding {
            session_id: session_id.to_string(),
            workstream_id: workstream_id.to_string(),
            role: role.to_string(),
            source: source.to_string(),
            confidence,
            workstream_path_id: workstream_path_id.map(Into::into),
            last_seen_revision: None,
            last_sync_cursor: 0,
            created_at: now(),
            last_used_at: now(),
        },
    )
}

fn check_role(role: &str) -> Result<()> {
    if role == "primary" || role == "related" {
        Ok(())
    } else {
        Err(other(&format!("未知绑定角色: {role}")))
    }
}

/// §19-7/8 — one user binding: ensure the WorkstreamPath, then record the
/// binding already carrying its claim, in a single transaction.
pub fn record_user_binding(
    db: &Db,
    session_id: &str,
    workstream_id: &str,
    role: &str,
    source: &str,
    confidence: f64,
) -> Result<()> {
    record_user_binding_growing(
        db,
        session_id,
        workstream_id,
        role,
        source,
        confidence,
        true,
    )
}

/// [`record_user_binding`] with §1.7's path-list gate made explicit.
///
/// `grow_path_list = false` records the same user-strong binding but refuses to
/// add the Session's directory to the Workstream's ordered list. The launcher
/// passes `false` when the directory it launched into was NoEnding's own default
/// workspace: the user chose the *Workstream*, never that directory, so letting
/// it into the list would turn an app-invented fallback into durable Workstream
/// state — one the user then has to notice and remove, and whose removal (§1.6)
/// takes the Sessions under it with it.
pub fn record_user_binding_growing(
    db: &Db,
    session_id: &str,
    workstream_id: &str,
    role: &str,
    source: &str,
    confidence: f64,
    grow_path_list: bool,
) -> Result<()> {
    check_role(role)?;
    let append = grow_path_list
        && (source == binding_source::EXPLICIT_LAUNCH || source == binding_source::USER_ASSIGNED);
    db.tx(|tx| {
        let claim = resolve_binding_path_conn(tx, session_id, workstream_id, None, append)?;
        insert_binding_conn(
            tx,
            session_id,
            workstream_id,
            role,
            source,
            confidence,
            claim.as_deref(),
        )?;
        session_paths::reconcile_binding_paths_conn(tx, session_id)?;
        Ok(())
    })
}

/// §19-6 — the atomic replace behind the Binding Modal save.
///
/// The diff is the pre-existing one: unchanged rows keep provenance,
/// `created_at` and sync cursors verbatim, a role edit changes only the role (and
/// upgrades an AUTOMATIC provenance to user_assigned so it survives the next
/// classification), a user removal is tombstoned, and only a row the user just
/// added becomes a fresh user_assigned binding. On top of that, v0.2 adds the
/// path half of a join: every added binding gets its WorkstreamPath ensured and
/// recorded (§1.8), and every kept binding has a NULL claim repaired when the
/// Session's own path is now in the list (§5.6 "逐步 repair").
///
/// Removing a binding never removes a WorkstreamPath (§1.9), and the whole thing
/// commits or rolls back as one (§41 atomicity).
pub fn replace_session_bindings(
    db: &Db,
    session_id: &str,
    desired: &[DesiredBinding],
) -> Result<()> {
    for b in desired {
        check_role(&b.role)?;
    }
    // De-duplicate repeated workstreams (last entry wins, order preserved).
    let mut desired: Vec<DesiredBinding> = desired.iter().rev().fold(Vec::new(), |mut acc, b| {
        if !acc.iter().any(|d| d.workstream_id == b.workstream_id) {
            acc.push(b.clone());
        }
        acc
    });
    desired.reverse();

    let existing = db.bindings_for_session(session_id)?;
    db.tx(|tx| {
        for b in &existing {
            if !desired.iter().any(|d| d.workstream_id == b.workstream_id) {
                // a user rejection is durable for ANY provenance: tombstone the
                // pair so classification cannot re-propose it
                crate::storage::remove_binding_by_user_conn(tx, session_id, &b.workstream_id)?;
            }
        }
        for d in &desired {
            match existing.iter().find(|b| b.workstream_id == d.workstream_id) {
                Some(b) if b.role != d.role => {
                    // role-only edit: created_at, cursors and last_used_at stay;
                    // an auto provenance becomes user_assigned so the user's
                    // choice survives the next auto-classification.
                    tx.execute(
                        "UPDATE session_workstream_bindings
                            SET role = ?3,
                                source = CASE WHEN source = ?4 THEN ?5 ELSE source END,
                                confidence = CASE WHEN source = ?4 THEN 1.0 ELSE confidence END
                          WHERE session_id = ?1 AND workstream_id = ?2",
                        params![
                            session_id,
                            d.workstream_id,
                            d.role,
                            binding_source::AUTO,
                            binding_source::USER_ASSIGNED
                        ],
                    )?;
                }
                Some(_) => {} // unchanged: the row is kept exactly as-is
                None => {
                    // Only a binding the user just added is user_assigned, and
                    // only a user action may grow the Workstream's path list.
                    // bind_conn also lifts any removal tombstone for the pair.
                    let claim = resolve_binding_path_conn(
                        tx,
                        session_id,
                        &d.workstream_id,
                        d.workstream_path_id.as_deref(),
                        true,
                    )?;
                    insert_binding_conn(
                        tx,
                        session_id,
                        &d.workstream_id,
                        &d.role,
                        binding_source::USER_ASSIGNED,
                        1.0,
                        claim.as_deref(),
                    )?;
                }
            }
        }
        // Derived-data hygiene for the rows this edit kept rather than added: a
        // NULL claim whose path is now in the list is repaired, and a claim that
        // stopped matching (cwd drift, path removal) goes back to NULL. Both
        // statements are self-guarded, so a consistent Session is not rewritten.
        session_paths::reconcile_binding_paths_conn(tx, session_id)?;
        Ok(())
    })
}
