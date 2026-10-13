//! Workspace Domain v0.2 launcher integration. Three pinned invariants:
//!
//! 1. The launch directory comes from the ordered `WorkstreamPath` list plus
//!    Home's default workspace — never from a frozen `workstreams.default_cwd`
//!    or another Session's cwd.
//! 2. Those inputs are in the PreparedLaunch fingerprint: a WorkstreamPath
//!    mutation does *not* bump `workstreams.updated_at`, so a reorder/add/remove
//!    could leave a stale preview looking fresh. Every stale test changes exactly
//!    ONE input and asserts the launch is refused.
//! 3. A resume launches in the directory the preview showed; a missing Session
//!    directory surfaces as fallback data in the payload, never a silent
//!    substitution ("发生 fallback 必须在 UI 明确显示").
//!
//! Everything is hermetic: temp DB/dirs, a `LaunchWorkspace` literal for Home,
//! and an injected spawn step (the fake echoes the directory it was handed).

use rusqlite::Connection;
mod support;

use noending::adapters::AgentCommand;
use noending::domain::{Agent, Session};
use noending::error::Result;
use noending::launcher::{CwdSource, LaunchWorkspace, SessionLauncher};
use noending::platform::exec_resolver::AgentInstallation;
use noending::platform::launcher::LaunchOutcome;
use noending::storage::workspace::insert_workspace_path_conn;
use noending::storage::{new_id, now, Db};
use noending::workspace::session::move_session_to_path_conn;
use noending::workspace::workstream::create_workstream;
use noending::workspace::{normalize_path, WorkspaceAttaching};

const PROJECT: &str = "p-launcher-ws";

// fixtures

fn open_db(tag: &str) -> Db {
    let dir = std::env::temp_dir().join(format!("noending-lws-{}-{}", tag, new_id()));
    let db = Db::open(&dir.join("test.db")).unwrap();
    db.upsert_project(&support::project(PROJECT.into(), "P"))
        .unwrap();
    db
}

/// Lexical, filesystem-free path identity: a fixture must hand the
/// production code the same spelling the production door would have stored, or
/// the assertion is a Unix-only assertion.
fn canonical(dir: &std::path::Path) -> String {
    normalize_path(&dir.to_string_lossy()).expect("fixture path normalizes")
}

fn temp_root(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("noending-lws-{}-{}", tag, new_id()))
}

/// A directory that exists: only launches into a usable one.
fn real_dir(tag: &str, name: &str) -> String {
    let dir = temp_root(tag).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    canonical(&dir)
}

/// A directory nobody created: the "the volume is not mounted" case.
fn missing_dir(tag: &str, name: &str) -> String {
    canonical(&temp_root(tag).join(name))
}

struct LexicalPaths;

impl WorkspaceAttaching for LexicalPaths {
    fn ensure_path(&self, conn: &Connection, raw: &str) -> Result<Option<String>> {
        Ok(match normalize_path(raw) {
            Some(canonical) => Some(insert_workspace_path_conn(conn, &canonical, PROJECT)?),
            None => None,
        })
    }
}

fn ws_with_paths(db: &Db, title: &str, dirs: &[String]) -> String {
    for dir in dirs {
        LexicalPaths.ensure_path(&db.write(), dir).unwrap();
    }
    let id = create_workstream(db, title, "", &[PROJECT.into()])
        .unwrap()
        .id;
    id
}

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

fn launcher_in(tag: &str) -> SessionLauncher {
    SessionLauncher {
        runtime_dir: temp_root(tag).join("runtime"),
    }
}

fn workspace(default_workspace: Option<&str>) -> LaunchWorkspace {
    LaunchWorkspace {
        default_workspace: default_workspace.map(|s| s.to_string()),
    }
}

fn count(db: &Db, sql: &str) -> i64 {
    db.read()
        .query_row(sql, [], |r| r.get::<_, i64>(0))
        .unwrap()
}

/// Echoes the directory it was asked to start in, so a test can prove *which*
/// cwd reached the OS. A `fn` pointer cannot capture, and a shared counter
/// would race across this binary's parallel test threads.
fn fake_open(_uri: &str) -> Result<()> {
    Ok(())
}

fn fake_spawn(cmd: &AgentCommand) -> Result<LaunchOutcome> {
    Ok(LaunchOutcome {
        launched_via: "test-spawn".into(),
        command_line: format!(
            "spawn cwd={}",
            cmd.cwd
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "<none>".into())
        ),
        pid: Some(4242),
        terminal_id: None,
    })
}

/// A Logical Session keyed by its root source's Resume identity, with a REAL
/// root source file: resume preparation refuses a Session whose root source
/// source is not present on disk, so every fixture session is
/// resumable. `workspace_path_id` goes through the production upsert, which
/// derives `project_id` from the path row in the same statement.
fn session_row(db: &Db, cwd: Option<&str>, workspace_path_id: Option<&str>) -> Session {
    let root_id = format!("as-{}", new_id());
    let raw = std::env::temp_dir().join(format!("noending-lws-raw-{}.jsonl", new_id()));
    std::fs::write(&raw, "").unwrap();
    let ts = now();
    let (id, _) = db
        .upsert_logical_session_unchecked(
            Agent::Codex,
            &root_id,
            None,
            cwd,
            workspace_path_id,
            None,
            Some(&ts),
            Some(&ts),
        )
        .unwrap();
    support::ensure_session_source(db, Agent::Codex, &root_id, &raw.to_string_lossy());
    db.get_session(&id).unwrap().unwrap()
}

fn is_stale(err: &str) -> bool {
    err.contains("stale")
}

// staleness of paths

/// The default workspace is part of the frozen launch: if Home moves between
/// preview and launch, the previewed directory is no longer the one to start.
#[test]
fn a_default_workspace_change_makes_a_prepared_launch_stale() {
    let db = open_db("default-ws");
    seed_installation(&db, Agent::Codex);
    let launcher = launcher_in("default-ws");
    let before = real_dir("default-ws", "home-a/workspace");
    let after = real_dir("default-ws", "home-b/workspace");

    let prepared = launcher
        .prepare_new_in(&db, Agent::Codex, None, None, &workspace(Some(&before)))
        .unwrap();
    assert_eq!(prepared.cwd.as_deref(), Some(before.as_str()));

    let err = launcher
        .launch_prepared_in(&db, &prepared, &workspace(Some(&after)), None)
        .expect_err("the Home moved: the previewed directory is not the one to launch");
    assert!(is_stale(&err.to_string()), "got: {err}");

    // Losing the default workspace entirely is a change too, never an accident.
    let err = launcher
        .launch_prepared_in(&db, &prepared, &LaunchWorkspace::default(), None)
        .expect_err("a launcher with no Home must not launch a Home-bound preview");
    assert!(is_stale(&err.to_string()), "got: {err}");

    let same = launcher
        .launch_prepared_with_in(
            &db,
            &prepared,
            &workspace(Some(&before)),
            fake_spawn,
            fake_open,
            None,
        )
        .expect("the unchanged Home still satisfies the preview");
    assert!(same.command_line.ends_with(&before));
}

// resume

/// "resume uses original cwd": the Session's own directory is tier 1 and it
/// reaches the OS.
#[test]
fn a_resume_launches_in_the_sessions_own_cwd() {
    let db = open_db("resume-cwd");
    seed_installation(&db, Agent::Codex);
    let home_dir = real_dir("resume-cwd", "session-home");
    let s = session_row(&db, Some(&home_dir), None);

    let launcher = launcher_in("resume-cwd");
    let prepared = launcher
        .prepare_resume_in(&db, &s.id, &LaunchWorkspace::default())
        .unwrap();
    assert_eq!(prepared.cwd.as_deref(), Some(home_dir.as_str()));
    assert_eq!(prepared.cwd_resolution.source, CwdSource::SessionCwd);
    assert!(!prepared.cwd_resolution.fallback);
    assert!(prepared.cwd_resolution.note.is_none());

    let result = launcher
        .launch_prepared_with_in(
            &db,
            &prepared,
            &LaunchWorkspace::default(),
            fake_spawn,
            fake_open,
            None,
        )
        .expect("a resume with its own directory launches");
    assert!(result.command_line.ends_with(&home_dir));
}

/// The preview's directory is authoritative and hashed, so background
/// discovery cannot move the Agent between preview and launch.
#[test]
fn a_cwd_drift_after_preview_makes_a_resume_plan_stale() {
    let db = open_db("resume-drift");
    seed_installation(&db, Agent::Codex);
    let original = real_dir("resume-drift", "original");
    let s = session_row(&db, Some(&original), None);

    let launcher = launcher_in("resume-drift");
    let prepared = launcher
        .prepare_resume_in(&db, &s.id, &LaunchWorkspace::default())
        .unwrap();

    // Discovery rewrites the cwd (the transcript is the source of truth)
    // through the production upsert, keyed by the ROOT Resume identity — and
    // it replaces the source columns wholesale, so they ride along.
    let moved = real_dir("resume-drift", "moved");
    let drifted = db.get_session(&s.id).unwrap().unwrap();
    db.upsert_logical_session(
        drifted.agent,
        &drifted.root_agent_session_id,
        None,
        None,
        Some(&moved),
        None,
        None,
        None,
        None,
        &drifted.source_kind,
        &drifted.source_path,
        &drifted.metadata,
    )
    .unwrap();

    let err = launcher
        .launch_prepared_in(&db, &prepared, &LaunchWorkspace::default(), None)
        .expect_err("a moved Session directory must abort, not silently follow");
    assert!(is_stale(&err.to_string()), "got: {err}");

    let fresh = launcher
        .prepare_resume_in(&db, &s.id, &LaunchWorkspace::default())
        .unwrap();
    assert_eq!(fresh.cwd.as_deref(), Some(moved.as_str()));
    let result = launcher
        .launch_prepared_with_in(
            &db,
            &fresh,
            &LaunchWorkspace::default(),
            fake_spawn,
            fake_open,
            None,
        )
        .expect("re-previewed under the new cwd the launch proceeds");
    assert!(result.command_line.ends_with(&moved));
}

/// "resume fallback visible": a Session whose directory is gone no longer
/// resolves to "no cwd, land in $HOME" — it falls back through, and the
/// payload says which tier and why.
#[test]
fn a_resume_fallback_is_recorded_in_the_prepared_payload() {
    let db = open_db("resume-fallback");
    seed_installation(&db, Agent::Codex);
    let lost = missing_dir("resume-fallback", "deleted-repo");
    let primary = real_dir("resume-fallback", "ws-primary");
    let s = session_row(&db, Some(&lost), None);
    let w = ws_with_paths(&db, "carries on", &[primary.clone()]);
    support::track(&db, &s.id, Some(&w)).unwrap();

    let workspace = LaunchWorkspace {
        default_workspace: Some(real_dir("resume-fallback", "default")),
    };
    let launcher = launcher_in("resume-fallback");
    let prepared = launcher.prepare_resume_in(&db, &s.id, &workspace).unwrap();

    assert_eq!(
        prepared.cwd.as_deref(),
        workspace.default_workspace.as_deref()
    );
    let resolution = &prepared.cwd_resolution;
    assert_eq!(resolution.source, CwdSource::DefaultWorkspace);
    assert!(
        resolution.fallback,
        "a resume that lost its directory is the case the UI must warn about"
    );
    let note = resolution.note.clone().unwrap_or_default();
    assert!(
        note.contains(&lost) || note.contains("不可用"),
        "the note must name what happened, got: {note}"
    );

    // And the Agent really starts there — `prepared.cwd`, not a re-read value.
    let result = launcher
        .launch_prepared_with_in(&db, &prepared, &workspace, fake_spawn, fake_open, None)
        .expect("a fallback is a legitimate launch");
    assert!(result
        .command_line
        .ends_with(workspace.default_workspace.as_ref().unwrap()));
}

/// "Session cwd/path change" is a stale input on the *identity* side as
/// well: the same cwd string resolving to a different WorkspacePath row is a
/// different launch, and `sessions.workspace_path_id` is hashed for it.
#[test]
fn a_session_workspace_path_change_makes_a_resume_plan_stale() {
    let db = open_db("session-path");
    seed_installation(&db, Agent::Codex);
    let dir = real_dir("session-path", "repo");
    let first = db
        .tx(|tx| insert_workspace_path_conn(tx, &dir, PROJECT))
        .unwrap();
    let s = session_row(&db, Some(&dir), Some(&first));

    let launcher = launcher_in("session-path");
    let prepared = launcher
        .prepare_resume_in(&db, &s.id, &LaunchWorkspace::default())
        .unwrap();
    assert_eq!(prepared.cwd.as_deref(), Some(dir.as_str()));

    // Re-point the Session at a *different row*. Nothing about the cwd string
    // changed, so only `session_path:` can notice it — which is the point of
    // listing "Session workspace path" as a stale input.
    let other = db
        .tx(|tx| insert_workspace_path_conn(tx, &format!("{dir}-reidentified"), PROJECT))
        .unwrap();
    assert_ne!(other, first);
    db.tx(|tx| move_session_to_path_conn(tx, &s.id, &other))
        .unwrap();

    let err = launcher
        .launch_prepared_in(&db, &prepared, &LaunchWorkspace::default(), None)
        .expect_err("a Session that now names a different WorkspacePath is a different launch");
    assert!(is_stale(&err.to_string()), "got: {err}");
}

// no phantom Session

/// a launch creates **no** Session,
/// no WorkspacePath and no WorkstreamPath. Those appear only when the real
/// Agent transcript is discovered.
#[test]
fn a_launch_creates_no_phantom_session_or_path_before_discovery() {
    let db = open_db("phantom");
    seed_installation(&db, Agent::Codex);
    let primary = real_dir("phantom", "primary");
    let w = ws_with_paths(&db, "watch it", &[primary.clone()]);
    let paths_before = count(&db, "SELECT COUNT(*) FROM workstream_projects");
    let wp_before = count(&db, "SELECT COUNT(*) FROM workspace_paths");

    let launcher = launcher_in("phantom");
    let prepared = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            Some(w.as_str()),
            Some(&primary),
            &LaunchWorkspace::default(),
        )
        .unwrap();

    assert_eq!(count(&db, "SELECT COUNT(*) FROM sessions"), 0);
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM workstream_projects"),
        paths_before,
        "prepare may not grow a Workstream's path list"
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
        .expect("the launch proceeds");
    assert_eq!(result.launched_via, "test-spawn");

    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM sessions"),
        0,
        "the Agent's Session is discovered, never manufactured at launch"
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM workspace_paths"),
        wp_before,
        "launching does not invent path rows"
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM workstream_projects"),
        paths_before,
        "and does not invent WorkstreamPath rows either"
    );
}

// payload coherence

/// The payload's own coherence: `PreparedLaunch.cwd` and
/// `PreparedLaunch.cwd_resolution` describe one answer, the tier is named for
/// every flow the New Session form can be opened from, and the directory the
/// payload names is the directory the spawn receives (Preview-Launch Identity).
#[test]
fn the_prepared_payload_names_its_tier_for_every_flow() {
    let db = open_db("payload");
    seed_installation(&db, Agent::Codex);
    let primary = real_dir("payload", "primary");
    let default_ws = real_dir("payload", "workspace");
    let w = ws_with_paths(&db, "payload ws", &[primary.clone()]);
    let lost = missing_dir("payload", "unmounted");
    let unmounted = ws_with_paths(&db, "unmounted ws", &[lost]);
    let launcher = launcher_in("payload");
    let home = workspace(Some(&default_ws));
    let no_home = LaunchWorkspace::default();

    let alias = format!("{primary}/.");
    let cases = vec![
        (
            &home,
            Some(w.clone()),
            Some(primary.as_str()),
            CwdSource::Explicit,
            false,
        ),
        (
            &home,
            Some(w.clone()),
            Some(alias.as_str()),
            CwdSource::Explicit,
            false,
        ),
        (&home, None, None, CwdSource::DefaultWorkspace, false),
        (&no_home, None, None, CwdSource::Unresolved, false),
    ];
    assert!(launcher
        .prepare_new_in(&db, Agent::Codex, Some(&unmounted), None, &home)
        .is_err());
    assert!(launcher
        .prepare_new_in(&db, Agent::Codex, Some(&w), Some("/typed/dir"), &home)
        .is_err());

    for (at_launch, owner, explicit, source, fallback) in cases {
        let prepared = launcher
            .prepare_new_in(&db, Agent::Codex, owner.as_deref(), explicit, at_launch)
            .unwrap();
        assert_eq!(prepared.cwd_resolution.source, source);
        assert_eq!(prepared.cwd_resolution.fallback, fallback);
        assert_eq!(prepared.cwd, prepared.cwd_resolution.cwd);
        assert_eq!(
            prepared.initial_workstream_ids,
            owner.into_iter().collect::<Vec<_>>()
        );
        let result = launcher
            .launch_prepared_with_in(&db, &prepared, at_launch, fake_spawn, fake_open, None)
            .unwrap_or_else(|e| panic!("{source:?} preview must launch: {e}"));
        match prepared.cwd.as_deref() {
            Some(dir) => assert!(
                result.command_line.ends_with(dir),
                "the spawn must receive the previewed directory, got {}",
                result.command_line
            ),
            None => assert!(result.command_line.ends_with("<none>")),
        }
    }
}

#[test]
fn one_time_runtime_reaches_spawn_without_mutating_saved_settings() {
    use noending::agent_runtime::{
        get_runtime_overrides, set_runtime_overrides, AgentRuntimeOverrides,
    };
    let db = open_db("one-time-runtime");
    seed_installation(&db, Agent::Codex);
    let l = launcher_in("one-time-runtime");
    let ws = workspace(Some(&real_dir("one-time-runtime", "cwd")));
    let saved = AgentRuntimeOverrides {
        model: Some("saved".into()),
        ..Default::default()
    };
    set_runtime_overrides(&db, Agent::Codex, &saved).unwrap();
    let custom = AgentRuntimeOverrides {
        model: Some("single".into()),
        effort: Some("low".into()),
        provider: None,
    };
    let prepared = l
        .prepare_new_with_runtime_in(&db, Agent::Codex, &[], None, &ws, Some(&custom))
        .unwrap();
    assert_eq!(
        get_runtime_overrides(&db, Agent::Codex)
            .unwrap()
            .model
            .as_deref(),
        Some("saved")
    );
    // Saved defaults may change after preview; an explicit launch is independent.
    set_runtime_overrides(
        &db,
        Agent::Codex,
        &AgentRuntimeOverrides {
            model: Some("changed".into()),
            ..Default::default()
        },
    )
    .unwrap();
    fn assert_custom(cmd: &AgentCommand) -> Result<LaunchOutcome> {
        assert!(cmd.args.windows(2).any(|w| w == ["-m", "single"]));
        assert!(cmd
            .args
            .iter()
            .any(|a| a == "model_reasoning_effort=\"low\""));
        fake_spawn(cmd)
    }
    l.launch_prepared_with_in(&db, &prepared, &ws, assert_custom, fake_open, None)
        .unwrap();
    let native = l
        .prepare_new_with_runtime_in(
            &db,
            Agent::Codex,
            &[],
            None,
            &ws,
            Some(&AgentRuntimeOverrides::default()),
        )
        .unwrap();
    fn assert_native(cmd: &AgentCommand) -> Result<LaunchOutcome> {
        assert!(!cmd
            .args
            .iter()
            .any(|a| a == "-m" || a.contains("model_reasoning_effort")));
        fake_spawn(cmd)
    }
    l.launch_prepared_with_in(&db, &native, &ws, assert_native, fake_open, None)
        .unwrap();
    assert_eq!(
        get_runtime_overrides(&db, Agent::Codex)
            .unwrap()
            .model
            .as_deref(),
        Some("changed")
    );
}

#[test]
fn disabling_new_launches_rejects_new_and_prepared_new_but_allows_resume() {
    use noending::agent_runtime::store::{launch_enabled, set_launch_enabled};
    let db = open_db("launch-preference");
    seed_installation(&db, Agent::Codex);
    let cwd = real_dir("launch-preference", "cwd");
    let ws = workspace(Some(&cwd));
    let l = launcher_in("launch-preference");
    let prepared = l
        .prepare_new_in(&db, Agent::Codex, None, None, &ws)
        .unwrap();
    let session = session_row(&db, Some(&cwd), None);
    set_launch_enabled(&db, Agent::Codex, false).unwrap();
    assert!(!launch_enabled(&db, Agent::Codex).unwrap());
    assert!(db.get_installation(Agent::Codex).unwrap().is_some());
    assert!(l
        .prepare_new_in(&db, Agent::Codex, None, None, &ws)
        .is_err());
    assert!(l
        .launch_prepared_with_in(&db, &prepared, &ws, fake_spawn, fake_open, None)
        .is_err());
    let resume = l.prepare_resume_in(&db, &session.id, &ws).unwrap();
    l.launch_prepared_with_in(&db, &resume, &ws, fake_spawn, fake_open, None)
        .unwrap();
    set_launch_enabled(&db, Agent::Codex, true).unwrap();
    assert!(l.prepare_new_in(&db, Agent::Codex, None, None, &ws).is_ok());
    assert!(set_launch_enabled(&db, Agent::Dsh, true).is_err());
}

#[test]
fn every_selected_task_is_validated_and_part_of_the_frozen_launch() {
    let db = open_db("multiple-launch-tasks");
    seed_installation(&db, Agent::Codex);
    let cwd = real_dir("multiple-launch-tasks", "project");
    let a = ws_with_paths(&db, "First task", &[cwd.clone()]);
    let b = ws_with_paths(&db, "Second task", &[cwd.clone()]);
    let launcher = launcher_in("multiple-launch-tasks");
    let workspace = LaunchWorkspace::default();
    let prepared = launcher
        .prepare_new_with_runtime_in(
            &db,
            Agent::Codex,
            &[a.clone(), b.clone()],
            Some(&cwd),
            &workspace,
            None,
        )
        .unwrap();
    // Changing any selected task invalidates the frozen preview.
    db.write()
        .execute(
            "UPDATE workstreams SET description='changed' WHERE id=?1",
            [&b],
        )
        .unwrap();
    let err = launcher
        .launch_prepared_with_in(&db, &prepared, &workspace, fake_spawn, fake_open, None)
        .unwrap_err();
    assert!(is_stale(&err.to_string()));
    let prepared = launcher
        .prepare_new_with_runtime_in(
            &db,
            Agent::Codex,
            &[a.clone(), b.clone()],
            Some(&cwd),
            &workspace,
            None,
        )
        .unwrap();
    // Removing one task from the project refuses launch even if another remains eligible.
    noending::workspace::workstream::set_workstream_projects(&db, &b, &[]).unwrap();
    assert!(launcher
        .launch_prepared_with_in(&db, &prepared, &workspace, fake_spawn, fake_open, None)
        .is_err());
    assert!(launcher
        .prepare_new_with_runtime_in(
            &db,
            Agent::Codex,
            &[a.clone(), b.clone()],
            Some(&cwd),
            &workspace,
            None
        )
        .is_err());
    noending::workspace::workstream::set_workstream_projects(&db, &b, &[PROJECT.into()]).unwrap();
    db.write()
        .execute(
            "UPDATE workstreams SET visibility='archived' WHERE id=?1",
            [&b],
        )
        .unwrap();
    assert!(launcher
        .prepare_new_with_runtime_in(&db, Agent::Codex, &[a, b], Some(&cwd), &workspace, None)
        .is_err());
}
