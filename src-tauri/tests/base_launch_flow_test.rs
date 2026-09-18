//! Base Experience launch tests (方案 v0.1 §15).
//!
//! These cover the invariants the simplified New / Resume flow leans on, because
//! the UI-side direct-launch path is gone: New Session now always runs
//! `prepare_new → launch_prepared`, including the standalone case with ZERO
//! Workstreams. What must stay true while Context Delivery is Off (the shipped
//! default):
//!
//! * a standalone prepared launch still succeeds (0 bindings is legal);
//! * it never writes a context file, never captures bundle markdown, and never
//!   advances a `context_deliveries` snapshot (AGENTS.md: delivery snapshots
//!   advance only when context was actually delivered);
//! * flipping `context.delivery_level` after Preview makes the prepared launch
//!   stale and aborts the launch instead of injecting what the user never saw.
//!
//! The process-spawn step is injected through
//! [`SessionLauncher::launch_prepared_with`] so no real Terminal opens.

use noending::adapters::AgentCommand;
use noending::context::{self, ContextDeliveryLevel};
use noending::domain::{Agent, LaunchIntent};
use noending::error::Result;
use noending::launcher::{PreparedLaunch, SessionLauncher};
use noending::platform::exec_resolver::AgentInstallation;
use noending::platform::launcher::LaunchOutcome;
use noending::settings;
use noending::storage::{new_id, now, Db};

fn open_db(tag: &str) -> Db {
    let dir = std::env::temp_dir().join(format!("noending-base-launch-{}-{}", tag, new_id()));
    Db::open(&dir.join("test.db")).unwrap()
}

fn workstream_with_cwd(
    db: &Db,
    title: &str,
    default_cwd: Option<&str>,
) -> noending::domain::Workstream {
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
    db.upsert_workstream(&w).unwrap();
    w
}

/// A real Context item, so an "injected" launch would have something to deliver.
fn seed_context(db: &Db, ws_id: &str, titles: &[&str]) {
    for t in titles {
        noending::sync::create_item(
            db,
            ws_id,
            "constraint",
            t,
            &format!("content of {}", t),
            "user_edit",
            "user_edit",
            &[],
            None,
            "user",
        )
        .unwrap();
    }
}

fn count(db: &Db, sql: &str) -> i64 {
    db.conn()
        .query_row(sql, [], |r| r.get::<_, i64>(0))
        .unwrap()
}

/// Point the resolver at a binary that certainly exists so `resolve_install`
/// never touches PATH (`current_exe` is real on macOS, Windows and Linux).
fn seed_installation(db: &Db, agent: Agent) {
    db.save_installation(&AgentInstallation {
        agent,
        executable_path: std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .to_string(),
        version: Some("test".into()),
        source: "test".into(),
        last_verified_at: now(),
    })
    .unwrap();
}

/// Records nothing: tests assert on `LaunchResult::launched_via`, which proves
/// the spawn step was reached *and* that no real Terminal was opened
/// (`crate::platform::launcher::launch` would report "Terminal"/"PowerShell").
/// A shared counter would race across the parallel test threads.
fn fake_spawn(_cmd: &AgentCommand) -> Result<LaunchOutcome> {
    Ok(LaunchOutcome {
        launched_via: FAKE_SPAWN_TAG.into(),
        command_line: "test spawn — no process started".into(),
        pid: Some(4242),
    })
}

const FAKE_SPAWN_TAG: &str = "test-spawn";

fn launcher_in(dir_tag: &str) -> SessionLauncher {
    SessionLauncher {
        app_data_dir: std::env::temp_dir()
            .join(format!("noending-base-launch-appdata-{}", dir_tag)),
    }
}

/// The single-use guarantee lives in the command layer, in front of `launch_prepared`.
fn consume_once(
    map: &std::sync::Mutex<std::collections::HashMap<String, PreparedLaunch>>,
    id: &str,
) -> Result<PreparedLaunch> {
    noending::commands::consume_prepared_launch(map, id)
}

#[test]
fn standalone_new_session_launches_through_the_prepared_flow() {
    let db = open_db("standalone-new");
    // Shipped default: delivery Off, and the flow must still work with 0 Workstreams.
    assert_eq!(
        settings::context_delivery_level_of(&db).unwrap(),
        ContextDeliveryLevel::Off
    );
    seed_installation(&db, Agent::Codex);
    let launcher = launcher_in("standalone-new");
    let bundle_dir = launcher.app_data_dir.join("context-bundles");
    let _ = std::fs::remove_dir_all(&bundle_dir);

    let prepared = launcher
        .prepare_new(&db, Agent::Codex, &[], None)
        .expect("standalone prepare");
    assert_eq!(prepared.mode, "new");
    assert!(prepared.workstream_ids.is_empty());
    assert_eq!(prepared.delivery_level, ContextDeliveryLevel::Off);
    assert!(prepared.bundle.sections.is_empty());
    assert_eq!(count(&db, "SELECT COUNT(*) FROM launch_intents"), 0);
    assert!(
        !bundle_dir.exists(),
        "prepare must not write a context file, even for a launch that will succeed"
    );

    let result = launcher
        .launch_prepared_with(&db, &prepared, fake_spawn)
        .expect("standalone prepared launch must succeed");
    assert_eq!(result.launched_via, FAKE_SPAWN_TAG);
    assert_eq!(result.context_file, "");
    assert!(result.bundle.sections.is_empty());

    // The user's explicit (empty) selection is still durably recorded for reconcile.
    let intents: Vec<LaunchIntent> = db.list_launch_intents(&[], 100).unwrap();
    assert_eq!(intents.len(), 1, "one launch = one LaunchIntent");
    assert_eq!(intents[0].launch_type, "new");
    assert!(
        intents[0].selected_workstream_ids.is_empty(),
        "standalone launch records a zero-binding selection, never a guessed one"
    );
    assert!(
        intents[0].context_bundle_markdown.is_none(),
        "nothing was delivered, so the intent must not claim a bundle snapshot"
    );
    assert!(intents[0].context_bundle_revisions.is_none());

    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM context_deliveries"),
        0,
        "a launch that delivered no context must not advance any ContextDelivery"
    );
    assert!(!bundle_dir.exists(), "no context file may be written");
}

/// Delivery Off with a real Workstream that HAS context: the launch still
/// succeeds, and still injects nothing — the gate is the level, not the data.
#[test]
fn off_launch_with_bound_workstream_injects_nothing() {
    let db = open_db("off-with-workstream");
    seed_installation(&db, Agent::Codex);
    let ws = workstream_with_cwd(&db, "injected ws", None);
    seed_context(&db, &ws.id, &["约束 A", "约束 B"]);
    let launcher = launcher_in("off-with-workstream");
    let bundle_dir = launcher.app_data_dir.join("context-bundles");
    let _ = std::fs::remove_dir_all(&bundle_dir);

    let prepared = launcher
        .prepare_new(&db, Agent::Codex, &[ws.id.clone()], None)
        .expect("prepare with workstream while off");
    assert!(
        prepared.bundle.sections.is_empty(),
        "Off must render an empty bundle, not a trimmed one"
    );

    let result = launcher
        .launch_prepared_with(&db, &prepared, fake_spawn)
        .expect("off launch must succeed");
    assert_eq!(result.launched_via, FAKE_SPAWN_TAG);
    assert_eq!(result.context_file, "");
    assert!(!bundle_dir.exists());
    assert_eq!(count(&db, "SELECT COUNT(*) FROM context_deliveries"), 0);
    let intents = db.list_launch_intents(&[], 100).unwrap();
    assert_eq!(intents[0].selected_workstream_ids, vec![ws.id.clone()]);
    assert!(intents[0].context_bundle_markdown.is_none());
}

/// The prepared launch captured `delivery_level` as part of its identity. If
/// Settings move while the modal is open, the launch is refused: the user must
/// see the new level before anything reaches the Agent.
#[test]
fn prepared_launch_goes_stale_when_delivery_level_leaves_off() {
    let db = open_db("stale-level-off-to-balanced");
    seed_installation(&db, Agent::Codex);
    let ws = workstream_with_cwd(&db, "stale ws", None);
    seed_context(&db, &ws.id, &["初始约束"]);
    let launcher = launcher_in("stale-level");

    let prepared = launcher
        .prepare_new(&db, Agent::Codex, &[ws.id.clone()], None)
        .unwrap();
    assert_eq!(prepared.delivery_level, ContextDeliveryLevel::Off);

    settings::set_context_delivery_level(&db, ContextDeliveryLevel::Balanced).unwrap();

    let err = launcher
        .launch_prepared(&db, &prepared)
        .expect_err("off → balanced must abort the prepared launch");
    assert!(
        err.to_string().contains("stale"),
        "expected a stale error, got: {}",
        err
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM launch_intents"),
        0,
        "a refused launch must not commit a LaunchIntent, so it never reaches the spawn step"
    );
    assert_eq!(count(&db, "SELECT COUNT(*) FROM context_deliveries"), 0);

    // Re-preparing under the new level works again.
    let fresh = launcher
        .prepare_new(&db, Agent::Codex, &[ws.id.clone()], None)
        .unwrap();
    assert_eq!(fresh.delivery_level, ContextDeliveryLevel::Balanced);
    let relaunch = launcher
        .launch_prepared_with(&db, &fresh, fake_spawn)
        .expect("a launch previewed at the current level proceeds");
    assert_eq!(relaunch.launched_via, FAKE_SPAWN_TAG);
    assert!(
        !relaunch.context_file.is_empty(),
        "Balanced with a real Context item must actually deliver one"
    );
}

/// Same gate, other direction: turning injection Off while a preview is open
/// must not let that preview deliver.
#[test]
fn prepared_launch_goes_stale_when_delivery_level_is_turned_off() {
    let db = open_db("stale-level-balanced-to-off");
    seed_installation(&db, Agent::Codex);
    settings::set_context_delivery_level(&db, ContextDeliveryLevel::Balanced).unwrap();
    let ws = workstream_with_cwd(&db, "stale ws 2", None);
    seed_context(&db, &ws.id, &["约束 A"]);
    let launcher = launcher_in("stale-level-reverse");

    let prepared = launcher
        .prepare_new(&db, Agent::Codex, &[ws.id.clone()], None)
        .unwrap();
    assert!(!prepared.bundle.sections.is_empty());

    settings::set_context_delivery_level(&db, ContextDeliveryLevel::Off).unwrap();
    let err = launcher
        .launch_prepared(&db, &prepared)
        .expect_err("balanced → off must abort too");
    assert!(err.to_string().contains("stale"), "got: {}", err);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM launch_intents"), 0);
}

/// The New modal consumes its capability on click; a second click (or a
/// concurrent one) must not launch a second Agent process.
#[test]
fn prepared_launch_capability_is_single_use() {
    let db = open_db("single-use-capability");
    seed_installation(&db, Agent::Codex);
    let launcher = launcher_in("single-use");

    let prepared = launcher.prepare_new(&db, Agent::Codex, &[], None).unwrap();
    let id = prepared.id.clone();
    let map: std::sync::Mutex<std::collections::HashMap<String, PreparedLaunch>> =
        Default::default();
    map.lock().unwrap().insert(id.clone(), prepared);

    let held = consume_once(&map, &id).expect("first consume");
    let held_launch = launcher
        .launch_prepared_with(&db, &held, fake_spawn)
        .expect("launch with the consumed token");
    assert_eq!(held_launch.launched_via, FAKE_SPAWN_TAG);

    assert!(
        consume_once(&map, &id).is_err(),
        "a consumed prepared launch must never be consumable again"
    );
    assert_eq!(count(&db, "SELECT COUNT(*) FROM launch_intents"), 1);
}

/// The cwd the New Session form shows is the backend's resolution, not a
/// frontend guess: Workstream `default_cwd` is what `prepare_new` reports.
#[test]
fn prepared_launch_reports_the_resolved_working_directory() {
    let db = open_db("prepared-cwd");
    seed_installation(&db, Agent::Codex);
    let ws = workstream_with_cwd(&db, "cwd ws", Some("/tmp/noending-example-cwd"));
    let launcher = launcher_in("prepared-cwd");

    let with_ws = launcher
        .prepare_new(&db, Agent::Codex, &[ws.id.clone()], None)
        .unwrap();
    assert_eq!(
        with_ws.cwd.as_deref(),
        Some("/tmp/noending-example-cwd"),
        "the form displays exactly what prepare resolved"
    );

    let standalone = launcher.prepare_new(&db, Agent::Codex, &[], None).unwrap();
    assert_eq!(
        standalone.cwd, None,
        "no Workstream and no history means no suggested directory"
    );
}

/// `context::build_bundle` must keep the empty-bundle contract the UI relies on
/// when it hides the Context Preview entirely: Off delivers literally nothing,
/// even for a Workstream full of Context items.
#[test]
fn off_bundle_is_literally_empty_even_with_context_items() {
    let db = open_db("off-empty-bundle");
    let ws = workstream_with_cwd(&db, "bundle ws", None);
    seed_context(&db, &ws.id, &["约束 A"]);
    let bundle = context::build_bundle(
        &db,
        "new",
        None,
        &[ws.id.clone()],
        ContextDeliveryLevel::Off,
    )
    .unwrap();
    assert!(bundle.sections.is_empty());
    assert_eq!(bundle.workstream_ids, vec![ws.id.clone()]);
    assert!(
        bundle.approx_tokens == 0 && bundle.markdown.is_empty(),
        "Off must deliver literally nothing"
    );
}
