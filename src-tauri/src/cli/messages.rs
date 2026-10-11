use super::{CliError, CliResult};
use crate::storage::{message_query, Db};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    session: String,
    generation: i64,
    through: i64,
    from: i64,
    query: Option<String>,
    requested_to: Option<i64>,
    position: usize,
}
pub(super) fn page(
    db: &Db,
    id: &str,
    cursor: Option<&str>,
    query: Option<&str>,
    from: i64,
    to: Option<i64>,
    limit: usize,
) -> CliResult<Value> {
    if !(1..=50).contains(&limit)
        || from < 1
        || to.is_some_and(|t| t < from)
        || query.is_some_and(|q| q.trim().is_empty())
    {
        return Err(CliError::new(
            "invalid_arguments",
            "无效的消息范围、搜索内容或分页大小（1～50）",
        ));
    }
    let conn = db.read();
    let tx = conn.unchecked_transaction()?;
    let state: Option<(i64, i64)> = tx
        .query_row(
            "SELECT fact_generation,latest_message_seq FROM sessions WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (generation, latest) = state.ok_or_else(|| CliError::new("not_found", "会话不存在"))?;
    let c = if let Some(cursor) = cursor {
        let bytes = URL_SAFE_NO_PAD
            .decode(cursor)
            .map_err(|_| CliError::new("invalid_cursor", "游标无效"))?;
        let c: Cursor = serde_json::from_slice(&bytes)
            .map_err(|_| CliError::new("invalid_cursor", "游标无效"))?;
        if c.session != id
            || c.query.as_deref() != query
            || c.from != from - 1
            || to != c.requested_to
            || c.position == 0
        {
            return Err(CliError::new("invalid_cursor", "游标与查询条件不一致"));
        }
        if c.generation != generation || c.through > latest {
            return Err(CliError::new(
                "stale_cursor",
                "消息历史已变化，请从第一页重新查询",
            ));
        }
        c
    } else {
        Cursor {
            session: id.into(),
            generation,
            through: to.unwrap_or(latest).min(latest),
            from: from - 1,
            query: query.map(str::to_owned),
            requested_to: to,
            position: 1,
        }
    };
    let total = message_query::part_count(&tx, id, c.from, c.through)?;
    if c.position > total + 1 {
        return Err(CliError::new("invalid_cursor", "游标超出消息范围"));
    }
    let mut records =
        message_query::parts(&tx, id, c.from, c.through, c.position, limit + 1, query)?;
    let more = records.len() > limit;
    records.truncate(limit);
    let next = if more {
        let position = records.last().unwrap()["cursor"].as_u64().unwrap() as usize + 1;
        Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&Cursor { position, ..c })?))
    } else {
        None
    };
    // The generation/cutoff refer to this short read transaction, not a cached
    // body export. New appends remain available on the next fresh query.
    let result = json!({"session_id":id,"ingest_generation":generation,"from_sequence":from,"through_sequence":c.through,"records":records,"next_cursor":next,"end":!more});
    tx.commit()?;
    Ok(result)
}
