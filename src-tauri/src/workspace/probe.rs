//! Read-only path intelligence for the "pick a working directory" UI.
//!
//! Two questions the user asks *before* committing a path, answered without
//! writing anything:
//!
//! * [`probe_workspace_path`] — what would `ensure_path` decide about this
//!   string, and which Project would it land in? The acceptance answer here is
//!   advisory only: the attacher's `Ok(None)` inside the caller's transaction
//!   stays the only authority (§8). Everything the probe reports is derived
//!   from the same observation the attacher would run, so the two cannot
//!   disagree about facts — only about timing.
//! * [`list_recent_workspace_paths`] — the recent/known directories a picker
//!   offers, unioned from the WorkspacePath registry and the Session cwd
//!   history. Ranking is pure reads; nothing here creates a WorkspacePath.
//!
//! Both run on the `Db` read half and never observe inside a write guard
//! (§42.3-M7: a `git` call never holds the DB lock).

use serde::Serialize;

use crate::domain::{git_state, GitDetection};
use crate::error::Result;
use crate::storage::workspace::{
    find_git_identity_by_common_dir_conn, get_project_conn, get_workspace_path_conn,
    project_by_git_id_conn,
};
use crate::storage::Db;
use crate::workspace::identity::auto_project_name;
use crate::workspace::project::{resolve_common_dir, WorkspacePolicy};
use crate::workspace::resolver::{
    exists_on_disk, is_observable, ProbeRejection, WorkspaceObserving,
};

/// [`PathProbe::status`] — why the string is not a WorkspacePath right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeStatus {
    /// Can be attached as a working path.
    Ok,
    /// Empty, un-normalizable, or relative with no base.
    Unresolvable,
    /// A reserved NoEnding Home path (§2).
    Reserved,
    /// The user Home itself (§1.4).
    Home,
}

/// The Project a path would land in, predicted read-only. `known = false`
/// means the commit would CREATE it with the reported (automatic) name.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectHint {
    pub id: Option<String>,
    pub name: Option<String>,
    pub known: bool,
}

/// One path as the picker's feedback line renders it. Every field is an
/// observation or a read — a probe must never turn the registry's rows into
/// promises the attacher has not made.
#[derive(Debug, Clone, Serialize)]
pub struct PathProbe {
    pub raw: String,
    pub status: ProbeStatus,
    pub canonical_path: Option<String>,
    pub exists: bool,
    /// `detected | none | unavailable` — the resolver's fresh evidence, not the
    /// stored row (which a reconcile may not have refreshed yet).
    pub git_state: Option<String>,
    pub git_kind: Option<String>,
    pub project: Option<ProjectHint>,
}

/// What would happen if this raw string were attached as a working path?
///
/// Pure reads: the observation runs exactly like the attacher's, then the
/// registry is consulted for the Project prediction — a known path keeps its
/// Project, a fresh Git family joins the Project that owns the family, and
/// anything else would start a new one with the automatic name.
pub fn probe_workspace_path(
    db: &Db,
    observing: &dyn WorkspaceObserving,
    policy: &dyn WorkspacePolicy,
    raw: &str,
) -> Result<PathProbe> {
    let trimmed = raw.trim();
    let mut probe = PathProbe {
        raw: raw.to_string(),
        status: ProbeStatus::Unresolvable,
        canonical_path: None,
        exists: false,
        git_state: None,
        git_kind: None,
        project: None,
    };
    if trimmed.is_empty() {
        return Ok(probe);
    }

    let observation = observing.observe(trimmed);
    if !is_observable(&observation) {
        probe.status = match observing.probe_rejection(trimmed) {
            ProbeRejection::Reserved => ProbeStatus::Reserved,
            ProbeRejection::Home => ProbeStatus::Home,
            ProbeRejection::Unresolvable => ProbeStatus::Unresolvable,
        };
        return Ok(probe);
    }

    probe.status = ProbeStatus::Ok;
    probe.canonical_path = Some(observation.canonical_path.clone());
    probe.exists = observation.exists;
    let (state, kind) = match &observation.git {
        GitDetection::Detected { kind, .. } => (
            git_state::DETECTED.to_string(),
            Some(kind.as_str().to_string()),
        ),
        // "Could not ask" is its own line in the UI — never quietly "none".
        GitDetection::Unavailable => ("unavailable".to_string(), None),
        GitDetection::None | GitDetection::Missing => (git_state::NONE.to_string(), None),
    };
    probe.git_state = Some(state);
    probe.git_kind = kind;

    let conn = db.read();
    probe.project = Some(
        match get_workspace_path_conn(&conn, &observation.path_id)? {
            Some(row) => ProjectHint {
                id: Some(row.project_id.clone()),
                name: get_project_conn(&conn, &row.project_id)?.map(|p| p.name),
                known: true,
            },
            None => {
                let family_project = match &observation.git {
                    GitDetection::Detected { common_dir, .. } => {
                        let common = resolve_common_dir(&observation.canonical_path, common_dir);
                        if common.is_empty() {
                            None
                        } else {
                            find_git_identity_by_common_dir_conn(&conn, &common)?
                                .map(|identity| identity.id)
                                .and_then(|git_id| {
                                    project_by_git_id_conn(&conn, &git_id).ok().flatten()
                                })
                        }
                    }
                    _ => None,
                };
                match family_project {
                    // An identity without a Project cannot happen through §8 (the
                    // Project is created with the family); if the data says
                    // otherwise, "would create one" stays the honest prediction.
                    Some(project) => ProjectHint {
                        id: Some(project.id),
                        name: Some(project.name),
                        known: true,
                    },
                    None => ProjectHint {
                        id: None,
                        name: Some(auto_project_name(
                            &observation.canonical_path,
                            policy.default_workspace().as_deref(),
                        )),
                        known: false,
                    },
                }
            }
        },
    );
    Ok(probe)
}

/// One entry of the picker's "recent / known directories" list. `known = false`
/// entries come straight from Session cwd history and are NOT WorkspacePaths
/// yet — attaching one is still the user's explicit act (§1.7).
#[derive(Debug, Clone, Serialize)]
pub struct RecentWorkspacePath {
    pub path: String,
    pub known: bool,
    pub exists: bool,
    pub project_name: Option<String>,
    pub git_state: Option<String>,
    pub git_kind: Option<String>,
    pub last_used_at: Option<String>,
}

/// The picker's candidate list: known WorkspacePaths plus Session cwds the
/// registry has never seen, ranked by the most recent session activity under
/// each directory. Trashed Sessions still count as usage — they were real work
/// in a real directory, and the trash says so about the Session, not the path.
///
/// Unknown cwds are normalized and checked against §2/§1.4, but never probed
/// with git: a picker listing must stay cheap, and the per-entry probe run
/// answers that properly once the user focuses a candidate.
pub fn list_recent_workspace_paths(
    db: &Db,
    policy: &dyn WorkspacePolicy,
    limit: usize,
) -> Result<Vec<RecentWorkspacePath>> {
    let conn = db.read();

    let mut known_ids = std::collections::HashSet::new();
    let mut out: Vec<RecentWorkspacePath> = Vec::new();
    {
        let mut st = conn.prepare(
            "SELECT wp.canonical_path, wp.id, wp.exists_on_disk, wp.git_state, wp.git_kind,
                    p.name,
                    (SELECT MAX(s.last_activity_at) FROM sessions s
                      WHERE s.workspace_path_id = wp.id) AS activity
             FROM workspace_paths wp
             LEFT JOIN projects p ON p.id = wp.project_id",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)? != 0,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?;
        for row in rows {
            let (path, id, exists, git_state, git_kind, project_name, activity) = row?;
            known_ids.insert(id);
            out.push(RecentWorkspacePath {
                path,
                known: true,
                exists,
                project_name,
                git_state,
                git_kind,
                last_used_at: activity,
            });
        }
    }

    // Session cwds the registry has never seen. Grouping in SQL collapses the
    // many-sessions-one-directory case into one entry with its latest activity.
    {
        let mut st = conn.prepare(
            "SELECT cwd, MAX(last_activity_at) FROM sessions
             WHERE cwd IS NOT NULL AND TRIM(cwd) <> ''
             GROUP BY cwd",
        )?;
        let rows = st.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })?;
        for row in rows {
            let (cwd, activity) = row?;
            let Some(canonical) = crate::workspace::normalize_path(cwd.trim()) else {
                continue;
            };
            let id = crate::workspace::path_identity(&canonical);
            if known_ids.contains(&id) {
                continue;
            }
            // A session cwd should never be a reserved path or the Home, but
            // the picker must not be the thing that finds out otherwise.
            if policy.is_reserved(&canonical) {
                continue;
            }
            // Dedupe distinct raw spellings of one directory against entries
            // this loop already produced.
            if out
                .iter()
                .any(|e| !e.known && crate::workspace::identity::same_location(&e.path, &canonical))
            {
                continue;
            }
            known_ids.insert(id);
            let exists = exists_on_disk(&canonical);
            out.push(RecentWorkspacePath {
                path: canonical,
                known: false,
                exists,
                project_name: None,
                git_state: None,
                git_kind: None,
                last_used_at: activity,
            });
        }
    }

    // RFC3339 UTC timestamps compare correctly as strings (storage::now's one
    // format); `None` ranks last so a directory with no activity still shows.
    out.sort_by(|a, b| b.last_used_at.cmp(&a.last_used_at));
    out.truncate(limit);
    Ok(out)
}
