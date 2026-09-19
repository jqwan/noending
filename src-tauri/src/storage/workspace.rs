//! WorkspacePath / GitIdentity persistence (schema v12).
//!
//! These are the *mechanical* operations every workspace layer shares: read a
//! row, insert a row, move a row's Project, and keep the derived Session cache
//! consistent when it moves. The policy that decides WHEN to call them lives in
//! `workspace::project` (方案 §17), so nothing here re-reads the filesystem or
//! runs Git.
//!
//! Column note: the schema calls it `exists_on_disk` because `EXISTS` is a
//! SQLite keyword (方案 §42.2-E13); the domain field is `exists`.

use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::domain::*;
use crate::error::Result;

use super::{new_id, now, Db};

fn row_workspace_path(r: &Row) -> rusqlite::Result<WorkspacePath> {
    Ok(WorkspacePath {
        id: r.get("id")?,
        canonical_path: r.get("canonical_path")?,
        project_id: r.get("project_id")?,
        git_state: r.get("git_state")?,
        git_kind: r.get("git_kind")?,
        exists: r.get::<_, i64>("exists_on_disk")? != 0,
        first_seen_at: r.get("first_seen_at")?,
        last_seen_at: r.get("last_seen_at")?,
    })
}

const WORKSPACE_PATH_COLUMNS: &str =
    "id, canonical_path, project_id, git_state, git_kind, exists_on_disk, first_seen_at, last_seen_at";

impl Db {
    pub fn get_workspace_path(&self, id: &str) -> Result<Option<WorkspacePath>> {
        Ok(self
            .0
            .query_row(
                &format!("SELECT {WORKSPACE_PATH_COLUMNS} FROM workspace_paths WHERE id = ?1"),
                params![id],
                row_workspace_path,
            )
            .optional()?)
    }

    pub fn get_workspace_path_by_canonical(
        &self,
        canonical: &str,
    ) -> Result<Option<WorkspacePath>> {
        Ok(self
            .0
            .query_row(
                &format!("SELECT {WORKSPACE_PATH_COLUMNS} FROM workspace_paths WHERE canonical_path = ?1"),
                params![canonical],
                row_workspace_path,
            )
            .optional()?)
    }

    pub fn list_workspace_paths(&self) -> Result<Vec<WorkspacePath>> {
        let mut st = self.0.prepare(&format!(
            "SELECT {WORKSPACE_PATH_COLUMNS} FROM workspace_paths ORDER BY canonical_path"
        ))?;
        let mapped = st.query_map([], row_workspace_path)?;
        Ok(mapped.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn list_workspace_paths_for_project(&self, project_id: &str) -> Result<Vec<WorkspacePath>> {
        let mut st = self.0.prepare(&format!(
            "SELECT {WORKSPACE_PATH_COLUMNS} FROM workspace_paths
              WHERE project_id = ?1 ORDER BY canonical_path"
        ))?;
        let mapped = st.query_map(params![project_id], row_workspace_path)?;
        Ok(mapped.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn count_workspace_paths_for_project(&self, project_id: &str) -> Result<i64> {
        Ok(self.0.query_row(
            "SELECT COUNT(*) FROM workspace_paths WHERE project_id = ?1",
            params![project_id],
            |r| r.get(0),
        )?)
    }
}

/// Insert a WorkspacePath for an already-normalized path.
///
/// `id` is derived from `canonical_path` by `workspace::path_identity`, so this
/// is idempotent on both unique keys and safe to call again after a partial
/// failure. An existing row is NEVER re-projected here: ownership changes go
/// through [`reassign_workspace_path_project_conn`], which also repairs the
/// derived Session cache. A silent re-projection here is what would let two
/// code paths disagree about which Project owns a path.
pub fn insert_workspace_path_conn(
    conn: &Connection,
    canonical_path: &str,
    project_id: &str,
) -> Result<String> {
    let id = crate::workspace::path_identity(canonical_path);
    let ts = now();
    conn.execute(
        "INSERT OR IGNORE INTO workspace_paths
           (id, canonical_path, project_id, git_state, git_kind, exists_on_disk, first_seen_at, last_seen_at)
         VALUES (?1, ?2, ?3, 'none', NULL, 0, ?4, ?4)",
        params![id, canonical_path, project_id, ts],
    )?;
    Ok(id)
}

/// Refresh the observed facts of an existing path. Git evidence and existence
/// belong to the WorkspacePath; this is the only writer of either.
pub fn update_workspace_path_observation_conn(
    conn: &Connection,
    path_id: &str,
    exists: bool,
    git_state: &str,
    git_kind: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE workspace_paths
            SET exists_on_disk = ?2, git_state = ?3, git_kind = ?4, last_seen_at = ?5
          WHERE id = ?1",
        params![path_id, exists as i64, git_state, git_kind, now()],
    )?;
    Ok(())
}

/// §1.12/§42.3-M3 — move a path to another Project and refresh every derived
/// cache that reads through it, in one transaction.
///
/// `git_id` on the *Project* side and the merge choice are policy
/// (`workspace::project`); this is the single mechanical door.
pub fn reassign_workspace_path_project_conn(
    conn: &Connection,
    path_id: &str,
    new_project_id: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE workspace_paths SET project_id = ?2, last_seen_at = ?3 WHERE id = ?1",
        params![path_id, new_project_id, now()],
    )?;
    super::session_paths::refresh_sessions_project_for_path_conn(conn, path_id)?;
    refresh_workstream_search_parents_conn(conn, &[path_id.to_string()])?;
    Ok(())
}

/// §10 — a WorkspacePath is a physical fact worth keeping only while something
/// references it. A missing directory or a lost `.git` sets flags instead; this
/// never runs then.
pub fn workspace_path_is_gcable(conn: &Connection, path_id: &str) -> Result<bool> {
    let refs: i64 = conn.query_row(
        "SELECT (SELECT COUNT(*) FROM sessions WHERE workspace_path_id = ?1)
              + (SELECT COUNT(*) FROM workstream_paths WHERE workspace_path_id = ?1)",
        params![path_id],
        |r| r.get(0),
    )?;
    Ok(refs == 0)
}

/// Delete a WorkspacePath the caller has already proven GC-eligible. Returns
/// its former Project so the caller can run the zero-path check.
pub fn delete_workspace_path_conn(conn: &Connection, path_id: &str) -> Result<Option<String>> {
    let project: Option<String> = conn
        .query_row(
            "SELECT project_id FROM workspace_paths WHERE id = ?1",
            params![path_id],
            |r| r.get(0),
        )
        .optional()?;
    conn.execute(
        "DELETE FROM workspace_paths WHERE id = ?1",
        params![path_id],
    )?;
    Ok(project)
}

// ---- Git identity -------------------------------------------------------

fn row_git_identity(r: &Row) -> rusqlite::Result<GitIdentity> {
    Ok(GitIdentity {
        id: r.get("id")?,
        common_dir: r.get("common_dir")?,
        first_seen_at: r.get("first_seen_at")?,
        last_seen_at: r.get("last_seen_at")?,
        metadata: serde_json::from_str(&r.get::<_, String>("metadata")?).unwrap_or_default(),
    })
}

impl Db {
    pub fn get_git_identity(&self, id: &str) -> Result<Option<GitIdentity>> {
        Ok(self
            .0
            .query_row(
                "SELECT * FROM git_identities WHERE id = ?1",
                params![id],
                row_git_identity,
            )
            .optional()?)
    }

    pub fn find_git_identity_by_common_dir(&self, common_dir: &str) -> Result<Option<GitIdentity>> {
        Ok(self
            .0
            .query_row(
                "SELECT * FROM git_identities WHERE common_dir = ?1",
                params![common_dir],
                row_git_identity,
            )
            .optional()?)
    }
}

/// `git_identities.id` is app-assigned, NOT derived from `common_dir`: a
/// repository can move and a path hash would silently rename its identity
/// (design doc §8). Idempotency comes from `common_dir UNIQUE` +
/// `INSERT OR IGNORE` + re-read, which is also what makes a replayed call
/// return the same id.
pub fn ensure_git_identity_conn(conn: &Connection, common_dir: &str) -> Result<String> {
    let ts = now();
    conn.execute(
        "INSERT OR IGNORE INTO git_identities (id, common_dir, first_seen_at, last_seen_at, metadata)
         VALUES (?1, ?2, ?3, ?3, '{}')",
        params![new_id(), common_dir, ts],
    )?;
    Ok(conn.query_row(
        "SELECT id FROM git_identities WHERE common_dir = ?1",
        params![common_dir],
        |r| r.get(0),
    )?)
}

pub fn touch_git_identity_conn(conn: &Connection, git_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE git_identities SET last_seen_at = ?2 WHERE id = ?1",
        params![git_id, now()],
    )?;
    Ok(())
}

/// §35 — the zero-path test a caller runs after removing a path. Deletion itself
/// goes through `Db::delete_project_and_children` (which owns its transaction
/// and the FTS un-index), never from inside a borrowed `tx`, so the FK ordering
/// and the index cleanup live in exactly one place.
pub fn project_is_unowned(conn: &Connection, project_id: &str) -> Result<bool> {
    let owned: i64 = conn.query_row(
        "SELECT COUNT(*) FROM workspace_paths WHERE project_id = ?1",
        params![project_id],
        |r| r.get(0),
    )?;
    Ok(owned == 0)
}

/// §42.3-M18 — a Workstream's search row is parented by its PRIMARY-path
/// Project, so any change behind `path_ids` can move it. Done as one SQL
/// statement pair so there is no Rust-side round trip to get wrong; a no-op when
/// the bundled SQLite has no FTS5 (`search_index` absent).
pub fn refresh_workstream_search_parents_conn(
    conn: &Connection,
    path_ids: &[String],
) -> Result<()> {
    if path_ids.is_empty() {
        return Ok(());
    }
    if conn
        .query_row("SELECT 1 FROM search_index LIMIT 1", [], |_| Ok(()))
        .is_err()
    {
        return Ok(());
    }
    let markers = vec!["?1"; path_ids.len()].join(",");
    let affected = format!(
        "SELECT DISTINCT workstream_id FROM workstream_paths WHERE workspace_path_id IN ({markers})"
    );
    let args: Vec<&dyn rusqlite::types::ToSql> = path_ids
        .iter()
        .map(|s| s as &dyn rusqlite::types::ToSql)
        .collect();
    conn.execute(
        &format!("DELETE FROM search_index WHERE kind = 'workstream' AND ref_id IN ({affected})"),
        rusqlite::params_from_iter(args.iter()),
    )?;
    conn.execute(
        &format!(
            "INSERT INTO search_index (kind, ref_id, parent_id, title, body)
             SELECT 'workstream', w.id,
                    (SELECT wp.project_id FROM workstream_paths wsp
                       JOIN workspace_paths wp ON wp.id = wsp.workspace_path_id
                      WHERE wsp.workstream_id = w.id AND wsp.position = 0),
                    w.title, w.description
             FROM workstreams w
             WHERE w.id IN ({affected})"
        ),
        rusqlite::params_from_iter(args.iter()),
    )?;
    Ok(())
}
