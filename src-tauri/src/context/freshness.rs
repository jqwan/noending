//! Read-only memory coverage. Counts refer to ingested user/assistant messages,
//! never to unseen bytes on disk or source-file tool events.
use crate::error::{other, Result};
use crate::storage::{freshness as repo, Db};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize)]
pub struct SummaryFreshness {
    pub refreshed_at: Option<String>,
    /// None when a rewrite or an unknown consumed frontier prevents an exact count.
    pub pending_messages: Option<i64>,
    pub history_rewritten_sessions: usize,
    pub unknown_coverage_sessions: usize,
    pub summary_changed_sessions: usize,
    pub scope_changed: bool,
    pub pending: bool,
}

pub fn summary(conn: &Connection, kind: &str, id: &str) -> Result<SummaryFreshness> {
    let (state, key, scope) = match kind {
        "session" => ("session_contexts", "session_id", "s.id=?1"),
        "task" => ("workstream_context_state", "workstream_id", "EXISTS(SELECT 1 FROM session_workstreams sw WHERE sw.session_id=s.id AND sw.workstream_id=?1)"),
        "project" => ("project_context_state", "project_id", "EXISTS(SELECT 1 FROM workspace_paths wp WHERE wp.id=s.workspace_path_id AND wp.project_id=?1)"),
        _ => return Err(other("无效的记忆范围")),
    };
    let refreshed_at: Option<String> = conn
        .query_row(
            &format!("SELECT refreshed_at FROM {state} WHERE {key}=?1"),
            [id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let frontier = match kind {
        "task" => {
            "LEFT JOIN workstream_session_frontiers f ON f.session_id=s.id AND f.workstream_id=?1"
        }
        "project" => {
            "LEFT JOIN project_session_frontiers f ON f.session_id=s.id AND f.project_id=?1"
        }
        _ => "",
    };
    let columns = if kind == "session" {
        "c.session_id,c.ingest_generation,c.processed_through_seq,c.revision"
    } else {
        "f.session_id,f.ingest_generation,f.processed_through_seq,f.session_context_revision"
    };
    let sql = format!("SELECT s.fact_generation,s.latest_message_seq,COALESCE(c.revision,0),{columns} FROM sessions s LEFT JOIN session_contexts c ON c.session_id=s.id {frontier} WHERE {scope}");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([id], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, Option<i64>>(4)?,
            r.get::<_, Option<i64>>(5)?,
            r.get::<_, Option<i64>>(6)?,
        ))
    })?;
    let mut result = SummaryFreshness {
        refreshed_at,
        pending_messages: Some(0),
        ..Default::default()
    };
    let mut total = 0;
    for row in rows {
        let (
            generation,
            latest,
            revision,
            consumed,
            consumed_generation,
            through,
            consumed_revision,
        ) = row?;
        if kind != "session" && consumed_revision.unwrap_or(0) != revision {
            result.summary_changed_sessions += 1;
        }
        if consumed.is_none() {
            total += latest;
            if kind != "session" {
                result.scope_changed = true;
            }
        } else if consumed_generation.is_none() || through.is_none() {
            result.unknown_coverage_sessions += 1;
        } else if consumed_generation != Some(generation) || through.is_some_and(|p| p > latest) {
            result.history_rewritten_sessions += 1;
        } else {
            total += latest - through.unwrap();
        }
    }
    if kind == "task" {
        let state = super::query::task_state(conn, id)?;
        let removed: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM workstream_session_frontiers f WHERE f.workstream_id=?1 AND NOT EXISTS(SELECT 1 FROM session_workstreams sw WHERE sw.workstream_id=?1 AND sw.session_id=f.session_id))", [id], |r|r.get(0))?;
        result.scope_changed |= state.input_revision != state.consumed_input_revision || removed;
    } else if kind == "project" {
        result.scope_changed |= super::project::input_changed(conn, id)?;
    }
    if result.history_rewritten_sessions > 0 || result.unknown_coverage_sessions > 0 {
        result.pending_messages = None;
    } else {
        result.pending_messages = Some(total);
    }
    result.pending = total > 0
        || result.pending_messages.is_none()
        || result.summary_changed_sessions > 0
        || result.scope_changed;
    Ok(result)
}

#[derive(Serialize)]
pub struct SourceCheck {
    pub id: String,
    pub agent: crate::domain::Agent,
    pub path: String,
    pub check: repo::SyncCheck,
}
#[derive(Default, Serialize)]
pub struct SessionChecks {
    pub total: usize,
    pub checked: usize,
    pub unknown: usize,
    pub failed: usize,
    pub missing: usize,
    pub oldest_checked_at: Option<String>,
    pub latest_checked_at: Option<String>,
}
#[derive(Serialize)]
pub struct FreshnessView {
    pub kind: Option<String>,
    pub id: Option<String>,
    pub all_sources: repo::SyncCheck,
    pub sources: Vec<SourceCheck>,
    pub sessions: SessionChecks,
    pub session_check: Option<repo::SyncCheck>,
    pub summary: Option<SummaryFreshness>,
}

pub fn view(db: &Db, kind: Option<&str>, id: Option<&str>) -> Result<FreshnessView> {
    let conn = db.read();
    let tx = conn.unchecked_transaction()?;
    let scope = match (kind,id) {
        (None,None) => "1=1",
        (Some("session"),Some(_)) => "s.id=?1",
        (Some("task"),Some(_)) => "EXISTS(SELECT 1 FROM session_workstreams sw WHERE sw.session_id=s.id AND sw.workstream_id=?1)",
        (Some("project"),Some(_)) => "EXISTS(SELECT 1 FROM workspace_paths wp WHERE wp.id=s.workspace_path_id AND wp.project_id=?1)",
        _ => return Err(other("kind 与 id 必须同时提供，kind 为 session/task/project")),
    };
    if let (Some(kind), Some(id)) = (kind, id) {
        let table = match kind {
            "session" => "sessions",
            "task" => "workstreams",
            _ => "projects",
        };
        let exists: bool = tx.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id=?1)"),
            [id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(other("记忆目标不存在"));
        }
    }
    let mut stats = SessionChecks::default();
    let mut source_paths = Vec::new();
    let sql = format!("SELECT s.agent,s.source_path,c.started_at,c.finished_at,c.outcome,c.checked_at FROM sessions s LEFT JOIN sync_checks c ON c.scope='session' AND c.target_id=s.id WHERE {scope}");
    let mut stmt = tx.prepare(&sql)?;
    let mut rows = if kind.is_some() {
        stmt.query(params![id])?
    } else {
        stmt.query([])?
    };
    while let Some(r) = rows.next()? {
        stats.total += 1;
        let check = repo::SyncCheck {
            started_at: r.get(2)?,
            finished_at: r.get(3)?,
            outcome: r.get(4)?,
            checked_at: r.get(5)?,
        };
        if let Some(at) = &check.checked_at {
            stats.checked += 1;
            if stats.oldest_checked_at.as_ref().is_none_or(|old| at < old) {
                stats.oldest_checked_at = Some(at.clone());
            }
            if stats.latest_checked_at.as_ref().is_none_or(|old| at > old) {
                stats.latest_checked_at = Some(at.clone());
            }
        } else {
            stats.unknown += 1;
        }
        match check.outcome.as_deref() {
            Some("missing") => stats.missing += 1,
            Some("failed" | "partial" | "cancelled") => stats.failed += 1,
            _ => {}
        }
        source_paths.push((r.get::<_, String>(0)?, r.get::<_, String>(1)?));
    }
    drop(rows);
    drop(stmt);
    let mut stmt = tx.prepare("SELECT id,agent,path,origin FROM ingest_sources ORDER BY id")?;
    let sources = stmt
        .query_map([], |r| {
            Ok(crate::domain::IngestSource {
                id: r.get(0)?,
                agent: crate::domain::Agent::parse(&r.get::<_, String>(1)?)
                    .ok_or_else(|| rusqlite::Error::InvalidQuery)?,
                path: r.get(2)?,
                origin: r.get(3)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut relevant = Vec::new();
    for source in sources {
        let roots = crate::adapters::source_discovery_roots(&source);
        if kind.is_none()
            || kind == Some("project")
            || source_paths.iter().any(|(agent, path)| {
                agent == source.agent.as_str()
                    && roots
                        .iter()
                        .any(|root| crate::workspace::is_within(path, &root.to_string_lossy()))
            })
        {
            let check = repo::check(&tx, "source", &source.id)?;
            relevant.push(SourceCheck {
                id: source.id,
                agent: source.agent,
                path: source.path,
                check,
            });
        }
    }
    let result = FreshnessView {
        kind: kind.map(str::to_owned),
        id: id.map(str::to_owned),
        all_sources: repo::check(&tx, "all", "")?,
        sources: relevant,
        sessions: stats,
        session_check: if kind == Some("session") {
            Some(repo::check(&tx, "session", id.unwrap())?)
        } else {
            None
        },
        summary: kind
            .zip(id)
            .map(|(kind, id)| summary(&tx, kind, id))
            .transpose()?,
    };
    drop(stmt);
    tx.commit()?;
    Ok(result)
}
