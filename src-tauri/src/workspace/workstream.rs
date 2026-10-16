//! Explicit task/project memberships and session tracking.
//!
//! Visibility (`normal | archived`) is the only task state. Archive/restore
//! preserve projects, tracked sessions and summary. Permanent deletion requires
//! archive state, removes task-owned data and tracking, and preserves sessions
//! and physical paths. Project membership does not import sessions or copy paths.

use serde::Serialize;

use crate::domain::*;
use crate::error::{other, Result};
use crate::storage::workstream_projects::{project_ids_for_workstream, purge_workstream_data_conn};
use crate::storage::{new_id, now, Db};

use super::WorkspaceAttaching;

/// Managed Tauri state carrying the one runtime implementation of
/// [`WorkspaceAttaching`] (`workspace::wiring::WorkspaceLayer`).
///
/// Path probing and launcher commands reuse this shared workspace service.
pub struct PathService {
    attaching: std::sync::Arc<dyn WorkspaceAttaching + Send + Sync>,
}

impl PathService {
    pub fn new(attaching: std::sync::Arc<dyn WorkspaceAttaching + Send + Sync>) -> Self {
        Self { attaching }
    }

    pub fn attaching(&self) -> &dyn WorkspaceAttaching {
        &*self.attaching
    }
}

// Creation and project membership
pub fn create_workstream(
    db: &Db,
    title: &str,
    description: &str,
    project_ids: &[String],
) -> Result<Workstream> {
    crate::storage::ensure_not_empty("任务标题", title)?;
    let task = Workstream {
        id: new_id(),
        title: title.trim().into(),
        description: description.trim().into(),
        visibility: "normal".into(),
        created_at: now(),
        updated_at: now(),
    };
    db.tx(|tx| {
        crate::storage::upsert_workstream_conn(tx, &task)?;
        crate::storage::workstream_projects::set_projects_conn(tx, &task.id, project_ids)?;
        Ok(())
    })?;
    Ok(task)
}
pub fn set_workstream_projects(db: &Db, id: &str, projects: &[String]) -> Result<()> {
    db.tx(|tx| {
        crate::storage::workstream_projects::set_projects_conn(tx, id, projects)?;
        tx.execute(
            "UPDATE workstreams SET updated_at=?2 WHERE id=?1",
            rusqlite::params![id, now()],
        )?;
        Ok(())
    })
}

// Archive state

/// Archive preserves projects, summary and tracked sessions.
/// Archived tasks cannot create sessions.
///
/// Absolute, not a toggle: a retry or double click must not quietly
/// un-archive the Workstream.
pub fn archive_workstream(db: &Db, workstream_id: &str) -> Result<Workstream> {
    set_visibility(db, workstream_id, workstream_visibility::ARCHIVED)
}

/// Unarchive without changing task data or associations.
pub fn restore_workstream(db: &Db, workstream_id: &str) -> Result<Workstream> {
    set_visibility(db, workstream_id, workstream_visibility::NORMAL)
}

fn set_visibility(db: &Db, workstream_id: &str, visibility: &str) -> Result<Workstream> {
    edit_fields(db, workstream_id, None, None, Some(visibility))
}

/// Partial edits preserve every field not explicitly supplied, including
/// archive state. Read, write, revision and search updates share a transaction.
pub fn update_workstream_fields(
    db: &Db,
    id: &str,
    title: Option<&str>,
    description: Option<&str>,
) -> Result<Workstream> {
    if title.is_none() && description.is_none() {
        return Err(other("需要提供任务标题或说明"));
    }
    if let Some(title) = title {
        crate::storage::ensure_not_empty("任务标题", title)?;
    }
    edit_fields(db, id, title, description, None)
}

fn edit_fields(
    db: &Db,
    id: &str,
    title: Option<&str>,
    description: Option<&str>,
    visibility: Option<&str>,
) -> Result<Workstream> {
    db.tx(|tx| {
        let mut w =
            crate::storage::get_workstream_conn(tx, id)?.ok_or_else(|| other("任务不存在"))?;
        if let Some(title) = title {
            w.title = title.trim().into();
        }
        if let Some(description) = description {
            w.description = description.into();
        }
        if let Some(visibility) = visibility {
            w.visibility = visibility.into();
        }
        w.updated_at = now();
        crate::storage::upsert_workstream_conn(tx, &w)?;

        Ok(w)
    })
}

/// The rules for the whole-object write (`update_workstream`), which the
/// detail page still uses to save a title or a description.
///
/// `visibility` may not travel through it: it has commands of
/// their own, and a stale object from another screen silently reverting an
/// archive is exactly the double-authority failure this rule exists to remove.
pub fn apply_whole_object_edit(db: &Db, payload: &Workstream) -> Result<Workstream> {
    let current = require_workstream(db, &payload.id)?;
    if payload.visibility != current.visibility {
        return Err(other(
            "归档与恢复请用 archive_workstream / restore_workstream",
        ));
    }
    Ok(Workstream {
        created_at: current.created_at,
        updated_at: now(),
        ..payload.clone()
    })
}

/// The only irreversible action in this module, and the reason it is gated on
/// `archived`: the user must have already moved the Workstream out of the way,
/// and the recycle bin is where "delete for real" is offered.
///
/// Deleting the Workstream ends its Context. That is the point of the action,
/// and the only part that is not recoverable from a backup — which is why
/// archive is a prerequisite rather than a courtesy.
pub fn delete_workstream_permanently(db: &Db, workstream_id: &str) -> Result<()> {
    let w = require_workstream(db, workstream_id)?;
    if w.visibility != workstream_visibility::ARCHIVED {
        return Err(other("只能永久删除已归档任务"));
    }
    db.tx(|tx| purge_workstream_data_conn(tx, workstream_id))?;
    Ok(())
}

// Projections

/// A task's explicit project associations, without directory snapshots or roles.
#[derive(Debug, Clone, Serialize)]
pub struct WorkstreamProject {
    pub id: Id,
    pub name: String,
}

pub fn projects_for_workstream(db: &Db, workstream_id: &str) -> Result<Vec<WorkstreamProject>> {
    let ids = project_ids_for_workstream(&db.read(), workstream_id)?;
    let mut projects = Vec::new();
    for id in ids {
        if let Some(project) = db.get_project(&id)? {
            projects.push(WorkstreamProject {
                id,
                name: project.name,
            });
        }
    }
    projects.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    Ok(projects)
}

// Helpers

fn require_workstream(db: &Db, workstream_id: &str) -> Result<Workstream> {
    db.get_workstream(workstream_id)?
        .ok_or_else(|| other("Workstream 不存在"))
}
