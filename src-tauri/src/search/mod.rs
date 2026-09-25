//! Search — FTS5, with a LIKE fallback for queries its tokenizer cannot match.
//! Prioritizes current context (items / workstreams), then raw events.

use rusqlite::params;
use serde::Serialize;

use crate::error::Result;
use crate::storage::Db;

#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub kind: String, // item | workstream | project | session | event
    pub ref_id: String,
    pub parent_id: String,
    pub title: String,
    pub snippet: String,
    pub rank: f64,
}

/// Review P1-1 — the read-side lifecycle authority: a Session's own document
/// (`kind = 'session'`, `ref_id` is the session id) and its event rows surface
/// only while that session is active. Belt and braces beside the write-side
/// guards (trash unindex + guarded backfill): a stale row left behind by an
/// interrupted write must not surface a trashed session in search.
const ACTIVE_EVENT_GUARD: &str = "(
    search_index.kind NOT IN ('event', 'session')
    OR EXISTS (
        SELECT 1 FROM sessions s
         WHERE s.id = CASE search_index.kind
                        WHEN 'event' THEN search_index.parent_id
                        ELSE search_index.ref_id
                      END
           AND s.trashed_at IS NULL))";

pub fn search(db: &Db, query: &str, limit: i64) -> Result<Vec<SearchHit>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(vec![]);
    }
    let hits = fts_search(db, q, limit)?;
    if hits.is_empty() {
        // FTS5 MATCH compares whole tokens, so a query that sits *inside* a
        // token — a substring of an identifier, or CJK without spaces — is only
        // answerable by LIKE.
        return like_search(db, q, limit);
    }
    Ok(hits)
}

/// The FTS5 pass. A failing statement is a real error now that the index is a
/// required part of the format, and must not be hidden behind the LIKE pass.
fn fts_search(db: &Db, q: &str, limit: i64) -> Result<Vec<SearchHit>> {
    let sql = format!(
        "SELECT kind, ref_id, parent_id, title,
               snippet(search_index, 4, '「', '」', '…', 12),
               bm25(search_index)
               FROM search_index WHERE search_index MATCH ?1
               AND {ACTIVE_EVENT_GUARD}
               ORDER BY bm25(search_index) LIMIT ?2"
    );
    let conn = db.read();
    let mut st = conn.prepare(&sql)?;
    let rows = st.query_map(params![to_fts_query(q), limit], |r| {
        Ok(SearchHit {
            kind: r.get(0)?,
            ref_id: r.get(1)?,
            // FTS columns admit NULL: a Workstream with no primary path has no
            // parent Project, and it still has to be findable.
            parent_id: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            title: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
            snippet: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
            rank: r.get::<_, f64>(5)?,
        })
    })?;
    let mut hits = Vec::new();
    for row in rows {
        hits.push(row?);
    }
    Ok(hits)
}

fn like_search(db: &Db, q: &str, limit: i64) -> Result<Vec<SearchHit>> {
    let pattern = format!("%{}%", q.replace('%', ""));
    let sql = format!(
        "SELECT kind, ref_id, parent_id, title, body FROM search_index
               WHERE (title LIKE ?1 OR body LIKE ?1) AND {ACTIVE_EVENT_GUARD}
               LIMIT ?2"
    );
    let conn = db.read();
    let mut st = conn.prepare(&sql)?;
    let rows = st.query_map(params![pattern, limit], |r| {
        Ok(SearchHit {
            kind: r.get(0)?,
            ref_id: r.get(1)?,
            parent_id: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            title: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
            snippet: crate::adapters::truncate_text(
                &r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                160,
            ),
            rank: 0.0,
        })
    })?;
    let mut hits = Vec::new();
    for row in rows {
        hits.push(row?);
    }
    Ok(hits)
}

/// Build an FTS query: quote each term, AND them; tolerate CJK bigrams
/// by also trying the raw phrase.
fn to_fts_query(q: &str) -> String {
    let terms: Vec<String> = q
        .split_whitespace()
        .map(|t| format!("\"{}\"", t.replace('"', "")))
        .collect();
    if terms.is_empty() {
        format!("\"{}\"", q.replace('"', ""))
    } else {
        terms.join(" AND ")
    }
}
