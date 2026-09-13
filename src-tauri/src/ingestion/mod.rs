//! Session ingestion: discovery → upsert → incremental read → cursor.
//!
//! Invariants:
//! - raw agent files are never modified;
//! - already-ingested history is never auto-deleted even when the source
//!   file is truncated / compacted;
//! - cursors only advance after events are durably stored.

use std::collections::HashMap;

use crate::adapters::AgentAdapter;
use crate::domain::{Agent, Session};
use crate::error::Result;
use crate::storage::{now, new_id, Db};

/// Ingest one session incrementally. Returns number of newly stored events.
pub fn ingest_session(db: &Db, adapter: &dyn AgentAdapter, session: &mut Session) -> Result<i64> {
    let from = db.get_cursor(&session.id)?;
    let delta = adapter.read_delta(session, from)?;
    if delta.events.is_empty() {
        db.set_cursor(&session.id, delta.last_sequence, delta.file_size)?;
        return Ok(0);
    }
    db.append_events(&delta.events)?;
    if let Err(e) = db.index_events(&delta.events) {
        eprintln!("[ingest] index_events failed: {}", e);
    }
    db.set_cursor(&session.id, delta.last_sequence, delta.file_size)?;
    let n = delta.events.len() as i64;
    // refresh activity timestamp
    session.last_activity_at = Some(now());
    Ok(n)
}

/// Ensure a session row exists for a discovered session; returns internal id.
pub fn ensure_session_row(db: &Db, d: &crate::adapters::DiscoveredSession) -> Result<Session> {
    let existing = db.find_session_by_agent_id(d.agent, &d.agent_session_id)?;
    if let Some(s) = existing {
        return Ok(s);
    }
    let title = d
        .first_user_text
        .as_deref()
        .and_then(crate::adapters::title_from_text);
    let s = Session {
        id: new_id(),
        agent: d.agent,
        agent_session_id: d.agent_session_id.clone(),
        title,
        cwd: d.cwd.clone(),
        project_id: None,
        raw_path: d.path.to_string_lossy().to_string(),
        parent_agent_session_id: d.parent_agent_session_id.clone(),
        started_at: d.started_at.clone(),
        last_activity_at: d.last_activity_at.clone(),
    };
    db.upsert_session(&s)?;
    Ok(s)
}

/// Full reconcile: discover everything, ingest new content.
/// Returns (sessions_discovered, events_ingested).
pub fn reconcile_all(db: &Db) -> Result<(usize, i64)> {
    reconcile_with_engine(db, &crate::sync::SyncEngine::default())
}

/// Reconcile with a specific sync engine (heuristic or CLI-backed).
/// Uses the single ingest+sync path so cursors only advance after the
/// engine has processed the delta.
pub fn reconcile_with_engine(
    db: &Db,
    engine: &crate::sync::SyncEngine,
) -> Result<(usize, i64)> {
    let adapters = crate::adapters::all_adapters();
    let mut total_discovered = 0usize;
    let mut total_events = 0i64;

    for adapter in &adapters {
        let discovered = match adapter.discover_sessions() {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[reconcile] {} discovery failed: {}", adapter.agent().display_name(), e);
                continue;
            }
        };
        total_discovered += discovered.len();
        for d in discovered {
            let s = match ensure_session_row(db, &d) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("[reconcile] ensure row failed: {}", e);
                    continue;
                }
            };
            match crate::launcher::ingest_and_sync_session(db, engine, &s) {
                Ok((events, _applied)) => total_events += events,
                Err(e) => eprintln!("[reconcile] ingest {} failed: {}", s.id, e),
            }
        }
    }
    Ok((total_discovered, total_events))
}

/// Sync Point: reconcile a single agent's sessions (e.g. before New Session).
pub fn reconcile_agent(db: &Db, agent: Agent) -> Result<i64> {
    let adapter = crate::adapters::adapter_for(agent);
    let discovered = adapter.discover_sessions()?;
    let mut total = 0i64;
    for d in discovered {
        let mut s = ensure_session_row(db, &d)?;
        total += ingest_session(db, adapter, &mut s)?;
    }
    Ok(total)
}

/// Map cwd → suggested project (auto project identification, heuristic).
/// A session whose cwd contains the project name (or vice versa) is proposed
/// for that project; nothing is silently reassigned away from a user choice.
pub fn suggest_project_for_session(db: &Db, session: &Session) -> Option<String> {
    let cwd = session.cwd.as_deref()?;
    let projects = db.list_projects().ok()?;
    for p in projects {
        let token = p.name.to_lowercase().replace([' ', '-'], "");
        if token.is_empty() {
            continue;
        }
        let cwd_norm = cwd.to_lowercase().replace(['\\', '/'], "");
        if cwd_norm.contains(&token) {
            return Some(p.id);
        }
    }
    None
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
