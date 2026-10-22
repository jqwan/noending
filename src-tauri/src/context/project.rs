//! Project extraction uses the same session/file protocol as task extraction.
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
    pub freshness: super::freshness::SummaryFreshness,
}
#[derive(Debug, Clone, Serialize)]
pub struct ProjectUpdateOutcome {
    pub project_id: String,
    pub status: ContextUpdateStatus,
    pub context_revision: i64,
    pub updated_sessions: Vec<String>,
    pub skipped_session_ids: Vec<String>,
    pub failed_sessions: Vec<SessionFailure>,
    pub remaining_pending: usize,
}

struct ProjectInput {
    project: crate::domain::Project,
    paths: Vec<String>,
    tasks: Vec<serde_json::Value>,
    session_ids: Vec<String>,
    fingerprint: String,
}
impl ProjectInput {
    fn read(conn: &Connection, id: &str) -> Result<Self> {
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
    for sid in &input.session_ids {
        let view = session_context_view(db, sid)?;
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
    with_operation(db, id, home, "project", |launch, operation| {
        let (input, state) = {
            let conn = db.read();
            (
                ProjectInput::read(&conn, id)?,
                query::project_state(&conn, id)?,
            )
        };
        let snapshots = input
            .session_ids
            .iter()
            .map(|sid| SessionSnapshot::read(db, db.get_session(sid)?.ok_or_else(stale)?, false))
            .collect::<Result<Vec<_>>>()?;
        let frontiers = repo::frontiers(&db.read(), id)?;
        let pending = input.fingerprint != state.input_fingerprint
            || snapshots.iter().any(|s| {
                s.pending()
                    || frontiers
                        .get(&s.session.id)
                        .is_none_or(|r| *r != s.revision())
            });
        if !regenerate && !pending {
            return Ok((
                ProjectUpdateOutcome {
                    project_id: id.into(),
                    status: ContextUpdateStatus::NoChange,
                    context_revision: state.revision,
                    updated_sessions: vec![],
                    skipped_session_ids: vec![],
                    failed_sessions: vec![],
                    remaining_pending: 0,
                },
                "no_change",
            ));
        }
        let policy = crate::workspace::wiring::HomePolicy::new(home.unwrap());
        let kind = crate::workspace::project::project_kind(&input.project, &input.paths, &policy);
        let batch = run_batch(
            db,
            id,
            "project",
            serde_json::json!({"type":"project","name":input.project.name,"kind":kind,"working_directories":input.paths,"related_tasks":input.tasks}),
            state.revision,
            &snapshots,
            regenerate,
            launch,
            operation,
            runner,
        )?;
        operation.set_stage("database_commit");
        let (revision, updated) = db.tx(|tx| {
            if get_project_conn(tx, id)?.is_none()
                || repo::state(tx, id)?.revision != state.revision
                || ProjectInput::read(tx, id)?.fingerprint != input.fingerprint
            {
                return Err(stale());
            }
            // Physical project scope is validated above; task ownership is independent.
            for s in &snapshots {
                s.validate(tx, None)?;
            }
            let mut updated = vec![];
            let mut consumed = vec![];
            for s in &snapshots {
                let Some(fields) = batch.results.get(&s.session.id) else {
                    continue;
                };
                let revision = if s.pending() {
                    updated.push(s.session.id.clone());
                    commit_session_context_conn(
                        tx,
                        &s.session.id,
                        fields,
                        s.revision(),
                        s.generation,
                        s.latest,
                    )
                    .and_then(|revision| {
                        crate::storage::freshness::mark_refreshed(tx, &s.session.id)?;
                        Ok(revision)
                    })?
                } else {
                    s.revision()
                };
                consumed.push((s.session.id.clone(), revision));
            }
            let revision = repo::commit(tx, id, &input.fingerprint, &batch.summary, &consumed)?;
            Ok((revision, updated))
        })?;
        let status = if !batch.failed.is_empty() {
            ContextUpdateStatus::Partial
        } else if batch.target_skipped {
            ContextUpdateStatus::Skipped
        } else {
            ContextUpdateStatus::Updated
        };
        Ok((
            ProjectUpdateOutcome {
                project_id: id.into(),
                status,
                context_revision: revision,
                updated_sessions: updated,
                skipped_session_ids: batch.skipped,
                failed_sessions: batch.failed,
                remaining_pending: project_context_view(db, id)?.pending_sessions,
            },
            match status {
                ContextUpdateStatus::Partial => "partial",
                ContextUpdateStatus::Skipped => "skipped",
                _ => "updated",
            },
        ))
    })
}
