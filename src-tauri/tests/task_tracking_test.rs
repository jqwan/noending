//! Observable task membership, tracking and project/preference lifecycle contracts.
mod support;
use noending::storage::{
    preferences,
    workspace::{insert_workspace_path_conn, reassign_workspace_path_project_conn},
    workstream_projects as relations,
};
use noending::{
    domain::Agent,
    storage::{new_id, Db},
    workspace::{project, workstream},
};
use noending_lib as noending;
struct Fixture {
    db: Option<Db>,
    root: std::path::PathBuf,
    sid: String,
    path: String,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("ne-tracking-{}", new_id()));
        let db = Db::open(&root.join("db.sqlite")).unwrap();
        db.upsert_project(&support::project("a".into(), "A"))
            .unwrap();
        let mut b = support::project("b".into(), "B");
        b.name_customized = true;
        db.upsert_project(&b).unwrap();
        let path =
            insert_workspace_path_conn(&db.write(), &root.join("work").to_string_lossy(), "a")
                .unwrap();
        let (sid, _) = db
            .upsert_logical_session_unchecked(
                Agent::Codex,
                "native",
                Some("Discussion"),
                Some("/work"),
                Some(&path),
                None,
                None,
                None,
            )
            .unwrap();
        Self {
            db: Some(db),
            root,
            sid,
            path,
        }
    }
    fn db(&self) -> &Db {
        self.db.as_ref().unwrap()
    }
    fn task(&self, title: &str, projects: &[&str]) -> String {
        workstream::create_workstream(
            self.db(),
            title,
            "Goal",
            &projects.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
        .unwrap()
        .id
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        drop(self.db.take());
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
#[test]
fn multiple_tasks_track_one_session_without_duplicate_membership_or_project_mutation() {
    let f = Fixture::new();
    let a = f.task("Task one", &["a"]);
    let b = f.task("Task two", &["a"]);
    for id in [&a, &b, &a] {
        f.db().set_session_tracking(&f.sid, id, true).unwrap();
    }
    let session = f.db().get_session(&f.sid).unwrap().unwrap();
    assert_eq!(session.tracked_workstream_ids.len(), 2);
    assert_eq!(session.project_id.as_deref(), Some("a"));
    assert_eq!(session.workspace_path_id.as_deref(), Some(f.path.as_str()));
    for id in [&a, &b] {
        assert_eq!(f.db().sessions_for_workstream(id).unwrap().len(), 1);
        assert_eq!(f.db().workstream_session_stats(id).unwrap().0, 1);
    }
    f.db().set_session_tracking(&f.sid, &a, false).unwrap();
    assert_eq!(
        f.db()
            .get_session(&f.sid)
            .unwrap()
            .unwrap()
            .tracked_workstream_ids,
        vec![b]
    );
}
#[test]
fn candidates_require_current_project_and_unarchived_task_but_existing_links_survive() {
    let f = Fixture::new();
    let other = f.task("Other", &["b"]);
    let task = f.task("Current", &["a"]);
    assert!(f.db().set_session_tracking(&f.sid, &other, true).is_err());
    f.db().set_session_tracking(&f.sid, &task, true).unwrap();
    workstream::archive_workstream(f.db(), &task).unwrap();
    assert_eq!(f.db().sessions_for_workstream(&task).unwrap().len(), 1);
    f.db().set_session_tracking(&f.sid, &task, false).unwrap();
    assert!(f.db().set_session_tracking(&f.sid, &task, true).is_err());
}
#[test]
fn directory_reassignment_moves_session_but_never_explicit_task_projects_or_tracking() {
    let f = Fixture::new();
    let task = f.task("Keep scope", &["a"]);
    f.db().set_session_tracking(&f.sid, &task, true).unwrap();
    reassign_workspace_path_project_conn(&f.db().write(), &f.path, "b").unwrap();
    assert_eq!(
        f.db()
            .get_session(&f.sid)
            .unwrap()
            .unwrap()
            .project_id
            .as_deref(),
        Some("b")
    );
    assert_eq!(
        relations::project_ids_for_workstream(&f.db().read(), &task).unwrap(),
        vec!["a"]
    );
    assert_eq!(f.db().sessions_for_workstream(&task).unwrap().len(), 1);
    f.db().set_session_tracking(&f.sid, &task, false).unwrap();
    assert!(f.db().set_session_tracking(&f.sid, &task, true).is_err());
}
#[test]
fn task_project_edits_are_atomic_and_do_not_import_project_sessions() {
    let f = Fixture::new();
    let task = f.task("Scope", &["a", "a"]);
    assert_eq!(
        relations::project_ids_for_workstream(&f.db().read(), &task).unwrap(),
        vec!["a"]
    );
    assert!(f.db().sessions_for_workstream(&task).unwrap().is_empty());
    assert!(
        workstream::set_workstream_projects(f.db(), &task, &["b".into(), "missing".into()])
            .is_err()
    );
    assert_eq!(
        relations::project_ids_for_workstream(&f.db().read(), &task).unwrap(),
        vec!["a"]
    );
    assert!(workstream::create_workstream(f.db(), "Failed", "", &["missing".into()]).is_err());
    assert_eq!(f.db().list_workstreams(None).unwrap().len(), 1);
}
#[test]
fn archived_task_deletion_removes_only_its_tracking_and_summary() {
    let f = Fixture::new();
    let a = f.task("One", &["a"]);
    let b = f.task("Two", &["a"]);
    f.db().set_session_tracking(&f.sid, &a, true).unwrap();
    f.db().set_session_tracking(&f.sid, &b, true).unwrap();
    assert!(workstream::delete_workstream_permanently(f.db(), &a).is_err());
    workstream::archive_workstream(f.db(), &a).unwrap();
    workstream::delete_workstream_permanently(f.db(), &a).unwrap();
    assert_eq!(
        f.db()
            .get_session(&f.sid)
            .unwrap()
            .unwrap()
            .tracked_workstream_ids,
        vec![b]
    );
    assert!(f.db().get_workspace_path(&f.path).unwrap().is_some());
}
#[test]
fn launch_tracking_adds_without_replacing_existing_and_missing_selection_does_not_block_binding() {
    let f = Fixture::new();
    let a = f.task("Chosen", &["a"]);
    let b = f.task("Existing", &["a"]);
    let c = f.task("Also chosen", &["a"]);
    f.db().set_session_tracking(&f.sid, &b, true).unwrap();
    let session = f.db().get_session(&f.sid).unwrap().unwrap();
    f.db()
        .inherit_terminal_tracking(
            &session,
            &[a.clone(), c.clone(), a.clone(), "deleted".into()],
        )
        .unwrap();
    assert_eq!(
        f.db()
            .get_session(&f.sid)
            .unwrap()
            .unwrap()
            .tracked_workstream_ids
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        [a.clone(), b, c]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
    );
    f.db()
        .inherit_terminal_tracking(&session, &["deleted".into()])
        .unwrap();
    let mut wrong = session;
    wrong.root_agent_session_id = "other".into();
    assert!(f
        .db()
        .inherit_terminal_tracking(&wrong, &[a.clone()])
        .is_err());
}
#[test]
fn preference_promotion_moves_scope_preserves_lock_and_coalesces_existing_user_item() {
    let f = Fixture::new();
    let item = preferences::save_item(f.db(), None, Some("a"), "Keep concise", true).unwrap();
    let promoted = preferences::promote_item(f.db(), &item.id).unwrap();
    assert!(promoted.project_id.is_none());
    assert!(promoted.locked);
    assert_eq!(promoted.id, item.id);
    assert!(preferences::list_items(f.db(), Some("a"))
        .unwrap()
        .is_empty());
    let p = preferences::save_item(f.db(), None, Some("a"), "Explain changes", true).unwrap();
    let u = preferences::save_item(f.db(), None, None, "Explain changes", false).unwrap();
    let combined = preferences::promote_item(f.db(), &p.id).unwrap();
    assert_eq!(combined.id, u.id);
    assert!(combined.locked);
    assert_eq!(preferences::list_items(f.db(), None).unwrap().len(), 2);
    assert!(preferences::save_item(f.db(), None, Some("a"), "Keep concise", false).is_err());
    let other = preferences::save_item(f.db(), None, Some("a"), "Another rule", false).unwrap();
    assert!(
        preferences::save_item(f.db(), Some(&other.id), Some("a"), "Keep concise", false).is_err()
    );
    assert_eq!(
        preferences::list_items(f.db(), Some("a")).unwrap()[0].content,
        "Another rule"
    );
}
#[test]
fn project_merge_folds_task_links_and_preferences_keeps_b_summary_and_locked_duplicates() {
    let f = Fixture::new();
    let task = f.task("Shared", &["a", "b"]);
    f.db()
        .tx(|tx| {
            noending::storage::project_context_repo::commit(
                tx,
                "a",
                "old",
                &noending_lib::domain::Summary::single("A summary"),
                &[],
            )?;
            noending::storage::project_context_repo::commit(
                tx,
                "b",
                "old",
                &noending_lib::domain::Summary::single("B summary"),
                &[],
            )?;
            Ok(())
        })
        .unwrap();
    preferences::save_item(f.db(), None, Some("a"), "Same rule", true).unwrap();
    preferences::save_item(f.db(), None, Some("b"), "Same rule", false).unwrap();
    preferences::save_item(f.db(), None, Some("a"), "Unique rule", false).unwrap();
    let revision_before =
        noending::storage::get_workstream_context_state_conn(&f.db().read(), &task)
            .unwrap()
            .input_revision;
    let merged = project::merge_projects(f.db(), "a", "b").unwrap();
    assert_eq!(merged.survivor, "b");
    assert!(
        noending::storage::get_workstream_context_state_conn(&f.db().read(), &task)
            .unwrap()
            .input_revision
            > revision_before
    );
    assert_eq!(
        relations::project_ids_for_workstream(&f.db().read(), &task).unwrap(),
        vec!["b"]
    );
    assert_eq!(
        noending::storage::project_context_repo::state(&f.db().read(), "b")
            .unwrap()
            .summary,
        noending_lib::domain::Summary::single("B summary")
    );
    let items = preferences::list_items(f.db(), Some("b")).unwrap();
    assert_eq!(items.len(), 2);
    assert!(
        items
            .iter()
            .find(|p| p.content == "Same rule")
            .unwrap()
            .locked
    );
    assert_eq!(
        f.db()
            .get_session(&f.sid)
            .unwrap()
            .unwrap()
            .project_id
            .as_deref(),
        Some("b")
    );
}
#[test]
fn project_merge_adopts_a_summary_only_when_b_has_none() {
    let f = Fixture::new();
    f.db()
        .tx(|tx| {
            noending::storage::project_context_repo::commit(
                tx,
                "a",
                "old",
                &noending_lib::domain::Summary::single("A summary"),
                &[],
            )
        })
        .unwrap();
    project::merge_projects(f.db(), "a", "b").unwrap();
    assert_eq!(
        noending::storage::project_context_repo::state(&f.db().read(), "b")
            .unwrap()
            .summary,
        noending_lib::domain::Summary::single("A summary")
    );
    assert!(
        noending::context::project_context_view(f.db(), "b")
            .unwrap()
            .pending
    );
}

#[test]
fn archive_restore_and_stale_object_edits_preserve_task_memberships_and_summary() {
    let f = Fixture::new();
    let task = f.task("Lifecycle", &["a"]);
    f.db().set_session_tracking(&f.sid, &task, true).unwrap();
    f.db()
        .write()
        .execute(
            "UPDATE workstream_context_state SET summary='{\"items\":[{\"title\":\"Retained summary\",\"description\":\"\",\"details\":[]}]}' WHERE workstream_id=?1",
            [&task],
        )
        .unwrap();
    let stale = f.db().get_workstream(&task).unwrap().unwrap();
    workstream::archive_workstream(f.db(), &task).unwrap();
    workstream::archive_workstream(f.db(), &task).unwrap();
    assert!(workstream::apply_whole_object_edit(f.db(), &stale).is_err());
    let restored = workstream::restore_workstream(f.db(), &task).unwrap();
    assert_eq!(restored.visibility, "normal");
    assert_eq!(
        relations::project_ids_for_workstream(&f.db().read(), &task).unwrap(),
        vec!["a"]
    );
    assert_eq!(f.db().sessions_for_workstream(&task).unwrap().len(), 1);
    assert_eq!(
        noending::storage::get_workstream_context_state_conn(&f.db().read(), &task)
            .unwrap()
            .summary,
        noending_lib::domain::Summary::single("Retained summary")
    );
    assert!(workstream::create_workstream(f.db(), " ", "", &[]).is_err());
}
#[test]
fn task_search_rename_and_purge_follow_all_tracking_links_and_remove_only_task_data() {
    let f = Fixture::new();
    let task = f.task("Search old", &["a"]);
    let sibling = f.task("Sibling", &["a"]);
    for id in [&task, &sibling] {
        f.db().set_session_tracking(&f.sid, id, true).unwrap();
    }
    f.db().write().execute("INSERT INTO workstream_session_frontiers(workstream_id,session_id,session_context_revision) VALUES (?1,?2,1)",rusqlite::params![task,f.sid]).unwrap();
    workstream::update_workstream_fields(f.db(), &task, Some("Search new"), None).unwrap();
    let body: String = f
        .db()
        .read()
        .query_row(
            "SELECT body FROM search_index WHERE kind='session' AND ref_id=?1",
            [&f.sid],
            |r| r.get(0),
        )
        .unwrap();
    assert!(body.contains("Search new"));
    assert!(!body.contains("Search old"));
    assert!(body.contains("Sibling"));
    workstream::archive_workstream(f.db(), &task).unwrap();
    workstream::delete_workstream_permanently(f.db(), &task).unwrap();
    for table in [
        "workstream_projects",
        "session_workstreams",
        "workstream_context_state",
        "workstream_session_frontiers",
    ] {
        let count: i64 = f
            .db()
            .read()
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE workstream_id=?1"),
                [&task],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
    let body: String = f
        .db()
        .read()
        .query_row(
            "SELECT body FROM search_index WHERE kind='session' AND ref_id=?1",
            [&f.sid],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!body.contains("Search new"));
    assert!(body.contains("Sibling"));
    assert!(f.db().get_workstream(&sibling).unwrap().is_some());
    assert!(f.db().get_session(&f.sid).unwrap().is_some());
}
