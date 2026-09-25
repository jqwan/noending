//! Fixture constructors shared by the integration tests.
//!
//! They live here rather than on the domain types because no production path
//! builds a whole object by hand: a Project is created by `workspace::project`
//! (naming policy), a Workstream by the Workstream commands, and a Session by
//! observation of an Agent source. These are only ever the *input* to such a
//! path, so the domain stays free of a constructor whose purpose is fixtures.
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

/// An ownerless Session with no workspace facts.
pub fn session(
    id: Id,
    agent: Agent,
    agent_session_id: impl Into<String>,
    raw_path: impl Into<String>,
) -> Session {
    Session {
        id,
        agent,
        agent_session_id: agent_session_id.into(),
        title: None,
        cwd: None,
        workspace_path_id: None,
        project_id: None,
        owner_workstream_id: None,
        raw_path: raw_path.into(),
        parent_agent_session_id: None,
        started_at: None,
        last_activity_at: None,
        trashed_at: None,
    }
}
