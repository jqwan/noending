use super::*;
use crate::domain::{
    Agent, ParsedSessionMessage, SessionMessageRole, SourceCursorUpdate, Workstream,
};
use crate::workspace::home::NoEndingHome;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
fn test_workstream() -> Workstream {
    Workstream {
        id: "workstream-context-test".into(),
        title: "Context test".into(),
        description: String::new(),
        visibility: "normal".into(),
        created_at: crate::storage::now(),
        updated_at: crate::storage::now(),
    }
}

pub(super) struct ServiceFixture {
    db: Option<Db>,
    root: PathBuf,
}

impl ServiceFixture {
    pub(super) fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "noending-context-service-{}",
            crate::storage::new_id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let db = Db::open(&root.join("db.sqlite")).unwrap();
        Self { db: Some(db), root }
    }

    pub(super) fn db(&self) -> &Db {
        self.db.as_ref().unwrap()
    }

    pub(super) fn home(&self) -> NoEndingHome {
        NoEndingHome::new(self.root.join("home").to_str().unwrap(), None).unwrap()
    }

    pub(super) fn add_session(&self, message_count: usize, message_size: usize) -> String {
        let db = self.db();
        let root_id = format!("context-service-root-{}", crate::storage::new_id());
        let session_id = db
            .upsert_logical_session(
                Agent::Codex,
                &root_id,
                None,
                Some("Context service fixture"),
                None,
                None,
                None,
                None,
                None,
                "context_service_test",
                "/tmp/context-service-fixture",
                &serde_json::json!({}),
            )
            .unwrap()
            .0;
        let content = "x".repeat(message_size);
        let messages: Vec<ParsedSessionMessage> = (0..message_count)
            .map(|i| ParsedSessionMessage {
                source_message_id: Some(format!("source-{i}")),
                ts: None,
                role: SessionMessageRole::User,
                content: content.clone(),
            })
            .collect();
        db.commit_ingest(
            &session_id,
            &messages,
            &SourceCursorUpdate {
                file_identity: "context-service-fixture".into(),
                generation: 0,
                byte_offset: 10_000,
                last_seen_size: 10_000,
                mtime: None,
                start_byte_offset: 0,
                prefix_hash: String::new(),
            },
        )
        .unwrap();
        session_id
    }

    fn add_workstream(&self, id: &str) -> Workstream {
        let mut workstream = test_workstream();
        workstream.id = id.into();
        self.db().upsert_workstream(&workstream).unwrap();
        workstream
    }
}

impl Drop for ServiceFixture {
    fn drop(&mut self) {
        drop(self.db.take());
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn records(path: &Path) -> Vec<Value> {
    tools::read_input(path)
}
fn result(dir: &Path, prefix: &str, id: &str, summary: &str) {
    std::fs::write(
        dir.join(format!("{prefix}-{id}.result.json")),
        json!({"summary":{"items":[{"title":summary,"description":"","details":[]}]}}).to_string(),
    )
    .unwrap();
}

fn summary_at(db: &Db, kind: &str, id: &str) -> (i64, crate::domain::Summary) {
    match kind {
        "session" => {
            let view = session_context_view(db, id).unwrap();
            (view.revision, view.fields.unwrap().summary)
        }
        "task" => {
            let view = workstream_context_view(db, id).unwrap();
            (view.context_revision, view.summary.unwrap())
        }
        "project" => {
            let view = project_context_view(db, id).unwrap();
            (view.context_revision, view.summary.unwrap())
        }
        _ => unreachable!(),
    }
}

#[test]
fn title_locks_persist_in_all_scopes_allow_body_edits_and_require_user_unlock_for_renames() {
    let mut f = ServiceFixture::new();
    let sid = f.add_session(1, 10);
    let (project, _) = project_fixture(&f, "title-lock-project", &[&sid]);
    let ws = task(&f, &[&sid]);
    let targets = [("session", sid), ("task", ws.id), ("project", project.id)];
    let original = json!({"items":[{"title":"用户固定的标题","description":"旧描述","details":["旧细则"],"title_locked":false}]});
    for (kind, id) in &targets {
        edit_summary(f.db(), kind, id, 0, &original.to_string()).unwrap();
        let locked = set_summary_title_lock(f.db(), kind, id, 1, 1, true).unwrap();
        assert_eq!(locked.revision, 2);
        assert_eq!(locked.summary.items[0].description, "旧描述");
        let updated = json!({"items":[{"title":"用户固定的标题","description":"Agent 更新的描述","details":["Agent 更新的细则"],"title_locked":true}]});
        edit_summary(f.db(), kind, id, 2, &updated.to_string()).unwrap();
        for invalid in [
            json!({"items":[{"title":"擅自改名","description":"新描述","details":[],"title_locked":true}]}),
            json!({"items":[{"title":"用户固定的标题","description":"新描述","details":[],"title_locked":false}]}),
            json!({"items":[{"title":"其他事项","description":"","details":[],"title_locked":false}]}),
        ] {
            assert!(edit_summary(f.db(), kind, id, 3, &invalid.to_string()).is_err());
        }
        assert!(set_summary_title_lock(f.db(), kind, id, 2, 1, false).is_err());
        assert!(set_summary_title_lock(f.db(), kind, id, 3, 0, false).is_err());
        assert!(set_summary_title_lock(f.db(), kind, id, 3, 2, false).is_err());
        let (revision, summary) = summary_at(f.db(), kind, id);
        assert_eq!(revision, 3);
        assert_eq!(summary.items[0].description, "Agent 更新的描述");
        assert_eq!(summary.items[0].details, vec!["Agent 更新的细则"]);
    }
    drop(f.db.take());
    f.db = Some(Db::open(&f.root.join("db.sqlite")).unwrap());
    for (kind, id) in &targets {
        let (revision, summary) = summary_at(f.db(), kind, id);
        assert_eq!(revision, 3);
        assert!(summary.items[0].title_locked);
        assert_eq!(summary.items[0].title, "用户固定的标题");
        let unlocked = set_summary_title_lock(f.db(), kind, id, 3, 1, false).unwrap();
        assert!(!unlocked.summary.items[0].title_locked);
        let mut renamed = unlocked.summary;
        renamed.items[0].title = "解锁后可改名".into();
        edit_summary(
            f.db(),
            kind,
            id,
            4,
            &serde_json::to_string(&renamed).unwrap(),
        )
        .unwrap();
        assert_eq!(
            summary_at(f.db(), kind, id).1.items[0].title,
            "解锁后可改名"
        );
    }
}

#[test]
fn session_extraction_updates_locked_title_bodies_in_increment_regeneration_and_rewritten_history()
{
    let f = ServiceFixture::new();
    let home = f.home();
    let id = f.add_session(1, 10);
    seed(&f, &id, "固定标题");
    set_summary_title_lock(f.db(), "session", &id, 1, 1, true).unwrap();
    for (regenerate, rewrite) in [(false, false), (true, false), (false, true)] {
        append(&f, &id, "需要更新描述和细则的新证据", rewrite);
        let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
            let header = records(&dir.join(format!("session-{id}.jsonl"))).remove(0);
            assert_eq!(header["locked_titles"], json!(["固定标题"]));
            if regenerate || rewrite {
                assert!(header["current_summary"].is_null());
            }
            std::fs::write(
                dir.join(format!("session-{id}.result.json")),
                json!({"summary":{"items":[
                    {"title":"新事项","description":"可新增其他事项","details":[]},
                    {"title":"固定标题","description":"新的结论","details":["新的细则"]}
                ]}})
                .to_string(),
            )
            .unwrap();
            Ok(r#"{"status":"completed"}"#.into())
        };
        extract_session(f.db(), &id, Some(&home), regenerate, &mut runner).unwrap();
        let (_, summary) = summary_at(f.db(), "session", &id);
        assert!(!summary.items[0].title_locked);
        assert_eq!(summary.items[1].title, "固定标题");
        assert_eq!(summary.items[1].description, "新的结论");
        assert_eq!(summary.items[1].details, vec!["新的细则"]);
        assert!(summary.items[1].title_locked);
    }
    let before = summary_at(f.db(), "session", &id);
    for generated in [
        json!({"items":[{"title":"被改名","description":"","details":[]}]}),
        json!({"items":[{"title":"固定标题","description":"","details":[],"title_locked":false}]}),
    ] {
        let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
            std::fs::write(
                dir.join(format!("session-{id}.result.json")),
                json!({"summary":generated}).to_string(),
            )
            .unwrap();
            Ok(r#"{"status":"completed"}"#.into())
        };
        assert_eq!(
            code(extract_session(f.db(), &id, Some(&home), true, &mut runner)),
            "invalid_output"
        );
        assert_eq!(summary_at(f.db(), "session", &id), before);
    }
}

#[test]
fn batch_extraction_protects_session_and_target_titles_while_refreshing_their_bodies() {
    for kind in ["task", "project"] {
        let f = ServiceFixture::new();
        let home = f.home();
        let sid = f.add_session(1, 10);
        let (project, _) = project_fixture(&f, "locked-batch", &[&sid]);
        let ws = task(&f, &[&sid]);
        let id = if kind == "task" { &ws.id } else { &project.id };
        seed(&f, &sid, "固定会话标题");
        set_summary_title_lock(f.db(), "session", &sid, 1, 1, true).unwrap();
        edit_summary(
            f.db(),
            kind,
            id,
            0,
            &serde_json::to_string(&crate::domain::Summary::single("固定汇总标题")).unwrap(),
        )
        .unwrap();
        set_summary_title_lock(f.db(), kind, id, 1, 1, true).unwrap();
        for regenerate in [false, true] {
            append(&f, &sid, "新结论", false);
            let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
                let input = records(&dir.join(format!("{kind}-{id}.jsonl")));
                assert_eq!(input[0]["locked_titles"], json!(["固定汇总标题"]));
                let session = input
                    .iter()
                    .find(|record| record["session_id"] == sid)
                    .unwrap();
                if regenerate {
                    assert!(input[0]["current_summary"].is_null());
                    let history =
                        records(Path::new(session["history_input_file"].as_str().unwrap()));
                    assert_eq!(history[0]["from_sequence"], 1);
                    assert_eq!(history[0]["message_count"], 3);
                    assert_eq!(history[1]["content"], "xxxxxxxxxx");
                } else {
                    assert!(session["history_input_file"].is_null());
                }
                let header = records(&dir.join(format!("session-{sid}.jsonl"))).remove(0);
                assert_eq!(header["locked_titles"], json!(["固定会话标题"]));
                for (scope, target, title) in [
                    ("session", &sid, "固定会话标题"),
                    (kind, id, "固定汇总标题"),
                ] {
                    std::fs::write(dir.join(format!("{scope}-{target}.result.json")),json!({"summary":{"items":[{"title":title,"description":"更新后的描述","details":["更新后的细则"]}]}}).to_string()).unwrap();
                }
                Ok(r#"{"status":"completed","skipped_session_ids":[]}"#.into())
            };
            if kind == "task" {
                extract_task(f.db(), id, Some(&home), regenerate, &mut runner).unwrap();
            } else {
                project::extract_project(f.db(), id, Some(&home), regenerate, &mut runner).unwrap();
            }
            for (scope, target, title) in [
                ("session", &sid, "固定会话标题"),
                (kind, id, "固定汇总标题"),
            ] {
                let (_, summary) = summary_at(f.db(), scope, target);
                assert_eq!(summary.items[0].title, title);
                assert!(summary.items[0].title_locked);
                assert_eq!(summary.items[0].description, "更新后的描述");
                assert_eq!(summary.items[0].details, vec!["更新后的细则"]);
            }
        }
    }
}

#[test]
fn locking_a_title_during_extraction_rejects_the_stale_result_and_preserves_the_user_lock() {
    let f = ServiceFixture::new();
    let home = f.home();
    let id = f.add_session(1, 10);
    seed(&f, &id, "用户保留的标题");
    append(&f, &id, "新增材料", false);
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        set_summary_title_lock(f.db(), "session", &id, 1, 1, true).unwrap();
        result(dir, "session", &id, "提取期间擅自改名");
        Ok(r#"{"status":"completed"}"#.into())
    };
    assert_eq!(
        code(extract_session(
            f.db(),
            &id,
            Some(&home),
            false,
            &mut runner
        )),
        "stale_snapshot"
    );
    let (revision, summary) = summary_at(f.db(), "session", &id);
    assert_eq!(revision, 2);
    assert_eq!(summary.items[0].title, "用户保留的标题");
    assert!(summary.items[0].title_locked);
    assert!(session_context_view(f.db(), &id).unwrap().pending);
}
fn task(f: &ServiceFixture, ids: &[&str]) -> Workstream {
    let ws = f.add_workstream("task-test");
    for id in ids {
        crate::storage::seed_tracking(&f.db(), id, Some(&ws.id)).unwrap();
    }
    ws
}
pub(super) fn append(f: &ServiceFixture, id: &str, text: &str, rewrite: bool) {
    let old = f.db().get_session(id).unwrap().unwrap().source_cursor();
    f.db()
        .commit_ingest(
            id,
            &[ParsedSessionMessage {
                source_message_id: Some(crate::storage::new_id()),

                ts: None,
                role: SessionMessageRole::Assistant,
                content: text.into(),
            }],
            &SourceCursorUpdate {
                file_identity: old.source_file_identity,
                generation: old.generation + i64::from(rewrite),
                start_byte_offset: if rewrite { 0 } else { old.byte_offset },
                byte_offset: old.byte_offset + 100,
                last_seen_size: old.byte_offset + 100,
                mtime: None,
                prefix_hash: String::new(),
            },
        )
        .unwrap();
}
pub(super) fn seed(f: &ServiceFixture, id: &str, body: &str) {
    let (generation, latest) = f.db().get_session_ingest_state(id).unwrap();
    f.db()
        .tx(|tx| {
            commit_session_context_conn(
                tx,
                id,
                &SessionContextFields {
                    summary: crate::domain::Summary::single(body),
                },
                0,
                generation,
                latest,
            )
        })
        .unwrap();
}
fn code<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result.unwrap_err() {
        AppError::ContextUpdateFailed(error) => error.code,
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn session_full_input_increment_regeneration_and_long_summary() {
    let f = ServiceFixture::new();
    let home = f.home();
    let id = f.add_session(50, 6000);
    extractor::ExtractionSettings {
        agent: "codex".into(),
        runtime: crate::agent_runtime::AgentRuntimeOverrides {
            model: Some("extraction-model".into()),
            effort: Some("high".into()),
            provider: None,
        },
    }
    .save(f.db())
    .unwrap();
    crate::agent_runtime::set_runtime_overrides(
        f.db(),
        Agent::Codex,
        &crate::agent_runtime::AgentRuntimeOverrides {
            model: Some("unrelated-global-model".into()),
            effort: Some("low".into()),
            provider: None,
        },
    )
    .unwrap();
    f.db()
        .write()
        .execute(
            "UPDATE sessions SET cwd=?1, started_at='2026-09-01T00:00:00Z' WHERE id=?2",
            rusqlite::params!["/workspace/项目 with spaces", id],
        )
        .unwrap();
    f.db()
        .write()
        .execute(
            "UPDATE session_messages SET ts='2026-10-01T00:00:00Z' WHERE session_id=?1 AND sequence=50",
            [&id],
        )
        .unwrap();
    let root = diagnostics::prepare_runtime_dir(&home).unwrap();
    let fixed = root.join(format!("session-{id}"));
    std::fs::create_dir_all(&fixed).unwrap();
    std::fs::write(
        fixed.join(format!("session-{id}.result.json")),
        r#"{"summary":{"items":[{"title":"stale","description":"","details":[]}]}}"#,
    )
    .unwrap();
    let long = "**结论**：".repeat(500);
    let mut calls = 0;
    let mut invocation = PathBuf::new();
    let mut runner = |cli: &CliExtractor, prompt: &str, dir: &Path| {
        assert_eq!(cli.agent, Agent::Codex);
        assert_eq!(cli.opts.model.as_deref(), Some("extraction-model"));
        assert_eq!(cli.opts.effort.as_deref(), Some("high"));
        calls += 1;
        invocation = dir.to_path_buf();
        assert_ne!(dir, fixed);
        assert!(!dir.join(format!("session-{id}.result.json")).exists());
        let input = records(&dir.join(format!("session-{id}.jsonl")));
        assert_eq!(input[0]["message_count"], 50);
        assert_eq!(input[0]["current_summary"], Value::Null);
        assert_eq!(input[0]["working_directory"], "/workspace/项目 with spaces");
        assert_eq!(input[0]["started_at"], "2026-09-01T00:00:00Z");
        assert_eq!(input[0]["last_message_at"], "2026-10-01T00:00:00Z");
        assert_eq!(input[0]["summary_through_at"], Value::Null);
        for seq in 1..=50 {
            let reconstructed: String = input
                .iter()
                .filter(|r| r["sequence"] == seq)
                .map(|r| r["content"].as_str().unwrap())
                .collect();
            assert_eq!(reconstructed, "x".repeat(6000));
        }
        assert_eq!(input.last().unwrap()["marker"], "END_OF_SESSION_INPUT");
        assert!(prompt.contains("SKILL.md"));
        assert!(!prompt.contains(&"x".repeat(6000)));
        result(dir, "session", &id, &long);
        Ok(r#"{"status":"completed"}"#.into())
    };
    extract_session(f.db(), &id, Some(&home), false, &mut runner).unwrap();
    assert_eq!(calls, 1);
    assert!(!invocation.exists());
    assert!(fixed.join(format!("session-{id}.result.json")).is_file());
    append(&f, &id, "验证完成", false);
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        let input = records(&dir.join(format!("session-{id}.jsonl")));
        assert_eq!(
            input[0]["current_summary"],
            json!(crate::domain::Summary::single(long.clone()))
        );
        assert_eq!(input[0]["from_sequence"], 51);
        assert_eq!(input[0]["message_count"], 1);
        assert_eq!(input[0]["summary_through_at"], "2026-10-01T00:00:00Z");
        assert_eq!(input[0]["last_message_at"], Value::Null);
        result(dir, "session", &id, "**结果**：验证完成");
        Ok(r#"{"status":"completed"}"#.into())
    };
    extract_session(f.db(), &id, Some(&home), false, &mut runner).unwrap();
    assert!(!session_context_view(f.db(), &id).unwrap().pending);
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        let input = records(&dir.join(format!("session-{id}.jsonl")));
        assert_eq!(input[0]["current_summary"], Value::Null);
        assert_eq!(input[0]["message_count"], 51);
        assert_eq!(input[0]["summary_through_at"], Value::Null);
        result(dir, "session", &id, "重新整理");
        Ok(r#"{"status":"completed"}"#.into())
    };
    extract_session(f.db(), &id, Some(&home), true, &mut runner).unwrap();
    let ctx = f.db().get_session_context(&id).unwrap().unwrap();
    assert_eq!(ctx.revision, 3);
    assert_eq!(
        ctx.fields.summary,
        crate::domain::Summary::single("重新整理")
    );
    append(&f, &id, "改写后唯一保留的对话", true);
    assert_eq!(
        f.db()
            .get_session_context(&id)
            .unwrap()
            .unwrap()
            .fields
            .summary,
        crate::domain::Summary::single("重新整理")
    );
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        let input = records(&dir.join(format!("session-{id}.jsonl")));
        assert_eq!(input[0]["current_summary"], Value::Null);
        assert_eq!(input[0]["from_sequence"], 1);
        assert_eq!(input[0]["message_count"], 1);
        assert!(input.iter().any(|r| r["content"] == "改写后唯一保留的对话"));
        result(dir, "session", &id, "仅根据当前对话生成的新摘要");
        Ok(r#"{"status":"completed"}"#.into())
    };
    extract_session(f.db(), &id, Some(&home), false, &mut runner).unwrap();
    assert_eq!(
        f.db()
            .get_session_context(&id)
            .unwrap()
            .unwrap()
            .fields
            .summary,
        crate::domain::Summary::single("仅根据当前对话生成的新摘要")
    );
    assert_eq!(
        f.db()
            .read()
            .query_row(
                "SELECT COUNT(*) FROM session_contexts WHERE session_id=?1",
                [&id],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn skip_advances_progress_preserves_valid_summary_and_avoids_repeat_calls() {
    let f = ServiceFixture::new();
    let home = f.home();
    let id = f.add_session(1, 10);
    seed(&f, &id, "有效旧摘要");
    append(&f, &id, "谢谢", false);
    let mut skip = |_: &CliExtractor, _: &str, _: &Path| {
        Ok(r#"{"status":"skipped","reason":"no_summary_content"}"#.into())
    };
    let out = extract_session(f.db(), &id, Some(&home), false, &mut skip).unwrap();
    assert_eq!(out.status, ContextUpdateStatus::Skipped);
    let view = session_context_view(f.db(), &id).unwrap();
    assert!(!view.pending);
    assert_eq!(
        view.fields.unwrap().summary,
        crate::domain::Summary::single("有效旧摘要")
    );
    assert_eq!(view.processed_through_seq, 2);
    let mut never =
        |_: &CliExtractor, _: &str, _: &Path| panic!("no increment must not launch an Agent");
    assert_eq!(
        extract_session(f.db(), &id, Some(&home), false, &mut never)
            .unwrap()
            .status,
        ContextUpdateStatus::NoChange
    );
}

#[test]
fn task_batch_once_partial_preserves_failed_summary_then_retries_only_failed_session() {
    let f = ServiceFixture::new();
    let home = f.home();
    let a = f.add_session(1, 10);
    let b = f.add_session(1, 20);
    let ws = task(&f, &[&a, &b]);
    seed(&f, &b, "旧的可用结论");
    append(&f, &b, "尚未处理的新信息", false);
    let mut calls = 0;
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        calls += 1;
        let input = records(&dir.join(format!("task-{}.jsonl", ws.id)));
        assert_eq!(input[0]["current_summary"], Value::Null);
        assert!(input.iter().any(|r| r["session_id"] == b
            && r["current_summary"] == json!(crate::domain::Summary::single("旧的可用结论"))));
        result(dir, "session", &a, "会话 A 新结论");
        result(dir, "task", &ws.id, "结合 A 与 B 的旧结论");
        Ok(json!({"status":"partial","failed_sessions":[{"session_id":b,"error":"读取失败"}],"skipped_session_ids":[]}).to_string().into())
    };
    let out = extract_task(f.db(), &ws.id, Some(&home), false, &mut runner).unwrap();
    assert_eq!(calls, 1);
    assert_eq!(out.status, ContextUpdateStatus::Partial);
    assert_eq!(out.updated_sessions, vec![a.clone()]);
    assert_eq!(out.remaining_pending, 1);
    assert_eq!(
        f.db()
            .get_session_context(&b)
            .unwrap()
            .unwrap()
            .fields
            .summary,
        crate::domain::Summary::single("旧的可用结论")
    );
    assert!(session_context_view(f.db(), &b).unwrap().pending);
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        assert!(!tools::input_available(
            &dir.join(format!("session-{a}.jsonl"))
        ));
        assert!(tools::input_available(
            &dir.join(format!("session-{b}.jsonl"))
        ));
        assert!(!records(&dir.join(format!("session-{b}.jsonl"))).is_empty());
        let input = records(&dir.join(format!("task-{}.jsonl", ws.id)));
        assert_eq!(
            input[0]["current_summary"],
            json!(crate::domain::Summary::single("结合 A 与 B 的旧结论"))
        );
        result(dir, "session", &b, "B 新结论");
        result(dir, "task", &ws.id, "完整最新结论");
        Ok(r#"{"status":"completed","skipped_session_ids":[]}"#.into())
    };
    extract_task(f.db(), &ws.id, Some(&home), false, &mut runner).unwrap();
    assert!(!workstream_context_view(f.db(), &ws.id).unwrap().pending);
    let mut never = |_: &CliExtractor, _: &str, _: &Path| {
        panic!("pure read and clean task must not invoke Agent")
    };
    assert_eq!(
        extract_task(f.db(), &ws.id, Some(&home), false, &mut never)
            .unwrap()
            .status,
        ContextUpdateStatus::NoChange
    );
}

#[test]
fn task_skip_retains_session_summary_and_marks_it_checked() {
    let f = ServiceFixture::new();
    let home = f.home();
    let id = f.add_session(1, 10);
    let ws = task(&f, &[&id]);
    seed(&f, &id, "旧摘要");
    append(&f, &id, "谢谢", false);
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        result(dir, "task", &ws.id, "任务当前结论");
        Ok(json!({"status":"completed","skipped_session_ids":[id]})
            .to_string()
            .into())
    };
    let out = extract_task(f.db(), &ws.id, Some(&home), false, &mut runner).unwrap();
    assert_eq!(out.skipped_session_ids, vec![id.clone()]);
    assert_eq!(out.remaining_pending, 0);
    assert!(!workstream_context_view(f.db(), &ws.id).unwrap().pending);
    assert_eq!(
        f.db()
            .get_session_context(&id)
            .unwrap()
            .unwrap()
            .fields
            .summary,
        crate::domain::Summary::single("旧摘要")
    );
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        let input = records(&dir.join(format!("task-{}.jsonl", ws.id)));
        assert_eq!(input[0]["current_summary"], Value::Null);
        assert!(!tools::input_available(
            &dir.join(format!("session-{id}.jsonl"))
        ));
        Ok(r#"{"status":"skipped","reason":"no_summary_content","skipped_session_ids":[]}"#.into())
    };
    extract_task(f.db(), &ws.id, Some(&home), true, &mut runner).unwrap();
    assert_eq!(
        workstream_context_view(f.db(), &ws.id)
            .unwrap()
            .summary
            .as_ref()
            .map(|summary| summary.items[0].title.as_str()),
        Some("任务当前结论")
    );
}

#[test]
fn task_changes_keep_summaries_until_update_and_rebuild_rewritten_sessions() {
    let f = ServiceFixture::new();
    let home = f.home();
    let a = f.add_session(1, 10);
    let b = f.add_session(1, 10);
    let ws = task(&f, &[&a, &b]);
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        result(dir, "session", &a, "A");
        result(dir, "session", &b, "B");
        result(dir, "task", &ws.id, "旧任务摘要");
        Ok(r#"{"status":"completed","skipped_session_ids":[]}"#.into())
    };
    extract_task(f.db(), &ws.id, Some(&home), false, &mut runner).unwrap();
    crate::storage::seed_tracking(&f.db(), &b, None).unwrap();
    append(&f, &a, "改写后的历史", true);
    assert_eq!(
        session_context_view(f.db(), &a)
            .unwrap()
            .fields
            .unwrap()
            .summary,
        crate::domain::Summary::single("A")
    );
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        let input = records(&dir.join(format!("task-{}.jsonl", ws.id)));
        assert_eq!(
            input[0]["current_summary"],
            json!(crate::domain::Summary::single("旧任务摘要"))
        );
        assert!(!input.iter().any(|r| r["session_id"] == b));
        let session = records(&dir.join(format!("session-{a}.jsonl")));
        assert_eq!(session[0]["current_summary"], Value::Null);
        assert_eq!(session[0]["message_count"], 1);
        assert_eq!(session[1]["content"], "改写后的历史");
        result(dir, "session", &a, "改写结果");
        result(dir, "task", &ws.id, "新任务摘要");
        Ok(r#"{"status":"completed","skipped_session_ids":[]}"#.into())
    };
    extract_task(f.db(), &ws.id, Some(&home), false, &mut runner).unwrap();
    assert_eq!(f.db().workstream_frontiers(&ws.id).unwrap().len(), 1);
    assert!(!workstream_context_view(f.db(), &ws.id).unwrap().pending);
}

#[test]
fn malformed_or_missing_batch_outputs_roll_back_all_summaries() {
    let f = ServiceFixture::new();
    let home = f.home();
    let a = f.add_session(1, 10);
    let b = f.add_session(1, 10);
    let ws = task(&f, &[&a, &b]);
    for (reply, task_body) in [
        (
            r#"{"status":"completed","skipped_session_ids":[]}"#,
            r#"{"summary":{"items":[{"title":"valid","description":"","details":[]}]}}"#,
        ),
        (
            r#"{"status":"completed","skipped_session_ids":["unknown"]}"#,
            r#"{"summary":{"items":[{"title":"valid","description":"","details":[]}]}}"#,
        ),
        (
            r#"{"status":"completed","skipped_session_ids":[]}"#,
            r#"{"summary":"","extra":true}"#,
        ),
        (
            r#"{"status":"partial","failed_sessions":[{"session_id":"unknown","error":"error"}],"skipped_session_ids":[]}"#,
            r#"{"summary":{"items":[{"title":"valid","description":"","details":[]}]}}"#,
        ),
    ] {
        let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
            result(dir, "session", &a, "valid");
            std::fs::write(dir.join(format!("task-{}.result.json", ws.id)), task_body).unwrap();
            Ok(reply.into())
        };
        assert_eq!(
            code(extract_task(
                f.db(),
                &ws.id,
                Some(&home),
                false,
                &mut runner
            )),
            "invalid_output"
        );
        assert!(f.db().get_session_context(&a).unwrap().is_none());
        assert!(f.db().get_session_context(&b).unwrap().is_none());
        assert_eq!(
            f.db()
                .get_workstream_context_state(&ws.id)
                .unwrap()
                .context_revision,
            0
        );
    }
}

#[test]
fn batch_and_standalone_share_locks_and_cleanup_after_failure() {
    let f = ServiceFixture::new();
    let home = f.home();
    let id = f.add_session(1, 10);
    let ws = task(&f, &[&id]);
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        let mut never = |_: &CliExtractor, _: &str, _: &Path| {
            panic!("overlapping extraction must not invoke Agent")
        };
        assert_eq!(
            code(extract_session(f.db(), &id, Some(&home), false, &mut never)),
            "summary_busy"
        );
        assert_eq!(
            code(extract_task(f.db(), &ws.id, Some(&home), true, &mut never)),
            "summary_busy"
        );
        assert!(dir.exists());
        Ok(r#"{"status":"failed","error":"cannot read input"}"#.into())
    };
    assert_eq!(
        code(extract_task(
            f.db(),
            &ws.id,
            Some(&home),
            false,
            &mut runner
        )),
        "model_call_failed"
    );
    assert!(!diagnostics::runtime_dir(&home)
        .join(format!("task-{}", ws.id))
        .exists());
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        result(dir, "session", &id, "retry works");
        Ok(r#"{"status":"completed"}"#.into())
    };
    extract_session(f.db(), &id, Some(&home), false, &mut runner).unwrap();
}

#[test]
fn appends_during_extraction_remain_pending_but_rewrites_and_owner_changes_reject_commit() {
    let f = ServiceFixture::new();
    let home = f.home();
    let id = f.add_session(1, 10);
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        append(&f, &id, "late message", false);
        result(dir, "session", &id, "snapshot summary");
        Ok(r#"{"status":"completed"}"#.into())
    };
    extract_session(f.db(), &id, Some(&home), false, &mut runner).unwrap();
    let view = session_context_view(f.db(), &id).unwrap();
    assert!(view.pending);
    assert_eq!(view.processed_through_seq, 1);
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        append(&f, &id, "rewritten", true);
        result(dir, "session", &id, "stale");
        Ok(r#"{"status":"completed"}"#.into())
    };
    assert_eq!(
        code(extract_session(
            f.db(),
            &id,
            Some(&home),
            false,
            &mut runner
        )),
        "stale_snapshot"
    );
    assert_eq!(
        f.db().get_session_context(&id).unwrap().unwrap().revision,
        1
    );
    let ws = task(&f, &[&id]);
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        crate::storage::seed_tracking(&f.db(), &id, None).unwrap();
        result(dir, "session", &id, "stale");
        result(dir, "task", &ws.id, "stale");
        Ok(r#"{"status":"completed","skipped_session_ids":[]}"#.into())
    };
    assert_eq!(
        code(extract_task(
            f.db(),
            &ws.id,
            Some(&home),
            false,
            &mut runner
        )),
        "stale_snapshot"
    );
    assert_eq!(
        f.db()
            .get_workstream_context_state(&ws.id)
            .unwrap()
            .context_revision,
        0
    );
}

#[test]
fn summary_reply_contract_rejects_unknown_duplicate_and_inconsistent_fields() {
    for invalid in [
        r#"{"status":"completed","extra":true}"#,
        r#"{"status":"completed","status":"failed","error":"x"}"#,
        r#"{"status":"skipped","reason":"other"}"#,
        r#"{"status":"failed","error":" "}"#,
        r#"```json
{"status":"completed"}
```"#,
    ] {
        assert!(summary::parse_session_reply(invalid).is_err(), "{invalid}");
    }
    for invalid in [
        r#"{"status":"completed"}"#,
        r#"{"status":"partial","failed_sessions":[],"skipped_session_ids":[]}"#,
        r#"{"status":"failed","error":"x","skipped_session_ids":[]}"#,
    ] {
        assert!(summary::parse_batch_reply(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn session_result_files_are_strict_and_failed_attempts_keep_progress() {
    let f = ServiceFixture::new();
    let home = f.home();
    let id = f.add_session(1, 10);
    seed(&f, &id, "valid summary");
    append(&f, &id, "new material", false);
    for body in [
        r#"{"summary":{"items":[{"title":" ","description":"","details":[]}]}}"#,
        r#"{"summary":{"items":[]},"summary":{"items":[]}}"#,
        r#"{"summary":{"items":[{"title":"事项","description":"","details":[]}]},"revision":10}"#,
        r#"{"error":"failure"}"#,
    ] {
        let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
            std::fs::write(dir.join(format!("session-{id}.result.json")), body).unwrap();
            Ok(r#"{"status":"completed"}"#.into())
        };
        assert_eq!(
            code(extract_session(
                f.db(),
                &id,
                Some(&home),
                false,
                &mut runner
            )),
            "invalid_output"
        );
        let stored = f.db().get_session_context(&id).unwrap().unwrap();
        assert_eq!(
            stored.fields.summary,
            crate::domain::Summary::single("valid summary")
        );
        assert_eq!(stored.processed_through_seq, 1);
        assert_eq!(stored.revision, 1);
    }
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        std::fs::create_dir(dir.join(format!("session-{id}.result.json"))).unwrap();
        Ok(r#"{"status":"completed"}"#.into())
    };
    assert_eq!(
        code(extract_session(
            f.db(),
            &id,
            Some(&home),
            false,
            &mut runner
        )),
        "invalid_output"
    );
    #[cfg(unix)]
    {
        let outside = f.root.join("outside.json");
        std::fs::write(
            &outside,
            r#"{"summary":{"items":[{"title":"outside","description":"","details":[]}]}}"#,
        )
        .unwrap();
        let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
            std::os::unix::fs::symlink(&outside, dir.join(format!("session-{id}.result.json")))
                .unwrap();
            Ok(r#"{"status":"completed"}"#.into())
        };
        assert_eq!(
            code(extract_session(
                f.db(),
                &id,
                Some(&home),
                false,
                &mut runner
            )),
            "invalid_output"
        );
        assert!(outside.exists());
    }
    assert!(session_context_view(f.db(), &id).unwrap().pending);
}

#[test]
fn unconfigured_agent_never_invokes_cli_and_logs_only_operation_metadata() {
    let f = ServiceFixture::new();
    let home = f.home();
    let id = f.add_session(1, 10);
    f.db()
        .write()
        .execute(
            "UPDATE session_messages SET content='PRIVATE_MESSAGE' WHERE session_id=?1",
            [&id],
        )
        .unwrap();
    f.db()
        .set_setting(
            "context.extraction",
            r#"{"agent":"none","runtime":{"model":null,"provider":null,"effort":null}}"#,
        )
        .unwrap();
    let mut never = |_: &CliExtractor, _: &str, _: &Path| panic!("no configured Agent");
    assert_eq!(
        code(extract_session(f.db(), &id, Some(&home), false, &mut never)),
        "agent_not_configured"
    );
    let logs: String = std::fs::read_dir(diagnostics::log_dir(&home))
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect();
    assert!(!logs.contains("PRIVATE_MESSAGE"));
    assert!(!logs.contains("current_summary"));
    assert!(logs.contains("agent_not_configured"));
    assert!(logs.contains("operation_id"));
    assert!(!diagnostics::runtime_dir(&home)
        .join(format!("session-{id}"))
        .exists());
}

fn project_fixture(
    f: &ServiceFixture,
    id: &str,
    session_ids: &[&str],
) -> (crate::domain::Project, String) {
    let p = crate::domain::Project {
        id: id.into(),
        name: format!("Project {id}"),
        git_id: None,
        name_customized: false,
        visibility: "normal".into(),
        created_at: crate::storage::now(),
        updated_at: crate::storage::now(),
    };
    f.db().upsert_project(&p).unwrap();
    let path = f
        .db()
        .tx(|tx| {
            let path = crate::storage::workspace::insert_workspace_path_conn(
                tx,
                f.root.join(id).to_str().unwrap(),
                id,
            )?;
            for sid in session_ids {
                crate::storage::session_paths::attach_session_workspace_path_conn(
                    tx,
                    sid,
                    Some(&path),
                )?;
            }
            Ok(path)
        })
        .unwrap();
    (p, path)
}

#[test]
fn project_batch_uses_physical_scope_reuses_task_session_protocol_and_retries_partial() {
    let f = ServiceFixture::new();
    let home = f.home();
    let a = f.add_session(1, 10);
    let b = f.add_session(1, 10);
    let outside = f.add_session(1, 10);
    let ws = task(&f, &[&a, &outside]);
    seed(&f, &b, "已有 B 摘要");
    append(&f, &b, "新内容", false);
    f.db()
        .write()
        .execute(
            "UPDATE session_messages SET ts=CASE WHEN sequence=1 THEN '2026-09-01T00:00:00Z' ELSE '2026-10-01T00:00:00Z' END WHERE session_id IN (?1,?2)",
            rusqlite::params![a, b],
        )
        .unwrap();
    let (p, _path) = project_fixture(&f, "scope", &[&a, &b]);
    let (_, other_path) = project_fixture(&f, "outside", &[&outside]);
    f.db()
        .tx(|tx| {
            tx.execute("INSERT INTO workstream_projects(workstream_id,project_id) VALUES (?1,?2)",rusqlite::params![ws.id,p.id])?;
            tx.execute("INSERT OR IGNORE INTO workstream_projects(workstream_id,project_id) SELECT ?1,project_id FROM workspace_paths WHERE id=?2",rusqlite::params![ws.id,other_path])?;
            tx.execute(
                "UPDATE sessions SET archived_at='2026-10-11' WHERE id=?1",
                [&b],
            )?;
            Ok(())
        })
        .unwrap();
    let mut calls = 0;
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        calls += 1;
        assert!(std::fs::read_to_string(
            dir.join("noending-extraction/references/requirements.md")
        )
        .unwrap()
        .contains("项目材料"));
        let input = records(&dir.join(format!("project-{}.jsonl", p.id)));
        assert_eq!(input[0]["related_tasks"][0]["task_id"], ws.id);
        assert_eq!(input[0]["related_tasks"][0]["title"], ws.title);
        assert_eq!(input.iter().filter(|r| r["type"] == "session").count(), 2);
        assert_eq!(
            input
                .iter()
                .filter(|r| r["type"] == "session")
                .map(|r| r["session_id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![a.as_str(), b.as_str()]
        );
        assert!(!input.iter().any(|r| r["session_id"] == outside));
        assert!(!tools::input_available(
            &dir.join(format!("session-{outside}.jsonl"))
        ));
        assert!(input.iter().any(|r| r["session_id"] == b
            && r["current_summary"] == json!(crate::domain::Summary::single("已有 B 摘要"))));
        let b_record = input.iter().find(|r| r["session_id"] == b).unwrap();
        assert_eq!(b_record["summary_through_at"], "2026-09-01T00:00:00Z");
        assert_eq!(b_record["last_message_at"], "2026-10-01T00:00:00Z");
        assert_eq!(input[0]["current_summary"], Value::Null);
        assert!(!tools::input_available(
            &dir.join(format!("task-{}.jsonl", ws.id))
        ));
        result(dir, "session", &a, "A 已完成");
        result(dir, "project", &p.id, "项目当前结论");
        Ok(json!({"status":"partial","failed_sessions":[{"session_id":b,"error":"读取失败"}],"skipped_session_ids":[]}).to_string().into())
    };
    let out = project::extract_project(f.db(), &p.id, Some(&home), false, &mut runner).unwrap();
    assert_eq!(calls, 1);
    assert_eq!(out.status, ContextUpdateStatus::Partial);
    assert_eq!(out.remaining_pending, 1);
    assert_eq!(out.updated_sessions, vec![a.clone()]);
    assert!(session_context_view(f.db(), &b).unwrap().pending);
    assert_eq!(
        f.db()
            .get_workstream_context_state(&ws.id)
            .unwrap()
            .context_revision,
        0
    );
    assert_eq!(
        f.db()
            .get_session_context(&outside)
            .unwrap()
            .map(|c| c.revision),
        None
    );
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        let input = records(&dir.join(format!("project-{}.jsonl", p.id)));
        assert_eq!(
            input[0]["current_summary"],
            json!(crate::domain::Summary::single("项目当前结论"))
        );
        assert!(input
            .iter()
            .any(|r| r["session_id"] == a && r["need_update"] == false));
        let a_record = input.iter().find(|r| r["session_id"] == a).unwrap();
        assert_eq!(a_record["summary_through_at"], "2026-09-01T00:00:00Z");
        assert_eq!(a_record["last_message_at"], "2026-09-01T00:00:00Z");
        assert!(!tools::input_available(
            &dir.join(format!("session-{a}.jsonl"))
        ));
        result(dir, "session", &b, "B 已更新");
        result(dir, "project", &p.id, "project searchable conclusion");
        Ok(r#"{"status":"completed","skipped_session_ids":[]}"#.into())
    };
    let out = project::extract_project(f.db(), &p.id, Some(&home), false, &mut runner).unwrap();
    assert_eq!(out.updated_sessions, vec![b.clone()]);
    assert!(!project_context_view(f.db(), &p.id).unwrap().pending);
    let mut no_call =
        |_: &CliExtractor, _: &str, _: &Path| panic!("unchanged update must not invoke Agent");
    assert_eq!(
        project::extract_project(f.db(), &p.id, Some(&home), false, &mut no_call)
            .unwrap()
            .status,
        ContextUpdateStatus::NoChange
    );
    assert_eq!(
        project_context_view(f.db(), &p.id)
            .unwrap()
            .summary
            .as_ref()
            .map(|summary| summary.items[0].title.as_str()),
        Some("project searchable conclusion")
    );
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        let input = records(&dir.join(format!("project-{}.jsonl", p.id)));
        assert_eq!(input[0]["current_summary"], Value::Null);
        assert!(input
            .iter()
            .filter(|r| r["type"] == "session")
            .all(|r| r["need_update"] == false));
        result(dir, "project", &p.id, "重新生成的项目摘要");
        Ok(r#"{"status":"completed","skipped_session_ids":[]}"#.into())
    };
    project::extract_project(f.db(), &p.id, Some(&home), true, &mut runner).unwrap();
    assert_eq!(
        project_context_view(f.db(), &p.id)
            .unwrap()
            .summary
            .as_ref()
            .map(|summary| summary.items[0].title.as_str()),
        Some("重新生成的项目摘要")
    );
}

#[test]
fn project_metadata_and_membership_changes_mark_pending_without_discarding_summary() {
    let f = ServiceFixture::new();
    let home = f.home();
    let a = f.add_session(1, 10);
    seed(&f, &a, "A");
    let (mut p, path) = project_fixture(&f, "changes", &[&a]);
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        result(dir, "project", &p.id, "旧项目摘要");
        Ok(r#"{"status":"completed","skipped_session_ids":[]}"#.into())
    };
    project::extract_project(f.db(), &p.id, Some(&home), false, &mut runner).unwrap();
    p.name = "用户重命名".into();
    p.name_customized = true;
    f.db().upsert_project(&p).unwrap();
    f.db()
        .tx(|tx| {
            crate::storage::session_paths::attach_session_workspace_path_conn(tx, &a, None)?;
            crate::storage::workspace::delete_workspace_path_conn(tx, &path)?;
            Ok(())
        })
        .unwrap();
    let view = project_context_view(f.db(), &p.id).unwrap();
    assert!(view.pending);
    assert_eq!(
        view.summary
            .as_ref()
            .map(|summary| summary.items[0].title.as_str()),
        Some("旧项目摘要")
    );
    assert_eq!(view.context_revision, 1);
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        let input = records(&dir.join(format!("project-{}.jsonl", p.id)));
        assert_eq!(
            input[0]["current_summary"],
            json!(crate::domain::Summary::single("旧项目摘要"))
        );
        assert_eq!(input[0]["name"], "用户重命名");
        assert_eq!(input.iter().filter(|r| r["type"] == "session").count(), 0);
        Ok(r#"{"status":"skipped","reason":"no_summary_content","skipped_session_ids":[]}"#.into())
    };
    project::extract_project(f.db(), &p.id, Some(&home), false, &mut runner).unwrap();
    assert_eq!(
        project_context_view(f.db(), &p.id)
            .unwrap()
            .summary
            .as_ref()
            .map(|summary| summary.items[0].title.as_str()),
        Some("旧项目摘要")
    );
    assert!(!project_context_view(f.db(), &p.id).unwrap().pending);
    crate::workspace::project::set_project_archived(f.db(), &p.id, true).unwrap();
    crate::workspace::project::delete_project_permanently(
        f.db(),
        &p.id,
        &crate::workspace::project::UnrestrictedWorkspace,
    )
    .unwrap();
    let conn = f.db().read();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM project_context_state WHERE project_id=?1",
            [&p.id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM project_session_frontiers WHERE project_id=?1",
            [&p.id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn project_scope_changes_and_invalid_results_roll_back_and_shared_sessions_are_locked() {
    let f = ServiceFixture::new();
    let home = f.home();
    let a = f.add_session(1, 10);
    let ws = task(&f, &[&a]);
    let (p, _) = project_fixture(&f, "locked", &[&a]);
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        let mut must_not_run =
            |_: &CliExtractor, _: &str, _: &Path| panic!("locked extraction must not invoke Agent");
        assert_eq!(
            code(extract_session(
                f.db(),
                &a,
                Some(&home),
                false,
                &mut must_not_run
            )),
            "summary_busy"
        );
        assert_eq!(
            code(extract_task(
                f.db(),
                &ws.id,
                Some(&home),
                false,
                &mut must_not_run
            )),
            "summary_busy"
        );
        assert_eq!(
            code(project::extract_project(
                f.db(),
                &p.id,
                Some(&home),
                false,
                &mut must_not_run
            )),
            "summary_busy"
        );
        result(dir, "session", &a, "应回滚");
        std::fs::write(
            dir.join(format!("project-{}.result.json", p.id)),
            r#"{"summary":"bad","extra":1}"#,
        )
        .unwrap();
        Ok(r#"{"status":"completed","skipped_session_ids":[]}"#.into())
    };
    assert_eq!(
        code(project::extract_project(
            f.db(),
            &p.id,
            Some(&home),
            false,
            &mut runner
        )),
        "invalid_output"
    );
    assert!(f.db().get_session_context(&a).unwrap().is_none());
    assert_eq!(
        project_context_view(f.db(), &p.id)
            .unwrap()
            .context_revision,
        0
    );
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        result(dir, "session", &a, "应回滚");
        result(dir, "project", &p.id, "应回滚");
        f.db()
            .tx(|tx| {
                crate::storage::session_paths::attach_session_workspace_path_conn(tx, &a, None)?;
                Ok(())
            })
            .unwrap();
        Ok(r#"{"status":"completed","skipped_session_ids":[]}"#.into())
    };
    assert_eq!(
        code(project::extract_project(
            f.db(),
            &p.id,
            Some(&home),
            false,
            &mut runner
        )),
        "stale_snapshot"
    );
    assert!(f.db().get_session_context(&a).unwrap().is_none());
    assert_eq!(
        project_context_view(f.db(), &p.id)
            .unwrap()
            .context_revision,
        0
    );
}

#[test]
fn project_merge_preserves_survivor_summary_and_cascades_retired_summary_state() {
    let f = ServiceFixture::new();
    let home = f.home();
    let a = f.add_session(1, 10);
    let b = f.add_session(1, 10);
    seed(&f, &a, "A");
    seed(&f, &b, "B");
    let (mut p, _) = project_fixture(&f, "merge-survivor", &[&a]);
    p.name_customized = true;
    f.db().upsert_project(&p).unwrap();
    let (q, _) = project_fixture(&f, "merge-loser", &[&b]);
    for (id, body) in [
        (&p.id, "survivor current summary"),
        (&q.id, "loser current summary"),
    ] {
        let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
            result(dir, "project", id, body);
            Ok(r#"{"status":"completed","skipped_session_ids":[]}"#.into())
        };
        project::extract_project(f.db(), id, Some(&home), false, &mut runner).unwrap();
    }
    assert_eq!(
        crate::storage::project_context_repo::frontiers(&f.db().read(), &q.id)
            .unwrap()
            .len(),
        1
    );
    let merged = f
        .db()
        .tx(|tx| crate::workspace::project::merge_projects_conn(tx, &p.id, &q.id))
        .unwrap();
    assert_eq!(merged.survivor, p.id);
    assert!(f.db().get_project(&q.id).unwrap().is_none());
    let view = project_context_view(f.db(), &p.id).unwrap();
    assert_eq!(
        view.summary
            .as_ref()
            .map(|summary| summary.items[0].title.as_str()),
        Some("survivor current summary")
    );
    assert_eq!(view.context_revision, 1);
    assert!(view.pending);
    assert_eq!(view.pending_sessions, 1);
    assert_eq!(
        crate::storage::project_context_repo::state(&f.db().read(), &q.id)
            .unwrap()
            .revision,
        0
    );
    assert!(
        crate::storage::project_context_repo::frontiers(&f.db().read(), &q.id)
            .unwrap()
            .is_empty()
    );
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        let input = records(&dir.join(format!("project-{}.jsonl", p.id)));
        assert_eq!(
            input[0]["current_summary"],
            json!(crate::domain::Summary::single("survivor current summary"))
        );
        assert_eq!(input.iter().filter(|r| r["type"] == "session").count(), 2);
        assert!(input
            .iter()
            .filter(|r| r["type"] == "session")
            .all(|r| r["need_update"] == false));
        result(dir, "project", &p.id, "combined project summary");
        Ok(r#"{"status":"completed","skipped_session_ids":[]}"#.into())
    };
    project::extract_project(f.db(), &p.id, Some(&home), false, &mut runner).unwrap();
    assert!(!project_context_view(f.db(), &p.id).unwrap().pending);
}

#[test]
fn prompt_settings_persist_only_custom_templates_and_reset_independently() {
    use super::prompts::{self, PromptKind};
    let f = ServiceFixture::new();
    let defaults = prompts::settings(f.db()).unwrap();
    assert!(f
        .db()
        .get_setting("summary_prompt_templates")
        .unwrap()
        .is_none());
    prompts::save(f.db(), PromptKind::Session, Some("  自定义会话要求  ")).unwrap();
    prompts::save(f.db(), PromptKind::Task, Some("自定义任务要求")).unwrap();
    assert!(prompts::save(f.db(), PromptKind::Project, Some("  ")).is_err());
    let reopened = Db::open(&f.root.join("db.sqlite")).unwrap();
    let settings = prompts::settings(&reopened).unwrap();
    assert_eq!(settings.templates.session, "自定义会话要求");
    assert_eq!(settings.templates.task, "自定义任务要求");
    assert_eq!(settings.templates.project, defaults.defaults.project);
    let stored: Value = serde_json::from_str(
        &reopened
            .get_setting("summary_prompt_templates")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        stored,
        json!({"session":"自定义会话要求","task":"自定义任务要求"})
    );
    let reset = prompts::save(f.db(), PromptKind::Session, None).unwrap();
    assert_eq!(reset.templates.session, defaults.defaults.session);
    assert_eq!(reset.templates.task, "自定义任务要求");
    prompts::save(f.db(), PromptKind::Task, Some(&defaults.defaults.task)).unwrap();
    assert!(f
        .db()
        .get_setting("summary_prompt_templates")
        .unwrap()
        .is_none());
}

#[test]
fn configured_templates_reach_actual_session_task_project_calls_and_previews() {
    use super::prompts::{self, PromptKind};
    let f = ServiceFixture::new();
    let home = f.home();
    for (kind, text) in [
        (PromptKind::Session, "SESSION CUSTOM"),
        (PromptKind::Task, "TASK CUSTOM"),
        (PromptKind::Project, "PROJECT CUSTOM"),
    ] {
        prompts::save(f.db(), kind, Some(text)).unwrap();
    }
    let s = f.add_session(1, 10);
    let t = f.add_session(1, 10);
    let p_session = f.add_session(1, 10);
    let ws = task(&f, &[&t]);
    let (p, _) = project_fixture(&f, "prompt-project", &[&p_session]);
    let templates = prompts::templates(f.db()).unwrap();
    for (kind, custom) in [
        (PromptKind::Session, "SESSION CUSTOM"),
        (PromptKind::Task, "TASK CUSTOM"),
        (PromptKind::Project, "PROJECT CUSTOM"),
    ] {
        let preview = summary::preview_prompt(kind, &templates);
        assert!(preview.contains(custom));
        assert!(preview.contains("{{输入查询名称}}"));
        assert!(preview.contains("{{结果文件路径}}"));
        assert!(preview.contains("extraction"));
    }
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        let prompt =
            std::fs::read_to_string(dir.join("noending-extraction/references/requirements.md"))
                .unwrap();
        assert!(prompt.contains("SESSION CUSTOM"));
        assert!(prompt.contains(&format!("session-{s}.jsonl")));
        assert!(!prompt.contains("TASK CUSTOM"));
        result(dir, "session", &s, "single");
        Ok(r#"{"status":"completed"}"#.into())
    };
    extract_session(f.db(), &s, Some(&home), false, &mut runner).unwrap();
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        let prompt =
            std::fs::read_to_string(dir.join("noending-extraction/references/requirements.md"))
                .unwrap();
        assert!(prompt.contains("SESSION CUSTOM"));
        assert!(prompt.contains("TASK CUSTOM"));
        assert!(!prompt.contains("PROJECT CUSTOM"));
        result(dir, "session", &t, "task session");
        result(dir, "task", &ws.id, "task");
        Ok(r#"{"status":"completed","skipped_session_ids":[]}"#.into())
    };
    extract_task(f.db(), &ws.id, Some(&home), false, &mut runner).unwrap();
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        let prompt =
            std::fs::read_to_string(dir.join("noending-extraction/references/requirements.md"))
                .unwrap();
        assert!(prompt.contains("SESSION CUSTOM"));
        assert!(prompt.contains("PROJECT CUSTOM"));
        assert!(!prompt.contains("TASK CUSTOM"));
        result(dir, "session", &p_session, "project session");
        result(dir, "project", &p.id, "project");
        Ok(r#"{"status":"completed","skipped_session_ids":[]}"#.into())
    };
    project::extract_project(f.db(), &p.id, Some(&home), false, &mut runner).unwrap();
}

#[test]
fn structured_summary_updates_can_reorganize_all_fields_without_changing_extraction_frontiers() {
    let f = ServiceFixture::new();
    let home = f.home();
    let id = f.add_session(2, 10);
    let first = json!({"items":[
        {"title":"旧方案","description":"待验证","details":["保留目录","还有重复内容"]},
        {"title":"相关事项","description":"补充背景","details":["重复内容"]}
    ]});
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        assert_eq!(
            records(&dir.join(format!("session-{id}.jsonl")))[0]["current_summary"],
            Value::Null
        );
        std::fs::write(
            dir.join(format!("session-{id}.result.json")),
            json!({"summary":first}).to_string(),
        )
        .unwrap();
        Ok(r#"{"status":"completed"}"#.into())
    };
    extract_session(f.db(), &id, Some(&home), false, &mut runner).unwrap();
    append(&f, &id, "合并两项，调整结论并删去重复细则", false);
    let merged = json!({"items":[{"title":"最终方案","description":"已确认合并","details":["保留目录并统一说明"]}]});
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        let input = records(&dir.join(format!("session-{id}.jsonl")));
        assert_eq!(input[0]["current_summary"]["items"][0]["title"], "旧方案");
        assert_eq!(input[0]["current_summary"]["items"][1]["title"], "相关事项");
        assert_eq!(
            input[0]["current_summary"]["items"][0]["title_locked"],
            false
        );
        assert_eq!(input[0]["message_count"], 1);
        std::fs::write(
            dir.join(format!("session-{id}.result.json")),
            json!({"summary":merged}).to_string(),
        )
        .unwrap();
        Ok(r#"{"status":"completed"}"#.into())
    };
    extract_session(f.db(), &id, Some(&home), false, &mut runner).unwrap();
    let view = session_context_view(f.db(), &id).unwrap();
    assert_eq!(
        serde_json::to_value(view.fields.unwrap().summary).unwrap(),
        json!({"items":[{"title":"最终方案","description":"已确认合并","details":["保留目录并统一说明"],"title_locked":false}]})
    );
    assert_eq!(view.revision, 2);
    assert_eq!(view.processed_through_seq, 3);
    assert!(!view.pending);
}
