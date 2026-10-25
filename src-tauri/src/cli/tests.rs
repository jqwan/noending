use super::*;
use crate::domain::{Agent, ParsedSessionMessage, SessionMessageRole, SourceCursorUpdate};
use crate::storage::{now, Db};
use crate::workspace::home::NoEndingHome;
use std::sync::Arc;

#[test]
fn launch_failures_distinguish_io_rejection_from_an_initial_message_that_may_have_started() {
    let unsent = CliError::from(crate::error::AppError::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "agent executable missing",
    )));
    assert_eq!(unsent.code, "io_error");
    let uncertain = CliError::from(crate::error::AppError::AgentLaunchUncertain(
        "setup failed after process creation".into(),
    ));
    assert_eq!(uncertain.code, "delivery_uncertain");
}

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
fn cli_resolves_exact_native_identity_across_agents_and_archive_states() {
    let f = Fixture::new();
    let sid = f.session();
    let session = f.db().get_session(&sid).unwrap().unwrap();
    let native = &session.root_agent_session_id;
    let other = f
        .db()
        .upsert_logical_session(
            Agent::ClaudeCode,
            native,
            None,
            Some("同原生 ID 的其他 Agent"),
            None,
            None,
            None,
            None,
            None,
            "test",
            "/test/other",
            &json!({}),
        )
        .unwrap()
        .0;
    let task = f
        .call(&["task", "create", "--title", "身份关联任务"])
        .unwrap();
    let tid = task["task"]["id"].as_str().unwrap();
    link_project(&f, tid, &sid);
    f.call(&["task", "attach", tid, "--session", &sid]).unwrap();
    let resolve = || {
        f.call(&[
            "session",
            "resolve",
            "--agent",
            "codex",
            "--native-id",
            native,
        ])
        .unwrap()
    };
    let found = resolve();
    assert_eq!(found["id"], sid);
    assert_eq!(found["agent"], "codex");
    assert_eq!(found["native_id"], *native);
    assert_eq!(found["project_id"], session.project_id.unwrap());
    assert_eq!(found["task_ids"], json!([tid]));
    assert!(found["archived_at"].is_null());
    f.call(&["session", "archive", &sid]).unwrap();
    let archived = resolve();
    assert_eq!(archived["id"], sid);
    assert!(archived["archived_at"].is_string());
    for spelling in ["claude", "claude_code"] {
        assert_eq!(
            f.call(&[
                "session",
                "resolve",
                "--agent",
                spelling,
                "--native-id",
                native
            ])
            .unwrap()["id"],
            other
        );
    }
    for (agent, id) in [
        ("pi", native.as_str()),
        ("codex", sid.as_str()),
        ("codex", "unseen-native"),
    ] {
        assert_eq!(
            f.call(&["session", "resolve", "--agent", agent, "--native-id", id])
                .unwrap_err()
                .code,
            "not_found"
        );
    }
    for args in [
        vec!["session", "resolve", "--native-id", native],
        vec!["session", "resolve", "--agent", "codex"],
        vec![
            "session",
            "resolve",
            "--agent",
            "unknown",
            "--native-id",
            native,
        ],
        vec!["session", "resolve", "--agent", "codex", "--native-id", " "],
    ] {
        assert_eq!(f.call(&args).unwrap_err().code, "invalid_arguments");
    }
    let separate = Fixture::new();
    assert_eq!(
        separate
            .call(&[
                "session",
                "resolve",
                "--agent",
                "codex",
                "--native-id",
                native
            ])
            .unwrap_err()
            .code,
        "not_found"
    );
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
    let task = first["task"]["id"].as_str().unwrap();
    let item = crate::workspace::task_items::create(
        f.db(),
        task,
        crate::workspace::task_items::CreateItem {
            title: "Receipt progress".into(),
            description: None,
        },
    )
    .unwrap();
    let args = [
        "task",
        "item",
        "complete",
        task,
        "--item-id",
        &item.id,
        "--revision",
        "99",
        "--request-id",
        "future-revision",
    ];
    assert_eq!(f.call(&args).unwrap_err().code, "stale_result");
    assert_eq!(
        f.call(&["request", "show", "future-revision"]).unwrap()["response"]["error"]["code"],
        "stale_result"
    );
    f.call(&[
        "task",
        "item",
        "complete",
        task,
        "--item-id",
        &item.id,
        "--revision",
        "1",
    ])
    .unwrap();
    assert_eq!(f.call(&args).unwrap_err().code, "stale_result");
    assert_eq!(
        f.call(&[
            "task",
            "item",
            "reopen",
            task,
            "--item-id",
            &item.id,
            "--revision",
            "2",
            "--request-id",
            "fresh-edit"
        ])
        .unwrap()["revision"],
        3
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
    for (kind, id) in [("task", tid), ("project", pid.as_str())] {
        seed_summary(
            f.db(),
            kind,
            id,
            1,
            crate::domain::Summary::single("项目结论"),
        )
        .unwrap();
        let view = f.call(&[kind, "summary", id]).unwrap();
        assert_eq!(view["revision"], 1);
        assert_eq!(view["pending"], true);
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
        {"title":"路径同步","description":"保留目录","details":["literal_% 的文件保持不变"]},
        {"title":"窗口修复","description":"路径同步后的行为","details":[]}
    ]});
    let topic = json!({"items":[{"title":"项目与任务","description":"路径同步","details":["CaseSensitiveWord"]}]});
    for (kind, id, body) in [
        ("session", sid.as_str(), &session),
        ("task", tid, &topic),
        ("project", pid.as_str(), &topic),
    ] {
        seed_summary(
            f.db(),
            kind,
            id,
            1,
            serde_json::from_value(body.clone()).unwrap(),
        )
        .unwrap();
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
    seed_summary(
        f.db(),
        "session",
        &sid,
        2,
        serde_json::from_value(
            json!({"items":[{"title":"合并后的路径事项","description":"已精简","details":[]}]}),
        )
        .unwrap(),
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
fn cli_summary_item_ids_locate_renamed_items_after_reordering_and_guard_stale_versions() {
    let f = Fixture::new();
    let sid = f.session();
    let created=seed_summary(f.db(), "session", &sid, 1, serde_json::from_value(json!({"items":[{"title":"同步策略","description":"保持内容","details":[]},{"title":"其他结论","description":"独立事项","details":[]}]})).unwrap()).unwrap();
    let id = created.items[0].id.clone();
    let mut reordered = created;
    reordered.items.reverse();
    reordered.items[1].title = "同步策略已修正".into();
    seed_summary(f.db(), "session", &sid, 2, reordered).unwrap();
    let read = f
        .call(&[
            "session",
            "summary",
            &sid,
            "--item-id",
            &id,
            "--revision",
            "2",
        ])
        .unwrap();
    assert_eq!(read["item_id"], id);
    assert_eq!(read["item_number"], 2);
    assert_eq!(read["item"]["title"], "同步策略已修正");
    assert_eq!(
        f.call(&[
            "session",
            "summary",
            &sid,
            "--item-id",
            &id,
            "--revision",
            "1"
        ])
        .unwrap_err()
        .code,
        "stale_result"
    );
    assert!(f
        .call(&[
            "session",
            "summary",
            &sid,
            "--item-id",
            &id,
            "--item",
            "1",
            "--revision",
            "2"
        ])
        .is_err());
    let search = f
        .call(&["summary", "search", "--query", "同步策略已修正"])
        .unwrap();
    assert_eq!(search["items"][0]["item_id"], id);
}

#[test]
fn cli_work_items_use_independent_revisions_and_receipts_without_rewriting_summaries() {
    let f = Fixture::new();
    let task = f.call(&["task", "create", "--title", "工作事项"]).unwrap()["task"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let file = f.root.join("work.json");
    std::fs::write(&file, r#"{"title":"实现","description":"通过回归"}"#).unwrap();
    let args = [
        "task",
        "item",
        "create",
        &task,
        "--file",
        file.to_str().unwrap(),
        "--request-id",
        "create-work",
    ];
    let created = f.call(&args).unwrap();
    assert_eq!(f.call(&args).unwrap(), created);
    let id = created["id"].as_str().unwrap();
    assert_eq!(created["completed"], false);
    std::fs::write(&file, r#"{"details":["验证通过"]}"#).unwrap();
    let updated = f
        .call(&[
            "task",
            "item",
            "update",
            &task,
            "--item-id",
            id,
            "--revision",
            "1",
            "--file",
            file.to_str().unwrap(),
        ])
        .unwrap();
    assert_eq!(updated["revision"], 2);
    let completed = f
        .call(&[
            "task",
            "item",
            "complete",
            &task,
            "--item-id",
            id,
            "--revision",
            "2",
            "--request-id",
            "complete-work",
        ])
        .unwrap();
    assert_eq!(completed["completed"], true);
    assert_eq!(completed["details"], json!(["验证通过"]));
    assert_eq!(
        f.call(&[
            "task",
            "item",
            "reopen",
            &task,
            "--item-id",
            id,
            "--revision",
            "2"
        ])
        .unwrap_err()
        .code,
        "stale_result"
    );
    let mut reopened = f
        .call(&[
            "task",
            "item",
            "reopen",
            &task,
            "--item-id",
            id,
            "--revision",
            "3",
        ])
        .unwrap();
    reopened.as_object_mut().unwrap().remove("request_id");
    assert_eq!(reopened["completed"], false);
    assert_eq!(
        f.call(&["task", "item", "list", &task]).unwrap()["items"],
        json!([reopened])
    );
    let summary = f.call(&["task", "summary", &task]).unwrap();
    assert_eq!(summary["revision"], 0);
    assert!(summary["summary"].is_null());
    let delete = [
        "task",
        "item",
        "delete",
        &task,
        "--item-id",
        id,
        "--revision",
        "4",
        "--request-id",
        "delete-work",
    ];
    assert_eq!(f.call(&delete).unwrap()["deleted"], true);
    assert_eq!(f.call(&delete).unwrap()["deleted"], true);
    assert_eq!(
        f.call(&["task", "item", "list", &task]).unwrap()["items"],
        json!([])
    );
}

pub fn seed_summary(
    db: &crate::storage::Db,
    kind: &str,
    id: &str,
    revision: i64,
    summary: crate::domain::Summary,
) -> crate::error::Result<crate::domain::Summary> {
    let summary = summary.resolve_ids()?;
    let generation = if kind == "session" {
        db.get_session_ingest_state(id)?.0
    } else {
        0
    };
    let conn = db.write();
    match kind {
        "session" => {
            conn.execute("INSERT INTO session_contexts(session_id,summary,revision,ingest_generation,updated_at) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(session_id) DO UPDATE SET summary=?2,revision=?3",rusqlite::params![id,summary,revision,generation,crate::storage::now()])?;
        }
        "task" => {
            conn.execute("INSERT INTO workstream_context_state(workstream_id,summary,context_revision) VALUES(?1,?2,?3) ON CONFLICT(workstream_id) DO UPDATE SET summary=?2,context_revision=?3",rusqlite::params![id,summary,revision])?;
        }
        "project" => {
            conn.execute("INSERT INTO project_context_state(project_id,summary,context_revision) VALUES(?1,?2,?3) ON CONFLICT(project_id) DO UPDATE SET summary=?2,context_revision=?3",rusqlite::params![id,summary,revision])?;
        }
        _ => panic!("invalid fixture kind"),
    }
    Ok(summary)
}
