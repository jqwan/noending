//! Explicit summary edits change the text/revision, never extraction frontiers.
use crate::error::{other, Result};
use crate::storage::{context_repo as repo, project_context_repo, Db};
use rusqlite::params;
pub fn edit_summary(
    db: &Db,
    kind: &str,
    id: &str,
    expected_revision: i64,
    text: &str,
) -> Result<i64> {
    if expected_revision < 0 || text.trim().is_empty() {
        return Err(other("需要当前摘要版本和非空 Markdown 正文"));
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
        match kind {
            "session"=> {
                let existing=repo::get_session_context_conn(tx,id)?;
                let generation=repo::get_ingest_state_conn(tx,id)?.0;
                repo::commit_session_context_conn(tx,id,&crate::domain::SessionContextFields{summary:text.trim().into()},revision,existing.as_ref().map_or(generation,|c|c.ingest_generation),existing.as_ref().map_or(0,|c|c.processed_through_seq))?;
            },
            "task"=> {
                repo::ensure_workstream_state_conn(tx,id)?;
                tx.execute("UPDATE workstream_context_state SET summary=?2,context_revision=context_revision+1,updated_at=?3 WHERE workstream_id=?1",params![id,text.trim(),crate::storage::now()])?;
                crate::storage::workstream_projects::reindex_workstream_search_conn(tx,id)?;
            },
            _=> {
                tx.execute("INSERT INTO project_context_state(project_id,summary,context_revision,updated_at) VALUES (?1,?2,1,?3) ON CONFLICT(project_id) DO UPDATE SET summary=?2,context_revision=context_revision+1,updated_at=?3",params![id,text.trim(),crate::storage::now()])?;
                project_context_repo::reindex(tx,id)?;
            },
        }
        Ok(revision+1)
    })
}
