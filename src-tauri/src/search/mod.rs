//! Object navigation and scoped conversation content search.
use serde::Serialize;
pub mod navigation;
pub mod objects;
pub mod sessions;

#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub kind: String, // session | message
    pub ref_id: String,
    pub parent_id: String,
    pub title: String,
    pub snippet: String,
    pub rank: f64,
}

/// Only the current conversation projection is searchable. Keep the read-side
/// guard even though ingestion retires obsolete message rows and search documents
/// atomically, so an unmapped document can never become a search hit.
const CURRENT_MESSAGE_GUARD: &str = "(
    search_index.kind != 'message'
    OR EXISTS (
        SELECT 1 FROM session_message_projection p
         WHERE p.session_id = search_index.parent_id
           AND p.session_message_id = search_index.ref_id))";

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
