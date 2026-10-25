//! Summary reads and extraction, delegated to an Agent or performed by the
//! current session. Both paths validate before a snapshot-guarded transaction.
pub(crate) mod diagnostics;
pub mod freshness;
pub(crate) mod interactive;
pub mod items;
mod project;
pub mod prompts;
mod query;
pub mod summary;
pub mod tools;
use crate::domain::{ContextUpdateError, ContextUpdateStatus, Session, SessionContextFields};
use crate::error::{other, AppError, Result};
use crate::storage::context_repo::{
    self, commit_session_context_conn, commit_workstream_summary_conn,
    get_workstream_context_state_conn, set_workstream_frontier_conn,
};
use crate::storage::Db;
use crate::sync::extractor::{self, CliExtractor, ExtractionSettings};
pub use project::{
    project_context_view, regenerate_project, update_project, ProjectContextView,
    ProjectUpdateOutcome,
};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use summary::{BatchReply, ExtractionFiles, SessionFailure, SessionReply};

#[derive(Debug, Clone, Serialize)]
pub struct SessionContextView {
    pub session_id: String,
    pub fields: Option<SessionContextFields>,
    pub revision: i64,
    pub ingest_generation: i64,
    pub processed_through_seq: i64,
    pub latest_message_seq: i64,
    pub updated_at: Option<String>,
    pub pending: bool,
    pub freshness: freshness::SummaryFreshness,
}
#[derive(Debug, Clone, Serialize)]
pub struct WorkstreamContextView {
    pub workstream_id: String,
    pub summary: Option<crate::domain::Summary>,
    pub context_revision: i64,
    pub updated_at: Option<String>,
    pub pending: bool,
    pub freshness: freshness::SummaryFreshness,
    pub pending_sessions: usize,
}
#[derive(Debug, Clone, Serialize)]
pub struct SessionUpdateOutcome {
    pub session_id: String,
    pub status: ContextUpdateStatus,
    pub revision: i64,
}
#[derive(Debug, Clone, Serialize)]
pub struct WorkstreamUpdateOutcome {
    pub workstream_id: String,
    pub status: ContextUpdateStatus,
    pub context_revision: i64,
    pub updated_sessions: Vec<String>,
    pub skipped_session_ids: Vec<String>,
    pub failed_sessions: Vec<SessionFailure>,
    pub remaining_pending: usize,
}

#[derive(Clone, Serialize, serde::Deserialize)]
struct SessionSnapshot {
    session: Session,
    revision: i64,
    summary_generation: Option<i64>,
    generation: i64,
    latest: i64,
    from: i64,
    summary_through_at: Option<String>,
    last_message_at: Option<String>,
}
impl SessionSnapshot {
    fn read(db: &Db, session: Session, regenerate: bool) -> Result<Self> {
        let conn = db.read();
        let tx = conn.unchecked_transaction()?;
        let existing: Option<(i64, i64, i64)> = tx.query_row(
            "SELECT revision,ingest_generation,processed_through_seq FROM session_contexts WHERE session_id=?1",
            [&session.id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
        ).optional()?;
        let (generation, latest) =
            crate::storage::context_repo::get_ingest_state_conn(&tx, &session.id)?;
        let from = existing
            .filter(|c| !regenerate && c.1 == generation)
            .map_or(0, |c| c.2);
        let summary_through_at = query::message_time(&tx, &session.id, from)?;
        let last_message_at = query::message_time(&tx, &session.id, latest)?;
        tx.commit()?;
        Ok(Self {
            session,
            revision: existing.map_or(0, |c| c.0),
            summary_generation: existing.map(|c| c.1),
            generation,
            latest,
            from,
            summary_through_at,
            last_message_at,
        })
    }
    fn last_message_at(&self) -> Option<&str> {
        self.last_message_at.as_deref()
    }
    fn revision(&self) -> i64 {
        self.revision
    }
    fn pending(&self) -> bool {
        self.latest > self.from
            || self
                .summary_generation
                .is_some_and(|g| g != self.generation)
    }
    fn retained(&self, conn: &rusqlite::Connection) -> Result<SessionContextFields> {
        Ok(SessionContextFields {
            summary: query::target_summary(conn, "session", &self.session.id, self.revision())?
                .unwrap_or_default(),
        })
    }
    fn validate(&self, conn: &rusqlite::Connection, tracked_task: Option<&str>) -> Result<()> {
        let identity: Option<(i64,i64,Option<String>,i64)>=conn.query_row(
            "SELECT fact_generation,COALESCE((SELECT revision FROM session_contexts WHERE session_id=sessions.id),0),(SELECT project_id FROM workspace_paths WHERE id=sessions.workspace_path_id),latest_message_seq FROM sessions WHERE id=?1",
            [&self.session.id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        if identity.as_ref().is_none_or(|i| {
            i.0 != self.generation
                || i.1 != self.revision()
                || i.2 != self.session.project_id
                || i.3 < self.latest
        }) {
            return Err(stale());
        }
        if let Some(task) = tracked_task {
            let linked:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM session_workstreams WHERE session_id=?1 AND workstream_id=?2)",rusqlite::params![self.session.id,task],|r|r.get(0))?;
            if !linked {
                return Err(stale());
            }
        }
        Ok(())
    }
}

use rusqlite::OptionalExtension;
fn stale() -> AppError {
    AppError::context(ContextUpdateError::ConcurrencyConflict(
        "内容已变化，请重新更新".into(),
    ))
}
fn invalid(error: impl std::fmt::Display) -> AppError {
    AppError::context(ContextUpdateError::InvalidOutput(error.to_string()))
}
fn read_summary_result(
    db: &Db,
    files: &ExtractionFiles,
    path: &std::path::Path,
    kind: &str,
    id: &str,
    revision: i64,
) -> Result<SessionContextFields> {
    let mut fields = files.read_result(path).map_err(invalid)?;
    let previous = query::target_summary(&db.read(), kind, id, revision)?.unwrap_or_default();
    fields.summary = fields
        .summary
        .resolve_extracted_ids(&previous)
        .map_err(invalid)?;
    Ok(fields)
}

pub fn session_context_view(db: &Db, id: &str) -> Result<SessionContextView> {
    let ctx = db.get_session_context(id)?;
    let (generation, latest) = db.get_session_ingest_state(id)?;
    let from = ctx
        .as_ref()
        .filter(|c| c.ingest_generation == generation)
        .map(|c| c.processed_through_seq)
        .unwrap_or(0);
    Ok(SessionContextView {
        session_id: id.into(),
        fields: ctx
            .as_ref()
            .filter(|c| !c.fields.summary.is_empty())
            .map(|c| c.fields.clone()),
        revision: ctx.as_ref().map(|c| c.revision).unwrap_or(0),
        ingest_generation: generation,
        processed_through_seq: from,
        latest_message_seq: latest,
        updated_at: ctx.as_ref().map(|c| c.updated_at.clone()),
        freshness: freshness::summary(&db.read(), "session", id)?,
        pending: latest > from
            || ctx
                .as_ref()
                .is_some_and(|c| c.ingest_generation != generation),
    })
}
pub fn workstream_context_view(db: &Db, id: &str) -> Result<WorkstreamContextView> {
    db.get_workstream(id)?.ok_or_else(|| other("任务不存在"))?;
    let state = db.get_workstream_context_state(id)?;
    let frontiers: HashMap<_, _> = db
        .workstream_frontiers(id)?
        .into_iter()
        .map(|f| (f.session_id.clone(), f))
        .collect();
    let sessions = db.sessions_for_workstream(id)?;
    let mut pending_sessions = 0;
    for session in &sessions {
        let view = session_context_view(db, &session.id)?;
        let consumed = frontiers.get(&session.id);
        if view.pending || consumed.is_none_or(|f| f.session_context_revision != view.revision) {
            pending_sessions += 1;
        }
    }
    let membership_changed = frontiers.len() != sessions.len();
    Ok(WorkstreamContextView {
        workstream_id: id.into(),
        summary: (!state.summary.is_empty()).then_some(state.summary),
        context_revision: state.context_revision,
        updated_at: state.updated_at,
        pending: state.input_revision != state.consumed_input_revision
            || membership_changed
            || pending_sessions > 0,
        pending_sessions,
        freshness: freshness::summary(&db.read(), "task", id)?,
    })
}

type ContextRunner<'a> = dyn FnMut(
        Option<&CliExtractor>,
        &str,
        &std::path::Path,
    ) -> std::result::Result<String, extractor::ContextCallFailure>
    + 'a;

pub fn update_session(
    db: &Db,
    id: &str,
    home: Option<&crate::workspace::home::NoEndingHome>,
) -> Result<SessionUpdateOutcome> {
    extract_session(db, id, home, false, &mut |cli, prompt, dir| {
        cli.unwrap().run_context(prompt, dir)
    })
}
pub fn regenerate_session(
    db: &Db,
    id: &str,
    home: Option<&crate::workspace::home::NoEndingHome>,
) -> Result<SessionUpdateOutcome> {
    extract_session(db, id, home, true, &mut |cli, prompt, dir| {
        cli.unwrap().run_context(prompt, dir)
    })
}
pub fn update_workstream(
    db: &Db,
    id: &str,
    home: Option<&crate::workspace::home::NoEndingHome>,
) -> Result<WorkstreamUpdateOutcome> {
    extract_task(db, id, home, false, &mut |cli, prompt, dir| {
        cli.unwrap().run_context(prompt, dir)
    })
}
pub fn regenerate_workstream(
    db: &Db,
    id: &str,
    home: Option<&crate::workspace::home::NoEndingHome>,
) -> Result<WorkstreamUpdateOutcome> {
    extract_task(db, id, home, true, &mut |cli, prompt, dir| {
        cli.unwrap().run_context(prompt, dir)
    })
}
fn with_operation<T>(
    db: &Db,
    id: &str,
    home: Option<&crate::workspace::home::NoEndingHome>,
    kind: &'static str,
    scope: Option<&interactive::Scope>,
    f: impl FnOnce(
        &ContextLaunchConfig,
        &mut diagnostics::ContextOperation,
        &mut crate::operations::Operation,
    ) -> Result<(T, &'static str)>,
) -> Result<T> {
    let mut activity = crate::operations::Operation::start(
        db,
        crate::operations::OperationStatus::summary(
            if kind == "workstream" { "task" } else { kind },
            id,
        ),
    )?;
    let mut operation = diagnostics::ContextOperation::new(kind, id, home);
    let launch = ContextLaunchConfig::from_db(db, scope)
        .map_err(|error| AppError::ContextUpdateFailed(operation.fail(&error)))?;
    operation.record_config(&launch.raw_agent, launch.agent, launch.opts.as_ref());
    if home.is_none() {
        operation.fail_with(
            "home_resolution",
            "home_unavailable",
            "应用工作目录未初始化，无法运行摘要提取。",
        );
    }
    let result = if home.is_none() {
        Err(other("context home unavailable"))
    } else {
        f(&launch, &mut operation, &mut activity)
    };
    match result {
        Ok((outcome, status)) => {
            operation.succeed(status);
            Ok(outcome)
        }
        Err(error) => Err(AppError::ContextUpdateFailed(operation.fail(&error))),
    }
}
fn files_for(
    operation: &mut diagnostics::ContextOperation,
    target: &str,
    sessions: &[String],
) -> Result<ExtractionFiles> {
    operation.set_stage("input_preparation");
    let root = diagnostics::prepare_runtime_dir(operation.home().unwrap())?;
    ExtractionFiles::create(&root, target, sessions).map_err(|error| {
        if matches!(&error,AppError::Io(io) if io.kind()==std::io::ErrorKind::WouldBlock) {
            operation.fail_with(
                "input_preparation",
                "summary_busy",
                "相关摘要正在提取，请稍后重试。",
            );
        }
        error
    })
}
fn extract_session(
    db: &Db,
    id: &str,
    home: Option<&crate::workspace::home::NoEndingHome>,
    regenerate: bool,
    runner: &mut ContextRunner<'_>,
) -> Result<SessionUpdateOutcome> {
    extract_session_using(db, id, home, regenerate, None, runner)
}
fn extract_session_using(
    db: &Db,
    id: &str,
    home: Option<&crate::workspace::home::NoEndingHome>,
    regenerate: bool,
    scope: Option<&interactive::Scope>,
    runner: &mut ContextRunner<'_>,
) -> Result<SessionUpdateOutcome> {
    with_operation(
        db,
        id,
        home,
        "session",
        scope,
        |launch, operation, _activity| {
            let session = db.get_session(id)?.ok_or_else(|| other("会话不存在"))?;
            let snapshot = snapshot_for(db, session, regenerate, launch.scope.as_ref())?;
            if snapshot.latest == snapshot.from {
                let revision = if snapshot.pending() {
                    db.tx(|tx| {
                        snapshot.validate(tx, None)?;
                        commit_session_context_conn(
                            tx,
                            id,
                            &snapshot.retained(tx)?,
                            snapshot.revision(),
                            snapshot.generation,
                            0,
                        )
                    })?
                } else {
                    snapshot.revision()
                };
                return Ok((
                    SessionUpdateOutcome {
                        session_id: id.into(),
                        status: if snapshot.pending() {
                            ContextUpdateStatus::Updated
                        } else {
                            ContextUpdateStatus::NoChange
                        },
                        revision,
                    },
                    if snapshot.pending() {
                        "updated"
                    } else {
                        "no_change"
                    },
                ));
            }
            let files = files_for(operation, &format!("session-{id}"), &[id.into()])?;
            files.add_session(
                &snapshot,
                !regenerate && snapshot.summary_generation == Some(snapshot.generation),
                false,
            );
            let raw = run_context_once(
                db,
                launch,
                &files.session_prompt(id, &launch.templates),
                &files,
                operation,
                runner,
            )?;
            operation.set_stage("output_validation");
            let reply = summary::parse_session_reply(&raw).map_err(invalid)?;
            let (fields, status) = match reply {
                SessionReply::Completed {} => (
                    read_summary_result(
                        db,
                        &files,
                        &files.session_result(id),
                        "session",
                        id,
                        snapshot.revision(),
                    )?,
                    ContextUpdateStatus::Updated,
                ),
                SessionReply::Skipped { .. } => {
                    files
                        .require_absent(&files.session_result(id))
                        .map_err(invalid)?;
                    (snapshot.retained(&db.read())?, ContextUpdateStatus::Skipped)
                }
                SessionReply::Failed { .. } => {
                    files
                        .require_absent(&files.session_result(id))
                        .map_err(invalid)?;
                    return Err(AppError::context(ContextUpdateError::ModelCallFailed(
                        "会话摘要提取失败".into(),
                    )));
                }
            };
            operation.set_stage("database_commit");
            let revision = db.tx(|tx| {
                snapshot.validate(tx, None)?;
                let revision = commit_session_context_conn(
                    tx,
                    id,
                    &fields,
                    snapshot.revision(),
                    snapshot.generation,
                    snapshot.latest,
                )?;
                crate::storage::freshness::mark_refreshed(tx, id)?;
                Ok(revision)
            })?;
            Ok((
                SessionUpdateOutcome {
                    session_id: id.into(),
                    status,
                    revision,
                },
                if status == ContextUpdateStatus::Skipped {
                    "skipped"
                } else {
                    "updated"
                },
            ))
        },
    )
}
fn task_projects(conn: &rusqlite::Connection, id: &str) -> Result<Vec<String>> {
    crate::storage::workstream_projects::project_ids_for_workstream(conn, id)
}
fn extract_task(
    db: &Db,
    id: &str,
    home: Option<&crate::workspace::home::NoEndingHome>,
    regenerate: bool,
    runner: &mut ContextRunner<'_>,
) -> Result<WorkstreamUpdateOutcome> {
    extract_task_using(db, id, home, regenerate, None, runner)
}
fn extract_task_using(
    db: &Db,
    id: &str,
    home: Option<&crate::workspace::home::NoEndingHome>,
    regenerate: bool,
    scope: Option<&interactive::Scope>,
    runner: &mut ContextRunner<'_>,
) -> Result<WorkstreamUpdateOutcome> {
    with_operation(
        db,
        id,
        home,
        "workstream",
        scope,
        |launch, operation, activity| {
            let ws = db.get_workstream(id)?.ok_or_else(|| other("任务不存在"))?;
            let state = query::task_state(&db.read(), id)?;
            let project_ids = task_projects(&db.read(), id)?;
            let snapshots = db
                .sessions_for_workstream(id)?
                .into_iter()
                .map(|s| snapshot_for(db, s, false, launch.scope.as_ref()))
                .collect::<Result<Vec<_>>>()?;
            let ids: Vec<String> = snapshots.iter().map(|s| s.session.id.clone()).collect();
            activity.add_sessions(ids.clone())?;
            let frontiers = db.workstream_frontiers(id)?;
            let pending = state.input_revision != state.consumed_input_revision
                || frontiers.len() != snapshots.len()
                || snapshots.iter().any(|s| {
                    s.pending()
                        || frontiers
                            .iter()
                            .find(|f| f.session_id == s.session.id)
                            .is_none_or(|f| f.session_context_revision != s.revision())
                });
            if !regenerate && !pending {
                return Ok((
                    WorkstreamUpdateOutcome {
                        workstream_id: id.into(),
                        status: ContextUpdateStatus::NoChange,
                        context_revision: state.context_revision,
                        updated_sessions: vec![],
                        skipped_session_ids: vec![],
                        failed_sessions: vec![],
                        remaining_pending: 0,
                    },
                    "no_change",
                ));
            }
            let batch = run_batch(
                db,
                id,
                "task",
                serde_json::json!({"type":"task","title":ws.title,"description":ws.description,
            "related_projects":crate::workspace::workstream::projects_for_workstream(db,id)?}),
                state.context_revision,
                &snapshots,
                regenerate,
                launch,
                operation,
                runner,
            )?;
            operation.set_stage("database_commit");
            let (revision, updated) = db.tx(|tx| {
            let current = get_workstream_context_state_conn(tx, id)?;
            let current_ws: Option<(String, String)> = tx
                .query_row(
                    "SELECT title,description FROM workstreams WHERE id=?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let mut current_ids: Vec<String> = {
                let mut st =
                    tx.prepare("SELECT id FROM sessions WHERE EXISTS (SELECT 1 FROM session_workstreams sw WHERE sw.session_id=sessions.id AND sw.workstream_id=?1) ORDER BY id")?;
                let rows = st
                    .query_map([id], |r| r.get(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                rows
            };
            let mut expected_ids = ids.clone();
            current_ids.sort();
            expected_ids.sort();
            if current.context_revision != state.context_revision
                || current.input_revision != state.input_revision
                || current_ws != Some((ws.title.clone(), ws.description.clone()))
                || current_ids != expected_ids
                || task_projects(tx, id)? != project_ids
            {
                return Err(stale());
            }
            for s in &snapshots {
                s.validate(tx, Some(id))?;
            }
            let mut updated = vec![];
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
                    ).and_then(|revision| {
                        crate::storage::freshness::mark_refreshed(tx, &s.session.id)?;
                        Ok(revision)
                    })?
                } else {
                    s.revision()
                };
                set_workstream_frontier_conn(
                    tx,
                    &crate::domain::WorkstreamSessionFrontier {
                        workstream_id: id.into(),
                        session_id: s.session.id.clone(),
                        session_context_revision: revision,
                    },
                )?;
            }
            context_repo::delete_unowned_workstream_frontiers_conn(tx, id)?;
            let revision = commit_workstream_summary_conn(tx, id, &batch.summary)?;
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
                WorkstreamUpdateOutcome {
                    workstream_id: id.into(),
                    status,
                    context_revision: revision,
                    updated_sessions: updated,
                    skipped_session_ids: batch.skipped,
                    failed_sessions: batch.failed,
                    remaining_pending: workstream_context_view(db, id)?.pending_sessions,
                },
                match status {
                    ContextUpdateStatus::Partial => "partial",
                    ContextUpdateStatus::Skipped => "skipped",
                    _ => "updated",
                },
            ))
        },
    )
}
struct BatchOutput {
    // Hold the locks and files until the transaction finishes.
    _files: ExtractionFiles,
    results: HashMap<String, SessionContextFields>,
    summary: crate::domain::Summary,
    failed: Vec<SessionFailure>,
    skipped: Vec<String>,
    target_skipped: bool,
}
fn run_batch(
    db: &Db,
    id: &str,
    kind: &str,
    header: serde_json::Value,
    target_revision: i64,
    snapshots: &[SessionSnapshot],
    regenerate: bool,
    launch: &ContextLaunchConfig,
    operation: &mut diagnostics::ContextOperation,
    runner: &mut ContextRunner<'_>,
) -> Result<BatchOutput> {
    let ids: Vec<String> = snapshots.iter().map(|s| s.session.id.clone()).collect();
    let files = files_for(operation, &format!("{kind}-{id}"), &ids)?;
    let mut targets = HashSet::new();
    add_batch_inputs(
        &files,
        kind,
        id,
        header,
        target_revision,
        regenerate,
        snapshots,
    );
    targets.extend(
        snapshots
            .iter()
            .filter(|s| s.latest > s.from)
            .map(|s| s.session.id.clone()),
    );
    let raw = run_context_once(
        db,
        launch,
        &files.batch_prompt(kind, id, &launch.templates),
        &files,
        operation,
        runner,
    )?;
    operation.set_stage("output_validation");
    let reply = summary::parse_batch_reply(&raw).map_err(invalid)?;
    let (failed, skipped, target_skipped) = match reply {
        BatchReply::Completed {
            skipped_session_ids,
        } => (vec![], skipped_session_ids, false),
        BatchReply::Partial {
            failed_sessions,
            skipped_session_ids,
        } => (failed_sessions, skipped_session_ids, false),
        BatchReply::Skipped {
            skipped_session_ids,
            ..
        } => (vec![], skipped_session_ids, true),
        BatchReply::Failed { .. } => {
            files
                .require_absent(&files.batch_result(kind, id))
                .map_err(invalid)?;
            return Err(AppError::context(ContextUpdateError::ModelCallFailed(
                "摘要提取失败".into(),
            )));
        }
    };
    let mut reported = HashSet::new();
    for failed in &failed {
        if failed.error.trim().is_empty() {
            return Err(invalid("失败原因为空"));
        }
    }
    for sid in failed.iter().map(|f| &f.session_id).chain(skipped.iter()) {
        if !targets.contains(sid) || !reported.insert(sid.clone()) {
            return Err(invalid("未知或重复会话 ID"));
        }
        files
            .require_absent(&files.session_result(sid))
            .map_err(invalid)?;
    }
    let target_summary = if target_skipped {
        files
            .require_absent(&files.batch_result(kind, id))
            .map_err(invalid)?;
        query::target_summary(&db.read(), kind, id, target_revision)?.unwrap_or_default()
    } else {
        read_summary_result(
            db,
            &files,
            &files.batch_result(kind, id),
            kind,
            id,
            target_revision,
        )?
        .summary
    };
    let failed_ids: HashSet<_> = failed.iter().map(|f| f.session_id.as_str()).collect();
    let mut results = HashMap::new();
    for s in snapshots {
        if failed_ids.contains(s.session.id.as_str()) {
            continue;
        }
        let fields = if targets.contains(&s.session.id) && !reported.contains(&s.session.id) {
            read_summary_result(
                db,
                &files,
                &files.session_result(&s.session.id),
                "session",
                &s.session.id,
                s.revision(),
            )?
        } else {
            if !targets.contains(&s.session.id) {
                files
                    .require_absent(&files.session_result(&s.session.id))
                    .map_err(invalid)?;
            }
            s.retained(&db.read())?
        };
        results.insert(s.session.id.clone(), fields);
    }
    Ok(BatchOutput {
        _files: files,
        results,
        summary: target_summary,
        failed,
        skipped,
        target_skipped,
    })
}

/// A single settings snapshot is used for both logging and the eventual CLI
/// invocation. This avoids recording a different Agent/model if settings
/// change while the extraction is being prepared.
struct ContextLaunchConfig {
    scope: Option<interactive::Scope>,
    templates: prompts::PromptTemplates,
    raw_agent: String,
    agent: Option<crate::domain::Agent>,
    opts: Option<crate::adapters::ExecOptions>,
}

impl ContextLaunchConfig {
    fn from_db(db: &Db, scope: Option<&interactive::Scope>) -> Result<Self> {
        if let Some(scope) = scope {
            scope.validate(db)?;
            return Ok(Self {
                scope: Some(scope.clone()),
                templates: scope.templates.clone(),
                raw_agent: "current-session".into(),
                agent: None,
                opts: None,
            });
        }
        let settings = ExtractionSettings::from_settings(db)?;
        let agent = crate::domain::Agent::parse(&settings.agent);
        let opts = settings.exec_options().ok().flatten().map(|(_, opts)| opts);
        let raw_agent = settings.agent;
        Ok(Self {
            scope: None,
            templates: prompts::PromptTemplates::default(),
            raw_agent,
            agent,
            opts,
        })
    }
}

fn require_cli(
    launch: &ContextLaunchConfig,
    operation: &mut diagnostics::ContextOperation,
) -> Result<CliExtractor> {
    if launch.raw_agent == "none" {
        operation.fail_with(
            "runtime_configuration",
            "agent_not_configured",
            "未选择提取 Agent，请在设置 → 摘要提取中选择 Codex、Claude Code 或 Pi。",
        );
        return Err(AppError::context(ContextUpdateError::AiUnavailable(
            "未选择 Context Agent".into(),
        )));
    }
    let Some(agent) = launch.agent else {
        operation.fail_with(
            "runtime_configuration",
            "agent_configuration_invalid",
            "Agent 配置无效，请重新选择 Agent。",
        );
        return Err(AppError::context(ContextUpdateError::AiUnavailable(
            "Agent 配置无效".into(),
        )));
    };
    let Some(opts) = launch.opts.as_ref() else {
        operation.fail_with(
            "runtime_configuration",
            "runtime_configuration_invalid",
            "提取运行参数无效，请检查设置 → 摘要提取。",
        );
        return Err(AppError::context(ContextUpdateError::AiUnavailable(
            "Agent Runtime 配置不可用".into(),
        )));
    };
    Ok(CliExtractor::new(agent, opts.clone()))
}

fn run_context_once(
    db: &Db,
    launch: &ContextLaunchConfig,
    prompt: &str,
    files: &ExtractionFiles,
    operation: &mut diagnostics::ContextOperation,
    runner: &mut ContextRunner<'_>,
) -> Result<String> {
    let cli = if launch.scope.is_some() {
        None
    } else {
        let mut cli = require_cli(launch, operation)?;
        cli.timeout_secs = if files.target.starts_with("session-") {
            30 * 60
        } else {
            90 * 60
        };
        operation.record_invocation_config(cli.agent, &cli.opts);
        Some(cli)
    };
    operation.set_stage("model_call");
    if let Some(scope) = &launch.scope {
        let raw = runner(None, prompt, &files.dir)
            .map_err(|e| invalid(format!("提取提交失败：{:?}", e.kind)))?;
        tools::validate_external(db, files, &raw, &scope.delivered)?;
        return Ok(raw);
    }
    tools::run(db, files, prompt, |tool_prompt| {
        // Service test runners simulate the Agent; use the real query bridge
        // to acknowledge their material reads. Production reads are on demand.
        #[cfg(test)]
        tools::read_all_for_test(&files.dir);
        match runner(cli.as_ref(), tool_prompt, &files.dir) {
            Ok(raw) => {
                operation.set_cli_result(cli.is_some(), cli.as_ref().map(|_| 0), None);
                operation.set_stage("output_validation");
                Ok(raw)
            }
            Err(failure) => {
                operation.set_cli_result(failure.spawned, failure.exit_code, failure.io_error_kind);
                let (code, message) = diagnostics::cli_failure(failure.kind);
                operation.fail_with("model_call", code, message);
                Err(other("context model call failed"))
            }
        }
    })
}

#[cfg(test)]
mod tests;

fn snapshot_for(
    db: &Db,
    session: Session,
    regenerate: bool,
    scope: Option<&interactive::Scope>,
) -> Result<SessionSnapshot> {
    if let Some(scope) = scope {
        let snapshot = scope
            .sessions
            .iter()
            .find(|s| s.session.id == session.id)
            .ok_or_else(stale)?
            .clone();
        snapshot.validate(&db.read(), None)?;
        Ok(snapshot)
    } else {
        SessionSnapshot::read(db, session, regenerate)
    }
}

fn add_batch_inputs(
    files: &ExtractionFiles,
    kind: &str,
    id: &str,
    header: serde_json::Value,
    target_revision: i64,
    regenerate: bool,
    snapshots: &[SessionSnapshot],
) {
    let mut ordered = snapshots.to_vec();
    ordered.sort_by_cached_key(|s| {
        (
            s.last_message_at()
                .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
                .map(|ts| ts.timestamp_millis()),
            s.session.id.clone(),
        )
    });
    for s in &ordered {
        if s.latest > s.from {
            files.add_session(
                s,
                !regenerate && s.summary_generation == Some(s.generation),
                false,
            );
        }
        if regenerate {
            files.add_session(s, false, true);
        }
    }
    files.add_batch(kind, id, header, target_revision, regenerate, ordered);
}
