//! Session → WorkspacePath, and the derived Project cache.
//!
//! Owned by Agent D (方案 §19).
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
//! Any other `UPDATE sessions SET project_id` is a domain violation. The old
//! `assign_session_project` command (a raw write into the cache,
//! `commands.rs:786-789`) left the product API for exactly that reason.
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

use crate::domain::Id;

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
