//! Scoped conversation search using the same FTS and current-message rules as UI search.
use super::{to_fts_query, SearchHit, CURRENT_MESSAGE_GUARD};
use crate::{error::Result, storage::Db};
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionFilter {
    pub project: Option<String>,
    pub task: Option<String>,
    pub agent: Option<String>,
    pub scope: String,
}

const FILTER: &str = "search_index.kind IN ('session','message')
    AND EXISTS (SELECT 1 FROM sessions s
      LEFT JOIN workspace_paths wp ON wp.id=s.workspace_path_id
      WHERE s.id=CASE search_index.kind WHEN 'message' THEN search_index.parent_id ELSE search_index.ref_id END
      AND (?2 IS NULL OR wp.project_id=?2)
      AND (?3 IS NULL OR EXISTS (SELECT 1 FROM session_workstreams sw WHERE sw.session_id=s.id AND sw.workstream_id=?3))
      AND (?4 IS NULL OR s.agent=?4)
      AND (?5='all' OR (?5='archived' AND s.archived_at IS NOT NULL)
        OR (?5='unarchived' AND s.archived_at IS NULL)))";

pub(crate) struct SessionHit {
    pub hit: SearchHit,
    pub sequence: Option<i64>,
}

/// Keep the search mode across pages: an exhausted FTS page must not restart
/// the LIKE fallback. Results are live, with deterministic ordering per query.
pub(crate) fn page(
    db: &Db,
    query: &str,
    filter: &SessionFilter,
    limit: usize,
    offset: usize,
    mode: Option<bool>,
) -> Result<(Vec<SessionHit>, bool)> {
    let conn = db.read();
    let tx = conn.unchecked_transaction()?;
    let fts_query = to_fts_query(query);
    let fts = match mode {
        Some(mode) => mode,
        None => tx.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM search_index WHERE search_index MATCH ?1 AND {FILTER} AND {CURRENT_MESSAGE_GUARD})"),
            params![fts_query, filter.project, filter.task, filter.agent, filter.scope],
            |row| row.get::<_, bool>(0),
        )?,
    };
    let escaped = query
        .replace('!', "!!")
        .replace('%', "!%")
        .replace('_', "!_");
    let value = if fts {
        fts_query
    } else {
        format!("%{escaped}%")
    };
    let (predicate, snippet, rank, order) = if fts {
        (
            "search_index MATCH ?1",
            "snippet(search_index,4,'「','」','…',12)",
            "bm25(search_index)",
            "bm25(search_index),kind,ref_id",
        )
    } else {
        (
            "(title LIKE ?1 ESCAPE '!' OR body LIKE ?1 ESCAPE '!')",
            "body",
            "0.0",
            "kind,ref_id",
        )
    };
    let sql = format!("SELECT kind,ref_id,parent_id,title,{snippet},{rank},
        CASE WHEN kind='message' THEN (SELECT ordinal FROM session_message_projection p WHERE p.session_id=search_index.parent_id AND p.session_message_id=search_index.ref_id) END
        FROM search_index WHERE {predicate} AND {FILTER} AND {CURRENT_MESSAGE_GUARD} ORDER BY {order} LIMIT ?6 OFFSET ?7");
    let hits = {
        let mut stmt = tx.prepare(&sql)?;
        let rows = stmt.query_map(
            params![
                value,
                filter.project,
                filter.task,
                filter.agent,
                filter.scope,
                limit as i64,
                offset as i64
            ],
            |r| {
                let snippet = r.get::<_, Option<String>>(4)?.unwrap_or_default();
                Ok(SessionHit {
                    hit: SearchHit {
                        kind: r.get(0)?,
                        ref_id: r.get(1)?,
                        parent_id: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                        title: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                        snippet: if fts {
                            snippet
                        } else {
                            crate::adapters::truncate_text(&snippet, 160)
                        },
                        rank: r.get(5)?,
                    },
                    sequence: r.get(6)?,
                })
            },
        )?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    tx.commit()?;
    Ok((hits, fts))
}
