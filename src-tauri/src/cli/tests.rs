use super::*;
use crate::domain::{Agent, ParsedSessionMessage, SessionMessageRole, SourceCursorUpdate};
use crate::storage::{now, Db};
use crate::workspace::{home::NoEndingHome, wiring::WorkspaceLayer};
use std::sync::Arc;

struct Fixture {
    root: PathBuf,
    home: NoEndingHome,
    server: Option<Server>,
    db: Option<Arc<Db>>,
    changes: Arc<std::sync::atomic::AtomicUsize>,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("noending-cli-{}", crate::storage::new_id()));
        let home = NoEndingHome::new(root.join("home").to_str().unwrap(), None).unwrap();
        home.ensure_dirs().unwrap();
        let db = Arc::new(Db::open(&home.db_path).unwrap());
        let layer = Arc::new(WorkspaceLayer::new(&home));
        let changes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let notifications = changes.clone();
        let server = start(db.clone(), home.clone(), layer, move || {
            notifications.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        })
        .unwrap();
        Self {
            root,
            home,
            server: Some(server),
            db: Some(db),
            changes,
        }
    }
    fn db(&self) -> &Db {
        self.db.as_ref().unwrap()
    }
    fn call(&self, args: &[&str]) -> CliResult<Value> {
        let mut argv = vec![
            "--connection".into(),
            self.home
                .runtime_dir
                .join("cli.json")
                .to_string_lossy()
                .into_owned(),
        ];
        argv.extend(args.iter().map(|s| s.to_string()));
        execute(&argv)
    }
    fn session(&self) -> String {
        let id = self
            .db()
            .upsert_logical_session(
                Agent::Codex,
                &crate::storage::new_id(),
                None,
                Some("测试会话"),
                None,
                None,
                None,
                None,
                None,
                "test",
                "/test/source",
                &json!({}),
            )
            .unwrap()
            .0;
        self.ingest(&id, &format!("{}乙", "甲".repeat(2000)), "m1", 0);
        id
    }
    fn ingest(&self, id: &str, content: &str, message: &str, offset: u64) {
        self.db()
            .commit_ingest(
                id,
                &[ParsedSessionMessage {
                    source_message_id: Some(message.into()),
                    source_position: message.into(),
                    role: SessionMessageRole::User,
                    ts: Some(now()),
                    content: content.into(),
                }],
                &SourceCursorUpdate {
                    file_identity: "test".into(),
                    generation: 0,
                    byte_offset: offset + 100,
                    last_seen_size: offset + 100,
                    mtime: None,
                    start_byte_offset: offset,
                    prefix_hash: String::new(),
                },
            )
            .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        drop(self.server.take());
        drop(self.db.take());
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
#[test]
fn cli_pages_search_whole_messages_preserve_order_cutoff_and_reject_rewritten_history() {
    let f = Fixture::new();
    let id = f.session();
    let first = f
        .call(&["session", "messages", &id, "--limit", "1"])
        .unwrap();
    assert_eq!(first["records"][0]["record"]["sequence"], 1);
    assert_eq!(first["records"][0]["record"]["content"], "甲".repeat(2000));
    assert!(!first["records"][0]["record"]["message_id"]
        .as_str()
        .unwrap()
        .is_empty());
    let cursor = first["next_cursor"].as_str().unwrap();
    f.ingest(&id, "查询开始后新增", "m2", 100);
    let next = f
        .call(&[
            "session", "messages", &id, "--cursor", cursor, "--limit", "1",
        ])
        .unwrap();
    assert_eq!(next["records"][0]["record"]["content"], "乙");
    assert_eq!(next["end"], true);
    let search = f
        .call(&["session", "search", &id, "--query", "甲乙"])
        .unwrap();
    assert_eq!(search["records"].as_array().unwrap().len(), 2);
    let all = f.call(&["session", "messages", &id]).unwrap();
    assert_eq!(all["through_sequence"], 2);
    f.ingest(&id, "历史改写", "replacement", 0);
    assert_eq!(
        f.call(&["session", "messages", &id, "--cursor", cursor])
            .unwrap_err()
            .code,
        "stale_cursor"
    );
}
#[test]
fn cli_preferences_respect_current_lock_and_summary_edits_preserve_pending_messages() {
    let f = Fixture::new();
    let pref = f
        .call(&[
            "preference",
            "add",
            "--scope",
            "user",
            "--content",
            "保持简洁",
        ])
        .unwrap();
    let id = pref["id"].as_str().unwrap();
    f.call(&["preference", "update", id, "--content", "说明要点"])
        .unwrap();
    crate::storage::preferences::save_item(f.db(), Some(id), None, "说明要点", true).unwrap();
    assert_eq!(
        f.call(&["preference", "delete", id]).unwrap_err().code,
        "locked"
    );
    assert_eq!(
        f.call(&["preference", "update", id, "--content", "不能覆盖"])
            .unwrap_err()
            .code,
        "locked"
    );
    assert_eq!(
        f.call(&["preference", "list", "--scope", "user"]).unwrap()["items"][0]["content"],
        "说明要点"
    );
    let sid = f.session();
    let file = f.root.join("summary.md");
    std::fs::write(&file, "### 主题\n\n- 人工修正。\n").unwrap();
    let path = file.to_str().unwrap();
    assert_eq!(
        f.call(&[
            "summary",
            "set",
            "session",
            &sid,
            "--file",
            path,
            "--revision",
            "0"
        ])
        .unwrap()["revision"],
        1
    );
    let view = f.call(&["session", "summary", &sid]).unwrap();
    assert_eq!(view["processed_through_seq"], 0);
    assert_eq!(view["pending"], true);
    assert_eq!(
        f.call(&[
            "summary",
            "set",
            "session",
            &sid,
            "--file",
            path,
            "--revision",
            "0"
        ])
        .unwrap_err()
        .code,
        "stale_result"
    );
    assert!(view["summary"].as_str().unwrap().contains("人工修正"));
    crate::storage::preferences::save_item(f.db(), Some(id), None, "说明要点", false).unwrap();
    f.call(&["preference", "delete", id]).unwrap();
    assert_eq!(
        f.call(&["preference", "list", "--scope", "user"]).unwrap()["items"],
        json!([])
    );
}

#[test]
fn cli_write_retries_reuse_receipts_and_reject_a_different_operation() {
    let f = Fixture::new();
    let args = [
        "preference",
        "add",
        "--scope",
        "user",
        "--content",
        "简明解释",
        "--request-id",
        "pref-1",
    ];
    let first = f.call(&args).unwrap();
    let second = f.call(&args).unwrap();
    assert_eq!(first["id"], second["id"]);
    assert_eq!(first["request_id"], "pref-1");
    assert_eq!(
        f.call(&["preference", "list", "--scope", "user"]).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let receipt = f.call(&["request", "show", "pref-1"]).unwrap();
    assert_eq!(receipt["status"], "completed");
    assert_eq!(receipt["response"]["ok"], true);
    assert_eq!(receipt["response"]["data"]["id"], first["id"]);
    let error = f
        .call(&[
            "task",
            "create",
            "--title",
            "不能重复用编号",
            "--request-id",
            "pref-1",
        ])
        .unwrap_err();
    assert_eq!(error.code, "request_conflict");
    assert!(f.db().list_workstreams(None).unwrap().is_empty());
    let id = first["id"].as_str().unwrap();
    crate::storage::preferences::save_item(f.db(), Some(id), None, "简明解释", true).unwrap();
    let locked = f
        .call(&["preference", "delete", id, "--request-id", "delete-1"])
        .unwrap_err();
    assert_eq!(locked.code, "locked");
    assert_eq!(
        f.call(&["request", "show", "delete-1"]).unwrap()["response"]["error"]["code"],
        "locked"
    );
    crate::storage::preferences::save_item(f.db(), Some(id), None, "简明解释", false).unwrap();
    assert_eq!(
        f.call(&["preference", "delete", id, "--request-id", "delete-1"])
            .unwrap_err()
            .code,
        "locked"
    );
    f.call(&["preference", "delete", id, "--request-id", "delete-2"])
        .unwrap();
}
#[test]
fn cli_tasks_reuse_ownership_and_lists_paginate_without_losing_sessions() {
    let f = Fixture::new();
    let sid = f.session();
    let other = f.session();
    let task = f.call(&["task", "create", "--title", "CLI 任务"]).unwrap();
    let tid = task["task"]["id"].as_str().unwrap();
    let task2 = f.call(&["task", "create", "--title", "另一任务"]).unwrap();
    let tid2 = task2["task"]["id"].as_str().unwrap();
    f.call(&["task", "attach", tid, "--session", &sid]).unwrap();
    let members = f.call(&["session", "list", "--task", tid]).unwrap();
    assert_eq!(members["items"].as_array().unwrap().len(), 1);
    assert_eq!(members["items"][0]["id"], sid);
    f.call(&["task", "attach", tid2, "--session", &sid])
        .unwrap();
    assert_eq!(
        f.call(&["task", "detach", tid, "--session", &sid])
            .unwrap_err()
            .code,
        "scope_mismatch"
    );
    assert_eq!(
        f.db()
            .get_session(&sid)
            .unwrap()
            .unwrap()
            .owner_workstream_id
            .as_deref(),
        Some(tid2)
    );
    f.call(&["task", "detach", tid2, "--session", &sid])
        .unwrap();
    let first = f.call(&["session", "list", "--limit", "1"]).unwrap();
    let cursor = first["next_cursor"].as_str().unwrap();
    let second = f
        .call(&["session", "list", "--limit", "1", "--cursor", cursor])
        .unwrap();
    let mut ids = vec![
        first["items"][0]["id"].as_str().unwrap().to_string(),
        second["items"][0]["id"].as_str().unwrap().to_string(),
    ];
    ids.sort();
    let mut expected = vec![sid, other];
    expected.sort();
    assert_eq!(ids, expected);
    assert_eq!(second["end"], true);
    let claude = f
        .db()
        .upsert_logical_session(
            Agent::ClaudeCode,
            "cli-claude",
            None,
            Some("Claude 会话"),
            None,
            None,
            None,
            None,
            None,
            "test",
            "/test/claude",
            &json!({}),
        )
        .unwrap()
        .0;
    for spelling in ["claude", "claude_code"] {
        let result = f.call(&["session", "list", "--agent", spelling]).unwrap();
        assert_eq!(result["items"].as_array().unwrap().len(), 1);
        assert_eq!(result["items"][0]["id"], claude);
    }
}
#[test]
fn cli_help_unknown_commands_and_connection_lifecycle_are_explicit() {
    assert!(execute(&["--help".into()]).unwrap()["usage"].is_array());
    assert_eq!(
        execute(&[
            "extraction".into(),
            "submit".into(),
            "--connection".into(),
            "unused".into()
        ])
        .unwrap_err()
        .code,
        "invalid_arguments"
    );
    let f = Fixture::new();
    let connection = f.home.runtime_dir.join("cli.json");
    assert_eq!(f.call(&["status"]).unwrap()["connected"], true);
    assert_eq!(
        transport::request(&connection, json!({"action":"inputs"}))
            .unwrap_err()
            .code,
        "invalid_command"
    );
    f.server.as_ref().unwrap().shutdown();
    assert_eq!(f.call(&["status"]).unwrap_err().code, "app_unavailable");
}

#[test]
fn cli_project_reads_and_task_project_edits_keep_unconsumed_material_pending() {
    let f = Fixture::new();
    let directory = f.root.join("project");
    std::fs::create_dir_all(&directory).unwrap();
    let task = f
        .call(&[
            "task",
            "create",
            "--title",
            "项目任务",
            "--path",
            directory.to_str().unwrap(),
        ])
        .unwrap();
    let tid = task["task"]["id"].as_str().unwrap();
    let paths = f.db().list_workspace_paths().unwrap();
    let path = &paths[0];
    let pid = &path.project_id;
    let sid = f.session();
    f.db()
        .tx(|tx| {
            crate::storage::session_paths::attach_session_workspace_path_conn(
                tx,
                &sid,
                Some(&path.id),
            )
        })
        .unwrap();
    f.call(&["task", "attach", tid, "--session", &sid]).unwrap();
    assert_eq!(
        f.call(&["project", "list"]).unwrap()["items"][0]["id"],
        *pid
    );
    assert_eq!(
        f.call(&["task", "list", "--project", pid]).unwrap()["items"][0]["id"],
        tid
    );
    assert_eq!(
        f.call(&["session", "list", "--project", pid]).unwrap()["items"][0]["id"],
        sid
    );
    assert_eq!(
        f.call(&["project", "show", pid]).unwrap()["paths"][0]["canonical_path"],
        directory.to_str().unwrap()
    );
    f.ingest(&sid, "projectquery unique", "m2", 100);
    let hits = f
        .call(&[
            "search",
            "--query",
            "projectquery",
            "--project",
            pid,
            "--agent",
            "codex",
        ])
        .unwrap();
    assert_eq!(hits["items"].as_array().unwrap().len(), 1);
    assert_eq!(hits["items"][0]["session_id"], sid);
    assert_eq!(hits["items"][0]["sequence"], 2);
    assert_eq!(
        f.call(&[
            "search",
            "--query",
            "projectquery",
            "--project",
            pid,
            "--agent",
            "claude"
        ])
        .unwrap()["items"],
        json!([])
    );
    for (kind, title, id) in [
        ("session", "测试", sid.as_str()),
        ("task", "项目任务", tid),
        ("project", "project", pid.as_str()),
    ] {
        let found = f
            .call(&[
                kind,
                "list",
                "--title",
                title,
                "--path",
                directory.to_str().unwrap(),
            ])
            .unwrap();
        assert_eq!(found["items"].as_array().unwrap().len(), 1);
        assert_eq!(found["items"][0]["id"], id);
        assert_eq!(
            f.call(&[kind, "list", "--title", "不存在的名称"]).unwrap()["items"],
            json!([])
        );
    }
    let file = f.root.join("summary.md");
    std::fs::write(&file, "### 项目进展\n\n- 已建立任务。\n").unwrap();
    for (kind, id) in [("task", tid), ("project", pid.as_str())] {
        f.call(&[
            "summary",
            "set",
            kind,
            id,
            "--file",
            file.to_str().unwrap(),
            "--revision",
            "0",
        ])
        .unwrap();
        let view = f.call(&[kind, "summary", id]).unwrap();
        assert_eq!(view["revision"], 1);
        assert_eq!(view["pending"], true);
        assert_eq!(
            f.call(&[
                "summary",
                "set",
                kind,
                id,
                "--file",
                file.to_str().unwrap(),
                "--revision",
                "0"
            ])
            .unwrap_err()
            .code,
            "stale_result"
        );
    }
    assert!(f.db().workstream_frontiers(tid).unwrap().is_empty());
    assert!(
        crate::storage::project_context_repo::frontiers(&f.db().read(), pid)
            .unwrap()
            .is_empty()
    );
    let pref = f
        .call(&[
            "preference",
            "add",
            "--scope",
            "project",
            "--project",
            pid,
            "--content",
            "项目内保持简洁",
        ])
        .unwrap();
    assert_eq!(pref["project_id"], *pid);
    assert_eq!(
        f.call(&["preference", "list", "--scope", "user"]).unwrap()["items"],
        json!([])
    );
}

#[test]
fn cli_extraction_jobs_report_backend_failure_without_claiming_summary_was_saved() {
    let f = Fixture::new();
    let sid = f.session();
    f.db().set_setting("assistant.agent", "none").unwrap();
    let started = f.call(&["summary", "update", "session", &sid]).unwrap();
    let job = started["job_id"].as_str().unwrap();
    let start = std::time::Instant::now();
    let result = loop {
        let result = f.call(&["job", "show", job]).unwrap();
        if result["status"] != "running" {
            break result;
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert_eq!(result["status"], "failed");
    assert!(!result["error"]["code"].as_str().unwrap().is_empty());
    assert!(result.get("result").is_none());
    assert!(f.db().get_session_context(&sid).unwrap().is_none());
    let waited = f.call(&["job", "wait", job]).unwrap_err();
    assert_eq!(waited.details.as_ref().unwrap()["job_id"], job);
    assert_eq!(waited.details.as_ref().unwrap()["status"], "failed");
    let waited = f
        .call(&[
            "summary",
            "update",
            "session",
            &sid,
            "--wait",
            "--timeout",
            "5",
        ])
        .unwrap_err();
    assert_eq!(waited.details.as_ref().unwrap()["status"], "failed");
    assert_eq!(
        f.call(&["job", "list"]).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn cli_global_search_filters_before_paging_and_excludes_retired_messages() {
    let f = Fixture::new();
    let target = f.session();
    let archived = f.session();
    f.ingest(&target, "needle_first 100%", "m2", 100);
    f.ingest(&target, "needle_second", "m3", 200);
    f.ingest(&target, "prefixneedlesuffix", "m4", 300);
    f.ingest(&archived, "needle_archived", "m2", 100);
    f.db()
        .write()
        .execute(
            "UPDATE sessions SET archived_at=?1 WHERE id=?2",
            rusqlite::params![now(), archived],
        )
        .unwrap();
    let task = f
        .call(&["task", "create", "--title", "SearchTask"])
        .unwrap();
    let tid = task["task"]["id"].as_str().unwrap();
    f.call(&["task", "attach", tid, "--session", &target])
        .unwrap();
    let first = f
        .call(&["search", "--query", "needle", "--task", tid, "--limit", "1"])
        .unwrap();
    assert_eq!(first["items"][0]["session_id"], target);
    let cursor = first["next_cursor"].as_str().unwrap();
    let second = f
        .call(&[
            "search", "--query", "needle", "--task", tid, "--limit", "1", "--cursor", cursor,
        ])
        .unwrap();
    assert_eq!(second["end"], true);
    let mut sequences = vec![
        first["items"][0]["sequence"].as_i64().unwrap(),
        second["items"][0]["sequence"].as_i64().unwrap(),
    ];
    sequences.sort();
    assert_eq!(sequences, vec![2, 3]);
    assert_eq!(
        f.call(&["search", "--query", "needle", "--cursor", cursor])
            .unwrap_err()
            .code,
        "invalid_cursor"
    );
    let archived_hits = f
        .call(&["search", "--query", "needle", "--scope", "archived"])
        .unwrap();
    assert_eq!(archived_hits["items"].as_array().unwrap().len(), 1);
    assert_eq!(archived_hits["items"][0]["session_id"], archived);
    assert_eq!(
        f.call(&["search", "--query", "%", "--task", tid]).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        f.call(&["session", "list", "--title", "%"]).unwrap()["items"],
        json!([])
    );
    f.ingest(&target, "replacement", "new", 0);
    assert_eq!(
        f.call(&["search", "--query", "needle", "--task", tid])
            .unwrap()["items"],
        json!([])
    );
}

#[test]
fn cli_wait_reports_success_and_timeout_without_stopping_the_job() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let f = Fixture::new();
    let connection = transport::Connection::create(f.root.join("wait-bridge")).unwrap();
    let path = f.root.join("wait.json");
    connection.publish(&path).unwrap();
    let completed = AtomicBool::new(false);
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !stop.load(Ordering::Acquire) {
                transport::serve_pending(&connection, |command| {
                    assert_eq!(command["action"], "job");
                    Ok(if completed.load(Ordering::Acquire) {
                        json!({"job_id":"job-1","status":"completed","result":{"summary":"已保存"}})
                    } else {
                        json!({"job_id":"job-1","status":"running"})
                    })
                });
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        });
        let error = wait_job(&path, "job-1", std::time::Duration::from_millis(30)).unwrap_err();
        assert_eq!(error.code, "wait_timeout");
        assert_eq!(error.details.as_ref().unwrap()["job_id"], "job-1");
        completed.store(true, Ordering::Release);
        let value = execute(&[
            "--connection".into(),
            path.to_string_lossy().into_owned(),
            "job".into(),
            "wait".into(),
            "job-1".into(),
        ])
        .unwrap();
        assert_eq!(value["result"]["summary"], "已保存");
        stop.store(true, Ordering::Release);
    });
}

#[test]
fn cli_task_partial_edits_and_archive_preserve_data_and_refresh_search() {
    let f = Fixture::new();
    let task = f
        .call(&[
            "task",
            "create",
            "--title",
            "OldTaskMarker",
            "--description",
            "KeepDescriptionMarker",
        ])
        .unwrap();
    let tid = task["task"]["id"].as_str().unwrap();
    let sid = f.session();
    f.call(&["task", "attach", tid, "--session", &sid]).unwrap();
    f.call(&["task", "archive", tid]).unwrap();
    let before = f.changes.load(std::sync::atomic::Ordering::SeqCst);
    let args = [
        "task",
        "update",
        tid,
        "--title",
        "NewTaskMarker",
        "--request-id",
        "task-patch",
    ];
    let updated = f.call(&args).unwrap();
    f.call(&args).unwrap();
    assert_eq!(
        f.changes.load(std::sync::atomic::Ordering::SeqCst),
        before + 1
    );
    assert_eq!(updated["task"]["title"], "NewTaskMarker");
    assert_eq!(updated["task"]["description"], "KeepDescriptionMarker");
    assert_eq!(updated["task"]["visibility"], "archived");
    assert_eq!(updated["task"]["created_at"], task["task"]["created_at"]);
    assert!(f.call(&["task", "list"]).unwrap()["items"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        f.call(&["task", "list", "--scope", "archived"]).unwrap()["items"][0]["id"],
        tid
    );
    let hits = f
        .call(&["search", "--query", "NewTaskMarker", "--task", tid])
        .unwrap();
    assert_eq!(hits["items"][0]["session_id"], sid);
    assert!(f
        .call(&["search", "--query", "OldTaskMarker", "--task", tid])
        .unwrap()["items"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(f.call(&["task", "update", tid, "--title", " "]).is_err());
    assert_eq!(
        f.call(&["task", "update", tid]).unwrap_err().code,
        "invalid_arguments"
    );
    f.call(&["task", "update", tid, "--description", ""])
        .unwrap();
    let task = f.db().get_workstream(tid).unwrap().unwrap();
    assert_eq!(task.title, "NewTaskMarker");
    assert_eq!(task.description, "");
    assert_eq!(task.visibility, "archived");
    f.call(&["task", "unarchive", tid]).unwrap();
    assert_eq!(f.call(&["task", "list"]).unwrap()["items"][0]["id"], tid);

    let original_messages = f.call(&["session", "messages", &sid]).unwrap();
    let original_summary = f.call(&["session", "summary", &sid]).unwrap();
    let archive = [
        "session",
        "archive",
        &sid,
        "--request-id",
        "session-archive",
    ];
    let archived = f.call(&archive).unwrap();
    let retry = f.call(&archive).unwrap();
    assert_eq!(retry["archived_at"], archived["archived_at"]);
    assert!(archived["archived_at"].is_string());
    assert!(
        f.call(&["session", "list", "--task", tid]).unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.call(&["session", "list", "--task", tid, "--scope", "archived"])
            .unwrap()["items"][0]["id"],
        sid
    );
    assert_eq!(
        f.call(&["session", "messages", &sid]).unwrap(),
        original_messages
    );
    assert_eq!(
        f.call(&["session", "summary", &sid]).unwrap(),
        original_summary
    );
    assert_eq!(f.call(&["session", "show", &sid]).unwrap()["task_id"], tid);
    f.call(&["session", "unarchive", &sid]).unwrap();
    assert!(f.call(&["session", "show", &sid]).unwrap()["archived_at"].is_null());
}

#[test]
fn cli_directory_registration_and_task_paths_preserve_projects_sessions_and_disk() {
    let f = Fixture::new();
    let first_dir = f.root.join("first directory");
    let second_dir = f.root.join("second directory");
    std::fs::create_dir(&first_dir).unwrap();
    std::fs::create_dir(&second_dir).unwrap();
    let first = f
        .call(&["path", "register", "--path", first_dir.to_str().unwrap()])
        .unwrap();
    let p1 = first["path"]["id"].as_str().unwrap();
    let pid = first["project"]["id"].as_str().unwrap();
    assert!(first["project"]["git_id"].is_null());
    assert_eq!(
        f.call(&["path", "register", "--path", first_dir.to_str().unwrap()])
            .unwrap()["path"]["id"],
        p1
    );
    assert_eq!(f.db().list_projects().unwrap().len(), 1);
    for path in [
        "relative",
        f.root.join("missing").to_str().unwrap(),
        f.home.db_path.to_str().unwrap(),
    ] {
        assert_eq!(
            f.call(&["path", "register", "--path", path])
                .unwrap_err()
                .code,
            "invalid_arguments"
        );
    }
    assert!(!f.root.join("missing").exists());
    assert!(f
        .call(&[
            "path",
            "register",
            "--path",
            f.home.data_dir.to_str().unwrap()
        ])
        .is_err());
    let task = f.call(&["task", "create", "--title", "PathsTask"]).unwrap();
    let tid = task["task"]["id"].as_str().unwrap();
    let sid = f.session();
    f.db()
        .tx(|tx| {
            crate::storage::session_paths::attach_session_workspace_path_conn(tx, &sid, Some(p1))
        })
        .unwrap();
    f.call(&["task", "attach", tid, "--session", &sid]).unwrap();
    f.call(&[
        "task",
        "path",
        "add",
        tid,
        "--path",
        first_dir.to_str().unwrap(),
    ])
    .unwrap();
    let second = f
        .call(&[
            "task",
            "path",
            "add",
            tid,
            "--path",
            second_dir.to_str().unwrap(),
        ])
        .unwrap();
    let p2 = second["path"]["workspace_path_id"].as_str().unwrap();
    f.call(&[
        "task",
        "path",
        "add",
        tid,
        "--path",
        first_dir.to_str().unwrap(),
    ])
    .unwrap();
    let paths = f.call(&["task", "path", "list", tid]).unwrap();
    assert_eq!(paths["items"].as_array().unwrap().len(), 2);
    assert_eq!(paths["items"][0]["workspace_path_id"], p1);
    assert_eq!(paths["items"][1]["workspace_path_id"], p2);
    assert_eq!(paths["items"][1]["position"], 1);
    f.call(&[
        "task",
        "path",
        "reorder",
        tid,
        "--path-id",
        p2,
        "--path-id",
        p1,
    ])
    .unwrap();
    for ids in [vec![p1], vec![p1, p1], vec![p1, "foreign"]] {
        let mut args = vec!["task", "path", "reorder", tid];
        for id in ids {
            args.extend(["--path-id", id]);
        }
        assert!(f.call(&args).is_err());
    }
    assert_eq!(
        f.call(&["task", "path", "list", tid]).unwrap()["items"][0]["workspace_path_id"],
        p2
    );
    assert_eq!(
        f.call(&["task", "path", "remove", tid, "--path-id", "foreign"])
            .unwrap_err()
            .code,
        "scope_mismatch"
    );
    f.call(&["task", "path", "remove", tid, "--path-id", p1])
        .unwrap();
    let remaining = f.call(&["task", "path", "list", tid]).unwrap();
    assert_eq!(remaining["items"].as_array().unwrap().len(), 1);
    assert_eq!(remaining["items"][0]["workspace_path_id"], p2);
    assert_eq!(remaining["items"][0]["position"], 0);
    assert!(first_dir.is_dir());
    assert!(f.db().get_workspace_path(p1).unwrap().is_some());
    let session = f.db().get_session(&sid).unwrap().unwrap();
    assert_eq!(session.workspace_path_id.as_deref(), Some(p1));
    assert_eq!(session.owner_workstream_id.as_deref(), Some(tid));
    assert_eq!(
        f.call(&["session", "messages", &sid]).unwrap()["through_sequence"],
        1
    );

    let renamed = f
        .call(&["project", "rename", pid, "--name", "CustomProjectMarker"])
        .unwrap();
    assert_eq!(renamed["project"]["name"], "CustomProjectMarker");
    assert_eq!(renamed["project"]["name_customized"], true);
    assert_eq!(renamed["project"]["git_id"], first["project"]["git_id"]);
    let registered = f
        .call(&["path", "register", "--path", first_dir.to_str().unwrap()])
        .unwrap();
    assert_eq!(registered["project"]["id"], pid);
    assert_eq!(registered["project"]["name"], "CustomProjectMarker");
    assert_eq!(
        f.call(&["search", "--query", "CustomProjectMarker", "--project", pid])
            .unwrap()["items"][0]["session_id"],
        sid
    );
    f.call(&["project", "archive", pid]).unwrap();
    assert!(f.call(&["project", "list"]).unwrap()["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p["id"] != pid));
    assert!(
        f.call(&["project", "list", "--scope", "archived"]).unwrap()["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == pid)
    );
    assert!(
        f.call(&["project", "list", "--scope", "all"]).unwrap()["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == pid)
    );
    assert_eq!(
        f.db().get_workstream(tid).unwrap().unwrap().visibility,
        "normal"
    );
    assert!(f
        .db()
        .get_session(&sid)
        .unwrap()
        .unwrap()
        .archived_at
        .is_none());
    f.call(&["project", "unarchive", pid]).unwrap();
    assert!(f.call(&["project", "list"]).unwrap()["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["id"] == pid));
    let conn = f.db().read();
    let title: String = conn
        .query_row(
            "SELECT title FROM search_index WHERE kind='project' AND ref_id=?1",
            [pid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(title, "CustomProjectMarker");
}
