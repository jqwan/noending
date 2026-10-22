use super::*;
use crate::domain::{Agent, ParsedSessionMessage, SessionMessageRole, SourceCursorUpdate};
use crate::storage::{now, Db};
use crate::workspace::home::NoEndingHome;
use std::sync::Arc;

struct Fixture {
    root: PathBuf,
    home: NoEndingHome,
    db: Option<Arc<Db>>,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("noending-cli-{}", crate::storage::new_id()));
        let home = NoEndingHome::new(root.join("home").to_str().unwrap(), None).unwrap();
        home.ensure_dirs().unwrap();
        let db = Arc::new(Db::open(&home.db_path).unwrap());
        Self {
            root,
            home,
            db: Some(db),
        }
    }
    fn db(&self) -> &Db {
        self.db.as_ref().unwrap()
    }
    fn call(&self, args: &[&str]) -> CliResult<Value> {
        let mut argv = vec![
            "--home".into(),
            self.home.root.to_string_lossy().into_owned(),
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
        let paths = self.db().list_workspace_paths().unwrap();
        let path = if let Some(path) = paths.first() {
            path.id.clone()
        } else {
            let directory = self.root.join("fixture-project");
            std::fs::create_dir_all(&directory).unwrap();
            self.call(&["path", "register", "--path", directory.to_str().unwrap()])
                .unwrap()["path"]["id"]
                .as_str()
                .unwrap()
                .to_string()
        };
        crate::storage::session_paths::attach_session_workspace_path_conn(
            &self.db().write(),
            &id,
            Some(&path),
        )
        .unwrap();
        self.ingest(&id, &format!("{}乙", "甲".repeat(2000)), "m1", 0);
        id
    }
    fn ingest(&self, id: &str, content: &str, message: &str, offset: u64) {
        self.db()
            .commit_ingest(
                id,
                &[ParsedSessionMessage {
                    source_message_id: Some(message.into()),
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
fn cli_summary_edits_preserve_pending_messages() {
    let f = Fixture::new();
    let sid = f.session();
    let file = f.root.join("summary.json");
    std::fs::write(
        &file,
        r#"{"items":[{"title":"主题","description":"","title_locked":false,"details":["人工修正。"]}]}"#,
    )
    .unwrap();
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
    assert_eq!(view["summary"]["items"][0]["details"][0], "人工修正。");
}

#[test]
fn cli_write_retries_reuse_receipts_and_reject_a_different_operation() {
    let f = Fixture::new();
    let args = [
        "task",
        "create",
        "--title",
        "Receipt task",
        "--request-id",
        "task-1",
    ];
    let first = f.call(&args).unwrap();
    let second = f.call(&args).unwrap();
    assert_eq!(first["task"]["id"], second["task"]["id"]);
    assert_eq!(first["request_id"], "task-1");
    assert_eq!(
        f.call(&["task", "list"]).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let receipt = f.call(&["request", "show", "task-1"]).unwrap();
    assert_eq!(receipt["status"], "completed");
    assert_eq!(receipt["response"]["ok"], true);
    assert_eq!(
        receipt["response"]["data"]["task"]["id"],
        first["task"]["id"]
    );
    assert_eq!(
        f.call(&[
            "task",
            "create",
            "--title",
            "Other task",
            "--request-id",
            "task-1"
        ])
        .unwrap_err()
        .code,
        "request_conflict"
    );
    assert_eq!(
        f.call(&["task", "list"]).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let sid = f.session();
    let file = f.root.join("summary.json");
    std::fs::write(
        &file,
        r#"{"items":[{"title":"Summary","description":"","details":[],"title_locked":false}]}"#,
    )
    .unwrap();
    let args = [
        "summary",
        "set",
        "session",
        &sid,
        "--file",
        file.to_str().unwrap(),
        "--revision",
        "1",
        "--request-id",
        "future-revision",
    ];
    assert_eq!(f.call(&args).unwrap_err().code, "stale_result");
    assert_eq!(
        f.call(&["request", "show", "future-revision"]).unwrap()["response"]["error"]["code"],
        "stale_result"
    );
    f.call(&[
        "summary",
        "set",
        "session",
        &sid,
        "--file",
        file.to_str().unwrap(),
        "--revision",
        "0",
    ])
    .unwrap();
    assert_eq!(f.call(&args).unwrap_err().code, "stale_result");
    assert_eq!(
        f.call(&[
            "summary",
            "set",
            "session",
            &sid,
            "--file",
            file.to_str().unwrap(),
            "--revision",
            "1",
            "--request-id",
            "fresh-edit"
        ])
        .unwrap()["revision"],
        2
    );
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
    link_project(&f, tid, &sid);
    f.call(&["task", "attach", tid, "--session", &sid]).unwrap();
    let members = f.call(&["session", "list", "--task", tid]).unwrap();
    assert_eq!(members["items"].as_array().unwrap().len(), 1);
    assert_eq!(members["items"][0]["id"], sid);
    link_project(&f, tid2, &sid);
    f.call(&["task", "attach", tid2, "--session", &sid])
        .unwrap();
    f.call(&["task", "detach", tid, "--session", &sid]).unwrap();
    assert_eq!(
        f.db()
            .get_session(&sid)
            .unwrap()
            .unwrap()
            .tracked_workstream_ids
            .first()
            .map(String::as_str),
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
fn cli_help_and_standalone_status_do_not_need_a_desktop_connection() {
    assert!(execute(&["--help".into()]).unwrap()["usage"].is_array());
    let f = Fixture::new();
    assert_eq!(
        f.call(&["status"]).unwrap()["home"],
        f.home.root.to_str().unwrap()
    );
    for args in [
        vec!["job", "list"],
        vec!["summary", "update", "session", "id", "--wait"],
        vec!["status", "--connection", "unused"],
        vec!["extraction", "inputs"],
        vec!["extraction", "submit", "--connection", "unused"],
    ] {
        assert_eq!(f.call(&args).unwrap_err().code, "invalid_arguments");
    }
}

#[test]
fn cli_project_reads_and_task_project_edits_keep_unconsumed_material_pending() {
    let f = Fixture::new();
    let directory = f.root.join("project");
    std::fs::create_dir_all(&directory).unwrap();
    let registered = f
        .call(&["path", "register", "--path", directory.to_str().unwrap()])
        .unwrap();
    let task = f
        .call(&[
            "task",
            "create",
            "--title",
            "项目任务",
            "--project",
            registered["project"]["id"].as_str().unwrap(),
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
    link_project(&f, tid, &sid);
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
    let file = f.root.join("summary.json");
    std::fs::write(
        &file,
        r#"{"items":[{"title":"项目进展","description":"","title_locked":false,"details":["已建立任务。"]}]}"#,
    )
    .unwrap();
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
}

#[test]
fn cli_extraction_returns_backend_failure_without_claiming_summary_was_saved() {
    let f = Fixture::new();
    let sid = f.session();
    f.db()
        .set_setting(
            "context.extraction",
            r#"{"agent":"none","runtime":{"model":null,"provider":null,"effort":null}}"#,
        )
        .unwrap();
    let error = f.call(&["summary", "update", "session", &sid]).unwrap_err();
    assert_eq!(error.code, "agent_not_configured");
    let receipt = f
        .call(&[
            "request",
            "show",
            error.details.unwrap()["request_id"].as_str().unwrap(),
        ])
        .unwrap();
    assert_eq!(receipt["status"], "completed");
    assert_eq!(receipt["response"]["error"]["code"], "agent_not_configured");
    assert!(f.db().get_session_context(&sid).unwrap().is_none());
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
    link_project(&f, tid, &target);
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
    link_project(&f, tid, &sid);
    f.call(&["task", "attach", tid, "--session", &sid]).unwrap();
    f.call(&["task", "archive", tid]).unwrap();
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
    assert_eq!(
        f.call(&["session", "show", &sid]).unwrap()["task_ids"],
        json!([tid])
    );
    f.call(&["session", "unarchive", &sid]).unwrap();
    assert!(f.call(&["session", "show", &sid]).unwrap()["archived_at"].is_null());
}

#[test]
fn cli_task_project_edits_and_directory_registration_preserve_session_tracking_and_disk() {
    let f = Fixture::new();
    let sid = f.session();
    let pid = f
        .db()
        .get_session(&sid)
        .unwrap()
        .unwrap()
        .project_id
        .unwrap();
    let task = f
        .call(&["task", "create", "--title", "Scope", "--project", &pid])
        .unwrap();
    let tid = task["task"]["id"].as_str().unwrap();
    f.call(&["task", "attach", tid, "--session", &sid]).unwrap();
    assert_eq!(
        f.call(&["task", "project", "list", tid]).unwrap()["items"][0]["id"],
        pid
    );
    f.call(&["task", "project", "set", tid]).unwrap();
    assert!(f.call(&["task", "project", "list", tid]).unwrap()["items"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(f.db().sessions_for_workstream(tid).unwrap().len(), 1);
    f.call(&["task", "detach", tid, "--session", &sid]).unwrap();
    assert!(f.call(&["task", "attach", tid, "--session", &sid]).is_err());
    assert!(f.root.join("fixture-project").is_dir());
}

fn link_project(f: &Fixture, task: &str, session: &str) {
    let project = f
        .db()
        .get_session(session)
        .unwrap()
        .unwrap()
        .project_id
        .unwrap();
    crate::workspace::workstream::set_workstream_projects(f.db(), task, &[project]).unwrap();
}

#[test]
fn summary_items_are_searchable_by_content_scope_and_kind_and_readable_at_a_revision() {
    let f = Fixture::new();
    let sid = f.session();
    let pid = f.db().list_workspace_paths().unwrap()[0].project_id.clone();
    let task = f
        .call(&["task", "create", "--title", "同步任务", "--project", &pid])
        .unwrap();
    let tid = task["task"]["id"].as_str().unwrap();
    let session = json!({"items":[
        {"title":"路径同步","description":"保留目录","title_locked":false,"details":["literal_% 的文件保持不变"]},
        {"title":"窗口修复","description":"路径同步后的行为","title_locked":false,"details":[]}
    ]});
    let topic = json!({"items":[{"title":"项目与任务","description":"路径同步","title_locked":false,"details":["CaseSensitiveWord"]}]});
    for (kind, id, body) in [
        ("session", sid.as_str(), &session),
        ("task", tid, &topic),
        ("project", pid.as_str(), &topic),
    ] {
        crate::context::edit_summary(f.db(), kind, id, 0, &body.to_string()).unwrap();
    }
    let first = f
        .call(&["summary", "search", "--query", "路径同步", "--limit", "1"])
        .unwrap();
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    let mut hits = first["items"].as_array().unwrap().clone();
    let mut page = first;
    while let Some(cursor) = page["next_cursor"].as_str() {
        page = f
            .call(&[
                "summary",
                "search",
                "--query",
                "路径同步",
                "--limit",
                "1",
                "--cursor",
                cursor,
            ])
            .unwrap();
        hits.extend(page["items"].as_array().unwrap().clone());
    }
    assert_eq!(hits.len(), 4);
    assert_eq!(
        hits.iter().filter(|hit| hit["kind"] == "session").count(),
        2
    );
    let detail = f
        .call(&["session", "summary", &sid, "--item", "2", "--revision", "1"])
        .unwrap();
    assert_eq!(detail["item"]["title"], "窗口修复");
    assert!(detail.get("summary").is_none());
    assert_eq!(
        f.call(&["summary", "search", "--query", "literal_%"])
            .unwrap()["items"][0]["item_number"],
        1
    );
    assert_eq!(
        f.call(&[
            "summary",
            "search",
            "--query",
            "casesensitiveword",
            "--kind",
            "task",
            "--project",
            &pid
        ])
        .unwrap()["items"][0]["id"],
        tid
    );
    assert_eq!(
        f.call(&["summary", "search", "--query", "details"])
            .unwrap()["items"],
        json!([])
    );
    f.call(&["task", "archive", tid]).unwrap();
    assert_eq!(
        f.call(&["summary", "search", "--query", "路径同步", "--kind", "task"])
            .unwrap()["items"],
        json!([])
    );
    assert_eq!(
        f.call(&[
            "summary",
            "search",
            "--query",
            "路径同步",
            "--kind",
            "task",
            "--scope",
            "archived"
        ])
        .unwrap()["items"][0]["id"],
        tid
    );
    let cursor = f
        .call(&["summary", "search", "--query", "路径同步", "--limit", "1"])
        .unwrap()["next_cursor"]
        .as_str()
        .unwrap()
        .to_string();
    crate::context::edit_summary(
        f.db(),
        "session",
        &sid,
        1,
        &json!({"items":[{"title":"合并后的路径事项","description":"已精简","title_locked":false,"details":[]}]})
            .to_string(),
    )
    .unwrap();
    assert_eq!(
        f.call(&[
            "summary",
            "search",
            "--query",
            "路径同步",
            "--cursor",
            &cursor
        ])
        .unwrap_err()
        .code,
        "stale_cursor"
    );
    assert_eq!(
        f.call(&["session", "summary", &sid, "--item", "2", "--revision", "1"])
            .unwrap_err()
            .code,
        "stale_result"
    );
    assert_eq!(
        f.call(&["session", "summary", &sid, "--item", "1", "--revision", "2"])
            .unwrap()["item"]["title"],
        "合并后的路径事项"
    );
}

#[test]
fn summary_edits_reject_invalid_document_input_and_items_without_changing_revision_or_progress() {
    let f = Fixture::new();
    let sid = f.session();
    let valid = json!({"items":[{"title":"保留事项","description":"原始描述","title_locked":false,"details":["原始细则"]}]});
    crate::context::edit_summary(f.db(), "session", &sid, 0, &valid.to_string()).unwrap();
    let before = f.call(&["session", "summary", &sid]).unwrap();
    for invalid in [
        "not a JSON document",
        r#"{"items":[{"title":"缺少必填锁定字段","description":"","details":[]}]}"#,
        r#"{"items":[]}"#,
        r#"{"items":[{"title":" ","description":"","title_locked":false,"details":[]}]}"#,
        r#"{"items":[{"title":"事项","description":"","title_locked":false,"details":[""]}]}"#,
        r#"{"items":[{"id":"fixed","title":"事项","description":"","title_locked":false,"details":[]}]}"#,
    ] {
        assert!(crate::context::edit_summary(f.db(), "session", &sid, 1, invalid).is_err());
    }
    assert_eq!(f.call(&["session", "summary", &sid]).unwrap(), before);
    for invalid in ["invalid JSON", "{}", "[]", r#"{"items":{}}"#] {
        assert!(f
            .db()
            .read()
            .execute(
                "UPDATE session_contexts SET summary=?2 WHERE session_id=?1",
                rusqlite::params![sid, invalid]
            )
            .is_err());
    }
    assert_eq!(f.call(&["session", "summary", &sid]).unwrap(), before);
}

#[test]
fn extraction_configuration_edits_preserve_fields_reset_on_agent_change_and_reject_invalid_values()
{
    let f = Fixture::new();
    let show = || f.call(&["summary", "config", "show"]).unwrap();
    assert_eq!(
        show(),
        json!({"agent":"codex","runtime":{"model":null,"provider":null,"effort":null}})
    );
    let selected = f
        .call(&[
            "summary",
            "config",
            "set",
            "--agent",
            "pi",
            "--model",
            "local-model",
            "--provider",
            "lmstudio",
            "--effort",
            "low",
        ])
        .unwrap();
    assert_eq!(
        selected["runtime"],
        json!({"model":"local-model","provider":"lmstudio","effort":"low"})
    );
    f.call(&["summary", "config", "set", "--model", "new-model"])
        .unwrap();
    assert_eq!(
        show()["runtime"],
        json!({"model":"new-model","provider":"lmstudio","effort":"low"})
    );
    f.call(&["summary", "config", "set", "--model", ""])
        .unwrap();
    assert_eq!(show()["runtime"]["model"], Value::Null);
    f.call(&[
        "summary",
        "config",
        "set",
        "--agent",
        "claude",
        "--model",
        "selected-model",
        "--effort",
        "high",
    ])
    .unwrap();
    assert_eq!(show()["agent"], "claude_code");
    assert_eq!(
        show()["runtime"],
        json!({"model":"selected-model","provider":null,"effort":"high"})
    );
    f.call(&[
        "summary",
        "config",
        "set",
        "--agent",
        "claude_code",
        "--model",
        "same-agent-model",
    ])
    .unwrap();
    assert_eq!(show()["runtime"]["effort"], "high");
    for args in [
        vec!["summary", "config", "set"],
        vec![
            "summary",
            "config",
            "set",
            "--provider",
            "unsupported-provider",
        ],
        vec!["summary", "config", "set", "--agent", "antigravity"],
        vec!["summary", "config", "set", "--agent", "unknown-agent"],
        vec![
            "summary", "config", "set", "--agent", "none", "--model", "unusable",
        ],
    ] {
        assert_eq!(f.call(&args).unwrap_err().code, "invalid_arguments");
    }
    assert_eq!(
        show()["runtime"],
        json!({"model":"same-agent-model","provider":null,"effort":"high"})
    );
    f.call(&["summary", "config", "set", "--agent", "none"])
        .unwrap();
    assert_eq!(
        show(),
        json!({"agent":"none","runtime":{"model":null,"provider":null,"effort":null}})
    );
}

#[test]
fn cli_summary_edits_preserve_title_locks_but_can_update_descriptions_and_details() {
    let f = Fixture::new();
    let id = f.session();
    let body = crate::domain::Summary::single("用户锁定的标题");
    crate::context::edit_summary(
        f.db(),
        "session",
        &id,
        0,
        &serde_json::to_string(&body).unwrap(),
    )
    .unwrap();
    let locked =
        crate::context::set_summary_title_lock(f.db(), "session", &id, 1, 1, true).unwrap();
    let path = f.root.join("locked-summary.json");
    let mut changed = locked.summary;
    changed.items[0].description = "CLI 更新描述".into();
    changed.items[0].details = vec!["CLI 更新细则".into()];
    std::fs::write(&path, serde_json::to_string(&changed).unwrap()).unwrap();
    f.call(&[
        "summary",
        "set",
        "session",
        &id,
        "--revision",
        "2",
        "--file",
        path.to_str().unwrap(),
    ])
    .unwrap();
    let stored = f.call(&["session", "summary", &id]).unwrap();
    assert_eq!(stored["summary"]["items"][0]["title"], "用户锁定的标题");
    assert_eq!(stored["summary"]["items"][0]["description"], "CLI 更新描述");
    assert_eq!(
        stored["summary"]["items"][0]["details"],
        json!(["CLI 更新细则"])
    );
    assert_eq!(stored["summary"]["items"][0]["title_locked"], true);
    changed.items[0].title_locked = false;
    std::fs::write(&path, serde_json::to_string(&changed).unwrap()).unwrap();
    assert!(f
        .call(&[
            "summary",
            "set",
            "session",
            &id,
            "--revision",
            "3",
            "--file",
            path.to_str().unwrap()
        ])
        .is_err());
    changed.items[0].title_locked = true;
    changed.items[0].title = "Agent 擅自改名".into();
    std::fs::write(&path, serde_json::to_string(&changed).unwrap()).unwrap();
    assert!(f
        .call(&[
            "summary",
            "set",
            "session",
            &id,
            "--revision",
            "3",
            "--file",
            path.to_str().unwrap()
        ])
        .is_err());
    assert_eq!(f.call(&["session", "summary", &id]).unwrap(), stored);
}
