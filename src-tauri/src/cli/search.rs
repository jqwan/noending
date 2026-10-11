use super::{CliError, CliResult};
use crate::{
    search::sessions::{self, SessionFilter},
    storage::Db,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    query: String,
    filter: SessionFilter,
    offset: usize,
    fts: bool,
}
pub(super) fn page(
    db: &Db,
    query: &str,
    filter: SessionFilter,
    cursor: Option<&str>,
    limit: usize,
) -> CliResult<Value> {
    let query = query.trim();
    if query.is_empty() || !(1..=50).contains(&limit) {
        return Err(CliError::new(
            "invalid_arguments",
            "搜索内容不能为空，limit 必须在 1 到 50 之间",
        ));
    }
    let (offset, mode) = if let Some(cursor) = cursor {
        let c: Cursor = URL_SAFE_NO_PAD
            .decode(cursor)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .ok_or_else(|| CliError::new("invalid_cursor", "搜索游标无效"))?;
        if c.query != query
            || c.filter != filter
            || c.offset == 0
            || c.offset > i64::MAX as usize - 51
        {
            return Err(CliError::new("invalid_cursor", "游标与搜索条件不一致"));
        }
        (c.offset, Some(c.fts))
    } else {
        (0, None)
    };
    let (mut hits, fts) = sessions::page(db, query, &filter, limit + 1, offset, mode)?;
    let more = hits.len() > limit;
    hits.truncate(limit);
    let items = hits
        .into_iter()
        .map(|result| {
            let hit = result.hit;
            let sid = if hit.kind == "message" {
                &hit.parent_id
            } else {
                &hit.ref_id
            };
            let mut value = json!(hit);
            value["session_id"] = json!(sid);
            value["sequence"] = json!(result.sequence);
            value
        })
        .collect::<Vec<_>>();
    let next = if more {
        Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&Cursor {
            query: query.into(),
            filter,
            offset: offset + limit,
            fts,
        })?))
    } else {
        None
    };
    Ok(json!({"items":items,"next_cursor":next,"end":!more}))
}
