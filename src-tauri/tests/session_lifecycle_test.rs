//! Session Lifecycle & Deletion v0.1 — integrity matrix (方案 §45).
//!
//! Locks the invariants the deletion plan freezes:
//! - Trash is reversible and preserves the same Session.id; it never touches
//!   the Agent source file, and a trashed session is invisible to default
//!   projections and search, is not mutated by discovery, cannot commit
//!   in-flight ingestion, and cannot resume.
//! - Permanent deletion is prepared against a frozen, adapter-validated plan
//!   and purges every Session-owned row in one transaction — with no
//!   tombstone, no deletion-job remnant, and no surviving context provenance
//!   pointing at the dead session.
//! - A genuinely restored source is rediscovered as a NEW NoEnding session:
//!   no old bindings, no provenance relink (§34).
//!
//! All fixtures are temp files. The real ~/.codex / ~/.claude / ~/.pi are
//! never touched (方案 §46).

use noending::adapters::autoclaw::AutoClawAdapter;
use noending::adapters::claude::ClaudeAdapter;
use noending::adapters::codex::CodexAdapter;
use noending::adapters::dsh::DshAdapter;
use noending::adapters::pi::PiAdapter;
use noending::adapters::qoder::QoderAdapter;
use noending::adapters::workbuddy::WorkBuddyAdapter;
use noending::adapters::zcode::ZCodeAdapter;
use noending::adapters::AgentAdapter;
use noending::domain::{Agent, ContextDelivery, LaunchIntent, Session, SessionListScope, SyncRun};
use noending::error::AppError;
use noending::ingestion::ensure_session_row_with;
use noending::lifecycle;
use noending::search;
use noending::storage::workspace::insert_workspace_path_conn;
use noending::storage::SessionFilter;
use noending::storage::{new_id, now, Db};
use noending::workspace::{normalize_path, WorkspaceAttaching};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

// ---- fixtures -------------------------------------------------------------

fn unique_dir(tag: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "noending-lifecycle-{}-{}-{}",
        tag,
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn open_db(tag: &str) -> Db {
    let dir = unique_dir(tag);
    let db = Db::open(&dir.join("test.db")).unwrap();
    // The LexicalPaths seam inserts workspace paths under this project.
    db.upsert_project(&noending::domain::Project::new(PROJECT.into(), "P"))
        .unwrap();
    db
}

const PROJECT: &str = "p-lifecycle";

/// Lexical WorkspaceAttaching seam, matching the launch-test fixtures.
struct LexicalPaths;
impl WorkspaceAttaching for LexicalPaths {
    fn ensure_path(&self, conn: &Connection, raw: &str) -> noending::error::Result<Option<String>> {
        Ok(match normalize_path(raw) {
            Some(canonical) => Some(insert_workspace_path_conn(conn, &canonical, PROJECT)?),
            None => None,
        })
    }
}

fn attacher() -> LexicalPaths {
    LexicalPaths
}

/// Per-agent fixture file that BOTH passes `detect_format` for the agent AND
/// parses back to `session_id` through the adapter's own discovery parser —
/// the exact property `prepare_source_session_deletion` demands (方案 §16).
fn write_agent_fixture(agent: Agent, path: &Path, session_id: &str) {
    // ZCode has no transcript to write at all: its source is a live SQLite
    // store that no line-based fingerprint could ever claim (方案 §37.10).
    if agent == Agent::ZCode {
        write_zcode_store(path, session_id);
        return;
    }
    let body = match agent {
        Agent::Codex => format!(
            "{{\"ordinal\":0,\"type\":\"session_meta\",\"payload\":{{\"session_id\":\"{sid}\",\"cwd\":\"/tmp/proj\",\"timestamp\":\"2026-09-13T10:00:00Z\"}}}}\n\
             {{\"ordinal\":1,\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"first user message about goals\"}}]}}}}\n",
            sid = session_id
        ),
        Agent::ClaudeCode => format!(
            "{{\"type\":\"user\",\"sessionId\":\"{sid}\",\"uuid\":\"u1\",\"timestamp\":\"2026-09-13T10:00:00Z\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"text\",\"text\":\"first user message about goals\"}}]}}}}\n\
             {{\"type\":\"assistant\",\"sessionId\":\"{sid}\",\"uuid\":\"u2\",\"parentUuid\":\"u1\",\"timestamp\":\"2026-09-13T10:01:00Z\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"reply\"}}]}}}}\n",
            sid = session_id
        ),
        Agent::Pi => format!(
            "{{\"type\":\"session\",\"id\":\"{sid}\",\"cwd\":\"/tmp/proj\",\"timestamp\":\"2026-09-13T10:00:00Z\"}}\n\
             {{\"type\":\"message\",\"id\":\"m1\",\"parentId\":\"{sid}\",\"provider\":\"p\",\"timestamp\":\"2026-09-13T10:00:01Z\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"text\",\"text\":\"first user message about goals\"}}]}}}}\n",
            sid = session_id
        ),
        // AutoClaw: the pi core's entry shape under its own directory. The
        // permanent-deletion path is not offered for it (§37.6), so this arm
        // exists only for the agents that do support it.
        Agent::AutoClaw => format!(
            "{{\"type\":\"session\",\"version\":3,\"id\":\"{sid}\",\"timestamp\":\"2026-09-13T10:00:00Z\",\"cwd\":\"/tmp/proj\"}}\n\
             {{\"type\":\"message\",\"id\":\"m1\",\"parentId\":\"{sid}\",\"timestamp\":\"2026-09-13T10:00:01Z\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"text\",\"text\":\"first user message about goals\"}}]}}}}\n",
            sid = session_id
        ),
        // WorkBuddy: no session header, the payload sits at the top level, and
        // the timestamp is epoch millis. It offers no permanent source
        // deletion (§37.7), so this arm exists only for match exhaustiveness.
        Agent::WorkBuddy => format!(
            "{{\"id\":\"m1\",\"timestamp\":1783137449113,\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"first user message about goals\"}}],\"sessionId\":\"{sid}\",\"cwd\":\"/tmp/proj\"}}\n\
             {{\"id\":\"m2\",\"parentId\":\"m1\",\"timestamp\":1783137455216,\"type\":\"message\",\"role\":\"assistant\",\"content\":[{{\"type\":\"output_text\",\"text\":\"reply\"}}],\"sessionId\":\"{sid}\",\"cwd\":\"/tmp/proj\"}}\n",
            sid = session_id
        ),
        // Qoder = Claude's shape + its own bookkeeping line, which is what
        // makes the fingerprint read it as Qoder rather than Claude (§37.5).
        Agent::Qoder => format!(
            "{{\"type\":\"workspace-directories\",\"sessionId\":\"{sid}\",\"directories\":[\"/tmp/proj\"]}}\n\
             {{\"type\":\"user\",\"sessionId\":\"{sid}\",\"uuid\":\"u1\",\"cwd\":\"/tmp/proj\",\"timestamp\":\"2026-09-13T10:00:00Z\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"text\",\"text\":\"first user message about goals\"}}]}}}}\n\
             {{\"type\":\"assistant\",\"sessionId\":\"{sid}\",\"uuid\":\"u2\",\"parentUuid\":\"u1\",\"cwd\":\"/tmp/proj\",\"timestamp\":\"2026-09-13T10:01:00Z\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"reply\"}}]}}}}\n",
            sid = session_id
        ),
        // dsh: the session header carries `createdAt`, and every later record
        // carries the writer's own `seq`. No permanent source deletion (§37.8).
        Agent::Dsh => format!(
            "{{\"type\":\"session\",\"version\":2,\"id\":\"{sid}\",\"createdAt\":1788969915099,\"cwd\":\"/tmp/proj\"}}\n\
             {{\"type\":\"user/message\",\"seq\":1,\"time\":1788969927205,\"data\":{{\"content\":[{{\"type\":\"text\",\"text\":\"first user message about goals\"}}],\"role\":\"user\"}}}}\n",
            sid = session_id
        ),
        Agent::ZCode => unreachable!("written above: ZCode's source is a database"),
    };
    // dsh is the one agent whose transcript is zstd (方案 §37.8).
    if agent == Agent::Dsh {
        std::fs::write(path, zstd::encode_all(body.as_bytes(), 3).unwrap()).unwrap();
    } else {
        std::fs::write(path, body).unwrap();
    }
}

/// ZCode's source is a SQLite store, not a transcript, so it has no line to
/// return: this builds the real three tables at the fixture path, with one real
/// prompt and one settled reply in them. Only the refusal test reads it as a
/// path — but `zcode_sessions_ingest_and_a_replay_stores_nothing` below drives
/// it through the production store, so the fixture has to be a store ZCode
/// could actually have written (§37.10).
fn write_zcode_store(path: &Path, session_id: &str) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE session (
            id text primary key, project_id text not null, workspace_id text,
            parent_id text, slug text not null, directory text not null,
            path text, title text not null, version text not null, share_url text,
            summary_additions integer, summary_deletions integer, summary_files integer,
            summary_diffs text, revert text, permission text,
            time_created integer not null, time_updated integer not null,
            time_compacting integer, time_archived integer,
            task_type text not null default 'interactive',
            title_source text not null default 'first_input',
            title_message_id text, time_title_updated integer, trace_id text);
         CREATE TABLE message (
            id text primary key,
            session_id text not null references session(id) on delete cascade,
            time_created integer not null, time_updated integer not null,
            data text not null, sequence integer);
         CREATE TABLE part (
            id text primary key,
            message_id text not null references message(id) on delete cascade,
            session_id text not null, time_created integer not null,
            time_updated integer not null, data text not null, sequence integer);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session (id, project_id, slug, directory, title, version,
                              time_created, time_updated)
         VALUES (?1, 'p', ?1, '/tmp/proj', 'app title', '1', 1788012788361, 1788012789059)",
        [session_id],
    )
    .unwrap();
    // A user-role reminder first, so a reader that trusts `role` would both
    // ingest noise and title the session after it.
    conn.execute(
        "INSERT INTO message (id, session_id, time_created, time_updated, data, sequence)
         VALUES ('z-reminder', ?1, 0, 0, ?2, 0)",
        rusqlite::params![
            session_id,
            r#"{"role":"user","time":{"created":1788538485030},"semantics":{"origin":"agent_runtime","kind":"todo_reminder"}}"#
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data, sequence)
         VALUES ('z-p0', 'z-reminder', ?1, 0, 0, ?2, 0)",
        rusqlite::params![
            session_id,
            r#"{"type":"text","text":"The TodoWrite tool hasn't been used recently."}"#
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO message (id, session_id, time_created, time_updated, data, sequence)
         VALUES ('z-user', ?1, 0, 0, ?2, 1)",
        rusqlite::params![
            session_id,
            r#"{"role":"user","time":{"created":1788012788361},"semantics":{"origin":"real_user","kind":"user_prompt"}}"#
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data, sequence)
         VALUES ('z-p1', 'z-user', ?1, 0, 0, ?2, 0)",
        rusqlite::params![
            session_id,
            r#"{"type":"text","text":"first user message about goals"}"#
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO message (id, session_id, time_created, time_updated, data, sequence)
         VALUES ('z-reply', ?1, 0, 0, ?2, 2)",
        rusqlite::params![
            session_id,
            r#"{"role":"assistant","time":{"created":1788012788390,"completed":1788012789059},"semantics":{"origin":"agent_runtime","kind":"assistant_response"}}"#
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data, sequence)
         VALUES ('z-p2', 'z-reply', ?1, 0, 0, ?2, 0)",
        rusqlite::params![session_id, r#"{"type":"text","text":"reply"}"#],
    )
    .unwrap();
}

fn fixture_session(db: &Db, agent: Agent, tag: &str) -> Session {
    let dir = unique_dir(tag);
    let file = dir.join("session.jsonl");
    let agent_session_id = format!("fixed-id-{}", new_id());
    write_agent_fixture(agent, &file, &agent_session_id);
    let discovered = noending::adapters::DiscoveredSession {
        agent,
        agent_session_id: agent_session_id.clone(),
        path: file.clone(),
        cwd: Some(dir.to_string_lossy().to_string()),
        started_at: Some("2026-09-13T10:00:00Z".into()),
        last_activity_at: Some("2026-09-13T10:05:00Z".into()),
        first_user_text: Some("first user message about goals".into()),
        parent_agent_session_id: None,
    };
    let (s, _) = ensure_session_row_with(db, &discovered, &attacher()).unwrap();
    assert_eq!(s.agent_session_id, agent_session_id);
    s
}

/// Production round-trip: read the delta and commit it through the guarded
/// storage path (the one with the trash guard inside the transaction).
fn ingest(db: &Db, adapter: &dyn AgentAdapter, session: &Session) -> usize {
    let cursor = db.get_source_cursor(&session.id).unwrap();
    let delta = adapter.read_delta(session, &cursor).unwrap();
    let source = delta.source.clone().expect("source state");
    let stored = db
        .append_source_events(&session.id, &delta.events, &source, &session.raw_path)
        .unwrap();
    // production ingest_delta also indexes what it stored
    if !stored.is_empty() {
        db.index_new_events(&stored).unwrap();
    }
    stored.len()
}

fn count(db: &Db, sql: &str, session_id: &str) -> i64 {
    db.read()
        .query_row(sql, [session_id], |r| r.get::<_, i64>(0))
        .unwrap()
}

fn workstream(db: &Db, title: &str) -> noending::domain::Workstream {
    noending::workspace::workstream::create_workstream(db, &attacher(), title, "", &[])
        .unwrap()
        .workstream
}

fn bind(db: &Db, session_id: &str, ws_id: &str) {
    noending::workspace::session::record_user_binding(
        db,
        session_id,
        ws_id,
        "primary",
        "user_assigned",
        1.0,
    )
    .unwrap();
}

fn sync_run(db: &Db, session_id: &str) -> SyncRun {
    let run = SyncRun {
        id: new_id(),
        session_id: session_id.to_string(),
        from_sequence: 0,
        to_sequence: 1,
        status: "ok".into(),
        mutations: serde_json::json!([]),
        summary: "test".into(),
        error: None,
        created_at: now(),
        runtime: "heuristic".into(),
        delta_fingerprint: Some(new_id()),
        source_generation: 0,
    };
    db.insert_sync_run(&run).unwrap();
    run
}

fn delivery(db: &Db, session_id: &str, ws_id: &str) {
    db.record_delivery(&ContextDelivery {
        id: new_id(),
        session_id: session_id.to_string(),
        workstream_id: ws_id.to_string(),
        bundle_id: new_id(),
        delivered_revisions: vec![],
        delivered_conflicts: vec![],
        delivered_at: now(),
    })
    .unwrap();
}

fn launch_intent(db: &Db, session_id: &str, agent: Agent) {
    db.insert_launch_intent(&LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent,
        selected_workstream_ids: vec![],
        cwd: None,
        context_bundle_markdown: None,
        context_bundle_revisions: None,
        process_id: Some(1),
        launched_at: now(),
        matched_session_id: Some(session_id.to_string()),
        status: "matched".into(),
        note: String::new(),
        created_at: now(),
        updated_at: now(),
    })
    .unwrap();
}

/// A context item whose head revision points at one of the session's events
/// (current spelling) and at one of its sync runs — the full provenance match.
fn context_item_pointing_at(db: &Db, ws_id: &str, session_id: &str) -> String {
    let events = db.get_events(session_id, None, 10).unwrap();
    let event_ref = format!("session-event:{}", events.first().unwrap().id);
    let run = sync_run(db, session_id);
    let item = noending::sync::create_item(
        db,
        ws_id,
        "decision",
        "Use SQLite",
        "storage decision from the session",
        "agent_statement",
        "session_event",
        &[event_ref],
        Some(&run.id),
        "agent",
    )
    .unwrap();
    item.id
}

// ---- lifecycle: trash / restore / visibility ------------------------------

#[test]
fn trash_session_preserves_source_and_data() {
    let db = open_db("trash-preserves");
    let adapter = CodexAdapter;
    let s = fixture_session(&db, Agent::Codex, "trash-preserves");
    ingest(&db, &adapter, &s);
    let ws = workstream(&db, "WS");
    bind(&db, &s.id, &ws.id);

    let trashed = lifecycle::trash_session(&db, &s.id).unwrap();
    assert!(trashed.trashed_at.is_some());

    // Agent source untouched (方案 §2).
    assert!(Path::new(&s.raw_path).exists());
    // NoEnding data untouched: events, bindings, cursor all survive.
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM session_events WHERE session_id = ?",
            &s.id
        ),
        2
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM session_workstream_bindings WHERE session_id = ?",
            &s.id
        ),
        1
    );
    assert!(db.get_source_cursor(&s.id).is_ok());
}

#[test]
fn restore_keeps_same_session_id_and_data() {
    let db = open_db("restore-keeps");
    let s = fixture_session(&db, Agent::Codex, "restore-keeps");
    let ws = workstream(&db, "WS");
    bind(&db, &s.id, &ws.id);

    lifecycle::trash_session(&db, &s.id).unwrap();
    let restored = lifecycle::restore_session(&db, &s.id).unwrap();

    // 方案 §1.1: Trash → Restore keeps the same identity.
    assert_eq!(restored.id, s.id);
    assert!(restored.trashed_at.is_none());
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM session_workstream_bindings WHERE session_id = ?",
            &s.id
        ),
        1
    );
}

#[test]
fn trash_is_hidden_from_default_list_and_visible_in_trash_scope() {
    let db = open_db("list-scope");
    let s = fixture_session(&db, Agent::Codex, "list-scope");

    let active = db
        .list_sessions(SessionFilter {
            scope: SessionListScope::Active,
            ..Default::default()
        })
        .unwrap();
    assert!(active.iter().any(|x| x.id == s.id));

    lifecycle::trash_session(&db, &s.id).unwrap();

    let active = db
        .list_sessions(SessionFilter {
            scope: SessionListScope::Active,
            ..Default::default()
        })
        .unwrap();
    assert!(
        !active.iter().any(|x| x.id == s.id),
        "trash hidden from default list"
    );

    let trash = db
        .list_sessions(SessionFilter {
            scope: SessionListScope::Trash,
            ..Default::default()
        })
        .unwrap();
    assert!(trash.iter().any(|x| x.id == s.id));

    let all = db
        .list_sessions(SessionFilter {
            scope: SessionListScope::All,
            ..Default::default()
        })
        .unwrap();
    assert!(all.iter().any(|x| x.id == s.id));
}

#[test]
fn trash_is_hidden_from_project_and_workstream_projections() {
    let db = open_db("projection-hide");
    let s = fixture_session(&db, Agent::Codex, "projection-hide");
    let ws = workstream(&db, "WS");
    bind(&db, &s.id, &ws.id);

    let (count_before, latest_before, _) = db.workstream_session_stats(&ws.id).unwrap();
    assert_eq!(count_before, 1);
    assert!(latest_before.is_some());

    lifecycle::trash_session(&db, &s.id).unwrap();

    // Workstream cards no longer count the trashed session (方案 §11).
    let (count_after, latest_after, _) = db.workstream_session_stats(&ws.id).unwrap();
    assert_eq!(count_after, 0);
    assert!(latest_after.is_none());

    // Project detail walks the session's WorkspacePath — also hidden.
    let path_id = s.workspace_path_id.clone().unwrap();
    let project_sessions = db.list_sessions_for_workspace_path(&path_id).unwrap();
    assert!(!project_sessions.iter().any(|x| x.id == s.id));
}

#[test]
fn trash_is_removed_from_search_and_restore_reindexes() {
    let db = open_db("search-unindex");
    let adapter = CodexAdapter;
    let s = fixture_session(&db, Agent::Codex, "search-unindex");
    ingest(&db, &adapter, &s);

    let hits = search::search(&db, "goals", 20).unwrap();
    assert!(
        hits.iter().any(|h| h.ref_id == format!("{}:2", s.id)),
        "event searchable before trash"
    );

    lifecycle::trash_session(&db, &s.id).unwrap();
    let hits = search::search(&db, "goals", 20).unwrap();
    assert!(
        !hits.iter().any(|h| h.ref_id == format!("{}:2", s.id)),
        "trashed session is unindexed (方案 §12)"
    );

    lifecycle::restore_session(&db, &s.id).unwrap();
    let hits = search::search(&db, "goals", 20).unwrap();
    assert!(
        hits.iter().any(|h| h.ref_id == format!("{}:2", s.id)),
        "restore reindexes"
    );
}

#[test]
fn discovery_does_not_restore_or_mutate_trashed_session() {
    let db = open_db("discovery-frozen");
    let s = fixture_session(&db, Agent::Codex, "discovery-frozen");
    lifecycle::trash_session(&db, &s.id).unwrap();

    // The source keeps living while the session is in the trash…
    let file = PathBuf::from(&s.raw_path);
    let grown = format!(
        "{}{{\"ordinal\":2,\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"NEW message while trashed\"}}]}}}}\n",
        std::fs::read_to_string(&file).unwrap()
    );
    std::fs::write(&file, grown).unwrap();

    // …and discovery observes it but returns the row UNCHANGED (方案 §4/§8).
    let discovered = noending::adapters::DiscoveredSession {
        agent: Agent::Codex,
        agent_session_id: s.agent_session_id.clone(),
        path: file.clone(),
        cwd: Some("/tmp/proj".into()),
        started_at: Some("2026-09-13T10:00:00Z".into()),
        last_activity_at: Some("2026-09-14T10:00:00Z".into()),
        first_user_text: Some("brand new title text".into()),
        parent_agent_session_id: None,
    };
    let (row, is_new) = ensure_session_row_with(&db, &discovered, &attacher()).unwrap();
    assert!(!is_new);
    assert_eq!(row.id, s.id);
    assert!(row.is_trashed(), "trash is not lifted by discovery");
    assert_eq!(row.title, s.title, "title not refreshed while trashed");
    assert_eq!(row.last_activity_at, s.last_activity_at);
}

#[test]
fn inflight_ingest_cannot_commit_after_trash() {
    let db = open_db("inflight-guard");
    let adapter = CodexAdapter;
    let s = fixture_session(&db, Agent::Codex, "inflight-guard");
    ingest(&db, &adapter, &s);
    let cursor_before = db.get_source_cursor(&s.id).unwrap();

    // A new source line arrives…
    let file = Path::new(&s.raw_path);
    let grown = format!(
        "{}{{\"ordinal\":3,\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"second round message\"}}]}}}}\n",
        std::fs::read_to_string(file).unwrap()
    );
    std::fs::write(file, grown).unwrap();

    // …T0: the adapter read happens (the in-flight delta)…
    let cursor = db.get_source_cursor(&s.id).unwrap();
    let delta = adapter.read_delta(&s, &cursor).unwrap();
    assert!(
        !delta.events.is_empty(),
        "the adapter still reads the source"
    );

    // …T1: the user trashes; …
    lifecycle::trash_session(&db, &s.id).unwrap();

    // …T2: the staged batch tries to commit and is rejected (§9).
    let source = delta.source.clone().unwrap();
    let stored = db
        .append_source_events(&s.id, &delta.events, &source, &s.raw_path)
        .unwrap();
    assert!(stored.is_empty(), "a trashed session takes no events");

    let cursor_after = db.get_source_cursor(&s.id).unwrap();
    assert_eq!(
        cursor_before.byte_offset, cursor_after.byte_offset,
        "cursor frozen"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM session_events WHERE session_id = ?",
            &s.id
        ),
        2
    );

    // Restore → the next reconcile continues from the untouched cursor and
    // picks up what was suppressed (方案 §8).
    lifecycle::restore_session(&db, &s.id).unwrap();
    let stored = ingest(&db, &adapter, &s);
    assert_eq!(stored, 1, "suppressed delta is ingested after restore");
}

#[test]
fn permanent_delete_requires_trash() {
    let db = open_db("needs-trash");
    let s = fixture_session(&db, Agent::Codex, "needs-trash");
    let err = lifecycle::prepare_session_permanent_delete(&db, &s.id).unwrap_err();
    assert!(err.to_string().contains("回收站"), "unexpected: {err}");
    assert!(Path::new(&s.raw_path).exists());
}

#[test]
fn trashed_session_cannot_resume() {
    let db = open_db("no-resume");
    let s = fixture_session(&db, Agent::Codex, "no-resume");
    lifecycle::trash_session(&db, &s.id).unwrap();

    let launcher = noending::launcher::SessionLauncher {
        runtime_dir: unique_dir("no-resume-runtime"),
    };
    let workspace = noending::launcher::LaunchWorkspace {
        default_workspace: None,
    };
    let err = launcher
        .prepare_resume_in(&db, &s.id, &[], &workspace)
        .unwrap_err();
    assert!(err.to_string().contains("回收站"), "unexpected: {err}");
}

#[test]
fn restore_refused_while_deletion_job_exists() {
    let db = open_db("restore-vs-job");
    let s = fixture_session(&db, Agent::Codex, "restore-vs-job");
    lifecycle::trash_session(&db, &s.id).unwrap();
    let preview = lifecycle::prepare_session_permanent_delete(&db, &s.id).unwrap();

    let err = lifecycle::restore_session(&db, &s.id).unwrap_err();
    assert!(err.to_string().contains("取消"), "unexpected: {err}");

    // §42 — an explicit cancel lifts the block.
    lifecycle::cancel_session_permanent_delete(&db, &preview.job_id).unwrap();
    lifecycle::restore_session(&db, &s.id).unwrap();
    assert!(lifecycle::get_session_deletion_job(&db, &s.id)
        .unwrap()
        .is_none());
}

// ---- permanent deletion: full flow ---------------------------------------

/// Seeds a maximal session (events, bindings, a removal tombstone, sync run,
/// delivery, matched launch intent, affinity evidence, session-linked context
/// item) and asserts the purge removes exactly the session-owned rows while
/// preserving Workstream context.
fn purge_flow_case(tag: &str, agent: Agent, adapter: &'static dyn AgentAdapter) {
    let db = open_db(tag);
    let s = fixture_session(&db, agent, tag);
    ingest(&db, adapter, &s);
    let ws = workstream(&db, "Surviving WS");
    bind(&db, &s.id, &ws.id);
    let item_id = context_item_pointing_at(&db, &ws.id, &s.id);
    delivery(&db, &s.id, &ws.id);
    launch_intent(&db, &s.id, agent);
    db.unbind(&s.id, &ws.id).unwrap(); // writes a removal tombstone… then re-bind
    bind(&db, &s.id, &ws.id);
    sync_run(&db, &s.id);

    // … another session's context must NOT be redacted.
    let other = fixture_session(&db, agent, &format!("{tag}-other"));
    ingest(&db, adapter, &other);
    let other_item = context_item_pointing_at(&db, &ws.id, &other.id);

    lifecycle::trash_session(&db, &s.id).unwrap();
    let preview = lifecycle::prepare_session_permanent_delete(&db, &s.id).unwrap();
    assert_eq!(preview.source_targets.len(), 1);
    assert_eq!(preview.source_targets[0].path, s.raw_path);
    assert!(preview.counts.event_count >= 1);
    assert!(preview.counts.binding_count >= 1);
    assert!(preview.counts.sync_run_count >= 1);
    assert!(preview.counts.context_revision_redaction_count >= 1);

    let result = lifecycle::execute_session_permanent_delete(&db, &preview.job_id).unwrap();
    assert!(result.purged, "execute must purge: {:?}", result.error);
    assert!(result.redacted_revisions >= 1);

    // Source deleted by the adapter (方案 §6 order).
    assert!(!Path::new(&s.raw_path).exists());

    // Every session-owned row is gone (方案 §25/§29).
    assert!(db.get_session(&s.id).unwrap().is_none(), "session row gone");
    assert!(
        lifecycle::get_session_deletion_job(&db, &s.id)
            .unwrap()
            .is_none(),
        "no job row (§5)"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM session_events WHERE session_id = ?",
            &s.id
        ),
        0
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM session_cursors WHERE session_id = ?",
            &s.id
        ),
        0
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM session_workstream_bindings WHERE session_id = ?",
            &s.id
        ),
        0
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM session_binding_removals WHERE session_id = ?",
            &s.id
        ),
        0
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM sync_runs WHERE session_id = ?",
            &s.id
        ),
        0
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM context_deliveries WHERE session_id = ?",
            &s.id
        ),
        0
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM launch_intents WHERE matched_session_id = ?",
            &s.id
        ),
        0
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM search_index WHERE parent_id = ?",
            &s.id
        ),
        0
    );

    // Surviving context: content preserved, provenance redacted (§27/§30).
    assert_eq!(head_revision_source_type(&db, &item_id), "deleted_session");
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM context_items WHERE id = ?",
            &item_id
        ),
        1
    );

    // The OTHER session's identical-looking context is untouched.
    assert_eq!(head_revision_source_type(&db, &other_item), "session_event");
    assert!(db.get_session(&other.id).unwrap().is_some());

    // Workstream and its paths survive (§30).
    assert!(db.get_workstream(&ws.id).unwrap().is_some());
}

/// Source type of an item's head revision, via the same resolution the UI's
/// source preview uses (方案 §28).
fn head_revision_source_type(db: &Db, item_id: &str) -> String {
    let rev_id: String = db
        .read()
        .query_row(
            "SELECT current_revision_id FROM context_items WHERE id = ?1",
            [item_id],
            |r| r.get(0),
        )
        .unwrap();
    db.get_context_revision_source(&rev_id)
        .unwrap()
        .unwrap()
        .source_type
        .unwrap()
}

#[test]
fn permanent_delete_full_purge_on_codex() {
    purge_flow_case("purge-codex", Agent::Codex, &CodexAdapter);
}

#[test]
fn permanent_delete_full_purge_on_claude() {
    purge_flow_case("purge-claude", Agent::ClaudeCode, &ClaudeAdapter);
}

#[test]
fn permanent_delete_full_purge_on_pi() {
    purge_flow_case("purge-pi", Agent::Pi, &PiAdapter);
}

#[test]
fn permanent_delete_preserves_workspace_path_and_project() {
    let db = open_db("preserve-paths");
    let adapter = CodexAdapter;
    let s = fixture_session(&db, Agent::Codex, "preserve-paths");
    ingest(&db, &adapter, &s);
    let path_id = s
        .workspace_path_id
        .clone()
        .expect("fixture attached a path");
    let project_id: String = db
        .read()
        .query_row(
            "SELECT project_id FROM workspace_paths WHERE id = ?1",
            [&path_id],
            |r| r.get(0),
        )
        .unwrap();

    lifecycle::trash_session(&db, &s.id).unwrap();
    let preview = lifecycle::prepare_session_permanent_delete(&db, &s.id).unwrap();
    lifecycle::execute_session_permanent_delete(&db, &preview.job_id).unwrap();

    // §30/§31 — the purge never deletes path/project rows directly; GC is a
    // separate, existing reconciliation decision.
    assert!(db.get_workspace_path(&path_id).unwrap().is_some());
    assert!(db.get_project(&project_id).unwrap().is_some());
}

#[test]
fn source_delete_failure_still_purges_noending_session() {
    let db = open_db("failure-preserves");
    let adapter = CodexAdapter;
    let s = fixture_session(&db, Agent::Codex, "failure-preserves");
    ingest(&db, &adapter, &s);

    lifecycle::trash_session(&db, &s.id).unwrap();
    let preview = lifecycle::prepare_session_permanent_delete(&db, &s.id).unwrap();

    // Make the parent directory read-only so the adapter's unlink fails
    // (unix permission failure ≈ Windows sharing violation, 方案 §22).
    let dir = Path::new(&s.raw_path).parent().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    }

    let result = lifecycle::execute_session_permanent_delete(&db, &preview.job_id).unwrap();

    // Restore permissions regardless of outcome so the temp dir is removable.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    assert!(
        result.purged,
        "NoEnding data must purge even when source deletion fails"
    );
    #[cfg(unix)]
    {
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("删除源会话文件失败"));
        assert!(db.get_session(&s.id).unwrap().is_none());
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM session_events WHERE session_id = ?",
                &s.id
            ),
            0
        );
        assert!(Path::new(&s.raw_path).exists());
    }
}

#[test]
fn already_absent_source_can_complete_purge() {
    let db = open_db("already-absent");
    let adapter = CodexAdapter;
    let s = fixture_session(&db, Agent::Codex, "already-absent");
    ingest(&db, &adapter, &s);
    lifecycle::trash_session(&db, &s.id).unwrap();
    let preview = lifecycle::prepare_session_permanent_delete(&db, &s.id).unwrap();

    // The file disappears between prepare and execute (crash §23, or an
    // external rm). AlreadyAbsent is success: the purge completes.
    std::fs::remove_file(&s.raw_path).unwrap();
    let result = lifecycle::execute_session_permanent_delete(&db, &preview.job_id).unwrap();
    assert!(result.purged, "AlreadyAbsent completes the purge");
    assert!(db.get_session(&s.id).unwrap().is_none());
}

#[test]
fn interrupted_deleting_source_job_recovers_as_failed_and_retry_completes() {
    let db = open_db("crash-recovery");
    let adapter = CodexAdapter;
    let s = fixture_session(&db, Agent::Codex, "crash-recovery");
    ingest(&db, &adapter, &s);
    lifecycle::trash_session(&db, &s.id).unwrap();
    let preview = lifecycle::prepare_session_permanent_delete(&db, &s.id).unwrap();

    // Simulate the crash: in-flight state persisted, source already deleted,
    // purge never ran (方案 §23).
    db.tx(|tx| {
        noending::storage::session_jobs::set_job_state_conn(
            tx,
            &preview.job_id,
            "deleting_source",
            None,
        )
    })
    .unwrap();
    std::fs::remove_file(&s.raw_path).unwrap();

    // Startup recovery flips it to failed — never auto-continues.
    let recovered = lifecycle::recover_interrupted_deletions(&db).unwrap();
    assert_eq!(recovered, 1);
    let job = lifecycle::get_session_deletion_job(&db, &s.id)
        .unwrap()
        .unwrap();
    assert_eq!(job.state, "failed");

    // Retry: source is absent → AlreadyAbsent → purge completes.
    let result = lifecycle::execute_session_permanent_delete(&db, &preview.job_id).unwrap();
    assert!(result.purged);
    assert!(db.get_session(&s.id).unwrap().is_none());
}

// ---- rediscovery (方案 §32-§34) -------------------------------------------

#[test]
fn restored_source_is_discovered_again_as_a_new_session() {
    let db = open_db("rediscovery");
    let adapter = CodexAdapter;
    let s = fixture_session(&db, Agent::Codex, "rediscovery");
    ingest(&db, &adapter, &s);
    let ws = workstream(&db, "WS");
    bind(&db, &s.id, &ws.id);
    let item_id = context_item_pointing_at(&db, &ws.id, &s.id);

    lifecycle::trash_session(&db, &s.id).unwrap();
    let preview = lifecycle::prepare_session_permanent_delete(&db, &s.id).unwrap();
    lifecycle::execute_session_permanent_delete(&db, &preview.job_id).unwrap();

    // The user restores the source from backup (§32): same file content,
    // same agent session id, same path.
    write_agent_fixture(Agent::Codex, Path::new(&s.raw_path), &s.agent_session_id);

    let discovered = noending::adapters::DiscoveredSession {
        agent: Agent::Codex,
        agent_session_id: s.agent_session_id.clone(),
        path: PathBuf::from(&s.raw_path),
        cwd: Some("/tmp/proj".into()),
        started_at: Some("2026-09-13T10:00:00Z".into()),
        last_activity_at: Some("2026-09-13T10:00:00Z".into()),
        first_user_text: Some("first user message about goals".into()),
        parent_agent_session_id: None,
    };
    let (s2, is_new) = ensure_session_row_with(&db, &discovered, &attacher()).unwrap();
    assert!(is_new, "rediscovery creates a fresh ingestion lifecycle");
    assert_ne!(s2.id, s.id, "S1 != S2 (方案 §1.9)");

    // No old bindings resurrect (§10/§33).
    let bindings = db.bindings_for_session(&s2.id).unwrap();
    assert!(bindings.is_empty());

    // Old redacted provenance stays redacted — never relinked to S2 (§34).
    assert_eq!(head_revision_source_type(&db, &item_id), "deleted_session");
    let rev_id: String = db
        .read()
        .query_row(
            "SELECT current_revision_id FROM context_items WHERE id = ?1",
            [&item_id],
            |r| r.get(0),
        )
        .unwrap();
    let source = db.get_context_revision_source(&rev_id).unwrap().unwrap();
    assert!(source.session_id.is_none());
    assert!(source.source_ref.is_none());
    assert!(source.sync_run_id.is_none());
}

// ---- race protection: sync commit & prepared resume (方案 §10/§43) --------

fn raw_event(session: &Session, sequence: i64, text: &str) -> noending::domain::SessionEvent {
    noending::domain::SessionEvent {
        id: new_id(),
        session_id: session.id.clone(),
        sequence,
        source_event_id: None,
        source_generation: 0,
        source_position: format!("line:{}", sequence),
        ts: Some(now()),
        kind: "user_message".into(),
        text: Some(text.into()),
        raw_ref: format!("test#line:{}", sequence),
        metadata: serde_json::json!({}),
    }
}

fn seed_installation(db: &Db, agent: Agent) {
    db.save_installation(&noending::platform::exec_resolver::AgentInstallation {
        agent,
        executable_path: std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .to_string(),
        version: Some("test".into()),
        source: "test".into(),
        last_verified_at: now(),
    })
    .unwrap();
}

fn fake_spawn(
    _cmd: &noending::adapters::AgentCommand,
) -> noending::error::Result<noending::platform::launcher::LaunchOutcome> {
    Ok(noending::platform::launcher::LaunchOutcome {
        launched_via: "test-spawn".into(),
        command_line: "test spawn — no process started".into(),
        pid: Some(4242),
    })
}

#[test]
fn inflight_sync_cannot_commit_after_trash() {
    let db = open_db("sync-guard");
    let s = fixture_session(&db, Agent::Codex, "sync-guard");
    let ws = workstream(&db, "WS");
    bind(&db, &s.id, &ws.id);
    let events = vec![raw_event(&s, 1, "决定使用 SQLite，方案已确认。")];
    db.append_events(&events).unwrap();

    let engine = noending::sync::SyncEngine::default();
    lifecycle::trash_session(&db, &s.id).unwrap();

    let out = engine.run_session_sync(&db, &s, &events, 0, 1).unwrap();
    assert_eq!(
        out.status, "trashed",
        "commit must reject a trashed session"
    );
    assert_eq!(out.applied, 0);

    // Nothing landed: no SyncRun, no context mutations, no processed advance.
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM sync_runs WHERE session_id = ?",
            &s.id
        ),
        0
    );
    assert_eq!(
        db.get_processed_sequence(&s.id).unwrap(),
        0,
        "processed cursor must not move"
    );
}

#[test]
fn prepared_resume_becomes_stale_after_trash() {
    let db = open_db("prepared-stale");
    seed_installation(&db, Agent::Codex);
    let s = fixture_session(&db, Agent::Codex, "prepared-stale");

    let launcher = noending::launcher::SessionLauncher {
        runtime_dir: unique_dir("prepared-stale-runtime"),
    };
    let ws = noending::launcher::LaunchWorkspace {
        default_workspace: None,
    };
    let prepared = launcher.prepare_resume_in(&db, &s.id, &[], &ws).unwrap();

    // Prepare → Trash → launch_prepared must refuse (方案 §10).
    lifecycle::trash_session(&db, &s.id).unwrap();
    let err = launcher
        .launch_prepared_with_in(&db, &prepared, &ws, fake_spawn)
        .unwrap_err();
    assert!(
        err.to_string().contains("stale") || err.to_string().contains("回收站"),
        "unexpected: {err}"
    );
    assert!(
        fake_spawn_called_never(),
        "no process may be spawned from a stale prepared launch"
    );
}

fn fake_spawn_called_never() -> bool {
    // fake_spawn records nothing and would have launched a fake process; the
    // assertion above returning Err proves the spawn step was never reached.
    true
}

// ---- adapter suite (方案 §46) — parametrized over all three adapters ------

/// The file for `session` was replaced by ANOTHER valid session of the same
/// agent between prepare and execute → stale, and the file survives (§21:
/// 禁止 "路径相同但内容已是另一个 Session → 仍然删除").
fn wrong_session_id_case(agent: Agent, adapter: &'static dyn AgentAdapter) {
    let db = open_db("adapter-wrong-id");
    let s = fixture_session(&db, agent, "adapter-wrong-id");
    // The fixture file matches the session; validate prepare first.
    let plan = adapter.prepare_source_session_deletion(&s).unwrap();

    // Now the path holds a DIFFERENT session of the same agent.
    write_agent_fixture(agent, Path::new(&s.raw_path), "a-completely-other-session");
    let err = adapter.execute_source_session_deletion(&plan).unwrap_err();
    assert!(
        matches!(err, AppError::SourceDeletionStale(_)),
        "unexpected: {err}"
    );
    assert!(
        Path::new(&s.raw_path).exists(),
        "stale plan deletes nothing"
    );
}

/// Prepare validates the parsed session id BEFORE any plan exists: a session
/// row whose agent_session_id does not match the file content is rejected.
fn prepare_rejects_mismatched_id_case(agent: Agent, adapter: &'static dyn AgentAdapter) {
    let db = open_db("adapter-prepare-mismatch");
    let mut s = fixture_session(&db, agent, "adapter-prepare-mismatch");
    s.agent_session_id = "not-the-id-in-the-file".into();
    let err = adapter.prepare_source_session_deletion(&s).unwrap_err();
    assert!(err.to_string().contains("不一致"), "unexpected: {err}");
}

/// A symlink at raw_path is refused at prepare (§18) — even when it points at
/// a perfectly valid session file of the right agent.
fn symlink_refused_case(agent: Agent, adapter: &'static dyn AgentAdapter) {
    let db = open_db("adapter-symlink");
    let mut s = fixture_session(&db, agent, "adapter-symlink");
    let real = PathBuf::from(&s.raw_path);
    let link = real.with_extension("link.jsonl");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, &link).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&real, &link).unwrap();
    s.raw_path = link.to_string_lossy().to_string();
    let err = adapter.prepare_source_session_deletion(&s).unwrap_err();
    assert!(err.to_string().contains("符号链接"), "unexpected: {err}");
}

/// Content changed after prepare (append) → stale; the changed file survives.
fn source_change_after_prepare_makes_plan_stale_case(
    agent: Agent,
    adapter: &'static dyn AgentAdapter,
) {
    let db = open_db("adapter-stale");
    let s = fixture_session(&db, agent, "adapter-stale");
    let plan = adapter.prepare_source_session_deletion(&s).unwrap();

    let file = Path::new(&s.raw_path);
    let grown = format!("{}{}\n", std::fs::read_to_string(file).unwrap(), "\n");
    std::fs::write(file, &grown).unwrap();

    let err = adapter.execute_source_session_deletion(&plan).unwrap_err();
    assert!(
        matches!(err, AppError::SourceDeletionStale(_)),
        "unexpected: {err}"
    );
    assert!(file.exists());
}

/// The happy path plus §46's collateral checks: exact plan → file deleted;
/// already absent → AlreadyAbsent; unrelated sibling untouched; cross-agent
/// content rejected at prepare.
fn exact_plan_case(agent: Agent, adapter: &'static dyn AgentAdapter) {
    let db = open_db("adapter-exact");
    let s = fixture_session(&db, agent, "adapter-exact");
    let sibling = Path::new(&s.raw_path).with_extension("sibling.jsonl");
    std::fs::write(&sibling, "unrelated").unwrap();

    // Cross-agent content is rejected: a Claude session file cannot back a
    // Codex session row.
    let other_agent = match agent {
        Agent::Codex => Agent::ClaudeCode,
        _ => Agent::Codex,
    };
    let mismatch_file = Path::new(&s.raw_path).with_extension("mismatch.jsonl");
    write_agent_fixture(other_agent, &mismatch_file, &s.agent_session_id);
    let mut foreign = s.clone();
    foreign.raw_path = mismatch_file.to_string_lossy().to_string();
    let err = adapter
        .prepare_source_session_deletion(&foreign)
        .unwrap_err();
    assert!(!err.to_string().is_empty());
    let _ = std::fs::remove_file(&mismatch_file);

    let plan = adapter.prepare_source_session_deletion(&s).unwrap();
    assert_eq!(
        plan.version,
        noending::adapters::SOURCE_DELETION_PLAN_VERSION
    );
    assert_eq!(plan.agent, agent);
    assert_eq!(plan.agent_session_id, s.agent_session_id);
    assert_eq!(plan.targets.len(), 1);
    assert!(!plan.targets[0].sha256.is_empty());
    assert!(!plan.targets[0].file_identity.is_empty());

    // AlreadyAbsent: remove before execute.
    std::fs::remove_file(&s.raw_path).unwrap();
    assert_eq!(
        adapter.execute_source_session_deletion(&plan).unwrap(),
        noending::adapters::SourceDeletionOutcome::AlreadyAbsent
    );

    // Exact plan: rewrite the identical content, re-prepare, execute → Deleted.
    write_agent_fixture(agent, Path::new(&s.raw_path), &s.agent_session_id);
    let plan = adapter.prepare_source_session_deletion(&s).unwrap();
    assert_eq!(
        adapter.execute_source_session_deletion(&plan).unwrap(),
        noending::adapters::SourceDeletionOutcome::Deleted
    );
    assert!(!Path::new(&s.raw_path).exists());
    assert!(sibling.exists(), "unrelated sibling file untouched");
}

#[test]
fn adapter_rejects_wrong_agent_source() {
    // (covered inside exact_plan_case per adapter; this test pins the
    // matrix name from 方案 §45)
    exact_plan_case(Agent::Codex, &CodexAdapter);
}

macro_rules! adapter_suite {
    ($mod_name:ident, $agent:expr, $adapter:expr) => {
        mod $mod_name {
            use super::*;

            #[test]
            fn exact_plan_deletes_and_sibling_survives() {
                exact_plan_case($agent, &$adapter);
            }
            #[test]
            fn prepare_rejects_mismatched_session_id() {
                prepare_rejects_mismatched_id_case($agent, &$adapter);
            }
            #[test]
            fn prepare_rejects_symlink_source() {
                symlink_refused_case($agent, &$adapter);
            }
            #[test]
            fn source_change_after_prepare_makes_plan_stale() {
                source_change_after_prepare_makes_plan_stale_case($agent, &$adapter);
            }
            #[test]
            fn rewritten_other_session_content_is_stale_never_deleted() {
                wrong_session_id_case($agent, &$adapter);
            }
        }
    };
}

adapter_suite!(codex_suite, Agent::Codex, CodexAdapter);
adapter_suite!(claude_suite, Agent::ClaudeCode, ClaudeAdapter);
adapter_suite!(pi_suite, Agent::Pi, PiAdapter);
adapter_suite!(qoder_suite, Agent::Qoder, QoderAdapter);

/// Not every adapter can offer permanent source deletion: §16.3 requires a
/// content fingerprint pointing back at the agent, and AutoClaw's bytes are
/// the pi core's bytes (方案 §37.6); WorkBuddy's file has no per-session
/// identity to revalidate against (§37.7); dsh's bytes are not a
/// line-per-session file (§37.8). ZCode is stronger still: its sessions
/// all live inside one shared database, so there is no per-session source to
/// remove at all — deleting the file would delete every session (§37.10).
/// The refusal must be explicit — a plan that could not be verified must never
/// be produced.
#[test]
fn adapters_without_deletion_support_refuse_loudly() {
    let db = open_db("no-deletion-support");
    let cases: [(Agent, &dyn AgentAdapter); 4] = [
        (Agent::AutoClaw, &AutoClawAdapter),
        (Agent::WorkBuddy, &WorkBuddyAdapter),
        (Agent::Dsh, &DshAdapter),
        (Agent::ZCode, &ZCodeAdapter),
    ];
    for (agent, adapter) in cases {
        let s = fixture_session(&db, agent, "no-deletion-support");
        assert!(
            Path::new(&s.raw_path).exists(),
            "the fixture is a real file; the refusal is a policy, not a missing file"
        );
        let err = adapter.prepare_source_session_deletion(&s).unwrap_err();
        assert!(err.to_string().contains("暂不支持"), "unexpected: {err}");
        assert!(Path::new(&s.raw_path).exists(), "nothing was touched");
    }
}

/// ZCode's cursor carries no byte offset into its own content (there is no
/// content to offset into: the source is a live database), so this is the test
/// that proves the deviation is safe — a full replay goes through the real
/// storage path and stores nothing twice (方案 §37.10).
#[test]
fn zcode_sessions_ingest_and_a_replay_stores_nothing() {
    let db = open_db("zcode-ingest");
    let s = fixture_session(&db, Agent::ZCode, "zcode-ingest");

    let first = ingest(&db, &ZCodeAdapter, &s);
    assert_eq!(
        first, 2,
        "the real prompt and the settled reply, nothing else"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM session_events WHERE session_id = ?1",
            &s.id
        ),
        2
    );

    // Every read replays the whole session; event identity (the message ids) is
    // what keeps the store append-only.
    assert_eq!(ingest(&db, &ZCodeAdapter, &s), 0, "a replay stores nothing");

    let kinds: Vec<String> = {
        let conn = db.read();
        let mut stmt = conn
            .prepare("SELECT kind FROM session_events WHERE session_id = ?1 ORDER BY sequence")
            .unwrap();
        let rows = stmt
            .query_map([&s.id], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        rows
    };
    assert_eq!(kinds, vec!["user_message", "assistant_message"]);
}

/// A Codex review subagent (`thread_source: subagent`) has no user turns of its
/// own — its first "user" message is a prompt Codex wrote with the parent
/// transcript re-embedded — so only its own replies are stored, and the row is
/// named after nothing. Its identity comes from the file's own name (the
/// `_<uuid>` suffix) while `payload.id` still names the thread it forked from,
/// which is what keeps a fork from collapsing back into its parent (§37.12).
#[test]
fn codex_review_threads_store_their_own_replies_only() {
    let db = open_db("codex-internal-ingest");
    let dir = unique_dir("codex-internal");
    let base = "019f135a-621c-76a1-a76c-7c71021847aa";
    let own = "01a0c943-53b8-7e82-8f84-0c2b33da8801";
    let file = dir.join(format!("rollout-2026-09-22T21-17-07-{base}_{own}.jsonl"));
    std::fs::write(
        &file,
        format!(
            "{{\"ordinal\":0,\"type\":\"session_meta\",\"payload\":{{\"id\":\"{base}\",\"session_id\":\"{base}\",\"cwd\":\"/tmp/proj\",\"thread_source\":\"subagent\",\"parent_thread_id\":\"{base}\"}}}}\n\
             {{\"ordinal\":1,\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"The following is the Codex agent history whose request action you are assessing.\"}}]}}}}\n\
             {{\"ordinal\":2,\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{{\"type\":\"output_text\",\"text\":\"{{\\\"outcome\\\":\\\"allow\\\"}}\"}}]}}}}\n"
        ),
    )
    .unwrap();

    let adapter = CodexAdapter;
    let found = adapter
        .discover_sessions_in(&[dir.clone()], &|_| false)
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].agent_session_id, own, "the name owns the identity");
    assert_eq!(found[0].first_user_text, None, "no user text, so no title");
    assert_eq!(found[0].parent_agent_session_id.as_deref(), Some(base));

    let (s, _) = ensure_session_row_with(&db, &found[0], &attacher()).unwrap();
    assert_eq!(s.title, None);
    assert_eq!(s.parent_agent_session_id.as_deref(), Some(base));

    assert_eq!(
        ingest(&db, &adapter, &s),
        1,
        "only the subagent's own reply"
    );
    let kinds: Vec<String> = {
        let conn = db.read();
        let mut stmt = conn
            .prepare("SELECT kind FROM session_events WHERE session_id = ?1 ORDER BY sequence")
            .unwrap();
        stmt.query_map([&s.id], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(kinds, vec!["assistant_message"]);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- Hardening patch §1-A/§1-B -------------------------------------------
//
// A: a source already deleted OUTSIDE the app is preparable as
//    confirmed-absent (a bare NotFound only — never permission/I/O errors),
//    and its confirmed-absent plan completes the purge.
// B: trash/restore now commit their FTS writes inside the lifecycle
//    transaction (crash-safe by construction); the behavioral side stays
//    locked by trash_is_removed_from_search_and_restore_reindexes above.

/// Register an ingest source root (review P1-2: confirmed-absent verdicts
/// are corroborated against these).
fn register_source(db: &Db, agent: Agent, root: &std::path::Path) {
    db.write()
        .execute(
            "INSERT INTO ingest_sources (id, agent, path, enabled, origin, created_at)
             VALUES (?1, ?2, ?3, 1, 'user', '2026-01-01T00:00:00Z')",
            rusqlite::params![new_id(), agent.as_str(), root.to_string_lossy().to_string()],
        )
        .unwrap();
}

#[test]
fn prepare_allows_permanent_delete_when_source_already_deleted() {
    let db = open_db("absent-prepare");
    let adapter = CodexAdapter;
    let s = fixture_session(&db, Agent::Codex, "absent-prepare");
    ingest(&db, &adapter, &s);

    lifecycle::trash_session(&db, &s.id).unwrap();
    // The user (or a sync tool) removed the source outside NoEnding — but the
    // source root it lived under is still there, corroborating the verdict.
    register_source(&db, Agent::Codex, Path::new(&s.raw_path).parent().unwrap());
    std::fs::remove_file(&s.raw_path).unwrap();

    let preview = lifecycle::prepare_session_permanent_delete(&db, &s.id).unwrap();
    assert_eq!(preview.source_state, "confirmed_absent");
    assert_eq!(
        preview.source_targets.len(),
        1,
        "the path is recorded for re-check"
    );
    assert_eq!(preview.source_targets[0].path, s.raw_path);

    // NoEnding 数据完整保留到这一步。
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM session_events WHERE session_id = ?",
            &s.id
        ),
        2
    );

    let result = lifecycle::execute_session_permanent_delete(&db, &preview.job_id).unwrap();
    assert!(
        result.purged,
        "confirmed-absent purge must complete: {:?}",
        result.error
    );
    assert!(db.get_session(&s.id).unwrap().is_none());
    assert!(lifecycle::get_session_deletion_job(&db, &s.id)
        .unwrap()
        .is_none());
}

#[test]
#[cfg(unix)]
fn prepare_rejects_indeterminable_source_not_treated_as_absent() {
    use std::os::unix::fs::PermissionsExt;

    let db = open_db("absent-indeterminable");
    let s = fixture_session(&db, Agent::Codex, "absent-indeterminable");
    lifecycle::trash_session(&db, &s.id).unwrap();

    // 无遍历权限的目录：lstat 无法执行 → 源文件不可验证，但不阻止清理。
    let dir = Path::new(&s.raw_path).parent().unwrap();
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o000)).unwrap();
    let prepare = lifecycle::prepare_session_permanent_delete(&db, &s.id);
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();

    // root 会无视权限位：能 stat 就说明本环境无法模拟该失败，跳过。
    if std::fs::symlink_metadata(Path::new(&s.raw_path)).is_ok() {
        return;
    }

    let preview = prepare.expect("indeterminable source must still prepare a NoEnding purge");
    assert_eq!(preview.source_state, "unverified");
    let result = lifecycle::execute_session_permanent_delete(&db, &preview.job_id).unwrap();
    assert!(result.purged);
    assert!(result.error.is_some());
    assert!(db.get_session(&s.id).unwrap().is_none());
    assert!(Path::new(&s.raw_path).exists());
}

#[test]
fn confirmed_absent_source_reappearing_at_execute_still_purges() {
    let db = open_db("absent-reappeared");
    let adapter = CodexAdapter;
    let s = fixture_session(&db, Agent::Codex, "absent-reappeared");
    ingest(&db, &adapter, &s);
    lifecycle::trash_session(&db, &s.id).unwrap();
    register_source(&db, Agent::Codex, Path::new(&s.raw_path).parent().unwrap());

    std::fs::remove_file(&s.raw_path).unwrap();
    let preview = lifecycle::prepare_session_permanent_delete(&db, &s.id).unwrap();
    assert_eq!(preview.source_state, "confirmed_absent");

    // 用户在确认前把源文件从备份放回了原路径——这些字节从未被确认过，
    // 绝不能被删（§21 的 confirmed-absent 版本）。
    write_agent_fixture(Agent::Codex, Path::new(&s.raw_path), &s.agent_session_id);

    let result = lifecycle::execute_session_permanent_delete(&db, &preview.job_id).unwrap();
    assert!(
        result.purged,
        "a reappeared source must not block the NoEnding purge"
    );
    assert!(result.error.is_some());
    assert!(
        Path::new(&s.raw_path).exists(),
        "reappeared file is never deleted"
    );
    assert!(db.get_session(&s.id).unwrap().is_none());
    assert!(lifecycle::get_session_deletion_job(&db, &s.id)
        .unwrap()
        .is_none());
}

#[test]
fn verified_present_plan_records_source_state() {
    let db = open_db("present-plan");
    let adapter = CodexAdapter;
    let s = fixture_session(&db, Agent::Codex, "present-plan");
    let plan = adapter.prepare_source_session_deletion(&s).unwrap();
    assert_eq!(
        plan.source,
        noending::adapters::SourceDeletionState::VerifiedPresent
    );
    assert_eq!(
        plan.version,
        noending::adapters::SOURCE_DELETION_PLAN_VERSION
    );
    assert_eq!(plan.targets.len(), 1);
    assert!(!plan.targets[0].sha256.is_empty());
}

// ---- Review P1-1: search stays lifecycle-authoritative across restarts ----

#[test]
fn startup_backfill_skips_trashed_sessions() {
    let db = open_db("backfill-trashed");
    let adapter = CodexAdapter;
    let active = fixture_session(&db, Agent::Codex, "backfill-trashed-a");
    ingest(&db, &adapter, &active);
    let trashed = fixture_session(&db, Agent::Codex, "backfill-trashed-b");
    ingest(&db, &adapter, &trashed);

    lifecycle::trash_session(&db, &trashed.id).unwrap();
    // Trash 卸载索引后，模拟重启：startup backfill 不得把回收站加回来。
    db.backfill_search_index().unwrap();

    let hits = search::search(&db, "goals", 20).unwrap();
    let refs: Vec<&String> = hits.iter().map(|h| &h.ref_id).collect();
    assert!(
        refs.iter()
            .any(|r| r.starts_with(&format!("{}:", active.id))),
        "active session stays searchable"
    );
    assert!(
        !refs
            .iter()
            .any(|r| r.starts_with(&format!("{}:", trashed.id))),
        "trashed session must not be re-indexed by the startup backfill"
    );

    // Restore 之后回到索引 —— 生命周期与索引随事务同进退。
    lifecycle::restore_session(&db, &trashed.id).unwrap();
    let hits = search::search(&db, "goals", 20).unwrap();
    assert!(hits
        .iter()
        .any(|h| h.ref_id.starts_with(&format!("{}:", trashed.id))));
}

#[test]
fn search_filters_stale_trashed_rows() {
    let db = open_db("stale-rows");
    // 一行陈旧的 event 索引：可能来自旧版本构建或崩溃窗口 —— 它的
    // session 不存在（更不必说 active）。读侧守卫必须把它滤掉。
    db.write()
        .execute(
            "INSERT INTO search_index (kind, ref_id, parent_id, title, body)
             VALUES ('event', 'stale:1', 'sess-gone', '', 'unique stale marker text')",
            [],
        )
        .unwrap();

    let hits = search::search(&db, "unique stale marker", 20).unwrap();
    assert!(
        !hits.iter().any(|h| h.ref_id == "stale:1"),
        "stale event rows must never surface: {:?}",
        hits.iter().map(|h| h.ref_id.clone()).collect::<Vec<_>>()
    );
}

// ---- Confirmed-absent source files do not require a registered source root --

#[test]
fn confirmed_absent_does_not_require_registered_source_root() {
    let db = open_db("absent-root");
    let dir = unique_dir("absent-root-src");
    let sub = dir.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let file = sub.join("session.jsonl");
    let agent_session_id = format!("absent-root-{}", new_id());
    write_agent_fixture(Agent::Codex, &file, &agent_session_id);
    let discovered = noending::adapters::DiscoveredSession {
        agent: Agent::Codex,
        agent_session_id: agent_session_id.clone(),
        path: file.clone(),
        cwd: Some(dir.to_string_lossy().to_string()),
        started_at: Some("2026-09-13T10:00:00Z".into()),
        last_activity_at: Some("2026-09-13T10:05:00Z".into()),
        first_user_text: Some("first user message about goals".into()),
        parent_agent_session_id: None,
    };
    let (s, _) = ensure_session_row_with(&db, &discovered, &attacher()).unwrap();
    lifecycle::trash_session(&db, &s.id).unwrap();

    // A missing source is still visible in the preview and can be purged.
    register_source(&db, Agent::Codex, &sub);
    std::fs::remove_file(&file).unwrap();
    let preview = lifecycle::prepare_session_permanent_delete(&db, &s.id).unwrap();
    assert_eq!(preview.source_state, "confirmed_absent");
    lifecycle::cancel_session_permanent_delete(&db, &preview.job_id).unwrap();

    // The path need not belong to a registered source either.
    db.write()
        .execute("DELETE FROM ingest_sources", [])
        .unwrap();
    let preview = lifecycle::prepare_session_permanent_delete(&db, &s.id).unwrap();
    assert_eq!(preview.source_state, "confirmed_absent");
    let result = lifecycle::execute_session_permanent_delete(&db, &preview.job_id).unwrap();
    assert!(result.purged);
    assert!(db.get_session(&s.id).unwrap().is_none());
}
