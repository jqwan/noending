//! Base Experience launch tests: the simplified New / Resume flow, where New
//! Session always runs `prepare_new_in → launch_prepared` (the UI-side
//! direct-launch path is gone), even standalone with zero Workstreams. A
//! launch carries launch facts only (Agent, cwd, runtime, Owner, LaunchIntent)
//! — never a Context sync, a Context file or an injection. Preparation commits
//! nothing, launching commits exactly one single-use LaunchIntent, and the
//! spawn step is injected via `launch_prepared_with_in` so no Terminal opens.

use rusqlite::Connection;
mod support;

use noending::adapters::adapter_for;
use noending::adapters::{AgentCommand, DesktopResume, ResumeRoute};
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
fn fake_open(_uri: &str) -> Result<()> {
    Ok(())
}

fn fake_spawn(_cmd: &AgentCommand) -> Result<LaunchOutcome> {
    Ok(LaunchOutcome {
        launched_via: FAKE_SPAWN_TAG.into(),
        command_line: "test spawn — no process started".into(),
        pid: Some(4242),
        terminal_id: None,
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
        .launch_prepared_with_in(
            &db,
            &prepared,
            &LaunchWorkspace::default(),
            fake_spawn,
            fake_open,
            None,
        )
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

/// An embedded NEW launch goes through the same seam with an UNBOUND
/// terminal: the LaunchResult carries the terminal id, the LaunchIntent is
/// still committed for attribution, and the target's session_id is None —
/// the registry entry starts unbound and ingestion binds it later.
#[test]
fn an_embedded_new_launch_spawns_unbound_and_still_commits_the_intent() {
    let db = open_db("embedded-new");
    seed_installation(&db, Agent::Codex);
    let launcher = launcher_in("embedded-new");

    let prepared = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            None,
            Some("/tmp"),
            &LaunchWorkspace::default(),
        )
        .expect("prepare");
    let mut prepared = prepared;
    prepared.embedded = true;

    let result = launcher
        .launch_prepared_with_in(
            &db,
            &prepared,
            &LaunchWorkspace::default(),
            fake_spawn,
            fake_open,
            Some(&noending::launcher::EmbeddedSpawn {
                registry: &noending::terminal::TerminalRegistry::new(None),
                spawn: fake_embedded_spawn,
            }),
        )
        .expect("embedded new launch must succeed");
    assert_eq!(result.launched_via, "test-embedded-spawn");
    assert_eq!(result.terminal_id.as_deref(), Some("term-test-1"));
    assert!(
        result.launch_intent_id.is_some(),
        "attribution still rides the LaunchIntent"
    );

    let intents: Vec<LaunchIntent> = db.list_launch_intents(&[], 100).unwrap();
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].status, noending::domain::launch_status::PENDING);
}

const FIRST_MESSAGE: &str = "  请开始实现\n$(id) 'literal' \"message\"  ";

fn capture_embedded_new_message(
    cmd: &AgentCommand,
    target: &noending::terminal::EmbeddedTarget,
) -> Result<LaunchOutcome> {
    assert!(target.session_id.is_none());
    assert_eq!(
        target.root_agent_session_id.is_some(),
        matches!(target.agent, Agent::ClaudeCode | Agent::Pi)
    );
    assert_eq!(cmd.args.last().map(String::as_str), Some(FIRST_MESSAGE));
    let expected_flag = if target.agent == Agent::Antigravity {
        "--prompt-interactive"
    } else {
        "--"
    };
    assert_eq!(cmd.args[cmd.args.len() - 2], expected_flag);
    Ok(LaunchOutcome {
        launched_via: "captured-embedded-new".into(),
        command_line: cmd.display(),
        pid: Some(77),
        terminal_id: Some("term-first-message".into()),
    })
}

#[test]
fn embedded_new_launch_passes_the_first_user_message_to_each_cli() {
    for agent in [
        Agent::Codex,
        Agent::ClaudeCode,
        Agent::Pi,
        Agent::Antigravity,
    ] {
        let db = open_db(&format!("first-message-{}", agent.as_str()));
        seed_installation(&db, agent);
        let launcher = launcher_in("first-message");
        let workspace = LaunchWorkspace::default();
        let mut prepared = launcher
            .prepare_new_in(&db, agent, None, None, &workspace)
            .unwrap();
        assert!(prepared.initial_message.is_none());
        prepared.embedded = true;
        prepared.initial_message = Some(FIRST_MESSAGE.into());

        let registry = noending::terminal::TerminalRegistry::new(None);
        let result = launcher
            .launch_prepared_with_in(
                &db,
                &prepared,
                &workspace,
                fake_spawn,
                fake_open,
                Some(&noending::launcher::EmbeddedSpawn {
                    registry: &registry,
                    spawn: capture_embedded_new_message,
                }),
            )
            .unwrap();
        assert!(result.command_line.contains("[首条消息]"), "{agent:?}");
        assert!(!result.command_line.contains("请开始实现"), "{agent:?}");
        assert!(!result.command_line.contains("$(id)"), "{agent:?}");
        assert_eq!(result.terminal_id.as_deref(), Some("term-first-message"));
        assert_eq!(count(&db, "SELECT COUNT(*) FROM launch_intents"), 1);
    }
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
        .launch_prepared_with_in(
            &db,
            &prepared,
            &LaunchWorkspace::default(),
            fake_spawn,
            fake_open,
            None,
        )
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
        .launch_prepared_with_in(
            &db,
            &held,
            &LaunchWorkspace::default(),
            fake_spawn,
            fake_open,
            None,
        )
        .expect("launch with the consumed token");
    assert_eq!(held_launch.launched_via, FAKE_SPAWN_TAG);

    assert!(
        consume_once(&map, &id).is_err(),
        "a consumed prepared launch must never be consumable again"
    );
    assert_eq!(count(&db, "SELECT COUNT(*) FROM launch_intents"), 1);
}

/// The cwd the New Session form shows is the backend's resolution, not a
/// frontend guess: the explicitly selected directory is what `prepare_new`
/// reports, together with the tier that produced it.
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
            Some(&primary),
            &LaunchWorkspace::default(),
        )
        .unwrap();
    assert_eq!(
        with_ws.cwd.as_deref(),
        Some(primary.as_str()),
        "the form displays exactly what prepare resolved"
    );
    assert_eq!(with_ws.cwd_resolution.source, CwdSource::Explicit);
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

/// The Continue route follows the ROOT member's SOURCE FORMAT, not the
/// agent: a desktop-only agent routes to its app (with the app present) and
/// refuses loudly when the app is absent, while a CLI agent keeps the
/// terminal route with `desktop_open` unset.
/// continue_session_desktop asks `desktop_resume_route` — its default derives
/// the Desktop variant from `continue_route`, so Antigravity's IDE store (no
/// override of its own) still opens the desktop app instead of refusing.
#[test]
fn antigravity_desktop_sessions_open_the_desktop_app() {
    let db = open_db("agy-desktop-open");
    let launcher = launcher_in("agy-desktop-open");
    let workspace = LaunchWorkspace::default();

    let raw = std::env::temp_dir().join(format!("noending-agy-{}.jsonl", new_id()));
    std::fs::write(&raw, "").unwrap();
    let ts = now();
    let (sid, _) = db
        .upsert_logical_session_unchecked(
            Agent::Antigravity,
            "agy-ide-root-1",
            Some("AGY"),
            None,
            None,
            None,
            Some(&ts),
            Some(&ts),
        )
        .unwrap();
    support::ensure_session_source(
        &db,
        Agent::Antigravity,
        "agy-ide-root-1",
        &raw.to_string_lossy(),
    );
    // The IDE store kind decides the route: this is the desktop half.
    db.write()
        .execute(
            "UPDATE sessions SET source_kind = 'antigravity_ide_conversation' WHERE root_agent_session_id = 'agy-ide-root-1'",
            [],
        )
        .unwrap();

    let prepared = launcher
        .prepare_resume_in(&db, &sid, &workspace)
        .expect("prepare");
    let adapter = noending::adapters::adapter_for(Agent::Antigravity);
    let session = db.get_session(&sid).unwrap().unwrap();
    match adapter.desktop_resume_route(&session) {
        noending::adapters::ResumeRoute::Desktop(open) => {
            assert_eq!(open.uri, "antigravity://");
        }
        other => panic!("expected the desktop route, got {other:?}"),
    }
    // prepare itself stays a terminal-route plan (embedded rides it); the
    // desktop answer lives on the adapter, consumed by continue_session_desktop.
    let _ = prepared;
}

#[test]
fn the_continue_route_follows_the_source_format() {
    let db = open_db("continue-route-format");
    let launcher = launcher_in("continue-route-format");
    let workspace = LaunchWorkspace::default();

    // WorkBuddy: desktop-only. The root member's source file must exist —
    // source presence is the resume gate ahead of the route decision.
    let raw = std::env::temp_dir().join(format!("noending-wb-{}.jsonl", new_id()));
    std::fs::write(&raw, "").unwrap();
    let ts = now();
    let (wb_id, _) = db
        .upsert_logical_session_unchecked(
            Agent::WorkBuddy,
            "wb-root-1",
            Some("WB"),
            None,
            None,
            None,
            Some(&ts),
            Some(&ts),
        )
        .unwrap();
    support::ensure_session_source(&db, Agent::WorkBuddy, "wb-root-1", &raw.to_string_lossy());

    match launcher.prepare_resume_in(&db, &wb_id, &workspace) {
        Ok(prepared) => {
            // App present on this machine: the desktop route is prepared.
            assert!(
                prepared.desktop_open.is_some(),
                "expected the desktop route"
            );
            assert!(
                prepared
                    .desktop_open
                    .as_ref()
                    .unwrap()
                    .uri
                    .starts_with("workbuddy://chat/"),
                "workbuddy continue must deep-link the conversation"
            );
        }
        Err(e) => {
            // App absent (CI): the refusal must say so — never a silent
            // fall-through to a CLI that does not exist.
            assert!(e.to_string().contains("未找到 WorkBuddy"), "{e}");
        }
    }

    // Codex: CLI route — `desktop_open` stays unset whatever the machine.
    let raw = std::env::temp_dir().join(format!("noending-cx-{}.jsonl", new_id()));
    std::fs::write(&raw, "").unwrap();
    let (cx_id, _) = db
        .upsert_logical_session_unchecked(
            Agent::Codex,
            "cx-root-1",
            Some("CX"),
            None,
            None,
            None,
            Some(&ts),
            Some(&ts),
        )
        .unwrap();
    support::ensure_session_source(&db, Agent::Codex, "cx-root-1", &raw.to_string_lossy());
    let prepared = launcher
        .prepare_resume_in(&db, &cx_id, &workspace)
        .expect("codex resume prepares through the terminal route");
    assert!(
        prepared.desktop_open.is_none(),
        "codex continues in the terminal"
    );
}

/// Launch honors the frozen desktop route: the URI goes through the
/// injected open, the terminal spawn never runs, no CLI installation is
/// required, and no LaunchIntent is committed (nothing new was launched).
#[test]
fn a_desktop_open_resume_dispatches_the_uri_instead_of_the_terminal() {
    let db = open_db("desktop-open-resume");
    seed_installation(&db, Agent::Codex);
    let launcher = launcher_in("desktop-open-resume");
    let workspace = LaunchWorkspace::default();

    let raw = std::env::temp_dir().join(format!("noending-dx-{}.jsonl", new_id()));
    std::fs::write(&raw, "").unwrap();
    let ts = now();
    let (sid, _) = db
        .upsert_logical_session_unchecked(
            Agent::Codex,
            "dx-root-1",
            Some("DX"),
            None,
            None,
            None,
            Some(&ts),
            Some(&ts),
        )
        .unwrap();
    support::ensure_session_source(&db, Agent::Codex, "dx-root-1", &raw.to_string_lossy());

    let mut prepared = launcher
        .prepare_resume_in(&db, &sid, &workspace)
        .expect("prepare");
    assert!(prepared.desktop_open.is_none());
    prepared.desktop_open = Some(DesktopResume {
        uri: "workbuddy://chat/test-session".into(),
        note: "test note".into(),
    });

    let result = launcher
        .launch_prepared_with_in(&db, &prepared, &workspace, fake_spawn, fake_open, None)
        .expect("desktop-open launch");
    assert_eq!(result.launched_via, "desktop app");
    assert_eq!(result.command_line, "workbuddy://chat/test-session");
    assert_eq!(result.note, "test note");
    assert!(
        result.launch_intent_id.is_none(),
        "a desktop open resumes an existing session; it launches nothing new"
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM launch_intents"),
        0,
        "no LaunchIntent without a terminal launch"
    );
}

/// The antigravity adapter keys its route on the SOURCE FORMAT: the CLI
/// store is terminal-resumable via `agy --conversation`; the IDE store is
/// app-only (and refuses when the app is not installed).
#[test]
fn antigravity_routes_by_source_format() {
    let session = |kind: &str, path: &str| noending::domain::Session {
        id: "s".into(),
        agent: Agent::Antigravity,
        root_agent_session_id: "conv-1".into(),
        title: None,
        owner_workstream_id: None,
        cwd: None,
        workspace_path_id: None,
        project_id: None,
        forked_from_session_id: None,
        started_at: None,
        last_activity_at: None,
        last_conversation_at: None,
        archived_at: None,
        source_kind: kind.into(),
        source_path: path.into(),
        metadata: serde_json::json!({}),
        source_file_identity: String::new(),
        source_generation: 0,
        source_byte_offset: 0,
        source_last_seen_size: 0,
        source_mtime: None,
        source_prefix_hash: String::new(),
        source_tail_hash: String::new(),
        fact_generation: 0,
        latest_message_seq: 0,
    };
    let adapter = adapter_for(Agent::Antigravity);

    assert!(matches!(
        adapter.continue_route(&session(
            "antigravity_cli_conversation",
            "/home/x/.gemini/antigravity-cli/conversations/a.db"
        )),
        ResumeRoute::Terminal
    ));

    let ide = adapter.continue_route(&session(
        "antigravity_ide_conversation",
        "/home/x/.gemini/antigravity/conversations/a.db",
    ));
    if noending::platform::paths::app_bundle_present("Antigravity") {
        assert!(matches!(ide, ResumeRoute::Desktop(_)));
    } else {
        assert!(matches!(ide, ResumeRoute::Refused(_)));
    }
}

// ---------------- Embedded terminal resume ----------------

/// Records nothing (fn pointers cannot capture); its distinct tag and the
/// frozen terminal id are the assertions: an embedded launch must come out
/// of THIS seam, never the external one.
fn fake_embedded_spawn(
    _cmd: &AgentCommand,
    target: &noending::terminal::EmbeddedTarget,
) -> Result<LaunchOutcome> {
    if let Some(session_id) = target.session_id {
        assert!(target.root_agent_session_id.is_some());
        assert!(
            target.registry.reserve_resume(session_id).is_err(),
            "the launcher must hold the resume reservation throughout spawn"
        );
    }
    Ok(LaunchOutcome {
        launched_via: "test-embedded-spawn".into(),
        command_line: format!("embedded session={:?}", target.session_id),
        pid: Some(77),
        terminal_id: Some("term-test-1".into()),
    })
}

fn codex_resume_session(db: &Db, tag: &str) -> String {
    let raw = std::env::temp_dir().join(format!("noending-emb-{}-{}.jsonl", tag, new_id()));
    std::fs::write(&raw, "").unwrap();
    let ts = now();
    let (sid, _) = db
        .upsert_logical_session_unchecked(
            Agent::Codex,
            &format!("emb-root-{tag}"),
            Some("EMB"),
            None,
            None,
            None,
            Some(&ts),
            Some(&ts),
        )
        .unwrap();
    support::ensure_session_source(
        db,
        Agent::Codex,
        &format!("emb-root-{tag}"),
        &raw.to_string_lossy(),
    );
    sid
}

#[test]
fn an_embedded_resume_launch_spawns_through_the_embedded_seam() {
    let db = open_db("embedded-resume");
    seed_installation(&db, Agent::Codex);
    let launcher = launcher_in("embedded-resume");
    let workspace = LaunchWorkspace::default();
    let sid = codex_resume_session(&db, "launch");

    let mut prepared = launcher
        .prepare_resume_in(&db, &sid, &workspace)
        .expect("prepare");
    // The command layer (launch_embedded_resume) forces this flag — the
    // launcher-level prepare resolves only the route; the test freezes it
    // the same way.
    prepared.embedded = true;
    assert!(prepared.desktop_open.is_none());

    let registry = noending::terminal::TerminalRegistry::new(None);
    let embedded = noending::launcher::EmbeddedSpawn {
        registry: &registry,
        spawn: fake_embedded_spawn,
    };
    let result = launcher
        .launch_prepared_with_in(
            &db,
            &prepared,
            &workspace,
            fake_spawn,
            fake_open,
            Some(&embedded),
        )
        .expect("embedded prepared launch");
    assert_eq!(result.launched_via, "test-embedded-spawn");
    assert_eq!(result.terminal_id.as_deref(), Some("term-test-1"));
    assert!(result.note.contains("内嵌终端"));
    // Embedded launches nothing new in the ingestion sense: no LaunchIntent.
    assert!(result.launch_intent_id.is_none());
    assert!(
        registry.reserve_resume(&sid).is_ok(),
        "the reservation is released after the injected spawn returns"
    );
}

fn unexpected_embedded_spawn(
    _cmd: &AgentCommand,
    _target: &noending::terminal::EmbeddedTarget,
) -> Result<LaunchOutcome> {
    panic!("a pending resume must be refused before the spawn step");
}

#[test]
fn an_embedded_resume_launch_refuses_an_existing_reservation_before_spawn() {
    let db = open_db("embedded-resume-pending");
    seed_installation(&db, Agent::Codex);
    let launcher = launcher_in("embedded-resume-pending");
    let workspace = LaunchWorkspace::default();
    let sid = codex_resume_session(&db, "pending");
    let mut prepared = launcher.prepare_resume_in(&db, &sid, &workspace).unwrap();
    prepared.embedded = true;

    let registry = noending::terminal::TerminalRegistry::new(None);
    let _existing = registry.reserve_resume(&sid).unwrap();
    let expected_error = registry.reserve_resume(&sid).err().unwrap();
    let error = launcher
        .launch_prepared_with_in(
            &db,
            &prepared,
            &workspace,
            fake_spawn,
            fake_open,
            Some(&noending::launcher::EmbeddedSpawn {
                registry: &registry,
                spawn: unexpected_embedded_spawn,
            }),
        )
        .expect_err("concurrent resume must be refused");
    assert_eq!(error.to_string(), expected_error);
}

fn failed_embedded_spawn(
    _cmd: &AgentCommand,
    target: &noending::terminal::EmbeddedTarget,
) -> Result<LaunchOutcome> {
    assert!(target
        .registry
        .reserve_resume(target.session_id.unwrap())
        .is_err());
    Err(noending::error::other("test spawn failed"))
}

#[test]
fn an_embedded_resume_launch_releases_its_reservation_after_spawn_failure() {
    let db = open_db("embedded-resume-failed");
    seed_installation(&db, Agent::Codex);
    let launcher = launcher_in("embedded-resume-failed");
    let workspace = LaunchWorkspace::default();
    let sid = codex_resume_session(&db, "failed");
    let mut prepared = launcher.prepare_resume_in(&db, &sid, &workspace).unwrap();
    prepared.embedded = true;

    let registry = noending::terminal::TerminalRegistry::new(None);
    let error = launcher
        .launch_prepared_with_in(
            &db,
            &prepared,
            &workspace,
            fake_spawn,
            fake_open,
            Some(&noending::launcher::EmbeddedSpawn {
                registry: &registry,
                spawn: failed_embedded_spawn,
            }),
        )
        .expect_err("spawn fails");
    assert_eq!(error.to_string(), "test spawn failed");
    assert!(
        registry.reserve_resume(&sid).is_ok(),
        "a failed spawn must not leave the session reserved"
    );
}

#[test]
fn an_external_resume_does_not_consume_an_embedded_reservation() {
    let db = open_db("external-resume-pending");
    seed_installation(&db, Agent::Codex);
    let launcher = launcher_in("external-resume-pending");
    let workspace = LaunchWorkspace::default();
    let sid = codex_resume_session(&db, "external");
    let prepared = launcher.prepare_resume_in(&db, &sid, &workspace).unwrap();
    assert!(!prepared.embedded);

    let registry = noending::terminal::TerminalRegistry::new(None);
    let _existing = registry.reserve_resume(&sid).unwrap();
    let result = launcher
        .launch_prepared_with_in(
            &db,
            &prepared,
            &workspace,
            fake_spawn,
            fake_open,
            Some(&noending::launcher::EmbeddedSpawn {
                registry: &registry,
                spawn: unexpected_embedded_spawn,
            }),
        )
        .expect("external resumes do not use the embedded registry");
    assert_eq!(result.launched_via, FAKE_SPAWN_TAG);
    assert!(registry.reserve_resume(&sid).is_err());
}

#[test]
fn an_embedded_prepared_launch_without_a_surface_is_refused_not_downgraded() {
    let db = open_db("embedded-refused");
    seed_installation(&db, Agent::Codex);
    let launcher = launcher_in("embedded-refused");
    let workspace = LaunchWorkspace::default();
    let sid = codex_resume_session(&db, "refuse");

    let mut prepared = launcher
        .prepare_resume_in(&db, &sid, &workspace)
        .expect("prepare");
    prepared.embedded = true;

    let err = launcher
        .launch_prepared_with_in(&db, &prepared, &workspace, fake_spawn, fake_open, None)
        .expect_err("embedded without a registry must refuse");
    assert!(err.to_string().contains("内嵌终端"));
}
