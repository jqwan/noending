//! Bounded reads of the current message projection. Never returns retired rows.
use crate::error::Result;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
pub const PART_SIZE: i64 = 2000;
pub fn part_count(conn: &Connection, id: &str, from: i64, through: i64) -> Result<usize> {
    Ok(conn.query_row("SELECT COALESCE(SUM(MAX(1,(LENGTH(m.content)+1999)/2000)),0) FROM session_message_projection p JOIN session_messages m ON m.id=p.session_message_id WHERE p.session_id=?1 AND p.ordinal>?2 AND p.ordinal<=?3",params![id,from,through],|r|r.get::<_,i64>(0))? as usize)
}
/// Virtual cursors start at 1, leaving cursor 0 for extraction's header. Search
/// examines the full original message; returned chunks never cut UTF-8 bytes.
pub fn parts(
    conn: &Connection,
    id: &str,
    from: i64,
    through: i64,
    cursor: usize,
    count: usize,
    search: Option<&str>,
) -> Result<Vec<Value>> {
    let mut st=conn.prepare("WITH sizes AS (
        SELECT p.ordinal,m.id,m.role,m.ts,MAX(1,(LENGTH(m.content)+1999)/2000) AS parts
        FROM session_message_projection p JOIN session_messages m ON m.id=p.session_message_id
        WHERE p.session_id=?1 AND p.ordinal>?2 AND p.ordinal<=?3
    ), numbered AS (
        SELECT *,COALESCE(SUM(parts) OVER (ORDER BY ordinal ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING),0) AS base FROM sizes
    ) SELECT n.ordinal,n.id,n.role,n.ts,n.parts,n.base FROM numbered n
      JOIN session_messages m ON m.id=n.id
      WHERE n.base+n.parts>=?4 AND (?5 IS NULL OR INSTR(m.content,?5)>0)
      ORDER BY n.ordinal LIMIT ?6")?;
    let rows = st.query_map(
        params![id, from, through, cursor as i64, search, count as i64],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
            ))
        },
    )?;
    let mut records = Vec::new();
    'messages: for row in rows {
        let (seq, message_id, role, ts, total, base) = row?;
        let first = (cursor as i64 - base - 1).max(0);
        for part in first..total {
            if records.len() == count {
                break 'messages;
            }
            let content: String = conn.query_row(
                "SELECT SUBSTR(content,?2,?3) FROM session_messages WHERE id=?1",
                params![message_id, part * PART_SIZE + 1, PART_SIZE],
                |r| r.get(0),
            )?;
            records.push(json!({"cursor":base+part+1,"record":{"type":"message","message_id":message_id,"sequence":seq,"role":role,"timestamp":ts,"part":part+1,"last_part":part+1==total,"content":content}}));
        }
    }
    Ok(records)
}
