//! Summary reads and extraction, delegated to an Agent or performed by the
//! current session. Both paths validate before a snapshot-guarded transaction.
mod collaboration;
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
use crate::storage::context_repo::commit_session_context_conn;
use crate::storage::Db;
use crate::sync::extractor::{self, CliExtractor, ExtractionSettings};
pub use project::{
    project_context_view, regenerate_project, update_project, ProjectContextView,
    ProjectUpdateOutcome,
};
use serde::{Deserialize, Serialize};

use summary::{SessionFailure, SessionReply};

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
    pub pending_messages: i64,
    pub processed_through_seq: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionUpdateOutcome {
    pub session_id: String,
    pub status: ContextUpdateStatus,
    pub revision: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkstreamUpdateOutcome {
    pub workstream_id: String,
    pub status: ContextUpdateStatus,
    pub context_revision: i64,
    pub processed_through_seq: i64,
    pub remaining_pending: i64,
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
    fn validate(&self, conn: &rusqlite::Connection) -> Result<()> {
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
        Ok(())
    }

    fn commit(&self, tx: &rusqlite::Connection, fields: &SessionContextFields) -> Result<i64> {
        self.validate(tx)?;
        let revision = commit_session_context_conn(
            tx,
            &self.session.id,
            fields,
            self.revision,
            self.generation,
            self.latest,
        )?;
        crate::storage::freshness::mark_refreshed(tx, &self.session.id)?;
        Ok(revision)
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
        freshness: freshness::summary(&db.read(), "session", id)?,
        pending: latest > from
            || ctx
                .as_ref()
                .is_some_and(|c| c.ingest_generation != generation),
    })
}
pub fn workstream_context_view(db: &Db, id: &str) -> Result<WorkstreamContextView> {
    collaboration::view(db, id)
}

type ContextRunner<'a> = dyn FnMut(
        &CliExtractor,
        &str,
        &std::path::Path,
    ) -> std::result::Result<(), extractor::ContextCallFailure>
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
fn extract_session(
    db: &Db,
    id: &str,
    home: Option<&crate::workspace::home::NoEndingHome>,
    regenerate: bool,
    runner: &mut ContextRunner<'_>,
) -> Result<SessionUpdateOutcome> {
    extract(db, id, home, "session", regenerate, runner)
}
fn extract_task(
    db: &Db,
    id: &str,
    home: Option<&crate::workspace::home::NoEndingHome>,
    regenerate: bool,
    runner: &mut ContextRunner<'_>,
) -> Result<WorkstreamUpdateOutcome> {
    extract(db, id, home, "task", regenerate, runner)
}

fn extract<T: serde::de::DeserializeOwned>(
    db: &Db,
    id: &str,
    home: Option<&crate::workspace::home::NoEndingHome>,
    kind: &'static str,
    regenerate: bool,
    runner: &mut ContextRunner<'_>,
) -> Result<T> {
    let mut activity = crate::operations::Operation::start(
        db,
        crate::operations::OperationStatus::summary(kind, id),
    )?;
    let log_kind = if kind == "task" { "workstream" } else { kind };
    let mut operation = diagnostics::ContextOperation::new(log_kind, id, home);
    let result = (|| {
        let home = home.ok_or_else(|| {
            operation.fail_with(
                "home_resolution",
                "home_unavailable",
                "应用工作目录未初始化，无法运行摘要提取。",
            );
            other("context home unavailable")
        })?;
        let launch = ContextLaunchConfig::from_db(db)?;
        operation.record_config(&launch.raw_agent, launch.agent, launch.opts.as_ref());
        operation.set_stage("input_preparation");
        let root = diagnostics::prepare_runtime_dir(home)?;
        let mut scope = interactive::Scope::new(db, home, kind, id, regenerate.into())?;
        let _locks = interactive::lock_scope(&root, &scope).map_err(|error| {
            if matches!(&error, AppError::Io(io) if io.kind() == std::io::ErrorKind::WouldBlock) {
                operation.fail_with(
                    "input_preparation",
                    "summary_busy",
                    "相关摘要正在提取，请稍后重试。",
                );
            }
            error
        })?;
        scope.validate_target(db)?;
        if let Some(outcome) = scope.no_change(db)? {
            return Ok(outcome);
        }
        if let Some(outcome) = scope.refresh_empty_session(db)? {
            return Ok(outcome);
        }
        activity.add_sessions(scope.pending_session_ids().map(str::to_owned))?;
        if let Some(progress) = scope.project_summary_progress() {
            activity.set_project_summary_progress(progress)?;
        }
        let mut cli = require_cli(&launch, &mut operation)?;
        cli.timeout_secs = if kind == "session" { 30 * 60 } else { 90 * 60 };
        operation.set_stage("model_call");
        let execution =
            tools::run(
                db,
                &mut scope,
                home,
                &root,
                &mut activity,
                |prompt, dir| match runner(&cli, prompt, dir) {
                    Ok(()) => {
                        operation.set_cli_result(true, Some(0), None);
                        Ok(())
                    }
                    Err(failure) => {
                        operation.set_cli_result(
                            failure.spawned,
                            failure.exit_code,
                            failure.io_error_kind,
                        );
                        let (code, message) = diagnostics::cli_failure(failure.kind);
                        operation.record_cli_failure(code, message);
                        Err(other("context model call failed"))
                    }
                },
            );
        operation.record_session_failures(scope.failures());
        if let Some(outcome) = scope.outcome(db)? {
            return Ok(outcome);
        }
        execution?;
        operation.set_stage("submission");
        Err(invalid("提取会话退出前未提交摘要"))
    })();
    match result {
        Ok(outcome) => {
            let status = match outcome["status"].as_str() {
                Some("updated") => "updated",
                Some("partial") => "partial",
                Some("skipped") => "skipped",
                _ => "no_change",
            };
            operation.succeed(status);
            Ok(serde_json::from_value(outcome)?)
        }
        Err(error) => Err(AppError::ContextUpdateFailed(operation.fail(&error))),
    }
}

fn task_projects(conn: &rusqlite::Connection, id: &str) -> Result<Vec<String>> {
    crate::storage::workstream_projects::project_ids_for_workstream(conn, id)
}

/// A single settings snapshot is used for logging and CLI invocation.
struct ContextLaunchConfig {
    raw_agent: String,
    agent: Option<crate::domain::Agent>,
    opts: Option<crate::adapters::ExecOptions>,
}

impl ContextLaunchConfig {
    fn from_db(db: &Db) -> Result<Self> {
        let settings = ExtractionSettings::from_settings(db)?;
        let agent = crate::domain::Agent::parse(&settings.agent);
        let opts = settings.exec_options().ok().flatten().map(|(_, opts)| opts);
        let raw_agent = settings.agent;
        Ok(Self {
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
            "未选择提取 Agent，请在设置 → 摘要提取中选择。",
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

#[cfg(test)]
mod tests;
