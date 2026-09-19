//! Workspace Domain v0.2 launcher integration (方案 §21, §42.3-M15/M16/M17/M21).
//!
//! Three things are pinned here, and they are the three that used to be
//! silently wrong:
//!
//! 1. **The launch directory comes from the ordered `WorkstreamPath` list** and
//!    NoEnding Home's default workspace — never from the frozen
//!    `workstreams.default_cwd` (§42.2-E6) and never from another Session's cwd.
//! 2. **Those inputs are in the PreparedLaunch fingerprint.** A WorkstreamPath
//!    mutation does *not* bump `workstreams.updated_at`, so before this existed
//!    a reorder, an added path or a removed primary could leave a preview that
//!    no longer described reality looking fresh (§12). Every stale test below
//!    changes exactly ONE input and asserts the launch is refused.
//! 3. **A resume launches in the directory the preview showed** (§42.3-M16), and
//!    when the Session's own directory is gone the fallback is data in the
//!    payload, not a silent substitution (§13 "发生 fallback 必须在 UI 明确显示").
//!
//! Everything is hermetic: temp databases, temp directories, and a
//! `LaunchWorkspace` literal standing in for the user's real Home (§42.3-M13).
//! The spawn step is injected, so no Terminal window opens and no Agent process
//! starts; the fake echoes the directory it was handed.

use rusqlite::Connection;

use noending::adapters::AgentCommand;
use noending::domain::{
    binding_source, launch_status, workstream_path_source, Agent, LaunchIntent, Project, Session,
};
use noending::error::Result;
use noending::launcher::{
    apply_match, record_binding, resolve_resume_cwd, try_match_launch_intents_in, CwdSource,
    LaunchWorkspace, SessionLauncher,
};
use noending::platform::exec_resolver::AgentInstallation;
use noending::platform::launcher::LaunchOutcome;
use noending::storage::workspace::insert_workspace_path_conn;
use noending::storage::{new_id, now, Db};
use noending::workspace::session::move_session_to_path_conn;
use noending::workspace::workstream::{
    add_workstream_path, create_workstream, remove_workstream_path, reorder_workstream_paths,
    workstream_launch_paths,
};
use noending::workspace::{normalize_path, path_identity, WorkspaceAttaching};

const PROJECT: &str = "p-launcher-ws";

// ------------------------------------------------------------------ fixtures

fn open_db(tag: &str) -> Db {
    let dir = std::env::temp_dir().join(format!("noending-lws-{}-{}", tag, new_id()));
    let db = Db::open(&dir.join("test.db")).unwrap();
    db.upsert_project(&Project::new(PROJECT.into(), "P"))
        .unwrap();
    db
}

/// Lexical, filesystem-free path identity (§42.3-M8): a fixture must hand the
/// production code the same spelling the production door would have stored, or
/// the assertion is a Unix-only assertion.
fn canonical(dir: &std::path::Path) -> String {
    normalize_path(&dir.to_string_lossy()).expect("fixture path normalizes")
}

fn temp_root(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("noending-lws-{}-{}", tag, new_id()))
}

/// A directory that exists: §13 only launches into a usable one.
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

fn path_id(db: &Db, canonical_path: &str) -> String {
    // 方案 §44.3-C2 — read the row through the key the app writes it with. A
    // lookup by spelling can miss a row stored under another case, which is what
    // a Windows host may now do to the same directory; a test that missed it
    // would assert against nothing.
    let id = path_identity(canonical_path);
    db.get_workspace_path(&id)
        .unwrap()
        .unwrap_or_else(|| panic!("no WorkspacePath row for {canonical_path}"))
        .id
}

fn ws_with_paths(db: &Db, title: &str, dirs: &[String]) -> String {
    let id = create_workstream(
        db,
        &LexicalPaths,
        title,
        "",
        dirs.first().map(String::as_str),
    )
    .unwrap()
    .id;
    for dir in dirs.iter().skip(1) {
        add_workstream_path(db, &LexicalPaths, &id, dir).unwrap();
    }
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
    db.conn()
        .query_row(sql, [], |r| r.get::<_, i64>(0))
        .unwrap()
}

/// Echoes the directory it was asked to start in, so a test can prove *which*
/// cwd reached the OS. A `fn` pointer cannot capture, and a shared counter
/// would race across this binary's parallel test threads.
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
    })
}

fn session_row(db: &Db, cwd: Option<&str>, workspace_path_id: Option<&str>) -> Session {
    let raw = std::env::temp_dir().join(format!("noending-lws-raw-{}.jsonl", new_id()));
    let _ = std::fs::write(&raw, "");
    let s = Session {
        id: new_id(),
        agent: Agent::Codex,
        agent_session_id: format!("as-{}", new_id()),
        title: None,
        cwd: cwd.map(|s| s.to_string()),
        workspace_path_id: workspace_path_id.map(|s| s.to_string()),
        project_id: None,
        raw_path: raw.to_string_lossy().to_string(),
        parent_agent_session_id: None,
        started_at: Some(now()),
        last_activity_at: Some(now()),
    };
    db.upsert_session(&s).unwrap();
    db.get_session(&s.id).unwrap().unwrap()
}

fn is_stale(err: &str) -> bool {
    err.contains("stale")
}

// ----------------------------------------------------- §12 staleness of paths

/// §21 "reorder primary makes plan stale". Position 0 is the whole meaning of
/// the ordered list, and reordering it changes no `updated_at` anywhere — the
/// only thing that can catch it is the list itself being a fingerprint input.
#[test]
fn reordering_the_primary_makes_a_prepared_launch_stale() {
    let db = open_db("reorder");
    seed_installation(&db, Agent::Codex);
    let first = real_dir("reorder", "first");
    let second = real_dir("reorder", "second");
    let w = ws_with_paths(&db, "two paths", &[first.clone(), second.clone()]);
    assert_eq!(
        workstream_launch_paths(&db, &w).unwrap().ordered_paths,
        vec![first.clone(), second.clone()]
    );

    let launcher = launcher_in("reorder");
    let prepared = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            &[w.clone()],
            None,
            &LaunchWorkspace::default(),
        )
        .unwrap();
    assert_eq!(prepared.cwd.as_deref(), Some(first.as_str()));

    // Same set, different order: `second` becomes primary.
    reorder_workstream_paths(&db, &w, &[path_id(&db, &second), path_id(&db, &first)]).unwrap();

    let err = launcher
        .launch_prepared_in(&db, &prepared, &LaunchWorkspace::default())
        .expect_err("a reordered primary must not launch the old preview");
    assert!(is_stale(&err.to_string()), "got: {err}");
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM launch_intents"),
        0,
        "a refused launch never reaches the spawn step"
    );

    // Re-previewing under the new order works and follows the new primary.
    let fresh = launcher
        .prepare_new_in(&db, Agent::Codex, &[w], None, &LaunchWorkspace::default())
        .unwrap();
    assert_eq!(fresh.cwd.as_deref(), Some(second.as_str()));
    let launched = launcher
        .launch_prepared_with_in(&db, &fresh, &LaunchWorkspace::default(), fake_spawn)
        .expect("a preview made after the reorder is launchable");
    assert!(
        launched.command_line.ends_with(&second),
        "the Agent must be started in the new primary path, got {}",
        launched.command_line
    );
}

/// §21 "remove primary makes plan stale" — and the same for any other entry:
/// the list is hashed as a list, so an addition is a change too.
#[test]
fn removing_or_adding_a_workstream_path_makes_a_prepared_launch_stale() {
    let db = open_db("remove");
    seed_installation(&db, Agent::Codex);
    let primary = real_dir("remove", "primary");
    let other = real_dir("remove", "other");
    let w = ws_with_paths(&db, "paths", &[primary.clone(), other.clone()]);
    let launcher = launcher_in("remove");

    let prepared = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            &[w.clone()],
            None,
            &LaunchWorkspace::default(),
        )
        .unwrap();
    assert_eq!(prepared.cwd.as_deref(), Some(primary.as_str()));

    // Removing the *non-primary* entry is also a state change: the launch did
    // not use it, but the plan described this Workstream's work locations.
    let rows = db.list_workstream_paths(&w).unwrap();
    let other_row = rows
        .iter()
        .find(|r| r.workspace_path_id == path_id(&db, &other))
        .unwrap();
    remove_workstream_path(&db, &w, &other_row.id).unwrap();
    let err = launcher
        .launch_prepared_in(&db, &prepared, &LaunchWorkspace::default())
        .expect_err("a removed path must invalidate the preview");
    assert!(is_stale(&err.to_string()), "got: {err}");

    // Re-previewed: only `primary` is left, and removing *it* must invalidate
    // the plan as well.
    let after_removal = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            &[w.clone()],
            None,
            &LaunchWorkspace::default(),
        )
        .unwrap();
    assert_eq!(after_removal.cwd.as_deref(), Some(primary.as_str()));
    let primary_row = db.list_workstream_paths(&w).unwrap().remove(0);
    remove_workstream_path(&db, &w, &primary_row.id).unwrap();
    let err = launcher
        .launch_prepared_in(&db, &after_removal, &LaunchWorkspace::default())
        .expect_err("removing the primary path must invalidate the preview");
    assert!(is_stale(&err.to_string()), "got: {err}");

    // A Workstream with zero paths and no Home previewed here: nothing to launch.
    let empty = launcher
        .prepare_new_in(&db, Agent::Codex, &[w], None, &LaunchWorkspace::default())
        .unwrap();
    assert_eq!(empty.cwd, None);
    add_workstream_path(&db, &LexicalPaths, &empty.workstream_ids[0], &primary).unwrap();
    let err = launcher
        .launch_prepared_in(&db, &empty, &LaunchWorkspace::default())
        .expect_err("an added path must invalidate the preview too");
    assert!(is_stale(&err.to_string()), "got: {err}");
}

/// §21 "default workspace change makes plan stale". The default workspace is a
/// Home fact with no DB row, so a DB-only fingerprint cannot see it — it is
/// hashed as its own labelled input, which is why this launch is refused.
#[test]
fn a_default_workspace_change_makes_a_prepared_launch_stale() {
    let db = open_db("default-ws");
    seed_installation(&db, Agent::Codex);
    let launcher = launcher_in("default-ws");
    let before = real_dir("default-ws", "home-a/workspace");
    let after = real_dir("default-ws", "home-b/workspace");

    let prepared = launcher
        .prepare_new_in(&db, Agent::Codex, &[], None, &workspace(Some(&before)))
        .unwrap();
    assert_eq!(prepared.cwd.as_deref(), Some(before.as_str()));

    let err = launcher
        .launch_prepared_in(&db, &prepared, &workspace(Some(&after)))
        .expect_err("the Home moved: the previewed directory is not the one to launch");
    assert!(is_stale(&err.to_string()), "got: {err}");

    // Losing the default workspace entirely is a change too, never an accident.
    let err = launcher
        .launch_prepared_in(&db, &prepared, &LaunchWorkspace::default())
        .expect_err("a launcher with no Home must not launch a Home-bound preview");
    assert!(is_stale(&err.to_string()), "got: {err}");

    let same = launcher
        .launch_prepared_with_in(&db, &prepared, &workspace(Some(&before)), fake_spawn)
        .expect("the unchanged Home still satisfies the preview");
    assert!(same.command_line.ends_with(&before));
}

// ------------------------------------------------------------------- §13 resume

/// §21 "resume uses original cwd": the Session's own directory is tier 1 and it
/// reaches the OS.
#[test]
fn a_resume_launches_in_the_sessions_own_cwd() {
    let db = open_db("resume-cwd");
    seed_installation(&db, Agent::Codex);
    let home_dir = real_dir("resume-cwd", "session-home");
    let s = session_row(&db, Some(&home_dir), None);
    let w = ws_with_paths(&db, "elsewhere", &[real_dir("resume-cwd", "ws-path")]);
    record_binding(
        &db,
        &s.id,
        &w,
        "related",
        binding_source::USER_ASSIGNED,
        1.0,
    )
    .unwrap();

    let launcher = launcher_in("resume-cwd");
    let prepared = launcher
        .prepare_resume_in(&db, &s.id, &[], &LaunchWorkspace::default())
        .unwrap();
    assert_eq!(prepared.cwd.as_deref(), Some(home_dir.as_str()));
    assert_eq!(prepared.cwd_resolution.source, CwdSource::SessionCwd);
    assert!(!prepared.cwd_resolution.fallback);
    assert!(prepared.cwd_resolution.note.is_none());

    let result = launcher
        .launch_prepared_with_in(&db, &prepared, &LaunchWorkspace::default(), fake_spawn)
        .expect("a resume with its own directory launches");
    assert!(
        result.command_line.ends_with(&home_dir),
        "the Workstream's path must not outrank the Session's own cwd, got {}",
        result.command_line
    );
}

/// §42.3-M16 — the pre-v0.2 resume branch re-read `sessions.cwd` at launch time,
/// so background discovery could move the Agent between preview and launch.
/// The preview's directory is now both authoritative and hashed.
#[test]
fn a_cwd_drift_after_preview_makes_a_resume_plan_stale() {
    let db = open_db("resume-drift");
    seed_installation(&db, Agent::Codex);
    let original = real_dir("resume-drift", "original");
    let s = session_row(&db, Some(&original), None);

    let launcher = launcher_in("resume-drift");
    let prepared = launcher
        .prepare_resume_in(&db, &s.id, &[], &LaunchWorkspace::default())
        .unwrap();

    // Discovery rewrites the cwd (§7.2: the transcript is the source of truth).
    let moved = real_dir("resume-drift", "moved");
    let mut drifted = db.get_session(&s.id).unwrap().unwrap();
    drifted.cwd = Some(moved.clone());
    db.upsert_session(&drifted).unwrap();

    let err = launcher
        .launch_prepared_in(&db, &prepared, &LaunchWorkspace::default())
        .expect_err("a moved Session directory must abort, not silently follow");
    assert!(is_stale(&err.to_string()), "got: {err}");

    let fresh = launcher
        .prepare_resume_in(&db, &s.id, &[], &LaunchWorkspace::default())
        .unwrap();
    assert_eq!(fresh.cwd.as_deref(), Some(moved.as_str()));
    let result = launcher
        .launch_prepared_with_in(&db, &fresh, &LaunchWorkspace::default(), fake_spawn)
        .expect("re-previewed under the new cwd the launch proceeds");
    assert!(result.command_line.ends_with(&moved));
}

/// §21 "resume fallback visible": a Session whose directory is gone no longer
/// resolves to "no cwd, land in $HOME" — it falls back through §13, and the
/// payload says which tier and why.
#[test]
fn a_resume_fallback_is_recorded_in_the_prepared_payload() {
    let db = open_db("resume-fallback");
    seed_installation(&db, Agent::Codex);
    let lost = missing_dir("resume-fallback", "deleted-repo");
    let primary = real_dir("resume-fallback", "ws-primary");
    let s = session_row(&db, Some(&lost), None);
    let w = ws_with_paths(&db, "carries on", &[primary.clone()]);
    record_binding(
        &db,
        &s.id,
        &w,
        "related",
        binding_source::USER_ASSIGNED,
        1.0,
    )
    .unwrap();

    let launcher = launcher_in("resume-fallback");
    let prepared = launcher
        .prepare_resume_in(&db, &s.id, &[], &LaunchWorkspace::default())
        .unwrap();

    assert_eq!(prepared.cwd.as_deref(), Some(primary.as_str()));
    let resolution = &prepared.cwd_resolution;
    assert_eq!(resolution.source, CwdSource::WorkstreamPath);
    assert_eq!(resolution.workstream_id.as_deref(), Some(w.as_str()));
    assert_eq!(resolution.path_position, Some(0));
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
        .launch_prepared_with_in(&db, &prepared, &LaunchWorkspace::default(), fake_spawn)
        .expect("a fallback is a legitimate launch");
    assert!(result.command_line.ends_with(&primary));
}

/// The last resume tier: no Session directory, no usable Workstream path, so
/// Home's default workspace takes it — again as data, not as a silent default.
#[test]
fn a_resume_falls_back_to_the_default_workspace_and_says_so() {
    let db = open_db("resume-default");
    seed_installation(&db, Agent::Codex);
    let lost = missing_dir("resume-default", "gone");
    let s = session_row(&db, Some(&lost), None);
    let default_ws = real_dir("resume-default", "workspace");

    let resolution = resolve_resume_cwd(&db, &s.id, &[], &workspace(Some(&default_ws))).unwrap();
    assert_eq!(resolution.cwd.as_deref(), Some(default_ws.as_str()));
    assert_eq!(resolution.source, CwdSource::DefaultWorkspace);
    assert!(resolution.fallback);
    assert!(resolution.note.unwrap().contains(&lost));

    let launcher = launcher_in("resume-default");
    let prepared = launcher
        .prepare_resume_in(&db, &s.id, &[], &workspace(Some(&default_ws)))
        .unwrap();
    assert_eq!(prepared.cwd.as_deref(), Some(default_ws.as_str()));
    let result = launcher
        .launch_prepared_with_in(&db, &prepared, &workspace(Some(&default_ws)), fake_spawn)
        .expect("a default-workspace resume launches");
    assert!(result.command_line.ends_with(&default_ws));
}

/// §12 "Session cwd/path change" is a stale input on the *identity* side as
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
        .prepare_resume_in(&db, &s.id, &[], &LaunchWorkspace::default())
        .unwrap();
    assert_eq!(prepared.cwd.as_deref(), Some(dir.as_str()));

    // Re-point the Session at a *different row*. Nothing about the cwd string
    // changed, so only `session_path:` can notice it — which is the point of
    // §12 listing "Session workspace path" as a stale input.
    let other = db
        .tx(|tx| insert_workspace_path_conn(tx, &format!("{dir}-reidentified"), PROJECT))
        .unwrap();
    assert_ne!(other, first);
    db.tx(|tx| move_session_to_path_conn(tx, &s.id, &other))
        .unwrap();

    let err = launcher
        .launch_prepared_in(&db, &prepared, &LaunchWorkspace::default())
        .expect_err("a Session that now names a different WorkspacePath is a different launch");
    assert!(is_stale(&err.to_string()), "got: {err}");
}

// ------------------------------------------------ §21-9/10 no phantom Session

/// §21-9 — a launch records a durable `LaunchIntent` and creates **no** Session,
/// no WorkspacePath and no WorkstreamPath. Those appear only when the real
/// Agent transcript is discovered (§21-10).
#[test]
fn a_launch_creates_no_phantom_session_or_path_before_discovery() {
    let db = open_db("phantom");
    seed_installation(&db, Agent::Codex);
    let primary = real_dir("phantom", "primary");
    let w = ws_with_paths(&db, "watch it", &[primary.clone()]);
    let paths_before = count(&db, "SELECT COUNT(*) FROM workstream_paths");
    let wp_before = count(&db, "SELECT COUNT(*) FROM workspace_paths");

    let launcher = launcher_in("phantom");
    let prepared = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            &[w.clone()],
            None,
            &LaunchWorkspace::default(),
        )
        .unwrap();
    assert_eq!(count(&db, "SELECT COUNT(*) FROM launch_intents"), 0);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM sessions"), 0);
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM workstream_paths"),
        paths_before,
        "prepare may not grow a Workstream's path list"
    );

    let result = launcher
        .launch_prepared_with_in(&db, &prepared, &LaunchWorkspace::default(), fake_spawn)
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
        count(&db, "SELECT COUNT(*) FROM workstream_paths"),
        paths_before,
        "and does not invent WorkstreamPath rows either"
    );
    assert_eq!(count(&db, "SELECT COUNT(*) FROM launch_intents"), 1);
    let intents = db.list_launch_intents(&[], 10).unwrap();
    assert_eq!(intents[0].status, launch_status::PENDING);
    assert_eq!(intents[0].cwd.as_deref(), Some(primary.as_str()));
    assert_eq!(intents[0].selected_workstream_ids, vec![w]);
}

/// §21-10 — when discovery finds the real Session and its transcript matches the
/// intent, the user's launch-time selection becomes an explicit binding, the
/// WorkstreamPath for the directory actually used is ensured, and the Project
/// comes from the derived cache. All three come out of one door
/// (`workspace::session::record_user_binding`), so there is no second engine
/// writing bindings or path lists from launch code.
#[test]
fn a_matched_session_produces_binding_workstream_path_and_project() {
    let db = open_db("matched");
    let dir = real_dir("matched", "repo");
    let session_path = db
        .tx(|tx| insert_workspace_path_conn(tx, &dir, PROJECT))
        .unwrap();
    let w = create_workstream(&db, &LexicalPaths, "launched", "", None)
        .unwrap()
        .id;
    let intent = LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent: Agent::Codex,
        selected_workstream_ids: vec![w.clone()],
        cwd: Some(dir.clone()),
        context_bundle_markdown: None,
        context_bundle_revisions: None,
        process_id: None,
        launched_at: now(),
        matched_session_id: None,
        status: launch_status::PENDING.into(),
        note: String::new(),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_launch_intent(&intent).unwrap();

    // Discovery observed the Session: its cwd resolved to a WorkspacePath, whose
    // Project was derived inside `upsert_session`.
    let s = session_row(&db, Some(&dir), Some(&session_path));
    assert_eq!(s.project_id.as_deref(), Some(PROJECT));
    assert_eq!(count(&db, "SELECT COUNT(*) FROM workstream_paths"), 0);

    apply_match(&db, &intent, &s, &LaunchWorkspace::default()).unwrap();

    let bindings = db.bindings_for_session(&s.id).unwrap();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].workstream_id, w);
    assert_eq!(bindings[0].source, binding_source::EXPLICIT_LAUNCH);
    assert_eq!(bindings[0].confidence, 1.0);
    assert!(
        bindings[0].workstream_path_id.is_some(),
        "the binding must record which path brought the Session in (§5.6)"
    );

    // §1.8: the Workstream had no paths, so the Session's path becomes position 0.
    let rows = db.list_workstream_paths(&w).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].position, 0);
    assert_eq!(rows[0].workspace_path_id, session_path);
    assert_eq!(
        workstream_launch_paths(&db, &w).unwrap().ordered_paths,
        vec![dir.clone()],
        "the ensured path is the one the launch used"
    );
    assert!(
        matches!(
            rows[0].source.as_str(),
            workstream_path_source::SESSION | workstream_path_source::LAUNCH
        ),
        "a path grown by a binding records how it got there, got {}",
        rows[0].source
    );

    // Derived Project membership: the Session's cache and the path agree, and the
    // Workstream's primary Project projection now resolves through the list.
    let (project_id, _) =
        noending::workspace::workstream::primary_project_for_workstream(&db, &w).unwrap();
    assert_eq!(project_id.as_deref(), Some(PROJECT));
    assert_eq!(s.project_id.as_deref(), Some(PROJECT));

    let stored = db.get_launch_intent(&intent.id).unwrap().unwrap();
    assert_eq!(stored.status, launch_status::MATCHED);
    assert_eq!(stored.matched_session_id.as_deref(), Some(s.id.as_str()));
}

// ---------------------------------------------------------------- §42.3-M15

/// §13's third tier routes independent launches into ONE shared directory, so
/// cwd stops being evidence: two standalone launches at the default workspace
/// must not auto-match each other's Session. They stay unresolved for a human,
/// and a human decision binds each intent to its own Session.
#[test]
fn concurrent_default_workspace_launches_stay_ambiguous() {
    let db = open_db("m15");
    let default_ws = real_dir("m15", "workspace");
    let a = ws_with_paths(&db, "A", &[real_dir("m15", "a")]);
    let b = ws_with_paths(&db, "B", &[real_dir("m15", "b")]);
    let launch = |ws: &str| LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent: Agent::Codex,
        selected_workstream_ids: vec![ws.to_string()],
        cwd: Some(default_ws.clone()),
        context_bundle_markdown: None,
        context_bundle_revisions: None,
        process_id: None,
        launched_at: now(),
        matched_session_id: None,
        status: launch_status::PENDING.into(),
        note: String::new(),
        created_at: now(),
        updated_at: now(),
    };
    let intent_a = launch(&a);
    let intent_b = launch(&b);
    db.insert_launch_intent(&intent_a).unwrap();
    db.insert_launch_intent(&intent_b).unwrap();
    let discovered = session_row(&db, Some(&default_ws), None);

    let matched =
        try_match_launch_intents_in(&db, &discovered, &workspace(Some(&default_ws))).unwrap();
    assert!(!matched, "a shared directory is not evidence: never guess");

    let intents = db.list_launch_intents(&[], 10).unwrap();
    assert_eq!(
        intents
            .iter()
            .filter(|i| i.status == launch_status::AMBIGUOUS)
            .count(),
        2,
        "§42.3-M15: 双双 AMBIGUOUS — every tied candidate surfaces for the user, \
         and neither may be auto-consumed by a directory they merely share"
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM session_workstream_bindings"),
        0,
        "an ambiguous match must not bind anything"
    );

    // The human decision is what binds, and it cannot cross the two intents.
    apply_match(&db, &intent_a, &discovered, &LaunchWorkspace::default()).unwrap();
    let bindings = db.bindings_for_session(&discovered.id).unwrap();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].workstream_id, a);
    assert_eq!(bindings[0].source, binding_source::EXPLICIT_LAUNCH);
    let after = db.get_launch_intent(&intent_a.id).unwrap().unwrap();
    assert_eq!(after.status, launch_status::MATCHED);
    let other = db.get_launch_intent(&intent_b.id).unwrap().unwrap();
    assert_eq!(other.status, launch_status::AMBIGUOUS);
    assert_eq!(
        other.matched_session_id, None,
        "the other intent is untouched and stays available for its own Session"
    );
}

/// §1.7 — a fallback directory is not a statement about the Workstream.
///
/// A New Session in a Workstream whose own paths are unusable lands in NoEnding
/// Home's shared `workspace/` (§13 tier 3). When that Session is matched back,
/// the *binding* is the user's decision and stays strong, but the directory
/// NoEnding invented must not be appended to the Workstream's ordered list: it
/// would outlive the fallback, and removing it again drags the Sessions under it
/// away (§1.6). A directory the user typed is a different matter and still grows
/// the list, which is the control at the end.
#[test]
fn a_default_workspace_fallback_binds_without_teaching_the_workstream_that_path() {
    let db = open_db("no-laundering");
    let default_ws = real_dir("no-laundering", "workspace");
    let own = real_dir("no-laundering", "repo");
    let w = ws_with_paths(&db, "Laundering", &[own.clone()]);
    let intent = LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent: Agent::Codex,
        selected_workstream_ids: vec![w.clone()],
        cwd: Some(default_ws.clone()),
        context_bundle_markdown: None,
        context_bundle_revisions: None,
        process_id: None,
        launched_at: now(),
        matched_session_id: None,
        status: launch_status::PENDING.into(),
        note: String::new(),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_launch_intent(&intent).unwrap();
    // Discovery observed the Session running in the shared directory, so its
    // cwd resolved to a WorkspacePath just like production's would.
    let shared_path = db
        .tx(|tx| insert_workspace_path_conn(tx, &default_ws, PROJECT))
        .unwrap();
    let s = session_row(&db, Some(&default_ws), Some(&shared_path));

    apply_match(&db, &intent, &s, &workspace(Some(&default_ws))).unwrap();

    let bindings = db.bindings_for_session(&s.id).unwrap();
    assert_eq!(bindings.len(), 1, "the user's binding is still recorded");
    assert_eq!(bindings[0].source, binding_source::EXPLICIT_LAUNCH);
    assert_eq!(
        bindings[0].workstream_path_id, None,
        "a binding that brings no path must hold no claim, so removing a real \
         path cannot take this Session with it"
    );
    assert_eq!(
        path_count(&db, &w),
        1,
        "the shared fallback must not enter the Workstream's path list"
    );
    assert_eq!(
        primary_titles(&db, &w),
        vec![own],
        "the one path it has is still the one the user added"
    );

    // Control: the gate keys on *who chose the directory*, not on being a launch.
    let typed = real_dir("no-laundering", "typed");
    let typed_path = db
        .tx(|tx| insert_workspace_path_conn(tx, &typed, PROJECT))
        .unwrap();
    let s2 = session_row(&db, Some(&typed), Some(&typed_path));
    apply_match(&db, &intent, &s2, &workspace(Some(&default_ws))).unwrap();
    assert_eq!(
        path_count(&db, &w),
        2,
        "a directory the caller named still grows the list (§1.8)"
    );
}

fn path_count(db: &Db, workstream_id: &str) -> i64 {
    db.conn()
        .query_row(
            "SELECT COUNT(*) FROM workstream_paths WHERE workstream_id = ?1",
            rusqlite::params![workstream_id],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
}

fn primary_titles(db: &Db, workstream_id: &str) -> Vec<String> {
    db.conn()
        .prepare(
            "SELECT wp.canonical_path FROM workstream_paths p
             JOIN workspace_paths wp ON wp.id = p.workspace_path_id
             WHERE p.workstream_id = ?1 ORDER BY p.position",
        )
        .unwrap()
        .query_map(rusqlite::params![workstream_id], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap()
}

/// A tie is only a tie when nothing else distinguishes the candidates. An
/// explicit Workstream selection is a user statement, so it outranks a
/// coincidental shared directory — but only against an equally-scored intent
/// that selected nothing.
#[test]
fn an_explicit_selection_breaks_a_tie_a_shared_directory_cannot() {
    let db = open_db("m15-tie");
    let default_ws = real_dir("m15-tie", "workspace");
    let selected = ws_with_paths(&db, "selected", &[real_dir("m15-tie", "p")]);
    let chosen = LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent: Agent::Codex,
        selected_workstream_ids: vec![selected.clone()],
        cwd: Some(default_ws.clone()),
        context_bundle_markdown: None,
        context_bundle_revisions: None,
        process_id: None,
        launched_at: now(),
        matched_session_id: None,
        status: launch_status::PENDING.into(),
        note: String::new(),
        created_at: now(),
        updated_at: now(),
    };
    let standalone = LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent: Agent::Codex,
        selected_workstream_ids: vec![],
        cwd: Some(default_ws.clone()),
        context_bundle_markdown: None,
        context_bundle_revisions: None,
        process_id: None,
        launched_at: now(),
        matched_session_id: None,
        status: launch_status::PENDING.into(),
        note: String::new(),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_launch_intent(&chosen).unwrap();
    db.insert_launch_intent(&standalone).unwrap();
    let discovered = session_row(&db, Some(&default_ws), None);

    assert!(
        try_match_launch_intents_in(&db, &discovered, &workspace(Some(&default_ws))).unwrap(),
        "exactly one candidate selected a Workstream: that is the user's intent"
    );
    let bindings = db.bindings_for_session(&discovered.id).unwrap();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].workstream_id, selected);
    assert_eq!(
        db.get_launch_intent(&standalone.id)
            .unwrap()
            .unwrap()
            .status,
        launch_status::PENDING,
        "the unselected intent is untouched, not consumed"
    );
}

// ------------------------------------------------------ payload coherence

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

    let cases: Vec<(&LaunchWorkspace, Vec<String>, Option<&str>, CwdSource, bool)> = vec![
        (
            &home,
            vec![w.clone()],
            Some("/typed/dir"),
            CwdSource::Explicit,
            false,
        ),
        (
            &home,
            vec![w.clone()],
            None,
            CwdSource::WorkstreamPath,
            false,
        ),
        (&home, vec![], None, CwdSource::DefaultWorkspace, false),
        (
            &home,
            vec![unmounted.clone()],
            None,
            CwdSource::DefaultWorkspace,
            true,
        ),
        (&no_home, vec![], None, CwdSource::Unresolved, false),
    ];

    for (at_launch, workstreams, explicit, source, fallback) in cases {
        let prepared = launcher
            .prepare_new_in(&db, Agent::Codex, &workstreams, explicit, at_launch)
            .unwrap();
        assert_eq!(prepared.cwd_resolution.source, source);
        assert_eq!(prepared.cwd_resolution.fallback, fallback);
        assert_eq!(prepared.cwd, prepared.cwd_resolution.cwd);
        assert_eq!(prepared.workstream_ids, workstreams);
        let result = launcher
            .launch_prepared_with_in(&db, &prepared, at_launch, fake_spawn)
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
