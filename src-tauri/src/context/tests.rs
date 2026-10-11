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

struct ServiceFixture {
    db: Option<Db>,
    root: PathBuf,
}

impl ServiceFixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "noending-context-service-{}",
            crate::storage::new_id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let db = Db::open(&root.join("db.sqlite")).unwrap();
        Self { db: Some(db), root }
    }

    fn db(&self) -> &Db {
        self.db.as_ref().unwrap()
    }

    fn home(&self) -> NoEndingHome {
        NoEndingHome::new(self.root.join("home").to_str().unwrap(), None).unwrap()
    }

    fn add_session(&self, message_count: usize, message_size: usize) -> String {
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
                source_position: i.to_string(),
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
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
fn empty_preferences(dir: &Path) {
    std::fs::write(dir.join("preferences.result.json"), r#"{"changes":[]}"#).unwrap();
}
fn result(dir: &Path, prefix: &str, id: &str, summary: &str) {
    std::fs::write(
        dir.join(format!("{prefix}-{id}.result.json")),
        json!({"summary":summary}).to_string(),
    )
    .unwrap();
}
fn task(f: &ServiceFixture, ids: &[&str]) -> Workstream {
    let ws = f.add_workstream("task-test");
    for id in ids {
        f.db().set_session_owner(id, Some(&ws.id)).unwrap();
    }
    ws
}
fn append(f: &ServiceFixture, id: &str, text: &str, rewrite: bool) {
    let old = f.db().get_session(id).unwrap().unwrap().source_cursor();
    f.db()
        .commit_ingest(
            id,
            &[ParsedSessionMessage {
                source_message_id: Some(crate::storage::new_id()),
                source_position: if rewrite {
                    "0".into()
                } else {
                    old.byte_offset.to_string()
                },
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
fn seed(f: &ServiceFixture, id: &str, body: &str) {
    let (generation, latest) = f.db().get_session_ingest_state(id).unwrap();
    f.db()
        .tx(|tx| {
            commit_session_context_conn(
                tx,
                id,
                &SessionContextFields {
                    summary: body.into(),
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
fn session_full_input_increment_regeneration_and_long_markdown() {
    let f = ServiceFixture::new();
    let home = f.home();
    let id = f.add_session(50, 6000);
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
        r#"{"summary":"stale"}"#,
    )
    .unwrap();
    let long = "**结论**：".repeat(500);
    let mut calls = 0;
    let mut runner = |_: &CliExtractor, prompt: &str, dir: &Path| {
        calls += 1;
        assert_eq!(dir, fixed);
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
        assert!(prompt.contains(&format!("session-{id}.jsonl")));
        assert!(!prompt.contains(&"x".repeat(6000)));
        result(dir, "session", &id, &long);
        empty_preferences(dir);
        Ok(r#"{"status":"completed","preferences":{"status":"completed"}}"#.into())
    };
    extract_session(f.db(), &id, Some(&home), false, &mut runner).unwrap();
    assert_eq!(calls, 1);
    assert!(!fixed.exists());
    append(&f, &id, "验证完成", false);
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        let input = records(&dir.join(format!("session-{id}.jsonl")));
        assert_eq!(input[0]["current_summary"], long);
        assert_eq!(input[0]["from_sequence"], 51);
        assert_eq!(input[0]["message_count"], 1);
        assert_eq!(input[0]["summary_through_at"], "2026-10-01T00:00:00Z");
        assert_eq!(input[0]["last_message_at"], Value::Null);
        result(dir, "session", &id, "**结果**：验证完成");
        empty_preferences(dir);
        Ok(r#"{"status":"completed","preferences":{"status":"completed"}}"#.into())
    };
    extract_session(f.db(), &id, Some(&home), false, &mut runner).unwrap();
    assert!(!session_context_view(f.db(), &id).unwrap().pending);
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        let input = records(&dir.join(format!("session-{id}.jsonl")));
        assert_eq!(input[0]["current_summary"], Value::Null);
        assert_eq!(input[0]["message_count"], 51);
        assert_eq!(input[0]["summary_through_at"], Value::Null);
        result(dir, "session", &id, "重新整理");
        empty_preferences(dir);
        Ok(r#"{"status":"completed","preferences":{"status":"completed"}}"#.into())
    };
    extract_session(f.db(), &id, Some(&home), true, &mut runner).unwrap();
    let ctx = f.db().get_session_context(&id).unwrap().unwrap();
    assert_eq!(ctx.revision, 3);
    assert_eq!(ctx.fields.summary, "重新整理");
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
    let mut skip = |_: &CliExtractor, _: &str, dir: &Path| {
        empty_preferences(dir);
        Ok(r#"{"status":"skipped","preferences":{"status":"completed"},"reason":"no_summary_content"}"#.into())
    };
    let out = extract_session(f.db(), &id, Some(&home), false, &mut skip).unwrap();
    assert_eq!(out.status, ContextUpdateStatus::Skipped);
    let view = session_context_view(f.db(), &id).unwrap();
    assert!(!view.pending);
    assert_eq!(view.fields.unwrap().summary, "有效旧摘要");
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
        assert!(input
            .iter()
            .any(|r| r["session_id"] == b && r["current_summary"] == "旧的可用结论"));
        result(dir, "session", &a, "会话 A 新结论");
        result(dir, "task", &ws.id, "结合 A 与 B 的旧结论");
        empty_preferences(dir);
        Ok(json!({"status":"partial","preferences":{"status":"completed"},"failed_sessions":[{"session_id":b,"error":"读取失败"}],"skipped_session_ids":[]}).to_string())
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
        "旧的可用结论"
    );
    assert!(session_context_view(f.db(), &b).unwrap().pending);
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        assert!(!dir.join(format!("session-{a}.jsonl")).exists());
        assert!(dir.join(format!("session-{b}.jsonl")).exists());
        let input = records(&dir.join(format!("task-{}.jsonl", ws.id)));
        assert_eq!(input[0]["current_summary"], "结合 A 与 B 的旧结论");
        result(dir, "session", &b, "B 新结论");
        result(dir, "task", &ws.id, "完整最新结论");
        empty_preferences(dir);
        Ok(r#"{"status":"completed","preferences":{"status":"completed"},"skipped_session_ids":[]}"#.into())
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
        empty_preferences(dir);
        Ok(json!({"status":"completed","preferences":{"status":"completed"},"skipped_session_ids":[id]}).to_string())
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
        "旧摘要"
    );
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        let input = records(&dir.join(format!("task-{}.jsonl", ws.id)));
        assert_eq!(input[0]["current_summary"], Value::Null);
        assert!(!dir.join(format!("session-{id}.jsonl")).exists());
        empty_preferences(dir);
        Ok(r#"{"status":"skipped","preferences":{"status":"completed"},"reason":"no_summary_content","skipped_session_ids":[]}"#.into())
    };
    extract_task(f.db(), &ws.id, Some(&home), true, &mut runner).unwrap();
    assert_eq!(
        workstream_context_view(f.db(), &ws.id)
            .unwrap()
            .summary
            .as_deref(),
        Some("任务当前结论")
    );
}

#[test]
fn task_changes_retain_current_summaries_until_explicit_regeneration() {
    let f = ServiceFixture::new();
    let home = f.home();
    let a = f.add_session(1, 10);
    let b = f.add_session(1, 10);
    let ws = task(&f, &[&a, &b]);
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        result(dir, "session", &a, "A");
        result(dir, "session", &b, "B");
        result(dir, "task", &ws.id, "旧任务摘要");
        empty_preferences(dir);
        Ok(r#"{"status":"completed","preferences":{"status":"completed"},"skipped_session_ids":[]}"#.into())
    };
    extract_task(f.db(), &ws.id, Some(&home), false, &mut runner).unwrap();
    f.db().set_session_owner(&b, None).unwrap();
    append(&f, &a, "改写后的历史", true);
    assert_eq!(
        session_context_view(f.db(), &a)
            .unwrap()
            .fields
            .unwrap()
            .summary,
        "A"
    );
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        let input = records(&dir.join(format!("task-{}.jsonl", ws.id)));
        assert_eq!(input[0]["current_summary"], "旧任务摘要");
        assert!(!input.iter().any(|r| r["session_id"] == b));
        let session = records(&dir.join(format!("session-{a}.jsonl")));
        assert_eq!(session[0]["current_summary"], "A");
        assert_eq!(session[0]["message_count"], 1);
        result(dir, "session", &a, "改写结果");
        result(dir, "task", &ws.id, "新任务摘要");
        empty_preferences(dir);
        Ok(r#"{"status":"completed","preferences":{"status":"completed"},"skipped_session_ids":[]}"#.into())
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
            r#"{"status":"completed","preferences":{"status":"completed"},"skipped_session_ids":[]}"#,
            r#"{"summary":"valid"}"#,
        ),
        (
            r#"{"status":"completed","preferences":{"status":"completed"},"skipped_session_ids":["unknown"]}"#,
            r#"{"summary":"valid"}"#,
        ),
        (
            r#"{"status":"completed","preferences":{"status":"completed"},"skipped_session_ids":[]}"#,
            r#"{"summary":"","extra":true}"#,
        ),
        (
            r#"{"status":"partial","preferences":{"status":"completed"},"failed_sessions":[{"session_id":"unknown","error":"error"}],"skipped_session_ids":[]}"#,
            r#"{"summary":"valid"}"#,
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
        empty_preferences(dir);
        Ok(r#"{"status":"completed","preferences":{"status":"completed"}}"#.into())
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
        empty_preferences(dir);
        Ok(r#"{"status":"completed","preferences":{"status":"completed"}}"#.into())
    };
    extract_session(f.db(), &id, Some(&home), false, &mut runner).unwrap();
    let view = session_context_view(f.db(), &id).unwrap();
    assert!(view.pending);
    assert_eq!(view.processed_through_seq, 1);
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        append(&f, &id, "rewritten", true);
        result(dir, "session", &id, "stale");
        empty_preferences(dir);
        Ok(r#"{"status":"completed","preferences":{"status":"completed"}}"#.into())
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
        f.db().set_session_owner(&id, None).unwrap();
        result(dir, "session", &id, "stale");
        result(dir, "task", &ws.id, "stale");
        empty_preferences(dir);
        Ok(r#"{"status":"completed","preferences":{"status":"completed"},"skipped_session_ids":[]}"#.into())
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
        r#"{"status":"completed","preferences":{"status":"completed"},"extra":true}"#,
        r#"{"status":"completed","preferences":{"status":"completed"},"status":"failed","error":"x"}"#,
        r#"{"status":"skipped","preferences":{"status":"completed"},"reason":"other"}"#,
        r#"{"status":"failed","error":" "}"#,
        r#"```json
{"status":"completed","preferences":{"status":"completed"}}
```"#,
    ] {
        assert!(summary::parse_session_reply(invalid).is_err(), "{invalid}");
    }
    for invalid in [
        r#"{"status":"completed","preferences":{"status":"completed"}}"#,
        r#"{"status":"partial","preferences":{"status":"completed"},"failed_sessions":[],"skipped_session_ids":[]}"#,
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
        r#"{"summary":" "}"#,
        r#"{"summary":"a","summary":"b"}"#,
        r#"{"summary":"a","revision":10}"#,
        r#"{"error":"failure"}"#,
    ] {
        let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
            std::fs::write(dir.join(format!("session-{id}.result.json")), body).unwrap();
            empty_preferences(dir);
            Ok(r#"{"status":"completed","preferences":{"status":"completed"}}"#.into())
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
        assert_eq!(stored.fields.summary, "valid summary");
        assert_eq!(stored.processed_through_seq, 1);
        assert_eq!(stored.revision, 1);
    }
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        std::fs::create_dir(dir.join(format!("session-{id}.result.json"))).unwrap();
        empty_preferences(dir);
        Ok(r#"{"status":"completed","preferences":{"status":"completed"}}"#.into())
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
        std::fs::write(&outside, r#"{"summary":"outside"}"#).unwrap();
        let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
            std::os::unix::fs::symlink(&outside, dir.join(format!("session-{id}.result.json")))
                .unwrap();
            empty_preferences(dir);
            Ok(r#"{"status":"completed","preferences":{"status":"completed"}}"#.into())
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
    f.db().set_setting("assistant.agent", "none").unwrap();
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
    let (p, path) = project_fixture(&f, "scope", &[&a, &b]);
    let (_, other_path) = project_fixture(&f, "outside", &[&outside]);
    f.db()
        .tx(|tx| {
            crate::storage::workstream_paths::append_workstream_path_conn(tx, &ws.id, &path)?;
            crate::storage::workstream_paths::append_workstream_path_conn(tx, &ws.id, &other_path)?;
            tx.execute(
                "UPDATE sessions SET archived_at='2026-10-11' WHERE id=?1",
                [&b],
            )?;
            Ok(())
        })
        .unwrap();
    let mut calls = 0;
    let mut runner = |_: &CliExtractor, prompt: &str, dir: &Path| {
        calls += 1;
        assert!(prompt.contains("END_OF_PROJECT_INPUT"));
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
        assert!(!dir.join(format!("session-{outside}.jsonl")).exists());
        assert!(input
            .iter()
            .any(|r| r["session_id"] == b && r["current_summary"] == "已有 B 摘要"));
        let b_record = input.iter().find(|r| r["session_id"] == b).unwrap();
        assert_eq!(b_record["summary_through_at"], "2026-09-01T00:00:00Z");
        assert_eq!(b_record["last_message_at"], "2026-10-01T00:00:00Z");
        assert_eq!(input[0]["current_summary"], Value::Null);
        assert!(!dir.join(format!("task-{}.jsonl", ws.id)).exists());
        result(dir, "session", &a, "A 已完成");
        result(dir, "project", &p.id, "项目当前结论");
        empty_preferences(dir);
        Ok(json!({"status":"partial","preferences":{"status":"completed"},"failed_sessions":[{"session_id":b,"error":"读取失败"}],"skipped_session_ids":[]}).to_string())
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
        assert_eq!(input[0]["current_summary"], "项目当前结论");
        assert!(input
            .iter()
            .any(|r| r["session_id"] == a && r["need_update"] == false));
        let a_record = input.iter().find(|r| r["session_id"] == a).unwrap();
        assert_eq!(a_record["summary_through_at"], "2026-09-01T00:00:00Z");
        assert_eq!(a_record["last_message_at"], "2026-09-01T00:00:00Z");
        assert!(!dir.join(format!("session-{a}.jsonl")).exists());
        result(dir, "session", &b, "B 已更新");
        result(dir, "project", &p.id, "project searchable conclusion");
        empty_preferences(dir);
        Ok(r#"{"status":"completed","preferences":{"status":"completed"},"skipped_session_ids":[]}"#.into())
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
    f.db().index_project(&p).unwrap();
    assert_eq!(
        f.db()
            .read()
            .query_row(
                "SELECT body FROM search_index WHERE kind='project' AND ref_id=?1",
                [&p.id],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "project searchable conclusion"
    );
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        let input = records(&dir.join(format!("project-{}.jsonl", p.id)));
        assert_eq!(input[0]["current_summary"], Value::Null);
        assert!(input
            .iter()
            .filter(|r| r["type"] == "session")
            .all(|r| r["need_update"] == false));
        result(dir, "project", &p.id, "重新生成的项目摘要");
        empty_preferences(dir);
        Ok(r#"{"status":"completed","preferences":{"status":"completed"},"skipped_session_ids":[]}"#.into())
    };
    project::extract_project(f.db(), &p.id, Some(&home), true, &mut runner).unwrap();
    assert_eq!(
        project_context_view(f.db(), &p.id)
            .unwrap()
            .summary
            .as_deref(),
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
        empty_preferences(dir);
        Ok(r#"{"status":"completed","preferences":{"status":"completed"},"skipped_session_ids":[]}"#.into())
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
    assert_eq!(view.summary.as_deref(), Some("旧项目摘要"));
    assert_eq!(view.context_revision, 1);
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        let input = records(&dir.join(format!("project-{}.jsonl", p.id)));
        assert_eq!(input[0]["current_summary"], "旧项目摘要");
        assert_eq!(input[0]["name"], "用户重命名");
        assert_eq!(input.iter().filter(|r| r["type"] == "session").count(), 0);
        empty_preferences(dir);
        Ok(r#"{"status":"skipped","preferences":{"status":"completed"},"reason":"no_summary_content","skipped_session_ids":[]}"#.into())
    };
    project::extract_project(f.db(), &p.id, Some(&home), false, &mut runner).unwrap();
    assert_eq!(
        project_context_view(f.db(), &p.id)
            .unwrap()
            .summary
            .as_deref(),
        Some("旧项目摘要")
    );
    assert!(!project_context_view(f.db(), &p.id).unwrap().pending);
    f.db()
        .tx(|tx| {
            crate::storage::workspace::retire_project_if_unowned(tx, &p.id)?;
            Ok(())
        })
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
        empty_preferences(dir);
        Ok(r#"{"status":"completed","preferences":{"status":"completed"},"skipped_session_ids":[]}"#.into())
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
        empty_preferences(dir);
        Ok(r#"{"status":"completed","preferences":{"status":"completed"},"skipped_session_ids":[]}"#.into())
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
            empty_preferences(dir);
            Ok(r#"{"status":"completed","preferences":{"status":"completed"},"skipped_session_ids":[]}"#.into())
        };
        project::extract_project(f.db(), id, Some(&home), false, &mut runner).unwrap();
    }
    assert_eq!(
        crate::storage::project_context_repo::frontiers(&f.db().read(), &q.id)
            .unwrap()
            .len(),
        1
    );
    let preference =
        crate::storage::preferences::save_item(f.db(), None, Some(&q.id), "合并后仍保留", true)
            .unwrap();
    let merged = f
        .db()
        .tx(|tx| crate::workspace::project::merge_projects_conn(tx, &p.id, &q.id))
        .unwrap();
    assert_eq!(merged.survivor, p.id);
    let moved = crate::storage::preferences::list_items(f.db(), Some(&p.id)).unwrap();
    assert!(moved
        .iter()
        .any(|v| v.id == preference.id && v.locked && v.content == "合并后仍保留"));
    assert!(f.db().get_project(&q.id).unwrap().is_none());
    let view = project_context_view(f.db(), &p.id).unwrap();
    assert_eq!(view.summary.as_deref(), Some("survivor current summary"));
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
        assert_eq!(input[0]["current_summary"], "survivor current summary");
        assert_eq!(input.iter().filter(|r| r["type"] == "session").count(), 2);
        assert!(input
            .iter()
            .filter(|r| r["type"] == "session")
            .all(|r| r["need_update"] == false));
        result(dir, "project", &p.id, "combined project summary");
        empty_preferences(dir);
        Ok(r#"{"status":"completed","preferences":{"status":"completed"},"skipped_session_ids":[]}"#.into())
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
        (PromptKind::Preferences, "PREFERENCES CUSTOM"),
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
        (PromptKind::Preferences, "PREFERENCES CUSTOM"),
    ] {
        let preview = summary::preview_prompt(kind, &templates);
        assert!(preview.contains(custom));
        assert!(preview.contains("{{输入文件路径}}"));
        assert!(preview.contains("{{结果文件路径}}"));
        assert!(preview.contains(r#"{"summary":"完整 Markdown 摘要"}"#));
        let single = r#"{"status":"completed","preferences":{"status":"completed"}}"#;
        let batch = r#"{"status":"completed","skipped_session_ids":[],"preferences":{"status":"completed"}}"#;
        match kind {
            PromptKind::Task | PromptKind::Project => {
                assert!(preview.contains(batch));
                assert!(!preview.contains(single));
            }
            PromptKind::Session | PromptKind::Preferences => {
                assert!(preview.contains(single));
                assert!(!preview.contains(batch));
            }
        }
    }
    let mut runner = |_: &CliExtractor, prompt: &str, dir: &Path| {
        assert!(prompt.contains("SESSION CUSTOM"));
        assert!(prompt.contains("PREFERENCES CUSTOM"));
        assert!(prompt.contains(&format!("session-{s}.jsonl")));
        assert!(!prompt.contains("TASK CUSTOM"));
        result(dir, "session", &s, "single");
        empty_preferences(dir);
        Ok(r#"{"status":"completed","preferences":{"status":"completed"}}"#.into())
    };
    extract_session(f.db(), &s, Some(&home), false, &mut runner).unwrap();
    let mut runner = |_: &CliExtractor, prompt: &str, dir: &Path| {
        assert!(prompt.contains("SESSION CUSTOM"));
        assert!(prompt.contains("PREFERENCES CUSTOM"));
        assert!(prompt.contains("TASK CUSTOM"));
        assert!(!prompt.contains("PROJECT CUSTOM"));
        result(dir, "session", &t, "task session");
        result(dir, "task", &ws.id, "task");
        empty_preferences(dir);
        Ok(r#"{"status":"completed","preferences":{"status":"completed"},"skipped_session_ids":[]}"#.into())
    };
    extract_task(f.db(), &ws.id, Some(&home), false, &mut runner).unwrap();
    let mut runner = |_: &CliExtractor, prompt: &str, dir: &Path| {
        assert!(prompt.contains("SESSION CUSTOM"));
        assert!(prompt.contains("PREFERENCES CUSTOM"));
        assert!(prompt.contains("PROJECT CUSTOM"));
        assert!(!prompt.contains("TASK CUSTOM"));
        result(dir, "session", &p_session, "project session");
        result(dir, "project", &p.id, "project");
        empty_preferences(dir);
        Ok(r#"{"status":"completed","preferences":{"status":"completed"},"skipped_session_ids":[]}"#.into())
    };
    project::extract_project(f.db(), &p.id, Some(&home), false, &mut runner).unwrap();
}

fn preference_result(dir: &Path, changes: Value) {
    std::fs::write(
        dir.join("preferences.result.json"),
        json!({"changes":changes}).to_string(),
    )
    .unwrap();
}

#[test]
fn users_manage_locked_items_and_current_preferences_persist_without_history() {
    use crate::storage::preferences as repo;
    let f = ServiceFixture::new();
    let (p, _) = project_fixture(&f, "preferences", &[]);
    let global = repo::save_item(f.db(), None, None, " 使用中文 ", true).unwrap();
    assert_eq!(global.content, "使用中文");
    assert!(repo::save_item(f.db(), None, None, "使用中文", false).is_err());
    assert!(repo::save_item(f.db(), None, None, "  ", false).is_err());
    assert!(repo::save_item(f.db(), None, Some("unknown-project"), "准则", false).is_err());
    assert!(repo::save_item(f.db(), Some(&global.id), Some(&p.id), "准则", true).is_err());
    let updated = repo::save_item(f.db(), Some(&global.id), None, "回复使用中文", true).unwrap();
    assert!(updated.locked);
    let reopened = Db::open(&f.root.join("db.sqlite")).unwrap();
    assert_eq!(
        repo::list_items(&reopened, None).unwrap()[0].content,
        "回复使用中文"
    );
    repo::delete_item(f.db(), &global.id).unwrap();
    assert!(repo::list_items(f.db(), None).unwrap().is_empty());
    let item = repo::save_item(f.db(), None, Some(&p.id), "项目准则", true).unwrap();
    f.db()
        .tx(|tx| {
            tx.execute("DELETE FROM workspace_paths WHERE project_id=?1", [&p.id])?;
            tx.execute("DELETE FROM projects WHERE id=?1", [&p.id])?;
            Ok(())
        })
        .unwrap();
    assert!(repo::get(&f.db().read(), &item.id).unwrap().is_none());
}

#[test]
fn session_skip_can_apply_global_and_project_preferences_in_same_invocation() {
    use crate::storage::preferences as repo;
    let f = ServiceFixture::new();
    let home = f.home();
    let id = f.add_session(1, 10);
    let (p, _) = project_fixture(&f, "preference-session", &[&id]);
    let edit = repo::save_item(f.db(), None, None, "旧要求", false).unwrap();
    let remove = repo::save_item(f.db(), None, Some(&p.id), "撤销的要求", false).unwrap();
    let locked = repo::save_item(f.db(), None, None, "锁定准则", true).unwrap();
    let mut calls = 0;
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        calls += 1;
        let input: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("preferences.input.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(input["projects"][0]["project_id"], p.id);
        assert!(input["user"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["id"] == locked.id && v["locked"] == true));
        assert_eq!(
            records(&dir.join(format!("session-{id}.jsonl")))[0]["project_id"],
            p.id
        );
        preference_result(
            dir,
            json!([
                {"scope":"user","action":"update","id":edit.id,"content":"新要求"},
                {"scope":"project","project_id":p.id,"action":"delete","id":remove.id},
                {"scope":"project","project_id":p.id,"action":"add","content":"项目新准则"},
                {"scope":"user","action":"add","content":"用户新偏好"}
            ]),
        );
        Ok(r#"{"status":"skipped","reason":"no_summary_content","preferences":{"status":"completed"}}"#.into())
    };
    let out = extract_session(f.db(), &id, Some(&home), false, &mut runner).unwrap();
    assert_eq!(calls, 1);
    assert!(out.preference_error.is_none());
    assert!(!session_context_view(f.db(), &id).unwrap().pending);
    let user = repo::list_items(f.db(), None).unwrap();
    assert_eq!(user.len(), 3);
    assert!(user
        .iter()
        .any(|v| v.id == edit.id && v.content == "新要求"));
    assert!(user.iter().any(|v| v.id == locked.id && v.locked));
    let project = repo::list_items(f.db(), Some(&p.id)).unwrap();
    assert_eq!(project.len(), 1);
    assert_eq!(project[0].content, "项目新准则");
    assert!(!project[0].locked);
}

#[test]
fn invalid_preference_files_and_changes_preserve_summaries_without_partial_writes() {
    use crate::storage::preferences as repo;
    let f = ServiceFixture::new();
    let home = f.home();
    let id = f.add_session(1, 10);
    let (p, _) = project_fixture(&f, "outside-preferences", &[]);
    let locked = repo::save_item(f.db(), None, None, "锁定准则", true).unwrap();
    let unlocked = repo::save_item(f.db(), None, None, "普通准则", false).unwrap();
    let bad_changes = vec![
        json!({"scope":"user","action":"update","id":locked.id,"content":"覆盖锁定"}),
        json!({"scope":"user","action":"delete","id":locked.id}),
        json!({"scope":"user","action":"update","id":"unknown","content":"未知"}),
        json!({"scope":"project","project_id":p.id,"action":"add","content":"越界"}),
        json!({"scope":"user","project_id":p.id,"action":"add","content":"错范围"}),
        json!({"scope":"project","action":"add","content":"缺项目"}),
        json!({"scope":"user","action":"add","content":" "}),
        json!({"scope":"user","action":"add","content":"锁定准则"}),
        json!({"scope":"user","action":"add","id":"invented","content":"多余ID"}),
        json!({"scope":"user","action":"update","id":unlocked.id,"content":"普通准则","locked":true}),
    ];
    for bad in bad_changes {
        let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
            result(dir, "session", &id, "有效摘要");
            preference_result(
                dir,
                json!([{"scope":"user","action":"add","content":"不能部分保存"},bad]),
            );
            Ok(r#"{"status":"completed","preferences":{"status":"completed"}}"#.into())
        };
        let out = extract_session(f.db(), &id, Some(&home), true, &mut runner).unwrap();
        assert!(out.preference_error.is_some(), "{bad}");
        assert_eq!(repo::list_items(f.db(), None).unwrap().len(), 2);
        assert_eq!(
            session_context_view(f.db(), &id)
                .unwrap()
                .fields
                .unwrap()
                .summary,
            "有效摘要"
        );
    }
    for malformed in [
        None,
        Some("not JSON"),
        Some(r#"{"changes":[],"extra":true}"#),
    ] {
        let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
            result(dir, "session", &id, "仍然有效");
            if let Some(text) = malformed {
                std::fs::write(dir.join("preferences.result.json"), text).unwrap();
            }
            Ok(r#"{"status":"completed","preferences":{"status":"completed"}}"#.into())
        };
        assert!(extract_session(f.db(), &id, Some(&home), true, &mut runner)
            .unwrap()
            .preference_error
            .is_some());
        assert_eq!(
            session_context_view(f.db(), &id)
                .unwrap()
                .fields
                .unwrap()
                .summary,
            "仍然有效"
        );
    }
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        result(dir, "session", &id, "摘要成功，偏好明确失败");
        Ok(r#"{"status":"completed","preferences":{"status":"failed","error":"未能完整读取偏好输入"}}"#.into())
    };
    let out = extract_session(f.db(), &id, Some(&home), true, &mut runner).unwrap();
    assert!(out.preference_error.unwrap().contains("未能完整读取"));
}

#[test]
fn preference_snapshot_rejects_user_edits_deletes_locks_and_concurrent_additions() {
    use crate::storage::preferences as repo;
    for mutation in ["edit", "delete", "lock", "add", "edit-and-restore"] {
        let f = ServiceFixture::new();
        let home = f.home();
        let id = f.add_session(1, 10);
        let item = repo::save_item(f.db(), None, None, "原要求", false).unwrap();
        let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
            match mutation {
                "edit" => {
                    repo::save_item(f.db(), Some(&item.id), None, "用户新要求", false).unwrap();
                }
                "delete" => {
                    repo::delete_item(f.db(), &item.id).unwrap();
                }
                "lock" => {
                    repo::save_item(f.db(), Some(&item.id), None, &item.content, true).unwrap();
                }
                "add" => {
                    repo::save_item(f.db(), None, None, "并发新增", false).unwrap();
                }
                _ => {
                    repo::save_item(f.db(), Some(&item.id), None, "暂时编辑", false).unwrap();
                    repo::save_item(f.db(), Some(&item.id), None, &item.content, false).unwrap();
                }
            }
            result(dir, "session", &id, "摘要可保存");
            preference_result(
                dir,
                json!([{"scope":"user","action":"update","id":item.id,"content":"Agent 旧结果"}]),
            );
            Ok(r#"{"status":"completed","preferences":{"status":"completed"}}"#.into())
        };
        let out = extract_session(f.db(), &id, Some(&home), false, &mut runner).unwrap();
        assert!(out.preference_error.is_some(), "{mutation}");
        assert!(repo::list_items(f.db(), None)
            .unwrap()
            .iter()
            .all(|p| p.content != "Agent 旧结果"));
        assert_eq!(
            session_context_view(f.db(), &id)
                .unwrap()
                .fields
                .unwrap()
                .summary,
            "摘要可保存"
        );
    }
}

#[test]
fn task_and_project_calls_each_emit_one_preference_result_for_their_project_scopes() {
    use crate::storage::preferences as repo;
    let f = ServiceFixture::new();
    let home = f.home();
    let a = f.add_session(1, 10);
    let b = f.add_session(1, 10);
    let (p, _) = project_fixture(&f, "multi-a", &[&a]);
    let (q, _) = project_fixture(&f, "multi-b", &[&b]);
    let ws = task(&f, &[&a, &b]);
    let mut calls = 0;
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        calls += 1;
        let input: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("preferences.input.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(input["projects"].as_array().unwrap().len(), 2);
        result(dir, "session", &a, "A");
        result(dir, "session", &b, "B");
        result(dir, "task", &ws.id, "任务摘要");
        preference_result(
            dir,
            json!([
                {"scope":"user","action":"add","content":"全局偏好"},
                {"scope":"project","project_id":p.id,"action":"add","content":"P 准则"},
                {"scope":"project","project_id":q.id,"action":"add","content":"Q 准则"}
            ]),
        );
        Ok(r#"{"status":"completed","skipped_session_ids":[],"preferences":{"status":"completed"}}"#.into())
    };
    assert!(
        extract_task(f.db(), &ws.id, Some(&home), false, &mut runner)
            .unwrap()
            .preference_error
            .is_none()
    );
    assert_eq!(calls, 1);
    append(&f, &a, "项目偏好调整", false);
    let q_item = repo::list_items(f.db(), Some(&q.id)).unwrap().remove(0);
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        let input: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("preferences.input.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(input["projects"].as_array().unwrap().len(), 1);
        assert_eq!(input["projects"][0]["project_id"], p.id);
        result(dir, "session", &a, "A 更新");
        result(dir, "project", &p.id, "项目摘要");
        preference_result(
            dir,
            json!([{"scope":"project","project_id":q.id,"action":"delete","id":q_item.id}]),
        );
        Ok(r#"{"status":"completed","skipped_session_ids":[],"preferences":{"status":"completed"}}"#.into())
    };
    let out = project::extract_project(f.db(), &p.id, Some(&home), false, &mut runner).unwrap();
    assert!(out.preference_error.is_some());
    assert_eq!(
        repo::list_items(f.db(), Some(&q.id)).unwrap()[0].content,
        "Q 准则"
    );
    assert_eq!(
        project_context_view(f.db(), &p.id)
            .unwrap()
            .summary
            .unwrap(),
        "项目摘要"
    );
    let mut runner = |_: &CliExtractor, _: &str, dir: &Path| {
        let input = records(&dir.join(format!("project-{}.jsonl", p.id)));
        let session = input.iter().find(|r| r["session_id"] == a).unwrap();
        assert_eq!(session["need_update"], false);
        assert_eq!(session["current_summary"], "A 更新");
        let raw = records(Path::new(
            session["preference_input_file"].as_str().unwrap(),
        ));
        assert_eq!(raw[0]["from_sequence"], 1);
        assert_eq!(raw[0]["message_count"], 2);
        assert!(!dir.join(format!("session-{a}.result.json")).exists());
        result(dir, "project", &p.id, "项目重新生成");
        preference_result(
            dir,
            json!([{"scope":"project","project_id":p.id,"action":"add","content":"从原始消息重试的准则"}]),
        );
        Ok(r#"{"status":"completed","skipped_session_ids":[],"preferences":{"status":"completed"}}"#.into())
    };
    let out = project::extract_project(f.db(), &p.id, Some(&home), true, &mut runner).unwrap();
    assert!(out.preference_error.is_none());
    assert_eq!(
        f.db()
            .get_session_context(&a)
            .unwrap()
            .unwrap()
            .fields
            .summary,
        "A 更新"
    );
    assert!(repo::list_items(f.db(), Some(&p.id))
        .unwrap()
        .iter()
        .any(|p| p.content == "从原始消息重试的准则"));
}
