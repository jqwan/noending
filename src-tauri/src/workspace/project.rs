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
//!
//! ## How that table is actually executed
//!
//! `ensure_workspace_path_conn` is the single implementation; every other entry
//! point is a wrapper around it, so the decision can only be made in one place:
//!
//! ```text
//! observation ────────────────┐
//!   (canonical, exists, git)  │
//!                             ↓
//!   normalize + recompute id  ├─ identity::path_identity is the authority;
//!                             │  observation.path_id is a copy we verify
//!                             │  against it, never the other way round
//!   resolve Git family        ├─ Detected → git_identities row keyed by
//!                             │  common_dir; anything else → no evidence
//!   apply §8 branch           ├─ create / keep / upgrade / merge / reassign
//!   refresh the row           ├─ exists_on_disk + git_state + git_kind only
//!   adopt sibling worktrees   └─ §9: new WorkspacePaths, never WorkstreamPaths
//! ```
//!
//! Two deliberate readings of the spec, both driven by "evidence is per-path":
//!
//! * A merge moves **all** of the losing Project's paths — which is only ever
//!   asked of a *path-backed* Project (one with no family identity of its own).
//!   A *git-backed* Project never loses its other paths: when one of them
//!   reports a different family, only that path moves (§8.5). Swallowing paths
//!   that produced no evidence would invent membership.
//! * Automatic naming happens **at Project creation only**. No reconcile, worktree
//!   discovery or merge renames an existing row; a merge may only *carry over* a
//!   `name_customized` name into a survivor that has no user-chosen name yet
//!   (§17-17). A name that changed every time a sibling appeared is a second
//!   authority for one fact.

use std::sync::Mutex;

use rusqlite::Connection;
use serde::Serialize;

use crate::domain::{
    git_state, GitDetection, GitWorktreeKind, Project, Session, WorkspaceObservation,
    WorkspacePath, Workstream,
};
use crate::error::{other, Result};
use crate::storage::workspace::{
    adopt_git_identity_conn, delete_workspace_path_conn, ensure_git_identity_conn,
    get_project_conn, get_workspace_path_conn, insert_workspace_path_conn,
    list_workspace_paths_for_project_conn, project_by_git_id_conn,
    reassign_workspace_path_project_conn, refresh_workstream_search_parents_conn,
    retire_project_if_unowned, scan_workspace_paths_conn, set_project_name_conn,
    touch_git_identity_conn, update_workspace_path_observation_conn, workspace_path_is_gcable,
};
use crate::storage::{new_id, now, upsert_project_conn, Db};
use crate::workspace::identity::{
    auto_project_name, normalize_path, normalize_path_with, path_identity, same_location,
    NormalizeOpts,
};
use crate::workspace::resolver::WorkspaceObserving;
use crate::workspace::WorkspaceAttaching;

/// What the Project policy may ask about the surrounding world without touching
/// it. `workspace::home` (Agent A) is the real implementation; this seam exists
/// so ownership rules are testable with a scripted answer instead of a Home
/// directory on disk (§42.3-M13).
pub trait WorkspacePolicy {
    /// The NoEnding Home itself and its reserved app directories are never a
    /// WorkspacePath (§2). Must be segment-wise (`identity::is_within`), not
    /// `starts_with`.
    fn is_reserved(&self, canonical_path: &str) -> bool {
        let _ = canonical_path;
        false
    }
    /// The current default workspace, used ONLY for automatic naming: exactly
    /// that path is called "NoEnding Workspace" (§37, §17-16).
    fn default_workspace(&self) -> Option<String> {
        None
    }
    /// Is this directory actually on this machine right now?
    ///
    /// Existence is an *observation* (§1.3), and this layer never reads the
    /// filesystem, so it asks rather than computing. Deliberately no default
    /// body: an implementor that has not been wired to a real host must choose
    /// an answer out loud, not inherit a guess about the user's disk.
    fn exists_on_disk(&self, canonical_path: &str) -> bool;
}

/// The policy for a registry that has no NoEnding Home in play yet (tests, and
/// any caller that has not resolved one): nothing is reserved and no path gets
/// the special name.
pub struct UnrestrictedWorkspace;

impl WorkspacePolicy for UnrestrictedWorkspace {
    fn exists_on_disk(&self, canonical_path: &str) -> bool {
        super::resolver::exists_on_disk(canonical_path)
    }
}

/// The WorkspacePath registry for one database. Holds the two things §8 needs
/// beyond SQL: who observes paths, and what the surrounding Home forbids.
pub struct ProjectProjection<'a> {
    observer: &'a dyn WorkspaceObserving,
    policy: &'a dyn WorkspacePolicy,
}

impl<'a> ProjectProjection<'a> {
    pub fn new(observer: &'a dyn WorkspaceObserving) -> Self {
        Self {
            observer,
            policy: &UnrestrictedWorkspace,
        }
    }

    pub fn with_policy(
        observer: &'a dyn WorkspaceObserving,
        policy: &'a dyn WorkspacePolicy,
    ) -> Self {
        Self { observer, policy }
    }

    pub fn observer(&self) -> &dyn WorkspaceObserving {
        self.observer
    }

    pub fn policy(&self) -> &dyn WorkspacePolicy {
        self.policy
    }
}

/// Everything one §8 application did beyond the path row itself.
///
/// The two lists exist because of §42.3-M4: FTS rows may only be dropped or
/// rewritten once the domain write has COMMITTED, so the transaction-scoped core
/// records what happened and the `Db`-scoped wrapper performs the index work
/// afterwards. A caller that runs the core inside its own transaction must do the
/// same, otherwise a rolled-back write leaves search describing a Project that
/// never came to be.
#[derive(Debug, Default, Clone)]
pub struct ProjectionEffect {
    /// Created, Git-upgraded or renamed: refresh `index_project` after commit.
    pub projects_touched: Vec<String>,
    /// Deleted as zero-path: `unindex("project", id)` after commit.
    pub projects_deleted: Vec<String>,
}

impl ProjectionEffect {
    /// Record a Project row whose indexed content (name, or existence) changed.
    pub fn touch(&mut self, id: &str) {
        push_unique(&mut self.projects_touched, id);
    }

    /// Record a Project row that was deleted. Mutually exclusive with `touch` for
    /// the same id, so a wrapper can never un-index and then re-index one row.
    pub fn delete(&mut self, id: &str) {
        push_unique(&mut self.projects_deleted, id);
        self.projects_touched.retain(|p| p != id);
    }

    /// Fold a nested decision's bookkeeping into this one (a merge inside an
    /// `ensure`, a GC inside a sweep).
    pub fn merge(&mut self, other: &ProjectionEffect) {
        for id in &other.projects_touched {
            self.touch(id);
        }
        for id in &other.projects_deleted {
            self.delete(id);
        }
    }
}

fn push_unique(list: &mut Vec<String>, id: &str) {
    if !list.iter().any(|known| known == id) {
        list.push(id.to_string());
    }
}

/// The result of applying one observation to the registry.
#[derive(Debug, Clone)]
pub struct EnsureOutcome {
    /// The path row AFTER the decision, so a reassignment is visible to the
    /// caller without a second read.
    pub path: WorkspacePath,
    /// Sibling worktrees §9 pulled in as new WorkspacePaths. They join the same
    /// Project; they are never added to any Workstream's path list.
    pub discovered: Vec<WorkspacePath>,
    pub effect: ProjectionEffect,
}

/// §8.4/§8.5 — a Project fold. `survivor`/`loser` are the outcome of
/// [`pick_canonical`], which is deterministic, so a replayed Sync run merges the
/// same pair the same way round.
#[derive(Debug, Clone)]
pub struct MergeOutcome {
    pub survivor: String,
    pub loser: String,
    pub moved_paths: Vec<String>,
    /// Set when the survivor took the loser's user-chosen name (§17-17).
    pub adopted_name: Option<String>,
}

/// §10 — what a GC pass removed.
#[derive(Debug, Default, Clone)]
pub struct GcOutcome {
    pub deleted_paths: Vec<String>,
    pub deleted_projects: Vec<String>,
}

/// §42.3-M7 — one reconcile sweep.
#[derive(Debug, Default, Clone)]
pub struct ReconcileReport {
    pub scanned: usize,
    /// Paths that ended up under a different Project than they started.
    pub moved_paths: usize,
    /// New WorkspacePaths discovered through Git worktree listings (§9).
    pub discovered_paths: Vec<String>,
    /// Paths that could not be observed or committed. One bad path never aborts
    /// the sweep: §42.3-M9 forbids a filesystem or `git` problem from becoming a
    /// user-visible failure, and the registry is per-path independent anyway.
    pub failed: Vec<(String, String)>,
    pub outcome: GcOutcome,
}

// ---------------------------------------------------------------------------
// the door
// ---------------------------------------------------------------------------

/// §8 — the single door for "this path exists and belongs somewhere".
///
/// Runs inside the CALLER's transaction: an atomic Sync run or ingest batch
/// commits path registration together with everything derived from it (§42.3-M3).
/// Post-commit work (FTS) is described by [`ProjectionEffect`].
pub fn ensure_workspace_path_conn(
    conn: &Connection,
    observation: &WorkspaceObservation,
    policy: &dyn WorkspacePolicy,
) -> Result<EnsureOutcome> {
    let raw = observation.canonical_path.trim();
    if raw.is_empty() {
        return Err(other(
            "ensure_workspace_path 需要一个已规范化的绝对路径（observation.canonical_path 为空）",
        ));
    }
    // Identity is recomputed from the canonical spelling and nowhere else
    // (§42.2-E1). An observation whose own `path_id` copy disagrees would put the
    // same directory under two keys and split every chain that reads through it,
    // so this door refuses it; [`WorkspaceAttaching::ensure_path`] turns the same
    // contradiction into `Ok(None)` so one bad string cannot fail an ingest run.
    let canonical = normalize_path(raw).unwrap_or_else(|| raw.to_string());
    let id = path_identity(&canonical);
    if observation.path_id != id {
        return Err(other(format!(
            "观察与 workspace::identity 冲突：path_id {} 不是 {canonical} 的身份",
            observation.path_id
        )));
    }
    if policy.is_reserved(&canonical) {
        return Err(other(format!(
            "保留的 NoEnding Home 路径不会成为 WorkspacePath: {canonical}"
        )));
    }

    let family = git_family(conn, &canonical, &observation.git)?;
    let known = get_workspace_path_conn(conn, &id)?;
    let (state, kind) = next_git_observation(known.as_ref().map(|p| p.git_state.as_str()), &family);
    let mut effect = ProjectionEffect::default();
    let mut discovered = Vec::new();

    match known {
        // §8.2 / §8.3 — a path the registry has never seen always ends up under a
        // Project, because `project_id` is NOT NULL. A recognized family joins
        // its existing Project; otherwise a Project is created, git-backed only
        // when there is evidence to back it.
        None => {
            let project_id = match &family {
                Some(f) => match project_by_git_id_conn(conn, &f.git_id)? {
                    Some(owner) => owner.id,
                    None => {
                        create_project_row(conn, &canonical, Some(&f.git_id), policy, &mut effect)?
                            .id
                    }
                },
                None => create_project_row(conn, &canonical, None, policy, &mut effect)?.id,
            };
            insert_workspace_path_conn(conn, &canonical, &project_id)?;
        }
        // §8.1 / §8.4 / §8.5 — a known path keeps its Project unless THIS path's
        // own evidence says otherwise.
        Some(row) => {
            let mine = get_project_conn(conn, &row.project_id)?
                .ok_or_else(|| other(format!("WorkspacePath {} 指向了不存在的 Project", row.id)))?;
            match &family {
                // §8.1/§1.3 — no evidence, so NOTHING about ownership changes.
                // This is the branch that keeps a lost `.git` from detaching a
                // path, splitting a Project or starting a new one.
                None => {}
                Some(f) => {
                    match project_by_git_id_conn(conn, &f.git_id)? {
                        Some(owner) if owner.id == mine.id => {
                            // Already the family's Project; an upgrade may still be
                            // owed (§8.4).
                            if mine.git_id.is_none()
                                && adopt_git_identity_conn(conn, &mine.id, &f.git_id)?
                            {
                                effect.touch(&mine.id);
                            }
                        }
                        Some(owner) => {
                            if mine.git_id.is_none() {
                                // §8.4 — this path is the first family evidence its
                                // (identity-less) Project ever had, and the family
                                // already has a Project: the two are one thing, so
                                // merge. `pick_canonical` makes the git-backed side
                                // the survivor and carries a user's name across.
                                let merged = merge_projects_conn(conn, &mine.id, &owner.id)?;
                                effect.delete(&merged.loser);
                                effect.touch(&merged.survivor);
                            } else {
                                // §8.5 — a different family under a path that already
                                // belongs to a git-backed Project is a strong identity
                                // change: only this path moves. Its siblings keep their
                                // own family's Project.
                                if owner.id != row.project_id {
                                    reassign_workspace_path_project_conn(conn, &id, &owner.id)?;
                                }
                                if retire_project_if_unowned(conn, &mine.id)? {
                                    effect.delete(&mine.id);
                                }
                            }
                        }
                        None => {
                            if mine.git_id.is_none() {
                                // §8.4, first branch: adopt in place. `Project.id`
                                // does not move, so no Session, binding or audit
                                // reference has to be repaired.
                                if adopt_git_identity_conn(conn, &mine.id, &f.git_id)? {
                                    effect.touch(&mine.id);
                                }
                            } else {
                                // §8.5 with an unseen family: the new Project is
                                // created git-backed, this path moves into it, and
                                // the old one dies if it is now empty.
                                let fresh = create_project_row(
                                    conn,
                                    &canonical,
                                    Some(&f.git_id),
                                    policy,
                                    &mut effect,
                                )?;
                                reassign_workspace_path_project_conn(conn, &id, &fresh.id)?;
                                if retire_project_if_unowned(conn, &mine.id)? {
                                    effect.delete(&mine.id);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // The observation is refreshed after the ownership decision so a reassignment
    // cannot be undone by a later write of the same row, and so this stays the
    // only writer of the observed facts even on the create path.
    update_workspace_path_observation_conn(conn, &id, observation.exists, &state, kind.as_deref())?;
    if let Some(f) = &family {
        touch_git_identity_conn(conn, &f.git_id)?;
        let path =
            get_workspace_path_conn(conn, &id)?.ok_or_else(|| other("WorkspacePath 写入后消失"))?;
        discovered = adopt_sibling_worktrees(conn, &path, f, policy, &mut effect)?;
    }

    let path =
        get_workspace_path_conn(conn, &id)?.ok_or_else(|| other("WorkspacePath 写入后消失"))?;
    Ok(EnsureOutcome {
        path,
        discovered,
        effect,
    })
}

/// §8 — the self-transacting form. Use this when you are not already inside a
/// write; use [`ensure_workspace_path_conn`] when you are.
pub fn ensure_workspace_path(
    db: &Db,
    observation: &WorkspaceObservation,
    policy: &dyn WorkspacePolicy,
) -> Result<WorkspacePath> {
    Ok(ensure_workspace_path_outcome(db, observation, policy)?.path)
}

/// §8 — the self-transacting form that also reports what it did to Projects.
pub fn ensure_workspace_path_outcome(
    db: &Db,
    observation: &WorkspaceObservation,
    policy: &dyn WorkspacePolicy,
) -> Result<EnsureOutcome> {
    let outcome = db.tx(|tx| ensure_workspace_path_conn(tx, observation, policy))?;
    apply_projection_effect(db, &outcome.effect);
    Ok(outcome)
}

/// §42.3-M4/M18 — the post-commit half of the effect: Project search rows follow
/// the domain write, never lead it. Index failures are logged, not propagated:
/// a stale search row must not roll back a committed physical fact.
pub fn apply_projection_effect(db: &Db, effect: &ProjectionEffect) {
    for id in &effect.projects_deleted {
        db.unindex("project", id);
    }
    for id in &effect.projects_touched {
        match db.get_project(id) {
            Ok(Some(project)) => {
                if let Err(e) = db.index_project(&project) {
                    eprintln!("[workspace] 索引 Project {} 失败: {e}", project.id);
                }
            }
            Ok(None) => db.unindex("project", id),
            Err(e) => eprintln!("[workspace] 读取 Project {id} 以索引失败: {e}"),
        }
    }
}

/// §17 — the string door every caller shares: a Session cwd, a chosen working
/// directory, a worktree path.
///
/// `Ok(None)` means "this string is not a WorkspacePath": empty or otherwise
/// unresolvable, a reserved app path under NoEnding Home, or an observation whose
/// identity does not agree with `workspace::identity`. Callers must then leave
/// `workspace_path_id` NULL — never substitute a guess (§5.5, §7.2).
///
/// Lock discipline: this observes the filesystem, so a caller must not hold the
/// DB mutex across it (§42.3-M7). Batch work belongs in
/// [`reconcile_workspace_paths`], which releases the lock around every `git`
/// call. A `git` failure is `GitDetection::Unavailable` inside the observation,
/// never an `Err` from here (§42.3-M9).
impl WorkspaceAttaching for ProjectProjection<'_> {
    fn ensure_path(&self, conn: &Connection, raw_path: &str) -> Result<Option<String>> {
        Ok(self
            .ensure_path_outcome(conn, raw_path)?
            .map(|outcome| outcome.path.id))
    }
}

impl ProjectProjection<'_> {
    /// The same door, for a caller that owns the transaction and therefore also
    /// owns the post-commit half of the work: collect the returned
    /// [`ProjectionEffect`] and hand it to [`apply_projection_effect`] once the
    /// transaction commits. Without that, a Project created inside someone else's
    /// write is correct but un-indexed until the next reconcile pass.
    pub fn ensure_path_outcome(
        &self,
        conn: &Connection,
        raw_path: &str,
    ) -> Result<Option<EnsureOutcome>> {
        if raw_path.trim().is_empty() {
            return Ok(None);
        }
        let observation = self.observer.observe(raw_path);
        let Some(canonical) = normalize_path(&observation.canonical_path) else {
            // The resolver could not make this absolute. Attaching a guessed
            // directory would create a physical fact out of nothing.
            return Ok(None);
        };
        if observation.path_id != path_identity(&canonical) {
            eprintln!(
                "[workspace] {} 的观察身份与 identity 不一致，忽略该路径而不写入任何一方",
                raw_path.trim()
            );
            return Ok(None);
        }
        if self.policy.is_reserved(&canonical) {
            return Ok(None);
        }
        Ok(Some(ensure_workspace_path_conn(
            conn,
            &observation,
            self.policy,
        )?))
    }
}

// ---------------------------------------------------------------------------
// §8 internals
// ---------------------------------------------------------------------------

/// A recognized family for one observation: the `git_identities` row plus the
/// worktree list the resolver reported with it.
#[derive(Debug, Clone)]
struct GitFamily {
    git_id: String,
    kind: String,
    worktrees: Vec<String>,
}

fn git_family(
    conn: &Connection,
    observed_canonical: &str,
    detection: &GitDetection,
) -> Result<Option<GitFamily>> {
    let GitDetection::Detected {
        common_dir,
        kind,
        worktrees,
        ..
    } = detection
    else {
        // `None` / `Unavailable` / `Missing`: no family evidence. Which of the
        // three it was only changes `git_state`, decided by
        // `next_git_observation`, never ownership.
        return Ok(None);
    };
    let common_dir = resolve_common_dir(observed_canonical, common_dir);
    if common_dir.is_empty() {
        // "Detected" with no common directory is not evidence of a family; a row
        // keyed on an empty string would silently adopt every other broken report.
        return Ok(None);
    }
    let git_id = ensure_git_identity_conn(conn, &common_dir)?;
    Ok(Some(GitFamily {
        git_id,
        kind: kind.as_str().to_string(),
        worktrees: worktrees.clone(),
    }))
}

/// `git rev-parse --git-common-dir` answers `.git` — relative to the directory it
/// was asked about — for a main worktree. Keying identities on that literal would
/// give one repository as many families as it has worktrees, i.e. exactly the
/// split §8.3 exists to prevent, so a relative answer is resolved against the
/// path we asked about. An already-absolute common dir is untouched (the resolver
/// is responsible for resolving git output; 方案 §42.3-M8 rule 6).
fn resolve_common_dir(observed_canonical: &str, common_dir: &str) -> String {
    let trimmed = common_dir.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    normalize_path(trimmed).unwrap_or_else(|| {
        normalize_path_with(
            trimmed,
            NormalizeOpts {
                style: None,
                base: Some(observed_canonical),
                home: None,
            },
        )
        // A common dir we cannot make absolute is keyed verbatim: it still
        // converges with any other report of the same string, and inventing a
        // base would be a guess about the filesystem.
        .unwrap_or_else(|| trimmed.to_string())
    })
}

/// The `git_state` / `git_kind` this observation implies.
///
/// `detected → missing` is §8.1; `missing → missing` is the point: a path that
/// once had evidence and does not now is remembered, not reclassified as "never
/// was a repository" (§1.3). `none → none` for a plain directory that is merely
/// unreadable right now.
fn next_git_observation(
    current: Option<&str>,
    family: &Option<GitFamily>,
) -> (String, Option<String>) {
    if let Some(f) = family {
        return (git_state::DETECTED.to_string(), Some(f.kind.clone()));
    }
    match current {
        Some(state) if state == git_state::DETECTED || state == git_state::MISSING => {
            (git_state::MISSING.to_string(), None)
        }
        // A first-ever observation without evidence is `none`: we have no history
        // that would make `missing` an honest statement.
        _ => (git_state::NONE.to_string(), None),
    }
}

fn create_project_row(
    conn: &Connection,
    canonical_path: &str,
    git_id: Option<&str>,
    policy: &dyn WorkspacePolicy,
    effect: &mut ProjectionEffect,
) -> Result<Project> {
    let ts = now();
    let project = Project {
        id: new_id(),
        // §37/§17-16: the basename, capitalized, and the exact default workspace
        // is "NoEnding Workspace". Never a guess about repositories or remotes.
        name: auto_project_name(canonical_path, policy.default_workspace().as_deref()),
        description: String::new(),
        // Column compatibility only: v0.2 gives a Project no lifecycle (§1.2).
        archived: false,
        git_id: git_id.map(|g| g.to_string()),
        // App-named by construction; only `rename_project` may set this (§17-15).
        name_customized: false,
        created_at: ts.clone(),
        updated_at: ts,
    };
    upsert_project_conn(conn, &project)?;
    effect.touch(&project.id);
    Ok(project)
}

/// §9 — a Git family's worktree list becomes WorkspacePaths, and only that.
///
/// Nothing here may touch `workstream_paths` (§9: 发现 WorkspacePath ≠ 添加
/// WorkstreamPath), and an existing row is left alone: we hold no observation of
/// our own about a sibling, so we must not overwrite facts another call owns.
/// A sibling therefore starts as `exists_on_disk = 0` — "learned about it, have
/// not stood there" — and the next direct scan of that path confirms or GCs it.
fn adopt_sibling_worktrees(
    conn: &Connection,
    path: &WorkspacePath,
    family: &GitFamily,
    policy: &dyn WorkspacePolicy,
    effect: &mut ProjectionEffect,
) -> Result<Vec<WorkspacePath>> {
    let mut created = Vec::new();
    for worktree in &family.worktrees {
        let Some(canonical) = normalize_path(worktree) else {
            continue;
        };
        // Same location, not same spelling: `git worktree list` and the row we
        // are holding may differ only in case on a Windows volume, and adopting
        // the "other" spelling would add a second WorkspacePath — and therefore
        // a second path to this Project — for one directory.
        if same_location(&canonical, &path.canonical_path) {
            continue;
        }
        if policy.is_reserved(&canonical) {
            continue;
        }
        let sibling_id = path_identity(&canonical);
        if get_workspace_path_conn(conn, &sibling_id)?.is_some() {
            continue;
        }
        insert_workspace_path_conn(conn, &canonical, &path.project_id)?;
        update_workspace_path_observation_conn(
            conn,
            &sibling_id,
            // Observed, not assumed. `git worktree list` reports registrations for
            // directories that may or may not still be on this machine, and a
            // hardcoded `false` here made the Projects page say "目录不存在"
            // about worktrees that were sitting right there until a later sweep
            // corrected the row.
            policy.exists_on_disk(&canonical),
            git_state::DETECTED,
            // Git listed this worktree of the same family; whether it is the main
            // one is not derivable from a sibling list, and guessing would be a
            // fabricated fact.
            Some(GitWorktreeKind::Unknown.as_str()),
        )?;
        if let Some(row) = get_workspace_path_conn(conn, &sibling_id)? {
            created.push(row);
        }
        effect.touch(&path.project_id);
    }
    Ok(created)
}

// ---------------------------------------------------------------------------
// §8.5 merge / reassignment
// ---------------------------------------------------------------------------

/// §8.5 — "选定 canonical Project", made deterministic.
///
/// The Git family decides first, because `projects.git_id` is UNIQUE: a family
/// has exactly one Project, so the git-backed side is the survivor and the
/// family-less side collapses into it. Only when the identity does not decide is
/// a human fact used: a user-chosen name outranks an automatic one, then the
/// older row, then the id. Never "whichever row the scan reached first" — a
/// retry must merge the same pair the same way round.
fn pick_canonical(a: &Project, b: &Project) -> (Project, Project) {
    let rank = |p: &Project| {
        (
            // higher survives
            p.git_id.is_some(),
            p.name_customized,
            // inverted so "older wins" and "smaller id wins" are both ascending
            std::cmp::Reverse(p.created_at.clone()),
            std::cmp::Reverse(p.id.clone()),
        )
    };
    if rank(a) >= rank(b) {
        (a.clone(), b.clone())
    } else {
        (b.clone(), a.clone())
    }
}

/// §8.4/§8.5 — fold two Projects into one.
///
/// Every WorkspacePath of the loser is reassigned, which batch-refreshes the
/// derived Session cache and the affected Workstream search parents in the same
/// statement (§42.3-M3/M18). The survivor adopts the loser's `git_id` if it has
/// none. A user-chosen name always survives: if the loser had one and the
/// survivor did not, the name and the `name_customized` flag move together
/// (§17-17). The loser is then deleted, because a zero-path Project may not
/// exist (§1.2).
///
/// Two different Git families are never merged: that is not a projection
/// question but a real contradiction in the evidence, and the honest answer is
/// to stop rather than pick a winner and silently drop an identity.
pub fn merge_projects_conn(conn: &Connection, a: &str, b: &str) -> Result<MergeOutcome> {
    let left = get_project_conn(conn, a)?.ok_or_else(|| other(format!("Project {a} 不存在")))?;
    let right = get_project_conn(conn, b)?.ok_or_else(|| other(format!("Project {b} 不存在")))?;
    if let (Some(g1), Some(g2)) = (&left.git_id, &right.git_id) {
        if g1 != g2 {
            return Err(other(format!(
                "两个不同的 Git family ({g1} / {g2}) 不能合并为一个 Project"
            )));
        }
    }
    if left.id == right.id {
        return Ok(MergeOutcome {
            survivor: left.id,
            loser: String::new(),
            moved_paths: Vec::new(),
            adopted_name: None,
        });
    }

    let (survivor, loser) = pick_canonical(&left, &right);
    let mut moved = Vec::new();
    for path in list_workspace_paths_for_project_conn(conn, &loser.id)? {
        reassign_workspace_path_project_conn(conn, &path.id, &survivor.id)?;
        moved.push(path.id);
    }
    // The survivor carries the family: a git-backed loser cannot merge into a
    // family-less survivor under any policy branch today, but the fold is written
    // to be total so a future caller does not have to re-derive the rule.
    if let Some(git_id) = loser.git_id.as_deref() {
        if survivor.git_id.is_none() {
            adopt_git_identity_conn(conn, &survivor.id, git_id)?;
        }
    }
    let mut adopted_name = None;
    if loser.name_customized && !survivor.name_customized {
        set_project_name_conn(conn, &survivor.id, &loser.name, true)?;
        adopted_name = Some(loser.name.clone());
    }
    // §42.3-M18/M3: `reassign_workspace_path_project_conn` already moved each
    // path's derived Session cache AND its Workstream search parents, so there is
    // no second pass to make — one door, one job, no half-updated middle state.
    let deleted = retire_project_if_unowned(conn, &loser.id)?;
    if !deleted {
        return Err(other(format!(
            "合并后 Project {} 仍然拥有路径，零路径 Project 规则被破坏",
            loser.id
        )));
    }
    Ok(MergeOutcome {
        survivor: survivor.id,
        loser: loser.id,
        moved_paths: moved,
        adopted_name,
    })
}

/// The `Db`-scoped merge: same decision, plus the post-commit index work.
pub fn merge_projects(db: &Db, a: &str, b: &str) -> Result<MergeOutcome> {
    let outcome = db.tx(|tx| merge_projects_conn(tx, a, b))?;
    let mut effect = ProjectionEffect::default();
    effect.touch(&outcome.survivor);
    if !outcome.loser.is_empty() {
        effect.delete(&outcome.loser);
    }
    apply_projection_effect(db, &effect);
    Ok(outcome)
}

/// §17-8 — move one path to another Project and refresh everything that reads
/// through it. Exposed because §8.5's "批量刷新引用该 WorkspacePath 的
/// Session.project_id" must not be re-implemented by callers; the Project the path
/// leaves is retired here so a caller cannot forget §1.2.
/// `Ok(Some(project_id))` is the Project this move left with nothing, i.e. the row
/// that was retired in the same transaction and whose FTS entry the caller must
/// drop after commit.
pub fn reassign_workspace_path_conn(
    conn: &Connection,
    path_id: &str,
    project_id: &str,
) -> Result<Option<String>> {
    let former = get_workspace_path_conn(conn, path_id)?
        .map(|p| p.project_id)
        .ok_or_else(|| other(format!("WorkspacePath {path_id} 不存在")))?;
    reassign_workspace_path_project_conn(conn, path_id, project_id)?;
    if former != project_id && retire_project_if_unowned(conn, &former)? {
        // Retiring it in the SAME transaction is what keeps §1.2 true at every
        // committed moment; only its FTS row waits for the commit.
        return Ok(Some(former));
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// §10 GC
// ---------------------------------------------------------------------------

/// §10 — physically remove WorkspacePaths that nothing references any more.
///
/// `gone_path_ids` must only contain paths the CALLER has just observed to be
/// absent (a failed `stat`, or a Git family that no longer lists the worktree).
/// Existence is what makes a row GC-eligible, never a lost `.git` and never the
/// passage of time: a directory that is temporarily unmounted, offline or simply
/// not scanned is a fact worth keeping. Each candidate must additionally pass
/// [`workspace_path_is_gcable`] — 0 Session references AND 0 WorkstreamPath
/// references — so a path a user's Workstream still lists survives its own
/// deletion from disk.
///
/// Deleting the last path of a Project deletes the Project (§1.2), which is why
/// this returns the Project ids it retired: their FTS rows go after the commit.
pub fn gc_gone_workspace_paths_conn(
    conn: &Connection,
    gone_path_ids: &[String],
) -> Result<GcOutcome> {
    let mut outcome = GcOutcome::default();
    let mut affected: Vec<String> = Vec::new();
    for path_id in gone_path_ids {
        if get_workspace_path_conn(conn, path_id)?.is_none() {
            continue; // already gone; a replay must not fail
        }
        if !workspace_path_is_gcable(conn, path_id)? {
            continue;
        }
        let former = delete_workspace_path_conn(conn, path_id)?;
        outcome.deleted_paths.push(path_id.clone());
        affected.push(path_id.clone());
        if let Some(project_id) = former {
            if retire_project_if_unowned(conn, &project_id)? {
                push_unique(&mut outcome.deleted_projects, &project_id);
            }
        }
    }
    // A deleted path took its Workstream's search parentage with it.
    refresh_workstream_search_parents_conn(conn, &affected)?;
    Ok(outcome)
}

/// The `Db`-scoped GC.
pub fn gc_gone_workspace_paths(db: &Db, gone_path_ids: &[String]) -> Result<GcOutcome> {
    let outcome = db.tx(|tx| gc_gone_workspace_paths_conn(tx, gone_path_ids))?;
    for id in &outcome.deleted_projects {
        db.unindex("project", id);
    }
    Ok(outcome)
}

// ---------------------------------------------------------------------------
// §26 / §42.3-M7 reconcile
// ---------------------------------------------------------------------------

impl ProjectProjection<'_> {
    /// §26 — re-observe one registered path and apply §8 to it. The only
    /// reconcile for a single path: it refreshes `exists`, Git detectability, the
    /// family, the Project, the derived Session cache and the affected Workstream
    /// parents, in that order, and it is the ONLY reason a stored path's Project
    /// may change without a user action.
    ///
    /// This consults the resolver, so it must not be called while holding the DB
    /// mutex: `db` has to be a guard the caller can afford to hold across one
    /// `git` call (an explicit "refresh this Project" click), or use
    /// [`reconcile_workspace_paths`] for everything at once.
    ///
    /// An unknown `path_id` is an error, not a silent no-op: reconcile operates on
    /// the registry, and creating a path from here would bypass the door that owns
    /// that decision (§26).
    pub fn reconcile_workspace_path(&self, db: &Db, path_id: &str) -> Result<WorkspacePath> {
        let row = db.get_workspace_path(path_id)?.ok_or_else(|| {
            other(format!(
                "WorkspacePath {path_id} 不在 registry 中：请先通过 ensure_workspace_path 注册"
            ))
        })?;
        let observation = self.observer.observe(&row.canonical_path);
        let outcome = ensure_workspace_path_outcome(db, &observation, self.policy)?;
        Ok(outcome.path)
    }
}

/// §42.3-M7 — the startup / explicit refresh sweep, in the one shape that keeps
/// the lock discipline by construction: read the registry, drop the lock, observe
/// each path, re-take it for one short transaction, repeat.
///
/// * Ordered by `path_id` and capped by `limit`, so a restart cannot reshuffle the
///   pass and move `last_seen_at` across the whole table at once (§42.3-M7).
/// * GC only ever considers paths THIS sweep observed as absent AND that no
///   detected family still lists as a work tree (§10's three conditions), and even
///   then only after the reference check — so a capped sweep can never mistake
///   "not scanned yet" for "gone".
/// * Writes `workspace_paths` / `projects` / the derived Session cache and the FTS
///   parents, nothing else. It never writes `workstream_paths` (§9) and never
///   advances a cursor.
pub fn reconcile_workspace_paths(
    db: &Mutex<Db>,
    projection: &ProjectProjection<'_>,
    limit: usize,
) -> Result<ReconcileReport> {
    let targets = {
        let guard = db.lock().map_err(|_| other("db lock poisoned"))?;
        scan_workspace_paths_conn(guard.conn(), limit)?
    };
    let mut report = ReconcileReport::default();
    let mut gone: Vec<String> = Vec::new();
    // Every path some Git family still listed as one of its work trees this round.
    // §10's third condition is not "we could not stand in it" but "no live Project
    // worktree any more", and `git worktree list` keeps prunable entries listed
    // until `git worktree prune` runs: without this the sweep would delete such a
    // path and re-create it from the same listing on the next pass, forever.
    let mut live_worktrees: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (path_id, canonical) in &targets {
        // No lock is held here — this is where `git` runs.
        let observation = projection.observer().observe(canonical);
        if let GitDetection::Detected { worktrees, .. } = &observation.git {
            live_worktrees.insert(path_id.clone());
            for worktree in worktrees {
                if let Some(sibling) = normalize_path(worktree) {
                    live_worktrees.insert(path_identity(&sibling));
                }
            }
        }
        if !observation.exists {
            gone.push(path_id.clone());
        }
        // One short transaction, and the index work in the same lock step: the
        // commit already happened inside `tx`, so this is post-commit, not
        // in-transaction.
        let committed = {
            let guard = db.lock().map_err(|_| other("db lock poisoned"))?;
            match guard.tx(|tx| ensure_workspace_path_conn(tx, &observation, projection.policy())) {
                Ok(outcome) => {
                    apply_projection_effect(&guard, &outcome.effect);
                    Ok(outcome)
                }
                Err(e) => Err(e.to_string()),
            }
        };
        match committed {
            Ok(outcome) => {
                if outcome.path.id != *path_id {
                    // The stored spelling and the resolver's answer hashed to
                    // different ids, so this sweep registered a second row rather
                    // than refreshing the first. Loud, because it means two
                    // components disagree about one directory (§42.3-M8).
                    eprintln!(
                        "[workspace] reconcile {path_id} 的观察落在 {}，registry 出现了同一目录的第二条路径",
                        outcome.path.id
                    );
                }
                for discovered in &outcome.discovered {
                    push_unique(&mut report.discovered_paths, &discovered.id);
                }
            }
            Err(message) => report.failed.push((path_id.clone(), message)),
        }
        report.scanned += 1;
    }

    let gone: Vec<String> = gone
        .into_iter()
        .filter(|id| !live_worktrees.contains(id))
        .collect();
    let gc = {
        let guard = db.lock().map_err(|_| other("db lock poisoned"))?;
        gc_gone_workspace_paths(&guard, &gone)?
    };
    report.outcome = gc;
    Ok(report)
}

// ---------------------------------------------------------------------------
// §11 read projections
// ---------------------------------------------------------------------------

/// One Workstream as seen from a Project: `is_primary` means it reaches this
/// Project through its position-0 path (§1.12 主关联, otherwise 关联).
#[derive(Debug, Clone, Serialize)]
pub struct ProjectWorkstream {
    pub workstream: Workstream,
    pub is_primary: bool,
}

/// §11/§42.2-E5 — the frozen Project detail shape.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectDetail {
    pub project: Project,
    pub workspace_paths: Vec<WorkspacePath>,
    pub workstreams: Vec<ProjectWorkstream>,
    pub sessions: Vec<Session>,
}

/// §13/§11 — Project detail as one read, derived entirely through the registry:
/// the paths it owns, the Workstreams that reach it through any of them, and the
/// Sessions whose cwd is one of them.
///
/// Sessions come through their `workspace_path_id`, not through the
/// `sessions.project_id` cache, so the detail page can never show a Session the
/// path chain does not support — a legacy row that only carries the cached
/// Project has no physical anchor to show.
pub fn project_detail(db: &Db, project_id: &str) -> Result<Option<ProjectDetail>> {
    let Some(project) = db.get_project(project_id)? else {
        return Ok(None);
    };
    let workspace_paths = db.list_workspace_paths_for_project(project_id)?;
    let workstreams =
        crate::storage::workstream_paths::workstreams_for_project(db.conn(), project_id)?
            .into_iter()
            .map(|(workstream, is_primary)| ProjectWorkstream {
                workstream,
                is_primary,
            })
            .collect();
    let mut sessions = Vec::new();
    for path in &workspace_paths {
        sessions.extend(db.list_sessions_for_workspace_path(&path.id)?);
    }
    Ok(Some(ProjectDetail {
        project,
        workspace_paths,
        workstreams,
        sessions,
    }))
}

/// §11/E4 — the Workstreams of one Project with the 主关联/关联 distinction.
pub fn project_workstreams(db: &Db, project_id: &str) -> Result<Vec<ProjectWorkstream>> {
    Ok(
        crate::storage::workstream_paths::workstreams_for_project(db.conn(), project_id)?
            .into_iter()
            .map(|(workstream, is_primary)| ProjectWorkstream {
                workstream,
                is_primary,
            })
            .collect(),
    )
}

/// Every Project the app currently derives, plus the paths behind it. Used by the
/// reconcile reporting surface and by §25's integrity audit.
pub fn list_projects_with_paths(db: &Db) -> Result<Vec<(Project, Vec<WorkspacePath>)>> {
    let mut out = Vec::new();
    for project in db.list_projects()? {
        out.push((
            project.clone(),
            list_workspace_paths_for_project_conn(db.conn(), &project.id)?,
        ));
    }
    Ok(out)
}

/// §42.5-style mechanical check, available to tests and to the integrity audit:
/// no WorkspacePath is shared, no Project is empty, no path is unparented.
pub fn registry_is_consistent(db: &Db) -> std::result::Result<(), String> {
    let conn = db.conn();
    let check = |sql: &str, label: &str| -> std::result::Result<(), String> {
        let bad: i64 = conn
            .query_row(sql, [], |r| r.get(0))
            .map_err(|e| format!("{label}: {e}"))?;
        if bad != 0 {
            return Err(format!("{label} = {bad}"));
        }
        Ok(())
    };
    check(
        "SELECT COUNT(*) FROM workspace_paths wp LEFT JOIN projects p ON p.id = wp.project_id WHERE p.id IS NULL",
        "workspace path without a project",
    )?;
    check(
        "SELECT COUNT(*) FROM projects p WHERE NOT EXISTS (
            SELECT 1 FROM workspace_paths wp WHERE wp.project_id = p.id)",
        "zero-path project",
    )?;
    // Structural through `idx_projects_git_id` (a partial UNIQUE index), so this
    // can only fire if that index is ever dropped or made conditional — which is
    // precisely the §25 row "non-null git_id unique", and the reason §8.3's
    // worktree convergence works at all.
    check(
        "SELECT COUNT(*) FROM (SELECT git_id FROM projects WHERE git_id IS NOT NULL GROUP BY git_id HAVING COUNT(*) > 1)",
        "git family owned by two projects",
    )?;
    // §1.1 is "every WorkspacePath belongs to exactly one Project". The obvious
    // SQL for that (`GROUP BY id HAVING COUNT(DISTINCT project_id) > 1`) is
    // vacuous and used to sit here: `id` is the PRIMARY KEY, so no group can
    // ever hold two values. What is *not* structural is that the key is the one
    // this path derives to. A row carrying an id its own `canonical_path` does
    // not hash to is a second identity for one directory, and every join in the
    // app would keep agreeing with it, because joins go through `id`.
    // `workspace::identity` is the only place a path key may be computed, so
    // this is the only place that can prove nobody computed it elsewhere.
    let rows: std::result::Result<Vec<(String, String)>, String> = conn
        .prepare("SELECT id, canonical_path FROM workspace_paths ORDER BY id")
        .map_err(|e| format!("workspace path id check: {e}"))
        .and_then(|mut st| {
            st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(|e| format!("workspace path id check: {e}"))
                .and_then(|rows| {
                    rows.collect::<std::result::Result<Vec<_>, _>>()
                        .map_err(|e| format!("workspace path id check: {e}"))
                })
        });
    let mut lying_id: Vec<String> = rows?
        .into_iter()
        .filter(|(id, canonical)| path_identity(canonical) != *id)
        .map(|(id, canonical)| format!("{id} <- {canonical}"))
        .collect();
    if !lying_id.is_empty() {
        lying_id.truncate(5);
        return Err(format!(
            "workspace path id not derived from its canonical_path: {lying_id:?}"
        ));
    }
    check(
        "SELECT COUNT(*) FROM sessions s JOIN workspace_paths wp ON wp.id = s.workspace_path_id WHERE s.project_id IS NOT wp.project_id",
        "session project cache out of sync with its path",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_git_common_dir_cannot_split_a_family() {
        // `git rev-parse --git-common-dir` answers `.git` for a main worktree. Keyed
        // literally, every worktree of one repository would look like its own
        // family and §8.3's convergence would never happen.
        assert_eq!(
            resolve_common_dir("/repo", ".git"),
            normalize_path("/repo/.git").unwrap()
        );
        assert_eq!(
            resolve_common_dir("/repo/wt/x", ".git"),
            normalize_path("/repo/wt/x/.git").unwrap(),
            "resolved against the path we asked about, not the process cwd"
        );
        // An absolute answer is only normalized, never rewritten.
        assert_eq!(
            resolve_common_dir("/repo", "/repo/.git/worktrees/y"),
            normalize_path("/repo/.git/worktrees/y").unwrap()
        );
        assert_eq!(resolve_common_dir("/repo", "   "), "");
    }

    #[test]
    fn git_state_transition_remembers_evidence_even_after_it_is_gone() {
        let no_family: Option<GitFamily> = None;
        let family = Some(GitFamily {
            git_id: "g1".into(),
            kind: "linked".into(),
            worktrees: vec![],
        });

        // First sight, no evidence: `none`. Nothing in our history would make
        // `missing` an honest sentence.
        assert_eq!(
            next_git_observation(None, &no_family),
            (git_state::NONE.to_string(), None)
        );
        assert_eq!(
            next_git_observation(Some(git_state::NONE), &no_family),
            (git_state::NONE.to_string(), None)
        );
        // §8.1: evidence, then silence, is `missing` — and it stays missing, because
        // `Unavailable` (no git binary, timeout, dubious ownership) is not a
        // reclassification of a repository into a plain directory.
        assert_eq!(
            next_git_observation(Some(git_state::DETECTED), &no_family),
            (git_state::MISSING.to_string(), None)
        );
        assert_eq!(
            next_git_observation(Some(git_state::MISSING), &no_family),
            (git_state::MISSING.to_string(), None)
        );
        // Evidence returns: the kind comes from the evidence, never from the row.
        assert_eq!(
            next_git_observation(Some(git_state::MISSING), &family),
            (git_state::DETECTED.to_string(), Some("linked".to_string()))
        );
        assert_eq!(
            next_git_observation(Some(git_state::DETECTED), &family),
            (git_state::DETECTED.to_string(), Some("linked".to_string()))
        );
    }

    #[test]
    fn canonical_pick_is_a_total_order_so_a_replay_merges_the_same_way_round() {
        let project = |id: &str, git: Option<&str>, customized: bool, created: &str| Project {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            archived: false,
            git_id: git.map(Into::into),
            name_customized: customized,
            created_at: created.into(),
            updated_at: created.into(),
        };
        let plain = project("a", None, false, "2026-01-01T00:00:00+00:00");
        let backed = project("b", Some("g"), false, "2026-02-01T00:00:00+00:00");
        let named = project("c", None, true, "2026-03-01T00:00:00+00:00");

        // The Git family outranks a newer or user-named row: it is the only fact
        // that cannot survive being dropped.
        assert_eq!(pick_canonical(&named, &backed).0.id, "b");
        assert_eq!(pick_canonical(&backed, &named).0.id, "b");
        // Then a person's choice, then age, then the id — and it is symmetric.
        assert_eq!(pick_canonical(&plain, &named).0.id, "c");
        assert_eq!(pick_canonical(&named, &plain).0.id, "c");
        assert_eq!(pick_canonical(&plain, &backed).0.id, "b");
        let older = project("z", None, false, "2025-01-01T00:00:00+00:00");
        assert_eq!(pick_canonical(&plain, &older).0.id, "z");
        let same_age = project("y", None, false, "2026-01-01T00:00:00+00:00");
        assert_eq!(pick_canonical(&plain, &same_age).0.id, "a");
        assert_eq!(pick_canonical(&same_age, &plain).0.id, "a");
    }
}
