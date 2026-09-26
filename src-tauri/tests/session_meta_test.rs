//! Session row metadata under the Logical Session model (重构方案 §4/§10).
//!
//! A Session row is keyed by `(agent, root_agent_session_id)` — the ROOT
//! member's real Resume identity — and follows the source through
//! `upsert_logical_session`:
//! - re-discovery of the same root updates the row in place (never duplicates);
//! - cwd / workspace / activity follow the ROOT (child facts never reach here);
//! - the title is write-once: a later pass may fill an absent title but never
//!   overwrite one (§4.2, the authority is the root's title chain);
//! - `project_id` is derived from `workspace_path_id` inside the statement, so
//!   the cache can never drift from its source.

use std::path::PathBuf;

use noending::domain::{Agent, SessionMemberRelation};
use noending::storage::workspace::insert_workspace_path_conn;
use noending::storage::{new_id, Db};

mod support;

fn db(tag: &str) -> Db {
    let dir = std::env::temp_dir().join(format!("noending-session-meta-{}-{}", tag, new_id()));
    Db::open(&dir.join("test.db")).unwrap()
}

/// Ensure the logical session through the same upsert discovery uses, then
/// return the stored row.
fn ensure(
    db: &Db,
    agent: Agent,
    root_id: &str,
    cwd: Option<&str>,
    activity: &str,
) -> noending::domain::Session {
    let (row_id, _) = db
        .upsert_logical_session_unchecked(
            agent,
            root_id,
            Some("帮我看看这个量化脚本"),
            cwd,
            None,
            None,
            Some("2026-08-18T14:21:22Z"),
            Some(activity),
        )
        .unwrap();
    db.get_session(&row_id).unwrap().expect("session row")
}

#[test]
fn upsert_follows_the_root_identity_in_place() {
    let database = db("identity");
    let root_id = format!("meta-{}", new_id());

    let first = ensure(
        &database,
        Agent::ClaudeCode,
        &root_id,
        None,
        "2026-08-18T14:30:00Z",
    );
    assert_eq!(
        first.root_agent_session_id, root_id,
        "the row is keyed by the ROOT's Resume identity"
    );

    // Re-discovery of the same root: same row, not a duplicate.
    let (second_id, is_new) = database
        .upsert_logical_session_unchecked(
            Agent::ClaudeCode,
            &root_id,
            Some("帮我看看这个量化脚本"),
            None,
            None,
            None,
            None,
            Some("2026-08-19T09:00:00Z"),
        )
        .unwrap();
    assert!(!is_new);
    assert_eq!(second_id, first.id, "same root identity, same row");
    assert_eq!(
        database
            .find_session_by_root_agent_id(Agent::ClaudeCode, &root_id)
            .unwrap()
            .expect("row")
            .id,
        first.id,
        "the root identity is THE lookup for Resume and matching"
    );

    // A different root is a different Logical Session.
    let (other_id, is_new) = database
        .upsert_logical_session_unchecked(
            Agent::ClaudeCode,
            "another-root",
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
    assert!(is_new);
    assert_ne!(other_id, first.id);
}

#[test]
fn a_later_upsert_refreshes_cwd_and_activity() {
    let database = db("refresh");
    let root_id = format!("meta-{}", new_id());

    let first = ensure(
        &database,
        Agent::ClaudeCode,
        &root_id,
        Some("/Users/jqk/projects/r/stock/quant"),
        "2026-08-18T14:30:00Z",
    );

    // Discovery re-read the root transcript: the cwd moved and activity
    // advanced, and the row follows.
    let (second_id, _) = database
        .upsert_logical_session_unchecked(
            Agent::ClaudeCode,
            &root_id,
            Some("帮我看看这个量化脚本"),
            Some("/Users/jqk/projects/r/stock_quant"),
            None,
            None,
            None,
            Some("2026-08-19T09:00:00Z"),
        )
        .unwrap();
    assert_eq!(second_id, first.id);
    let stored = database.get_session(&first.id).unwrap().unwrap();
    assert_eq!(
        stored.cwd.as_deref(),
        Some("/Users/jqk/projects/r/stock_quant")
    );
    assert_eq!(
        stored.last_activity_at.as_deref(),
        Some("2026-08-19T09:00:00Z")
    );
    assert_eq!(
        stored.started_at.as_deref(),
        Some("2026-08-18T14:21:22Z"),
        "started_at is filled once and never rewritten"
    );
}

#[test]
fn upsert_without_cwd_keeps_stored_value() {
    let database = db("keep");
    let root_id = format!("meta-{}", new_id());

    ensure(
        &database,
        Agent::ClaudeCode,
        &root_id,
        Some("/Users/jqk/projects/r/stock_quant"),
        "2026-08-18T14:30:00Z",
    );

    // A later scan that fails to read cwd (parse gap) must not wipe the
    // stored value — the absence of a fact is not a new fact.
    let (_, _) = database
        .upsert_logical_session_unchecked(
            Agent::ClaudeCode,
            &root_id,
            Some("帮我看看这个量化脚本"),
            None,
            None,
            None,
            None,
            Some("2026-08-18T14:30:00Z"),
        )
        .unwrap();

    let stored = database
        .find_session_by_root_agent_id(Agent::ClaudeCode, &root_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.cwd.as_deref(),
        Some("/Users/jqk/projects/r/stock_quant")
    );
}

/// §4.2 — the title is write-once: a discovery that finds a title source
/// fills an absent title, and no later pass may overwrite it.
#[test]
fn title_is_write_once() {
    let database = db("title");
    let root_id = format!("meta-{}", new_id());

    // Row created without a title (no usable title source yet).
    let (row_id, _) = database
        .upsert_logical_session_unchecked(
            Agent::ClaudeCode,
            &root_id,
            None,
            Some("/tmp/w"),
            None,
            None,
            None,
            Some("2026-08-18T14:30:00Z"),
        )
        .unwrap();
    let first = database.get_session(&row_id).unwrap().unwrap();
    assert!(first.title.is_none());

    // The title source appears on the next pass: the absent title is filled.
    database
        .upsert_logical_session_unchecked(
            Agent::ClaudeCode,
            &root_id,
            Some("帮我看看这个量化脚本"),
            Some("/tmp/w"),
            None,
            None,
            None,
            Some("2026-08-18T14:31:00Z"),
        )
        .unwrap();
    let filled = database.get_session(&row_id).unwrap().unwrap();
    assert_eq!(filled.title.as_deref(), Some("帮我看看这个量化脚本"));

    // A later source (or a source that changed its mind) never overwrites.
    database
        .upsert_logical_session_unchecked(
            Agent::ClaudeCode,
            &root_id,
            Some("一个全新的标题"),
            Some("/tmp/w"),
            None,
            None,
            None,
            Some("2026-08-18T14:32:00Z"),
        )
        .unwrap();
    let stored = database.get_session(&row_id).unwrap().unwrap();
    assert_eq!(
        stored.title.as_deref(),
        Some("帮我看看这个量化脚本"),
        "an existing title is the authority — write-once"
    );
}

#[test]
fn unchanged_upsert_rewrites_nothing() {
    let database = db("stable");
    let root_id = format!("meta-{}", new_id());

    let first = ensure(
        &database,
        Agent::ClaudeCode,
        &root_id,
        Some("/Users/jqk/projects/r/stock_quant"),
        "2026-08-18T14:30:00Z",
    );

    // Identical scan: row must come back byte-identical (same id, no churn).
    let (again, is_new) = database
        .upsert_logical_session_unchecked(
            Agent::ClaudeCode,
            &root_id,
            Some("帮我看看这个量化脚本"),
            Some("/Users/jqk/projects/r/stock_quant"),
            None,
            None,
            Some("2026-08-18T14:21:22Z"),
            Some("2026-08-18T14:30:00Z"),
        )
        .unwrap();
    assert!(!is_new);
    assert_eq!(again, first.id);
    let stored = database.get_session(&first.id).unwrap().unwrap();
    assert_eq!(stored.cwd, first.cwd);
    assert_eq!(stored.last_activity_at, first.last_activity_at);
    assert_eq!(stored.started_at, first.started_at);
    assert_eq!(stored.title, first.title);
}

/// The derived Project cache: `project_id` is read off
/// `workspace_paths.project_id` inside the upsert statement, so a Session is a
/// Project member exactly while its own path says so (§1.10).
#[test]
fn project_cache_follows_workspace_path() {
    let database = db("project-cache");
    database
        .upsert_project(&support::project("p-meta".into(), "Meta"))
        .unwrap();
    let path_id = {
        let conn = database.write();
        insert_workspace_path_conn(&conn, "/repo/meta", "p-meta").unwrap()
    };
    let root_id = format!("meta-{}", new_id());

    let (row_id, _) = database
        .upsert_logical_session_unchecked(
            Agent::ClaudeCode,
            &root_id,
            Some("t"),
            Some("/repo/meta"),
            Some(&path_id),
            None,
            None,
            None,
        )
        .unwrap();
    let stored = database.get_session(&row_id).unwrap().unwrap();
    assert_eq!(stored.workspace_path_id.as_deref(), Some(path_id.as_str()));
    assert_eq!(
        stored.project_id.as_deref(),
        Some("p-meta"),
        "the cache is derived in the same statement, never set apart"
    );

    // A later upsert with no path leaves the attachment alone.
    database
        .upsert_logical_session_unchecked(
            Agent::ClaudeCode,
            &root_id,
            Some("t"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
    let after = database.get_session(&row_id).unwrap().unwrap();
    assert_eq!(after.workspace_path_id.as_deref(), Some(path_id.as_str()));
    assert_eq!(after.project_id.as_deref(), Some("p-meta"));
}

/// A child member's execution identity never resolves as a root: only the
/// session's `root_agent_session_id` is the Resume/lookup authority (§4.1).
#[test]
fn a_child_member_identity_never_resolves_as_a_root() {
    let database = db("child-not-root");
    let root_id = format!("root-{}", new_id());
    let session =
        support::ensure_session(&database, format!("s-{}", new_id()), Agent::Codex, &root_id);

    // A child of that session, with its own source identity.
    let child_source_id = format!("child-{}", new_id());
    database
        .upsert_session_member(
            &session.id,
            Agent::Codex,
            &child_source_id,
            SessionMemberRelation::Child,
            Some(&root_id),
            "subagent_file",
            "/tmp/child.jsonl",
            Some("/child/only/cwd"),
            None,
            None,
            &serde_json::json!({}),
        )
        .unwrap();

    assert!(
        database
            .find_session_by_root_agent_id(Agent::Codex, &child_source_id)
            .unwrap()
            .is_none(),
        "a child's source id must never look up the Logical Session"
    );
    // ...and the child's cwd never flowed up onto the Session row (§4.1/§18).
    let stored = database.get_session(&session.id).unwrap().unwrap();
    assert_eq!(stored.cwd, None);
    assert_eq!(stored.project_id, None);
    let _ = PathBuf::new();
}
