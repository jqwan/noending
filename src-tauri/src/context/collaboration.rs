//! A task's shared collaboration memory. Room messages are append-only and
//! bounded by one sequence cursor across runs; member transcripts are untouched.
use super::*;
use crate::storage::context_repo::{
    commit_workstream_summary_conn, get_workstream_context_state_conn,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub(super) fn guard(conn: &Connection, id: &str) -> Result<Value> {
    let task = crate::storage::get_workstream_conn(conn, id)?.ok_or_else(stale)?;
    let state = query::task_state(conn, id)?;
    let room: Option<String> = conn
        .query_row(
            "SELECT id FROM collaboration_rooms WHERE task_id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    let mut st =
        conn.prepare("SELECT id,title,description FROM task_items WHERE task_id=?1 ORDER BY id")?;
    let items = st
        .query_map([id], |row| {
            Ok(json!({
                "id":row.get::<_,String>(0)?,
                "title":row.get::<_,String>(1)?,
                "description":row.get::<_,String>(2)?,
            }))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(
        json!({"task_id":id,"title":task.title,"description":task.description,"task_items":items,
        "projects":task_projects(conn,id)?,"room_id":room,"revision":state.context_revision,"input_revision":state.input_revision}),
    )
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Snapshot {
    id: String,
    guard: Value,
    from: i64,
    through: i64,
    regenerate: bool,
}
impl Snapshot {
    pub(super) fn read(conn: &Connection, id: &str, regenerate: bool) -> Result<Self> {
        let guard = guard(conn, id)?;
        let state = query::task_state(conn, id)?;
        let through = conn.query_row("SELECT COALESCE(MAX(seq),0) FROM collaboration_messages WHERE room_id=(SELECT id FROM collaboration_rooms WHERE task_id=?1)",[id],|r|r.get(0))?;
        Ok(Self {
            id: id.into(),
            guard,
            from: if regenerate { 0 } else { state.through_seq },
            through,
            regenerate,
        })
    }
    pub(super) fn validate(&self, conn: &Connection) -> Result<()> {
        if guard(conn, &self.id)? != self.guard {
            return Err(stale());
        }
        Ok(())
    }
    pub(super) fn count(&self, conn: &Connection) -> Result<usize> {
        self.validate(conn)?;
        Ok(conn.query_row("SELECT COALESCE(SUM(MAX(1,(LENGTH(body)+1999)/2000)),0) FROM collaboration_messages WHERE room_id=(SELECT id FROM collaboration_rooms WHERE task_id=?1) AND seq>?2 AND seq<=?3",params![self.id,self.from,self.through],|r|r.get::<_,i64>(0))? as usize + 2)
    }
    pub(super) fn page(
        &self,
        conn: &Connection,
        cursor: usize,
        limit: usize,
        search: Option<&str>,
    ) -> Result<Value> {
        let total = self.count(conn)?;
        let mut records = Vec::new();
        if cursor == 0 && search.is_none() {
            let previous = query::target_summary(
                conn,
                "task",
                &self.id,
                self.guard["revision"].as_i64().unwrap(),
            )?;
            let mut header = self.guard.clone();
            header["type"] = json!("collaboration");
            header["current_summary"] = if self.regenerate {
                Value::Null
            } else {
                json!(previous)
            };
            header["item_identities"] = json!(previous
                .as_ref()
                .map(|s| s.identities())
                .unwrap_or_default());
            header["from_sequence"] = json!(self.from + 1);
            header["through_sequence"] = json!(self.through);
            records.push(json!({"cursor":0,"record":header}));
        }
        let count = limit + 1;
        // Calculate virtual chunk positions without exporting complete bodies.
        // Filtering after the window preserves cursors during search.
        let mut st = conn.prepare("WITH parts AS (
          SELECT *, MAX(1,(LENGTH(body)+1999)/2000) AS n,
            SUM(MAX(1,(LENGTH(body)+1999)/2000)) OVER (ORDER BY seq) AS part_end
          FROM collaboration_messages WHERE room_id=(SELECT id FROM collaboration_rooms WHERE task_id=?1) AND seq>?2 AND seq<=?3
        ) SELECT id,run_id,seq,sender_session_id,kind,created_at,n,part_end,
            SUBSTR(body,MAX(0,?4-(part_end-n)-1)*2000+1,?5*2000)
          FROM parts WHERE part_end>=?4 AND (?6 IS NULL OR INSTR(body,?6)>0)
          ORDER BY seq LIMIT ?5")?;
        let rows = st.query_map(
            params![
                self.id,
                self.from,
                self.through,
                cursor.max(1) as i64,
                count as i64,
                search
            ],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, String>(8)?,
                ))
            },
        )?;
        'messages: for row in rows {
            let (id, run, seq, sender, kind, ts, n, end, text) = row?;
            let base = end - n;
            let first = (cursor.max(1) as i64 - base - 1).max(0);
            let mut chars = text.chars();
            for part in first..n {
                if records.len() == count {
                    break 'messages;
                }
                let content: String = chars.by_ref().take(2000).collect();
                records.push(json!({"cursor":base+part+1,"record":{"type":"message","message_id":id,"run_id":run,"sequence":seq,"sender_session_id":sender,"kind":kind,"timestamp":ts,"part":part+1,"last_part":part+1==n,"content":content}}));
            }
        }
        if search.is_none() && records.len() <= limit && cursor < total {
            records.push(json!({"cursor":total-1,"record":{"type":"end","marker":"END_OF_COLLABORATION_INPUT"}}));
        }
        let more = records.len() > limit;
        records.truncate(limit);
        let next = more.then(|| records.last().unwrap()["cursor"].as_u64().unwrap() + 1);
        Ok(json!({"records":records,"next_cursor":next,"end":!more,"total_records":total}))
    }
}

pub(super) fn freshness(conn: &Connection, id: &str) -> Result<freshness::SummaryFreshness> {
    let snapshot = Snapshot::read(conn, id, false)?;
    let state = query::task_state(conn, id)?;
    let pending_messages:i64=conn.query_row("SELECT COUNT(*) FROM collaboration_messages WHERE room_id=(SELECT id FROM collaboration_rooms WHERE task_id=?1) AND seq>?2",params![id,state.through_seq],|r|r.get(0))?;
    let refreshed_at = conn
        .query_row(
            "SELECT refreshed_at FROM workstream_context_state WHERE workstream_id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let scope_changed =
        snapshot.through > 0 && state.input_revision != state.consumed_input_revision;
    Ok(freshness::SummaryFreshness {
        refreshed_at,
        pending_messages: Some(pending_messages),
        scope_changed,
        pending: pending_messages > 0 || scope_changed,
        ..Default::default()
    })
}
pub(super) fn view(db: &Db, id: &str) -> Result<WorkstreamContextView> {
    let conn = db.read();
    let tx = conn.unchecked_transaction()?;
    let state = get_workstream_context_state_conn(&tx, id)?;
    let freshness = freshness(&tx, id)?;
    let view = WorkstreamContextView {
        workstream_id: id.into(),
        summary: (!state.summary.is_empty()).then_some(state.summary),
        context_revision: state.context_revision,
        updated_at: state.updated_at,
        pending: freshness.pending,
        pending_messages: freshness.pending_messages.unwrap_or(0),
        processed_through_seq: state.processed_through_seq,
        freshness,
    };
    tx.commit()?;
    Ok(view)
}

impl Snapshot {
    pub(super) fn needs_update(&self, db: &Db) -> Result<bool> {
        Ok(self.through > 0 && (self.regenerate || freshness(&db.read(), &self.id)?.pending))
    }

    pub(super) fn commit(
        &self,
        db: &Db,
        summary: &crate::domain::Summary,
        status: ContextUpdateStatus,
    ) -> Result<WorkstreamUpdateOutcome> {
        let revision = db.tx(|tx| {
            self.validate(tx)?;
            commit_workstream_summary_conn(tx, &self.id, summary, self.through)
        })?;
        let after = view(db, &self.id)?;
        Ok(WorkstreamUpdateOutcome {
            workstream_id: self.id.clone(),
            status,
            context_revision: revision,
            processed_through_seq: self.through,
            remaining_pending: after.pending_messages,
        })
    }

    pub(super) fn revision(&self) -> i64 {
        self.guard["revision"].as_i64().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::tests::{append, submit_reply, submit_result, ServiceFixture};
    use std::path::Path;

    pub(super) fn room(f: &ServiceFixture) -> String {
        let task =
            crate::workspace::workstream::create_workstream(f.db(), "协作目标", "范围与验收", &[])
                .unwrap();
        let timestamp = crate::storage::now();
        f.db().write().execute("INSERT INTO collaboration_rooms(id,task_id,selected_members,created_at,updated_at) VALUES(?1,?1,'[]',?2,?2)",params![task.id,timestamp]).unwrap();
        f.db().write().execute("INSERT INTO collaboration_runs(id,room_id,status,started_at) VALUES(?1,?1,'active',?2)",params![task.id,timestamp]).unwrap();
        task.id
    }
    pub(super) fn message(f: &ServiceFixture, id: &str, text: &str) -> i64 {
        f.db().write().query_row("INSERT INTO collaboration_messages(id,room_id,run_id,seq,kind,body,created_at) SELECT ?2,?1,?1,COALESCE(MAX(seq),0)+1,'text',?3,?4 FROM collaboration_messages WHERE room_id=?1 RETURNING seq",params![id,crate::storage::new_id(),text,crate::storage::now()],|r|r.get(0)).unwrap()
    }
    fn result(dir: &Path, id: &str, title: &str) -> crate::cli::CliResult<Value> {
        submit_result(
            dir,
            "task",
            id,
            json!({"summary":{"items":[{"title":title,"description":"","details":[]}]}}),
            false,
        )
    }
    fn read(dir: &Path, id: &str) -> Vec<Value> {
        tools::read_input(&dir.join(format!("task-{id}.jsonl")))
    }
    #[test]
    fn collaboration_material_is_scoped_chunked_and_leaves_sessions_items_and_reads_untouched() {
        let f = ServiceFixture::new();
        let id = room(&f);
        let other = room(&f);
        let session = f.add_session(1, 10);
        crate::storage::seed_tracking(f.db(), &session, Some(&id)).unwrap();
        let item = crate::workspace::task_items::create(
            f.db(),
            &id,
            crate::workspace::task_items::CreateItem {
                title: "独立执行事项".into(),
                description: None,
            },
        )
        .unwrap();
        let content = "协作🌱".repeat(1400);
        message(&f, &id, &content);
        message(&f, &other, "其他任务的消息");
        let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
            let rows = read(dir, &id);
            assert_eq!(rows[0]["type"], "collaboration");
            assert_eq!(
                rows[0]["task_items"],
                json!([{"id":item.id,"title":"独立执行事项","description":""}])
            );
            let messages: Vec<_> = rows.iter().filter(|r| r["type"] == "message").collect();
            assert_eq!(messages.len(), 3);
            assert_eq!(
                messages
                    .iter()
                    .map(|r| r["content"].as_str().unwrap())
                    .collect::<String>(),
                content
            );
            assert_eq!(messages[2]["last_part"], true);
            assert!(rows.iter().all(|r| r["content"] != "其他任务的消息"));
            result(dir, &id, "已形成共识").unwrap();
            Ok(())
        };
        let out = extract_task(f.db(), &id, Some(&f.home()), false, &mut runner).unwrap();
        assert_eq!(out.remaining_pending, 0);
        assert_eq!(
            view(f.db(), &id).unwrap().summary.unwrap().items[0].title,
            "已形成共识"
        );
        assert!(f.db().get_session_context(&session).unwrap().is_none());
        assert_eq!(
            crate::workspace::task_items::list(f.db(), &id).unwrap(),
            vec![item]
        );
        assert_eq!(
            f.db()
                .read()
                .query_row("SELECT COUNT(*) FROM collaboration_reads", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        append(&f, &session, "成员自己的新消息", false);
        assert!(!view(f.db(), &id).unwrap().pending);
    }
    #[test]
    fn concurrent_appends_are_pending_incremental_reads_exclude_consumed_messages_and_regeneration_reads_all_runs(
    ) {
        let f = ServiceFixture::new();
        let id = room(&f);
        message(&f, &id, "初始决定");
        let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
            read(dir, &id);
            message(&f, &id, "晚到的修正");
            result(dir, &id, "初始决定").unwrap();
            Ok(())
        };
        let out = extract_task(f.db(), &id, Some(&f.home()), false, &mut runner).unwrap();
        assert_eq!(out.remaining_pending, 1);
        assert_eq!(out.processed_through_seq, 1);
        let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
            let rows = read(dir, &id);
            assert_eq!(rows[0]["current_summary"]["items"][0]["title"], "初始决定");
            let messages: Vec<_> = rows.iter().filter(|r| r["type"] == "message").collect();
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0]["content"], "晚到的修正");
            result(dir, &id, "修正结论").unwrap();
            Ok(())
        };
        extract_task(f.db(), &id, Some(&f.home()), false, &mut runner).unwrap();
        f.db().write().execute("INSERT INTO collaboration_runs(id,room_id,status,started_at) VALUES('second',?1,'stopped',?2)",params![id,crate::storage::now()]).unwrap();
        f.db().write().execute("INSERT INTO collaboration_messages(id,room_id,run_id,seq,kind,body,created_at) VALUES('second-message',?1,'second',3,'text','第二次协作',?2)",params![id,crate::storage::now()]).unwrap();
        let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
            let rows = read(dir, &id);
            assert!(rows[0]["current_summary"].is_null());
            assert_eq!(rows.iter().filter(|r| r["type"] == "message").count(), 3);
            result(dir, &id, "跨次协作结论").unwrap();
            Ok(())
        };
        extract_task(f.db(), &id, Some(&f.home()), true, &mut runner).unwrap();
        assert!(!view(f.db(), &id).unwrap().pending);
    }
    #[test]
    fn no_room_or_messages_do_not_start_an_agent_and_skips_consume_only_the_snapshot() {
        let f = ServiceFixture::new();
        let task =
            crate::workspace::workstream::create_workstream(f.db(), "单人任务", "", &[]).unwrap();
        let mut never = |_: &CliExtractor, _: &str, _: &Path| panic!("没有协作材料不启动提取");
        assert_eq!(
            extract_task(f.db(), &task.id, Some(&f.home()), true, &mut never)
                .unwrap()
                .status,
            ContextUpdateStatus::NoChange
        );
        let id = room(&f);
        assert!(!view(f.db(), &id).unwrap().pending);
        message(&f, &id, "收到，谢谢");
        let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
            read(dir, &id);
            submit_reply(
                dir,
                None,
                json!({"status":"skipped","reason":"no_summary_content"}),
                json!({}),
            )
            .unwrap();
            Ok(())
        };
        let out = extract_task(f.db(), &id, Some(&f.home()), false, &mut runner).unwrap();
        assert_eq!(out.status, ContextUpdateStatus::Skipped);
        assert!(view(f.db(), &id).unwrap().summary.is_none());
        assert!(!view(f.db(), &id).unwrap().pending);
        extract_task(f.db(), &id, Some(&f.home()), false, &mut never).unwrap();
    }
    #[test]
    fn failed_or_invalid_outputs_keep_previous_summary_and_cursor_and_changed_background_rejects_commit(
    ) {
        let f = ServiceFixture::new();
        let id = room(&f);
        message(&f, &id, "第一条有效结论");
        let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
            read(dir, &id);
            result(dir, &id, "保留结论").unwrap();
            Ok(())
        };
        extract_task(f.db(), &id, Some(&f.home()), false, &mut runner).unwrap();
        message(&f, &id, "后续材料");
        for reply in [
            r#"{"status":"failed","error":"无法提取"}"#,
            r#"{"status":"completed"}"#,
            r#"{"status":"partial","failed_sessions":[],"skipped_session_ids":[]}"#,
        ] {
            let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
                read(dir, &id);
                assert!(
                    submit_reply(dir, None, serde_json::from_str(reply).unwrap(), json!({}))
                        .is_err()
                );
                Ok(())
            };
            assert!(extract_task(f.db(), &id, Some(&f.home()), false, &mut runner).is_err());
            let v = view(f.db(), &id).unwrap();
            assert_eq!(v.context_revision, 1);
            assert_eq!(v.processed_through_seq, 1);
            assert_eq!(v.summary.unwrap().items[0].title, "保留结论");
        }
        let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
            read(dir, &id);
            crate::workspace::workstream::update_workstream_fields(
                f.db(),
                &id,
                Some("新目标"),
                None,
            )
            .unwrap();
            assert_eq!(
                result(dir, &id, "已过期的结论").unwrap_err().code,
                "stale_result"
            );
            Ok(())
        };
        assert!(extract_task(f.db(), &id, Some(&f.home()), false, &mut runner).is_err());
        assert_eq!(view(f.db(), &id).unwrap().context_revision, 1);
    }
}

#[cfg(test)]
mod interactive_tests {
    use super::*;
    use crate::context::summary::Submission;
    use crate::context::tests::ServiceFixture;
    use std::collections::HashMap;
    #[test]
    fn caller_can_extract_collaboration_without_agent_configuration_and_search_does_not_replace_complete_reading(
    ) {
        let f = ServiceFixture::new();
        f.db()
            .set_setting("context.extraction", r#"{"agent":"none","runtime":{}}"#)
            .unwrap();
        let task = super::tests::room(&f);
        let text = format!("{}跨段{}", "前".repeat(1999), "后".repeat(5));
        super::tests::message(&f, &task, &text);
        let c = interactive::context(f.db(), &f.home(), "task", &task, interactive::Mode::Update)
            .unwrap();
        assert_eq!(c["inputs"].as_array().unwrap().len(), 1);
        let cid = c["context_id"].as_str().unwrap();
        let name = c["inputs"][0]["name"].as_str().unwrap();
        let found = interactive::read(f.db(), &f.home(), cid, name, 0, 8, Some("前跨段")).unwrap();
        assert_eq!(found["records"].as_array().unwrap().len(), 2);
        let results = HashMap::from([(
            format!("task-{task}"),
            json!({"summary":{"items":[{"title":"协作共识","description":"","details":[]}]}}),
        )]);
        assert!(interactive::submit(
            f.db(),
            &f.home(),
            cid,
            Submission {
                session_id: None,
                reply: json!({"status":"completed"}),
                results: results.clone()
            }
        )
        .is_err());
        let mut cursor = 0;
        let mut content = String::new();
        loop {
            let page = interactive::read(f.db(), &f.home(), cid, name, cursor, 1, None).unwrap();
            for r in page["records"].as_array().unwrap() {
                if let Some(text) = r["record"]["content"].as_str() {
                    content.push_str(text);
                }
            }
            if let Some(next) = page["next_cursor"].as_u64() {
                cursor = next as usize;
            } else {
                break;
            }
        }
        assert_eq!(content, text);
        super::tests::message(&f, &task, "截止位置之后");
        let out = interactive::submit(
            f.db(),
            &f.home(),
            cid,
            Submission {
                session_id: None,
                reply: json!({"status":"completed"}),
                results,
            },
        )
        .unwrap();
        assert_eq!(out["remaining_pending"], 1);
        assert_eq!(out["processed_through_seq"], 1);
        assert_eq!(
            view(f.db(), &task).unwrap().summary.unwrap().items[0].title,
            "协作共识"
        );
        assert!(interactive::read(f.db(), &f.home(), cid, name, 0, 1, None).is_err());
    }
}
