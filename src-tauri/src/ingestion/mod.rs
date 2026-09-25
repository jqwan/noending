//! Session ingestion: discovery → upsert → incremental read → cursor.
//!
//! Invariants (Context Integrity):
//! - raw agent files are never modified;
//! - already-ingested history is never auto-deleted or overwritten, even
//!   when the source file is truncated / compacted / replaced: the source
//!   cursor tracks file identity + generation + byte offset, and re-scans
//!   dedup by content identity;
//! - the read cursor only advances in the same transaction that durably
//!   stored the events.
//!
//! Concurrency: the storage layer owns it (`Db` = one writer + a WAL reader),
//! so ingestion just reads files and calls the store; a (possibly minutes-long,
//! LLM-backed) sync cannot stall UI commands. Discovery skips transcripts
//! whose stored cursor still matches the file on disk, so a reconcile pass
//! costs O(changed files), not O(all history).

use std::path::PathBuf;

use crate::adapters::AgentAdapter;
use crate::domain::{IngestSource, Session};
use crate::error::Result;
use crate::storage::{new_id, now, Db};
use crate::sync::SyncEngine;

/// Ingest one session incrementally (caller holds the DB lock).
/// Returns number of newly stored events.
pub fn ingest_session(db: &Db, adapter: &dyn AgentAdapter, session: &mut Session) -> Result<i64> {
    let stored = ingest_delta(db, adapter, session)?;
    if !stored.is_empty() {
        session.last_activity_at = Some(now());
    }
    Ok(stored.len() as i64)
}

/// Read the file delta and durably store it (content-deduped, read cursor
/// advanced atomically). Returns the newly stored events.
pub(crate) fn ingest_delta(
    db: &Db,
    adapter: &dyn AgentAdapter,
    session: &Session,
) -> Result<Vec<crate::domain::SessionEvent>> {
    let cursor = db.get_source_cursor(&session.id)?;
    let delta = adapter.read_delta(session, &cursor)?;
    let source = delta
        .source
        .clone()
        .ok_or_else(|| crate::error::other("adapter returned no source state"))?;
    let stored = db.append_source_events(&session.id, &delta.events, &source, &session.raw_path)?;
    if !stored.is_empty() {
        if let Err(e) = db.index_new_events(&stored) {
            eprintln!("[ingest] index_events failed: {}", e);
        }
    }
    Ok(stored)
}

/// Ingest + sync one session: read the file delta, then extract and commit
/// the pending events. Shared by interactive single-session flows (resume,
/// per-session sync) and background reconcile alike — file parsing needs no
/// database lock, and the storage layer serializes the writes itself.
///
/// With Context Intelligence off this stops after ingestion: events are stored
/// and indexed, the read cursor advances, and nothing is prepared, extracted
/// or committed, so `processed_sequence` stays frozen for a later replay.
pub fn ingest_and_sync_session(
    db: &Db,
    engine: &SyncEngine,
    session: &Session,
) -> Result<(i64, usize)> {
    // §9 — re-read the lifecycle state: the caller's struct may predate a
    // concurrent Trash. The authoritative guard lives inside
    // `append_source_events` anyway; this just avoids preparing work that
    // cannot commit.
    if !db
        .get_session(&session.id)?
        .map(|s| !s.is_trashed())
        .unwrap_or(false)
    {
        return Ok((0, 0));
    }
    let adapter = crate::adapters::adapter_for(session.agent);
    let stored = ingest_delta(db, adapter, session)?;
    if !crate::settings::context_intelligence_enabled(db)? {
        return Ok((stored.len() as i64, 0));
    }
    let processed = db.get_processed_sequence(&session.id)?;
    let pending = db.get_events(&session.id, Some(processed), 10_000)?;
    let mut applied = 0usize;
    if !pending.is_empty() {
        let to = pending.last().map(|e| e.sequence).unwrap_or(processed);
        let out = engine.run_session_sync(db, session, &pending, processed, to)?;
        applied = out.applied;
    }
    Ok((stored.len() as i64, applied))
}

/// Ensure a session row exists for a discovered session, attaching its
/// WorkspacePath through the app-wide seam (see
/// [`crate::workspace::session::workspace_attacher`]).
pub fn ensure_session_row(
    db: &Db,
    d: &crate::adapters::DiscoveredSession,
) -> Result<(Session, bool)> {
    let attacher = crate::workspace::session::workspace_attacher();
    ensure_session_row_with(db, d, attacher.as_ref())
}

/// A Session's display title: the first of the three sources that has one, in
/// this order (§37.15) — the transcript's own title, the first user text, the
/// first agent text. The rule lives here and only here; adapters report the
/// three sources, they do not decide between them.
///
/// Public because a one-off re-derive has to produce exactly what a fresh
/// discovery would: two implementations of this order would be two rules.
pub fn session_title(d: &crate::adapters::DiscoveredSession) -> Option<String> {
    d.native_title
        .as_deref()
        .or(d.first_user_text.as_deref())
        .or(d.first_agent_text.as_deref())
        .and_then(crate::adapters::title_from_text)
}

/// Ensure a session row exists for a discovered session, resolving its cwd
/// against the injected [`WorkspaceAttaching`] seam.
///
/// Returns the internal row and whether this session is NEW to us — new
/// sessions are the only ones allowed to claim a pending LaunchIntent.
///
/// The Session→WorkspacePath attach is discovery's only piece of workspace
/// work, and it is deliberately not per-event: `workspace_path_id` is re-resolved
/// only when the row is created, when its cwd moved, or when a Session with a cwd
/// has never been attached. Ordinary ingestion performs none of it (§1.11).
pub fn ensure_session_row_with(
    db: &Db,
    d: &crate::adapters::DiscoveredSession,
    attacher: &dyn crate::workspace::WorkspaceAttaching,
) -> Result<(Session, bool)> {
    let title = session_title(d);
    let existing = db.find_session_by_agent_id(d.agent, &d.agent_session_id)?;
    let raw_path = d.path.to_string_lossy().to_string();
    if let Some(s) = existing {
        // §8 — a trashed Session is inactive: discovery may observe it, but it
        // MUST NOT refresh metadata, re-attach paths, ingest or sync it. The
        // row is returned unchanged so the caller can skip it; after a
        // Restore, the next reconcile continues from the untouched cursor.
        if s.is_trashed() {
            return Ok((s, false));
        }
        // Discovery just read the transcript, so it is the source of truth
        // for source-derived fields; refresh the row when it learned
        // something new (e.g. cwd read from file content after an older
        // ingestion stored a different value). id stays as-is, and so does
        // project_id — it is derived from the WorkspacePath inside the
        // statement, never taken from here (§42.3-M3).
        let dirty = title.is_some() && s.title.is_none()
            || d.cwd.is_some() && s.cwd != d.cwd
            || s.raw_path != raw_path
            || d.started_at.is_some() && s.started_at.is_none()
            || d.last_activity_at.is_some() && s.last_activity_at != d.last_activity_at;
        // §42.3-M2 — cwd is authoritative, so the WorkspacePath it names follows
        // it. Re-resolve only when the cwd moved or the row has never been
        // attached: a steady-state scan performs no workspace work at all, so
        // the seam (which may create a WorkspacePath row) is not called per
        // session per pass (§1.11).
        let observed_cwd = d.cwd.as_deref().map(str::trim).filter(|p| !p.is_empty());
        let needs_path =
            observed_cwd.is_some() && (d.cwd != s.cwd || s.workspace_path_id.is_none());
        let path_id = if needs_path {
            crate::workspace::session::resolve_session_path(&db.write(), attacher, observed_cwd)?
        } else {
            None
        };
        let moved: Option<String> = match (&path_id, &s.workspace_path_id) {
            (Some(p), current) if Some(p.as_str()) != current.as_deref() => Some(p.clone()),
            _ => None,
        };
        if dirty || moved.is_some() {
            let mut updated = s;
            if updated.title.is_none() {
                updated.title = title;
            }
            if d.cwd.is_some() {
                updated.cwd = d.cwd.clone();
            }
            updated.raw_path = raw_path;
            if updated.started_at.is_none() {
                updated.started_at = d.started_at.clone();
            }
            updated.last_activity_at = d.last_activity_at.clone().or(updated.last_activity_at);
            // The path itself is deliberately NOT handed to upsert here: the
            // attachment moves in the one transaction below, so an interrupted
            // pass can never leave the row claiming a path it no longer has —
            // and `moved` stays true until it is done, which makes the retry
            // converge instead of double-applying.
            db.upsert_session(&updated)?;
            if let Some(new_path) = moved {
                let session_id = updated.id.clone();
                db.tx(|tx| {
                    crate::workspace::session::move_session_to_path_conn(tx, &session_id, &new_path)
                })?;
            }
            // Re-read: the cached Project is whatever the path says it is now,
            // which is not necessarily what the caller handed us.
            let stored = db.get_session(&updated.id)?.unwrap_or(updated);
            return Ok((stored, false));
        }
        return Ok((s, false));
    }
    let s = Session {
        id: new_id(),
        agent: d.agent,
        agent_session_id: d.agent_session_id.clone(),
        title,
        cwd: d.cwd.clone(),
        // §19-1/2 — resolved from the observed cwd by the seam. A Session
        // discovered with no cwd keeps None and gets no fabricated path, and
        // neither does one whose cwd resolves to nothing (§5.5, §7.2).
        workspace_path_id: crate::workspace::session::resolve_session_path(
            &db.write(),
            attacher,
            d.cwd.as_deref(),
        )?,
        // Never taken from the caller: `upsert_session` derives it from that
        // WorkspacePath in the same statement (§42.3-M3).
        project_id: None,
        // Discovery never assigns semantic ownership (方案 §49): only an
        // explicit user action or a matched LaunchIntent sets this.
        owner_workstream_id: None,
        raw_path,
        parent_agent_session_id: d.parent_agent_session_id.clone(),
        started_at: d.started_at.clone(),
        last_activity_at: d.last_activity_at.clone(),
        // A freshly discovered Session always starts Normal.
        trashed_at: None,
    };
    db.upsert_session(&s)?;
    let stored = db.get_session(&s.id)?.unwrap_or(s);
    Ok((stored, true))
}

/// The "file is unchanged since its last ingest" predicate discovery uses to
/// skip re-parsing transcripts whose facts are already fully stored. A file
/// counts as unchanged when its stored cursor identity, size and mtime all
/// still match the file on disk; anything else (new file, grown, touched,
/// replaced) fails the check and gets parsed.
fn unchanged_since_cursor(db: &Db) -> Result<impl Fn(&std::path::Path) -> bool> {
    let skipset = db.discovery_skipset()?;
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

/// Full reconcile over every agent's enabled sources. Discovery skips
/// transcripts the stored cursors say are unchanged; the store itself
/// serializes writes. `on_session` observes each session being processed.
///
/// `workspace` is §13's tier-3 fact, needed because a freshly discovered
/// Session may claim a pending LaunchIntent: matching it asks whether that
/// Session's cwd is just the shared default workspace, which is not in the DB.
/// What a Session owes the workspace the first time it is discovered, and the
/// retry window after that.
///
/// A Session acquires its Owner in exactly two ways: a user action, or the
/// LaunchIntent that started it (方案 §15.1/§21). Matching is the only one
/// ingestion can do, and every discovery door (full reconcile, single source,
/// re-ingest) has to run it — a source-scoped sync that discovers the Session
/// first must offer the same chance, or the later full reconcile sees a known
/// Session and the intent stays pending forever.
///
/// The gate is therefore "new OR still ownerless", not `is_new` alone: the
/// Session ROW is already persisted by the time this runs, so `is_new` is true
/// exactly once, and a single transient failure would burn that one chance for
/// good. Re-attempting while the Session has no Owner keeps the recovery
/// available; the matcher's own agent and time-window rules are what keep it
/// from matching a Session that never had an intent.
///
/// A match failure is logged, never fatal: one bad intent must not abort the
/// discovery pass that is ingesting everything else.
pub fn finalize_newly_discovered_session(
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
    let mut total_events = 0i64;

    // housekeeping: expire launch intents that never got a session
    let _ = crate::launcher::expire_stale_launch_intents(db);

    for adapter in &adapters {
        let roots: Vec<String> = db.enabled_roots(adapter.agent())?;
        if roots.is_empty() {
            continue;
        }
        let roots: Vec<PathBuf> = roots.into_iter().map(PathBuf::from).collect();
        let discovered = match adapter.discover_sessions_in(&roots, &unchanged) {
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
        for d in discovered {
            let (s, is_new) = match ensure_session_row(db, &d) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[reconcile] ensure row failed: {}", e);
                    continue;
                }
            };
            // A brand-new external session may claim a pending LaunchIntent
            // (crash recovery included); a still-ownerless one gets the retry.
            //
            // §42.2-E11 — no name-substring Project evidence is recorded any
            // more. A Project is derived from the Session's WorkspacePath
            // (§1.10), which `ensure_session_row` has just resolved, so this
            // hot path does no Project work.
            finalize_newly_discovered_session(db, &s, is_new, workspace)?;
            // §8 — trashed sessions are skipped entirely: no ingest, no sync.
            if s.is_trashed() {
                continue;
            }
            on_session(&s);
            match ingest_and_sync_session(db, engine, &s) {
                Ok((events, _applied)) => total_events += events,
                Err(e) => eprintln!("[reconcile] ingest {} failed: {}", s.id, e),
            }
        }
    }

    // §19-4/§37.19 — the one piece of workspace work a skipped Session still
    // owes. It runs AFTER the loop so a path first registered by this very pass
    // is already visible to it, and it never observes a path (the identity is
    // stored), so a pass over frozen files stays as cheap as it looks.
    match crate::workspace::session::attach_sessions_to_registered_paths(db) {
        Ok(0) => {}
        Ok(n) => eprintln!("[reconcile] 补绑 {} 个会话到已注册的 workspace 路径", n),
        Err(e) => eprintln!("[reconcile] workspace 补绑失败: {e}"),
    }
    Ok((total_discovered, total_events))
}

/// Sync one specific ingest source: discover sessions under that source's
/// own path only, ingest + sync them.
///
/// Takes the [`crate::launcher::LaunchWorkspace`] because first discovery can
/// happen here as easily as in a full reconcile, and a Session's Owner is
/// decided once per Session — see [`finalize_newly_discovered_session`].
pub fn reconcile_source<F>(
    db: &Db,
    engine: &SyncEngine,
    source: &IngestSource,
    workspace: &crate::launcher::LaunchWorkspace,
    on_session: &F,
) -> Result<(usize, i64)>
where
    F: Fn(&Session),
{
    let adapter = crate::adapters::adapter_for(source.agent);
    let unchanged = unchanged_since_cursor(db)?;
    let roots = vec![PathBuf::from(&source.path)];
    let discovered = adapter.discover_sessions_in(&roots, &unchanged)?;
    let discovered_count = discovered.len();
    let mut total_events = 0i64;
    for d in &discovered {
        let (s, is_new) = ensure_session_row(db, d)?;
        finalize_newly_discovered_session(db, &s, is_new, workspace)?;
        // §8 — trashed sessions are skipped entirely.
        if s.is_trashed() {
            continue;
        }
        on_session(&s);
        match ingest_and_sync_session(db, engine, &s) {
            Ok((events, _)) => total_events += events,
            Err(e) => eprintln!("[reconcile] ingest {} failed: {}", s.id, e),
        }
    }
    Ok((discovered_count, total_events))
}

/// Re-ingest one source: rewind the read cursor of every session found
/// under its path (generation bump, offset 0) and re-scan from scratch.
/// The EVENT STORE IS NEVER DELETED — event ids and every SourceReference
/// in context revisions stay valid, because unchanged content dedups by
/// identity and only genuinely new/changed source content appends.
/// Context items and audit history are untouched.
///
/// Deliberately passes the all-false "unchanged" predicate: a re-ingest
/// WANTS to re-read every file, cursor match or not.
///
/// Like [`reconcile_source`], it takes the launch workspace so a Session it
/// discovers first still gets the chance to claim its LaunchIntent.
pub fn reingest_source<F>(
    db: &Db,
    engine: &SyncEngine,
    source: &IngestSource,
    workspace: &crate::launcher::LaunchWorkspace,
    on_session: &F,
) -> Result<(usize, i64)>
where
    F: Fn(&Session),
{
    let adapter = crate::adapters::adapter_for(source.agent);
    let roots = vec![PathBuf::from(&source.path)];
    let discovered = adapter.discover_sessions_in(&roots, &|_| false)?;
    let discovered_count = discovered.len();
    let mut total_events = 0i64;
    for d in &discovered {
        let (s, is_new) = ensure_session_row(db, d)?;
        finalize_newly_discovered_session(db, &s, is_new, workspace)?;
        // §8 — a trashed session keeps its cursor untouched: no rewind,
        // no re-scan. (Its source file is also invisible to discovery
        // updates, so a rewind would be a mutation with no consumer.)
        if s.is_trashed() {
            continue;
        }
        // overwrite/refresh: re-read the source from position 0
        db.reset_session_source_cursor(&s.id)?;
        on_session(&s);
        match ingest_and_sync_session(db, engine, &s) {
            Ok((events, _)) => total_events += events,
            Err(e) => eprintln!("[reingest] ingest {} failed: {}", s.id, e),
        }
    }
    Ok((discovered_count, total_events))
}

/// Sessions with pending (un-ingested) activity, used by "sync stale" flows.
///
/// Scoped to the Sessions that OWN this Workstream (方案 §40) — a Session
/// belongs to at most one, so it can never be synced twice under this rule.
/// "Stale" = the Agent source file was modified after the Session's last
/// recorded activity, so there may be a delta the processed cursor has not
/// seen. Read-only observation; nothing is written here.
pub fn stale_sessions(db: &Db, workstream_id: &str) -> Result<Vec<Session>> {
    let mut out = Vec::new();
    for s in db.sessions_for_workstream(workstream_id)? {
        if let Ok(meta) = std::fs::metadata(&s.raw_path) {
            if let Ok(modified) = meta.modified() {
                let m: chrono::DateTime<chrono::Utc> = modified.into();
                let last_used = s
                    .last_activity_at
                    .as_deref()
                    .or(s.started_at.as_deref())
                    .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                    .map(|t| t.with_timezone(&chrono::Utc))
                    .unwrap_or_else(chrono::Utc::now);
                if m > last_used {
                    out.push(s);
                }
            }
        }
    }
    Ok(out)
}
