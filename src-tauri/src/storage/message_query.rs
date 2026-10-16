//! Bounded reads of the current message projection. Never returns retired rows.
use crate::error::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
pub const PART_SIZE: i64 = 2000;

/// Projection offsets are rebuilt atomically on a rewrite and extended only
/// for new messages on append. Two ordinal lookups replace a prose-wide SUM.
fn part_end(conn: &Connection, id: &str, ordinal: i64) -> Result<i64> {
    Ok(conn
        .query_row(
            "SELECT part_end FROM session_message_projection
             WHERE session_id=?1 AND ordinal<=?2 ORDER BY ordinal DESC LIMIT 1",
            params![id, ordinal],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0))
}

pub fn part_count(conn: &Connection, id: &str, from: i64, through: i64) -> Result<usize> {
    Ok((part_end(conn, id, through)? - part_end(conn, id, from)?).max(0) as usize)
}

/// Virtual cursors start at 1, leaving cursor 0 for extraction's header. Search
/// examines the full original message; returned chunks never cut UTF-8 bytes.
/// The indexed cumulative offset seeks directly to the requested chunk. Each
/// selected message supplies one bounded substring for all its chunks on this
/// page, avoiding both a full prefix scan and a separate SQL query per chunk.
pub fn parts(
    conn: &Connection,
    id: &str,
    from: i64,
    through: i64,
    cursor: usize,
    count: usize,
    search: Option<&str>,
) -> Result<Vec<Value>> {
    if count == 0 || through <= from {
        return Ok(Vec::new());
    }
    let offset = part_end(conn, id, from)?;
    let absolute_cursor = offset + (cursor as i64).max(1);
    let through_part = part_end(conn, id, through)?;
    if absolute_cursor > through_part {
        return Ok(Vec::new());
    }
    let mut st = conn.prepare(
        "SELECT p.ordinal,m.id,m.role,m.ts,p.part_end,
                COALESCE(prev.part_end,0),
                SUBSTR(m.content, MAX(0,?4-COALESCE(prev.part_end,0)-1)*2000+1, ?5*2000)
         FROM session_message_projection p
         JOIN session_messages m ON m.id=p.session_message_id
         LEFT JOIN session_message_projection prev
           ON prev.session_id=p.session_id AND prev.ordinal=p.ordinal-1
         WHERE p.session_id=?1 AND p.ordinal>?2 AND p.ordinal<=?3
           AND p.part_end>=?4 AND p.part_end<=?7
           AND (?6 IS NULL OR INSTR(m.content,?6)>0)
         ORDER BY p.part_end LIMIT ?5",
    )?;
    let rows = st.query_map(
        params![
            id,
            from,
            through,
            absolute_cursor,
            count as i64,
            search,
            through_part
        ],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, String>(6)?,
            ))
        },
    )?;
    let mut records = Vec::new();
    'messages: for row in rows {
        let (seq, message_id, role, ts, end, base, text) = row?;
        let total = end - base;
        let first = (absolute_cursor - base - 1).max(0);
        let mut chars = text.chars();
        for part in first..total {
            if records.len() == count {
                break 'messages;
            }
            let content: String = chars.by_ref().take(PART_SIZE as usize).collect();
            records.push(json!({"cursor":base-offset+part+1,"record":{"type":"message","message_id":message_id,"sequence":seq,"role":role,"timestamp":ts,"part":part+1,"last_part":part+1==total,"content":content}}));
        }
    }
    Ok(records)
}
