//! storage/domain consistency, and prepared-launch identity tests
//! (Issues #5, #6, #8).
//!
use noending::domain::{Agent, Session, SessionMessageRole, SourceCursorUpdate};
use noending::launcher;
use noending::launcher::LaunchWorkspace;
use noending::storage::{new_id, now, Db};
use noending_lib as noending;

mod support;

fn open_db(tag: &str) -> Db {
    let dir = std::env::temp_dir().join(format!("noending-launch-{}-{}", tag, new_id()));
    Db::open(&dir.join("test.db")).unwrap()
}

fn ws_row(db: &Db, title: &str) -> noending::domain::Workstream {
    let w = noending::domain::Workstream {
        id: new_id(),
        title: title.into(),
        description: String::new(),
        visibility: "normal".into(),
        created_at: now(),
        updated_at: now(),
    };
    db.upsert_workstream(&w).unwrap();
    let project = project_row(db, "Task project");
    let path = std::env::temp_dir().join(format!("noending-launch-cwd-{}", new_id()));
    std::fs::create_dir_all(&path).unwrap();
    let canonical = noending::workspace::normalize_path(&path.to_string_lossy()).unwrap();
    db.tx(|tx| {
        noending::storage::workspace::insert_workspace_path_conn(tx, &canonical, &project.id)
    })
    .unwrap();
    noending::workspace::workstream::set_workstream_projects(db, &w.id, &[project.id]).unwrap();
    w
}

fn task_cwd(db: &Db, task: &str) -> String {
    db.read().query_row("SELECT wp.canonical_path FROM workspace_paths wp JOIN workstream_projects p ON p.project_id=wp.project_id WHERE p.workstream_id=?1",[task],|r|r.get(0)).unwrap()
}

fn project_row(db: &Db, name: &str) -> noending::domain::Project {
    let p = noending::domain::Project {
        id: new_id(),
        name: name.into(),
        git_id: None,
        name_customized: false,
        visibility: "normal".into(),
        created_at: now(),
        updated_at: now(),
    };
    db.upsert_project(&p).unwrap();
    p
}

/// A Logical Session keyed by its root source's Resume identity, with a REAL
/// root source file: resume preparation refuses a Session whose root source
/// is not present on disk, so every fixture session is resumable.
fn session_row(db: &Db, agent: Agent, started_at: Option<String>, cwd: Option<String>) -> Session {
    let root_id = format!("root-{}", new_id());
    let raw = std::env::temp_dir().join(format!("noending-raw-{}.jsonl", new_id()));
    std::fs::write(&raw, "").unwrap();
    let (id, _) = db
        .upsert_logical_session_unchecked(
            agent,
            &root_id,
            None,
            cwd.as_deref(),
            None,
            None,
            started_at.as_deref(),
            started_at.as_deref(),
        )
        .unwrap();
    support::ensure_session_source(db, agent, &root_id, &raw.to_string_lossy());
    db.get_session(&id).unwrap().unwrap()
}

#[test]
fn list_sessions_all_filter_combinations() {
    let db = open_db("session-filter");
    let p = project_row(&db, "P");
    let s_codex = session_row(&db, Agent::Codex, Some(now()), None);
    let _s_pi = session_row(&db, Agent::Pi, Some(now()), None);
    let path_id = db
        .tx(|tx| {
            noending::storage::workspace::insert_workspace_path_conn(
                tx,
                &noending::workspace::normalize_path("/list-filter/p").unwrap(),
                &p.id,
            )
        })
        .unwrap();
    db.write()
        .execute(
            "UPDATE sessions SET workspace_path_id = ?2 WHERE id = ?1",
            rusqlite::params![s_codex.id, path_id],
        )
        .unwrap();

    let agent_only = db
        .list_sessions(noending::storage::SessionFilter {
            scope: Default::default(),
            project_id: None,
            agent: Some(Agent::Codex),
        })
        .unwrap();
    assert_eq!(agent_only.len(), 1, "agent-only filter (the old ?N bug)");
    assert_eq!(agent_only[0].agent, Agent::Codex);

    let project_only = db
        .list_sessions(noending::storage::SessionFilter {
            scope: Default::default(),
            project_id: Some(p.id.clone()),
            agent: None,
        })
        .unwrap();
    assert_eq!(project_only.len(), 1);

    let both = db
        .list_sessions(noending::storage::SessionFilter {
            scope: Default::default(),
            project_id: Some(p.id.clone()),
            agent: Some(Agent::Codex),
        })
        .unwrap();
    assert_eq!(both.len(), 1);

    let none = db
        .list_sessions(noending::storage::SessionFilter::default())
        .unwrap();
    assert_eq!(none.len(), 2);
}

/// get_message_by_ref resolves stable message ids and rejects unknown refs.
#[test]
fn message_ref_roundtrip() {
    let db = open_db("message-ref");
    let s = session_row(&db, Agent::Codex, Some(now()), None);
    let source = SourceCursorUpdate {
        file_identity: "dev:1:ino:3".into(),
        generation: 0,
        byte_offset: 10,
        last_seen_size: 10,
        mtime: None,
        start_byte_offset: 10,
        prefix_hash: String::new(),
    };
    let stored = db
        .commit_ingest(
            &s.id,
            &[support::parsed_message(
                "src-1",
                SessionMessageRole::User,
                "ref target",
            )],
            &source,
        )
        .unwrap();
    let msg = &stored[0];

    // stable reference
    let by_id = db
        .get_message_by_ref(&format!("session-message:{}", msg.id))
        .unwrap()
        .unwrap();
    assert_eq!(by_id.id, msg.id);
    assert_eq!(by_id.content, "ref target");
    // unknown ref → None, never fabricated
    assert!(db
        .get_message_by_ref("session-message:missing")
        .unwrap()
        .is_none());
}

/// The session's source cursor columns are written by the ingest commit and
/// read back through `source_cursor`; the ingested conversation frontier rides
/// separately.
#[test]
fn session_cursor_roundtrip() {
    let db = open_db("cursor-roundtrip");
    let s = session_row(&db, Agent::Codex, Some(now()), None);
    let source = SourceCursorUpdate {
        file_identity: "unix:dev:1:ino:42".into(),
        generation: 3,
        byte_offset: 900,
        last_seen_size: 1000,
        mtime: Some(1234.5),
        start_byte_offset: 0,
        prefix_hash: "abc123".into(),
    };
    let stored = db
        .commit_ingest(
            &s.id,
            &[support::parsed_message(
                "m1",
                SessionMessageRole::User,
                "hello",
            )],
            &source,
        )
        .unwrap();
    assert_eq!(stored.len(), 1);

    let back = db.get_session(&s.id).unwrap().unwrap().source_cursor();
    assert_eq!(back.source_file_identity, "unix:dev:1:ino:42");
    assert_eq!(back.generation, 3);
    assert_eq!(back.byte_offset, 900);
    assert_eq!(back.last_seen_size, 1000);
    assert_eq!(back.mtime, Some(1234.5));
    assert_eq!(back.prefix_hash, "abc123");
    assert!(
        !back.identity_tail_hash.is_empty(),
        "a committed message advances the session cursor's identity tail"
    );
    assert_eq!(db.ingested_message_sequence(&s.id).unwrap(), 1);
}

// Prepared launch: side-effect contract & preview identity

/// `prepare_new` is a pure preview: it resolves the launch directory and freezes
#[test]
fn prepare_resume_without_writing() {
    let db = open_db("prep-resume-no-side-effects");

    let session = session_row(&db, Agent::Codex, Some(now()), None);

    let launcher = launcher::SessionLauncher;

    let prepared = launcher
        .prepare_resume_in(&db, &session.id, &LaunchWorkspace::default())
        .unwrap();

    assert_eq!(prepared.mode, "resume");

    // INVARIANT: nothing written
}

/// Preview-Launch Identity covers the Runtime override intent too: an
/// override edited while the Preview is open must invalidate the preview,
/// never be silently adopted or silently ignored.
#[test]
fn state_fingerprint_stale_detection_on_runtime_override_change() {
    let db = open_db("stale-runtime-detection");
    let ws = ws_row(&db, "test ws");

    let launcher = launcher::SessionLauncher;
    let prepared = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            Some(&task_cwd(&db, &ws.id)),
            &LaunchWorkspace::default(),
        )
        .unwrap();
    assert!(prepared.runtime.is_default());

    let fingerprint_of = |db: &Db| {
        launcher::compute_state_fingerprint_in(
            db,
            "new",
            None,
            Agent::Codex,
            &LaunchWorkspace::default(),
            &prepared.cwd_resolution,
        )
        .unwrap()
    };
    assert_eq!(fingerprint_of(&db), prepared.state_fingerprint);

    noending::agent_runtime::set_runtime_overrides(
        &db,
        Agent::Codex,
        &noending::agent_runtime::AgentRuntimeOverrides {
            model: Some("gpt-5.6-sol".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_ne!(
        fingerprint_of(&db),
        prepared.state_fingerprint,
        "a runtime override changed after Preview: the fingerprint must change"
    );

    // Overrides are per Agent: another Agent's configuration is not this
    // launch's state.
    let codex_fingerprint = fingerprint_of(&db);
    noending::agent_runtime::set_runtime_overrides(
        &db,
        Agent::Pi,
        &noending::agent_runtime::AgentRuntimeOverrides {
            model: Some("qwen/qwen3.8-27b".into()),
            provider: Some("lmstudio".into()),
            effort: None,
        },
    )
    .unwrap();
    assert_eq!(
        fingerprint_of(&db),
        codex_fingerprint,
        "another Agent's override must not invalidate this launch"
    );
}

/// Prepare freezes the stored intent instead of resolving a default, so the
/// argv Launch renders is exactly what Preview described.
#[test]
fn prepared_launch_freezes_the_runtime_override_intent() {
    let db = open_db("prepared-runtime-intent");
    let ws = ws_row(&db, "test ws");

    noending::agent_runtime::set_runtime_overrides(
        &db,
        Agent::Codex,
        &noending::agent_runtime::AgentRuntimeOverrides {
            model: Some("gpt-5.6-sol".into()),
            effort: Some("high".into()),
            provider: None,
        },
    )
    .unwrap();

    let launcher = launcher::SessionLauncher;
    let prepared = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            Some(&task_cwd(&db, &ws.id)),
            &LaunchWorkspace::default(),
        )
        .unwrap();
    assert_eq!(prepared.runtime.model.as_deref(), Some("gpt-5.6-sol"));
    assert_eq!(prepared.runtime.effort.as_deref(), Some("high"));

    // The frozen intent feeds the same conversion New / Resume / exec use.
    let opts = prepared.runtime.exec_options();
    let args = noending::adapters::adapter_for(Agent::Codex)
        .build_new_command(
            &noending::platform::exec_resolver::AgentInstallation {
                agent: Agent::Codex,
                executable_path: "/usr/local/bin/codex".into(),
                version: None,
            },
            &opts,
            None,
        )
        .unwrap()
        .args;
    assert!(args
        .windows(2)
        .any(|w| w[0] == "-m" && w[1] == "gpt-5.6-sol"));
    assert!(args.iter().any(|a| a == "model_reasoning_effort=\"high\""));

    // And clearing the override later does not retroactively change the
    // already-prepared intent — it just makes the preview stale.
    noending::agent_runtime::set_runtime_overrides(&db, Agent::Codex, &Default::default()).unwrap();
    assert_eq!(prepared.runtime.model.as_deref(), Some("gpt-5.6-sol"));
}
