//! Project summary and its independently updated session inputs.
use super::*;
use crate::storage::{project_context_repo as repo, workspace::get_project_conn};
use rusqlite::Connection;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize)]
pub struct ProjectContextView {
    pub project_id: String,
    pub summary: Option<crate::domain::Summary>,
    pub context_revision: i64,
    pub updated_at: Option<String>,
    pub pending: bool,
    pub pending_sessions: usize,
    pub pending_session_summaries: usize,
    pub freshness: super::freshness::SummaryFreshness,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectUpdateOutcome {
    pub project_id: String,
    pub status: ContextUpdateStatus,
    pub context_revision: i64,
    pub updated_sessions: Vec<String>,
    pub skipped_session_ids: Vec<String>,
    pub failed_sessions: Vec<SessionFailure>,
    pub remaining_pending: usize,
    pub remaining_session_summaries: usize,
}

pub(super) struct ProjectInput {
    project: crate::domain::Project,
    paths: Vec<String>,
    tasks: Vec<serde_json::Value>,
    pub(super) session_ids: Vec<String>,
    pub(super) fingerprint: String,
}
impl ProjectInput {
    pub(super) fn header(&self, home: &crate::workspace::home::NoEndingHome) -> serde_json::Value {
        let policy = crate::workspace::wiring::HomePolicy::new(home);
        let kind = crate::workspace::project::project_kind(&self.project, &self.paths, &policy);
        serde_json::json!({"type":"project","name":self.project.name,"kind":kind,"working_directories":self.paths,"related_tasks":self.tasks})
    }
    pub(super) fn read(conn: &Connection, id: &str) -> Result<Self> {
        let project = get_project_conn(conn, id)?.ok_or_else(|| other("项目不存在"))?;
        let mut st=conn.prepare("SELECT canonical_path FROM workspace_paths WHERE project_id=?1 ORDER BY canonical_path")?;
        let paths = st
            .query_map([id], |r| r.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        let mut tasks = crate::storage::workstream_projects::workstreams_for_project(conn, id)?;
        tasks.sort_by(|a, b| a.id.cmp(&b.id));
        let tasks: Vec<_> = tasks
            .into_iter()
            .map(
                |w| serde_json::json!({"task_id":w.id,"title":w.title,"description":w.description}),
            )
            .collect();
        let mut st=conn.prepare("SELECT s.id FROM sessions s JOIN workspace_paths wp ON wp.id=s.workspace_path_id WHERE wp.project_id=?1 ORDER BY s.id")?;
        let session_ids = st
            .query_map([id], |r| r.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        // Store a small change token, not duplicate names, directories or task bodies.
        // Membership is included so even deleting a session (cascaded frontier) marks pending.
        let bytes = serde_json::to_vec(
            &serde_json::json!({"name":project.name,"git_id":project.git_id,"paths":paths,"tasks":tasks,"sessions":session_ids}),
        )?;
        let fingerprint = format!("{:x}", Sha256::digest(bytes));
        Ok(Self {
            project,
            paths,
            tasks,
            session_ids,
            fingerprint,
        })
    }
}
pub(super) fn input_changed(conn: &Connection, id: &str) -> Result<bool> {
    Ok(ProjectInput::read(conn, id)?.fingerprint != repo::state(conn, id)?.input_fingerprint)
}

pub fn project_context_view(db: &Db, id: &str) -> Result<ProjectContextView> {
    let (input, state, frontiers) = {
        let conn = db.read();
        (
            ProjectInput::read(&conn, id)?,
            repo::state(&conn, id)?,
            repo::frontiers(&conn, id)?,
        )
    };
    let mut pending_sessions = 0;
    let mut pending_session_summaries = 0;
    for sid in &input.session_ids {
        let view = session_context_view(db, sid)?;
        pending_session_summaries += usize::from(view.pending);
        if view.pending || frontiers.get(sid).is_none_or(|rev| *rev != view.revision) {
            pending_sessions += 1;
        }
    }
    Ok(ProjectContextView {
        project_id: id.into(),
        summary: (!state.summary.is_empty()).then_some(state.summary),
        context_revision: state.revision,
        updated_at: state.updated_at,
        pending: input.fingerprint != state.input_fingerprint || pending_sessions > 0,
        pending_sessions,
        pending_session_summaries,
        freshness: super::freshness::summary(&db.read(), "project", id)?,
    })
}
pub fn update_project(
    db: &Db,
    id: &str,
    home: Option<&crate::workspace::home::NoEndingHome>,
) -> Result<ProjectUpdateOutcome> {
    extract_project(db, id, home, false, &mut |cli, prompt, dir| {
        cli.run_context(prompt, dir)
    })
}
pub fn regenerate_project(
    db: &Db,
    id: &str,
    home: Option<&crate::workspace::home::NoEndingHome>,
) -> Result<ProjectUpdateOutcome> {
    extract_project(db, id, home, true, &mut |cli, prompt, dir| {
        cli.run_context(prompt, dir)
    })
}
pub(super) fn extract_project(
    db: &Db,
    id: &str,
    home: Option<&crate::workspace::home::NoEndingHome>,
    regenerate: bool,
    runner: &mut ContextRunner<'_>,
) -> Result<ProjectUpdateOutcome> {
    extract(db, id, home, "project", regenerate, runner)
}

pub(super) fn needs_update(
    db: &Db,
    id: &str,
    fingerprint: &str,
    sessions: &[SessionSnapshot],
) -> Result<bool> {
    let conn = db.read();
    let state = repo::state(&conn, id)?;
    let frontiers = repo::frontiers(&conn, id)?;
    Ok(fingerprint != state.input_fingerprint
        || sessions
            .iter()
            .any(|s| s.pending() || frontiers.get(&s.session.id) != Some(&s.revision())))
}

pub(super) fn commit(
    db: &Db,
    id: &str,
    fingerprint: &str,
    revision: i64,
    sessions: &[SessionSnapshot],
    summary: &crate::domain::Summary,
) -> Result<i64> {
    db.tx(|tx| {
        if repo::state(tx, id)?.revision != revision
            || ProjectInput::read(tx, id)?.fingerprint != fingerprint
        {
            return Err(stale());
        }
        let mut consumed = Vec::with_capacity(sessions.len());
        for session in sessions {
            session.validate(tx)?;
            consumed.push((session.session.id.clone(), session.revision()));
        }
        repo::commit(tx, id, fingerprint, summary, &consumed)
    })
}
