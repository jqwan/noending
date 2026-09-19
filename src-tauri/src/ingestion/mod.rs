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
//! Locking model: reconcile functions take `&Mutex<Db>` and acquire the
//! lock per session / per phase, so concurrent UI commands interleave while
//! a (possibly minutes-long, LLM-backed) sync runs in the background.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::adapters::AgentAdapter;
use crate::domain::{IngestSource, ProjectAffinityEvidence, Session};
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

/// Ingest + sync one session while the caller holds the DB lock — used by
/// interactive single-session flows (resume, per-session sync), which are
/// heuristic-speed. Background reconcile uses the non-blocking twin below.
///
/// With Context Intelligence off this stops after ingestion: events are stored
/// and indexed, the read cursor advances, and nothing is prepared, extracted
/// or committed, so `processed_sequence` stays frozen for a later replay.
pub fn ingest_and_sync_session(
    db: &Db,
    engine: &SyncEngine,
    session: &Session,
) -> Result<(i64, usize)> {
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

/// Non-blocking twin: takes the DB lock per phase so the UI never stalls
/// behind extraction. Returns (events_ingested, mutations_applied).
pub fn ingest_and_sync_session_nb(
    db_lock: &Mutex<Db>,
    engine: &SyncEngine,
    session: &Session,
) -> Result<(i64, usize)> {
    // Phase 1 (lock): ingest the file delta + prepare the run.
    let (stored, pre) = {
        let guard = crate::sync::lock_db(db_lock)?;
        let adapter = crate::adapters::adapter_for(session.agent);
        let stored = ingest_delta(&guard, adapter, session)?;
        let pre = if !crate::settings::context_intelligence_enabled(&guard)? {
            None
        } else {
            let processed = guard.get_processed_sequence(&session.id)?;
            let pending = guard.get_events(&session.id, Some(processed), 10_000)?;
            if pending.is_empty() {
                None
            } else {
                let to = pending.last().map(|e| e.sequence).unwrap_or(processed);
                engine.prepare(&guard, session, &pending, processed, to)?
            }
        };
        (stored.len() as i64, pre)
    }; // lock released

    let Some(pre) = pre else {
        return Ok((stored, 0));
    };

    // Phase 2 (NO lock): extraction — may run the agent CLI for minutes.
    let (mutations, runtime, diagnostics) = engine.extract(session, &pre)?;

    // Phase 3 (lock): atomic commit.
    let applied = {
        let guard = crate::sync::lock_db(db_lock)?;
        let out = engine.commit(&guard, session, &pre, mutations, &runtime, diagnostics)?;
        out.applied
    };
    Ok((stored, applied))
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
    let title = d
        .first_user_text
        .as_deref()
        .and_then(crate::adapters::title_from_text);
    let existing = db.find_session_by_agent_id(d.agent, &d.agent_session_id)?;
    let raw_path = d.path.to_string_lossy().to_string();
    if let Some(s) = existing {
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
            crate::workspace::session::resolve_session_path(db.conn(), attacher, observed_cwd)?
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
            // attachment and the binding claims that read through it move
            // together in the one transaction below, so an interrupted pass can
            // never leave a claim pointing at a path the Session no longer has —
            // and `moved` stays true until both are done, which makes the retry
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
            db.conn(),
            attacher,
            d.cwd.as_deref(),
        )?,
        // Never taken from the caller: `upsert_session` derives it from that
        // WorkspacePath in the same statement (§42.3-M3).
        project_id: None,
        raw_path,
        parent_agent_session_id: d.parent_agent_session_id.clone(),
        started_at: d.started_at.clone(),
        last_activity_at: d.last_activity_at.clone(),
    };
    db.upsert_session(&s)?;
    let stored = db.get_session(&s.id)?.unwrap_or(s);
    Ok((stored, true))
}

/// Full reconcile over every agent's enabled sources. The DB lock is taken
/// per session; `on_session` observes each session being processed.
///
/// `workspace` is §13's tier-3 fact, needed because a freshly discovered
/// Session may claim a pending LaunchIntent: matching it asks whether that
/// Session's cwd is just the shared default workspace, which is not in the DB.
pub fn reconcile_with_engine<F>(
    db_lock: &Mutex<Db>,
    engine: &SyncEngine,
    workspace: &crate::launcher::LaunchWorkspace,
    on_session: &F,
) -> Result<(usize, i64)>
where
    F: Fn(&Session),
{
    let adapters = crate::adapters::all_adapters();
    let mut total_discovered = 0usize;
    let mut total_events = 0i64;

    // housekeeping: expire launch intents that never got a session
    {
        let guard = crate::sync::lock_db(db_lock)?;
        let _ = crate::launcher::expire_stale_launch_intents(&guard);
    }

    for adapter in &adapters {
        let roots: Vec<String> = {
            let guard = crate::sync::lock_db(db_lock)?;
            guard.enabled_roots(adapter.agent())?
        };
        if roots.is_empty() {
            continue;
        }
        let roots: Vec<PathBuf> = roots.into_iter().map(PathBuf::from).collect();
        let discovered = match adapter.discover_sessions_in(&roots) {
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
            let (s, is_new) = {
                let guard = crate::sync::lock_db(db_lock)?;
                match ensure_session_row(&guard, &d) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("[reconcile] ensure row failed: {}", e);
                        continue;
                    }
                }
            };
            if is_new {
                // A brand-new external session may claim a pending
                // LaunchIntent (crash recovery included).
                {
                    let guard = crate::sync::lock_db(db_lock)?;
                    match crate::launcher::try_match_launch_intents_in(&guard, &s, workspace) {
                        Ok(true) => {
                            eprintln!("[reconcile] launch intent matched to session {}", s.id)
                        }
                        Ok(false) => {}
                        Err(e) => eprintln!("[reconcile] intent match failed: {}", e),
                    }
                    // §42.2-E11 — no name-substring Project evidence is recorded
                    // any more. A Project is derived from the Session's
                    // WorkspacePath (§1.10), which `ensure_session_row` has just
                    // resolved, so this hot path does no Project work.
                }
            }
            on_session(&s);
            match ingest_and_sync_session_nb(db_lock, engine, &s) {
                Ok((events, _applied)) => total_events += events,
                Err(e) => eprintln!("[reconcile] ingest {} failed: {}", s.id, e),
            }
        }
    }
    Ok((total_discovered, total_events))
}

/// Sync one specific ingest source: discover sessions under that source's
/// own path only, ingest + sync them.
pub fn reconcile_source<F>(
    db_lock: &Mutex<Db>,
    engine: &SyncEngine,
    source: &IngestSource,
    on_session: &F,
) -> Result<(usize, i64)>
where
    F: Fn(&Session),
{
    let adapter = crate::adapters::adapter_for(source.agent);
    let roots = vec![PathBuf::from(&source.path)];
    let discovered = adapter.discover_sessions_in(&roots)?;
    let discovered_count = discovered.len();
    let mut total_events = 0i64;
    for d in &discovered {
        let s = {
            let guard = crate::sync::lock_db(db_lock)?;
            ensure_session_row(&guard, d)?.0
        };
        on_session(&s);
        match ingest_and_sync_session_nb(db_lock, engine, &s) {
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
/// Bindings, context items and audit history are untouched.
pub fn reingest_source<F>(
    db_lock: &Mutex<Db>,
    engine: &SyncEngine,
    source: &IngestSource,
    on_session: &F,
) -> Result<(usize, i64)>
where
    F: Fn(&Session),
{
    let adapter = crate::adapters::adapter_for(source.agent);
    let roots = vec![PathBuf::from(&source.path)];
    let discovered = adapter.discover_sessions_in(&roots)?;
    let discovered_count = discovered.len();
    let mut total_events = 0i64;
    for d in &discovered {
        let s = {
            let guard = crate::sync::lock_db(db_lock)?;
            let (s, _is_new) = ensure_session_row(&guard, d)?;
            // overwrite/refresh: re-read the source from position 0
            guard.reset_session_source_cursor(&s.id)?;
            s
        };
        on_session(&s);
        match ingest_and_sync_session_nb(db_lock, engine, &s) {
            Ok((events, _)) => total_events += events,
            Err(e) => eprintln!("[reingest] ingest {} failed: {}", s.id, e),
        }
    }
    Ok((discovered_count, total_events))
}

/// RETIRED (方案 §42.2-E11) — the name-substring affinity matcher.
///
/// It used to run on every newly discovered Session and write `cwd_match`
/// evidence by matching `session.cwd` against a lowercased substring of
/// `Project.name`. Once a Project name is itself derived from a path, that turns
/// two path facts into a third, invented membership relation — and it is the
/// amplifier behind the `~/.git` dotfiles mis-classification in §1.4. No product
/// path calls it any more.
///
/// Kept, with the `project_affinity_evidence` rows it wrote, as read-only
/// history: evidence rows are audit provenance for decisions already taken, and
/// deleting them would rewrite why an old suggestion looked the way it did.
pub fn record_session_project_evidence(db: &Db, session: &Session) {
    let Some(cwd) = &session.cwd else { return };
    let Ok(projects) = db.list_projects() else {
        return;
    };
    let cwd_norm = cwd.to_lowercase().replace(['\\', '/'], "");
    for p in projects {
        let token = p.name.to_lowercase().replace([' ', '-'], "");
        if token.is_empty() {
            continue;
        }
        if cwd_norm.contains(&token) {
            let e = ProjectAffinityEvidence {
                id: new_id(),
                session_id: Some(session.id.clone()),
                workstream_id: None,
                project_id: p.id.clone(),
                evidence_type: "cwd_match".into(),
                source: format!("cwd={}", cwd),
                score: 1.0,
                created_at: now(),
            };
            if let Err(err) = db.insert_evidence(&e) {
                eprintln!("[ingest] evidence insert failed: {}", err);
            }
        }
    }
}

/// RETIRED (方案 §11, §42.2-E11) — score-based Project suggestion built on the
/// affinity evidence above. It already had no caller in the product; it stays
/// compilable so old read paths keep resolving recorded history, and it still
/// only ever *suggests*. Membership itself is derived from the Session's
/// WorkspacePath now (§1.10).
pub fn suggest_project_for_session(db: &Db, session: &Session) -> Option<(String, f32)> {
    db.resolve_project_affinity(&session.id).ok().flatten()
}

/// Sessions with pending (un-ingested) activity, used by "sync stale" flows.
pub fn stale_sessions(db: &Db, workstream_id: &str) -> Result<Vec<Session>> {
    let bindings = db.bindings_for_workstream(workstream_id)?;
    let mut out = Vec::new();
    let mut seen: HashMap<String, ()> = HashMap::new();
    for b in bindings {
        if seen.contains_key(&b.session_id) {
            continue;
        }
        seen.insert(b.session_id.clone(), ());
        if let Some(s) = db.get_session(&b.session_id)? {
            // compare stored cursor vs file size via quick re-discover is
            // expensive; consider stale when file mtime newer than last used
            if let Ok(meta) = std::fs::metadata(&s.raw_path) {
                if let Ok(modified) = meta.modified() {
                    let m: chrono::DateTime<chrono::Utc> = modified.into();
                    let last_used = chrono::DateTime::parse_from_rfc3339(&b.last_used_at)
                        .map(|t| t.with_timezone(&chrono::Utc))
                        .unwrap_or_else(|_| chrono::Utc::now());
                    if m > last_used {
                        out.push(s);
                    }
                }
            }
        }
    }
    Ok(out)
}
