//! SQLite storage: schema, migrations and repository helpers.
//! Owns: domain data, session index, cursors, context items, FTS search.

use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::domain::*;
use crate::error::{other, Result};

pub struct Db(pub Connection);

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

impl Db {
    pub fn open(path: &std::path::Path) -> Result<Db> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Db(conn);
        db.migrate()?;
        Ok(db)
    }

    pub fn conn(&self) -> &Connection {
        &self.0
    }

    fn migrate(&self) -> Result<()> {
        self.0.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS projects (
              id TEXT PRIMARY KEY,
              name TEXT NOT NULL,
              description TEXT NOT NULL DEFAULT '',
              archived INTEGER NOT NULL DEFAULT 0,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );            CREATE TABLE IF NOT EXISTS project_resources (
              id TEXT PRIMARY KEY,
              project_id TEXT NOT NULL REFERENCES projects(id),
              kind TEXT NOT NULL,
              uri TEXT,
              metadata TEXT NOT NULL DEFAULT '{}',
              created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS workstreams (
              id TEXT PRIMARY KEY,
              project_id TEXT NOT NULL REFERENCES projects(id),
              title TEXT NOT NULL,
              description TEXT NOT NULL DEFAULT '',
              lifecycle TEXT NOT NULL DEFAULT 'open',
              visibility TEXT NOT NULL DEFAULT 'normal',
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS sessions (
              id TEXT PRIMARY KEY,
              agent TEXT NOT NULL,
              agent_session_id TEXT NOT NULL,
              title TEXT,
              cwd TEXT,
              project_id TEXT REFERENCES projects(id),
              raw_path TEXT NOT NULL,
              parent_agent_session_id TEXT,
              started_at TEXT,
              last_activity_at TEXT,
              UNIQUE(agent, agent_session_id)
            );
            CREATE TABLE IF NOT EXISTS session_events (
              session_id TEXT NOT NULL REFERENCES sessions(id),
              sequence INTEGER NOT NULL,
              ts TEXT,
              kind TEXT NOT NULL,
              text TEXT,
              raw_ref TEXT NOT NULL,
              metadata TEXT NOT NULL DEFAULT '{}',
              PRIMARY KEY (session_id, sequence)
            );
            CREATE TABLE IF NOT EXISTS session_cursors (
              session_id TEXT PRIMARY KEY REFERENCES sessions(id),
              last_sequence INTEGER NOT NULL DEFAULT 0,
              last_seen_size INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS session_workstream_bindings (
              session_id TEXT NOT NULL REFERENCES sessions(id),
              workstream_id TEXT NOT NULL REFERENCES workstreams(id),
              role TEXT NOT NULL DEFAULT 'related',
              last_seen_revision TEXT,
              last_sync_cursor INTEGER NOT NULL DEFAULT 0,
              created_at TEXT NOT NULL,
              last_used_at TEXT NOT NULL,
              PRIMARY KEY (session_id, workstream_id)
            );
            CREATE TABLE IF NOT EXISTS context_items (
              id TEXT PRIMARY KEY,
              workstream_id TEXT NOT NULL REFERENCES workstreams(id),
              kind TEXT NOT NULL,
              status TEXT NOT NULL DEFAULT 'active',
              authority TEXT NOT NULL DEFAULT 'system_observed',
              current_revision_id TEXT,
              supersedes_item_id TEXT,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS context_item_revisions (
              id TEXT PRIMARY KEY,
              item_id TEXT NOT NULL REFERENCES context_items(id),
              title TEXT NOT NULL,
              content TEXT NOT NULL DEFAULT '',
              metadata TEXT NOT NULL DEFAULT '{}',
              source_type TEXT,
              source_ref TEXT,
              sync_run_id TEXT,
              created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS sync_runs (
              id TEXT PRIMARY KEY,
              session_id TEXT NOT NULL,
              from_sequence INTEGER NOT NULL,
              to_sequence INTEGER NOT NULL,
              status TEXT NOT NULL,
              mutations TEXT NOT NULL DEFAULT '[]',
              summary TEXT NOT NULL DEFAULT '',
              error TEXT,
              created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS agent_installations (
              agent TEXT PRIMARY KEY,
              executable_path TEXT,
              version TEXT,
              source TEXT,
              last_verified_at TEXT
            );
            CREATE TABLE IF NOT EXISTS settings (
              key TEXT PRIMARY KEY,
              value TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_workstreams_project ON workstreams(project_id);
            CREATE INDEX IF NOT EXISTS idx_sessions_project ON sessions(project_id);
            CREATE INDEX IF NOT EXISTS idx_items_workstream ON context_items(workstream_id);
            CREATE INDEX IF NOT EXISTS idx_bindings_ws ON session_workstream_bindings(workstream_id);
            "#,
        )?;

        self.0.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS assistant_sessions (
              id TEXT PRIMARY KEY,
              title TEXT NOT NULL DEFAULT 'Assistant',
              created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS assistant_messages (
              id TEXT PRIMARY KEY,
              session_id TEXT NOT NULL REFERENCES assistant_sessions(id),
              role TEXT NOT NULL,              -- user | assistant
              content TEXT NOT NULL DEFAULT '',
              action_json TEXT,                -- proposed action (needs user confirm)
              runtime TEXT,
              created_at TEXT NOT NULL
            );
            "#,
        )?;

        // v2.1: sync_runs.runtime records which extractor ran.
        let has_runtime: i64 = self.0.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sync_runs') WHERE name = 'runtime'",
            [],
            |r| r.get(0),
        )?;
        if has_runtime == 0 {
            self.0
                .execute_batch("ALTER TABLE sync_runs ADD COLUMN runtime TEXT NOT NULL DEFAULT 'heuristic';")?;
        }

        // FTS5 search index (external-content style: we manage rows manually).
        // If the bundled build lacks FTS5, search falls back to LIKE at query time.
        let fts_ok = self
            .0
            .execute_batch(
                r#"
                CREATE VIRTUAL TABLE IF NOT EXISTS search_index USING fts5(
                  kind, ref_id, parent_id, title, body, tokenize = 'unicode61'
                );
                "#,
            )
            .is_ok();
        if !fts_ok {
            eprintln!("[storage] FTS5 unavailable, search will use LIKE fallback");
        }

        // v2 migration: workstreams.project_id becomes nullable — Workstream
        // may exist standalone (UX rule: New Workstream is a primary action).
        let notnull: i64 = self.0.query_row(
            "SELECT \"notnull\" FROM pragma_table_info('workstreams') WHERE name = 'project_id'",
            [],
            |r| r.get(0),
        )?;
        if notnull != 0 {
            self.0.pragma_update(None, "foreign_keys", "OFF")?;
            self.0.execute_batch(
                r#"
                BEGIN;
                CREATE TABLE workstreams_v2 (
                  id TEXT PRIMARY KEY,
                  project_id TEXT REFERENCES projects(id),
                  title TEXT NOT NULL,
                  description TEXT NOT NULL DEFAULT '',
                  lifecycle TEXT NOT NULL DEFAULT 'open',
                  visibility TEXT NOT NULL DEFAULT 'normal',
                  created_at TEXT NOT NULL,
                  updated_at TEXT NOT NULL
                );
                INSERT INTO workstreams_v2 SELECT id, project_id, title, description, lifecycle, visibility, created_at, updated_at FROM workstreams;
                DROP TABLE workstreams;
                ALTER TABLE workstreams_v2 RENAME TO workstreams;
                CREATE INDEX idx_workstreams_project ON workstreams(project_id);
                COMMIT;
                "#,
            )?;
            self.0.pragma_update(None, "foreign_keys", "ON")?;
            eprintln!("[storage] migrated workstreams.project_id to nullable");
        }
        Ok(())
    }

    // ---------------- Projects ----------------

    pub fn list_projects(&self) -> Result<Vec<Project>> {
        let mut st = self.0.prepare(
            "SELECT id, name, description, archived, created_at, updated_at
             FROM projects WHERE archived = 0 ORDER BY updated_at DESC",
        )?;
        let rows = st
            .query_map([], row_project)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_project(&self, id: &str) -> Result<Option<Project>> {
        Ok(self
            .0
            .query_row(
                "SELECT id, name, description, archived, created_at, updated_at
                 FROM projects WHERE id = ?1",
                params![id],
                row_project,
            )
            .optional()?)
    }

    pub fn upsert_project(&self, p: &Project) -> Result<()> {
        self.0.execute(
            "INSERT INTO projects (id, name, description, archived, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET name = ?2, description = ?3, archived = ?4, updated_at = ?6",
            params![p.id, p.name, p.description, p.archived as i64, p.created_at, p.updated_at],
        )?;
        self.index_project(p)?;
        Ok(())
    }

    pub fn delete_project(&self, id: &str) -> Result<()> {
        self.0.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
        self.unindex("project", id);
        Ok(())
    }

    pub fn add_resource(&self, r: &ProjectResource) -> Result<()> {
        self.0.execute(
            "INSERT INTO project_resources (id, project_id, kind, uri, metadata, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![r.id, r.project_id, r.kind, r.uri, r.metadata.to_string(), r.created_at],
        )?;
        Ok(())
    }

    pub fn list_resources(&self, project_id: &str) -> Result<Vec<ProjectResource>> {
        let mut st = self.0.prepare(
            "SELECT id, project_id, kind, uri, metadata, created_at
             FROM project_resources WHERE project_id = ?1 ORDER BY created_at",
        )?;
        let rows = st
            .query_map(params![project_id], |r| {
                Ok(ProjectResource {
                    id: r.get(0)?,
                    project_id: r.get(1)?,
                    kind: r.get(2)?,
                    uri: r.get(3)?,
                    metadata: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default(),
                    created_at: r.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn remove_resource(&self, id: &str) -> Result<()> {
        self.0.execute("DELETE FROM project_resources WHERE id = ?1", params![id])?;
        Ok(())
    }

    // ---------------- Workstreams ----------------

    pub fn list_workstreams(&self, project_id: Option<&str>) -> Result<Vec<Workstream>> {
        let (sql, has_filter): (&str, bool) = if project_id.is_some() {
            ("SELECT id, project_id, title, description, lifecycle, visibility, created_at, updated_at
              FROM workstreams WHERE project_id = ?1 ORDER BY updated_at DESC", true)
        } else {
            ("SELECT id, project_id, title, description, lifecycle, visibility, created_at, updated_at
              FROM workstreams ORDER BY updated_at DESC", false)
        };
        let mut st = self.0.prepare(sql)?;
        let map = |r: &Row| row_workstream(r);
        let rows = if has_filter {
            st.query_map(params![project_id.unwrap()], map)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            st.query_map([], map)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        Ok(rows)
    }

    pub fn get_workstream(&self, id: &str) -> Result<Option<Workstream>> {
        Ok(self
            .0
            .query_row(
                "SELECT id, project_id, title, description, lifecycle, visibility, created_at, updated_at
                 FROM workstreams WHERE id = ?1",
                params![id],
                row_workstream,
            )
            .optional()?)
    }

    pub fn upsert_workstream(&self, w: &Workstream) -> Result<()> {
        self.0.execute(
            "INSERT INTO workstreams (id, project_id, title, description, lifecycle, visibility, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET title = ?3, description = ?4, lifecycle = ?5, visibility = ?6, updated_at = ?8",
            params![w.id, w.project_id, w.title, w.description, w.lifecycle, w.visibility, w.created_at, w.updated_at],
        )?;
        self.index_workstream(w)?;
        Ok(())
    }

    pub fn delete_workstream(&self, id: &str) -> Result<()> {
        self.0.execute("DELETE FROM workstreams WHERE id = ?1", params![id])?;
        self.unindex("workstream", id);
        Ok(())
    }

    // ---------------- Sessions ----------------

    pub fn upsert_session(&self, s: &Session) -> Result<bool> {
        let existed = self
            .0
            .query_row("SELECT 1 FROM sessions WHERE id = ?1", params![s.id], |_| Ok(()))
            .optional()?
            .is_some();
        self.0.execute(
            "INSERT INTO sessions (id, agent, agent_session_id, title, cwd, project_id, raw_path, parent_agent_session_id, started_at, last_activity_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(agent, agent_session_id) DO UPDATE SET
               title = COALESCE(?4, title),
               cwd = COALESCE(?5, cwd),
               raw_path = ?7,
               last_activity_at = COALESCE(?10, last_activity_at),
               started_at = COALESCE(?9, started_at)",
            params![
                s.id, s.agent.as_str(), s.agent_session_id, s.title, s.cwd,
                s.project_id, s.raw_path, s.parent_agent_session_id, s.started_at, s.last_activity_at
            ],
        )?;
        Ok(existed)
    }

    pub fn get_session(&self, id: &str) -> Result<Option<Session>> {
        Ok(self
            .0
            .query_row("SELECT * FROM sessions WHERE id = ?1", params![id], row_session)
            .optional()?)
    }

    pub fn find_session_by_agent_id(&self, agent: Agent, agent_session_id: &str) -> Result<Option<Session>> {
        Ok(self
            .0
            .query_row(
                "SELECT * FROM sessions WHERE agent = ?1 AND agent_session_id = ?2",
                params![agent.as_str(), agent_session_id],
                row_session,
            )
            .optional()?)
    }

    pub fn list_sessions(&self, filter: SessionFilter) -> Result<Vec<Session>> {
        let mut sql = "SELECT * FROM sessions WHERE 1=1".to_string();
        if filter.project_id.is_some() {
            sql.push_str(" AND project_id = ?1");
        }
        if filter.agent.is_some() {
            sql.push_str(" AND agent = ?2");
        }
        sql.push_str(" ORDER BY COALESCE(last_activity_at, started_at) DESC LIMIT 500");
        let mut st = self.0.prepare(&sql)?;
        let map = |r: &Row| row_session(r);
        let rows = match (filter.project_id, filter.agent) {
            (Some(p), Some(a)) => st.query_map(params![p, a.as_str()], map)?.collect::<std::result::Result<Vec<_>, _>>()?,
            (Some(p), None) => st.query_map(params![p], map)?.collect::<std::result::Result<Vec<_>, _>>()?,
            (None, Some(a)) => st.query_map(params![a.as_str()], map)?.collect::<std::result::Result<Vec<_>, _>>()?,
            (None, None) => st.query_map([], map)?.collect::<std::result::Result<Vec<_>, _>>()?,
        };
        Ok(rows)
    }

    pub fn append_events(&self, events: &[SessionEvent]) -> Result<()> {
        let mut st = self.0.prepare(
            "INSERT OR REPLACE INTO session_events (session_id, sequence, ts, kind, text, raw_ref, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for e in events {
            st.execute(params![
                e.session_id, e.sequence, e.ts, e.kind, e.text, e.raw_ref, e.metadata.to_string()
            ])?;
        }
        Ok(())
    }

    pub fn get_events(&self, session_id: &str, after: Option<i64>, limit: i64) -> Result<Vec<SessionEvent>> {
        let mut st = self.0.prepare(
            "SELECT session_id, sequence, ts, kind, text, raw_ref, metadata
             FROM session_events WHERE session_id = ?1 AND sequence > ?2
             ORDER BY sequence LIMIT ?3",
        )?;
        let rows = st
            .query_map(params![session_id, after.unwrap_or(0), limit], |r| {
                Ok(SessionEvent {
                    session_id: r.get(0)?,
                    sequence: r.get(1)?,
                    ts: r.get(2)?,
                    kind: r.get(3)?,
                    text: r.get(4)?,
                    raw_ref: r.get(5)?,
                    metadata: serde_json::from_str(&r.get::<_, String>(6)?).unwrap_or_default(),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_cursor(&self, session_id: &str) -> Result<i64> {
        Ok(self
            .0
            .query_row(
                "SELECT last_sequence FROM session_cursors WHERE session_id = ?1",
                params![session_id],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0))
    }

    pub fn set_cursor(&self, session_id: &str, seq: i64, size: i64) -> Result<()> {
        self.0.execute(
            "INSERT INTO session_cursors (session_id, last_sequence, last_seen_size)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(session_id) DO UPDATE SET last_sequence = ?2, last_seen_size = ?3",
            params![session_id, seq, size],
        )?;
        Ok(())
    }

    // ---------------- Bindings ----------------

    pub fn bind(&self, b: &SessionWorkstreamBinding) -> Result<()> {
        self.0.execute(
            "INSERT INTO session_workstream_bindings
             (session_id, workstream_id, role, last_seen_revision, last_sync_cursor, created_at, last_used_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(session_id, workstream_id) DO UPDATE SET
               role = ?3, last_used_at = ?7",
            params![b.session_id, b.workstream_id, b.role, b.last_seen_revision, b.last_sync_cursor, b.created_at, b.last_used_at],
        )?;
        Ok(())
    }

    pub fn unbind(&self, session_id: &str, workstream_id: &str) -> Result<()> {
        self.0.execute(
            "DELETE FROM session_workstream_bindings WHERE session_id = ?1 AND workstream_id = ?2",
            params![session_id, workstream_id],
        )?;
        Ok(())
    }

    pub fn bindings_for_session(&self, session_id: &str) -> Result<Vec<SessionWorkstreamBinding>> {
        let mut st = self.0.prepare(
            "SELECT session_id, workstream_id, role, last_seen_revision, last_sync_cursor, created_at, last_used_at
             FROM session_workstream_bindings WHERE session_id = ?1",
        )?;
        let rows = st
            .query_map(params![session_id], row_binding)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn bindings_for_workstream(&self, workstream_id: &str) -> Result<Vec<SessionWorkstreamBinding>> {
        let mut st = self.0.prepare(
            "SELECT session_id, workstream_id, role, last_seen_revision, last_sync_cursor, created_at, last_used_at
             FROM session_workstream_bindings WHERE workstream_id = ?1",
        )?;
        let rows = st
            .query_map(params![workstream_id], row_binding)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn update_binding_sync(&self, session_id: &str, workstream_id: &str, cursor: i64, last_seen_revision: Option<&str>) -> Result<()> {
        self.0.execute(
            "UPDATE session_workstream_bindings
             SET last_sync_cursor = ?3, last_seen_revision = COALESCE(?4, last_seen_revision)
             WHERE session_id = ?1 AND workstream_id = ?2",
            params![session_id, workstream_id, cursor, last_seen_revision],
        )?;
        Ok(())
    }

    // ---------------- Context Items ----------------

    pub fn insert_item(&self, item: &ContextItem, revision: &ContextItemRevision) -> Result<()> {
        self.0.execute(
            "INSERT INTO context_items (id, workstream_id, kind, status, authority, current_revision_id, supersedes_item_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![item.id, item.workstream_id, item.kind, item.status, item.authority,
                    item.current_revision_id, item.supersedes_item_id, item.created_at, item.updated_at],
        )?;
        self.insert_revision(revision)?;
        self.0.execute(
            "UPDATE context_items SET current_revision_id = ?2 WHERE id = ?1",
            params![item.id, revision.id],
        )?;
        self.index_item(item, revision)
    }

    pub fn insert_revision(&self, r: &ContextItemRevision) -> Result<()> {
        self.0.execute(
            "INSERT INTO context_item_revisions (id, item_id, title, content, metadata, source_type, source_ref, sync_run_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![r.id, r.item_id, r.title, r.content, r.metadata.to_string(), r.source_type, r.source_ref, r.sync_run_id, r.created_at],
        )?;
        Ok(())
    }

    pub fn set_item_head(&self, item_id: &str, revision_id: &str, status: Option<&str>) -> Result<()> {
        if let Some(s) = status {
            self.0.execute(
                "UPDATE context_items SET current_revision_id = ?2, status = ?3, updated_at = ?4 WHERE id = ?1",
                params![item_id, revision_id, s, now()],
            )?;
        } else {
            self.0.execute(
                "UPDATE context_items SET current_revision_id = ?2, updated_at = ?3 WHERE id = ?1",
                params![item_id, revision_id, now()],
            )?;
        }
        Ok(())
    }

    pub fn set_item_status(&self, item_id: &str, status: &str) -> Result<()> {
        self.0.execute(
            "UPDATE context_items SET status = ?2, updated_at = ?3 WHERE id = ?1",
            params![item_id, status, now()],
        )?;
        Ok(())
    }

    pub fn set_item_authority(&self, item_id: &str, authority: &str) -> Result<()> {
        self.0.execute(
            "UPDATE context_items SET authority = ?2, updated_at = ?3 WHERE id = ?1",
            params![item_id, authority, now()],
        )?;
        Ok(())
    }

    pub fn get_item(&self, id: &str) -> Result<Option<ContextItem>> {
        Ok(self
            .0
            .query_row(
                "SELECT id, workstream_id, kind, status, authority, current_revision_id, supersedes_item_id, created_at, updated_at
                 FROM context_items WHERE id = ?1",
                params![id],
                row_item,
            )
            .optional()?)
    }

    pub fn get_revision(&self, id: &str) -> Result<Option<ContextItemRevision>> {
        Ok(self
            .0
            .query_row(
                "SELECT id, item_id, title, content, metadata, source_type, source_ref, sync_run_id, created_at
                 FROM context_item_revisions WHERE id = ?1",
                params![id],
                row_revision,
            )
            .optional()?)
    }

    pub fn items_for_workstream(&self, workstream_id: &str, include_inactive: bool) -> Result<Vec<(ContextItem, ContextItemRevision)>> {
        let status_filter = if include_inactive { "" } else { " AND i.status = 'active'" };
        let sql = format!(
            "SELECT i.id, i.workstream_id, i.kind, i.status, i.authority, i.current_revision_id, i.supersedes_item_id, i.created_at, i.updated_at,
                    r.id, r.item_id, r.title, r.content, r.metadata, r.source_type, r.source_ref, r.sync_run_id, r.created_at
             FROM context_items i LEFT JOIN context_item_revisions r ON r.id = i.current_revision_id
             WHERE i.workstream_id = ?1{} ORDER BY i.updated_at DESC",
            status_filter
        );
        let mut st = self.0.prepare(&sql)?;
        let rows = st
            .query_map(params![workstream_id], |r| {
                let item = row_item_at(r, 0)?;
                let rev = row_revision_at(r, 9)?;
                Ok((item, rev))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn item_history(&self, item_id: &str) -> Result<Vec<ContextItemRevision>> {
        let mut st = self.0.prepare(
            "SELECT id, item_id, title, content, metadata, source_type, source_ref, sync_run_id, created_at
             FROM context_item_revisions WHERE item_id = ?1 ORDER BY created_at",
        )?;
        let rows = st
            .query_map(params![item_id], row_revision)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn active_items_since(&self, workstream_id: &str, since: &str) -> Result<Vec<(ContextItem, ContextItemRevision)>> {
        let mut st = self.0.prepare(
            "SELECT i.id, i.workstream_id, i.kind, i.status, i.authority, i.current_revision_id, i.supersedes_item_id, i.created_at, i.updated_at,
                    r.id, r.item_id, r.title, r.content, r.metadata, r.source_type, r.source_ref, r.sync_run_id, r.created_at
             FROM context_items i JOIN context_item_revisions r ON r.id = i.current_revision_id
             WHERE i.workstream_id = ?1 AND i.status = 'active' AND i.updated_at > ?2
             ORDER BY i.updated_at DESC",
        )?;
        let rows = st
            .query_map(params![workstream_id, since], |r| {
                let item = row_item_at(r, 0)?;
                let rev = row_revision_at(r, 9)?;
                Ok((item, rev))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---------------- Sync runs ----------------

    pub fn insert_sync_run(&self, run: &SyncRun) -> Result<()> {
        self.0.execute(
            "INSERT INTO sync_runs (id, session_id, from_sequence, to_sequence, status, mutations, summary, error, created_at, runtime)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![run.id, run.session_id, run.from_sequence, run.to_sequence, run.status,
                    run.mutations.to_string(), run.summary, run.error, run.created_at, run.runtime],
        )?;
        Ok(())
    }

    pub fn list_sync_runs(&self, limit: i64) -> Result<Vec<SyncRun>> {
        let mut st = self.0.prepare(
            "SELECT id, session_id, from_sequence, to_sequence, status, mutations, summary, error, created_at, runtime
             FROM sync_runs ORDER BY created_at DESC LIMIT ?1",
        )?;
        let rows = st
            .query_map(params![limit], row_sync_run)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---------------- Settings ----------------

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .0
            .query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.0.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = ?2",
            params![key, value],
        )?;
        Ok(())
    }

    // ---------------- Assistant ----------------

    pub fn ensure_assistant_session(&self, session_id: Option<&str>) -> Result<String> {
        if let Some(id) = session_id {
            let exists = self
                .0
                .query_row("SELECT 1 FROM assistant_sessions WHERE id = ?1", params![id], |_| Ok(()))
                .optional()?;
            if exists.is_some() {
                return Ok(id.to_string());
            }
        }
        let id = new_id();
        self.0.execute(
            "INSERT INTO assistant_sessions (id, title, created_at) VALUES (?1, ?2, ?3)",
            params![id, "Assistant", now()],
        )?;
        Ok(id)
    }

    #[allow(clippy::type_complexity)]
    pub fn insert_assistant_message(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        action_json: Option<&str>,
        runtime: Option<&str>,
    ) -> Result<(String, String)> {
        let id = new_id();
        let ts = now();
        self.0.execute(
            "INSERT INTO assistant_messages (id, session_id, role, content, action_json, runtime, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, session_id, role, content, action_json, runtime, ts],
        )?;
        Ok((id, ts))
    }

    pub fn list_assistant_messages(&self, session_id: &str, limit: i64) -> Result<Vec<AssistantMessageRow>> {
        let mut st = self.0.prepare(
            "SELECT id, session_id, role, content, action_json, runtime, created_at
             FROM assistant_messages WHERE session_id = ?1 ORDER BY created_at LIMIT ?2",
        )?;
        let rows = st
            .query_map(params![session_id, limit], |r| {
                Ok(AssistantMessageRow {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    role: r.get(2)?,
                    content: r.get(3)?,
                    action_json: r.get(4)?,
                    runtime: r.get(5)?,
                    created_at: r.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---------------- Agent installations cache ----------------

    pub fn save_installation(&self, i: &crate::platform::exec_resolver::AgentInstallation) -> Result<()> {
        self.0.execute(
            "INSERT INTO agent_installations (agent, executable_path, version, source, last_verified_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(agent) DO UPDATE SET executable_path = ?2, version = ?3, source = ?4, last_verified_at = ?5",
            params![i.agent.as_str(), i.executable_path, i.version, i.source, i.last_verified_at],
        )?;
        Ok(())
    }

    pub fn get_installation(&self, agent: Agent) -> Result<Option<crate::platform::exec_resolver::AgentInstallation>> {
        Ok(self
            .0
            .query_row(
                "SELECT agent, executable_path, version, source, last_verified_at
                 FROM agent_installations WHERE agent = ?1",
                params![agent.as_str()],
                |r| {
                    Ok(crate::platform::exec_resolver::AgentInstallation {
                        agent: Agent::parse(&r.get::<_, String>(0)?).unwrap_or(Agent::Codex),
                        executable_path: r.get(1)?,
                        version: r.get(2)?,
                        source: r.get(3)?,
                        last_verified_at: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    // ---------------- FTS ----------------

    pub fn fts_available(&self) -> bool {
        self.0
            .query_row("SELECT 1 FROM sqlite_master WHERE name = 'search_index'", [], |_| Ok(()))
            .optional()
            .unwrap_or(None)
            .is_some()
    }

    pub fn unindex(&self, kind: &str, ref_id: &str) {
        let _ = self.0.execute(
            "DELETE FROM search_index WHERE kind = ?1 AND ref_id = ?2",
            params![kind, ref_id],
        );
    }

    pub fn index_project(&self, p: &Project) -> Result<()> {
        self.unindex("project", &p.id);
        self.0.execute(
            "INSERT INTO search_index (kind, ref_id, parent_id, title, body) VALUES ('project', ?1, '', ?2, ?3)",
            params![p.id, p.name, p.description],
        )?;
        Ok(())
    }

    pub fn index_workstream(&self, w: &Workstream) -> Result<()> {
        self.unindex("workstream", &w.id);
        self.0.execute(
            "INSERT INTO search_index (kind, ref_id, parent_id, title, body) VALUES ('workstream', ?1, ?2, ?3, ?4)",
            params![w.id, w.project_id, w.title, w.description],
        )?;
        Ok(())
    }

    pub fn index_item(&self, item: &ContextItem, rev: &ContextItemRevision) -> Result<()> {
        self.unindex("item", &item.id);
        self.0.execute(
            "INSERT INTO search_index (kind, ref_id, parent_id, title, body) VALUES ('item', ?1, ?2, ?3, ?4)",
            params![item.id, item.workstream_id, rev.title, rev.content],
        )?;
        Ok(())
    }

    pub fn index_events(&self, events: &[SessionEvent]) -> Result<()> {
        // re-reads (e.g. after a cursor reset) must not duplicate rows
        let mut del = self.0.prepare("DELETE FROM search_index WHERE kind = 'event' AND parent_id = ?1")?;
        let mut seen_sessions: std::collections::HashSet<String> = Default::default();
        for e in events {
            if seen_sessions.insert(e.session_id.clone()) {
                let _ = del.execute(params![e.session_id]);
            }
        }
        let mut st = self.0.prepare(
            "INSERT INTO search_index (kind, ref_id, parent_id, title, body) VALUES ('event', ?1, ?2, ?3, ?4)",
        )?;
        for e in events {
            if let Some(t) = &e.text {
                if t.len() < 20 {
                    continue;
                }
                st.execute(params![
                    format!("{}:{}", e.session_id, e.sequence),
                    e.session_id,
                    "",
                    t
                ])?;
            }
        }
        Ok(())
    }

    /// One-time backfill so events ingested before the index existed (or
    /// failed to index) become searchable. Idempotent.
    pub fn backfill_search_index(&self) -> Result<()> {
        if !self.fts_available() {
            return Ok(());
        }
        self.0.execute(
            "INSERT INTO search_index (kind, ref_id, parent_id, title, body)
             SELECT 'event', session_id || ':' || sequence, session_id, '', text
             FROM session_events
             WHERE length(COALESCE(text, '')) >= 20
               AND session_id || ':' || sequence NOT IN (
                   SELECT ref_id FROM search_index WHERE kind = 'event')",
            [],
        )?;
        Ok(())
    }

    // ---------------- helpers ----------------

    pub fn touch_project(&self, project_id: &str) -> Result<()> {
        self.0.execute(
            "UPDATE projects SET updated_at = ?2 WHERE id = ?1",
            params![project_id, now()],
        )?;
        Ok(())
    }

    pub fn stats(&self) -> Result<serde_json::Value> {
        let one = |sql: &str| -> i64 { self.0.query_row(sql, [], |r| r.get(0)).unwrap_or(0) };
        Ok(serde_json::json!({
            "projects": one("SELECT COUNT(*) FROM projects WHERE archived = 0"),
            "workstreams": one("SELECT COUNT(*) FROM workstreams"),
            "sessions": one("SELECT COUNT(*) FROM sessions"),
            "events": one("SELECT COUNT(*) FROM session_events"),
            "context_items": one("SELECT COUNT(*) FROM context_items WHERE status = 'active'"),
            "sync_runs": one("SELECT COUNT(*) FROM sync_runs"),
        }))
    }
}

// row mappers -------------------------------------------------------------

fn row_project(r: &Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: r.get(0)?,
        name: r.get(1)?,
        description: r.get(2)?,
        archived: r.get::<_, i64>(3)? != 0,
        created_at: r.get(4)?,
        updated_at: r.get(5)?,
    })
}

fn row_workstream(r: &Row) -> rusqlite::Result<Workstream> {
    Ok(Workstream {
        id: r.get(0)?,
        project_id: r.get(1)?,
        title: r.get(2)?,
        description: r.get(3)?,
        lifecycle: r.get(4)?,
        visibility: r.get(5)?,
        created_at: r.get(6)?,
        updated_at: r.get(7)?,
    })
}

fn row_session(r: &Row) -> rusqlite::Result<Session> {
    Ok(Session {
        id: r.get("id")?,
        agent: Agent::parse(&r.get::<_, String>("agent")?).unwrap_or(Agent::Codex),
        agent_session_id: r.get("agent_session_id")?,
        title: r.get("title")?,
        cwd: r.get("cwd")?,
        project_id: r.get("project_id")?,
        raw_path: r.get("raw_path")?,
        parent_agent_session_id: r.get("parent_agent_session_id")?,
        started_at: r.get("started_at")?,
        last_activity_at: r.get("last_activity_at")?,
    })
}

fn row_binding(r: &Row) -> rusqlite::Result<SessionWorkstreamBinding> {
    Ok(SessionWorkstreamBinding {
        session_id: r.get(0)?,
        workstream_id: r.get(1)?,
        role: r.get(2)?,
        last_seen_revision: r.get(3)?,
        last_sync_cursor: r.get(4)?,
        created_at: r.get(5)?,
        last_used_at: r.get(6)?,
    })
}

fn row_item_at(r: &Row, base: usize) -> rusqlite::Result<ContextItem> {
    Ok(ContextItem {
        id: r.get(base + 0)?,
        workstream_id: r.get(base + 1)?,
        kind: r.get(base + 2)?,
        status: r.get(base + 3)?,
        authority: r.get(base + 4)?,
        current_revision_id: r.get(base + 5)?,
        supersedes_item_id: r.get(base + 6)?,
        created_at: r.get(base + 7)?,
        updated_at: r.get(base + 8)?,
    })
}

fn row_item(r: &Row) -> rusqlite::Result<ContextItem> {
    row_item_at(r, 0)
}

fn row_revision_at(r: &Row, base: usize) -> rusqlite::Result<ContextItemRevision> {
    Ok(ContextItemRevision {
        id: r.get(base + 0)?,
        item_id: r.get(base + 1)?,
        title: r.get(base + 2).unwrap_or_default(),
        content: r.get(base + 3).unwrap_or_default(),
        metadata: serde_json::from_str(&r.get::<_, String>(base + 4).unwrap_or_default()).unwrap_or_default(),
        source_type: r.get(base + 5)?,
        source_ref: r.get(base + 6)?,
        sync_run_id: r.get(base + 7)?,
        created_at: r.get(base + 8)?,
    })
}

fn row_revision(r: &Row) -> rusqlite::Result<ContextItemRevision> {
    row_revision_at(r, 0)
}

fn row_sync_run(r: &Row) -> rusqlite::Result<SyncRun> {
    Ok(SyncRun {
        id: r.get(0)?,
        session_id: r.get(1)?,
        from_sequence: r.get(2)?,
        to_sequence: r.get(3)?,
        status: r.get(4)?,
        mutations: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or_default(),
        summary: r.get(6)?,
        error: r.get(7)?,
        created_at: r.get(8)?,
        runtime: r.get::<_, Option<String>>(9)?.unwrap_or_else(|| "heuristic".into()),
    })
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AssistantMessageRow {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub action_json: Option<String>,
    pub runtime: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Default, Clone)]
pub struct SessionFilter {
    pub project_id: Option<String>,
    pub agent: Option<Agent>,
}

pub fn ensure_not_empty(name: &str, v: &str) -> Result<()> {
    if v.trim().is_empty() {
        Err(other(format!("{} 不能为空", name)))
    } else {
        Ok(())
    }
}
