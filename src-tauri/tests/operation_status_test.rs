mod support;
use noending_lib::{
    operations::{self, Operation, OperationStatus, State},
    storage::Db,
};
use std::{
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Command, Stdio},
};
struct Fixture {
    root: PathBuf,
    db: Option<Db>,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("ne-operation-{}", noending_lib::storage::new_id()));
        std::fs::create_dir_all(&root).unwrap();
        let db = Db::open(&root.join("test.db")).unwrap();
        Self { root, db: Some(db) }
    }
    fn db(&self) -> &Db {
        self.db.as_ref().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        drop(self.db.take());
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn operations_are_visible_to_other_database_connections_and_clear_after_failure() {
    let f = Fixture::new();
    let other = Db::open(&f.root.join("test.db")).unwrap();
    let result: Result<(), &str> = (|| {
        let mut operation =
            Operation::start(f.db(), OperationStatus::summary("task", "task-one")).unwrap();
        operation
            .add_sessions(["session-one".into(), "session-two".into()])
            .unwrap();
        let statuses = operations::list(&other).unwrap();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].state, State::Running);
        assert_eq!(
            statuses[0]
                .targets
                .iter()
                .map(|t| (t.kind.as_str(), t.id.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("task", "task-one"),
                ("session", "session-one"),
                ("session", "session-two")
            ]
        );
        let unrelated = Fixture::new();
        assert!(operations::list(unrelated.db()).unwrap().is_empty());
        Err("model failed")
    })();
    assert!(result.is_err());
    assert!(operations::list(&other).unwrap().is_empty());
}

#[test]
fn independent_process_operations_are_visible_and_do_not_stay_busy_after_a_crash() {
    let f = Fixture::new();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "operation_owner", "--ignored", "--nocapture"])
        .env("NOENDING_OPERATION_TEST_DB", f.root.join("test.db"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    loop {
        let mut line = String::new();
        assert!(
            reader.read_line(&mut line).unwrap() > 0,
            "owner exited before becoming active"
        );
        if line.contains("OWNER_READY") {
            break;
        }
    }
    let statuses = operations::list(f.db()).unwrap();
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].targets[0].id, "external-session");
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(operations::list(f.db()).unwrap().is_empty());
}

#[test]
#[ignore = "subprocess fixture, invoked by independent_process_operations_are_visible_and_do_not_stay_busy_after_a_crash"]
fn operation_owner() {
    let path = std::env::var_os("NOENDING_OPERATION_TEST_DB")
        .expect("parent must provide an isolated database");
    let db = Db::open(&PathBuf::from(path)).unwrap();
    let _operation = Operation::start(
        &db,
        OperationStatus::sync("session", "external-session", None),
    )
    .unwrap();
    println!("OWNER_READY");
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
}

#[test]
fn source_sync_only_affects_sessions_in_its_discovery_roots() {
    use noending_lib::domain::Agent;
    let f = Fixture::new();
    let root = f.root.join("source");
    let other_root = f.root.join("source-other");
    let source = f
        .db()
        .add_ingest_source(Agent::Codex, &root.to_string_lossy())
        .unwrap();
    let _operation = Operation::start(
        f.db(),
        OperationStatus::source(f.db(), &source.id, Some(Agent::Codex)).unwrap(),
    )
    .unwrap();
    let (inside, _) = support::seed_conversation(f.db(), Agent::Codex, "inside", &[]);
    let (outside, _) = support::seed_conversation(f.db(), Agent::Codex, "outside", &[]);
    f.db()
        .write()
        .execute(
            "UPDATE sessions SET source_path=?2 WHERE id=?1",
            rusqlite::params![inside.id, root.join("s.jsonl").to_string_lossy()],
        )
        .unwrap();
    f.db()
        .write()
        .execute(
            "UPDATE sessions SET source_path=?2 WHERE id=?1",
            rusqlite::params![outside.id, other_root.join("s.jsonl").to_string_lossy()],
        )
        .unwrap();
    let status = operations::list(f.db()).unwrap().remove(0);
    assert!(status
        .targets
        .iter()
        .any(|t| t.kind == "source" && t.id == source.id));
    assert!(status
        .targets
        .iter()
        .any(|t| t.kind == "session" && t.id == inside.id));
    assert!(!status
        .targets
        .iter()
        .any(|t| t.kind == "session" && t.id == outside.id));
}
