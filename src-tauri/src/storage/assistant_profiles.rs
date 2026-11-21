//! Maps each assistant project to the working directory created for it.
use crate::{domain::WorkspacePath, error::Result};
use rusqlite::{params, Connection, OptionalExtension};

pub fn path_for_project(conn: &Connection, project_id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT workspace_path_id FROM project_assistants WHERE project_id=?1",
            [project_id],
            |row| row.get(0),
        )
        .optional()?)
}

pub fn project_for_path(conn: &Connection, path: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT project_id FROM project_assistants WHERE workspace_path_id=?1",
            [path],
            |row| row.get(0),
        )
        .optional()?)
}

pub fn register(conn: &Connection, path: &WorkspacePath) -> Result<()> {
    conn.execute(
        "INSERT INTO project_assistants(project_id,workspace_path_id) VALUES(?1,?2)",
        params![path.project_id, path.id],
    )?;
    Ok(())
}
