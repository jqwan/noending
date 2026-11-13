//! Scoped database reads. Guards contain only identities and progress; bodies
//! are selected when the Agent requests a page, never exported or cached.
use super::{stale, SessionSnapshot};
use crate::error::{other, Result};
use crate::storage::Db;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub(super) enum Input<'a> {
    Collaboration {
        snapshot: &'a super::collaboration::Snapshot,
    },
    Session {
        snapshot: &'a SessionSnapshot,
        include_summary: bool,
    },
    Batch {
        id: &'a str,
        header: &'a Value,
        revision: i64,
        regenerate: bool,
        sessions: &'a [SessionSnapshot],
    },
}

pub(super) fn message_time(conn: &Connection, id: &str, ordinal: i64) -> Result<Option<String>> {
    Ok(conn.query_row("SELECT m.ts FROM session_message_projection p JOIN session_messages m ON m.id=p.session_message_id WHERE p.session_id=?1 AND p.ordinal=?2", params![id,ordinal], |r| r.get(0)).optional()?.flatten())
}

pub(super) struct TaskState {
    pub context_revision: i64,
    pub input_revision: i64,
    pub consumed_input_revision: i64,
    pub through_seq: i64,
}
pub(super) fn task_state(conn: &Connection, id: &str) -> Result<TaskState> {
    Ok(conn.query_row("SELECT context_revision,input_revision,consumed_input_revision,processed_through_seq FROM workstream_context_state WHERE workstream_id=?1", [id], |r| Ok(TaskState { context_revision:r.get(0)?, input_revision:r.get(1)?, consumed_input_revision:r.get(2)?, through_seq:r.get(3)? })).optional()?.unwrap_or(TaskState {context_revision:0,input_revision:0,consumed_input_revision:0, through_seq:0}))
}
pub(super) fn target_summary(
    conn: &Connection,
    kind: &str,
    id: &str,
    revision: i64,
) -> Result<Option<crate::domain::Summary>> {
    let sql = match kind {
        "session" => "SELECT revision,summary FROM session_contexts WHERE session_id=?1",
        "task" => {
            "SELECT context_revision,summary FROM workstream_context_state WHERE workstream_id=?1"
        }
        "project" => {
            "SELECT context_revision,summary FROM project_context_state WHERE project_id=?1"
        }
        _ => return Err(other("未知提取目标")),
    };
    let row: Option<(i64, crate::domain::Summary)> = conn
        .query_row(sql, [id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    if row.as_ref().map_or(0, |r| r.0) != revision {
        return Err(stale());
    }
    Ok(row.map(|r| r.1).filter(|s| !s.is_empty()))
}

impl Input<'_> {
    pub fn count(&self, conn: &Connection) -> Result<usize> {
        match self {
            Self::Collaboration { snapshot } => snapshot.count(conn),
            Self::Session { snapshot, .. } => {
                let from = snapshot.from;
                Ok(crate::storage::message_query::part_count(
                    conn,
                    &snapshot.session.id,
                    from,
                    snapshot.latest,
                )? + 2)
            }
            Self::Batch { sessions, .. } => Ok(sessions.len() + 2),
        }
    }
    pub fn page(
        &self,
        db: &Db,
        cursor: usize,
        limit: usize,
        search: Option<&str>,
    ) -> Result<Value> {
        let conn = db.read();
        // Short read transaction makes each page consistent. No transaction is
        // held while the Agent thinks or invokes its next command.
        let tx = conn.unchecked_transaction()?;
        let total = self.count(&tx)?;
        if cursor > total {
            return Err(other("游标超出输入范围"));
        }
        let value = match self {
            Self::Collaboration { snapshot } => snapshot.page(&tx, cursor, limit, search)?,
            Self::Session {
                snapshot,
                include_summary,
            } => {
                snapshot.validate(&tx)?;
                session_page(
                    &tx,
                    snapshot,
                    *include_summary,
                    cursor,
                    limit,
                    search,
                    total,
                )?
            }
            Self::Batch {
                id,
                header,
                revision,
                regenerate,
                sessions,
            } => {
                if search.is_some() {
                    return Err(other("搜索仅用于消息输入"));
                }
                let summary = target_summary(&tx, "project", id, *revision)?;
                for s in *sessions {
                    s.validate(&tx)?;
                }
                let end = (cursor + limit).min(total);
                let mut records = Vec::new();
                for position in cursor..end {
                    let record = if position == 0 {
                        let mut header = (*header).clone();
                        header["current_summary"] = if *regenerate {
                            Value::Null
                        } else {
                            json!(summary)
                        };
                        header["item_identities"] =
                            json!(summary.as_ref().map(|s| s.identities()).unwrap_or_default());
                        header
                    } else if position == total - 1 {
                        json!({"type":"end","marker":"END_OF_PROJECT_INPUT"})
                    } else {
                        let s = &sessions[position - 1];
                        let current = target_summary(&tx, "session", &s.session.id, s.revision())?;
                        json!({"type":"session","session_id":s.session.id,"project_id":s.session.project_id,"title":s.session.title,
                            "started_at":s.session.started_at,"last_message_at":s.last_message_at(),"summary_through_at":s.summary_through_at,
                            "current_summary":current,
                            "item_identities":current.as_ref().map(|s|s.identities()).unwrap_or_default()})
                    };
                    records.push(json!({"cursor":position,"record":record}));
                }
                json!({"records":records,"next_cursor":(end<total).then_some(end),"end":end==total,"total_records":total})
            }
        };
        tx.commit()?;
        Ok(value)
    }
}

fn session_page(
    conn: &Connection,
    s: &SessionSnapshot,
    include_summary: bool,
    cursor: usize,
    limit: usize,
    search: Option<&str>,
    total: usize,
) -> Result<Value> {
    let from = s.from;
    let mut records = Vec::new();
    if search.is_none() && cursor == 0 {
        let current = target_summary(conn, "session", &s.session.id, s.revision())?;
        records.push(json!({"cursor":0,"record":{"type":"session","session_id":s.session.id,"project_id":s.session.project_id,"title":s.session.title,
            "working_directory":s.session.cwd,"agent":s.session.agent,"current_summary":if include_summary {current.as_ref()} else {None},
            "item_identities":current.as_ref().map(|s|s.identities()).unwrap_or_default(),
            "started_at":s.session.started_at,"last_message_at":s.last_message_at(),"summary_through_at":s.summary_through_at.as_deref(),
            "from_sequence":from+1,"through_sequence":s.latest,"message_count":s.latest-from}}));
    }
    records.extend(crate::storage::message_query::parts(
        conn,
        &s.session.id,
        from,
        s.latest,
        cursor,
        limit + 1,
        search,
    )?);
    if search.is_none() && records.len() <= limit && cursor <= total - 1 {
        records.push(
            json!({"cursor":total-1,"record":{"type":"end","marker":"END_OF_SESSION_INPUT"}}),
        );
    }
    let has_more = records.len() > limit;
    records.truncate(limit);
    let next = has_more.then(|| records.last().unwrap()["cursor"].as_u64().unwrap() as usize + 1);
    Ok(json!({"records":records,"next_cursor":next,"end":!has_more,"total_records":total}))
}
