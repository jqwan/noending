//! Session ingestion: discover members → resolve the logical graph → ensure
//! Sessions + SessionMembers → ingest changed members → Context sync
//!.
//!
//! Invariants (Context Integrity):
//! - raw agent sources are never modified;
//! - already-ingested conversation is never auto-deleted or overwritten, even
//!   when the source is truncated / compacted / replaced: member cursors track
//!   source identity + generation + offset, and re-scans dedup by message
//!   identity;
//! - a member's messages, stats and cursor advance only in the ONE
//!   transaction inside `Db::commit_member_ingest`, which re-checks the
//!   session's lifecycle and the member's attachment first.
//!
//! Graph resolution never lets scan order decide identity: the batch is
//! resolved in two passes — roots first, then children/sides walk their
//! parent chain against the batch AND the database — so a parent that appears
//! later in the same batch still attaches. A child/side that resolves to
//! nothing is an ingestion diagnostic, never a Session.
//!
//! Concurrency: the storage layer owns it (`Db` = one writer + a WAL reader),
//! so ingestion just reads files and calls the store; a (possibly minutes-long,
//! LLM-backed) sync cannot stall UI commands.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use crate::adapters::{AgentAdapter, DiscoveredMember, DiscoveredMemberKind};
use crate::domain::{Agent, Session};
use crate::error::Result;
use crate::storage::Db;
use crate::sync::SyncEngine;

/// A Session's display title: the first of the three sources that has one, in
/// this order — the root's native title, the first real user
/// text, the first visible assistant text. Child / side members can never
/// rename a Logical Session because they never reach this function.
///
/// The rule lives here and only here; adapters report the three sources, they
/// do not decide between them.
pub fn session_title(d: &DiscoveredMember) -> Option<String> {
    d.native_title
        .as_deref()
        .or(d.first_user_text.as_deref())
        .or(d.first_agent_text.as_deref())
        .and_then(crate::adapters::title_from_text)
}

/// One reconcile batch for one agent: the discovered members plus the index
/// resolution walks against.
struct DiscoveryBatch<'a> {
    by_id: HashMap<&'a str, &'a DiscoveredMember>,
}

impl<'a> DiscoveryBatch<'a> {
    fn new(discovered: &'a [DiscoveredMember]) -> Self {
        Self {
            by_id: discovered
                .iter()
                .map(|m| (m.source_member_id.as_str(), m))
                .collect(),
        }
    }
}

/// Depth cap for parent-chain walks: a defensive bound, not a semantic one —
/// real graphs are a few levels deep, and a cyclic or absurdly long source
/// chain must end in "unresolved", never in a hang.
const MAX_CHAIN_DEPTH: usize = 32;

/// The Logical Session a child/side member belongs to, walked through the
/// source's own parent chain. Resolution order per step:
/// 1. the parent sits in THIS batch → keep walking (roots resolve below);
/// 2. the parent is already a member row in the DB → its session is the
///    answer (attached in an earlier pass, possibly under a diagnostic the
///    moment it resolves);
/// 3. otherwise unresolved — the caller records the diagnostic.
fn resolve_logical_session(
    db: &Db,
    agent: Agent,
    batch: &DiscoveryBatch,
    member: &DiscoveredMember,
) -> Result<Option<String>> {
    let mut current: Option<&str> = member.parent_source_member_id.as_deref();
    for _ in 0..MAX_CHAIN_DEPTH {
        let Some(parent_id) = current else {
            break;
        };
        if let Some(parent) = batch.by_id.get(parent_id).copied() {
            if parent.kind.is_logical_root() {
                return Ok(db
                    .find_session_by_root_agent_id(agent, &parent.source_member_id)?
                    .map(|s| s.id));
            }
            current = parent.parent_source_member_id.as_deref();
            continue;
        }
        // Not in this batch: is the parent already a stored member?
        if let Some(stored) = db.find_member_by_source_id(agent, parent_id)? {
            return Ok(Some(stored.session_id));
        }
        break;
    }
    // The strict chain failed. `root_hint` is the adapter's cross-file
    // shortcut — never an authority on its own, but enough when the parent
    // file names the root directly (e.g. a hint that IS a known root).
    if let Some(hint) = member.root_hint.as_deref() {
        if let Some(stored) = db.find_member_by_source_id(agent, hint)? {
            return Ok(Some(stored.session_id));
        }
    }
    Ok(None)
}

/// Ensure the Logical Session row for a Root/ForkRoot discovery:
/// create or refresh keyed by the root's Resume identity, attach its
/// WorkspacePath through the app-wide seam, and resolve fork provenance.
///
/// The Session→WorkspacePath attach is discovery's only piece of workspace
/// work, and it is deliberately not per-message: `workspace_path_id` is
/// re-resolved only when the row is created, when its cwd moved, or when a
/// Session with a cwd has never been attached.
fn ensure_logical_session(
    db: &Db,
    d: &DiscoveredMember,
    attacher: &dyn crate::workspace::WorkspaceAttaching,
) -> Result<(Session, bool)> {
    if let Some(existing) = db.find_session_by_root_agent_id(d.agent, &d.source_member_id)? {
        if existing.is_trashed() {
            return Ok((existing, false));
        }
    }
    let title = session_title(d);
    let raw_path = d.source_path.to_string_lossy().to_string();
    let observed_cwd = d.cwd.as_deref().map(str::trim).filter(|p| !p.is_empty());
    let path_id =
        crate::workspace::session::resolve_session_path(&db.write(), attacher, observed_cwd)?;

    let (session_id, is_new) = db.upsert_logical_root(
        d.agent,
        &d.source_member_id,
        title.as_deref(),
        d.cwd.as_deref(),
        path_id.as_deref(),
        None, // fork provenance is resolved separately, below
        d.started_at.as_deref(),
        d.last_activity_at.as_deref(),
        &d.source_kind,
        &raw_path,
        d.parent_source_member_id.as_deref(),
        &d.metadata,
    )?;

    let stored = db
        .get_session(&session_id)?
        .unwrap_or_else(|| unreachable!());
    if stored.is_trashed() {
        return Ok((stored, false));
    }

    // A topology refresh may have moved the root's cwd: the attachment moves
    // in the one transaction below, so an interrupted pass can never leave the
    // row claiming a path it no longer has.
    if !is_new {
        if let Some(new_path) = path_id {
            if stored.workspace_path_id.as_deref() != Some(new_path.as_str()) {
                db.tx(|tx| {
                    crate::workspace::session::move_session_to_path_conn(tx, &session_id, &new_path)
                })?;
            }
        }
    }

    let stored = db
        .get_session(&session_id)?
        .unwrap_or_else(|| unreachable!());
    Ok((stored, is_new))
}

/// The "source is unchanged since its last ingest" predicate discovery uses to
/// skip re-parsing members whose facts are already fully stored. A source
/// counts as unchanged when its stored cursor identity, size and mtime all
/// still match the source on disk; anything else (new, grown, touched,
/// replaced) fails the check and gets parsed. Only members of TITLED sessions
/// are listed: an untitled row may just predate a title source, and re-parsing
/// its (unchanged) file is exactly what heals it.
fn unchanged_since_cursor(db: &Db) -> Result<impl Fn(&std::path::Path) -> bool> {
    let skipset = db.member_source_skipset()?;
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

/// Record (or re-observe) an unattachable member, and clear the diagnostic of
/// one that resolved. Diagnostics never enter Sessions/Search/Context/
/// lifecycle — they are a Settings page, nothing else.
fn note_unresolved(db: &Db, d: &DiscoveredMember, reason: &str) -> Result<()> {
    db.upsert_ingestion_diagnostic(
        d.agent,
        crate::domain::diagnostic_kind::UNRESOLVED_SESSION_MEMBER,
        Some(&d.source_member_id),
        d.parent_source_member_id.as_deref(),
        Some(&d.source_path.to_string_lossy()),
        reason,
        &serde_json::json!({
            "kind": format!("{:?}", d.kind),
            "root_hint": d.root_hint,
        }),
    )
}

fn resolve_diagnostic(db: &Db, d: &DiscoveredMember) -> Result<()> {
    db.resolve_ingestion_diagnostic(
        d.agent,
        crate::domain::diagnostic_kind::UNRESOLVED_SESSION_MEMBER,
        &d.source_member_id,
    )
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
    let batch = DiscoveryBatch::new(discovered);
    let attacher = crate::workspace::session::workspace_attacher();
    let mut roots_for_intent = Vec::new();
    let mut touched_session_ids = BTreeSet::new();

    for d in discovered {
        if !d.kind.is_logical_root() {
            continue;
        }
        match ensure_logical_session(db, d, attacher.as_ref()) {
            Ok((s, is_new)) if !s.is_trashed() => {
                resolve_diagnostic(db, d)?;
                touched_session_ids.insert(s.id.clone());
                roots_for_intent.push((s, is_new));
            }
            Ok(_) => {}
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
        let Ok(Some(parent_member)) = db.find_member_by_source_id(agent, parent_id) else {
            continue;
        };
        let Ok(Some(fork_session)) = db.find_session_by_root_agent_id(agent, &d.source_member_id)
        else {
            continue;
        };
        if !fork_session.is_trashed()
            && fork_session.forked_from_session_id.is_none()
            && parent_member.session_id != fork_session.id
        {
            let _ = db.tx(|tx| {
                tx.execute(
                    "UPDATE sessions SET forked_from_session_id = ?2
                     WHERE id = ?1 AND forked_from_session_id IS NULL AND trashed_at IS NULL",
                    rusqlite::params![fork_session.id, parent_member.session_id],
                )?;
                Ok(())
            });
        }
    }

    for d in discovered {
        if d.kind.is_logical_root() {
            continue;
        }
        match resolve_logical_session(db, agent, &batch, d)? {
            Some(session_id) => {
                let Some(session) = db.get_session(&session_id)? else {
                    continue;
                };
                if session.is_trashed() {
                    continue;
                }
                let raw_path = d.source_path.to_string_lossy().to_string();
                if db
                    .upsert_active_session_member(
                        &session_id,
                        agent,
                        &d.source_member_id,
                        d.kind.relation(),
                        d.parent_source_member_id.as_deref(),
                        &d.source_kind,
                        &raw_path,
                        d.cwd.as_deref(),
                        d.started_at.as_deref(),
                        d.last_activity_at.as_deref(),
                        &d.metadata,
                    )?
                    .is_some()
                {
                    resolve_diagnostic(db, d)?;
                    touched_session_ids.insert(session_id);
                }
            }
            None => note_unresolved(db, d, "尚未发现其 Root 会话，无法归属到任何逻辑会话")?,
        }
    }
    Ok(ResolvedBatch {
        roots_for_intent,
        touched_session_ids,
    })
}

/// Ingest + sync ONE logical session: read every changed member's delta,
/// commit each atomically, then sync the root conversation. Shared by
/// interactive single-session flows (resume, per-session sync) and background
/// reconcile alike — source parsing needs no database lock, and the storage
/// layer serializes the writes itself.
///
/// With Context Intelligence off this stops after ingestion: messages are
/// stored and indexed, member cursors advance, and nothing is prepared,
/// extracted or committed, so the context frontier stays frozen for a later
/// replay. The same happens for an ownerless session — the sync engine's
/// prepare refuses (ownerless semantics preserved).
pub fn ingest_and_sync_session(
    db: &Db,
    engine: &SyncEngine,
    session: &Session,
) -> Result<(i64, usize)> {
    // re-read the lifecycle state: the caller's struct may predate a
    // concurrent Trash. The authoritative guard lives inside
    // `commit_member_ingest` anyway; this just avoids reading files that
    // cannot commit.
    if !db
        .get_session(&session.id)?
        .map(|s| !s.is_trashed())
        .unwrap_or(false)
    {
        return Ok((0, 0));
    }
    let adapter = crate::adapters::adapter_for(session.agent);
    let members = db.members_for_session(&session.id)?;
    let mut stored_total = 0i64;
    for member in &members {
        // A member that raced a topology change mid-pass simply fails its
        // membership re-check inside the commit and stores nothing.
        let cursor = db.get_member_cursor(&member.id)?;
        let delta = match adapter.read_member_delta(member, &cursor) {
            Ok(d) => d,
            Err(e) => {
                eprintln!(
                    "[ingest] read member {} failed: {}",
                    member.source_member_id, e
                );
                continue;
            }
        };
        let Some(source) = delta.source else {
            continue;
        };
        // The provenance frontier travels with the commit: same transaction,
        // same lifetime as the messages the state covers.
        let stored = db.commit_member_ingest_with_provenance_state(
            &session.id,
            &member.id,
            &delta.messages,
            delta.stats,
            &source,
            delta.next_active_provider,
            delta.next_active_model,
        )?;
        if !stored.is_empty() {
            stored_total += stored.len() as i64;
            if let Err(e) = db.index_new_messages(&stored) {
                eprintln!("[ingest] index_messages failed: {}", e);
            }
        }
    }
    if !crate::settings::context_intelligence_enabled(db)? {
        return Ok((stored_total, 0));
    }
    // Sync everything not yet processed — ownerless sessions were already
    // refused by prepare, so the frontier stays frozen until an Owner exists.
    let processed = db
        .get_context_state(&session.id)?
        .processed_message_sequence;
    let pending = db.get_messages_after(&session.id, processed, 10_000)?;
    let mut applied = 0usize;
    if !pending.is_empty() {
        let to = pending.last().map(|m| m.sequence).unwrap_or(processed);
        let out = engine.run_session_sync(db, session, &pending, processed, to)?;
        applied = out.applied;
    }
    Ok((stored_total, applied))
}

/// Root-only LaunchIntent matching. The gate is "new OR still
/// ownerless", not `is_new` alone: the ROW is already persisted by the time
/// this runs, so `is_new` is true exactly once, and a single transient failure
/// would burn that one chance for good. Re-attempting while the session has no
/// Owner keeps the recovery available; the matcher's own agent and time-window
/// rules keep it from matching a session that never had an intent. A match
/// failure is logged, never fatal.
pub fn finalize_newly_discovered_root(
    db: &Db,
    session: &Session,
    is_new: bool,
    workspace: &crate::launcher::LaunchWorkspace,
) -> Result<()> {
    // The ROW decides, not the caller's copy: discovery hands over a struct
    // that may already be behind a concurrent ownership change or trash.
    let Some(current) = db.get_session(&session.id)? else {
        return Ok(());
    };
    if current.is_trashed() {
        return Ok(());
    }
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
    engine: &SyncEngine,
    workspace: &crate::launcher::LaunchWorkspace,
    batch: ResolvedBatch,
    on_session: &F,
    reingest: bool,
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
        if session.is_trashed() {
            continue;
        }
        processed.insert(id.clone());
        if reingest {
            db.reset_member_cursors(&id)?;
        }
        on_session(&session);
        match ingest_and_sync_session(db, engine, &session) {
            Ok((messages, _)) => total_messages += messages,
            Err(e) => eprintln!(
                "[{}] ingest {} failed: {}",
                if reingest { "reingest" } else { "reconcile" },
                session.id,
                e
            ),
        }
    }
    Ok((total_messages, processed))
}

fn retry_candidate_in_source(
    db: &Db,
    session: &Session,
    source: &crate::domain::IngestSource,
) -> Result<bool> {
    let members = if session.owner_workstream_id.is_none() {
        db.root_member_for_session(&session.id)?
            .into_iter()
            .collect()
    } else {
        db.members_for_session(&session.id)?
    };
    Ok(members
        .iter()
        .any(|member| crate::workspace::is_within(&member.source_path, &source.path)))
}

fn process_retry_sessions<F>(
    db: &Db,
    engine: &SyncEngine,
    workspace: &crate::launcher::LaunchWorkspace,
    on_session: &F,
    scope: Option<&crate::domain::IngestSource>,
    skip: &BTreeSet<String>,
) -> Result<i64>
where
    F: Fn(&Session),
{
    let retry_intents = db.has_pending_launch_intents()?;
    let retry_context = crate::settings::context_intelligence_enabled(db)?;
    let agent = scope.map(|source| source.agent);
    let mut total_messages = 0;
    for candidate in db.reconcile_retry_sessions(retry_intents, retry_context, agent)? {
        if skip.contains(&candidate.id) {
            continue;
        }
        if let Some(source) = scope {
            if !retry_candidate_in_source(db, &candidate, source)? {
                continue;
            }
        }
        if candidate.is_trashed() {
            continue;
        }
        if candidate.owner_workstream_id.is_none() {
            finalize_newly_discovered_root(db, &candidate, false, workspace)?;
        }
        let Some(session) = db.get_session(&candidate.id)? else {
            continue;
        };
        if session.is_trashed() || session.owner_workstream_id.is_none() {
            continue;
        }
        on_session(&session);
        match ingest_and_sync_session(db, engine, &session) {
            Ok((messages, _)) => total_messages += messages,
            Err(e) => eprintln!("[reconcile] retry {} failed: {}", session.id, e),
        }
    }
    Ok(total_messages)
}

/// Full reconcile over every agent's enabled sources: discover members,
/// resolve the logical graph, then ingest + sync each active session.
/// `on_session` observes each session being processed.
///
/// `workspace` is a launch-level fact: a freshly discovered root may claim a
/// pending LaunchIntent, and matching it asks whether that session's cwd is
/// just the shared default workspace, which is not in the DB.
pub fn reconcile_with_engine<F>(
    db: &Db,
    engine: &SyncEngine,
    workspace: &crate::launcher::LaunchWorkspace,
    on_session: &F,
) -> Result<(usize, i64)>
where
    F: Fn(&Session),
{
    let adapters = crate::adapters::all_adapters();
    let unchanged = unchanged_since_cursor(db)?;
    let mut total_discovered = 0usize;
    let mut total_messages = 0i64;
    let mut processed_session_ids = BTreeSet::new();

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
                continue;
            }
        };
        total_discovered += discovered.len();
        let resolved = match resolve_batch(db, adapter.as_ref(), &discovered) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[reconcile] resolve failed: {}", e);
                continue;
            }
        };
        let (messages, processed) =
            process_resolved_batch(db, engine, workspace, resolved, on_session, false)?;
        total_messages += messages;
        processed_session_ids.extend(processed);
    }

    total_messages += process_retry_sessions(
        db,
        engine,
        workspace,
        on_session,
        None,
        &processed_session_ids,
    )?;

    // The one piece of workspace work a skipped Session still owes. It runs
    // AFTER the loop so a path first registered by this very pass is already
    // visible to it, and it never observes a path (the identity is stored),
    // so a pass over frozen files stays as cheap as it looks.
    match crate::workspace::session::attach_sessions_to_registered_paths(db) {
        Ok(0) => {}
        Ok(n) => eprintln!("[reconcile] 补绑 {} 个会话到已注册的 workspace 路径", n),
        Err(e) => eprintln!("[reconcile] workspace 补绑失败: {e}"),
    }
    Ok((total_discovered, total_messages))
}

/// Sync one specific ingest source: discover members under that source's own
/// path only, resolve + ingest + sync them. Takes the
/// [`crate::launcher::LaunchWorkspace`] because first discovery can happen
/// here as easily as in a full reconcile.
pub fn reconcile_source<F>(
    db: &Db,
    engine: &SyncEngine,
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
    let resolved = resolve_batch(db, adapter, &discovered)?;
    let (messages, processed) =
        process_resolved_batch(db, engine, workspace, resolved, on_session, false)?;
    total_messages += messages;
    total_messages +=
        process_retry_sessions(db, engine, workspace, on_session, Some(source), &processed)?;
    Ok((discovered_count, total_messages))
}

/// Re-ingest one source: rewind the member cursors of every session found
/// under its path and re-scan from scratch. THE MESSAGE STORE IS
/// NEVER DELETED — message ids and every provenance ref stay valid, because
/// unchanged content dedups by identity and only genuinely new/changed source
/// content appends. Stats snapshots replace on the re-scan; Context items,
/// Owner and audit history are untouched.
///
/// Deliberately passes the all-false "unchanged" predicate: a re-ingest WANTS
/// to re-read every source, cursor match or not.
pub fn reingest_source<F>(
    db: &Db,
    engine: &SyncEngine,
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
    let resolved = resolve_batch(db, adapter, &discovered)?;
    total_messages += process_resolved_batch(db, engine, workspace, resolved, on_session, true)?.0;
    Ok((discovered_count, total_messages))
}

/// Sessions with pending (un-ingested) activity, used by "sync stale" flows.
///
/// Scoped to the Sessions that OWN this Workstream — a Session
/// belongs to at most one, so it can never be synced twice under this rule.
/// "Stale" = any member's source was modified after the Session's last
/// recorded activity, so there may be a delta the cursors have not seen.
/// Read-only observation; nothing is written here.
pub fn stale_sessions(db: &Db, workstream_id: &str) -> Result<Vec<Session>> {
    let mut out = Vec::new();
    for s in db.sessions_for_workstream(workstream_id)? {
        let members = db.members_for_session(&s.id)?;
        let last_used = s
            .last_activity_at
            .as_deref()
            .or(s.started_at.as_deref())
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|t| t.with_timezone(&chrono::Utc))
            .unwrap_or_else(chrono::Utc::now);
        let stale = members.iter().any(|m| {
            std::fs::metadata(&m.source_path)
                .ok()
                .and_then(|meta| meta.modified().ok())
                .map(|modified| {
                    let m: chrono::DateTime<chrono::Utc> = modified.into();
                    m > last_used
                })
                .unwrap_or(false)
        });
        if stale {
            out.push(s);
        }
    }
    Ok(out)
}
