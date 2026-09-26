//! Fixture constructors shared by the integration tests.
//!
//! They live here rather than on the domain types because no production path
//! builds a whole object by hand: a Project is created by `workspace::project`
//! (naming policy), a Workstream by the Workstream commands, and a Session by
//! resolution of a discovered ROOT member. These are only ever the *input* to
//! such a path, so the domain stays free of a constructor whose purpose is
//! fixtures.
#![allow(dead_code)] // each test binary uses a subset

use noending::domain::{Agent, Id, Project, Session, Workstream};

/// A path-backed, app-named Project.
pub fn project(id: Id, name: impl Into<String>) -> Project {
    let ts = noending::storage::now();
    Project {
        id,
        name: name.into(),
        description: String::new(),
        git_id: None,
        name_customized: false,
        created_at: ts.clone(),
        updated_at: ts,
    }
}

/// A title-only Workstream: no paths, active, visible.
pub fn workstream(id: Id, title: impl Into<String>) -> Workstream {
    let ts = noending::storage::now();
    Workstream {
        id,
        title: title.into(),
        description: String::new(),
        lifecycle: noending::domain::workstream_lifecycle::ACTIVE.into(),
        visibility: noending::domain::workstream_visibility::NORMAL.into(),
        created_at: ts.clone(),
        updated_at: ts,
    }
}

/// An ownerless Logical Session with no workspace facts. `root_agent_session_id`
/// is the ROOT member's Agent-side Resume identity; the `raw_path` argument of
/// the old fixture is gone — sources live on member rows now.
pub fn session(id: Id, agent: Agent, root_agent_session_id: impl Into<String>) -> Session {
    Session {
        id,
        agent,
        root_agent_session_id: root_agent_session_id.into(),
        title: None,
        cwd: None,
        workspace_path_id: None,
        project_id: None,
        owner_workstream_id: None,
        forked_from_session_id: None,
        started_at: None,
        last_activity_at: None,
        last_conversation_at: None,
        trashed_at: None,
    }
}

/// Ensure the logical session row exists (keyed by root identity) and return
/// it — the way every integration test creates a Session now, mirroring what
/// `ingestion::ensure_logical_session` does for real discoveries.
pub fn ensure_session(
    db: &noending::storage::Db,
    id: Id,
    agent: Agent,
    root_agent_session_id: impl Into<String>,
) -> Session {
    let root_id = root_agent_session_id.into();
    let (row_id, _) = db
        .upsert_logical_session_unchecked(agent, &root_id, None, None, None, None, None, None)
        .expect("ensure logical session");
    let _ = id; // the row id is store-assigned; callers use the returned Session
    db.get_session(&row_id).unwrap().expect("session row")
}

/// Ensure the ROOT member row of a logical session (member identity = the
/// session's root id), pointing at `source_path`.
pub fn ensure_root_member(
    db: &noending::storage::Db,
    session_id: &str,
    agent: Agent,
    root_agent_session_id: &str,
    source_path: &str,
) -> String {
    db.upsert_session_member(
        session_id,
        agent,
        root_agent_session_id,
        noending::domain::SessionMemberRelation::Root,
        None,
        "test_root",
        source_path,
        None,
        None,
        None,
        &serde_json::json!({}),
    )
    .expect("ensure root member")
}

/// A parsed root-conversation message, ready for `commit_member_ingest`.
pub fn parsed_message(
    source_message_id: impl Into<String>,
    role: noending::domain::SessionMessageRole,
    content: impl Into<String>,
) -> noending::domain::ParsedSessionMessage {
    noending::domain::ParsedSessionMessage {
        source_message_id: Some(source_message_id.into()),
        source_position: String::new(),
        ts: None,
        role,
        content: content.into(),
        provider: None,
        model: None,
    }
}

/// Seed a logical session + root member + conversation messages through the
/// production commit path (`commit_member_ingest`), so a fixture conversation
/// satisfies the same invariants a real ingest does. Returns (session row,
/// root member id, stored messages).
pub fn seed_conversation(
    db: &noending::storage::Db,
    agent: Agent,
    root_agent_session_id: &str,
    messages: &[noending::domain::ParsedSessionMessage],
) -> (Session, String, Vec<noending::domain::SessionMessage>) {
    let (session_id, _) = db
        .upsert_logical_session_unchecked(
            agent,
            root_agent_session_id,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("seed: ensure logical session");
    let member_id = ensure_root_member(db, &session_id, agent, root_agent_session_id, "/tmp/seed");
    let stored = db
        .commit_member_ingest(&session_id, &member_id, messages, None, &seed_source(0))
        .expect("seed: commit member ingest");
    let session = db.get_session(&session_id).unwrap().expect("seed: session");
    (session, member_id, stored)
}

/// A replay-shaped source update (full scan from genesis).
pub fn seed_source(generation: i64) -> noending::domain::SourceCursorUpdate {
    noending::domain::SourceCursorUpdate {
        file_identity: format!("seed-identity-{generation}"),
        generation,
        byte_offset: 100,
        last_seen_size: 100,
        mtime: None,
        start_byte_offset: 0,
        prefix_hash: String::new(),
    }
}
