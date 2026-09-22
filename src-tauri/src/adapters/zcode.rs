//! ZCode Adapter: the session store is a **live SQLite database**,
//! `~/.zcode/cli/db/db.sqlite` (144 MB locally, WAL, mutated in place).
//!
//! This is the one agent with no transcript file at all, so none of the file
//! machinery applies: no byte offsets, no prefix fingerprints, no append
//! detection. Three tables carry the conversation (read-only open; NoEnding
//! never writes, never migrates, never VACUUMs):
//! - `session(id, parent_id, directory, title, time_created, time_updated,
//!   time_archived, task_type, …)` — `directory` is the cwd;
//! - `message(id, session_id, data, sequence)` — `data` is JSON with `role`
//!   (`user` / `assistant` — the only two roles in the store) and
//!   `time.created` / `time.completed`;
//! - `part(id, message_id, session_id, data, sequence)` — `data.type` is
//!   `text` / `reasoning` / `tool` / `step-start` / `step-finish` / `timeline`
//!   / `compaction` / `file`.
//!
//! What is ingested is therefore exactly the prose: the `text` parts of a
//! message, joined in `sequence` order, under the message's own id as the
//! native event id. Reasoning, tool traffic, step markers, timeline, and file
//! references are machine chatter; a `compaction` part becomes the usual
//! boundary marker. The user's prompt needs no unwrapping — dsh and WorkBuddy
//! wrap theirs, ZCode keeps the environment snapshot in
//! `message.data.contextSnapshot` and leaves `text` clean (measured: the first
//! user text of a real session is the sentence the user typed).
//!
//! Three decisions worth recording:
//! - **`role` is not who wrote it.** ZCode tags every message with
//!   `data.semantics.{origin,kind}`, and under `role:"user"` the runtime's own
//!   talking-to-the-model hides in plain sight: over 6 620 real messages, 308
//!   are `real_user`/`user_prompt` against 489 `todo_reminder`, 50
//!   `system_reminder`, 14 `background_notification`, 10 `compact_summary` and
//!   2 `system` reminders. Reading `role` would have ingested three times as
//!   much machine chatter as human prose — and titled every session after a
//!   reminder. Only `real_user`/`user_prompt` is a human turn (this is
//!   WorkBuddy's envelope lesson again, §37.7, in a different costume).
//! - **Only settled assistant messages.** A row is inserted when generation
//!   starts (`time.created`) and updated as it streams, and storage dedups by
//!   id — so ingesting a half-written message would freeze a truncated reply
//!   forever. A reply is read only once `time.completed` exists; user messages
//!   never carry it (measured), which is why the guard is on the reply alone.
//! - **Archived sessions are left alone.** `time_archived` is ZCode's own
//!   "retired from the list" verdict, and dropping a source is the failure the
//!   design tolerates; inventing sessions the app itself hides is the one it
//!   does not. Sub-agent sessions, by contrast, are ordinary rows with a real
//!   `parent_id`, so they are ingested with their parent link (15 of 27
//!   locally). ZCode's own `title` is not used — NoEnding's title is derived
//!   from the first user message and is write-once (§37.7's decision, applied
//!   consistently).
//!
//! There is no launchable CLI: ZCode is a desktop app (`~/.zcode/cli` is its
//! own data dir, not a user-facing command), so `detect()` never succeeds and
//! no command is built.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::adapters::{
    file_identity, ms_epoch_to_rfc3339, mtime_secs, AgentCommand, DiscoveredSession, ExecOptions,
    ParsedEvent, ReadDelta,
};
use crate::domain::{Agent, Session, SourceCursor, SourceCursorUpdate};
use crate::error::{other, Result};
use crate::platform::exec_resolver::AgentInstallation;

pub struct ZCodeAdapter;

/// The store, as ZCode lays it out under its data root. A root that IS the
/// database file is accepted too, so the user can register either.
fn db_path(root: &Path) -> Option<PathBuf> {
    if root.is_file() {
        return Some(root.to_path_buf());
    }
    let candidate = root.join("cli").join("db").join("db.sqlite");
    candidate.is_file().then_some(candidate)
}

fn open_read_only(path: &Path) -> Result<Connection> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| {
        other(format!(
            "只读打开 ZCode 数据库失败 {}: {}",
            path.display(),
            e
        ))
    })
}

/// `data.type == "text"` parts of a message, in sequence order.
fn text_of(parts: &[Value]) -> String {
    let mut ordered: Vec<&Value> = parts.iter().collect();
    ordered.sort_by_key(|p| p.get("sequence").and_then(|s| s.as_i64()).unwrap_or(0));
    ordered
        .iter()
        .filter(|p| p.pointer("/data/type").and_then(|t| t.as_str()) == Some("text"))
        .filter_map(|p| p.pointer("/data/text").and_then(|t| t.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn has_compaction(parts: &[Value]) -> bool {
    parts
        .iter()
        .any(|p| p.pointer("/data/type").and_then(|t| t.as_str()) == Some("compaction"))
}

/// The conversation kind of a message, or `None` for the runtime's own
/// traffic. See the module doc: `semantics.{origin,kind}` is the only thing
/// that separates a human turn from a reminder, and `role` cannot.
fn message_kind(data: &Value) -> Option<&'static str> {
    let origin = data.pointer("/semantics/origin").and_then(|o| o.as_str());
    let kind = data.pointer("/semantics/kind").and_then(|k| k.as_str());
    match (origin, kind) {
        (Some("real_user"), Some("user_prompt")) => Some("user_prompt"),
        (Some("agent_runtime"), Some("assistant_response")) => Some("assistant_response"),
        _ => None,
    }
}

/// A message is settled once generation finished; see the module doc.
fn is_settled(data: &Value) -> bool {
    data.pointer("/time/completed")
        .map(|c| !c.is_null())
        .unwrap_or(false)
}

/// Messages of one session, each with its parts, in sequence order.
fn conversation(conn: &Connection, session_id: &str) -> Result<Vec<(Value, Vec<Value>)>> {
    let mut parts_by_message: HashMap<String, Vec<Value>> = HashMap::new();
    let mut part_stmt = conn
        .prepare(
            "SELECT message_id, id, data, sequence FROM part WHERE session_id = ?1
             ORDER BY sequence, time_created, id",
        )
        .map_err(|e| other(format!("查询 ZCode part 失败: {e}")))?;
    let part_rows = part_stmt
        .query_map([session_id], |row| {
            let message_id: String = row.get(0)?;
            let id: String = row.get(1)?;
            let data: String = row.get(2)?;
            let sequence: Option<i64> = row.get(3)?;
            Ok((message_id, id, data, sequence))
        })
        .map_err(|e| other(format!("查询 ZCode part 失败: {e}")))?;
    for row in part_rows {
        let (message_id, id, data, sequence) =
            row.map_err(|e| other(format!("读取 ZCode part 失败: {e}")))?;
        let Some(parsed) = serde_json::from_str::<Value>(&data).ok() else {
            continue;
        };
        parts_by_message.entry(message_id).or_default().push(
            serde_json::json!({ "id": id, "data": parsed, "sequence": sequence.unwrap_or(0) }),
        );
    }

    let mut msg_stmt = conn
        .prepare("SELECT id, data FROM message WHERE session_id = ?1 ORDER BY sequence")
        .map_err(|e| other(format!("查询 ZCode message 失败: {e}")))?;
    let msg_rows = msg_stmt
        .query_map([session_id], |row| {
            let id: String = row.get(0)?;
            let data: String = row.get(1)?;
            Ok((id, data))
        })
        .map_err(|e| other(format!("查询 ZCode message 失败: {e}")))?;

    let mut out = Vec::new();
    for row in msg_rows {
        let (id, data) = row.map_err(|e| other(format!("读取 ZCode message 失败: {e}")))?;
        let Ok(data) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        let parts = parts_by_message.remove(&id).unwrap_or_default();
        out.push((serde_json::json!({ "id": id, "data": data }), parts));
    }
    Ok(out)
}

/// The events one message contributes: its prose, then a compaction boundary
/// if the message carries one.
fn events_of(message: &Value, parts: &[Value]) -> Vec<ParsedEvent> {
    let mut out = Vec::new();
    let id = message.get("id").and_then(|i| i.as_str()).unwrap_or("");
    let data = message.get("data").unwrap_or(&Value::Null);
    // A reply is only read once generation finished: the row appears when
    // streaming starts and is rewritten in place, so an early read would
    // freeze a truncated answer under its permanent event id.
    let kind = match message_kind(data) {
        Some("assistant_response") if !is_settled(data) => None,
        other => other,
    };
    if let Some(kind) = kind {
        let text = text_of(parts);
        if !text.trim().is_empty() {
            out.push(ParsedEvent {
                source_event_id: Some(id.to_string()),
                source_position: format!("msg:{id}"),
                ts: data
                    .pointer("/time/created")
                    .and_then(|t| t.as_i64())
                    .and_then(ms_epoch_to_rfc3339),
                kind: if kind == "user_prompt" {
                    "user_message".into()
                } else {
                    "assistant_message".into()
                },
                text: Some(text),
                metadata: serde_json::json!({ "agent": "zcode", "type": kind }),
            });
        }
    }
    if has_compaction(parts) {
        let part_id = parts
            .iter()
            .find(|p| p.pointer("/data/type").and_then(|t| t.as_str()) == Some("compaction"))
            .and_then(|p| p.get("id"))
            .and_then(|i| i.as_str())
            .unwrap_or(id);
        out.push(ParsedEvent {
            source_event_id: Some(part_id.to_string()),
            source_position: format!("part:{part_id}"),
            ts: None,
            kind: "compact".into(),
            text: Some("conversation compacted".into()),
            metadata: serde_json::json!({ "agent": "zcode", "type": "compaction" }),
        });
    }
    out
}

impl ZCodeAdapter {
    fn parse_session_row(row: &rusqlite::Row) -> rusqlite::Result<(DiscoveredSession, String)> {
        let id: String = row.get(0)?;
        let parent_id: Option<String> = row.get(1)?;
        let directory: Option<String> = row.get(2)?;
        let time_created: Option<i64> = row.get(3)?;
        let time_updated: Option<i64> = row.get(4)?;
        Ok((
            DiscoveredSession {
                agent: Agent::ZCode,
                agent_session_id: id.clone(),
                // Filled by the caller, which knows the database path.
                path: PathBuf::new(),
                cwd: directory.filter(|d| !d.is_empty()),
                started_at: time_created.and_then(ms_epoch_to_rfc3339),
                last_activity_at: time_updated.and_then(ms_epoch_to_rfc3339),
                first_user_text: None,
                parent_agent_session_id: parent_id,
            },
            id,
        ))
    }

    /// The first human turn, for the title — the first `real_user` prompt, not
    /// the app's own title and not the first `role:"user"` row (which is a
    /// reminder more often than not).
    fn first_user_text(conn: &Connection, session_id: &str) -> Result<Option<String>> {
        for (message, parts) in conversation(conn, session_id)? {
            let data = message.get("data").unwrap_or(&Value::Null);
            if message_kind(data) != Some("user_prompt") {
                continue;
            }
            let text = text_of(&parts);
            if !text.trim().is_empty() {
                return Ok(Some(crate::adapters::truncate_text(&text, 400)));
            }
        }
        Ok(None)
    }
}

impl crate::adapters::AgentAdapter for ZCodeAdapter {
    fn agent(&self) -> Agent {
        Agent::ZCode
    }

    /// Always `None`: ZCode is a desktop app, not a CLI (方案 §37.10).
    fn detect(&self) -> Option<AgentInstallation> {
        None
    }

    fn discover_sessions_in(
        &self,
        roots: &[PathBuf],
        _unchanged: &dyn Fn(&Path) -> bool,
    ) -> Result<Vec<DiscoveredSession>> {
        let mut out = Vec::new();
        let mut seen: Vec<PathBuf> = Vec::new();
        for root in roots {
            let Some(db) = db_path(root) else {
                continue;
            };
            if seen.contains(&db) {
                continue;
            }
            seen.push(db.clone());
            // `unchanged` is deliberately NOT consulted. It is keyed by raw
            // path, and every ZCode session shares this one path, so its verdict
            // is "SOME session on this path is ingested" — not "all of them
            // are". Worse, the store commits in WAL mode: rows can appear in
            // `db.sqlite-wal` while the main file's size and mtime stay
            // identical, so the stat-based check would call the store unchanged
            // and strand every new session until the next checkpoint. Reading
            // all live sessions every pass costs one indexed query per session
            // and is absorbed by event-identity dedup, which is the right trade
            // for never losing a session (方案 §37.10).
            let conn = open_read_only(&db)?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, parent_id, directory, time_created, time_updated FROM session
                     WHERE time_archived IS NULL ORDER BY time_created",
                )
                .map_err(|e| other(format!("查询 ZCode session 失败: {e}")))?;
            let rows = stmt
                .query_map([], Self::parse_session_row)
                .map_err(|e| other(format!("查询 ZCode session 失败: {e}")))?;
            for row in rows {
                let (mut discovered, id) =
                    row.map_err(|e| other(format!("读取 ZCode session 失败: {e}")))?;
                discovered.path = db.clone();
                discovered.first_user_text = Self::first_user_text(&conn, &id)?;
                out.push(discovered);
            }
        }
        Ok(out)
    }

    fn read_delta(&self, session: &Session, cursor: &SourceCursor) -> Result<ReadDelta> {
        let path = PathBuf::from(&session.raw_path);
        let conn = open_read_only(&path)?;
        let mut events = Vec::new();
        for (message, parts) in conversation(&conn, &session.agent_session_id)? {
            events.extend(events_of(&message, &parts));
        }

        // There is no file to seek into and no stable prefix to fingerprint:
        // the source is a database that changes under us. Identity is the
        // framework's own event ids (message and part ids), so a re-read of the
        // whole session stores nothing; a replaced database is the only shape
        // change worth a generation bump.
        let meta = std::fs::metadata(&path)?;
        let identity = file_identity(&path);
        let size = meta.len();
        let first_ever = cursor.source_file_identity.is_empty() && cursor.last_seen_size == 0;
        let generation = if first_ever || identity != cursor.source_file_identity {
            if first_ever {
                0
            } else {
                cursor.generation + 1
            }
        } else {
            cursor.generation
        };
        Ok(ReadDelta {
            events,
            source: Some(SourceCursorUpdate {
                file_identity: identity,
                generation,
                byte_offset: size,
                last_seen_size: size,
                mtime: mtime_secs(&meta),
                start_byte_offset: 0,
                prefix_hash: String::new(),
            }),
        })
    }

    fn build_new_command(
        &self,
        _install: &AgentInstallation,
        _opts: &ExecOptions,
        _context_file: Option<&Path>,
        _cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        Err(other("ZCode 是桌面应用，没有可启动的 CLI"))
    }

    fn build_resume_command(
        &self,
        _install: &AgentInstallation,
        _opts: &ExecOptions,
        _agent_session_id: &str,
        _context_file: Option<&Path>,
        _cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        Err(other("ZCode 是桌面应用，没有可启动的 CLI"))
    }

    fn build_exec_command(
        &self,
        _install: &AgentInstallation,
        _opts: &ExecOptions,
        _prompt: &str,
    ) -> Result<AgentCommand> {
        Err(other("ZCode 是桌面应用，没有可启动的 CLI"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::AgentAdapter;

    fn unique_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "noending-zcode-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The store as ZCode lays it out. The schema is the real one (columns the
    /// reader never touches are omitted only where they are nullable — the
    /// NOT NULL ones are all here, so a fixture that inserts is a fixture that
    /// could have been written by ZCode itself).
    fn store(root: &Path) -> PathBuf {
        let path = root.join("cli").join("db").join("db.sqlite");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let conn = Connection::open(&path).unwrap();
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
        path
    }

    fn open(path: &Path) -> Connection {
        Connection::open(path).unwrap()
    }

    fn session_row(conn: &Connection, id: &str, parent: Option<&str>, dir: &str, created: i64) {
        conn.execute(
            "INSERT INTO session
               (id, project_id, parent_id, slug, directory, title, version,
                time_created, time_updated)
             VALUES (?1, 'p', ?2, ?1, ?3, 'app title', '1', ?4, ?4)",
            rusqlite::params![id, parent, dir, created],
        )
        .unwrap();
    }

    fn archive(conn: &Connection, id: &str, at: i64) {
        conn.execute(
            "UPDATE session SET time_archived = ?2 WHERE id = ?1",
            rusqlite::params![id, at],
        )
        .unwrap();
    }

    fn message_row(conn: &Connection, id: &str, session: &str, seq: i64, data: Value) {
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data, sequence)
             VALUES (?1, ?2, 0, 0, ?3, ?4)",
            rusqlite::params![id, session, data.to_string(), seq],
        )
        .unwrap();
    }

    fn rewrite_message(conn: &Connection, id: &str, data: Value) {
        conn.execute(
            "UPDATE message SET data = ?2 WHERE id = ?1",
            rusqlite::params![id, data.to_string()],
        )
        .unwrap();
    }

    fn part_row(conn: &Connection, id: &str, message: &str, session: &str, seq: i64, data: Value) {
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data, sequence)
             VALUES (?1, ?2, ?3, 0, 0, ?4, ?5)",
            rusqlite::params![id, message, session, data.to_string(), seq],
        )
        .unwrap();
    }

    /// A real human turn. The words live in the message's `text` parts, not in
    /// the message data — which is exactly what keeps ZCode's prose clean.
    fn prompt() -> Value {
        serde_json::json!({
            "role": "user", "time": {"created": 1788012788361i64},
            "semantics": {"origin": "real_user", "kind": "user_prompt"},
            "contextSnapshot": {"envInfo": {"cwd": "/repo"}},
        })
    }

    /// The runtime talking to the model under `role:"user"`.
    fn reminder(kind: &str) -> Value {
        serde_json::json!({
            "role": "user", "time": {"created": 1788538485030i64},
            "semantics": {"origin": "agent_runtime", "kind": kind},
        })
    }

    fn reply(settled: bool) -> Value {
        let mut v = serde_json::json!({
            "role": "assistant", "time": {"created": 1788012788390i64},
            "semantics": {"origin": "agent_runtime", "kind": "assistant_response"},
        });
        if settled {
            v["time"]["completed"] = serde_json::json!(1788012789059i64);
        }
        v
    }

    fn text_part(text: &str) -> Value {
        serde_json::json!({"type": "text", "text": text})
    }

    fn session_of(db: &Path, id: &str) -> Session {
        Session {
            id: "sess-zcode".into(),
            agent: Agent::ZCode,
            agent_session_id: id.into(),
            title: None,
            cwd: None,
            workspace_path_id: None,
            project_id: None,
            raw_path: db.to_string_lossy().to_string(),
            parent_agent_session_id: None,
            started_at: None,
            last_activity_at: None,
            trashed_at: None,
        }
    }

    fn cursor_of(u: &SourceCursorUpdate) -> SourceCursor {
        SourceCursor {
            session_id: "sess-zcode".into(),
            source_file_identity: u.file_identity.clone(),
            generation: u.generation,
            byte_offset: u.byte_offset,
            last_seen_size: u.last_seen_size,
            mtime: u.mtime,
            prefix_hash: u.prefix_hash.clone(),
            identity_tail_hash: String::new(),
            last_sequence: 0,
        }
    }

    #[test]
    fn the_root_or_the_database_file_itself_is_accepted() {
        let root = unique_dir("db-path");
        let db = store(&root);
        assert_eq!(db_path(&root), Some(db.clone()));
        assert_eq!(db_path(&db), Some(db));
        assert_eq!(db_path(&root.join("cli")), None, "not every dir is a store");
    }

    #[test]
    fn discovery_lists_live_sessions_with_cwd_and_parent_and_skips_archived() {
        let root = unique_dir("discover");
        let db = store(&root);
        let conn = open(&db);
        session_row(&conn, "root-child", Some("main"), "/repo/sub", 300);
        session_row(&conn, "main", None, "/repo", 200);
        session_row(&conn, "retired", None, "/repo/old", 100);
        archive(&conn, "retired", 400);
        drop(conn);

        let found = ZCodeAdapter
            .discover_sessions_in(&[root], &|_| false)
            .unwrap();
        // Newest first: the store's `ORDER BY time_created`, and the archived
        // row is ZCode's own "hidden" verdict — NoEnding does not resurrect it.
        let ids: Vec<&str> = found.iter().map(|s| s.agent_session_id.as_str()).collect();
        assert_eq!(ids, vec!["main", "root-child"]);
        let child = &found[1];
        assert_eq!(child.parent_agent_session_id.as_deref(), Some("main"));
        assert_eq!(child.cwd.as_deref(), Some("/repo/sub"));
        assert_eq!(child.agent, Agent::ZCode);
        assert_eq!(child.path, db);
    }

    #[test]
    fn the_title_is_the_first_real_prompt_not_a_reminder() {
        let root = unique_dir("title");
        let db = store(&root);
        let conn = open(&db);
        session_row(&conn, "s", None, "/repo", 100);
        // The runtime speaks first and much more often: a reminder would have
        // supplied the title if `role` were trusted (方案 §37.10).
        message_row(&conn, "m1", "s", 0, reminder("todo_reminder"));
        part_row(
            &conn,
            "p1",
            "m1",
            "s",
            0,
            text_part("The TodoWrite tool hasn't been used recently."),
        );
        message_row(&conn, "m2", "s", 1, prompt());
        part_row(&conn, "p2", "m2", "s", 0, text_part("真正的问题"));
        drop(conn);

        let found = ZCodeAdapter
            .discover_sessions_in(&[root], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].first_user_text.as_deref(), Some("真正的问题"));
    }

    #[test]
    fn reminders_reasoning_and_tool_traffic_are_not_conversation() {
        let root = unique_dir("kinds");
        let db = store(&root);
        let conn = open(&db);
        session_row(&conn, "s", None, "/repo", 100);
        message_row(&conn, "m1", "s", 0, reminder("todo_reminder"));
        part_row(&conn, "p1", "m1", "s", 0, text_part("tool reminder"));
        message_row(&conn, "m2", "s", 1, reminder("system_reminder"));
        part_row(&conn, "p2", "m2", "s", 0, text_part("system reminder"));
        message_row(&conn, "m3", "s", 2, prompt());
        part_row(&conn, "p3", "m3", "s", 0, text_part("人写的"));
        message_row(&conn, "m4", "s", 3, reply(true));
        // Prose is only the `text` parts, and they are joined in `sequence`
        // order: reasoning and tool payloads are not the answer.
        part_row(&conn, "p4", "m4", "s", 0, text_part("回答"));
        part_row(
            &conn,
            "p5",
            "m4",
            "s",
            1,
            serde_json::json!({"type": "reasoning", "text": "thinking"}),
        );
        part_row(
            &conn,
            "p6",
            "m4",
            "s",
            2,
            serde_json::json!({"type": "tool", "tool": "bash"}),
        );
        part_row(&conn, "p7", "m4", "s", 3, text_part("尾部"));
        drop(conn);

        let delta = ZCodeAdapter
            .read_delta(&session_of(&db, "s"), &SourceCursor::default())
            .unwrap();
        let got: Vec<(&str, &str, Option<&str>)> = delta
            .events
            .iter()
            .map(|e| {
                (
                    e.kind.as_str(),
                    e.text.as_deref().unwrap_or(""),
                    e.source_event_id.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("user_message", "人写的", Some("m3")),
                ("assistant_message", "回答\n尾部", Some("m4")),
            ]
        );
        // The cursor stays in the file's own coordinates: nothing was seeked.
        assert_eq!(
            delta.source.unwrap().byte_offset,
            std::fs::metadata(&db).unwrap().len()
        );
    }

    #[test]
    fn a_reply_is_read_only_once_it_settles() {
        let root = unique_dir("settled");
        let db = store(&root);
        let conn = open(&db);
        session_row(&conn, "s", None, "/repo", 100);
        message_row(&conn, "m1", "s", 0, prompt());
        part_row(&conn, "p1", "m1", "s", 0, text_part("提问"));
        message_row(&conn, "m2", "s", 1, reply(false));
        part_row(&conn, "p2", "m2", "s", 0, text_part("半句"));
        drop(conn);

        let session = session_of(&db, "s");
        let first = ZCodeAdapter
            .read_delta(&session, &SourceCursor::default())
            .unwrap();
        let ids = |d: &ReadDelta| -> Vec<String> {
            d.events
                .iter()
                .filter_map(|e| e.source_event_id.clone())
                .collect()
        };
        assert_eq!(
            ids(&first),
            vec!["m1"],
            "a half-written reply is not frozen"
        );

        let conn = open(&db);
        rewrite_message(&conn, "m2", reply(true));
        drop(conn);

        // The replay is unconditional, so the prompt comes back too — and that
        // is fine: event identity is the message id, so the store keeps it once.
        let second = ZCodeAdapter
            .read_delta(&session, &cursor_of(first.source.as_ref().unwrap()))
            .unwrap();
        assert_eq!(ids(&second), vec!["m1", "m2"]);
        assert_eq!(
            second.source.unwrap().generation,
            first.source.unwrap().generation,
            "settling a reply is not a change of source shape"
        );
    }

    #[test]
    fn compaction_is_a_boundary_not_a_message() {
        let root = unique_dir("compact");
        let db = store(&root);
        let conn = open(&db);
        session_row(&conn, "s", None, "/repo", 100);
        // The summary is generated text, so the message itself is not prose;
        // the `compaction` PART is the structural marker (measured: 20 parts
        // across the local store, one per compaction event).
        message_row(&conn, "m1", "s", 0, reminder("compact_summary"));
        part_row(
            &conn,
            "cmp1",
            "m1",
            "s",
            0,
            serde_json::json!({"type": "compaction"}),
        );
        part_row(
            &conn,
            "p1",
            "m1",
            "s",
            1,
            text_part("这是摘要，不是用户说的话"),
        );
        drop(conn);

        let delta = ZCodeAdapter
            .read_delta(&session_of(&db, "s"), &SourceCursor::default())
            .unwrap();
        let got: Vec<(&str, Option<&str>)> = delta
            .events
            .iter()
            .map(|e| (e.kind.as_str(), e.source_event_id.as_deref()))
            .collect();
        assert_eq!(got, vec![("compact", Some("cmp1"))]);
    }
}
