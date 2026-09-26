//! Base Experience launch tests: the simplified New / Resume flow, where New
//! Session always runs `prepare_new_in → launch_prepared` (the UI-side
//! direct-launch path is gone), even standalone with zero Workstreams. A
//! launch carries launch facts only (Agent, cwd, runtime, Owner, LaunchIntent)
//! — never a Context sync, a Context file or an injection. Preparation commits
//! nothing, launching commits exactly one single-use LaunchIntent, and the
//! spawn step is injected via `launch_prepared_with_in` so no Terminal opens.

use rusqlite::Connection;
mod support;

use noending::adapters::AgentCommand;
use noending::domain::{Agent, LaunchIntent};
use noending::error::Result;
use noending::launcher::{CwdSource, LaunchWorkspace, PreparedLaunch, SessionLauncher};
use noending::platform::exec_resolver::AgentInstallation;
use noending::platform::launcher::LaunchOutcome;
use noending::storage::workspace::insert_workspace_path_conn;
use noending::storage::{new_id, now, Db};
use noending::workspace::{normalize_path, WorkspaceAttaching};

const PROJECT: &str = "p-base-launch";

fn open_db(tag: &str) -> Db {
    let dir = std::env::temp_dir().join(format!("noending-base-launch-{}-{}", tag, new_id()));
    let db = Db::open(&dir.join("test.db")).unwrap();
    db.upsert_project(&support::project(PROJECT.into(), "P"))
        .unwrap();
    db
}

/// A directory that exists — only launches into a usable one, and
/// note 7 says never compare a canonical path against a literal.
fn real_dir(tag: &str, name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("noending-base-launch-{}-{}", tag, new_id()));
    let dir = dir.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    normalize_path(&dir.to_string_lossy()).expect("fixture path normalizes")
}

/// Stand-in for `workspace::project`: lexical, no Git, writes through the
/// caller's connection (`WorkspaceAttaching`'s contract).
struct LexicalPaths;

impl WorkspaceAttaching for LexicalPaths {
    fn ensure_path(&self, conn: &Connection, raw: &str) -> Result<Option<String>> {
        Ok(match normalize_path(raw) {
            Some(canonical) => Some(insert_workspace_path_conn(conn, &canonical, PROJECT)?),
            None => None,
        })
    }
}

fn count(db: &Db, sql: &str) -> i64 {
    db.read()
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
        runtime_dir: std::env::temp_dir().join(format!("noending-base-launch-appdata-{}", dir_tag)),
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
    seed_installation(&db, Agent::Codex);
    let launcher = launcher_in("standalone-new");
    let bundle_dir = launcher.runtime_dir.join("context-bundles");
    let _ = std::fs::remove_dir_all(&bundle_dir);

    let prepared = launcher
        .prepare_new_in(&db, Agent::Codex, None, None, &LaunchWorkspace::default())
        .expect("standalone prepare");
    assert_eq!(prepared.mode, "new");
    assert!(prepared.owner_workstream_id.is_none());
    assert!(prepared.cwd.is_none());
    assert!(prepared.runtime.is_default());
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM launch_intents"),
        0,
        "prepare must not commit a LaunchIntent"
    );
    assert!(
        !bundle_dir.exists(),
        "prepare must not write a context file, even for a launch that will succeed"
    );

    let result = launcher
        .launch_prepared_with_in(&db, &prepared, &LaunchWorkspace::default(), fake_spawn)
        .expect("standalone prepared launch must succeed");
    assert_eq!(result.launched_via, FAKE_SPAWN_TAG);
    assert!(result.launch_intent_id.is_some());

    // The user's explicit (empty) selection is still durably recorded for reconcile.
    let intents: Vec<LaunchIntent> = db.list_launch_intents(&[], 100).unwrap();
    assert_eq!(intents.len(), 1, "one launch = one LaunchIntent");
    assert_eq!(intents[0].launch_type, "new");
    assert!(
        intents[0].owner_workstream_id.is_none(),
        "standalone launch records no Owner, never a guessed one"
    );

    assert!(!bundle_dir.exists(), "no context file may be written");
}

#[test]
fn bookkeeping_failure_after_spawn_keeps_the_successful_launch_result() {
    let db = open_db("post-spawn-bookkeeping-failure");
    seed_installation(&db, Agent::Codex);
    let launcher = launcher_in("post-spawn-bookkeeping-failure");
    let prepared = launcher
        .prepare_new_in(&db, Agent::Codex, None, None, &LaunchWorkspace::default())
        .unwrap();
    db.write()
        .execute_batch(
            "CREATE TRIGGER reject_launch_note BEFORE UPDATE OF note ON launch_intents
             BEGIN SELECT RAISE(ABORT, 'simulated bookkeeping failure'); END;",
        )
        .unwrap();

    let result = launcher
        .launch_prepared_with_in(&db, &prepared, &LaunchWorkspace::default(), fake_spawn)
        .expect("a successful OS spawn remains a successful launch");
    assert_eq!(result.launched_via, FAKE_SPAWN_TAG);
    assert!(result.launch_intent_id.is_some());
    let intents = db.list_launch_intents(&[], 10).unwrap();
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].note, "");
}

/// The New modal consumes its capability on click; a second click (or a
/// concurrent one) must not launch a second Agent process.
#[test]
fn prepared_launch_capability_is_single_use() {
    let db = open_db("single-use-capability");
    seed_installation(&db, Agent::Codex);
    let launcher = launcher_in("single-use");

    let prepared = launcher
        .prepare_new_in(&db, Agent::Codex, None, None, &LaunchWorkspace::default())
        .unwrap();
    let id = prepared.id.clone();
    let map: std::sync::Mutex<std::collections::HashMap<String, PreparedLaunch>> =
        Default::default();
    map.lock().unwrap().insert(id.clone(), prepared);

    let held = consume_once(&map, &id).expect("first consume");
    let held_launch = launcher
        .launch_prepared_with_in(&db, &held, &LaunchWorkspace::default(), fake_spawn)
        .expect("launch with the consumed token");
    assert_eq!(held_launch.launched_via, FAKE_SPAWN_TAG);

    assert!(
        consume_once(&map, &id).is_err(),
        "a consumed prepared launch must never be consumable again"
    );
    assert_eq!(count(&db, "SELECT COUNT(*) FROM launch_intents"), 1);
}

/// The cwd the New Session form shows is the backend's resolution, not a
/// frontend guess: the selected Workstream's primary `WorkstreamPath` is what
/// `prepare_new` reports, together with the tier that produced it.
#[test]
fn prepared_launch_reports_the_resolved_working_directory() {
    let db = open_db("prepared-cwd");
    seed_installation(&db, Agent::Codex);
    let primary = real_dir("prepared-cwd", "primary");
    let ws = noending::workspace::workstream::create_workstream(
        &db,
        &LexicalPaths,
        "cwd ws",
        "",
        &[primary.clone()],
    )
    .unwrap()
    .workstream;
    let launcher = launcher_in("prepared-cwd");

    let with_ws = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            Some(ws.id.as_str()),
            None,
            &LaunchWorkspace::default(),
        )
        .unwrap();
    assert_eq!(
        with_ws.cwd.as_deref(),
        Some(primary.as_str()),
        "the form displays exactly what prepare resolved"
    );
    assert_eq!(with_ws.cwd_resolution.source, CwdSource::WorkstreamPath);
    assert_eq!(with_ws.cwd_resolution.path_position, Some(0));
    assert!(!with_ws.cwd_resolution.fallback);

    let standalone = launcher
        .prepare_new_in(&db, Agent::Codex, None, None, &LaunchWorkspace::default())
        .unwrap();
    assert_eq!(
        standalone.cwd, None,
        "a launcher that was not given a Home knows no default workspace, so it \
         reports no directory rather than an implied one"
    );
    assert_eq!(standalone.cwd_resolution.source, CwdSource::Unresolved);
}

/// the same flow against a real Home: a standalone New Session starts in
/// NoEnding's default workspace, and the payload says so.
#[test]
fn standalone_prepared_launch_reports_the_default_workspace() {
    let db = open_db("prepared-default-ws");
    seed_installation(&db, Agent::Codex);
    let default_ws = std::env::temp_dir().join(format!("noending-default-ws-{}", new_id()));
    let default_ws = noending::workspace::normalize_path(&default_ws.to_string_lossy())
        .expect("temp path normalizes");
    let launcher = launcher_in("prepared-default-ws");

    let prepared = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            None,
            None,
            &LaunchWorkspace {
                default_workspace: Some(default_ws.clone()),
            },
        )
        .unwrap();
    assert_eq!(prepared.cwd.as_deref(), Some(default_ws.as_str()));
    assert_eq!(prepared.cwd_resolution.source, CwdSource::DefaultWorkspace,);
    assert!(
        std::fs::metadata(&default_ws)
            .map(|m| m.is_dir())
            .unwrap_or(false),
        "the default workspace is created rather than handed over missing"
    );
}
