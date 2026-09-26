//! Context read models and the explicit Context update service.
//!
//! Two read commands (`getSessionContext`, `getWorkstreamContext`) plus
//! pending-state reads are pure reads — they never scan a Source or call AI.
//!
//! Two write entry points each run AT MOST ONE model call, then validate and
//! commit in a single transaction:
//!
//! * `update_session` — asks the model for one new four-field summary of the
//!   Session and commits it with CAS on the revision / generation / message
//!   prefix.
//! * `update_workstream` — asks once for all affected Session Contexts AND the
//!   Workstream mutations, and commits everything atomically.
//!
//! CLI failure / timeout / invalid output NEVER falls back to a heuristic.

pub(crate) mod diagnostics;

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::domain::WorkstreamSessionFrontier;
use crate::domain::{
    workstream_visibility, ContextItem, ContextItemRevision, ContextUpdateError,
    ContextUpdateStatus, Session, SessionContextFields, SessionContextRecord, SessionMessage,
};
use crate::error::{other, AppError, Result};
use crate::storage::context_repo::{
    self, commit_session_context_conn, consume_input_revision_conn, get_ingest_state_conn,
    get_session_context_conn, get_workstream_context_state_conn, projection_ids_conn,
    set_workstream_frontier_conn,
};
use crate::storage::Db;
use crate::sync::extractor::{
    self, AssistantConfig, CliExtractor, PromptInput, PromptRefs, PromptSession, INPUT_LIMIT_BYTES,
    SESSION_CONTEXT_RESERVE_BYTES, WORKSTREAM_RESERVE_BYTES,
};
use crate::sync::merge::MergeEngine;
use crate::sync::{MergeContext, SessionContextDraft};

use crate::domain::CORE_ITEM_TYPES;

// Read models

/// L1 projection: active items of core types, in canonical order.
#[derive(Debug, Clone, Serialize)]
pub struct ContextSection {
    pub kind: String,
    pub title: String,
    pub content: String,
    pub authority: String,
    pub source_ref: Option<String>,
    pub item_id: String,
    pub revision_id: Option<String>,
}

pub fn resolve_core_context(db: &Db, workstream_id: &str) -> Result<Vec<ContextSection>> {
    let items = db.items_for_workstream(workstream_id, false)?;
    let mut out = Vec::new();
    for want in CORE_ITEM_TYPES {
        for (item, rev) in &items {
            if &item.kind == want {
                out.push(section_of(item, rev));
            }
        }
    }
    Ok(out)
}

fn section_of(item: &ContextItem, rev: &ContextItemRevision) -> ContextSection {
    ContextSection {
        kind: item.kind.clone(),
        title: rev.title.clone(),
        content: rev.content.clone(),
        authority: item.authority.clone(),
        source_ref: rev.source_ref.clone(),
        item_id: item.id.clone(),
        revision_id: Some(rev.id.clone()),
    }
}

/// The read model behind `getSessionContext`.
#[derive(Debug, Clone, Serialize)]
pub struct SessionContextView {
    pub session_id: String,
    /// `None` means the Session has never had a summary generated.
    pub fields: Option<SessionContextFields>,
    pub revision: i64,
    pub ingest_generation: i64,
    pub processed_through_seq: i64,
    pub latest_message_seq: i64,
    pub updated_at: Option<String>,
    /// True when there is work an "更新摘要" would process.
    pub pending: bool,
}

/// The read model behind `getWorkstreamContext`.
#[derive(Debug, Clone, Serialize)]
pub struct WorkstreamContextView {
    pub workstream_id: String,
    pub title: String,
    pub description: String,
    pub lifecycle: String,
    pub sections: Vec<ContextSection>,
    pub context_revision: i64,
    pub input_revision: i64,
    pub consumed_input_revision: i64,
    /// True when a "更新状态" has something to synthesize.
    pub pending: bool,
    pub pending_sessions: usize,
}

pub fn session_context_view(db: &Db, session_id: &str) -> Result<SessionContextView> {
    let ctx = db.get_session_context(session_id)?;
    let ingest = db.get_session_ingest_state(session_id)?;
    let from = match &ctx {
        Some(c) if c.ingest_generation == ingest.generation => c.processed_through_seq,
        _ => 0,
    };
    let generation_changed = ctx
        .as_ref()
        .is_some_and(|context| context.ingest_generation != ingest.generation);
    let pending = generation_changed || ingest.latest_message_seq > from;
    Ok(SessionContextView {
        session_id: session_id.to_string(),
        fields: ctx.as_ref().map(|c| c.fields.clone()),
        revision: ctx.as_ref().map(|c| c.revision).unwrap_or(0),
        ingest_generation: ingest.generation,
        processed_through_seq: from,
        latest_message_seq: ingest.latest_message_seq,
        updated_at: ctx.as_ref().map(|c| c.updated_at.clone()),
        pending,
    })
}

pub fn workstream_context_view(db: &Db, workstream_id: &str) -> Result<WorkstreamContextView> {
    let ws = db
        .get_workstream(workstream_id)?
        .ok_or_else(|| other("Workstream 不存在"))?;
    let state = db.get_workstream_context_state(workstream_id)?;
    let sessions = db.sessions_for_workstream(workstream_id)?;
    let mut pending_sessions = 0;
    for s in &sessions {
        if session_needs_workstream_update(db, workstream_id, s)? {
            pending_sessions += 1;
        }
    }
    let input_pending = state.input_revision > state.consumed_input_revision;
    let items = db.items_for_workstream(workstream_id, false)?;
    let sections = items.iter().map(|(i, r)| section_of(i, r)).collect();
    Ok(WorkstreamContextView {
        workstream_id: ws.id.clone(),
        title: ws.title.clone(),
        description: ws.description.clone(),
        lifecycle: ws.lifecycle.clone(),
        sections,
        context_revision: state.context_revision,
        input_revision: state.input_revision,
        consumed_input_revision: state.consumed_input_revision,
        pending: input_pending || pending_sessions > 0,
        pending_sessions,
    })
}

fn session_needs_workstream_update(db: &Db, workstream_id: &str, s: &Session) -> Result<bool> {
    let ingest = db.get_session_ingest_state(&s.id)?;
    let ctx = db.get_session_context(&s.id)?;
    if ctx
        .as_ref()
        .is_some_and(|context| context.ingest_generation != ingest.generation)
    {
        return Ok(true);
    }
    let frontier = db
        .workstream_frontiers(workstream_id)?
        .into_iter()
        .find(|f| f.session_id == s.id);
    let Some(f) = frontier else {
        return Ok(ingest.latest_message_seq > 0);
    };
    if f.ingest_generation != ingest.generation {
        return Ok(true);
    }
    if ingest.latest_message_seq > f.consumed_through_seq {
        return Ok(true);
    }
    let rev = ctx.as_ref().map(|c| c.revision).unwrap_or(0);
    Ok(rev > f.session_context_revision)
}

// Outcomes

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
    pub mutations_applied: usize,
    /// Sessions still needing an update after this call (budget-limited).
    pub remaining_pending: usize,
}

// Session update

pub fn update_session(
    db: &Db,
    session_id: &str,
    home: Option<&crate::workspace::home::NoEndingHome>,
) -> Result<SessionUpdateOutcome> {
    let mut runner = |cli: &CliExtractor, prompt: &str, runtime_dir: &std::path::Path| {
        cli.run_context(prompt, runtime_dir)
    };
    update_session_with_runner(db, session_id, home, &mut runner)
}

type ContextRunner<'a> = dyn FnMut(
        &CliExtractor,
        &str,
        &std::path::Path,
    ) -> std::result::Result<String, extractor::ContextCallFailure>
    + 'a;

/// A single settings snapshot is used for both logging and the eventual CLI
/// invocation. This avoids recording a different Agent/model if settings
/// change while the extraction is being prepared.
struct ContextLaunchConfig {
    raw_agent: String,
    agent: Option<crate::domain::Agent>,
    opts: Option<crate::adapters::ExecOptions>,
}

impl ContextLaunchConfig {
    fn from_db(db: &Db) -> Self {
        let raw_agent = AssistantConfig::from_settings(db).agent;
        let agent = crate::domain::Agent::parse(&raw_agent);
        let opts =
            agent.and_then(|agent| crate::agent_runtime::runtime_exec_options(db, agent).ok());
        Self {
            raw_agent,
            agent,
            opts,
        }
    }
}

fn update_session_with_runner(
    db: &Db,
    session_id: &str,
    home: Option<&crate::workspace::home::NoEndingHome>,
    runner: &mut ContextRunner<'_>,
) -> Result<SessionUpdateOutcome> {
    let mut operation = diagnostics::ContextOperation::new("session", session_id, home);
    let launch = ContextLaunchConfig::from_db(db);
    operation.record_config(&launch.raw_agent, launch.agent, launch.opts.as_ref());
    if home.is_none() {
        operation.fail_with(
            "home_resolution",
            "home_unavailable",
            "NoEnding Home 未初始化，无法安全运行 Context Agent。",
        );
        let failure = operation.fail(&other("context home unavailable"));
        return Err(AppError::ContextUpdateFailed(failure));
    }
    match update_session_inner(db, session_id, &launch, &mut operation, runner) {
        Ok(outcome) => {
            operation.succeed(match outcome.status {
                ContextUpdateStatus::NoChange => "no_change",
                ContextUpdateStatus::Partial => "partial",
                _ => "updated",
            });
            Ok(outcome)
        }
        Err(error) => {
            let failure = operation.fail(&error);
            Err(AppError::ContextUpdateFailed(failure))
        }
    }
}

fn update_session_inner(
    db: &Db,
    session_id: &str,
    launch: &ContextLaunchConfig,
    operation: &mut diagnostics::ContextOperation,
    runner: &mut ContextRunner<'_>,
) -> Result<SessionUpdateOutcome> {
    let session = db.get_session(session_id)?.ok_or_else(|| {
        operation.fail_with("snapshot", "session_missing", "找不到要更新的会话。");
        other("会话不存在")
    })?;
    if session.trashed_at.is_some() {
        operation.fail_with(
            "snapshot",
            "session_trashed",
            "会话已移入回收站，恢复后才能更新摘要。",
        );
        return Err(other("会话已移入回收站，暂不可更新摘要"));
    }

    let existing = db.get_session_context(session_id)?;
    let ingest = db.get_session_ingest_state(session_id)?;
    let from = match &existing {
        Some(c) if c.ingest_generation == ingest.generation => c.processed_through_seq,
        _ => 0,
    };
    let all = db.get_messages_after(session_id, from, 1_000_000)?;
    if all.is_empty() {
        if existing
            .as_ref()
            .is_some_and(|context| context.ingest_generation != ingest.generation)
        {
            let expect_rev = existing
                .as_ref()
                .map(|context| context.revision)
                .unwrap_or(0);
            operation.set_stage("database_commit");
            let revision = commit_session_update(
                db,
                session_id,
                expect_rev,
                ingest.generation,
                0,
                0,
                &[],
                &SessionContextFields::default(),
            )?
            .ok_or_else(|| {
                AppError::context(ContextUpdateError::ConcurrencyConflict(
                    "内容已变化，请重新更新".into(),
                ))
            })?;
            return Ok(SessionUpdateOutcome {
                session_id: session_id.to_string(),
                status: ContextUpdateStatus::Updated,
                revision,
            });
        }
        return Ok(SessionUpdateOutcome {
            session_id: session_id.to_string(),
            status: ContextUpdateStatus::NoChange,
            revision: existing.as_ref().map(|c| c.revision).unwrap_or(0),
        });
    }

    operation.set_stage("runtime_configuration");
    let cli = require_cli(launch, operation)?;

    let heading = session
        .title
        .clone()
        .unwrap_or_else(|| "该会话".to_string());
    let build = |msgs: &[SessionMessage]| -> (String, PromptRefs) {
        let current_context = existing
            .as_ref()
            .filter(|context| context.ingest_generation == ingest.generation);
        let input = PromptInput {
            workstream_title: format!("仅更新该 Session 摘要（{heading}）"),
            workstream_description: String::new(),
            item_lines: Vec::new(),
            sessions: vec![PromptSession {
                session_id: session_id.to_string(),
                is_target: true,
                existing: current_context.map(|c| c.fields.clone()),
                existing_revision: current_context.map(|c| c.revision),
                messages: msgs.to_vec(),
            }],
            messages: msgs.to_vec(),
        };
        extractor::build_update_prompt(&input)
    };

    operation.set_stage("input_preparation");
    let (msgs, prompt, refs) = select_message_prefix(build, &all, SESSION_CONTEXT_RESERVE_BYTES)?;
    let upper = from + msgs.len() as i64;

    let home = operation
        .home()
        .ok_or_else(|| other("context home unavailable"))?;
    let runtime_dir = diagnostics::prepare_runtime_dir(home).map_err(|_| {
        operation.fail_with(
            "runtime_setup",
            "runtime_dir_unavailable",
            "无法创建 Context 专用运行目录，请检查 NoEnding Home。",
        );
        other("context runtime directory unavailable")
    })?;
    let raw = run_context_once(&cli, &prompt, &runtime_dir, operation, runner)?;
    operation.set_stage("output_validation");
    let expected_targets = vec![session_id.to_string()];
    let allowed_items: HashSet<String> = HashSet::new();
    let parsed = extractor::parse_update_output(&raw, &refs, "", &expected_targets, &allowed_items)
        .map_err(|e| AppError::context(ContextUpdateError::InvalidOutput(e.to_string())))?;
    let draft = parsed.session_contexts.into_iter().next().ok_or_else(|| {
        AppError::context(ContextUpdateError::InvalidOutput("缺少摘要输出".into()))
    })?;

    let expect_rev = existing.as_ref().map(|c| c.revision).unwrap_or(0);
    let prefix: Vec<String> = msgs.iter().map(|m| m.id.clone()).collect();
    operation.set_stage("database_commit");
    let committed = commit_session_update(
        db,
        session_id,
        expect_rev,
        ingest.generation,
        from,
        upper,
        &prefix,
        &draft.fields,
    )?;
    let revision = committed.ok_or_else(|| {
        AppError::context(ContextUpdateError::ConcurrencyConflict(
            "内容已变化，请重新更新".into(),
        ))
    })?;

    let status = if upper >= ingest.latest_message_seq {
        ContextUpdateStatus::Updated
    } else {
        ContextUpdateStatus::Partial
    };
    Ok(SessionUpdateOutcome {
        session_id: session_id.to_string(),
        status,
        revision,
    })
}

/// Commit one Session Context update with CAS on revision / generation /
/// message prefix. Returns `Ok(None)` when the snapshot moved (nothing was
/// written) so the caller can surface `stale_snapshot`.
#[allow(clippy::too_many_arguments)]
fn commit_session_update(
    db: &Db,
    session_id: &str,
    expect_rev: i64,
    generation: i64,
    from: i64,
    upper: i64,
    prefix: &[String],
    fields: &SessionContextFields,
) -> Result<Option<i64>> {
    db.tx(|tx| {
        let cur_rev = get_session_context_conn(tx, session_id)?
            .map(|c| c.revision)
            .unwrap_or(0);
        if cur_rev != expect_rev {
            return Ok(None);
        }
        if get_ingest_state_conn(tx, session_id)?.generation != generation {
            return Ok(None);
        }
        let ids = projection_ids_conn(tx, session_id)?;
        if ids.len() < upper as usize {
            return Ok(None);
        }
        let start = from as usize;
        for (i, exp) in prefix.iter().enumerate() {
            if ids.get(start + i) != Some(exp) {
                return Ok(None);
            }
        }
        let rev =
            commit_session_context_conn(tx, session_id, fields, expect_rev, generation, upper)?;
        Ok(Some(rev))
    })
}

// Workstream combined update

struct TargetSession {
    session: Session,
    existing: Option<SessionContextRecord>,
    frontier: Option<WorkstreamSessionFrontier>,
    generation: i64,
    from: i64,
    messages: Vec<SessionMessage>,
}

impl Clone for TargetSession {
    fn clone(&self) -> Self {
        TargetSession {
            session: self.session.clone(),
            existing: self.existing.clone(),
            frontier: self.frontier.clone(),
            generation: self.generation,
            from: self.from,
            messages: self.messages.clone(),
        }
    }
}

pub fn update_workstream(
    db: &Db,
    workstream_id: &str,
    home: Option<&crate::workspace::home::NoEndingHome>,
) -> Result<WorkstreamUpdateOutcome> {
    let mut runner = |cli: &CliExtractor, prompt: &str, runtime_dir: &std::path::Path| {
        cli.run_context(prompt, runtime_dir)
    };
    update_workstream_with_runner(db, workstream_id, home, &mut runner)
}

fn update_workstream_with_runner(
    db: &Db,
    workstream_id: &str,
    home: Option<&crate::workspace::home::NoEndingHome>,
    runner: &mut ContextRunner<'_>,
) -> Result<WorkstreamUpdateOutcome> {
    let mut operation = diagnostics::ContextOperation::new("workstream", workstream_id, home);
    let launch = ContextLaunchConfig::from_db(db);
    operation.record_config(&launch.raw_agent, launch.agent, launch.opts.as_ref());
    if home.is_none() {
        operation.fail_with(
            "home_resolution",
            "home_unavailable",
            "NoEnding Home 未初始化，无法安全运行 Context Agent。",
        );
        let failure = operation.fail(&other("context home unavailable"));
        return Err(AppError::ContextUpdateFailed(failure));
    }
    match update_workstream_inner(db, workstream_id, &launch, &mut operation, runner) {
        Ok(outcome) => {
            operation.succeed(match outcome.status {
                ContextUpdateStatus::NoChange => "no_change",
                ContextUpdateStatus::Partial => "partial",
                _ => "updated",
            });
            Ok(outcome)
        }
        Err(error) => {
            let failure = operation.fail(&error);
            Err(AppError::ContextUpdateFailed(failure))
        }
    }
}

fn update_workstream_inner(
    db: &Db,
    workstream_id: &str,
    launch: &ContextLaunchConfig,
    operation: &mut diagnostics::ContextOperation,
    runner: &mut ContextRunner<'_>,
) -> Result<WorkstreamUpdateOutcome> {
    let ws = db.get_workstream(workstream_id)?.ok_or_else(|| {
        operation.fail_with(
            "snapshot",
            "workstream_missing",
            "找不到要更新的 Workstream。",
        );
        other("Workstream 不存在")
    })?;
    if ws.visibility == workstream_visibility::ARCHIVED {
        operation.fail_with(
            "snapshot",
            "workstream_archived",
            "已归档的 Workstream 只读，请先恢复后再更新状态。",
        );
        return Err(other("已归档的 Workstream 只读，恢复后才能更新状态"));
    }

    let state = db.get_workstream_context_state(workstream_id)?;
    let items = db.items_for_workstream(workstream_id, false)?;
    let allowed_items: HashSet<String> = items.iter().map(|(i, _)| i.id.clone()).collect();
    let item_lines: Vec<String> = items
        .iter()
        .map(|(i, r)| format!("- item_id={} | {} | {}", i.id, i.kind, r.title))
        .collect();

    let frontiers: HashMap<String, WorkstreamSessionFrontier> = db
        .workstream_frontiers(workstream_id)?
        .into_iter()
        .map(|f| (f.session_id.clone(), f))
        .collect();

    let sessions = db.sessions_for_workstream(workstream_id)?;

    let mut targets: Vec<TargetSession> = Vec::new();
    let mut read_only: Vec<ReadOnlySession> = Vec::new();

    for session in sessions {
        let existing = db.get_session_context(&session.id)?;
        let ingest = db.get_session_ingest_state(&session.id)?;
        let frontier = frontiers.get(&session.id).cloned();

        let generation_changed = match &frontier {
            Some(f) => f.ingest_generation != ingest.generation,
            None => ingest.latest_message_seq > 0,
        } || existing
            .as_ref()
            .is_some_and(|context| context.ingest_generation != ingest.generation);
        let from = if generation_changed {
            0
        } else {
            frontier
                .as_ref()
                .map(|f| f.consumed_through_seq)
                .unwrap_or(0)
        };
        let messages = db.get_messages_after(&session.id, from, 1_000_000)?;

        if !messages.is_empty() {
            targets.push(TargetSession {
                session,
                existing: existing.clone(),
                frontier,
                generation: ingest.generation,
                from,
                messages,
            });
        } else if generation_changed {
            read_only.push(ReadOnlySession {
                session_id: session.id,
                context: existing,
                generation: ingest.generation,
                latest_message_seq: ingest.latest_message_seq,
                invalidate_context: true,
            });
        } else if let Some(c) = existing {
            let rev = c.revision;
            let consumed = frontier
                .as_ref()
                .map(|f| f.session_context_revision)
                .unwrap_or(0);
            if rev > consumed {
                read_only.push(ReadOnlySession {
                    session_id: session.id,
                    context: Some(c),
                    generation: ingest.generation,
                    latest_message_seq: ingest.latest_message_seq,
                    invalidate_context: false,
                });
            }
        }
    }

    let input_pending = state.input_revision > state.consumed_input_revision;
    if targets.is_empty() && read_only.is_empty() && !input_pending {
        return Ok(WorkstreamUpdateOutcome {
            workstream_id: workstream_id.to_string(),
            status: ContextUpdateStatus::NoChange,
            context_revision: state.context_revision,
            updated_sessions: Vec::new(),
            mutations_applied: 0,
            remaining_pending: 0,
        });
    }

    operation.set_stage("runtime_configuration");
    let cli = require_cli(launch, operation)?;

    // Budget: reserve output space only for target sessions actually included.
    // Read-only sessions only carry their existing summary.
    operation.set_stage("input_preparation");
    let (included, prompt, refs) =
        select_workstream_inputs(&ws, &item_lines, &targets, &read_only)?;

    let expected_targets: Vec<String> = included.iter().map(|t| t.session.id.clone()).collect();

    let home = operation
        .home()
        .ok_or_else(|| other("context home unavailable"))?;
    let runtime_dir = diagnostics::prepare_runtime_dir(home).map_err(|_| {
        operation.fail_with(
            "runtime_setup",
            "runtime_dir_unavailable",
            "无法创建 Context 专用运行目录，请检查 NoEnding Home。",
        );
        other("context runtime directory unavailable")
    })?;
    let raw = run_context_once(&cli, &prompt, &runtime_dir, operation, runner)?;
    operation.set_stage("output_validation");
    let parsed = extractor::parse_update_output(
        &raw,
        &refs,
        workstream_id,
        &expected_targets,
        &allowed_items,
    )
    .map_err(|e| AppError::context(ContextUpdateError::InvalidOutput(e.to_string())))?;

    operation.set_stage("database_commit");
    let outcome = commit_workstream_update(
        db,
        workstream_id,
        &state,
        &included,
        &read_only,
        parsed,
        cli.name(),
    )?;

    let remaining_pending = targets.len().saturating_sub(included.len());
    Ok(WorkstreamUpdateOutcome {
        workstream_id: workstream_id.to_string(),
        status: if remaining_pending > 0 {
            ContextUpdateStatus::Partial
        } else {
            ContextUpdateStatus::Updated
        },
        context_revision: outcome.context_revision,
        updated_sessions: outcome.updated_sessions,
        mutations_applied: outcome.mutations_applied,
        remaining_pending,
    })
}

struct CommitOutcome {
    context_revision: i64,
    updated_sessions: Vec<String>,
    mutations_applied: usize,
}

#[derive(Clone)]
struct ReadOnlySession {
    session_id: String,
    context: Option<SessionContextRecord>,
    generation: i64,
    latest_message_seq: i64,
    invalidate_context: bool,
}

#[allow(clippy::too_many_arguments)]
fn commit_workstream_update(
    db: &Db,
    workstream_id: &str,
    snapshot: &crate::domain::WorkstreamContextState,
    included: &[TargetSession],
    read_only: &[ReadOnlySession],
    parsed: crate::sync::ContextUpdateOutput,
    runtime: String,
) -> Result<CommitOutcome> {
    let ctx = MergeContext {
        runtime,
        workstream_id: workstream_id.to_string(),
    };
    let merger = MergeEngine;
    let session_contexts: HashMap<String, SessionContextFields> = parsed
        .session_contexts
        .into_iter()
        .map(|SessionContextDraft { session_id, fields }| (session_id, fields))
        .collect();
    let mutations = parsed.workstream_mutations;

    db.tx(|tx| {
        // CAS on the two Workstream revisions.
        let cur = get_workstream_context_state_conn(tx, workstream_id)?;
        if cur.context_revision != snapshot.context_revision
            || cur.input_revision != snapshot.input_revision
        {
            return Ok(Err(AppError::context(
                ContextUpdateError::ConcurrencyConflict("Workstream 状态已变化，请重新更新".into()),
            )));
        }
        // CAS on each included target's Session Context revision / generation /
        // message prefix.
        for t in included {
            let cur_rev = get_session_context_conn(tx, &t.session.id)?
                .map(|c| c.revision)
                .unwrap_or(0);
            let expect_rev = t.existing.as_ref().map(|c| c.revision).unwrap_or(0);
            if cur_rev != expect_rev {
                return Ok(Err(AppError::context(
                    ContextUpdateError::ConcurrencyConflict(format!(
                        "会话 {} 的摘要已变化，请重新更新",
                        t.session.id
                    )),
                )));
            }
            if get_ingest_state_conn(tx, &t.session.id)?.generation != t.generation {
                return Ok(Err(AppError::context(
                    ContextUpdateError::ConcurrencyConflict(format!(
                        "会话 {} 的对话已改写，请重新更新",
                        t.session.id
                    )),
                )));
            }
            let ids = projection_ids_conn(tx, &t.session.id)?;
            let upper = t.from + t.messages.len() as i64;
            if ids.len() < upper as usize {
                return Ok(Err(AppError::context(
                    ContextUpdateError::ConcurrencyConflict("会话内容已变化，请重新更新".into()),
                )));
            }
            for (i, m) in t.messages.iter().enumerate() {
                if ids.get(t.from as usize + i) != Some(&m.id) {
                    return Ok(Err(AppError::context(
                        ContextUpdateError::ConcurrencyConflict(
                            "会话内容已变化，请重新更新".into(),
                        ),
                    )));
                }
            }
        }
        // Read-only Sessions can still have new Session Context revisions or
        // an empty replacement generation to consume. Guard the snapshot and
        // advance their frontier even though no new messages are sent as model
        // targets.
        for r in read_only {
            let current = get_session_context_conn(tx, &r.session_id)?;
            let current_revision = current.as_ref().map(|c| c.revision).unwrap_or(0);
            let expected_revision = r.context.as_ref().map(|c| c.revision).unwrap_or(0);
            let ingest = get_ingest_state_conn(tx, &r.session_id)?;
            if current_revision != expected_revision
                || ingest.generation != r.generation
                || ingest.latest_message_seq != r.latest_message_seq
            {
                return Ok(Err(AppError::context(
                    ContextUpdateError::ConcurrencyConflict(format!(
                        "会话 {} 的摘要或对话已变化，请重新更新",
                        r.session_id
                    )),
                )));
            }
        }

        // Apply Workstream mutations.
        let mut applied = 0usize;
        for m in &mutations {
            if merger.apply(tx, m, &ctx)? {
                applied += 1;
            }
        }

        // Write every included Session Context + revision + Workstream frontier.
        let mut updated_sessions = Vec::new();
        for t in included {
            let Some(fields) = session_contexts.get(&t.session.id) else {
                continue;
            };
            let expect_rev = t.existing.as_ref().map(|c| c.revision).unwrap_or(0);
            let upper = t.from + t.messages.len() as i64;
            let rev = commit_session_context_conn(
                tx,
                &t.session.id,
                fields,
                expect_rev,
                t.generation,
                upper,
            )?;
            set_workstream_frontier_conn(
                tx,
                &WorkstreamSessionFrontier {
                    workstream_id: workstream_id.to_string(),
                    session_id: t.session.id.clone(),
                    session_context_revision: rev,
                    ingest_generation: t.generation,
                    consumed_through_seq: upper,
                },
            )?;
            updated_sessions.push(t.session.id.clone());
        }

        // Preserve frontiers for every Session that still belongs to this
        // Workstream, including unchanged and budget-excluded Sessions.
        context_repo::delete_unowned_workstream_frontiers_conn(tx, workstream_id)?;

        for r in read_only {
            let revision = if r.invalidate_context {
                if let Some(context) = &r.context {
                    commit_session_context_conn(
                        tx,
                        &r.session_id,
                        &SessionContextFields::default(),
                        context.revision,
                        r.generation,
                        0,
                    )?
                } else {
                    0
                }
            } else {
                r.context.as_ref().map(|c| c.revision).unwrap_or(0)
            };
            set_workstream_frontier_conn(
                tx,
                &WorkstreamSessionFrontier {
                    workstream_id: workstream_id.to_string(),
                    session_id: r.session_id.clone(),
                    session_context_revision: revision,
                    ingest_generation: r.generation,
                    consumed_through_seq: r.latest_message_seq,
                },
            )?;
        }

        let context_revision = context_repo::bump_context_revision_conn(tx, workstream_id)?;
        consume_input_revision_conn(tx, workstream_id)?;

        Ok(Ok(CommitOutcome {
            context_revision,
            updated_sessions,
            mutations_applied: applied,
        }))
    })
    .and_then(|r| r)
}

// Helpers

fn require_cli(
    launch: &ContextLaunchConfig,
    operation: &mut diagnostics::ContextOperation,
) -> Result<CliExtractor> {
    if launch.raw_agent == "none" {
        operation.fail_with(
            "runtime_configuration",
            "agent_not_configured",
            "未选择 Context Agent，请在 Assistant 设置中选择 Codex、Claude Code 或 Pi。",
        );
        return Err(AppError::context(ContextUpdateError::AiUnavailable(
            "未选择 Context Agent".into(),
        )));
    }
    let Some(agent) = launch.agent else {
        operation.fail_with(
            "runtime_configuration",
            "agent_configuration_invalid",
            "Assistant Agent 配置无效，请重新选择 Agent。",
        );
        return Err(AppError::context(ContextUpdateError::AiUnavailable(
            "Assistant Agent 配置无效".into(),
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

/// Choose the largest whole-message prefix whose prompt fits the input budget.
fn select_message_prefix(
    build: impl Fn(&[SessionMessage]) -> (String, PromptRefs),
    all: &[SessionMessage],
    reserve: usize,
) -> Result<(Vec<SessionMessage>, String, PromptRefs)> {
    let budget = INPUT_LIMIT_BYTES.saturating_sub(reserve);
    let (full_prompt, full_refs) = build(all);
    if full_prompt.len() <= budget {
        return Ok((all.to_vec(), full_prompt, full_refs));
    }
    let (one_prompt, _) = build(&all[..1]);
    if one_prompt.len() > budget {
        return Err(AppError::context(ContextUpdateError::InputTooLarge(
            "即使最小的请求也超出输入预算".into(),
        )));
    }
    let (mut lo, mut hi) = (1usize, all.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let (p, _) = build(&all[..mid]);
        if p.len() <= budget {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let (p, r) = build(&all[..lo]);
    Ok((all[..lo].to_vec(), p, r))
}

/// Include whole target Sessions in stable order while the request fits.
fn select_workstream_inputs(
    ws: &crate::domain::Workstream,
    item_lines: &[String],
    targets: &[TargetSession],
    read_only: &[ReadOnlySession],
) -> Result<(Vec<TargetSession>, String, PromptRefs)> {
    let build = |inc: &[usize]| -> (String, PromptRefs) {
        let mut sessions: Vec<PromptSession> = Vec::new();
        let mut messages: Vec<SessionMessage> = Vec::new();
        for &i in inc {
            let t = &targets[i];
            let current_context = t
                .existing
                .as_ref()
                .filter(|context| context.ingest_generation == t.generation);
            sessions.push(PromptSession {
                session_id: t.session.id.clone(),
                is_target: true,
                existing: current_context.map(|c| c.fields.clone()),
                existing_revision: current_context.map(|c| c.revision),
                messages: t.messages.clone(),
            });
            messages.extend(t.messages.iter().cloned());
        }
        for r in read_only {
            sessions.push(PromptSession {
                session_id: r.session_id.clone(),
                is_target: false,
                existing: if r.invalidate_context {
                    Some(SessionContextFields::default())
                } else {
                    r.context.as_ref().map(|c| c.fields.clone())
                },
                existing_revision: r.context.as_ref().map(|c| c.revision),
                messages: Vec::new(),
            });
        }
        let input = PromptInput {
            workstream_title: ws.title.clone(),
            workstream_description: ws.description.clone(),
            item_lines: item_lines.to_vec(),
            sessions,
            messages,
        };
        extractor::build_update_prompt(&input)
    };

    if targets.is_empty() {
        let (p, r) = build(&[]);
        let budget = INPUT_LIMIT_BYTES.saturating_sub(WORKSTREAM_RESERVE_BYTES);
        if p.len() > budget {
            return Err(AppError::context(ContextUpdateError::InputTooLarge(
                "Workstream 与只读输入的组合已超出预算".into(),
            )));
        }
        return Ok((Vec::new(), p, r));
    }

    // Greedy: add targets while they fit; a target is included WHOLE.
    let mut included: Vec<usize> = Vec::new();
    for i in 0..targets.len() {
        let mut trial = included.clone();
        trial.push(i);
        let (p, _) = build(&trial);
        let reserve = WORKSTREAM_RESERVE_BYTES + SESSION_CONTEXT_RESERVE_BYTES * trial.len();
        let budget = INPUT_LIMIT_BYTES.saturating_sub(reserve);
        if p.len() <= budget {
            included = trial;
        } else {
            break;
        }
    }
    if included.is_empty() {
        return Err(AppError::context(ContextUpdateError::InputTooLarge(
            "第一个目标会话已超出输入预算".into(),
        )));
    }
    let (p, r) = build(&included);
    let chosen = included.into_iter().map(|i| targets[i].clone()).collect();
    Ok((chosen, p, r))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        Agent, ParsedSessionMessage, SessionMessageRole, SourceCursorUpdate, Workstream,
    };
    use crate::workspace::home::NoEndingHome;
    use serde_json::Value;
    use std::path::PathBuf;

    fn test_workstream() -> Workstream {
        Workstream {
            id: "workstream-context-test".into(),
            title: "Context test".into(),
            description: String::new(),
            lifecycle: "active".into(),
            visibility: "normal".into(),
            created_at: crate::storage::now(),
            updated_at: crate::storage::now(),
        }
    }

    fn test_session(id: &str) -> Session {
        Session {
            id: id.into(),
            agent: Agent::Codex,
            root_agent_session_id: format!("root-{id}"),
            title: Some(id.into()),
            owner_workstream_id: None,
            cwd: None,
            workspace_path_id: None,
            project_id: None,
            forked_from_session_id: None,
            started_at: None,
            last_activity_at: None,
            last_conversation_at: None,
            trashed_at: None,
        }
    }

    struct ServiceFixture {
        db: Option<Db>,
        root: PathBuf,
    }

    impl ServiceFixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "noending-context-service-{}",
                crate::storage::new_id()
            ));
            std::fs::create_dir_all(&root).unwrap();
            let db = Db::open(&root.join("db.sqlite")).unwrap();
            Self { db: Some(db), root }
        }

        fn db(&self) -> &Db {
            self.db.as_ref().unwrap()
        }

        fn home(&self) -> NoEndingHome {
            NoEndingHome::new(self.root.join("home").to_str().unwrap(), None).unwrap()
        }

        fn add_session(&self, message_count: usize, message_size: usize) -> String {
            let db = self.db();
            let root_id = format!("context-service-root-{}", crate::storage::new_id());
            let session_id = db
                .upsert_logical_session_unchecked(
                    Agent::Codex,
                    &root_id,
                    Some("Context service fixture"),
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap()
                .0;
            let member_id = db
                .upsert_session_member(
                    &session_id,
                    Agent::Codex,
                    &root_id,
                    crate::domain::SessionMemberRelation::Root,
                    None,
                    "context_service_test",
                    "/tmp/context-service-fixture",
                    None,
                    None,
                    None,
                    &serde_json::json!({}),
                )
                .unwrap();
            let content = "x".repeat(message_size);
            let messages: Vec<ParsedSessionMessage> = (0..message_count)
                .map(|i| ParsedSessionMessage {
                    source_message_id: Some(format!("source-{i}")),
                    source_position: i.to_string(),
                    ts: None,
                    role: SessionMessageRole::User,
                    content: content.clone(),
                    provider: None,
                    model: None,
                })
                .collect();
            db.commit_member_ingest(
                &session_id,
                &member_id,
                &messages,
                None,
                &SourceCursorUpdate {
                    file_identity: "context-service-fixture".into(),
                    generation: 0,
                    byte_offset: 10_000,
                    last_seen_size: 10_000,
                    mtime: None,
                    start_byte_offset: 0,
                    prefix_hash: String::new(),
                },
            )
            .unwrap();
            session_id
        }

        fn add_workstream(&self, id: &str) -> Workstream {
            let mut workstream = test_workstream();
            workstream.id = id.into();
            self.db().upsert_workstream(&workstream).unwrap();
            workstream
        }
    }

    impl Drop for ServiceFixture {
        fn drop(&mut self) {
            drop(self.db.take());
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn valid_output(session_id: &str) -> String {
        format!(
            r#"{{"session_contexts":[{{"session_id":"{session_id}","summary_current_state":"context updated","decisions":[],"open_questions":[],"next_steps":[]}}],"workstream_mutations":[]}}"#
        )
    }

    fn log_records(home: &NoEndingHome) -> Vec<Value> {
        std::fs::read_dir(diagnostics::log_dir(home))
            .unwrap()
            .flat_map(|entry| {
                std::fs::read_to_string(entry.unwrap().path())
                    .unwrap()
                    .lines()
                    .map(serde_json::from_str)
                    .collect::<Vec<serde_json::Result<Value>>>()
            })
            .map(|result| result.unwrap())
            .collect()
    }

    fn failure_code(result: crate::error::Result<impl std::fmt::Debug>) -> String {
        match result.unwrap_err() {
            AppError::ContextUpdateFailed(failure) => failure.code,
            other => panic!("expected structured Context error, got {other:?}"),
        }
    }

    #[test]
    fn service_no_change_and_early_config_failure_are_logged_without_cli() {
        let fixture = ServiceFixture::new();
        let home = fixture.home();
        let ws = fixture.add_workstream("service-no-change");
        fixture
            .db()
            .tx(|tx| context_repo::consume_input_revision_conn(tx, &ws.id))
            .unwrap();
        let calls = std::cell::Cell::new(0);
        let mut runner = |_: &CliExtractor, _: &str, _: &std::path::Path| {
            calls.set(calls.get() + 1);
            unreachable!("no-change workstream must not invoke the CLI")
        };
        let no_change =
            update_workstream_with_runner(fixture.db(), &ws.id, Some(&home), &mut runner).unwrap();
        assert_eq!(no_change.status, ContextUpdateStatus::NoChange);
        assert_eq!(calls.get(), 0);
        let record = &log_records(&home)[0];
        assert_eq!(record["stage"], "no_change");
        assert_eq!(record["outcome"], "no_change");
        assert_eq!(record["cli_invoked"], false);

        let empty_session_id = fixture.add_session(0, 0);
        let empty_session =
            update_session_with_runner(fixture.db(), &empty_session_id, Some(&home), &mut runner)
                .unwrap();
        assert_eq!(empty_session.status, ContextUpdateStatus::NoChange);
        assert_eq!(calls.get(), 0);
        assert_eq!(log_records(&home)[1]["outcome"], "no_change");
        assert_eq!(log_records(&home)[1]["cli_invoked"], false);

        let session_id = fixture.add_session(1, 64);
        fixture.db().set_setting("assistant.agent", "none").unwrap();
        let result =
            update_session_with_runner(fixture.db(), &session_id, Some(&home), &mut runner);
        assert_eq!(failure_code(result), "agent_not_configured");
        assert_eq!(calls.get(), 0);
        let records = log_records(&home);
        assert_eq!(records.len(), 3);
        let record = records.last().unwrap();
        assert_eq!(record["stage"], "runtime_configuration");
        assert_eq!(record["cli_invoked"], false);
        assert_eq!(record["agent"], "none");
    }

    #[test]
    fn service_session_updates_are_partial_then_complete_and_record_each_call() {
        let fixture = ServiceFixture::new();
        let home = fixture.home();
        let session_id = fixture.add_session(50, 1_200);
        let mut runner = |cli: &CliExtractor, _: &str, runtime_dir: &std::path::Path| {
            assert_eq!(cli.agent, Agent::Codex);
            assert_eq!(runtime_dir, diagnostics::runtime_dir(&home));
            Ok(valid_output(&session_id))
        };

        let first = update_session_with_runner(fixture.db(), &session_id, Some(&home), &mut runner)
            .unwrap();
        assert_eq!(first.status, ContextUpdateStatus::Partial);
        assert!(
            session_context_view(fixture.db(), &session_id)
                .unwrap()
                .pending
        );

        let second =
            update_session_with_runner(fixture.db(), &session_id, Some(&home), &mut runner)
                .unwrap();
        assert_eq!(second.status, ContextUpdateStatus::Updated);
        let view = session_context_view(fixture.db(), &session_id).unwrap();
        assert!(!view.pending);
        assert_eq!(
            view.fields.unwrap().summary_current_state,
            "context updated"
        );

        let records = log_records(&home);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["outcome"], "partial");
        assert_eq!(records[1]["outcome"], "updated");
        assert!(records.iter().all(|record| record["cli_invoked"] == true));
    }

    #[test]
    fn service_freezes_the_launch_agent_and_override_for_invocation_and_log() {
        let fixture = ServiceFixture::new();
        let home = fixture.home();
        let session_id = fixture.add_session(1, 80);
        fixture
            .db()
            .set_setting("assistant.agent", "codex")
            .unwrap();
        fixture
            .db()
            .set_setting("agent.runtime.codex", r#"{"model":"gpt-context-test"}"#)
            .unwrap();
        let mut runner = |cli: &CliExtractor, _: &str, _: &std::path::Path| {
            assert_eq!(cli.agent, Agent::Codex);
            assert_eq!(cli.opts.model.as_deref(), Some("gpt-context-test"));
            fixture.db().set_setting("assistant.agent", "none").unwrap();
            fixture
                .db()
                .set_setting("agent.runtime.codex", r#"{"model":"changed-during-call"}"#)
                .unwrap();
            Ok(valid_output(&session_id))
        };

        update_session_with_runner(fixture.db(), &session_id, Some(&home), &mut runner).unwrap();
        let record = &log_records(&home)[0];
        assert_eq!(record["agent"], "codex");
        assert_eq!(record["model_override"], "model=gpt-context-test");
    }

    #[test]
    fn service_invalid_workstream_output_logs_validation_and_preserves_context_and_frontiers() {
        let fixture = ServiceFixture::new();
        let home = fixture.home();
        let ws = fixture.add_workstream("service-invalid-output");
        let session_id = fixture.add_session(1, 80);
        fixture
            .db()
            .set_session_owner(&session_id, Some(&ws.id))
            .unwrap();
        let frontiers_before = fixture.db().workstream_frontiers(&ws.id).unwrap();
        assert!(frontiers_before.is_empty());
        let state_before = fixture.db().get_workstream_context_state(&ws.id).unwrap();
        let mut runner = |_: &CliExtractor, _: &str, _: &std::path::Path| Ok("not JSON".into());

        let result = update_workstream_with_runner(fixture.db(), &ws.id, Some(&home), &mut runner);
        assert_eq!(failure_code(result), "invalid_output");
        assert!(fixture
            .db()
            .get_session_context(&session_id)
            .unwrap()
            .is_none());
        let state_after = fixture.db().get_workstream_context_state(&ws.id).unwrap();
        assert_eq!(state_after.context_revision, state_before.context_revision);
        assert_eq!(state_after.input_revision, state_before.input_revision);
        assert_eq!(
            state_after.consumed_input_revision,
            state_before.consumed_input_revision
        );
        assert!(fixture
            .db()
            .workstream_frontiers(&ws.id)
            .unwrap()
            .is_empty());
        let record = &log_records(&home)[0];
        assert_eq!(record["stage"], "output_validation");
        assert_eq!(record["cli_invoked"], true);
        assert_eq!(record["error_code"], "invalid_output");
    }

    #[test]
    fn service_workstream_success_commits_session_context_and_frontier() {
        let fixture = ServiceFixture::new();
        let home = fixture.home();
        let ws = fixture.add_workstream("service-workstream-success");
        let session_id = fixture.add_session(1, 80);
        fixture
            .db()
            .set_session_owner(&session_id, Some(&ws.id))
            .unwrap();
        let mut runner =
            |_: &CliExtractor, _: &str, _: &std::path::Path| Ok(valid_output(&session_id));

        let outcome =
            update_workstream_with_runner(fixture.db(), &ws.id, Some(&home), &mut runner).unwrap();
        assert_eq!(outcome.status, ContextUpdateStatus::Updated);
        assert_eq!(outcome.updated_sessions, vec![session_id.clone()]);
        assert_eq!(
            fixture
                .db()
                .get_session_context(&session_id)
                .unwrap()
                .unwrap()
                .fields
                .summary_current_state,
            "context updated"
        );
        let frontiers = fixture.db().workstream_frontiers(&ws.id).unwrap();
        assert_eq!(frontiers.len(), 1);
        assert_eq!(frontiers[0].session_id, session_id);
        assert_eq!(frontiers[0].consumed_through_seq, 1);
        let record = &log_records(&home)[0];
        assert_eq!(record["stage"], "updated");
        assert_eq!(record["cli_invoked"], true);
    }

    #[test]
    fn service_workstream_commit_conflict_is_logged_after_the_cli_call() {
        let fixture = ServiceFixture::new();
        let home = fixture.home();
        let ws = fixture.add_workstream("service-commit-conflict");
        let session_id = fixture.add_session(1, 80);
        fixture
            .db()
            .set_session_owner(&session_id, Some(&ws.id))
            .unwrap();
        let mut runner = |_: &CliExtractor, _: &str, _: &std::path::Path| {
            fixture
                .db()
                .tx(|tx| {
                    crate::storage::context_repo::bump_input_revision_conn(tx, &ws.id)?;
                    Ok(())
                })
                .expect("inject Workstream snapshot conflict");
            Ok(valid_output(&session_id))
        };

        let result = update_workstream_with_runner(fixture.db(), &ws.id, Some(&home), &mut runner);
        assert_eq!(failure_code(result), "stale_snapshot");
        assert!(fixture
            .db()
            .get_session_context(&session_id)
            .unwrap()
            .is_none());
        let record = &log_records(&home)[0];
        assert_eq!(record["stage"], "database_commit");
        assert_eq!(record["cli_invoked"], true);
        assert_eq!(record["error_code"], "stale_snapshot");
    }

    #[test]
    fn service_runtime_directory_failure_never_invokes_runner() {
        let fixture = ServiceFixture::new();
        let home = fixture.home();
        let session_id = fixture.add_session(1, 80);
        std::fs::create_dir_all(&home.root).unwrap();
        std::fs::write(&home.runtime_dir, "not a directory").unwrap();
        let mut calls = 0;
        let mut runner = |_: &CliExtractor, _: &str, _: &std::path::Path| {
            calls += 1;
            Ok(valid_output(&session_id))
        };

        let result =
            update_session_with_runner(fixture.db(), &session_id, Some(&home), &mut runner);
        assert_eq!(failure_code(result), "runtime_dir_unavailable");
        assert_eq!(calls, 0);
        let record = &log_records(&home)[0];
        assert_eq!(record["stage"], "runtime_setup");
        assert_eq!(record["cli_invoked"], false);
    }

    #[test]
    fn service_success_survives_unwritable_context_log_directory() {
        let fixture = ServiceFixture::new();
        let home = fixture.home();
        let session_id = fixture.add_session(1, 80);
        std::fs::create_dir_all(&home.logs_dir).unwrap();
        std::fs::write(diagnostics::log_dir(&home), "not a directory").unwrap();
        let mut runner =
            |_: &CliExtractor, _: &str, _: &std::path::Path| Ok(valid_output(&session_id));

        let outcome =
            update_session_with_runner(fixture.db(), &session_id, Some(&home), &mut runner)
                .unwrap();
        assert_eq!(outcome.status, ContextUpdateStatus::Updated);
        assert_eq!(
            fixture
                .db()
                .get_session_context(&session_id)
                .unwrap()
                .unwrap()
                .fields
                .summary_current_state,
            "context updated"
        );
    }

    #[test]
    fn service_target_state_errors_keep_recovery_instructions() {
        let fixture = ServiceFixture::new();
        let home = fixture.home();
        let session_id = fixture.add_session(1, 80);
        fixture
            .db()
            .tx(|tx| {
                tx.execute(
                    "UPDATE sessions SET trashed_at = ?1 WHERE id = ?2",
                    rusqlite::params![crate::storage::now(), session_id],
                )?;
                Ok(())
            })
            .unwrap();
        let mut runner = |_: &CliExtractor, _: &str, _: &std::path::Path| {
            unreachable!("trashed session cannot invoke the CLI")
        };
        let result =
            update_session_with_runner(fixture.db(), &session_id, Some(&home), &mut runner);
        assert_eq!(failure_code(result), "session_trashed");
        assert!(log_records(&home)[0]["message"]
            .as_str()
            .unwrap()
            .contains("恢复"));

        let archived = fixture.add_workstream("service-archived");
        let mut archived = archived;
        archived.visibility = workstream_visibility::ARCHIVED.into();
        fixture.db().upsert_workstream(&archived).unwrap();
        let result =
            update_workstream_with_runner(fixture.db(), &archived.id, Some(&home), &mut runner);
        assert_eq!(failure_code(result), "workstream_archived");
        let records = log_records(&home);
        assert!(records[1]["message"].as_str().unwrap().contains("恢复"));
    }

    #[test]
    fn backlog_budget_reserves_output_for_only_the_included_prefix() {
        let ws = test_workstream();
        let targets: Vec<TargetSession> = (0..10)
            .map(|i| TargetSession {
                session: test_session(&format!("session-{i}")),
                existing: None,
                frontier: None,
                generation: 0,
                from: 0,
                messages: Vec::new(),
            })
            .collect();

        let (included, _, _) =
            select_workstream_inputs(&ws, &[], &targets, &[]).expect("first short target fits");
        assert!(!included.is_empty());
        assert!(
            included.len() < targets.len(),
            "the backlog is returned as a prefix"
        );
    }

    #[test]
    fn workstream_update_advances_read_only_and_preserves_unchanged_frontiers() {
        let dir =
            std::env::temp_dir().join(format!("noending-context-{}", crate::storage::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Db::open(&dir.join("test.db")).unwrap();
        let ws = test_workstream();
        db.upsert_workstream(&ws).unwrap();
        let session_id = db
            .upsert_logical_session_unchecked(
                Agent::Codex,
                "context-revision-frontier-root",
                Some("Context frontier"),
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap()
            .0;
        db.set_session_owner(&session_id, Some(&ws.id)).unwrap();

        let unchanged_session_id = db
            .upsert_logical_session_unchecked(
                Agent::Codex,
                "unchanged-context-frontier-root",
                Some("Unchanged frontier"),
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap()
            .0;
        db.set_session_owner(&unchanged_session_id, Some(&ws.id))
            .unwrap();
        db.tx(|tx| {
            set_workstream_frontier_conn(
                tx,
                &WorkstreamSessionFrontier {
                    workstream_id: ws.id.clone(),
                    session_id: unchanged_session_id.clone(),
                    session_context_revision: 0,
                    ingest_generation: 0,
                    consumed_through_seq: 0,
                },
            )
        })
        .unwrap();

        let former_session_id = db
            .upsert_logical_session_unchecked(
                Agent::Codex,
                "former-context-frontier-root",
                Some("Former owner"),
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap()
            .0;
        db.tx(|tx| {
            set_workstream_frontier_conn(
                tx,
                &WorkstreamSessionFrontier {
                    workstream_id: ws.id.clone(),
                    session_id: former_session_id.clone(),
                    session_context_revision: 0,
                    ingest_generation: 0,
                    consumed_through_seq: 0,
                },
            )
        })
        .unwrap();

        let first_revision = db
            .tx(|tx| {
                commit_session_context_conn(
                    tx,
                    &session_id,
                    &SessionContextFields {
                        summary_current_state: "first summary".into(),
                        ..Default::default()
                    },
                    0,
                    0,
                    0,
                )
            })
            .unwrap();
        db.tx(|tx| {
            set_workstream_frontier_conn(
                tx,
                &WorkstreamSessionFrontier {
                    workstream_id: ws.id.clone(),
                    session_id: session_id.clone(),
                    session_context_revision: first_revision,
                    ingest_generation: 0,
                    consumed_through_seq: 0,
                },
            )
        })
        .unwrap();
        let next_revision = db
            .tx(|tx| {
                commit_session_context_conn(
                    tx,
                    &session_id,
                    &SessionContextFields {
                        summary_current_state: "revised summary".into(),
                        ..Default::default()
                    },
                    first_revision,
                    0,
                    0,
                )
            })
            .unwrap();
        let state = db.get_workstream_context_state(&ws.id).unwrap();
        assert!(workstream_context_view(&db, &ws.id).unwrap().pending);

        commit_workstream_update(
            &db,
            &ws.id,
            &state,
            &[],
            &[ReadOnlySession {
                session_id: session_id.clone(),
                context: db.get_session_context(&session_id).unwrap(),
                generation: 0,
                latest_message_seq: 0,
                invalidate_context: false,
            }],
            crate::sync::ContextUpdateOutput::default(),
            "test".into(),
        )
        .unwrap();

        let frontiers = db.workstream_frontiers(&ws.id).unwrap();
        let frontier = frontiers
            .iter()
            .find(|f| f.session_id == session_id)
            .expect("read-only Session frontier remains");
        assert_eq!(frontier.session_context_revision, next_revision);
        assert!(frontiers
            .iter()
            .any(|f| f.session_id == unchanged_session_id));
        assert!(!frontiers.iter().any(|f| f.session_id == former_session_id));
        assert!(!workstream_context_view(&db, &ws.id).unwrap().pending);
        let _ = std::fs::remove_dir_all(dir);
    }
}
