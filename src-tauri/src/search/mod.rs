//! Search — FTS5 when available, LIKE fallback otherwise.
//! Prioritizes current context (items / workstreams), then raw events.

use rusqlite::params;
use serde::Serialize;

use crate::error::Result;
use crate::storage::Db;

#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub kind: String, // item | workstream | project | event
    pub ref_id: String,
    pub parent_id: String,
    pub title: String,
    pub snippet: String,
    pub rank: f64,
}

/// Review P1-1 — the read-side lifecycle authority: event rows surface only
/// while their session is active. Belt and braces beside the write-side
/// guards (trash unindex + guarded backfill): a stale row left by any older
/// build or crash must not surface a trashed session in search.
const ACTIVE_EVENT_GUARD: &str = "(search_index.kind != 'event' OR EXISTS (SELECT 1 FROM sessions s WHERE s.id = search_index.parent_id AND s.trashed_at IS NULL))";

pub fn search(db: &Db, query: &str, limit: i64) -> Result<Vec<SearchHit>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(vec![]);
    }
    if db.fts_available() {
        let fts_q = to_fts_query(q);
        let sql = format!(
            "SELECT kind, ref_id, parent_id, title,
                   snippet(search_index, 4, '「', '」', '…', 12),
                   bm25(search_index)
                   FROM search_index WHERE search_index MATCH ?1
                   AND {ACTIVE_EVENT_GUARD}
                   ORDER BY bm25(search_index) LIMIT ?2"
        );
        match db.conn().prepare(&sql) {
            Ok(mut st) => {
                let rows = st.query_map(params![fts_q, limit], |r| {
                    Ok(SearchHit {
                        kind: r.get(0)?,
                        ref_id: r.get(1)?,
                        parent_id: r.get(2)?,
                        title: r.get(3)?,
                        snippet: r.get(4)?,
                        rank: r.get::<_, f64>(5)?,
                    })
                })?;
                let hits: Vec<SearchHit> = rows.filter_map(|r| r.ok()).collect();
                if !hits.is_empty() || looks_indexed(db, q) {
                    return Ok(hits);
                }
                // fall through to LIKE when FTS finds nothing (e.g. tokenization)
                return like_search(db, q, limit);
            }
            Err(_) => return like_search(db, q, limit),
        }
    }
    like_search(db, q, limit)
}

fn looks_indexed(db: &Db, _q: &str) -> bool {
    db.conn()
        .query_row("SELECT COUNT(*) > 0 FROM search_index", [], |r| {
            r.get::<_, bool>(0)
        })
        .unwrap_or(false)
}

fn like_search(db: &Db, q: &str, limit: i64) -> Result<Vec<SearchHit>> {
    let pattern = format!("%{}%", q.replace('%', ""));
    let sql = format!(
        "SELECT kind, ref_id, parent_id, title, body FROM search_index
               WHERE (title LIKE ?1 OR body LIKE ?1) AND {ACTIVE_EVENT_GUARD}
               LIMIT ?2"
    );
    let mut st = db.conn().prepare(&sql)?;
    let rows = st.query_map(params![pattern, limit], |r| {
        Ok(SearchHit {
            kind: r.get(0)?,
            ref_id: r.get(1)?,
            parent_id: r.get(2)?,
            title: r.get(3)?,
            snippet: crate::adapters::truncate_text(&r.get::<_, String>(4)?, 160),
            rank: 0.0,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
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
