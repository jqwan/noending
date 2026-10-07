//! Session ingestion: discover sources → ensure Sessions → ingest changed
//! sources.
//!
//! Ingestion is the AUTOMATIC half of the pipeline and it NEVER calls AI: it
//! writes facts only (messages, the source cursor, the current-message
//! projection and the fact generation). Context is written only by an explicit
//! user action elsewhere.
//!
//! Invariants:
//! - raw agent sources are never modified;
//! - already-ingested conversation is never auto-deleted or overwritten, even
//!   when the source is truncated / compacted / replaced: the session cursor
//!   tracks source identity + generation + offset, and re-scans dedup by
//!   message identity;
//! - messages, the cursor and the activity stamps advance only in the ONE
//!   transaction inside `Db::commit_ingest`, which re-checks the session's
//!   lifecycle first.
//!
//! A session IS its root source. Child/side sources an Agent spawns alongside
//! (subagent transcripts, sidechains, derived pages) are recognized at
//! discovery and skipped silently — recognition is what keeps e.g. codex
//! subagent rollouts from becoming fake sessions. Concurrency is the storage
//! layer's (`Db` = one writer + a WAL reader), so ingestion just reads files
//! and calls the store.

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::adapters::{AgentAdapter, DiscoveredMember, DiscoveredMemberKind};
use crate::domain::{Session, SourceAvailability};
use crate::error::{other, Result};
use crate::storage::Db;

/// A Session's display title, in two tiers: the root's NATIVE title (the Agent
/// app's own name — it may appear or be renamed as the conversation moves on,
/// so each ingest writes it through) and a text-derived FALLBACK (the first
/// real user text, then the first visible assistant text — fixed at the
/// conversation's beginning, so it only ever fills an EMPTY slot). Child /
/// side members can never rename a Logical Session because they never reach
/// this function, and the split lives here and only here: adapters report the
/// sources, they do not decide between them.
pub fn session_title_sources(d: &DiscoveredMember) -> (Option<String>, Option<String>) {
    let native = d
        .native_title
        .as_deref()
        .and_then(crate::adapters::title_from_text);
    let fallback = d
        .first_user_text
        .as_deref()
        .or(d.first_agent_text.as_deref())
        .and_then(crate::adapters::title_from_text);
    (native, fallback)
}

/// Ensure the Logical Session row for a Root/ForkRoot discovery: create or refresh
/// keyed by the root's Resume identity, and attach its WorkspacePath through the
/// app-wide seam.
///
/// The attach is discovery's only piece of workspace work, and deliberately not
/// per-message: `workspace_path_id` is re-resolved only when the row is created or
/// its cwd moved.
fn ensure_logical_session(
    db: &Db,
    d: &DiscoveredMember,
    attacher: &dyn crate::workspace::WorkspaceAttaching,
) -> Result<(Session, bool)> {
    let (native_title, fallback_title) = session_title_sources(d);
    let raw_path = d.source_path.to_string_lossy().to_string();
    let observed_cwd = d.cwd.as_deref().map(str::trim).filter(|p| !p.is_empty());
    let path_id =
        crate::workspace::session::resolve_session_path(&db.write(), attacher, observed_cwd)?;

    let (session_id, is_new) = db.upsert_logical_session(
        d.agent,
        &d.source_member_id,
        native_title.as_deref(),
        fallback_title.as_deref(),
        d.cwd.as_deref(),
        path_id.as_deref(),
        None, // fork provenance is resolved separately, below
        d.started_at.as_deref(),
        d.last_activity_at.as_deref(),
        &d.source_kind,
        &raw_path,
        &d.metadata,
    )?;

    let mut stored = db
        .get_session(&session_id)?
        .unwrap_or_else(|| unreachable!());

    // A topology refresh may have moved the root's cwd: the attachment moves
    // in the one transaction below, so an interrupted pass can never leave the
    // row claiming a path it no longer has.
    if !is_new {
        if let Some(new_path) = path_id {
            if stored.workspace_path_id.as_deref() != Some(new_path.as_str()) {
                db.tx(|tx| {
                    crate::workspace::session::move_session_to_path_conn(tx, &session_id, &new_path)
                })?;
                stored = db
                    .get_session(&session_id)?
                    .unwrap_or_else(|| unreachable!());
            }
        }
    }
    Ok((stored, is_new))
}

/// The "source is unchanged since its last ingest" predicate discovery uses to
/// skip re-parsing sources whose facts are already stored. A source is unchanged
/// when its stored cursor identity, size and mtime all still match the file on
/// disk; anything else (new, grown, touched, replaced) gets parsed. Only TITLED
/// sessions are listed: an untitled row may just predate a title source, and
/// re-parsing its unchanged file is what heals it.
fn unchanged_since_cursor(db: &Db) -> Result<impl Fn(&std::path::Path) -> bool> {
    let skipset = db.session_source_skipset()?;
    Ok(move |path: &std::path::Path| {
        let Some((identity, size, mtime)) = skipset.get(&path.to_string_lossy().to_string()) else {
            return false;
        };
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(_) => return false,
        };
        if *size != meta.len() as i64 {
            return false;
        }
        let Some(stored) = mtime else { return false };
        let same_mtime = match crate::adapters::mtime_secs(&meta) {
            // Same 1e-6 tolerance the delta reader uses: a touch without a
            // byte change must not force a full re-parse.
            Some(observed) => (observed - stored).abs() <= 1e-6,
            None => false,
        };
        same_mtime && crate::adapters::file_identity(path) == *identity
    })
}
/// One full discovery→resolve→attach pass for one agent's batch. Roots
/// first, children/sides second — so a parent discovered later in the same
/// batch is still found, and scan order never decides identity.
struct ResolvedBatch {
    roots_for_intent: Vec<(Session, bool)>,
    touched_session_ids: BTreeSet<String>,
}

fn resolve_batch(
    db: &Db,
    adapter: &dyn AgentAdapter,
    discovered: &[DiscoveredMember],
) -> Result<ResolvedBatch> {
    let agent = adapter.agent();
    let attacher = crate::workspace::session::workspace_attacher();
    let mut roots_for_intent = Vec::new();
    let mut touched_session_ids = BTreeSet::new();

    for d in discovered {
        if !d.kind.is_logical_root() {
            continue;
        }
        match ensure_logical_session(db, d, attacher.as_ref()) {
            Ok((s, is_new)) => {
                touched_session_ids.insert(s.id.clone());
                roots_for_intent.push((s, is_new));
            }
            Err(e) => eprintln!("[ingest] root {} failed: {}", d.source_member_id, e),
        }
    }

    for d in discovered {
        if d.kind != DiscoveredMemberKind::ForkRoot {
            continue;
        }
        let Some(parent_id) = d.parent_source_member_id.as_deref() else {
            continue;
        };
        let Ok(Some(parent_session)) = db.find_session_by_root_agent_id(agent, parent_id) else {
            continue;
        };
        let Ok(Some(fork_session)) = db.find_session_by_root_agent_id(agent, &d.source_member_id)
        else {
            continue;
        };
        if fork_session.forked_from_session_id.is_none() && parent_session.id != fork_session.id {
            let _ = db.tx(|tx| {
                tx.execute(
                    "UPDATE sessions SET forked_from_session_id = ?2
                     WHERE id = ?1 AND forked_from_session_id IS NULL",
                    rusqlite::params![fork_session.id, parent_session.id],
                )?;
                Ok(())
            });
        }
    }

    // 会话 = 根会话（2026-09-29 起）：内部执行单元（子代理转录、sidechain、
    // 派生页）被识别后静默跳过——识别必须保留，否则 codex 的子代理 rollout
    // 会被误当成独立逻辑会话。
    Ok(ResolvedBatch {
        roots_for_intent,
        touched_session_ids,
    })
}

/// Ingest ONE logical session: read every changed member's delta and commit each
/// atomically. Shared by interactive single-session flows (resume) and the
/// reconcile pass alike — source parsing needs no database lock, and the storage
/// layer serializes the writes itself.
///
/// Pure ingestion: writes messages, cursors, the current-message
/// projection and the fact generation, and NOTHING else. No Context, no AI.
/// Returns the number of newly stored messages.
pub fn ingest_session(db: &Db, session: &Session) -> Result<i64> {
    // A purge that raced this pass leaves nothing to attach the facts to.
    if db.get_session(&session.id)?.is_none() {
        return Ok(0);
    }
    let adapter = crate::adapters::adapter_for(session.agent);
    let cursor = session.source_cursor();
    // The source may have vanished since discovery; keep the facts and cursor
    // intact so a later discovery can resume it if the file returns.
    if matches!(
        adapter.inspect_session_source(session),
        Ok(SourceAvailability::Missing)
    ) {
        return Ok(0);
    }
    let delta = match adapter.read_session_delta(session, &cursor) {
        Ok(d) => d,
        Err(e) => {
            // The source can disappear between inspection and reading;
            // adapters wrap read errors differently, so confirm the current
            // source verdict instead of matching error text.
            if matches!(
                adapter.inspect_session_source(session),
                Ok(SourceAvailability::Missing)
            ) {
                return Ok(0);
            }
            return Err(other(format!(
                "session {} source read failed: {e}",
                session.root_agent_session_id
            )));
        }
    };
    let Some(source) = delta.source else {
        return Ok(0);
    };
    let mut failures = Vec::new();
    if !delta.complete_snapshot {
        failures.push("source contains an incomplete or invalid frame".to_string());
    }
    // The bytes frontier travels with the commit: same transaction, same
    // lifetime as the messages it covers. `complete_snapshot` tells the
    // projection maintenance whether a full re-scan is whole.
    let stored = db.commit_ingest_snapshot(
        &session.id,
        &delta.messages,
        &source,
        delta.complete_snapshot,
    )?;
    if !failures.is_empty() {
        return Err(other(format!(
            "Session {} ingestion incomplete: {}",
            session.id,
            failures.join("; ")
        )));
    }
    Ok(stored.len() as i64)
}

/// Root-only LaunchIntent matching. The gate is "new OR still ownerless", not
/// `is_new` alone: the ROW is already persisted by the time this runs, so
/// `is_new` is true exactly once and a single transient failure would burn that
/// one chance for good. The matcher's own agent and time-window rules keep it from
/// matching a session that never had an intent. A match failure is logged, never
/// fatal.
pub fn finalize_newly_discovered_root(
    db: &Db,
    session: &Session,
    is_new: bool,
    workspace: &crate::launcher::LaunchWorkspace,
) -> Result<()> {
    // The ROW decides, not the caller's copy: it may be behind a concurrent
    // ownership change.
    let Some(current) = db.get_session(&session.id)? else {
        return Ok(());
    };
    if !is_new && current.owner_workstream_id.is_some() {
        return Ok(());
    }
    // Nothing waiting for a Session: skip the per-Session matcher entirely.
    // On a first pass over a large history this is the common answer.
    if !db.has_pending_launch_intents()? {
        return Ok(());
    }
    match crate::launcher::try_match_launch_intents_in(db, &current, workspace) {
        Ok(true) => eprintln!("[ingest] launch intent matched to session {}", current.id),
        Ok(false) => {}
        Err(e) => eprintln!("[ingest] intent match failed for {}: {}", current.id, e),
    }
    Ok(())
}

fn process_resolved_batch<F>(
    db: &Db,
    workspace: &crate::launcher::LaunchWorkspace,
    batch: ResolvedBatch,
    on_session: &F,
    reingest: bool,
    failures: &mut Vec<String>,
) -> Result<(i64, BTreeSet<String>)>
where
    F: Fn(&Session),
{
    for (session, is_new) in &batch.roots_for_intent {
        finalize_newly_discovered_root(db, session, *is_new, workspace)?;
    }
    let mut total_messages = 0;
    let mut processed = BTreeSet::new();
    for id in batch.touched_session_ids {
        let Some(session) = db.get_session(&id)? else {
            continue;
        };
        processed.insert(id.clone());
        if reingest {
            db.rewind_source_cursor(&id)?;
        }
        on_session(&session);
        match ingest_session(db, &session) {
            Ok(messages) => total_messages += messages,
            Err(e) => {
                eprintln!(
                    "[{}] ingest {} failed: {}",
                    if reingest { "reingest" } else { "reconcile" },
                    session.id,
                    e
                );
                failures.push(format!("Session {} ingest failed: {e}", session.id));
            }
        }
    }
    Ok((total_messages, processed))
}

fn retry_candidate_in_source(
    _db: &Db,
    session: &Session,
    source: &crate::domain::IngestSource,
) -> Result<bool> {
    Ok(session.owner_workstream_id.is_none()
        && crate::workspace::is_within(&session.source_path, &source.path))
}

/// Sessions with no Owner that may still claim a pending LaunchIntent, so a
/// later pass can (re)attempt the match and ingest them. Nothing is written
/// for Context here.
fn process_retry_sessions<F>(
    db: &Db,
    workspace: &crate::launcher::LaunchWorkspace,
    on_session: &F,
    scope: Option<&crate::domain::IngestSource>,
    skip: &BTreeSet<String>,
    failures: &mut Vec<String>,
) -> Result<i64>
where
    F: Fn(&Session),
{
    let retry_intents = db.has_pending_launch_intents()?;
    let agent = scope.map(|source| source.agent);
    let mut total_messages = 0;
    for candidate in db.reconcile_retry_sessions(agent)? {
        if skip.contains(&candidate.id) {
            continue;
        }
        if let Some(source) = scope {
            if !retry_candidate_in_source(db, &candidate, source)? {
                continue;
            }
        }
        if candidate.owner_workstream_id.is_none() && retry_intents {
            finalize_newly_discovered_root(db, &candidate, false, workspace)?;
        }
        let Some(session) = db.get_session(&candidate.id)? else {
            continue;
        };
        on_session(&session);
        match ingest_session(db, &session) {
            Ok(messages) => total_messages += messages,
            Err(e) => {
                eprintln!("[reconcile] retry {} failed: {}", session.id, e);
                failures.push(format!("retry Session {} ingest failed: {e}", session.id));
            }
        }
    }
    Ok(total_messages)
}

/// Full reconcile over every agent's enabled sources: discover members,
/// resolve the logical graph, then ingest each active session — facts only,
/// no AI. `on_session` observes each session being processed.
///
/// `workspace` is a launch-level fact: a freshly discovered root may claim a
/// pending LaunchIntent, and matching it asks whether that session's cwd is
/// just the shared default workspace, which is not in the DB.
pub fn reconcile_all<F>(
    db: &Db,
    workspace: &crate::launcher::LaunchWorkspace,
    on_session: &F,
) -> Result<(usize, i64)>
where
    F: Fn(&Session),
{
    let report = reconcile_all_report(db, workspace, on_session)?;
    if !report.failures.is_empty() {
        return Err(other(format!(
            "部分来源摄入失败：{}",
            report.failures.join("; ")
        )));
    }
    Ok((report.discovered, report.messages))
}

/// Detailed result for the background coordinator. A pass is considered
/// successful for freshness only when every enabled source and Session was
/// discovered, resolved, and ingested without a partial failure.
pub struct ReconcileReport {
    pub discovered: usize,
    pub messages: i64,
    pub failures: Vec<String>,
}

pub fn reconcile_all_report<F>(
    db: &Db,
    workspace: &crate::launcher::LaunchWorkspace,
    on_session: &F,
) -> Result<ReconcileReport>
where
    F: Fn(&Session),
{
    let adapters = crate::adapters::all_adapters();
    let unchanged = unchanged_since_cursor(db)?;
    let mut total_discovered = 0usize;
    let mut total_messages = 0i64;
    let mut processed_session_ids = BTreeSet::new();
    let mut failures = Vec::new();

    // housekeeping: expire launch intents that never got a session
    let _ = crate::launcher::expire_stale_launch_intents(db);

    for adapter in &adapters {
        let roots: Vec<String> = db.enabled_roots(adapter.agent())?;
        if roots.is_empty() {
            continue;
        }
        let roots: Vec<PathBuf> = roots.into_iter().map(PathBuf::from).collect();
        let discovered = match adapter.discover_members_in(&roots, &unchanged) {
            Ok(v) => v,
            Err(e) => {
                eprintln!(
                    "[reconcile] {} discovery failed: {}",
                    adapter.agent().display_name(),
                    e
                );
                failures.push(format!(
                    "{} discovery failed: {e}",
                    adapter.agent().display_name()
                ));
                continue;
            }
        };
        total_discovered += discovered.len();
        let resolved = match resolve_batch(db, adapter.as_ref(), &discovered) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[reconcile] resolve failed: {}", e);
                failures.push(format!(
                    "{} resolve failed: {e}",
                    adapter.agent().display_name()
                ));
                continue;
            }
        };
        let (messages, processed) =
            process_resolved_batch(db, workspace, resolved, on_session, false, &mut failures)?;
        total_messages += messages;
        processed_session_ids.extend(processed);
    }

    total_messages += process_retry_sessions(
        db,
        workspace,
        on_session,
        None,
        &processed_session_ids,
        &mut failures,
    )?;

    // The one piece of workspace work a skipped Session still owes. It runs
    // AFTER the loop so a path first registered by this very pass is already
    // visible to it, and it never observes a path (the identity is stored),
    // so a pass over frozen files stays as cheap as it looks.
    match crate::workspace::session::attach_sessions_to_registered_paths(db) {
        Ok(0) => {}
        Ok(n) => eprintln!("[reconcile] 补绑 {} 个会话到已注册的 workspace 路径", n),
        Err(e) => {
            eprintln!("[reconcile] workspace 补绑失败: {e}");
            failures.push(format!("workspace path attach failed: {e}"));
        }
    }
    Ok(ReconcileReport {
        discovered: total_discovered,
        messages: total_messages,
        failures,
    })
}

/// Ingest one specific ingest source: discover members under that source's own
/// path only, resolve + ingest them. Takes the
/// [`crate::launcher::LaunchWorkspace`] because first discovery can happen
/// here as easily as in a full reconcile.
pub fn reconcile_source<F>(
    db: &Db,
    source: &crate::domain::IngestSource,
    workspace: &crate::launcher::LaunchWorkspace,
    on_session: &F,
) -> Result<(usize, i64)>
where
    F: Fn(&Session),
{
    let adapter = crate::adapters::adapter_for(source.agent);
    let unchanged = unchanged_since_cursor(db)?;
    let roots = vec![PathBuf::from(&source.path)];
    let discovered = adapter.discover_members_in(&roots, &unchanged)?;
    let discovered_count = discovered.len();
    let mut total_messages = 0i64;
    let mut failures = Vec::new();
    let resolved = resolve_batch(db, adapter, &discovered)?;
    let (messages, processed) =
        process_resolved_batch(db, workspace, resolved, on_session, false, &mut failures)?;
    total_messages += messages;
    total_messages += process_retry_sessions(
        db,
        workspace,
        on_session,
        Some(source),
        &processed,
        &mut failures,
    )?;
    if !failures.is_empty() {
        return Err(other(format!("部分来源摄入失败：{}", failures.join("; "))));
    }
    Ok((discovered_count, total_messages))
}

/// Re-ingest one source: rewind the cursors of every session found under
/// its path and re-scan from scratch. THE MESSAGE STORE IS NEVER DELETED —
/// message ids and every provenance ref stay valid, because unchanged content
/// dedups by identity and only new/changed content appends. Context items,
/// Owner and audit history are untouched.
///
/// A re-scan that finds the conversation rewritten, truncated or reordered raises
/// the Session's fact generation (via the projection), which makes its Context
/// pending again — without touching any Context itself.
///
/// Deliberately passes the all-false "unchanged" predicate: a re-ingest WANTS to
/// re-read every source, cursor match or not.
pub fn reingest_source<F>(
    db: &Db,
    source: &crate::domain::IngestSource,
    workspace: &crate::launcher::LaunchWorkspace,
    on_session: &F,
) -> Result<(usize, i64)>
where
    F: Fn(&Session),
{
    let adapter = crate::adapters::adapter_for(source.agent);
    let roots = vec![PathBuf::from(&source.path)];
    let discovered = adapter.discover_members_in(&roots, &|_| false)?;
    let discovered_count = discovered.len();
    let mut total_messages = 0i64;
    let mut failures = Vec::new();
    let resolved = resolve_batch(db, adapter, &discovered)?;
    total_messages +=
        process_resolved_batch(db, workspace, resolved, on_session, true, &mut failures)?.0;
    if !failures.is_empty() {
        return Err(other(format!("部分来源重扫失败：{}", failures.join("; "))));
    }
    Ok((discovered_count, total_messages))
}

/// Refresh one Session's already-registered members incrementally. Used by the
/// Resume-success trigger: only this Session is read, never a full Source scan.
pub fn refresh_session(db: &Db, session_id: &str) -> Result<i64> {
    let Some(session) = db.get_session(session_id)? else {
        return Ok(0);
    };
    ingest_session(db, &session)
}
