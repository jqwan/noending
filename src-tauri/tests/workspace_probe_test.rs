//! The read-only path picker supports (`workspace::probe`).
//!
//! `probe_workspace_path` must describe what the attacher WOULD decide, from
//! the same observation the attacher runs — never write anything, and never
//! guess past the resolver's three refusal faces (§2, §1.4). The recent-paths
//! list must be pure reads over the registry and the Session cwd history.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use noending::domain::{Agent, Project, Session};
use noending::storage::workspace::insert_workspace_path_conn;
use noending::storage::{new_id, Db};
use noending::workspace::home::NoEndingHome;
use noending::workspace::probe::{list_recent_workspace_paths, probe_workspace_path, ProbeStatus};
use noending::workspace::resolver::{ResolverContext, WorkspaceResolver};
use noending::workspace::wiring::HomePolicy;
use noending::workspace::{normalize_path, PathStyle};

// ------------------------------------------------------------- fixtures

fn temp_db() -> (PathBuf, Db) {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "noending-probe-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("noending.db")).expect("temp db");
    (dir, db)
}

fn unix_resolver(home_home: &str, user_home: &str) -> WorkspaceResolver {
    let home = NoEndingHome::new_with_style(home_home, Some(user_home), PathStyle::Unix).unwrap();
    WorkspaceResolver::new(ResolverContext {
        style: Some(PathStyle::Unix),
        user_home: Some(user_home.to_string()),
        reserved: home.reserved(),
        ..ResolverContext::inert()
    })
}

fn home_policy(home_home: &str, user_home: &str) -> HomePolicy {
    let home = NoEndingHome::new_with_style(home_home, Some(user_home), PathStyle::Unix).unwrap();
    HomePolicy::new(&home)
}

fn canonical(raw: &str) -> String {
    normalize_path(raw).expect("fixture path is normalizable")
}

fn session(db: &Db, tag: &str, cwd: &str, activity: &str) -> Session {
    let mut s = Session::new(
        new_id(),
        Agent::Codex,
        format!("src-{tag}"),
        format!("/raw/{tag}.jsonl"),
    );
    s.cwd = Some(cwd.to_string());
    s.last_activity_at = Some(activity.to_string());
    db.upsert_session(&s).unwrap();
    s
}

// --------------------------------------------------------------- rejections

/// The three refusal faces of `try_observe`, named for the UI: the Home itself,
/// a reserved app path, and an un-normalizable string. None of them may leak
/// into a probe that pretends to be usable.
#[test]
fn probe_names_why_a_string_is_not_a_workspace_path() {
    let (_dir, db) = temp_db();
    let resolver = unix_resolver("/Users/tester/.noending", "/Users/tester");
    let policy = home_policy("/Users/tester/.noending", "/Users/tester");

    let probe = probe_workspace_path(&db, &resolver, &policy, "/Users/tester").unwrap();
    assert_eq!(probe.status, ProbeStatus::Home);
    assert!(probe.canonical_path.is_none());
    assert!(probe.project.is_none());

    let probe =
        probe_workspace_path(&db, &resolver, &policy, "/Users/tester/.noending/data").unwrap();
    assert_eq!(probe.status, ProbeStatus::Reserved);

    let probe = probe_workspace_path(&db, &resolver, &policy, "relative/path").unwrap();
    assert_eq!(probe.status, ProbeStatus::Unresolvable);

    // The default workspace under the Home is deliberately NOT reserved (§2).
    let probe =
        probe_workspace_path(&db, &resolver, &policy, "/Users/tester/.noending/workspace").unwrap();
    assert_eq!(probe.status, ProbeStatus::Ok);
}

// ------------------------------------------------------------- predictions

/// An observable directory the registry has never seen: the probe reports the
/// fresh observation and predicts a NEW Project with an automatic name.
#[test]
fn probe_of_an_unknown_directory_predicts_a_new_project() {
    let (_dir, db) = temp_db();
    let resolver = WorkspaceResolver::new(ResolverContext {
        style: Some(PathStyle::Unix),
        ..ResolverContext::inert()
    });
    let policy = HomePolicy::new(
        &NoEndingHome::new_with_style("/Users/tester/.noending", None, PathStyle::Unix).unwrap(),
    );

    let dir = std::env::temp_dir().join("noending-probe-brand-new");
    std::fs::create_dir_all(&dir).unwrap();
    let probe = probe_workspace_path(&db, &resolver, &policy, &dir.to_string_lossy()).unwrap();

    assert_eq!(probe.status, ProbeStatus::Ok);
    assert_eq!(
        probe.canonical_path.as_deref(),
        Some(canonical(&dir.to_string_lossy())).as_deref()
    );
    assert!(probe.exists, "the fixture directory was created");
    // No git binary in the inert context: "could not ask" is its own line.
    assert_eq!(probe.git_state.as_deref(), Some("unavailable"));
    let hint = probe
        .project
        .expect("an observable path always predicts a Project");
    assert!(!hint.known, "the registry has never seen this directory");
    assert!(hint.id.is_none());
    assert!(hint.name.as_deref().is_some_and(|n| !n.is_empty()));
}

/// A path the registry already holds keeps its Project — the probe reads the
/// same row the attacher would reuse, so the two cannot disagree.
#[test]
fn probe_of_a_registered_path_predicts_its_existing_project() {
    let (_dir, db) = temp_db();
    db.upsert_project(&Project::new("p1".into(), "P1")).unwrap();
    let canonical = canonical("/repo/main");
    db.tx(|tx| insert_workspace_path_conn(tx, &canonical, "p1"))
        .unwrap();

    let resolver = WorkspaceResolver::new(ResolverContext {
        style: Some(PathStyle::Unix),
        ..ResolverContext::inert()
    });
    let policy = HomePolicy::new(
        &NoEndingHome::new_with_style("/Users/tester/.noending", None, PathStyle::Unix).unwrap(),
    );

    let probe = probe_workspace_path(&db, &resolver, &policy, "/repo/main").unwrap();
    assert_eq!(probe.status, ProbeStatus::Ok);
    let hint = probe.project.expect("registered path has a Project");
    assert!(hint.known);
    assert_eq!(hint.id.as_deref(), Some("p1"));
    assert_eq!(hint.name.as_deref(), Some("P1"));
    // A missing directory is a legal observation (§42.3-M8): usable, with a warn.
    assert!(!probe.exists);
    assert_eq!(probe.git_state.as_deref(), Some("none"));
}

// ------------------------------------------------------------- recent list

/// The picker's candidates: known paths keep their Project, unknown Session
/// cwds join as candidates in their own right, garbage and reserved cwds stay
/// out, and the order follows the most recent activity per directory.
#[test]
fn recent_paths_union_the_registry_with_session_cwd_history() {
    let (_dir, db) = temp_db();
    let policy = home_policy("/Users/tester/.noending", "/Users/tester");
    db.upsert_project(&Project::new("p1".into(), "P1")).unwrap();
    let main_id = db
        .tx(|tx| insert_workspace_path_conn(tx, &canonical("/repo/main"), "p1"))
        .unwrap();

    // Two sessions in the same unknown directory: one entry, latest activity.
    session(&db, "a1", "/repo/alpha", "2026-03-01T00:00:00+00:00");
    session(&db, "a2", "/repo/alpha", "2026-02-01T00:00:00+00:00");
    // A session working in the known path boosts its activity.
    let mut s3 = session(&db, "m1", "/repo/main", "2026-01-05T00:00:00+00:00");
    s3.workspace_path_id = Some(main_id.clone());
    db.upsert_session(&s3).unwrap();
    // Noise: a relative cwd can never be normalized, and a reserved app path
    // must never be offered as a working directory.
    session(&db, "bad", "relative/x", "2026-06-01T00:00:00+00:00");
    session(
        &db,
        "res",
        "/Users/tester/.noending/data",
        "2026-06-02T00:00:00+00:00",
    );

    let recent = list_recent_workspace_paths(&db, &policy, 10).unwrap();

    let paths: Vec<&str> = recent.iter().map(|r| r.path.as_str()).collect();
    assert!(!paths.iter().any(|p| p.contains("relative")), "{paths:?}");
    assert!(!paths.iter().any(|p| p.contains(".noending")), "{paths:?}");

    let alpha = recent
        .iter()
        .find(|r| r.path == canonical("/repo/alpha"))
        .expect("both alpha sessions collapse into one candidate");
    assert!(!alpha.known);
    assert_eq!(
        alpha.last_used_at.as_deref(),
        Some("2026-03-01T00:00:00+00:00")
    );

    let main = recent
        .iter()
        .find(|r| r.path == canonical("/repo/main"))
        .unwrap();
    assert!(main.known);
    assert_eq!(main.project_name.as_deref(), Some("P1"));

    // Newest activity first: alpha (03-01) outranks main (01-05).
    assert_eq!(recent[0].path, alpha.path);

    // The limit truncates AFTER ranking, so the newest directories survive.
    let recent = list_recent_workspace_paths(&db, &policy, 1).unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].path, alpha.path);
}
