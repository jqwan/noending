use super::{CliError, CliResult};
use crate::storage::{
    message_query::{self, HistoryMessage, PART_SIZE},
    Db,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rmcp::schemars::{self, JsonSchema};
use rusqlite::OptionalExtension;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    /// NoEnding session ID.
    pub id: String,
    /// Case-sensitive text to locate in this session. Returns snippets.
    pub query: Option<String>,
    /// Inclusive first message ordinal, starting at 1.
    #[schemars(range(min = 1))]
    pub from: Option<i64>,
    /// Inclusive last message ordinal. A range is read oldest first.
    #[schemars(range(min = 1))]
    pub to: Option<i64>,
    /// Pass next_cursor with the session ID to continue the original query.
    pub cursor: Option<String>,
    /// Pass next_content_cursor with the session ID to continue one long message.
    pub content_cursor: Option<String>,
    /// Number of messages per page, default 8, maximum 50.
    #[schemars(range(min = 1, max = 50))]
    pub limit: Option<usize>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PageCursor {
    session: String,
    generation: i64,
    first: i64,
    last: i64,
    next: i64,
    newest_first: bool,
    query: Option<String>,
    limit: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContentCursor {
    session: String,
    generation: i64,
    sequence: i64,
    message_id: String,
    offset: i64,
}

pub(super) fn page(db: &Db, request: Request) -> CliResult<Value> {
    let Request {
        id,
        query,
        from,
        to,
        cursor,
        content_cursor,
        limit,
    } = request;
    let first = from.unwrap_or(1);
    if first < 1
        || to.is_some_and(|last| last < first)
        || limit.is_some_and(|limit| !(1..=50).contains(&limit))
        || query.as_deref().is_some_and(|text| text.trim().is_empty())
        || (cursor.is_some() && content_cursor.is_some())
        || ((cursor.is_some() || content_cursor.is_some())
            && (query.is_some() || from.is_some() || to.is_some() || limit.is_some()))
    {
        return Err(CliError::new(
            "invalid_arguments",
            "无效的消息范围、搜索内容或分页大小（1～50）；使用游标时只需提供会话 ID 和一个游标",
        ));
    }

    let conn = db.read();
    let tx = conn.unchecked_transaction()?;
    let state: Option<(i64, i64)> = tx
        .query_row(
            "SELECT fact_generation,latest_message_seq FROM sessions WHERE id=?1",
            [&id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let (generation, latest) = state.ok_or_else(|| CliError::new("not_found", "会话不存在"))?;

    if let Some(token) = content_cursor {
        let cursor: ContentCursor = decode_cursor(&token)?;
        if cursor.session != id
            || cursor.sequence < 1
            || cursor.offset < PART_SIZE
            || cursor.offset % PART_SIZE != 0
        {
            return Err(invalid_cursor());
        }
        if cursor.generation != generation {
            return Err(stale_cursor());
        }
        let row = message_query::history_content(
            &tx,
            &id,
            cursor.sequence,
            &cursor.message_id,
            cursor.offset,
        )?
        .filter(|message| !message.content.is_empty())
        .ok_or_else(invalid_cursor)?;
        let message = message_body(row, &id, generation, cursor.offset)?;
        return Ok(json!({"session_id": id, "message": message}));
    }

    let mut page = if let Some(token) = cursor {
        let cursor: PageCursor = decode_cursor(&token)?;
        if cursor.session != id
            || cursor.first < 1
            || !(cursor.first..=cursor.last).contains(&cursor.next)
            || !(1..=50).contains(&cursor.limit)
        {
            return Err(invalid_cursor());
        }
        if cursor.generation != generation || cursor.last > latest {
            return Err(stale_cursor());
        }
        cursor
    } else {
        let last = to.unwrap_or(latest).min(latest);
        let newest_first = from.is_none() && to.is_none();
        PageCursor {
            session: id.clone(),
            generation,
            first,
            last,
            next: if newest_first { last } else { first },
            newest_first,
            query,
            limit: limit.unwrap_or(8),
        }
    };
    let range = if page.newest_first {
        page.first..=page.next
    } else {
        page.next..=page.last
    };
    let mut rows = message_query::history_page(
        &tx,
        &id,
        range,
        page.newest_first,
        page.query.as_deref(),
        page.limit + 1,
    )?;
    let next_cursor = if rows.len() > page.limit {
        page.next = rows.pop().unwrap().sequence;
        Some(encode_cursor(&page)?)
    } else {
        None
    };
    if page.newest_first {
        rows.reverse();
    }
    if page.query.is_some() {
        let items: Vec<_> = rows
            .into_iter()
            .map(|row| {
                json!({"message_id":row.id,"sequence":row.sequence,"role":row.role,
                       "timestamp":row.timestamp,"snippet":row.content})
            })
            .collect();
        Ok(
            json!({"session_id":id,"through_sequence":page.last,"items":items,
                  "next_cursor":next_cursor}),
        )
    } else {
        let messages = rows
            .into_iter()
            .map(|row| message_body(row, &id, generation, 0))
            .collect::<CliResult<Vec<_>>>()?;
        Ok(
            json!({"session_id":id,"through_sequence":page.last,"messages":messages,
                  "next_cursor":next_cursor}),
        )
    }
}

fn message_body(
    mut row: HistoryMessage,
    session: &str,
    generation: i64,
    offset: i64,
) -> CliResult<Value> {
    let truncated = row.content.chars().count() > PART_SIZE as usize;
    let next_content_cursor = if truncated {
        row.content.pop();
        Some(encode_cursor(&ContentCursor {
            session: session.into(),
            generation,
            sequence: row.sequence,
            message_id: row.id.clone(),
            offset: offset + PART_SIZE,
        })?)
    } else {
        None
    };
    Ok(
        json!({"message_id":row.id,"sequence":row.sequence,"role":row.role,
              "timestamp":row.timestamp,"content":row.content,"truncated":truncated,
              "next_content_cursor":next_content_cursor}),
    )
}

fn encode_cursor(value: &impl Serialize) -> CliResult<String> {
    Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(value)?))
}

fn decode_cursor<T: DeserializeOwned>(token: &str) -> CliResult<T> {
    let bytes = URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| invalid_cursor())?;
    serde_json::from_slice(&bytes).map_err(|_| invalid_cursor())
}

fn invalid_cursor() -> CliError {
    CliError::new("invalid_cursor", "游标无效或不属于此会话")
}

fn stale_cursor() -> CliError {
    CliError::new("stale_cursor", "消息历史已变化，请重新查询")
}
