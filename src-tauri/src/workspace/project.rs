//! Project projection — the WorkspacePath registry and every ownership rule.
//!
//! Owned by Agent B (方案 §17). Consumes `WorkspaceObservation`; never
//! re-implements path identity or Git detection.
//!
//! ## Entry point
//!
//! ```text
//! ensure_workspace_path(db, observation) -> Result<WorkspacePath>
//! reconcile_workspace_path(db, path_id)  -> Result<WorkspacePath>   // §26, the only reconcile
//! ```
//!
//! Both are the whole of §8's decision table, and they are the ONLY writers of
//! `workspace_paths.project_id` and `projects.git_id`:
//!
//! | situation | action |
//! | --- | --- |
//! | known path, no Git now | keep Project + `git_id`; `detected → missing` |
//! | new path, no Git | create path-backed Project (`git_id = NULL`) |
//! | new path, Git `G` known to Project B | join B |
//! | new path, Git `G` unrecognized | create `git_identities` row + git-backed Project |
//! | known path upgrades to `G`, `G` unrecognized | set current Project's `git_id = G`; `Project.id` unchanged |
//! | known path upgrades to `G`, `G` owned by B | merge into B (canonical pick), zero-path Project deleted |
//! | known path now reports a different `G2` | reassign path to `G2`'s Project; batch-refresh Sessions; old Project deleted if empty |
//!
//! ## Hard rules
//!
//! * `projects.git_id` is UNIQUE when non-null (partial index) and
//!   `workspace_paths.project_id` is NOT NULL: a WorkspacePath belongs to
//!   exactly one Project, a Project owns at least one.
//! * Deleting the last WorkspacePath deletes the Project — in FK order
//!   `project_resources` → `project_affinity_evidence` → `projects`, then
//!   `unindex("project", id)` after commit (§42.3-M4). `foreign_keys` is ON, so
//!   the naive `DELETE FROM projects` fails on any Project that ever had a
//!   resource row.
//! * A `WorkspacePath` is only physically GC'd with 0 Session references, 0
//!   WorkstreamPath references and no longer a discovered worktree of any live
//!   Project (§10). A missing directory or a lost `.git` sets `exists` /
//!   `git_state` and changes nothing else.
//! * `name_customized` wins over every automatic rename: after a merge, after
//!   worktree discovery, after a Home move.
//! * Reconcile must not hold the DB mutex across a `git` call (§42.3-M7), must
//!   not write `workstream_paths` (§9), and must not advance any cursor.
//! * `git_identities.id` is app-assigned (uuid), unlike `workspace_paths.id`
//!   which is derived from the path — see `identity` module docs and design §8.
//! * WorkspacePath / Project mutations must re-index the affected search rows
//!   (`index_project` / `unindex`, and the workstream rows whose primary-path
//!   Project changed) — §42.3-M18.
