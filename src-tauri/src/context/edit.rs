//! Explicit summary edits change the text/revision, never extraction frontiers.
use crate::domain::Summary;
use crate::error::{other, Result};
use crate::storage::{context_repo as repo, project_context_repo, Db};
use rusqlite::params;

#[derive(serde::Serialize)]
pub struct SummaryLockResult {
    pub revision: i64,
    pub summary: Summary,
}
pub fn edit_summary(
    db: &Db,
    kind: &str,
    id: &str,
    expected_revision: i64,
    text: &str,
) -> Result<i64> {
    let summary: Summary = serde_json::from_str(text)?;
    let summary = summary.validated()?;
    mutate_summary(db, kind, id, expected_revision, |previous| {
        summary.validate_title_locks(&previous)?;
        Ok(summary)
    })
    .map(|result| result.revision)
}

pub fn set_summary_title_lock(
    db: &Db,
    kind: &str,
    id: &str,
    expected_revision: i64,
    item_number: usize,
    locked: bool,
) -> Result<SummaryLockResult> {
    mutate_summary(db, kind, id, expected_revision, |mut summary| {
        let index = item_number
            .checked_sub(1)
            .ok_or_else(|| other("事项序号从 1 开始"))?;
        let item = summary
            .items
            .get_mut(index)
            .ok_or_else(|| other("事项不存在"))?;
        item.title_locked = locked;
        Ok(summary)
    })
}

fn mutate_summary(
    db: &Db,
    kind: &str,
    id: &str,
    expected_revision: i64,
    change: impl FnOnce(Summary) -> Result<Summary>,
) -> Result<SummaryLockResult> {
    if expected_revision < 0 {
        return Err(other("需要当前摘要版本"));
    }
    db.tx(|tx| {
        let table=match kind {"session"=>"sessions","task"=>"workstreams","project"=>"projects",_=>return Err(other("摘要类型无效"))};
        let exists:bool=tx.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id=?1)"),[id],|r|r.get(0))?;
        if !exists {return Err(other("摘要目标不存在"));}
        let revision=match kind {
            "session"=>repo::get_session_context_conn(tx,id)?.map_or(0,|c|c.revision),
            "task"=>repo::get_workstream_context_state_conn(tx,id)?.context_revision,
            _=>project_context_repo::state(tx,id)?.revision,
        };
        if revision!=expected_revision {return Err(super::stale());}
        let previous = super::query::target_summary(tx,kind,id,revision)?.unwrap_or_default();
        let summary = change(previous)?;
        match kind {
            "session"=> {
                let existing=repo::get_session_context_conn(tx,id)?;
                let generation=repo::get_ingest_state_conn(tx,id)?.0;
                repo::commit_session_context_conn(tx,id,&crate::domain::SessionContextFields{summary:summary.clone()},revision,existing.as_ref().map_or(generation,|c|c.ingest_generation),existing.as_ref().map_or(0,|c|c.processed_through_seq))?;
            },
            "task"=> {
                repo::ensure_workstream_state_conn(tx,id)?;
                tx.execute("UPDATE workstream_context_state SET summary=?2,context_revision=context_revision+1,updated_at=?3 WHERE workstream_id=?1",params![id,summary,crate::storage::now()])?;

            },
            _=> {
                tx.execute("INSERT INTO project_context_state(project_id,summary,context_revision,updated_at) VALUES (?1,?2,1,?3) ON CONFLICT(project_id) DO UPDATE SET summary=?2,context_revision=context_revision+1,updated_at=?3",params![id,summary,crate::storage::now()])?;

            },
        }
        Ok(SummaryLockResult { revision:revision+1, summary })
    })
}
