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

pub(crate) fn active_names_and_paths(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut statement = conn.prepare(
        "SELECT p.name, wp.canonical_path FROM project_assistants a
         JOIN projects p ON p.id=a.project_id
         JOIN workspace_paths wp ON wp.id=a.workspace_path_id
         WHERE p.visibility='normal'
         ORDER BY p.name COLLATE NOCASE, wp.canonical_path",
    )?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn register(conn: &Connection, path: &WorkspacePath) -> Result<()> {
    conn.execute(
        "INSERT INTO project_assistants(project_id,workspace_path_id) VALUES(?1,?2)",
        params![path.project_id, path.id],
    )?;
    Ok(())
}
