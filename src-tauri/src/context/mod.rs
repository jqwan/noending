//! Explicit current-summary reads and extraction. One user action launches one
//! Agent; model replies and files validate before a snapshot-guarded transaction.
pub(crate) mod diagnostics;
mod preferences;
mod project;
pub mod prompts;
pub mod summary;
use crate::domain::{
    ContextUpdateError, ContextUpdateStatus, Session, SessionContextFields, SessionContextRecord,
    SessionMessage,
};
use crate::error::{other, AppError, Result};
use crate::storage::context_repo::{
    self, commit_session_context_conn, commit_workstream_summary_conn, get_session_context_conn,
    get_workstream_context_state_conn, projection_ids_conn, set_workstream_frontier_conn,
};
use crate::storage::Db;
use crate::sync::extractor::{self, AssistantConfig, CliExtractor};
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
}
#[derive(Debug, Clone, Serialize)]
pub struct WorkstreamContextView {
    pub workstream_id: String,
    pub summary: Option<String>,
    pub context_revision: i64,
    pub updated_at: Option<String>,
    pub pending: bool,
    pub pending_sessions: usize,
}
#[derive(Debug, Clone, Serialize)]
pub struct SessionUpdateOutcome {
    pub session_id: String,
    pub status: ContextUpdateStatus,
    pub revision: i64,
    pub preference_error: Option<String>,
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
    pub preference_error: Option<String>,
}

struct SessionSnapshot {
    session: Session,
    existing: Option<SessionContextRecord>,
    generation: i64,
    latest: i64,
    from: i64,
    messages: Vec<SessionMessage>,
    summary_through_at: Option<String>,
}
impl SessionSnapshot {
    fn read(db: &Db, session: Session, regenerate: bool) -> Result<Self> {
        let existing = db.get_session_context(&session.id)?;
        let (generation, latest) = db.get_session_ingest_state(&session.id)?;
        let from = existing
            .as_ref()
            .filter(|c| !regenerate && c.ingest_generation == generation)
            .map(|c| c.processed_through_seq)
            .unwrap_or(0);
        let messages = db.get_messages_after(&session.id, from, latest.saturating_sub(from))?;
        let summary_through_at = if from > 0 {
            db.get_messages_after(&session.id, from - 1, 1)?
                .into_iter()
                .next()
                .and_then(|m| m.ts)
        } else {
            None
        };
        Ok(Self {
            session,
            existing,
            generation,
            latest,
            from,
            messages,
            summary_through_at,
        })
    }
    fn last_message_at(&self) -> Option<&str> {
        match self.messages.last() {
            Some(message) => message.ts.as_deref(),
            None => self.summary_through_at.as_deref(),
        }
    }
    fn revision(&self) -> i64 {
        self.existing.as_ref().map(|c| c.revision).unwrap_or(0)
    }
    fn current_summary(&self) -> Option<&str> {
        self.existing
            .as_ref()
            .filter(|c| !c.fields.summary.is_empty())
            .map(|c| c.fields.summary.as_str())
    }
    fn pending(&self) -> bool {
        !self.messages.is_empty()
            || self
                .existing
                .as_ref()
                .is_some_and(|c| c.ingest_generation != self.generation)
    }
    fn retained(&self) -> SessionContextFields {
        SessionContextFields {
            summary: self.current_summary().unwrap_or_default().to_string(),
        }
    }
    fn validate(&self, conn: &rusqlite::Connection, owner: Option<&str>) -> Result<()> {
        let current = get_session_context_conn(conn, &self.session.id)?;
        let identity: Option<(i64, Option<String>, Option<String>)> = conn
            .query_row(
                "SELECT fact_generation, owner_workstream_id, (SELECT project_id FROM workspace_paths WHERE id=sessions.workspace_path_id) FROM sessions WHERE id = ?1",
                [&self.session.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if current.as_ref().map(|c| c.revision).unwrap_or(0) != self.revision()
            || identity.as_ref().map(|i| i.0) != Some(self.generation)
            || identity.as_ref().and_then(|i| i.2.as_ref()) != self.session.project_id.as_ref()
            || owner
                .is_some_and(|owner| identity.as_ref().and_then(|i| i.1.as_deref()) != Some(owner))
        {
            return Err(stale());
        }
        let ids = projection_ids_conn(conn, &self.session.id)?;
        let upper = self.from + self.messages.len() as i64;
        if ids.len() < upper as usize
            || self
                .messages
                .iter()
                .enumerate()
                .any(|(i, m)| ids.get(self.from as usize + i) != Some(&m.id))
        {
            return Err(stale());
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
    })
}

type ContextRunner<'a> = dyn FnMut(
        &CliExtractor,
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
        cli.run_context(prompt, dir)
    })
}
pub fn regenerate_session(
    db: &Db,
    id: &str,
    home: Option<&crate::workspace::home::NoEndingHome>,
) -> Result<SessionUpdateOutcome> {
    extract_session(db, id, home, true, &mut |cli, prompt, dir| {
        cli.run_context(prompt, dir)
    })
}
pub fn update_workstream(
    db: &Db,
    id: &str,
    home: Option<&crate::workspace::home::NoEndingHome>,
) -> Result<WorkstreamUpdateOutcome> {
    extract_task(db, id, home, false, &mut |cli, prompt, dir| {
        cli.run_context(prompt, dir)
    })
}
pub fn regenerate_workstream(
    db: &Db,
    id: &str,
    home: Option<&crate::workspace::home::NoEndingHome>,
) -> Result<WorkstreamUpdateOutcome> {
    extract_task(db, id, home, true, &mut |cli, prompt, dir| {
        cli.run_context(prompt, dir)
    })
}
fn with_operation<T>(
    db: &Db,
    id: &str,
    home: Option<&crate::workspace::home::NoEndingHome>,
    kind: &'static str,
    f: impl FnOnce(
        &ContextLaunchConfig,
        &mut diagnostics::ContextOperation,
    ) -> Result<(T, &'static str)>,
) -> Result<T> {
    let mut operation = diagnostics::ContextOperation::new(kind, id, home);
    let launch = ContextLaunchConfig::from_db(db)
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
        f(&launch, &mut operation)
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
    with_operation(db, id, home, "session", |launch, operation| {
        let session = db.get_session(id)?.ok_or_else(|| other("会话不存在"))?;
        let snapshot = SessionSnapshot::read(db, session, regenerate)?;
        if snapshot.messages.is_empty() {
            let revision = if snapshot.pending() {
                db.tx(|tx| {
                    snapshot.validate(tx, None)?;
                    commit_session_context_conn(
                        tx,
                        id,
                        &snapshot.retained(),
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
                    preference_error: None,
                },
                if snapshot.pending() {
                    "updated"
                } else {
                    "no_change"
                },
            ));
        }
        let files = files_for(operation, &format!("session-{id}"), &[id.into()])?;
        let preference_snapshot =
            preferences::Snapshot::read(db, snapshot.session.project_id.clone())?;
        files.write_preferences(&preference_snapshot)?;
        files.write_session(
            &snapshot.session,
            if regenerate {
                None
            } else {
                snapshot.current_summary()
            },
            snapshot.from,
            &snapshot.messages,
            snapshot.summary_through_at.as_deref(),
        )?;
        let mut cli = require_cli(launch, operation)?;
        cli.timeout_secs = 300;
        let raw = run_context_once(
            &cli,
            &files.session_prompt(id, &launch.templates),
            &files.dir,
            operation,
            runner,
        )?;
        operation.set_stage("output_validation");
        let reply = summary::parse_session_reply(&raw).map_err(invalid)?;
        let (fields, status, preference_reply) = match reply {
            SessionReply::Completed { preferences } => (
                files
                    .read_result(&files.session_result(id))
                    .map_err(invalid)?,
                ContextUpdateStatus::Updated,
                preferences,
            ),
            SessionReply::Skipped { preferences, .. } => {
                files
                    .require_absent(&files.session_result(id))
                    .map_err(invalid)?;
                (
                    snapshot.retained(),
                    ContextUpdateStatus::Skipped,
                    preferences,
                )
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
        let pending_preferences = preferences::Pending::read(
            preference_snapshot,
            &files.preference_result(),
            preference_reply,
        );
        operation.set_stage("database_commit");
        let (revision, preference_error) = db.tx(|tx| {
            snapshot.validate(tx, None)?;
            let revision = commit_session_context_conn(
                tx,
                id,
                &fields,
                snapshot.revision(),
                snapshot.generation,
                snapshot.latest,
            )?;
            Ok((revision, pending_preferences.apply(tx)?))
        })?;
        Ok((
            SessionUpdateOutcome {
                session_id: id.into(),
                status,
                revision,
                preference_error,
            },
            if status == ContextUpdateStatus::Skipped {
                "skipped"
            } else {
                "updated"
            },
        ))
    })
}
fn task_paths(conn: &rusqlite::Connection, id: &str) -> Result<Vec<String>> {
    let mut st=conn.prepare("SELECT wp.canonical_path FROM workstream_paths p JOIN workspace_paths wp ON wp.id=p.workspace_path_id WHERE p.workstream_id=?1 ORDER BY p.position")?;
    let rows = st
        .query_map([id], |r| r.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}
fn extract_task(
    db: &Db,
    id: &str,
    home: Option<&crate::workspace::home::NoEndingHome>,
    regenerate: bool,
    runner: &mut ContextRunner<'_>,
) -> Result<WorkstreamUpdateOutcome> {
    with_operation(db, id, home, "workstream", |launch, operation| {
        let ws = db.get_workstream(id)?.ok_or_else(|| other("任务不存在"))?;
        let state = db.get_workstream_context_state(id)?;
        let paths = task_paths(&db.read(), id)?;
        let snapshots = db
            .sessions_for_workstream(id)?
            .into_iter()
            .map(|s| SessionSnapshot::read(db, s, false))
            .collect::<Result<Vec<_>>>()?;
        let ids: Vec<String> = snapshots.iter().map(|s| s.session.id.clone()).collect();
        let view = workstream_context_view(db, id)?;
        if !regenerate && !view.pending {
            return Ok((
                WorkstreamUpdateOutcome {
                    workstream_id: id.into(),
                    status: ContextUpdateStatus::NoChange,
                    context_revision: state.context_revision,
                    updated_sessions: vec![],
                    skipped_session_ids: vec![],
                    failed_sessions: vec![],
                    remaining_pending: 0,
                    preference_error: None,
                },
                "no_change",
            ));
        }
        let batch = run_batch(
            db,
            id,
            "task",
            serde_json::json!({"type":"task","title":ws.title,"description":ws.description,
            "working_directories":paths,"current_summary":if regenerate || state.summary.is_empty() {None} else {Some(&state.summary)}}),
            &state.summary,
            &snapshots,
            regenerate,
            launch,
            operation,
            runner,
        )?;
        operation.set_stage("database_commit");
        let (revision, updated, preference_error) = db.tx(|tx| {
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
                    tx.prepare("SELECT id FROM sessions WHERE owner_workstream_id=?1 ORDER BY id")?;
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
                || task_paths(tx, id)? != paths
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
                    )?
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
            Ok((revision, updated, batch.preferences.apply(tx)?))
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
                preference_error,
            },
            match status {
                ContextUpdateStatus::Partial => "partial",
                ContextUpdateStatus::Skipped => "skipped",
                _ => "updated",
            },
        ))
    })
}
struct BatchOutput {
    // Hold the locks and files until the transaction finishes.
    _files: ExtractionFiles,
    results: HashMap<String, SessionContextFields>,
    summary: String,
    failed: Vec<SessionFailure>,
    skipped: Vec<String>,
    target_skipped: bool,
    preferences: preferences::Pending,
}
fn run_batch(
    db: &Db,
    id: &str,
    kind: &str,
    header: serde_json::Value,
    current_summary: &str,
    snapshots: &[SessionSnapshot],
    regenerate: bool,
    launch: &ContextLaunchConfig,
    operation: &mut diagnostics::ContextOperation,
    runner: &mut ContextRunner<'_>,
) -> Result<BatchOutput> {
    let ids: Vec<String> = snapshots.iter().map(|s| s.session.id.clone()).collect();
    let files = files_for(operation, &format!("{kind}-{id}"), &ids)?;
    let projects = snapshots
        .iter()
        .filter_map(|s| s.session.project_id.clone())
        .chain((kind == "project").then(|| id.to_string()));
    let preference_snapshot = preferences::Snapshot::read(db, projects)?;
    files.write_preferences(&preference_snapshot)?;
    let mut records = vec![header];
    let mut targets = HashSet::new();
    let mut ordered: Vec<_> = snapshots.iter().collect();
    ordered.sort_by_cached_key(|s| {
        (
            s.last_message_at()
                .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
                .map(|ts| ts.timestamp_millis()),
            s.session.id.clone(),
        )
    });
    for s in ordered {
        let need_update = !s.messages.is_empty();
        if need_update {
            targets.insert(s.session.id.clone());
            files.write_session(
                &s.session,
                s.current_summary(),
                s.from,
                &s.messages,
                s.summary_through_at.as_deref(),
            )?;
        }
        let preference_input_file = if regenerate {
            let messages = db.get_messages_after(&s.session.id, 0, s.latest)?;
            files.write_preference_session(&s.session, &messages)?;
            Some(files.preference_session_input(&s.session.id))
        } else {
            None
        };
        records.push(
            serde_json::json!({"type":"session","session_id":s.session.id,"project_id":s.session.project_id,"title":s.session.title,
                "started_at":s.session.started_at,"last_message_at":s.last_message_at(),"summary_through_at":s.summary_through_at,
                "need_update":need_update,"current_summary":s.current_summary(),
                "preference_input_file":preference_input_file,
                "input_file":need_update.then(||files.session_input(&s.session.id)),
                "result_file":need_update.then(||files.session_result(&s.session.id))}),
        );
    }
    files.write_batch(kind, id, &records)?;
    let mut cli = require_cli(launch, operation)?;
    cli.timeout_secs = 900;
    let raw = run_context_once(
        &cli,
        &files.batch_prompt(kind, id, &launch.templates),
        &files.dir,
        operation,
        runner,
    )?;
    operation.set_stage("output_validation");
    let reply = summary::parse_batch_reply(&raw).map_err(invalid)?;
    let (failed, skipped, target_skipped, preference_reply) = match reply {
        BatchReply::Completed {
            skipped_session_ids,
            preferences,
        } => (vec![], skipped_session_ids, false, preferences),
        BatchReply::Partial {
            failed_sessions,
            skipped_session_ids,
            preferences,
        } => (failed_sessions, skipped_session_ids, false, preferences),
        BatchReply::Skipped {
            skipped_session_ids,
            preferences,
            ..
        } => (vec![], skipped_session_ids, true, preferences),
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
        current_summary.to_string()
    } else {
        files
            .read_result(&files.batch_result(kind, id))
            .map_err(invalid)?
            .summary
    };
    let failed_ids: HashSet<_> = failed.iter().map(|f| f.session_id.as_str()).collect();
    let mut results = HashMap::new();
    for s in snapshots {
        if failed_ids.contains(s.session.id.as_str()) {
            continue;
        }
        let fields = if targets.contains(&s.session.id) && !reported.contains(&s.session.id) {
            files
                .read_result(&files.session_result(&s.session.id))
                .map_err(invalid)?
        } else {
            if !targets.contains(&s.session.id) {
                files
                    .require_absent(&files.session_result(&s.session.id))
                    .map_err(invalid)?;
            }
            s.retained()
        };
        results.insert(s.session.id.clone(), fields);
    }
    let preferences = preferences::Pending::read(
        preference_snapshot,
        &files.preference_result(),
        preference_reply,
    );
    Ok(BatchOutput {
        preferences,
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
    templates: prompts::PromptTemplates,
    raw_agent: String,
    agent: Option<crate::domain::Agent>,
    opts: Option<crate::adapters::ExecOptions>,
}

impl ContextLaunchConfig {
    fn from_db(db: &Db) -> Result<Self> {
        let raw_agent = AssistantConfig::from_settings(db).agent;
        let agent = crate::domain::Agent::parse(&raw_agent);
        let opts =
            agent.and_then(|agent| crate::agent_runtime::runtime_exec_options(db, agent).ok());
        Ok(Self {
            templates: prompts::templates(db)?,
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
            "未选择 Context Agent，请在助手设置中选择 Codex、Claude Code 或 Pi。",
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
            "Agent Runtime 配置无效，请检查 Settings → Agent。",
        );
        return Err(AppError::context(ContextUpdateError::AiUnavailable(
            "Agent Runtime 配置不可用".into(),
        )));
    };
    Ok(CliExtractor::new(agent, opts.clone()))
}

fn run_context_once(
    cli: &CliExtractor,
    prompt: &str,
    runtime_dir: &std::path::Path,
    operation: &mut diagnostics::ContextOperation,
    runner: &mut ContextRunner<'_>,
) -> Result<String> {
    operation.record_invocation_config(cli.agent, &cli.opts);
    operation.set_stage("model_call");
    match runner(cli, prompt, runtime_dir) {
        Ok(raw) => {
            operation.set_cli_result(true, Some(0), None);
            Ok(raw)
        }
        Err(failure) => {
            operation.set_cli_result(failure.spawned, failure.exit_code, failure.io_error_kind);
            let (code, message) = diagnostics::cli_failure(failure.kind);
            operation.fail_with("model_call", code, message);
            Err(other("context model call failed"))
        }
    }
}

#[cfg(test)]
mod tests;
