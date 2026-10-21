-- Frozen format-12 fixture for the explicit offline upgrade test.

    CREATE TABLE IF NOT EXISTS projects (
      id TEXT PRIMARY KEY,
      name TEXT NOT NULL,
      git_id TEXT,
      name_customized INTEGER NOT NULL DEFAULT 0,
      visibility TEXT NOT NULL DEFAULT 'normal' CHECK (visibility IN ('normal', 'archived')),
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS workstreams (
      id TEXT PRIMARY KEY,
      title TEXT NOT NULL,
      description TEXT NOT NULL DEFAULT '',
      visibility TEXT NOT NULL DEFAULT 'normal',
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL
    );
    -- The Logical Session: user-visible conversation + lifecycle + tracking.
    -- Identity is (agent, root_agent_session_id): the root source's real
    -- Resume identity. A session IS its root source: exactly one source, one
    -- cursor, one frontier per session.
    CREATE TABLE IF NOT EXISTS sessions (
      id TEXT PRIMARY KEY,
      agent TEXT NOT NULL,
      root_agent_session_id TEXT NOT NULL,
      title TEXT,
      cwd TEXT,
      workspace_path_id TEXT,
      -- Fork provenance only: lifecycle / tracking / Conversation stay
      -- fully independent of the fork source.
      forked_from_session_id TEXT
        REFERENCES sessions(id) ON DELETE SET NULL,
      started_at TEXT,
      last_activity_at TEXT,
      last_conversation_at TEXT,
      -- NULL = unarchived, timestamp = archived. Blocks NoEnding resume and
      -- enables permanent local deletion; ingestion/search/Context stay live.
      archived_at TEXT,
      -- The ROOT source: format descriptor, path (reveal in UI) and the
      -- adapter's non-conversation structural facts (thread_source, …).
      source_kind TEXT NOT NULL,
      source_path TEXT NOT NULL,
      metadata TEXT NOT NULL DEFAULT '{}',
      -- Where reading stopped: file identity, source-shape generation, byte
      -- frontier and the append-proof / chain-tail hashes.
      source_file_identity TEXT NOT NULL DEFAULT '',
      source_generation INTEGER NOT NULL DEFAULT 0,
      source_byte_offset INTEGER NOT NULL DEFAULT 0,
      source_last_seen_size INTEGER NOT NULL DEFAULT 0,
      source_mtime REAL,
      source_prefix_hash TEXT NOT NULL DEFAULT '',
      source_tail_hash TEXT NOT NULL DEFAULT '',
      -- The conversation fact frontier: the generation of the CURRENT
      -- conversation and how many messages the store holds.
      fact_generation INTEGER NOT NULL DEFAULT 0,
      latest_message_seq INTEGER NOT NULL DEFAULT 0,
      UNIQUE(agent, root_agent_session_id)
    );
    -- The ONLY conversation store: user/assistant turns of the root source.
    -- Identity dedup is (session_id, source_identity_hash); sequence is
    -- NoEnding's own per-session storage order, independent of the current
    -- conversation ordinal. Complete snapshots remove rows no longer
    -- referenced by the projection below.
    CREATE TABLE IF NOT EXISTS session_messages (
      id TEXT PRIMARY KEY,
      session_id TEXT NOT NULL
        REFERENCES sessions(id) ON DELETE CASCADE,
      sequence INTEGER NOT NULL,
      source_message_id TEXT,
      source_generation INTEGER NOT NULL DEFAULT 0,
      source_position TEXT NOT NULL DEFAULT '',
      source_identity_hash TEXT NOT NULL,
      ts TEXT,
      role TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
      content TEXT NOT NULL,
      -- Assistant rows only: 1 = this is the turn's FINAL reply in the
      -- current conversation (the next projected message is not another
      -- assistant message). Derived from the projection at commit time, never
      -- ingested; an append that continues a turn demotes the old final.
      turn_final INTEGER NOT NULL DEFAULT 0,
      UNIQUE(session_id, sequence),
      UNIQUE(session_id, source_identity_hash)
    );
    -- The CURRENT effective conversation: an ordered view over
    -- session_messages holding exactly one generation. A normal append adds
    -- to the tail; a source rewrite / truncate / reorder that changes the
    -- conversation raises the session fact generation and atomically
    -- replaces the whole projection and retires obsolete session_messages.
    -- Conversation, search and Context read HERE.
    CREATE TABLE IF NOT EXISTS session_message_projection (
      session_id TEXT NOT NULL
        REFERENCES sessions(id) ON DELETE CASCADE,
      ordinal INTEGER NOT NULL,
      session_message_id TEXT NOT NULL
        REFERENCES session_messages(id) ON DELETE CASCADE,
      -- Cumulative 2000-character chunks through this ordinal. Maintained
      -- with the projection, so paging never rescans old message bodies.
      part_end INTEGER NOT NULL CHECK (part_end > 0),
      PRIMARY KEY (session_id, ordinal)
    );
    CREATE UNIQUE INDEX IF NOT EXISTS idx_projection_message
      ON session_message_projection(session_message_id);
    CREATE UNIQUE INDEX IF NOT EXISTS idx_projection_parts
      ON session_message_projection(session_id, part_end);
    -- The Logical Session's Context: system-derived, read-only, one current
    -- row per Session. No row means "never generated". Summary is Markdown;
    -- revision is the CAS guard, and
    -- (ingest_generation, processed_through_seq) is the Session frontier in
    -- CURRENT-message-projection ordinals.
    CREATE TABLE IF NOT EXISTS session_contexts (
      session_id TEXT PRIMARY KEY
        REFERENCES sessions(id) ON DELETE CASCADE,
      summary TEXT NOT NULL DEFAULT '',
      revision INTEGER NOT NULL DEFAULT 0,
      ingest_generation INTEGER NOT NULL DEFAULT 0,
      processed_through_seq INTEGER NOT NULL DEFAULT 0,
      updated_at TEXT NOT NULL
    );
    -- The Workstream side of the two-level revision scheme.
    -- context_revision guards summary writes;
    -- input_revision marks tracked sessions / projects / title /
    -- description changes that leave inputs pending; AI records the
    -- input_revision it consumed so its own success does not leave the
    -- Workstream "pending" again. Created as (0, 1, 0) so the first explicit
    -- generation is allowed.
    CREATE TABLE IF NOT EXISTS workstream_context_state (
      workstream_id TEXT PRIMARY KEY REFERENCES workstreams(id) ON DELETE CASCADE,
      context_revision INTEGER NOT NULL DEFAULT 0,
      input_revision INTEGER NOT NULL DEFAULT 0,
      consumed_input_revision INTEGER NOT NULL DEFAULT 0,
      summary TEXT NOT NULL DEFAULT '',
      updated_at TEXT
    );
    -- What the Workstream has already consumed from each tracked Session.
    -- Written in the same transaction as a combined update; a Session that
    -- is no longer tracked is cleaned up on the next successful synthesis.
    CREATE TABLE IF NOT EXISTS workstream_session_frontiers (
      workstream_id TEXT NOT NULL REFERENCES workstreams(id) ON DELETE CASCADE,
      session_id TEXT NOT NULL REFERENCES sessions(id),
      session_context_revision INTEGER NOT NULL DEFAULT 0,
      PRIMARY KEY (workstream_id, session_id)
    );
    -- One current project summary, an input fingerprint and consumed session
    -- summary revisions. Metadata changes mark pending, never discard a summary.
    CREATE TABLE IF NOT EXISTS project_context_state (
      project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
      context_revision INTEGER NOT NULL DEFAULT 0,
      input_fingerprint TEXT NOT NULL DEFAULT '',
      summary TEXT NOT NULL DEFAULT '',
      updated_at TEXT
    );
    CREATE TABLE IF NOT EXISTS project_session_frontiers (
      project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
      session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
      session_context_revision INTEGER NOT NULL DEFAULT 0,
      PRIMARY KEY (project_id, session_id)
    );
    CREATE TABLE IF NOT EXISTS agent_installations (
      agent TEXT PRIMARY KEY,
      executable_path TEXT,
      version TEXT,
      source TEXT,
      last_verified_at TEXT
    );
    CREATE TABLE IF NOT EXISTS preferences (
      id TEXT PRIMARY KEY,
      project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
      content TEXT NOT NULL CHECK(length(trim(content)) > 0),
      locked INTEGER NOT NULL DEFAULT 0 CHECK(locked IN (0, 1)),
      revision INTEGER NOT NULL DEFAULT 1
    );
    CREATE INDEX IF NOT EXISTS idx_preferences_project ON preferences(project_id);

    CREATE TABLE IF NOT EXISTS settings (
      key TEXT PRIMARY KEY,
      value TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS ingest_sources (
      id TEXT PRIMARY KEY,
      agent TEXT NOT NULL,
      path TEXT NOT NULL,
      origin TEXT NOT NULL DEFAULT 'user',   -- default | user
      created_at TEXT NOT NULL,
      UNIQUE(agent, path)
    );
    CREATE INDEX IF NOT EXISTS idx_sessions_workspace_path ON sessions(workspace_path_id);
    CREATE INDEX IF NOT EXISTS idx_sessions_activity
      ON sessions(COALESCE(last_conversation_at, last_activity_at, started_at) DESC, id);
    CREATE INDEX IF NOT EXISTS idx_sessions_unarchived_activity
      ON sessions(COALESCE(last_conversation_at, last_activity_at, started_at) DESC, id)
      WHERE archived_at IS NULL;
    CREATE INDEX IF NOT EXISTS idx_sessions_fork_parent ON sessions(forked_from_session_id);
    CREATE INDEX IF NOT EXISTS idx_workstream_frontiers_session
      ON workstream_session_frontiers(session_id);
    CREATE INDEX IF NOT EXISTS idx_project_frontiers_session
      ON project_session_frontiers(session_id);

    CREATE TABLE IF NOT EXISTS git_identities (
      id TEXT PRIMARY KEY,
      common_dir TEXT NOT NULL UNIQUE,
      first_seen_at TEXT NOT NULL,
      last_seen_at TEXT NOT NULL,
      metadata TEXT NOT NULL DEFAULT '{}'
    );
    CREATE TABLE IF NOT EXISTS workspace_paths (
      id TEXT PRIMARY KEY,
      canonical_path TEXT NOT NULL UNIQUE,
      project_id TEXT NOT NULL REFERENCES projects(id),
      git_state TEXT NOT NULL DEFAULT 'none',
      git_kind TEXT,
      -- Named `exists_on_disk`, not `exists`: EXISTS is a SQLite keyword and
      -- `exists INTEGER NOT NULL` is a syntax error, so the spec's column name
      -- would need quoting in every statement. The domain
      -- field stays `exists`.
      exists_on_disk INTEGER NOT NULL DEFAULT 1,
      first_seen_at TEXT NOT NULL,
      last_seen_at TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS workstream_projects (
      workstream_id TEXT NOT NULL REFERENCES workstreams(id) ON DELETE CASCADE,
      project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
      PRIMARY KEY(workstream_id, project_id)
    );
    CREATE INDEX IF NOT EXISTS idx_workstream_projects_project ON workstream_projects(project_id);
    CREATE TABLE IF NOT EXISTS session_workstreams (
      session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
      workstream_id TEXT NOT NULL REFERENCES workstreams(id) ON DELETE CASCADE,
      PRIMARY KEY(session_id, workstream_id)
    );
    CREATE INDEX IF NOT EXISTS idx_session_workstreams_task ON session_workstreams(workstream_id);
    CREATE INDEX IF NOT EXISTS idx_workspace_paths_project ON workspace_paths(project_id);

    CREATE UNIQUE INDEX IF NOT EXISTS idx_projects_git_id
      ON projects(git_id)
      WHERE git_id IS NOT NULL;

    -- Routing has ordinary indexes. Message documents contain no prose copy;
    -- their immutable content is resolved through search_content. Other kinds
    -- store their compact, assembled search document here.
    CREATE TABLE IF NOT EXISTS search_documents (
      id INTEGER PRIMARY KEY,
      kind TEXT NOT NULL CHECK (kind IN ('session','message')),
      ref_id TEXT NOT NULL,
      parent_id TEXT NOT NULL DEFAULT '',
      title TEXT NOT NULL DEFAULT '',
      body TEXT NOT NULL DEFAULT '',
      UNIQUE(kind, ref_id)
    );
    CREATE INDEX IF NOT EXISTS idx_search_documents_parent
      ON search_documents(kind, parent_id);
    CREATE VIEW IF NOT EXISTS search_content AS
      SELECT d.id, d.kind, d.ref_id, d.parent_id, d.title,
             CASE WHEN d.kind = 'message' THEN COALESCE(m.content, d.body) ELSE d.body END AS body
      FROM search_documents d LEFT JOIN session_messages m
        ON d.kind = 'message' AND m.id = d.ref_id AND m.session_id = d.parent_id;
    -- External content avoids FTS5's second stored copy of every document.
    CREATE VIRTUAL TABLE IF NOT EXISTS search_index USING fts5(
      kind UNINDEXED, ref_id UNINDEXED, parent_id UNINDEXED, title, body,
      content='search_content', content_rowid='id', tokenize = 'unicode61'
    );
    CREATE TRIGGER IF NOT EXISTS search_documents_insert AFTER INSERT ON search_documents BEGIN
      INSERT INTO search_index(rowid, kind, ref_id, parent_id, title, body)
        SELECT id, kind, ref_id, parent_id, title, body FROM search_content WHERE id = new.id;
    END;
    CREATE TRIGGER IF NOT EXISTS search_documents_delete BEFORE DELETE ON search_documents BEGIN
      INSERT INTO search_index(search_index, rowid, kind, ref_id, parent_id, title, body)
        SELECT 'delete', id, kind, ref_id, parent_id, title, body FROM search_content WHERE id = old.id;
    END;
    CREATE TRIGGER IF NOT EXISTS search_documents_before_update BEFORE UPDATE ON search_documents BEGIN
      INSERT INTO search_index(search_index, rowid, kind, ref_id, parent_id, title, body)
        SELECT 'delete', id, kind, ref_id, parent_id, title, body FROM search_content WHERE id = old.id;
    END;
    CREATE TRIGGER IF NOT EXISTS search_documents_update AFTER UPDATE ON search_documents BEGIN
      INSERT INTO search_index(rowid, kind, ref_id, parent_id, title, body)
        SELECT id, kind, ref_id, parent_id, title, body FROM search_content WHERE id = new.id;
    END;
    -- Keep the external content available until its postings are retired,
    -- including a message removed by a session foreign-key cascade.
    CREATE TRIGGER IF NOT EXISTS session_messages_search_delete BEFORE DELETE ON session_messages BEGIN
      DELETE FROM search_documents WHERE kind = 'message' AND ref_id = old.id;
    END;

PRAGMA application_id=1315915118;
PRAGMA user_version=12;
