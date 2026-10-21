use super::{CliError, CliResult};
use crate::{
    context::items::{self, SummaryFilter},
    storage::Db,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    query: String,
    filter: SummaryFilter,
    fingerprint: String,
    offset: usize,
}

pub(super) fn page(
    db: &Db,
    query: &str,
    filter: SummaryFilter,
    cursor: Option<&str>,
    limit: usize,
) -> CliResult<Value> {
    let query = query.trim();
    filter
        .validate()
        .map_err(|e| CliError::new("invalid_arguments", e))?;
    if query.is_empty() || !(1..=50).contains(&limit) {
        return Err(CliError::new(
            "invalid_arguments",
            "搜索内容不能为空，limit 必须在 1 到 50 之间",
        ));
    }
    let conn = db.read();
    let tx = conn.unchecked_transaction()?;
    let fingerprint = items::fingerprint(&tx, &filter)?;
    let offset = if let Some(raw) = cursor {
        let cursor: Cursor = URL_SAFE_NO_PAD
            .decode(raw)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .ok_or_else(|| CliError::new("invalid_cursor", "摘要检索游标无效"))?;
        if cursor.query != query
            || cursor.filter != filter
            || cursor.offset == 0
            || cursor.offset > i64::MAX as usize - 51
        {
            return Err(CliError::new("invalid_cursor", "游标与搜索条件不一致"));
        }
        if cursor.fingerprint != fingerprint {
            return Err(CliError::new(
                "stale_cursor",
                "摘要或范围已变化，请重新检索",
            ));
        }
        cursor.offset
    } else {
        0
    };
    let mut hits = items::search_page(&tx, query, &filter, offset, limit + 1)?;
    tx.commit()?;
    let more = hits.len() > limit;
    hits.truncate(limit);
    let next = if more {
        Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&Cursor {
            query: query.into(),
            filter,
            fingerprint,
            offset: offset + limit,
        })?))
    } else {
        None
    };
    Ok(json!({"items":hits,"next_cursor":next,"end":!more}))
}
