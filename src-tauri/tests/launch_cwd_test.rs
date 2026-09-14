//! New Session launch directory resolution (launcher::resolve_new_session_cwd).
//! Priority: explicit caller value > a selected Workstream's default_cwd
//! (first set one in selection order) > the most recent session cwd across
//! the selected Workstreams > None. A Workstream's default_cwd is a launch
//! convenience — a Workstream is not a path, and Sessions keep their own cwd.

use std::path::PathBuf;

use noending::adapters::DiscoveredSession;
use noending::domain::{binding_source, Agent};
use noending::ingestion::ensure_session_row;
use noending::launcher::{record_binding, resolve_new_session_cwd};
use noending::storage::{new_id, now, Db};

fn db(tag: &str) -> Db {
    let dir = std::env::temp_dir().join(format!("noending-launch-cwd-{}-{}", tag, new_id()));
    Db::open(&dir.join("test.db")).unwrap()
}

fn workstream(database: &Db, title: &str, default_cwd: Option<&str>) -> String {
    let w = noending::domain::Workstream {
        id: new_id(),
        project_id: None,
        title: title.into(),
        description: String::new(),
        lifecycle: "open".into(),
        visibility: "normal".into(),
        default_cwd: default_cwd.map(|s| s.to_string()),
        created_at: now(),
        updated_at: now(),
    };
    database.upsert_workstream(&w).unwrap();
    w.id
}

fn session_with_cwd(database: &Db, agent_session_id: &str, cwd: &str, activity: &str) -> String {
    ensure_session_row(
        database,
        &DiscoveredSession {
            agent: Agent::Codex,
            agent_session_id: agent_session_id.into(),
            path: PathBuf::from(format!("/tmp/fake/{}.jsonl", agent_session_id)),
            cwd: Some(cwd.into()),
            started_at: Some("2026-09-01T08:00:00Z".into()),
            last_activity_at: Some(activity.into()),
            first_user_text: None,
            parent_agent_session_id: None,
        },
    )
    .unwrap()
    .0
    .id
}

fn bind(database: &Db, session_id: &str, workstream_id: &str) {
    record_binding(
        database,
        session_id,
        workstream_id,
        "related",
        binding_source::USER_ASSIGNED,
        1.0,
    )
    .unwrap();
}

#[test]
fn explicit_cwd_wins_over_everything() {
    let database = db("explicit");
    let w = workstream(&database, "W", Some("/default/dir"));

    assert_eq!(
        resolve_new_session_cwd(&database, &[w], Some("/explicit/dir"))
            .unwrap()
            .as_deref(),
        Some("/explicit/dir")
    );
}

#[test]
fn workstream_default_cwd_beats_inferred_session_cwd() {
    let database = db("ws-default");
    let w = workstream(&database, "W", Some("/default/dir"));
    let sid = session_with_cwd(&database, "s1", "/session/cwd", "2026-09-12T09:00:00Z");
    bind(&database, &sid, &w);

    assert_eq!(
        resolve_new_session_cwd(&database, &[w], None)
            .unwrap()
            .as_deref(),
        Some("/default/dir")
    );
}

#[test]
fn first_selected_workstream_with_default_cwd_wins() {
    let database = db("order");
    let a = workstream(&database, "A", None);
    let b = workstream(&database, "B", Some("/b/dir"));
    let c = workstream(&database, "C", Some("/c/dir"));

    assert_eq!(
        resolve_new_session_cwd(&database, &[a.clone(), b.clone(), c.clone()], None)
            .unwrap()
            .as_deref(),
        Some("/b/dir"),
        "selection order decides which default applies"
    );
    assert_eq!(
        resolve_new_session_cwd(&database, &[c.clone(), b], None)
            .unwrap()
            .as_deref(),
        Some("/c/dir")
    );
    let _ = a;
}

#[test]
fn falls_back_to_latest_session_cwd_then_none() {
    let database = db("fallback");
    let w = workstream(&database, "W", None);

    // no bound sessions → None (terminal default / $HOME)
    assert_eq!(
        resolve_new_session_cwd(&database, &[w.clone()], None).unwrap(),
        None
    );

    // the most recent session's cwd is the inference
    let old = session_with_cwd(&database, "s-old", "/old/dir", "2026-09-10T09:00:00Z");
    let new = session_with_cwd(&database, "s-new", "/new/dir", "2026-09-12T09:00:00Z");
    bind(&database, &old, &w);
    bind(&database, &new, &w);

    assert_eq!(
        resolve_new_session_cwd(&database, &[w], None)
            .unwrap()
            .as_deref(),
        Some("/new/dir")
    );
}

#[test]
fn default_cwd_roundtrips_through_storage() {
    let database = db("roundtrip");
    let w = workstream(&database, "W", Some("/some/dir"));

    let stored = database.get_workstream(&w).unwrap().unwrap();
    assert_eq!(stored.default_cwd.as_deref(), Some("/some/dir"));

    // clearing persists too
    let mut cleared = stored;
    cleared.default_cwd = None;
    database.upsert_workstream(&cleared).unwrap();
    assert_eq!(
        database.get_workstream(&w).unwrap().unwrap().default_cwd,
        None
    );
}
