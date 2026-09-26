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
    self, CliExtractor, PromptInput, PromptRefs, PromptSession, INPUT_LIMIT_BYTES,
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

pub fn update_session(db: &Db, session_id: &str) -> Result<SessionUpdateOutcome> {
    let session = db
        .get_session(session_id)?
        .ok_or_else(|| other("会话不存在"))?;
    if session.trashed_at.is_some() {
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

    let cli = require_cli(db)?;

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

    let (msgs, prompt, refs) = select_message_prefix(build, &all, SESSION_CONTEXT_RESERVE_BYTES)?;
    let upper = from + msgs.len() as i64;

    let raw = cli
        .run(&prompt)
        .map_err(|e| AppError::context(ContextUpdateError::ModelCallFailed(e.to_string())))?;
    let expected_targets = vec![session_id.to_string()];
    let allowed_items: HashSet<String> = HashSet::new();
    let parsed = extractor::parse_update_output(&raw, &refs, "", &expected_targets, &allowed_items)
        .map_err(|e| AppError::context(ContextUpdateError::InvalidOutput(e.to_string())))?;
    let draft = parsed.session_contexts.into_iter().next().ok_or_else(|| {
        AppError::context(ContextUpdateError::InvalidOutput("缺少摘要输出".into()))
    })?;

    let expect_rev = existing.as_ref().map(|c| c.revision).unwrap_or(0);
    let prefix: Vec<String> = msgs.iter().map(|m| m.id.clone()).collect();
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

pub fn update_workstream(db: &Db, workstream_id: &str) -> Result<WorkstreamUpdateOutcome> {
    let ws = db
        .get_workstream(workstream_id)?
        .ok_or_else(|| other("Workstream 不存在"))?;
    if ws.visibility == workstream_visibility::ARCHIVED {
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

    let cli = require_cli(db)?;

    // Budget: reserve output space only for target sessions actually included.
    // Read-only sessions only carry their existing summary.
    let (included, prompt, refs) =
        select_workstream_inputs(&ws, &item_lines, &targets, &read_only)?;

    let expected_targets: Vec<String> = included.iter().map(|t| t.session.id.clone()).collect();

    let raw = cli
        .run(&prompt)
        .map_err(|e| AppError::context(ContextUpdateError::ModelCallFailed(e.to_string())))?;
    let parsed = extractor::parse_update_output(
        &raw,
        &refs,
        workstream_id,
        &expected_targets,
        &allowed_items,
    )
    .map_err(|e| AppError::context(ContextUpdateError::InvalidOutput(e.to_string())))?;

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

fn require_cli(db: &Db) -> Result<CliExtractor> {
    CliExtractor::try_from_settings(db)
        .map_err(|e| AppError::context(ContextUpdateError::AiUnavailable(e.to_string())))?
        .ok_or_else(|| {
            AppError::context(ContextUpdateError::AiUnavailable(
                "Assistant Agent 未配置（none）".into(),
            ))
        })
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
    use crate::domain::{Agent, Workstream};

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
