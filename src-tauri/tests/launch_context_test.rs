//! LaunchIntent, storage/domain consistency, and prepared-launch identity tests
//! (Issues #5, #6, #8).
//!
//! Single-Owner model: a Session has at most one Owner Workstream, so an
//! intent's `owner_workstream_id` is inherited by the matched Session verbatim —
//! there are no primary/related roles or bundles. A launch carries launch facts
//! only (Agent, cwd, runtime, Owner, LaunchIntent) and never syncs or injects
//! Context, so there is no delivery level or snapshot to assert on.

use noending::domain::{
    launch_status, Agent, LaunchIntent, Session, SessionMessageRole, SourceCursorUpdate,
};
use noending::launcher;
use noending::launcher::LaunchWorkspace;
use noending::storage::{new_id, now, Db};

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
        lifecycle: "active".into(),
        visibility: "normal".into(),
        created_at: now(),
        updated_at: now(),
    };
    db.upsert_workstream(&w).unwrap();
    w
}

fn project_row(db: &Db, name: &str) -> noending::domain::Project {
    let p = noending::domain::Project {
        id: new_id(),
        name: name.into(),
        description: String::new(),
        git_id: None,
        name_customized: false,
        created_at: now(),
        updated_at: now(),
    };
    db.upsert_project(&p).unwrap();
    p
}

/// A Logical Session keyed by its ROOT member's Resume identity, with a REAL
/// root source file: resume preparation refuses a Session whose ROOT member
/// source is not present on disk, so every fixture session is
/// resumable.
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
    support::ensure_root_member(db, &id, agent, &root_id, &raw.to_string_lossy());
    db.get_session(&id).unwrap().unwrap()
}

/// A pending New Session intent carrying the user's single Owner selection.
fn pending_intent(db: &Db, agent: Agent, owner: Option<&str>, cwd: Option<String>) -> LaunchIntent {
    let i = LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent,
        owner_workstream_id: owner.map(str::to_string),
        cwd,
        process_id: None,
        launched_at: now(),
        matched_session_id: None,
        status: launch_status::PENDING.into(),
        note: String::new(),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_launch_intent(&i).unwrap();
    i
}

/// The Session's current Owner, straight from the store.
fn owner_of(db: &Db, session_id: &str) -> Option<String> {
    db.get_session(session_id)
        .unwrap()
        .unwrap()
        .owner_workstream_id
}

// Issue #5: LaunchIntent

/// Launch → discovery → Owner: a pending intent matches a newly discovered
/// session and the user's explicit Workstream selection becomes the Session's
/// Owner Workstream. This is also the crash-recovery path: the intent was
/// persisted before the "crash", the session is discovered by a later reconcile.
#[test]
fn pending_intent_matches_new_session_and_sets_owner() {
    let db = open_db("intent-match");
    let ws_a = ws_row(&db, "Workstream A");

    // user picks A in the New Session dialog, then NoEnding launches
    let intent = pending_intent(&db, Agent::Codex, Some(&ws_a.id), None);

    // the agent CLI creates its session; we discover it afterwards
    let session = session_row(&db, Agent::Codex, Some(now()), None);
    assert!(
        launcher::try_match_launch_intents_in(&db, &session, &LaunchWorkspace::default()).unwrap(),
        "the fresh session must claim the pending intent"
    );

    // the explicit selection becomes the single Owner Workstream
    assert_eq!(
        owner_of(&db, &session.id).as_deref(),
        Some(ws_a.id.as_str())
    );
    let owned = db.sessions_for_workstream(&ws_a.id).unwrap();
    assert_eq!(owned.len(), 1, "exactly one owned Session");
    assert_eq!(owned[0].id, session.id);

    let intent = db.get_launch_intent(&intent.id).unwrap().unwrap();
    assert_eq!(intent.status, launch_status::MATCHED);
    assert_eq!(
        intent.matched_session_id.as_deref(),
        Some(session.id.as_str())
    );
}

/// Zero selected workstreams is a valid state: the intent matches but the
/// Session stays unowned, and no classification adds an owner behind the
/// user's back.
#[test]
fn contextless_launch_matches_but_stays_unowned() {
    let db = open_db("intent-zero");
    let _ws = ws_row(&db, "Unrelated");
    let intent = pending_intent(&db, Agent::Pi, None, None);

    let session = session_row(&db, Agent::Pi, Some(now()), None);
    assert!(
        launcher::try_match_launch_intents_in(&db, &session, &LaunchWorkspace::default()).unwrap()
    );
    assert_eq!(owner_of(&db, &session.id), None);
    assert_eq!(
        db.get_launch_intent(&intent.id).unwrap().unwrap().status,
        launch_status::MATCHED
    );
}

/// Several similarly-plausible candidates → ambiguous, never a silent guess.
#[test]
fn ambiguous_candidates_wait_for_the_user() {
    let db = open_db("intent-ambiguous");
    let ws = ws_row(&db, "WS");
    // two intents launched at nearly the same time for the same agent
    let i1 = pending_intent(&db, Agent::ClaudeCode, Some(&ws.id), None);
    let i2 = pending_intent(&db, Agent::ClaudeCode, Some(&ws.id), None);
    let _ = (&i1, &i2);

    let session = session_row(&db, Agent::ClaudeCode, Some(now()), None);
    assert!(
        !launcher::try_match_launch_intents_in(&db, &session, &LaunchWorkspace::default()).unwrap(),
        "must not silently pick one"
    );
    let ambiguous = db
        .list_launch_intents(&[launch_status::AMBIGUOUS], 10)
        .unwrap();
    assert!(
        !ambiguous.is_empty(),
        "the best candidate is marked ambiguous"
    );
    assert_eq!(
        owner_of(&db, &session.id),
        None,
        "no owner before the user resolves"
    );

    // user resolves manually
    launcher::apply_match(
        &db,
        &ambiguous[0].id,
        &session,
        &launcher::LaunchWorkspace::default(),
    )
    .unwrap();
    let resolved = db.get_launch_intent(&ambiguous[0].id).unwrap().unwrap();
    assert_eq!(resolved.status, launch_status::MATCHED);
    assert_eq!(owner_of(&db, &session.id).as_deref(), Some(ws.id.as_str()));
}

/// Wrong agent / stale sessions never match; stale intents expire.
#[test]
fn stale_intents_expire_and_wrong_agent_never_matches() {
    let db = open_db("intent-expire");
    let ws = ws_row(&db, "WS");
    let intent = pending_intent(&db, Agent::Codex, Some(&ws.id), None);

    // a Claude session cannot claim a Codex intent
    let claude_session = session_row(&db, Agent::ClaudeCode, Some(now()), None);
    assert!(!launcher::try_match_launch_intents_in(
        &db,
        &claude_session,
        &LaunchWorkspace::default()
    )
    .unwrap());

    // expire pending intents that are older than the TTL
    db.tx(|tx| {
        noending::storage::update_waiting_launch_intent_conn(
            tx,
            &intent.id,
            launch_status::PENDING,
            "",
        )
    })
    .unwrap();
    db.write()
        .execute(
            "UPDATE launch_intents SET launched_at = ?2 WHERE id = ?1",
            rusqlite::params![
                intent.id,
                (chrono::Utc::now() - chrono::Duration::hours(48)).to_rfc3339()
            ],
        )
        .unwrap();
    let expired = launcher::expire_stale_launch_intents(&db).unwrap();
    assert_eq!(expired, 1);
    assert_eq!(
        db.get_launch_intent(&intent.id).unwrap().unwrap().status,
        launch_status::EXPIRED
    );

    // ...and an old session cannot claim a fresh intent either
    let old_session = session_row(
        &db,
        Agent::Codex,
        Some((chrono::Utc::now() - chrono::Duration::hours(48)).to_rfc3339()),
        None,
    );
    let fresh = pending_intent(&db, Agent::Codex, Some(&ws.id), None);
    assert!(
        !launcher::try_match_launch_intents_in(&db, &old_session, &LaunchWorkspace::default())
            .unwrap()
    );
    assert_eq!(
        db.get_launch_intent(&fresh.id).unwrap().unwrap().status,
        launch_status::PENDING
    );
}

/// Resume uses the stored Owner; discovery never re-derives it.
#[test]
fn resume_does_not_reguess_owner() {
    let db = open_db("resume-no-guess");
    let ws = ws_row(&db, "WS");
    let s = session_row(&db, Agent::Codex, Some(now()), None);
    db.set_session_owner(&s.id, Some(&ws.id)).unwrap();

    // No pending intent exists, so the discovery path has nothing to match:
    // the Owner is a stored fact and must survive untouched.
    assert!(!launcher::try_match_launch_intents_in(&db, &s, &LaunchWorkspace::default()).unwrap());
    assert_eq!(owner_of(&db, &s.id).as_deref(), Some(ws.id.as_str()));
}

// Issue #8: storage / domain consistency

fn seed_context(db: &Db, ws_id: &str, titles: &[&str]) -> Vec<noending::domain::ContextItem> {
    titles
        .iter()
        .map(|t| {
            noending::sync::create_item(
                db,
                ws_id,
                "constraint",
                t,
                &format!("content of {}", t),
                "user_edit",
                "user_edit",
                &[],
                "user",
            )
            .unwrap()
        })
        .collect()
}

/// The list_sessions parameter bug: filtering by agent ONLY used to bind
/// ?2 with a single parameter and fail. All four filter combinations work.
///
/// The Project half goes through the authoritative chain: the Session is
/// a member because its `workspace_path_id` says so, not because the derived
/// cache happens to hold a value.
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
    let root = db.root_member_for_session(&s.id).unwrap().unwrap();
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
        .commit_member_ingest(
            &s.id,
            &root.id,
            &[support::parsed_message(
                "src-1",
                SessionMessageRole::User,
                "ref target",
            )],
            None,
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
    // unknown ref → None, never fabricated
    assert!(db
        .get_message_by_ref("session-message:missing")
        .unwrap()
        .is_none());
}

/// A member cursor row is written by the ingest commit and read back through
/// `get_member_cursor`; the ingested conversation frontier rides separately.
#[test]
fn member_cursor_roundtrip() {
    let db = open_db("cursor-roundtrip");
    let s = session_row(&db, Agent::Codex, Some(now()), None);
    let root = db.root_member_for_session(&s.id).unwrap().unwrap();
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
        .commit_member_ingest(
            &s.id,
            &root.id,
            &[support::parsed_message(
                "m1",
                SessionMessageRole::User,
                "hello",
            )],
            None,
            &source,
        )
        .unwrap();
    assert_eq!(stored.len(), 1);

    let back = db.get_member_cursor(&root.id).unwrap();
    assert_eq!(back.member_id, root.id);
    assert_eq!(back.source_file_identity, "unix:dev:1:ino:42");
    assert_eq!(back.generation, 3);
    assert_eq!(back.byte_offset, 900);
    assert_eq!(back.last_seen_size, 1000);
    assert_eq!(back.mtime, Some(1234.5));
    assert_eq!(back.prefix_hash, "abc123");
    assert!(
        !back.identity_tail_hash.is_empty(),
        "a committed message advances the member cursor's identity tail"
    );
    assert_eq!(db.ingested_message_sequence(&s.id).unwrap(), 1);
}

// Prepared launch: side-effect contract & preview identity

/// `prepare_new` is a pure preview: it resolves the launch directory and freezes
/// the runtime intent, and writes nothing — no LaunchIntent, no file.
#[test]
fn prepare_new_does_not_create_intent_or_file() {
    let db = open_db("prep-new-no-side-effects");
    let ws = ws_row(&db, "test ws");

    let tmp_dir = std::env::temp_dir().join(format!("noending-launcher-{}", new_id()));
    let launcher = launcher::SessionLauncher {
        runtime_dir: tmp_dir.clone(),
    };

    let prepared = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            Some(ws.id.as_str()),
            None,
            &LaunchWorkspace::default(),
        )
        .unwrap();

    // 1. PreparedLaunch captures the launch facts & fingerprint
    assert_eq!(prepared.mode, "new");
    assert_eq!(
        prepared.owner_workstream_id.as_deref(),
        Some(ws.id.as_str())
    );
    assert!(!prepared.state_fingerprint.is_empty());

    // 2. INVARIANT: No LaunchIntent created
    let intents = db.list_launch_intents(&[], 100).unwrap();
    assert!(
        intents.is_empty(),
        "prepare_new must not insert LaunchIntent"
    );

    // 3. INVARIANT: nothing written to disk
    assert!(
        !tmp_dir.exists(),
        "prepare_new must not write any launch artifact"
    );
}

/// `prepare_resume` reads the Session's stored Owner and commits nothing: the
/// Owner survives untouched and no launch artifact is written.
#[test]
fn prepare_resume_uses_current_owner_without_writing() {
    let db = open_db("prep-resume-no-side-effects");
    let ws1 = ws_row(&db, "ws1");
    let ws2 = ws_row(&db, "ws2");

    let session = session_row(&db, Agent::Codex, Some(now()), None);
    // ws1 is the Session's current Owner
    db.set_session_owner(&session.id, Some(&ws1.id)).unwrap();

    let tmp_dir = std::env::temp_dir().join(format!("noending-launcher-{}", new_id()));
    let launcher = launcher::SessionLauncher {
        runtime_dir: tmp_dir.clone(),
    };

    let prepared = launcher
        .prepare_resume_in(&db, &session.id, &LaunchWorkspace::default())
        .unwrap();

    assert_eq!(prepared.mode, "resume");
    assert_eq!(
        prepared.owner_workstream_id.as_deref(),
        Some(ws1.id.as_str())
    );
    // ws2 is a different Workstream and is never adopted as the Owner.
    assert_ne!(
        prepared.owner_workstream_id.as_deref(),
        Some(ws2.id.as_str())
    );

    // INVARIANT: the Owner is unchanged (prepare only reads it)
    assert_eq!(owner_of(&db, &session.id).as_deref(), Some(ws1.id.as_str()));

    // INVARIANT: nothing written
    assert!(db.list_launch_intents(&[], 100).unwrap().is_empty());
    assert!(!tmp_dir.exists());
}

#[test]
fn state_fingerprint_stale_detection_on_context_change() {
    let db = open_db("stale-context-detection");
    let ws = ws_row(&db, "test ws");
    seed_context(&db, &ws.id, &["初始约束"]);

    let tmp_dir = std::env::temp_dir().join(format!("noending-launcher-{}", new_id()));
    let launcher = launcher::SessionLauncher {
        runtime_dir: tmp_dir,
    };

    let prepared = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            Some(ws.id.as_str()),
            None,
            &LaunchWorkspace::default(),
        )
        .unwrap();

    // Owner Context changes in the background (e.g. new item added)
    seed_context(&db, &ws.id, &["新插入的约束"]);

    // Attempting to launch with stale prepared launch MUST fail with stale error
    let err = launcher
        .launch_prepared_in(&db, &prepared, &LaunchWorkspace::default())
        .unwrap_err();
    assert!(
        err.to_string().contains("stale"),
        "expected stale error, got: {}",
        err
    );
}

/// Preview-Launch Identity covers the Runtime override intent too: an
/// override edited while the Preview is open must invalidate the preview,
/// never be silently adopted or silently ignored.
#[test]
fn state_fingerprint_stale_detection_on_runtime_override_change() {
    let db = open_db("stale-runtime-detection");
    let ws = ws_row(&db, "test ws");
    seed_context(&db, &ws.id, &["初始约束"]);

    let launcher = launcher::SessionLauncher {
        runtime_dir: std::env::temp_dir(),
    };
    let prepared = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            Some(ws.id.as_str()),
            None,
            &LaunchWorkspace::default(),
        )
        .unwrap();
    assert!(prepared.runtime.is_default());

    let fingerprint_of = |db: &Db| {
        launcher::compute_state_fingerprint(
            db,
            "new",
            None,
            prepared.owner_workstream_id.as_deref(),
            Agent::Codex,
            &LaunchWorkspace::default(),
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
        launcher::compute_state_fingerprint(
            &db,
            "new",
            None,
            prepared.owner_workstream_id.as_deref(),
            Agent::Codex,
            &LaunchWorkspace::default()
        )
        .unwrap()
    );
}

/// Prepare freezes the stored intent instead of resolving a default, so the
/// argv Launch renders is exactly what Preview described.
#[test]
fn prepared_launch_freezes_the_runtime_override_intent() {
    let db = open_db("prepared-runtime-intent");
    let ws = ws_row(&db, "test ws");
    seed_context(&db, &ws.id, &["约束"]);

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

    let launcher = launcher::SessionLauncher {
        runtime_dir: std::env::temp_dir(),
    };
    let prepared = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            Some(ws.id.as_str()),
            None,
            &LaunchWorkspace::default(),
        )
        .unwrap();
    assert_eq!(prepared.runtime.model.as_deref(), Some("gpt-5.6-sol"));
    assert_eq!(prepared.runtime.effort.as_deref(), Some("high"));
    assert_eq!(
        prepared.runtime.intent_summary(),
        "model=gpt-5.6-sol,effort=high"
    );

    // The frozen intent feeds the same conversion New / Resume / exec use.
    let opts = prepared.runtime.exec_options();
    let args = noending::adapters::adapter_for(Agent::Codex)
        .build_new_command(
            &noending::platform::exec_resolver::AgentInstallation {
                agent: Agent::Codex,
                executable_path: "/usr/local/bin/codex".into(),
                version: None,
                source: "test".into(),
                last_verified_at: String::new(),
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

#[test]
fn prepared_launch_single_use_atomic_consumption() {
    let db = open_db("single-use-prep");
    let ws = ws_row(&db, "test ws");
    let launcher = launcher::SessionLauncher {
        runtime_dir: std::env::temp_dir(),
    };
    let prepared = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            Some(ws.id.as_str()),
            None,
            &LaunchWorkspace::default(),
        )
        .unwrap();

    let map = std::sync::Mutex::new(std::collections::HashMap::new());
    map.lock()
        .unwrap()
        .insert(prepared.id.clone(), prepared.clone());

    // 1. First consume succeeds
    let first = noending::commands::consume_prepared_launch(&map, &prepared.id);
    assert!(first.is_ok());
    assert_eq!(first.unwrap().id, prepared.id);

    // 2. Second consume fails immediately with "已被使用或已过期"
    let second = noending::commands::consume_prepared_launch(&map, &prepared.id);
    assert!(second.is_err());
    let err_msg = second.unwrap_err().to_string();
    assert!(
        err_msg.contains("已过期") || err_msg.contains("已被使用"),
        "expected consumed error, got: {}",
        err_msg
    );
}

#[test]
fn prepared_launch_concurrent_consumption_is_exclusive() {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    let db = open_db("concurrent-prep");
    let ws = ws_row(&db, "test ws");
    let launcher = launcher::SessionLauncher {
        runtime_dir: std::env::temp_dir(),
    };
    let prepared = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            Some(ws.id.as_str()),
            None,
            &LaunchWorkspace::default(),
        )
        .unwrap();

    let map = Arc::new(Mutex::new(HashMap::new()));
    map.lock()
        .unwrap()
        .insert(prepared.id.clone(), prepared.clone());

    let num_threads = 8;
    let mut handles = Vec::new();

    for _ in 0..num_threads {
        let map_clone = Arc::clone(&map);
        let pid = prepared.id.clone();
        handles.push(std::thread::spawn(move || {
            noending::commands::consume_prepared_launch(&map_clone, &pid)
        }));
    }

    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let successes = results.iter().filter(|r| r.is_ok()).count();
    let failures = results.iter().filter(|r| r.is_err()).count();

    // INVARIANT: Exactly one consumer succeeds; all other 7 fail.
    assert_eq!(successes, 1, "exactly 1 thread must succeed in consuming");
    assert_eq!(failures, num_threads - 1, "all other threads must fail");
}

#[test]
fn prepared_launch_lazy_ttl_cleanup() {
    use std::collections::HashMap;
    use std::sync::Mutex;

    let db = open_db("ttl-cleanup-prep");
    let ws = ws_row(&db, "test ws");
    let launcher = launcher::SessionLauncher {
        runtime_dir: std::env::temp_dir(),
    };
    let mut stale_prepared = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            Some(ws.id.as_str()),
            None,
            &LaunchWorkspace::default(),
        )
        .unwrap();
    // Simulate an old timestamp: 35 minutes ago
    let old_ts = chrono::Utc::now() - chrono::Duration::seconds(35 * 60);
    stale_prepared.prepared_at = old_ts.to_rfc3339();

    let fresh_prepared = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            Some(ws.id.as_str()),
            None,
            &LaunchWorkspace::default(),
        )
        .unwrap();

    let map = Mutex::new(HashMap::new());
    {
        let mut guard = map.lock().unwrap();
        guard.insert(stale_prepared.id.clone(), stale_prepared.clone());
        guard.insert(fresh_prepared.id.clone(), fresh_prepared.clone());
    }

    // 1. Attempting to consume stale prepared fails because TTL pruned it
    let stale_res = noending::commands::consume_prepared_launch(&map, &stale_prepared.id);
    assert!(stale_res.is_err());

    // 2. Fresh prepared launch is still present and can be consumed
    let fresh_res = noending::commands::consume_prepared_launch(&map, &fresh_prepared.id);
    assert!(fresh_res.is_ok());
    assert_eq!(fresh_res.unwrap().id, fresh_prepared.id);
}
