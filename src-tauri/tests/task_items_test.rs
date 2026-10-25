mod support;
struct TempDir(std::path::PathBuf);
impl TempDir {
    fn new() -> Self {
        let p =
            std::env::temp_dir().join(format!("ne-task-items-{}", noending_lib::storage::new_id()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
use noending_lib::{
    domain::{Summary, Workstream},
    storage::{new_id, Db},
    workspace::task_items::{self, CreateItem, ItemPatch},
};

fn task(db: &Db, title: &str) -> Workstream {
    let task = Workstream {
        id: new_id(),
        title: title.into(),
        description: "goal".into(),
        visibility: "normal".into(),
        created_at: "2026-10-25T00:00:00Z".into(),
        updated_at: "2026-10-25T00:00:00Z".into(),
    };
    db.upsert_workstream(&task).unwrap();
    task
}
fn create(db: &Db, task: &str) -> task_items::TaskItem {
    task_items::create(
        db,
        task,
        CreateItem {
            title: " 实现并验证 ".into(),
            description: Some("验收目标".into()),
        },
    )
    .unwrap()
}
#[test]
fn work_items_persist_separately_from_summaries_and_completion_retains_progress() {
    let dir = TempDir::new();
    let path = dir.path().join("db.sqlite");
    let db = Db::open(&path).unwrap();
    let task = task(&db, "独立工作事项");
    support::summary::seed_summary(&db, "task", &task.id, 1, Summary::single("历史结论")).unwrap();
    let first = create(&db, &task.id);
    let other = create(&db, &task.id);
    assert_eq!(first.title, "实现并验证");
    assert!(!first.completed);
    assert!(first.details.is_empty());
    let done = task_items::update(
        &db,
        &task.id,
        &first.id,
        1,
        ItemPatch {
            details: Some(vec!["回归通过，Windows 尚未实测".into()]),
            completed: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(done.description, "验收目标");
    assert_eq!(task_items::list(&db, &task.id).unwrap().len(), 2);
    // A generated summary replacement is never an execution-state change.
    support::summary::seed_summary(
        &db,
        "task",
        &task.id,
        2,
        Summary::single("重新整理后的记忆"),
    )
    .unwrap();
    drop(db);
    let db = Db::open(&path).unwrap();
    let items = task_items::list(&db, &task.id).unwrap();
    assert_eq!(items.iter().find(|i| i.id == done.id).unwrap(), &done);
    assert_eq!(items.iter().find(|i| i.id == other.id).unwrap(), &other);
    let reopened = task_items::update(
        &db,
        &task.id,
        &first.id,
        2,
        ItemPatch {
            completed: Some(false),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!reopened.completed);
    assert_eq!(reopened.details, done.details);
    let state = db.get_workstream_context_state(&task.id).unwrap();
    assert_eq!(state.context_revision, 2);
    assert_eq!(state.summary.items[0].title, "重新整理后的记忆");
    drop(db);
}
#[test]
fn item_edits_reject_stale_cross_task_and_invalid_input_without_affecting_other_work() {
    let dir = TempDir::new();
    let db = Db::open(&dir.path().join("db")).unwrap();
    let a = task(&db, "a");
    let b = task(&db, "b");
    let item = create(&db, &a.id);
    assert!(task_items::create(
        &db,
        "missing",
        CreateItem {
            title: "x".into(),
            description: None
        }
    )
    .is_err());
    assert!(task_items::update(
        &db,
        &b.id,
        &item.id,
        1,
        ItemPatch {
            completed: Some(true),
            ..Default::default()
        }
    )
    .is_err());
    assert!(task_items::delete(&db, &b.id, &item.id, 1).is_err());
    for patch in [
        ItemPatch::default(),
        ItemPatch {
            title: Some(" \n ".into()),
            ..Default::default()
        },
        ItemPatch {
            details: Some(vec![" ".into()]),
            ..Default::default()
        },
    ] {
        assert!(task_items::update(&db, &a.id, &item.id, 1, patch).is_err());
    }
    assert_eq!(task_items::list(&db, &a.id).unwrap(), vec![item.clone()]);
    let edited = task_items::update(
        &db,
        &a.id,
        &item.id,
        1,
        ItemPatch {
            description: Some(String::new()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(edited.description.is_empty());
    assert!(task_items::update(
        &db,
        &a.id,
        &item.id,
        1,
        ItemPatch {
            completed: Some(true),
            ..Default::default()
        }
    )
    .is_err());
    assert!(task_items::delete(&db, &a.id, &item.id, 1).is_err());
    assert_eq!(task_items::list(&db, &a.id).unwrap(), vec![edited]);
    task_items::delete(&db, &a.id, &item.id, 2).unwrap();
    assert!(task_items::list(&db, &a.id).unwrap().is_empty());
    drop(db);
}
#[test]
fn permanent_task_deletion_removes_its_work_items_but_not_other_tasks() {
    let dir = TempDir::new();
    let db = Db::open(&dir.path().join("db")).unwrap();
    let a = task(&db, "a");
    let b = task(&db, "b");
    let first = create(&db, &a.id);
    let second = create(&db, &b.id);
    noending_lib::workspace::workstream::archive_workstream(&db, &a.id).unwrap();
    assert_eq!(task_items::list(&db, &a.id).unwrap(), vec![first]);
    noending_lib::workspace::workstream::delete_workstream_permanently(&db, &a.id).unwrap();
    assert!(task_items::list(&db, &a.id).is_err());
    assert_eq!(task_items::list(&db, &b.id).unwrap(), vec![second]);
    drop(db);
}

#[test]
fn concurrent_progress_notes_preserve_existing_details_and_make_stale_replacements_fail() {
    let dir = TempDir::new();
    let path = dir.path().join("db");
    let db = Db::open(&path).unwrap();
    let current_task = task(&db, "并发追记");
    let item = create(&db, &current_task.id);
    support::summary::seed_summary(
        &db,
        "task",
        &current_task.id,
        1,
        Summary::single("独立摘要"),
    )
    .unwrap();
    let previous = (1..=9)
        .map(|i| format!("已核实成果 {i}"))
        .collect::<Vec<_>>();
    let before = task_items::update(
        &db,
        &current_task.id,
        &item.id,
        1,
        ItemPatch {
            details: Some(previous.clone()),
            completed: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut threads = vec![];
    let notes = ["验证了新增入口", "包含 \"引号\" 与中文\n保留必要边界"];
    for note in notes {
        let writer = Db::open(&path).unwrap();
        let wait = barrier.clone();
        let tid = current_task.id.clone();
        let iid = item.id.clone();
        threads.push(std::thread::spawn(move || {
            wait.wait();
            task_items::append_note(&writer, &tid, &iid, note).unwrap();
        }));
    }
    for thread in threads {
        thread.join().unwrap();
    }
    let actual = task_items::list(&db, &current_task.id).unwrap().remove(0);
    assert_eq!(&actual.details[..9], previous.as_slice());
    let mut appended = actual.details[9..].to_vec();
    appended.sort();
    let mut expected = notes.map(str::to_owned).to_vec();
    expected.sort();
    assert_eq!(appended, expected);
    assert_eq!(actual.revision, 4);
    assert!(actual.completed);
    assert_eq!(actual.title, before.title);
    assert_eq!(actual.description, before.description);
    assert_eq!(actual.created_at, before.created_at);
    let other = task(&db, "其他任务");
    assert!(task_items::append_note(&db, &other.id, &item.id, "不能串任务").is_err());
    assert!(task_items::append_note(&db, &current_task.id, &item.id, "  ").is_err());
    assert!(task_items::update(
        &db,
        &current_task.id,
        &item.id,
        before.revision,
        ItemPatch {
            details: Some(vec!["旧草稿".into()]),
            ..Default::default()
        }
    )
    .is_err());
    drop(db);
    let db = Db::open(&path).unwrap();
    assert_eq!(
        task_items::list(&db, &current_task.id).unwrap(),
        vec![actual]
    );
    let summary = db.get_workstream_context_state(&current_task.id).unwrap();
    assert_eq!(summary.context_revision, 1);
    assert_eq!(summary.summary.items[0].title, "独立摘要");
}
