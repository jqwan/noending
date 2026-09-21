//! WorkspacePath / GitIdentity / Project-row persistence (schema v12).
//!
//! These are the *mechanical* operations every workspace layer shares: read a
//! row, insert a row, move a row's Project, and keep the derived Session cache
//! consistent when it moves. The policy that decides WHEN to call them lives in
//! `workspace::project` (方案 §17), so nothing here re-reads the filesystem or
//! runs Git — and nothing here decides ownership, it only applies it.
//!
//! Column note: the schema calls it `exists_on_disk` because `EXISTS` is a
//! SQLite keyword (方案 §42.2-E13); the domain field is `exists`.

use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::domain::*;
use crate::error::{other, Result};

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

// connection-level readers -------------------------------------------------
// Free functions over `&Connection`, so Project policy can run inside the
// caller's transaction (`workspace::project` is called from ingest batches that
// must commit atomically — 方案 §42.3-M3). The `Db` methods delegate here, so
// there stays exactly one SQL path per fact.

pub fn get_workspace_path_conn(conn: &Connection, id: &str) -> Result<Option<WorkspacePath>> {
    Ok(conn
        .query_row(
            &format!("SELECT {WORKSPACE_PATH_COLUMNS} FROM workspace_paths WHERE id = ?1"),
            params![id],
            row_workspace_path,
        )
        .optional()?)
}

pub fn list_workspace_paths_conn(conn: &Connection) -> Result<Vec<WorkspacePath>> {
    let mut st = conn.prepare(&format!(
        "SELECT {WORKSPACE_PATH_COLUMNS} FROM workspace_paths ORDER BY canonical_path"
    ))?;
    let mapped = st.query_map([], row_workspace_path)?;
    Ok(mapped.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub fn list_workspace_paths_for_project_conn(
    conn: &Connection,
    project_id: &str,
) -> Result<Vec<WorkspacePath>> {
    let mut st = conn.prepare(&format!(
        "SELECT {WORKSPACE_PATH_COLUMNS} FROM workspace_paths
          WHERE project_id = ?1 ORDER BY canonical_path"
    ))?;
    let mapped = st.query_map(params![project_id], row_workspace_path)?;
    Ok(mapped.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// §42.3-M7 — the reconcile sweep walks the registry in `path_id` order under a
/// hard cap, so a restart cannot shuffle the pass and move `last_seen_at` across
/// the whole table at once.
pub fn scan_workspace_paths_conn(conn: &Connection, limit: usize) -> Result<Vec<(String, String)>> {
    let mut st =
        conn.prepare("SELECT id, canonical_path FROM workspace_paths ORDER BY id LIMIT ?1")?;
    let mapped = st.query_map(params![limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(mapped.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn row_project_row(r: &Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: r.get("id")?,
        name: r.get("name")?,
        description: r.get("description")?,
        git_id: r.get("git_id")?,
        name_customized: r.get::<_, i64>("name_customized")? != 0,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
    })
}

pub fn get_project_conn(conn: &Connection, id: &str) -> Result<Option<Project>> {
    Ok(conn
        .query_row(
            "SELECT * FROM projects WHERE id = ?1",
            params![id],
            row_project_row,
        )
        .optional()?)
}

/// The Project that owns a Git family, if any. `idx_projects_git_id` is a partial
/// UNIQUE index, so "at most one row" is structural (§8.3): one Git family is one
/// Project, which is what makes two worktrees of one repository converge instead
/// of compete.
pub fn project_by_git_id_conn(conn: &Connection, git_id: &str) -> Result<Option<Project>> {
    Ok(conn
        .query_row(
            "SELECT * FROM projects WHERE git_id = ?1",
            params![git_id],
            row_project_row,
        )
        .optional()?)
}

impl Db {
    pub fn get_workspace_path(&self, id: &str) -> Result<Option<WorkspacePath>> {
        let conn = self.read();
        get_workspace_path_conn(&conn, id)
    }

    pub fn list_workspace_paths(&self) -> Result<Vec<WorkspacePath>> {
        let conn = self.read();
        list_workspace_paths_conn(&conn)
    }

    pub fn list_workspace_paths_for_project(&self, project_id: &str) -> Result<Vec<WorkspacePath>> {
        let conn = self.read();
        list_workspace_paths_for_project_conn(&conn, project_id)
    }

    pub fn count_workspace_paths_for_project(&self, project_id: &str) -> Result<i64> {
        let conn = self.read();
        Ok(conn.query_row(
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

/// §10 — a WorkspacePath is kept only while something references it. Directory
/// existence and Git state are observations, not retention conditions.
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
        let conn = self.read();
        Ok(conn
            .query_row(
                "SELECT * FROM git_identities WHERE id = ?1",
                params![id],
                row_git_identity,
            )
            .optional()?)
    }

    pub fn find_git_identity_by_common_dir(&self, common_dir: &str) -> Result<Option<GitIdentity>> {
        let conn = self.read();
        find_git_identity_by_common_dir_conn(&conn, common_dir)
    }
}

/// Connection-level twin of [`Db::find_git_identity_by_common_dir`], for
/// read-only callers that already hold the reader lock. Same two-step lookup as
/// [`ensure_git_identity_conn`] minus the write: the exact spelling first, then
/// the domain's location relation (方案 §44.6), so a Windows case variant of one
/// repository is still recognized as the family it is.
pub fn find_git_identity_by_common_dir_conn(
    conn: &Connection,
    common_dir: &str,
) -> Result<Option<GitIdentity>> {
    if let Some(row) = conn
        .query_row(
            "SELECT * FROM git_identities WHERE common_dir = ?1",
            params![common_dir],
            row_git_identity,
        )
        .optional()?
    {
        return Ok(Some(row));
    }
    let all: Vec<GitIdentity> = {
        let mut stmt = conn.prepare("SELECT * FROM git_identities ORDER BY id")?;
        let rows = stmt.query_map([], row_git_identity)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(all
        .into_iter()
        .find(|row| crate::workspace::identity::same_location(&row.common_dir, common_dir)))
}

/// `git_identities.id` is app-assigned, NOT derived from `common_dir`: a
/// repository can move and a path hash would silently rename its identity
/// (design doc §8). A replayed call returns the same id because the row is found
/// before it is created — by exact `common_dir` first, then, if that misses, by
/// the domain's location relation (方案 §44.6), which is what stops a Windows
/// case spelling of one repository from becoming two families.
pub fn ensure_git_identity_conn(conn: &Connection, common_dir: &str) -> Result<String> {
    // The exact string first: it is the common case on both platforms and costs
    // one indexed lookup.
    if let Ok(id) = conn.query_row(
        "SELECT id FROM git_identities WHERE common_dir = ?1",
        params![common_dir],
        |r| r.get::<_, String>(0),
    ) {
        return Ok(id);
    }
    // 方案 §44.6 — a miss is not proof of a second repository. On a Windows volume
    // `C:\Code\Repo\.git` and `c:\code\repo\.git` are one directory seen through two
    // spellings, because git echoes back whatever cwd it was called with. Keyed
    // literally they become two `git_identities`, and §8.5 reads that as a family
    // change and moves the WorkspacePath into a second Project — splitting exactly
    // what §8.3 exists to converge. So ask the domain's location question before
    // creating anything. On Unix this reduces to the separator-insensitive string
    // comparison the lookup above already covered, so no macOS identity moves.
    //
    // A scan, deliberately: the table holds one row per repository family (tens),
    // and an index cannot answer a case-folded comparison anyway.
    let existing: Vec<(String, String)> = {
        let mut stmt = conn.prepare("SELECT id, common_dir FROM git_identities ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    if let Some((id, _)) = existing
        .iter()
        .find(|(_, stored)| crate::workspace::identity::same_location(stored, common_dir))
    {
        return Ok(id.clone());
    }

    let ts = now();
    conn.execute(
        "INSERT INTO git_identities (id, common_dir, first_seen_at, last_seen_at, metadata)
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

/// §1.2 — the zero-path test a caller runs after removing a path. The delete
/// itself is [`delete_zero_path_project_conn`] (borrowed connection, caller's
/// transaction, FTS row left for the post-commit `unindex`).
pub fn project_is_unowned(conn: &Connection, project_id: &str) -> Result<bool> {
    let owned: i64 = conn.query_row(
        "SELECT COUNT(*) FROM workspace_paths WHERE project_id = ?1",
        params![project_id],
        |r| r.get(0),
    )?;
    Ok(owned == 0)
}

// ---- Project writes (narrow, one column-set each) -------------------------
//
// `upsert_project_conn` is the *whole-object* write the legacy command used, and
// v0.2 retires it from the product surface: a Project row is now app-owned, so
// every write here changes one documented fact and nothing else. In particular
// no function here can clear `git_id` or `name_customized` — both are one-way.

// Creating a Project lives in `workspace::project` (it needs the naming policy);
// it calls `upsert_project_conn`, the one whole-object write that is safe on a
// row that does not exist yet. Everything below is a narrow, single-fact update.

/// §8.4 — a Project that had no Git family adopts one. `Project.id` does not
/// change, so every Session, WorkstreamPath and audit reference keeps pointing
/// at the same row: this is the whole reason an upgrade is not a merge.
/// Returns false when the Project already carries a family, which is a different
/// decision (§8.5) and must never be taken here.
pub fn adopt_git_identity_conn(conn: &Connection, project_id: &str, git_id: &str) -> Result<bool> {
    Ok(conn.execute(
        "UPDATE projects SET git_id = ?2, updated_at = ?3
          WHERE id = ?1 AND git_id IS NULL",
        params![project_id, git_id, now()],
    )? > 0)
}

/// §17-17 — a merge may hand the survivor a name that came from a user.
/// `customized` is written with the name because the pair is one fact: a name a
/// person chose is a name automatic naming may not touch again.
pub fn set_project_name_conn(
    conn: &Connection,
    project_id: &str,
    name: &str,
    customized: bool,
) -> Result<()> {
    conn.execute(
        "UPDATE projects SET name = ?2, name_customized = ?3, updated_at = ?4 WHERE id = ?1",
        params![project_id, name, customized as i64, now()],
    )?;
    Ok(())
}

/// §17-14/15 — the only user-facing Project name write in v0.2. It sets
/// `name_customized`, which is what stops every later automatic rename
/// (`upsert_project_conn`'s `CASE`, the merge adoption below).
pub fn rename_project_conn(conn: &Connection, project_id: &str, name: &str) -> Result<Project> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(other("项目名称不能为空"));
    }
    let changed = conn.execute(
        "UPDATE projects SET name = ?2, name_customized = 1, updated_at = ?3 WHERE id = ?1",
        params![project_id, name, now()],
    )?;
    if changed == 0 {
        return Err(other(format!("Project {project_id} 不存在")));
    }
    get_project_conn(conn, project_id)?.ok_or_else(|| other(format!("Project {project_id} 不存在")))
}

/// §1.2/§42.3-M4 — delete a Project that no longer owns any WorkspacePath.
///
/// Sessions keep a nullable derived cache, so it is cleared before deleting the
/// now-unowned Project.
/// The FTS row is NOT dropped here: `unindex` belongs after the commit
/// (`ProjectionEffect::projects_deleted` carries the ids), and an un-committed
/// delete must not leave search with a hole if this transaction rolls back.
///
/// A Project that still owns a path is refused rather than silently emptied —
/// §1.2 says a Project exists exactly while it owns one, so reaching this point
/// with paths is a caller bug, and deleting a live Project would orphan the very
/// fact chain everything else reads through.
pub fn delete_zero_path_project_conn(conn: &Connection, project_id: &str) -> Result<bool> {
    if get_project_conn(conn, project_id)?.is_none() {
        return Ok(false);
    }
    if !project_is_unowned(conn, project_id)? {
        return Err(other(format!(
            "Project {project_id} 仍然拥有 WorkspacePath，不能删除（§1.2）"
        )));
    }
    // Referential hygiene, not a write to the derived cache: no Session can
    // still be projecting onto a Project that owns no path.
    super::session_paths::clear_sessions_project_for_project_conn(conn, project_id)?;
    conn.execute("DELETE FROM projects WHERE id = ?1", params![project_id])?;
    Ok(true)
}

/// §1.2 — the zero-path rule as an action: retire a Project the moment it owns
/// nothing. Returns true when it is gone, so callers can record the FTS cleanup.
pub fn retire_project_if_unowned(conn: &Connection, project_id: &str) -> Result<bool> {
    if !project_is_unowned(conn, project_id)? {
        return Ok(false);
    }
    delete_zero_path_project_conn(conn, project_id)
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
    // Distinct `?1..?n` indexes, one per path id: repeating `?1` would make the
    // statement take ONE parameter while the caller supplies N, which rusqlite
    // rejects (`InvalidParameterCount`) — so a batch over two or more paths has to
    // number them.
    let markers = (1..=path_ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
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
