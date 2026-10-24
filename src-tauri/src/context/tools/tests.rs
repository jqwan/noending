use super::*;
use crate::context::{
    tests::{append, seed, ServiceFixture},
    SessionSnapshot,
};

fn fixture(f: &ServiceFixture, id: &str) -> ExtractionFiles {
    let root = std::env::temp_dir().join(format!("noending-tools-{}", crate::storage::new_id()));
    let files = ExtractionFiles::create(&root, &format!("session-{id}"), &[id.into()]).unwrap();
    let snapshot =
        SessionSnapshot::read(f.db(), f.db().get_session(id).unwrap().unwrap(), false).unwrap();
    files.add_session(&snapshot, true, false);
    files
}
fn bridge<'a>(f: &'a ServiceFixture, files: &'a ExtractionFiles) -> Bridge<'a> {
    Bridge {
        db: f.db(),
        files,
        token: "test-capability".into(),
        delivered: Mutex::default(),
    }
}
fn send(bridge: &Bridge<'_>, command: Command) -> Result<Value> {
    bridge.dispatch(Request {
        token: "test-capability".into(),
        command,
    })
}
fn read_all(bridge: &Bridge<'_>) {
    let inputs = send(bridge, Command::Inputs).unwrap();
    for input in inputs["inputs"].as_array().unwrap() {
        let name = input["name"].as_str().unwrap();
        let mut cursor = 0;
        loop {
            let page = send(
                bridge,
                Command::Read {
                    name: name.into(),
                    cursor,
                    limit: 1,
                },
            )
            .unwrap();
            match page["next_cursor"].as_u64() {
                Some(next) => cursor = next as usize,
                None => break,
            }
        }
    }
}
#[test]
fn scoped_database_pages_and_cross_part_search_require_complete_reading() {
    let f = ServiceFixture::new();
    let id = f.add_session(1, 4000);
    // Distinct unicode parts verify exact character slicing and original order.
    f.db()
        .write()
        .execute(
            "UPDATE session_messages SET content=?1 WHERE session_id=?2",
            rusqlite::params![format!("{}{}", "甲".repeat(2000), "乙".repeat(2000)), id],
        )
        .unwrap();
    let files = fixture(&f, &id);
    let b = bridge(&f, &files);
    assert!(b
        .dispatch(Request {
            token: "wrong".into(),
            command: Command::Inputs
        })
        .is_err());
    let name = format!("session-{id}.jsonl");
    assert!(send(
        &b,
        Command::Read {
            name: "session-other.jsonl".into(),
            cursor: 0,
            limit: 1
        }
    )
    .is_err());
    assert!(send(
        &b,
        Command::Read {
            name: name.clone(),
            cursor: 0,
            limit: 0
        }
    )
    .is_err());
    let page = send(
        &b,
        Command::Read {
            name: name.clone(),
            cursor: 0,
            limit: 2,
        },
    )
    .unwrap();
    assert_eq!(page["records"][1]["record"]["content"], "甲".repeat(2000));
    assert_eq!(page["next_cursor"], 2);
    let search = send(
        &b,
        Command::Search {
            name: name.clone(),
            query: "甲乙".into(),
            cursor: 0,
            limit: 1,
        },
    )
    .unwrap();
    assert_eq!(search["records"][0]["cursor"], 1);
    assert_eq!(search["next_cursor"], 2);
    let next = send(
        &b,
        Command::Search {
            name,
            query: "甲乙".into(),
            cursor: 2,
            limit: 1,
        },
    )
    .unwrap();
    assert_eq!(next["records"][0]["record"]["content"], "乙".repeat(2000));
    assert_eq!(next["end"], true);
    std::fs::write(
        files.session_result(&id),
        r#"{"summary":{"items":[{"title":"结果","description":"","details":[]}]}}"#,
    )
    .unwrap();
    let reply = json!({"status":"completed"});
    assert!(b.validate(&reply).is_err());
    read_all(&b);
    assert!(b.validate(&reply).is_ok());
}
#[test]
fn queries_read_current_database_bodies_but_keep_message_frontier_and_detect_rewrites() {
    let f = ServiceFixture::new();
    let id = f.add_session(1, 10);
    seed(&f, &id, "旧摘要");
    append(&f, &id, "本次消息", false);
    let files = fixture(&f, &id);
    let b = bridge(&f, &files);
    // Changes after scope creation are visible when queried, not cached bodies.
    let updated = crate::domain::Summary::single("查询时摘要");
    f.db()
        .write()
        .execute(
            "UPDATE session_contexts SET summary=?2 WHERE session_id=?1",
            rusqlite::params![&id, &updated],
        )
        .unwrap();
    append(&f, &id, "截止位置之外", false);
    let name = format!("session-{id}.jsonl");
    let page = send(
        &b,
        Command::Read {
            name: name.clone(),
            cursor: 0,
            limit: 8,
        },
    )
    .unwrap();
    assert_eq!(
        page["records"][0]["record"]["current_summary"],
        json!(updated)
    );
    assert_eq!(page["records"][0]["record"]["through_sequence"], 2);
    assert_eq!(page["records"][1]["record"]["content"], "本次消息");
    assert_eq!(page["records"].as_array().unwrap().len(), 3);
    append(&f, &id, "改写后的历史", true);
    assert!(send(
        &b,
        Command::Read {
            name: name.clone(),
            cursor: 1,
            limit: 8
        }
    )
    .is_err());
    let fresh = fixture(&f, &id);
    let fresh_bridge = bridge(&f, &fresh);
    f.db()
        .write()
        .execute(
            "UPDATE session_contexts SET revision=revision+1 WHERE session_id=?1",
            [&id],
        )
        .unwrap();
    assert!(send(
        &fresh_bridge,
        Command::Read {
            name,
            cursor: 0,
            limit: 8
        }
    )
    .is_err());
}
#[test]
fn cli_is_read_only_reply_is_required_and_connection_expires() {
    let f = ServiceFixture::new();
    let id = f.add_session(1, 10);
    let files = fixture(&f, &id);
    let connection = files.dir.join("connection.json");
    assert!(run(f.db(), &files, "requirements", |_| Ok("普通说明".into())).is_err());
    assert!(request(&connection, Command::Inputs).is_err());
    let result = run(f.db(), &files, "requirements", |_| {
        let args = vec![
            "--connection".into(),
            connection.to_string_lossy().into_owned(),
            "extraction".into(),
            "inputs".into(),
        ];
        assert_eq!(
            crate::cli::execute(&args).map_err(other)?["inputs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|input| input["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![format!("session-{id}.jsonl")]
        );
        let mut invalid = args.clone();
        invalid.extend(["--sql".into(), "DELETE FROM sessions".into()]);
        assert!(crate::cli::execute(&invalid).is_err());
        assert!(crate::cli::execute(&[
            "--connection".into(),
            connection.to_string_lossy().into_owned(),
            "extraction".into(),
            "submit".into()
        ])
        .is_err());
        read_all_for_test(&files.dir);
        std::fs::write(
            files.session_result(&id),
            r#"{"summary":{"items":[{"title":"结果","description":"","details":[]}]}}"#,
        )?;
        Ok(r#"{"status":"completed"}"#.into())
    })
    .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&result).unwrap()["status"],
        "completed"
    );
    assert!(request(&connection, Command::Inputs).is_err());
}
#[test]
fn batch_partial_only_accepts_failed_ids_within_pending_scope() {
    let f = ServiceFixture::new();
    let a = f.add_session(1, 10);
    let b = f.add_session(1, 10);
    let root = std::env::temp_dir().join(format!("noending-tools-{}", crate::storage::new_id()));
    let files = ExtractionFiles::create(&root, "task-t", &[a.clone(), b.clone()]).unwrap();
    let sessions: Vec<_> = [&a, &b]
        .into_iter()
        .map(|id| {
            SessionSnapshot::read(f.db(), f.db().get_session(id).unwrap().unwrap(), false).unwrap()
        })
        .collect();
    for s in &sessions {
        files.add_session(s, true, false);
    }
    files.add_batch("task", "t", json!({"type":"task"}), 0, false, sessions);
    std::fs::write(
        files.batch_result("task", "t"),
        r#"{"summary":{"items":[{"title":"任务结果","description":"","details":[]}]}}"#,
    )
    .unwrap();
    std::fs::write(
        files.session_result(&a),
        r#"{"summary":{"items":[{"title":"A","description":"","details":[]}]}}"#,
    )
    .unwrap();
    let bridge = bridge(&f, &files);
    read_all(&bridge);
    let reply = |id: &str| json!({"status":"partial","failed_sessions":[{"session_id":id,"error":"无法读取"}],"skipped_session_ids":[]});
    assert!(bridge.validate(&reply("outside")).is_err());
    assert!(bridge.validate(&reply(&b)).is_ok());
}
