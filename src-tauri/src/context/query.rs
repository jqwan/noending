//! Scoped database reads. Guards contain only identities and progress; bodies
//! are selected when the Agent requests a page, never exported or cached.
use super::{preferences, stale, summary::ExtractionFiles, SessionSnapshot};
use crate::error::{other, Result};
use crate::storage::Db;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

#[derive(Clone)]
pub(super) enum Input {
    Session {
        snapshot: SessionSnapshot,
        include_summary: bool,
        full_history: bool,
    },
    Batch {
        kind: String,
        id: String,
        header: Value,
        revision: i64,
        regenerate: bool,
        sessions: Vec<SessionSnapshot>,
    },
    Preferences(Vec<String>),
}

pub(super) fn message_time(conn: &Connection, id: &str, ordinal: i64) -> Result<Option<String>> {
    Ok(conn.query_row("SELECT m.ts FROM session_message_projection p JOIN session_messages m ON m.id=p.session_message_id WHERE p.session_id=?1 AND p.ordinal=?2", params![id,ordinal], |r| r.get(0)).optional()?.flatten())
}

pub(super) struct TaskState {
    pub context_revision: i64,
    pub input_revision: i64,
    pub consumed_input_revision: i64,
}
pub(super) fn task_state(conn: &Connection, id: &str) -> Result<TaskState> {
    Ok(conn.query_row("SELECT context_revision,input_revision,consumed_input_revision FROM workstream_context_state WHERE workstream_id=?1", [id], |r| Ok(TaskState { context_revision:r.get(0)?, input_revision:r.get(1)?, consumed_input_revision:r.get(2)? })).optional()?.unwrap_or(TaskState {context_revision:0,input_revision:0,consumed_input_revision:0}))
}
pub(super) struct ProjectState {
    pub revision: i64,
    pub input_fingerprint: String,
}
pub(super) fn project_state(conn: &Connection, id: &str) -> Result<ProjectState> {
    Ok(conn.query_row("SELECT context_revision,input_fingerprint FROM project_context_state WHERE project_id=?1", [id], |r| Ok(ProjectState {revision:r.get(0)?,input_fingerprint:r.get(1)?})).optional()?.unwrap_or(ProjectState {revision:0,input_fingerprint:String::new()}))
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

impl Input {
    pub fn count(&self, conn: &Connection) -> Result<usize> {
        match self {
            Self::Session {
                snapshot,
                full_history,
                ..
            } => {
                snapshot.validate(conn, None)?;
                let from = if *full_history { 0 } else { snapshot.from };
                Ok(crate::storage::message_query::part_count(
                    conn,
                    &snapshot.session.id,
                    from,
                    snapshot.latest,
                )? + 2)
            }
            Self::Batch { sessions, .. } => Ok(sessions.len() + 2),
            Self::Preferences(_) => Ok(1),
        }
    }
    pub fn page(
        &self,
        db: &Db,
        files: &ExtractionFiles,
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
            Self::Session {
                snapshot,
                include_summary,
                full_history,
            } => session_page(
                &tx,
                snapshot,
                *include_summary,
                *full_history,
                cursor,
                limit,
                search,
                total,
            )?,
            Self::Preferences(projects) => {
                if search.is_some() {
                    return Err(other("搜索仅用于消息输入"));
                }
                let records = if cursor == 0 {
                    vec![
                        json!({"cursor":0,"record":preferences::Snapshot::read_conn(&tx,projects.clone())?}),
                    ]
                } else {
                    vec![]
                };
                json!({"records":records,"next_cursor":null,"end":true,"total_records":1})
            }
            Self::Batch {
                kind,
                id,
                header,
                revision,
                regenerate,
                sessions,
            } => {
                if search.is_some() {
                    return Err(other("搜索仅用于消息输入"));
                }
                let summary = target_summary(&tx, kind, id, *revision)?;
                for s in sessions {
                    s.validate(&tx, None)?;
                }
                let end = (cursor + limit).min(total);
                let mut records = Vec::new();
                for position in cursor..end {
                    let record = if position == 0 {
                        let mut header = header.clone();
                        header["current_summary"] = if *regenerate {
                            Value::Null
                        } else {
                            json!(summary)
                        };
                        header
                    } else if position == total - 1 {
                        json!({"type":"end","marker":format!("END_OF_{}_INPUT",kind.to_uppercase())})
                    } else {
                        let s = &sessions[position - 1];
                        let need_update = s.latest > s.from;
                        json!({"type":"session","session_id":s.session.id,"project_id":s.session.project_id,"title":s.session.title,
                            "started_at":s.session.started_at,"last_message_at":s.last_message_at(),"summary_through_at":s.summary_through_at,
                            "need_update":need_update,"current_summary":target_summary(&tx,"session",&s.session.id,s.revision())?,
                            "preference_input_file":regenerate.then(||files.preference_session_input(&s.session.id)),
                            "input_file":need_update.then(||files.session_input(&s.session.id)),
                            "result_file":need_update.then(||files.session_result(&s.session.id))})
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
    full_history: bool,
    cursor: usize,
    limit: usize,
    search: Option<&str>,
    total: usize,
) -> Result<Value> {
    let from = if full_history { 0 } else { s.from };
    let mut records = Vec::new();
    if search.is_none() && cursor == 0 {
        records.push(json!({"cursor":0,"record":{"type":"session","session_id":s.session.id,"project_id":s.session.project_id,"title":s.session.title,
            "working_directory":s.session.cwd,"agent":s.session.agent,"current_summary":if include_summary {target_summary(conn,"session",&s.session.id,s.revision())?} else {None},
            "started_at":s.session.started_at,"last_message_at":s.last_message_at(),"summary_through_at":if full_history {None} else {s.summary_through_at.as_deref()},
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
