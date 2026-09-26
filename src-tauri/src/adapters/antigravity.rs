//! Antigravity Adapter (Google's agentic IDE): the conversation store is one
//! live SQLite database per conversation,
//! `~/.gemini/antigravity/conversations/<conversation-id>.db` (WAL, mutated
//! in place). Raw data is read-only, always.
//!
//! Store layout, measured on this machine's real corpus:
//! - `steps` — the whole event log, ordered by `idx`. Payloads are protobuf
//!   blobs with no published schema; only these step types carry
//!   conversation-relevant content (field paths verified against the blobs):
//!     - 14  user turn     → `payload.19.2` (fallback `19.3.1`) is the typed
//!       prompt, `payload.5.1` the timestamp;
//!     - 15  agent turn    → each `payload.20` part may carry visible prose
//!       (`20.3`); a `20.7` part only ANNOUNCES a tool call — not counted;
//!     - 132 tool call     → the execution record of the announced call
//!       (same call id, immediately after; 827/828 ids appear in both) —
//!       the single tool_call authority, and 17 (API error) a tool error;
//!     - 23/101 generation metadata and inter-agent notices → ignored.
//!   Steps are appended once and never rewritten, so message identity is
//!   `step:<idx>` and the full replay dedups exactly.
//! - `conversation_summaries.db` (sibling store) — the session list: title
//!   (may be empty; `preview` — the first user input — is the natural
//!   fallback), `parent_conversation_id` (subagent conversations) and
//!   `workspace_uris` (cwd).
//! - `trajectory_metadata_blob` — workspace URI + the conversation's start
//!   timestamp.
//!
//! Member mapping: one conversation, one member. A conversation with a
//! `parent_conversation_id` is a CHILD of that parent; its steps contribute
//! execution observations only. The parent relation comes from the summaries
//! store, and a conversation whose summary row has not landed yet is SKIPPED —
//! unknown is never promoted to Root (a root later re-homed as a child would
//! leave a ghost Logical Session with no root member).
//!
//! Change detection: `unchanged` is deliberately not consulted. The store is
//! live SQLite in WAL mode — new steps can sit only in `<id>.db-wal` while
//! the main file's size/mtime stay identical — and the title/parent half of
//! the facts lives in a second store the skipset never sees. Discovery
//! re-reads every conversation on every pass; the replay dedup absorbs it.
//!
//! Message provenance: the source carries no per-message model/provider —
//! the model name only appears in runtime feature-flag blobs, which is
//! configuration, not generation evidence. Everything stays NULL.
//!
//! There is no headless CLI, so this adapter ingests history only.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::adapters::{
    ms_epoch_to_rfc3339, replay_cursor_update, AgentCommand, DiscoveredMember,
    DiscoveredMemberKind, ExecOptions, MemberObservation, MemberReadDelta, ParsedLine,
    SessionMessageRole,
};
use crate::domain::{Agent, SessionMember, SessionMemberCursor, SourceAvailability};
use crate::error::{other, Result};

pub struct AntigravityAdapter;

/// The conversations directory under the data root; a root that IS a
/// conversation `.db` file is accepted too.
fn conversations_dir(root: &Path) -> Option<PathBuf> {
    if root.is_file() && root.extension().and_then(|e| e.to_str()) == Some("db") {
        return root.parent().map(|p| p.to_path_buf());
    }
    let candidate = root.join("conversations");
    candidate.is_dir().then_some(candidate)
}

fn open_read_only(path: &Path) -> Result<Connection> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| {
        other(format!(
            "只读打开 Antigravity 数据库失败 {}: {}",
            path.display(),
            e
        ))
    })
}

// ---- schema-less protobuf walk (the blobs have no published schema) ----

/// One wire-format field: field number plus either a varint or a byte slice.
enum PbVal<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
}

fn pb_fields(buf: &[u8]) -> Vec<(u32, PbVal<'_>)> {
    let mut out = Vec::new();
    let (mut i, n) = (0usize, buf.len());
    while i < n {
        let mut tag = 0u64;
        let mut shift = 0u32;
        loop {
            if i >= n {
                return out;
            }
            let b = buf[i];
            i += 1;
            tag |= ((b & 0x7f) as u64) << shift;
            shift += 7;
            if b & 0x80 == 0 {
                break;
            }
        }
        let fnum = (tag >> 3) as u32;
        match tag & 7 {
            0 => {
                let mut v = 0u64;
                let mut shift = 0u32;
                loop {
                    if i >= n {
                        return out;
                    }
                    let b = buf[i];
                    i += 1;
                    v |= ((b & 0x7f) as u64) << shift;
                    shift += 7;
                    if b & 0x80 == 0 {
                        break;
                    }
                }
                out.push((fnum, PbVal::Varint(v)));
            }
            2 => {
                let mut ln = 0u64;
                let mut shift = 0u32;
                loop {
                    if i >= n {
                        return out;
                    }
                    let b = buf[i];
                    i += 1;
                    ln |= ((b & 0x7f) as u64) << shift;
                    shift += 7;
                    if b & 0x80 == 0 {
                        break;
                    }
                }
                let end = i + ln as usize;
                if end > n {
                    return out;
                }
                out.push((fnum, PbVal::Bytes(&buf[i..end])));
                i = end;
            }
            5 => i += 4,
            1 => i += 8,
            _ => return out,
        }
    }
    out
}

fn pb_varint(buf: &[u8], fnum: u32) -> Option<u64> {
    pb_fields(buf).into_iter().find_map(|(f, v)| match (f, v) {
        (n, PbVal::Varint(v)) if n == fnum => Some(v),
        _ => None,
    })
}

fn pb_sub<'a>(buf: &'a [u8], fnum: u32) -> Option<&'a [u8]> {
    pb_fields(buf).into_iter().find_map(|(f, v)| match (f, v) {
        (n, PbVal::Bytes(b)) if n == fnum && !b.is_empty() => Some(b),
        _ => None,
    })
}

fn pb_str(buf: &[u8], fnum: u32) -> Option<String> {
    pb_sub(buf, fnum).and_then(|b| String::from_utf8(b.to_vec()).ok())
}

fn pb_text(buf: &[u8], fnum: u32) -> Option<String> {
    pb_sub(buf, fnum)
        .and_then(|b| String::from_utf8(b.to_vec()).ok())
        .filter(|s| !s.trim().is_empty())
}

/// `{seconds, nanos}` protobuf timestamp → RFC3339. Step envelopes keep it at
/// `payload.5.1`.
fn step_timestamp(payload: &[u8]) -> Option<String> {
    let envelope = pb_sub(payload, 5)?;
    let ts = pb_sub(envelope, 1)?;
    let secs = pb_varint(ts, 1)? as i64;
    ms_epoch_to_rfc3339(secs * 1000)
}

/// The typed user prompt of a step-14 payload.
fn user_text(payload: &[u8]) -> Option<String> {
    let turn = pb_sub(payload, 19)?;
    pb_text(turn, 2)
        .or_else(|| pb_sub(turn, 3).and_then(|m| pb_text(m, 1)))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Visible prose parts of a step-15 payload. The `20.7` tool-call parts are
/// NOT counted here: the call is announced in the 15 and EXECUTED as its own
/// step-132 record with the same call id (verified: 827/828 ids appear in
/// both, announcement immediately before execution) — counting both would
/// double-count, and 132 alone covers the calls that have no 15 announcement.
fn agent_turn(payload: &[u8]) -> Vec<String> {
    let mut prose = Vec::new();
    for (fnum, val) in pb_fields(payload) {
        if fnum != 20 {
            continue;
        }
        let PbVal::Bytes(part) = val else { continue };
        if let Some(text) = pb_text(part, 3) {
            prose.push(text);
        }
    }
    prose
}

/// One conversation DB's facts for discovery: cwd + start time from its
/// metadata blob, title / parent / workspace from the summaries store.
///
/// The summaries ROW is load-bearing, not decoration: `parent_conversation_id`
/// decides Root vs Child. When the row is missing — the summaries store lags
/// the conversation DB in normal dual-store writes — the relation is UNKNOWN,
/// and unknown must never be promoted to Root: a member created as a root and
/// upserted into a different logical session later would leave a ghost
/// Logical Session with no root member behind. So a row-less conversation is
/// skipped this round and picked up on the next reconcile.
fn parse_member(
    db_path: &Path,
    summaries: Option<&Connection>,
) -> Result<Option<DiscoveredMember>> {
    let Some(conversation_id) = db_path.file_stem().and_then(|s| s.to_str()) else {
        return Ok(None);
    };
    if !conversation_id
        .bytes()
        .all(|b| b == b'-' || b.is_ascii_hexdigit())
    {
        return Ok(None);
    }

    let conn = match open_read_only(db_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[discover] skip {}: {}", db_path.display(), e);
            return Ok(None);
        }
    };
    let metadata: Option<Vec<u8>> = conn
        .query_row(
            "SELECT data FROM trajectory_metadata_blob WHERE id = 'main'",
            [],
            |r| r.get(0),
        )
        .ok();
    let started_at = metadata.as_deref().and_then(|m| {
        let ts = pb_sub(m, 2)?;
        let secs = pb_varint(ts, 1)? as i64;
        ms_epoch_to_rfc3339(secs * 1000)
    });
    drop(conn);

    let summary: Option<(String, String, Option<String>, String)> = summaries.and_then(|c| {
        c.query_row(
            "SELECT title, preview, parent_conversation_id, workspace_uris
             FROM conversation_summaries WHERE conversation_id = ?1",
            [conversation_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, String>(3)?,
                ))
            },
        )
        .ok()
    });
    let Some((title, preview, parent, workspaces)) = summary else {
        // No summary row yet: the parent relation is unknown. Skip — the next
        // reconcile picks the conversation up once the row has landed.
        eprintln!(
            "[discover] skip {}: no conversation_summaries row yet (relation unknown)",
            db_path.display()
        );
        return Ok(None);
    };

    // `workspace_uris` is a JSON array of `file:///…` URIs; the metadata
    // blob's workspace message says the same thing.
    let workspace_uri: Option<String> = serde_json::from_str::<serde_json::Value>(&workspaces)
        .ok()
        .and_then(|v| v.get(0).cloned())
        .and_then(|u| u.as_str().map(String::from))
        .or_else(|| {
            metadata
                .as_deref()
                .and_then(|m| pb_sub(m, 1))
                .and_then(|w| pb_str(w, 1))
        });
    let cwd = workspace_uri
        .map(|uri| uri.strip_prefix("file://").unwrap_or(&uri).to_string())
        .filter(|p| !p.is_empty());

    let parent = parent.filter(|p| !p.is_empty());
    let native_title = [title, preview]
        .into_iter()
        .map(|t| t.trim().to_string())
        .find(|t| !t.is_empty());

    let meta = std::fs::metadata(db_path)?;
    let last_activity = meta
        .modified()
        .ok()
        .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());

    Ok(Some(DiscoveredMember {
        agent: Agent::Antigravity,
        source_member_id: conversation_id.to_string(),
        kind: if parent.is_some() {
            DiscoveredMemberKind::Child
        } else {
            DiscoveredMemberKind::Root
        },
        parent_source_member_id: parent,
        root_hint: None,
        source_kind: "antigravity_conversation".into(),
        source_path: db_path.to_path_buf(),
        cwd,
        started_at,
        last_activity_at: last_activity,
        native_title,
        first_user_text: None,
        first_agent_text: None,
        metadata: serde_json::json!({}),
    }))
}

/// One step row's contribution: conversation text (root only), or execution
/// observations. `idx` is the step's stable native id.
fn parse_step(idx: i64, step_type: i64, payload: &[u8], is_root: bool) -> Option<ParsedLine> {
    match step_type {
        14 if is_root => {
            let text = user_text(payload)?;
            Some(ParsedLine::message_only(parsed_message(
                idx,
                SessionMessageRole::User,
                text,
                payload,
            )))
        }
        15 if is_root => {
            let prose = agent_turn(payload);
            let text = prose.join("\n\n").trim().to_string();
            if text.is_empty() {
                return None;
            }
            Some(ParsedLine::message_only(parsed_message(
                idx,
                SessionMessageRole::Assistant,
                text,
                payload,
            )))
        }
        132 => Some(ParsedLine::observation_only(MemberObservation {
            tool_calls: 1,
            ..Default::default()
        })),
        17 => Some(ParsedLine::observation_only(MemberObservation {
            tool_errors: 1,
            ..Default::default()
        })),
        _ => None,
    }
}

fn parsed_message(
    idx: i64,
    role: SessionMessageRole,
    text: String,
    payload: &[u8],
) -> crate::domain::ParsedSessionMessage {
    crate::adapters::parsed_message(Some(format!("step:{idx}")), role, text)
        .with_position(format!("step:{idx}"))
        .with_ts(step_timestamp(payload))
}

impl crate::adapters::AgentAdapter for AntigravityAdapter {
    fn agent(&self) -> Agent {
        Agent::Antigravity
    }

    fn detect(&self) -> Option<crate::platform::exec_resolver::AgentInstallation> {
        None
    }

    fn discover_members_in(
        &self,
        roots: &[PathBuf],
        _unchanged: &dyn Fn(&Path) -> bool,
    ) -> Result<Vec<DiscoveredMember>> {
        let mut out = Vec::new();
        for root in roots {
            let Some(dir) = conversations_dir(root) else {
                continue;
            };
            // The summaries store sits beside the conversations directory. A
            // missing store means every parent relation is unknown, so all
            // members wait for it (never discovered as speculative roots).
            let summaries = dir
                .parent()
                .map(|p| p.join("conversation_summaries.db"))
                .filter(|p| p.is_file())
                .and_then(|p| {
                    Connection::open_with_flags(p, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
                });
            if summaries.is_none() {
                return Ok(out);
            }
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut paths: Vec<PathBuf> = rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("db"))
                .collect();
            paths.sort();
            for path in paths {
                // `unchanged` is deliberately NOT consulted. It stats the main
                // `.db` file, but this is a live WAL store: new steps can sit
                // only in `<id>.db-wal` while size and mtime stay identical,
                // and the conversation's title/parent live in a SECOND store
                // (`conversation_summaries.db`) the skipset never sees.
                match parse_member(&path, summaries.as_ref()) {
                    Ok(Some(m)) => out.push(m),
                    Ok(None) => {}
                    Err(e) => eprintln!("[discover] skip {}: {}", path.display(), e),
                }
            }
        }
        Ok(out)
    }

    fn read_member_delta(
        &self,
        member: &SessionMember,
        cursor: &SessionMemberCursor,
    ) -> Result<MemberReadDelta> {
        let path = PathBuf::from(&member.source_path);
        let conn = open_read_only(&path)?;
        let is_root = member.relation.as_str() == "root";

        let mut stmt =
            conn.prepare("SELECT idx, step_type, step_payload FROM steps ORDER BY idx")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Vec<u8>>(2)?,
            ))
        })?;

        let mut messages = Vec::new();
        let mut observation = MemberObservation::default();
        for row in rows {
            let (idx, step_type, payload) = row?;
            if let Some(parsed) = parse_step(idx, step_type, &payload, is_root) {
                observation.add(&parsed.observation);
                if let Some(m) = parsed.message {
                    if m.content.trim().is_empty() {
                        continue;
                    }
                    messages.push(m);
                }
            }
        }
        drop(stmt);
        drop(conn);

        let raw = std::fs::read(&path).unwrap_or_default();
        let source = replay_cursor_update(&path, cursor, &raw)?;
        Ok(MemberReadDelta {
            stats: crate::adapters::stats_update_from(
                &observation,
                &source,
                crate::adapters::StatsCapabilities::TOOL_CALLS_AND_ERRORS,
            ),
            messages,
            source: Some(source),
            next_active_provider: None,
            next_active_model: None,
        })
    }

    fn inspect_member_source(&self, member: &SessionMember) -> Result<SourceAvailability> {
        Ok(crate::adapters::inspect_file_source(Path::new(
            &member.source_path,
        )))
    }

    fn build_new_command(
        &self,
        _install: &crate::platform::exec_resolver::AgentInstallation,
        _opts: &ExecOptions,
        _context_file: Option<&Path>,
        _cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        Err(other(
            "Antigravity 没有可启动的 CLI，NoEnding 只读取它的历史会话",
        ))
    }

    fn build_resume_command(
        &self,
        _install: &crate::platform::exec_resolver::AgentInstallation,
        _opts: &ExecOptions,
        _agent_session_id: &str,
        _context_file: Option<&Path>,
        _cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        Err(other("Antigravity 没有可恢复的 CLI，无法恢复会话"))
    }

    fn build_exec_command(
        &self,
        _install: &crate::platform::exec_resolver::AgentInstallation,
        _opts: &ExecOptions,
        _prompt: &str,
    ) -> Result<AgentCommand> {
        Err(other("Antigravity 没有可执行的 CLI，无法一次性执行"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::AgentAdapter;
    use crate::domain::SessionMemberRelation;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "noending-antigravity-{}-{}",
            tag,
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // Minimal protobuf wire-format encoder for the fixtures.
    fn varint(mut v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                break;
            }
            out.push(b | 0x80);
        }
        out
    }
    fn tag(f: u32, wt: u32) -> Vec<u8> {
        varint(((f << 3) | wt) as u64)
    }
    fn int(f: u32, v: u64) -> Vec<u8> {
        [tag(f, 0), varint(v)].concat()
    }
    fn msg(f: u32, inner: &[u8]) -> Vec<u8> {
        [tag(f, 2), varint(inner.len() as u64), inner.to_vec()].concat()
    }
    fn str(f: u32, s: &str) -> Vec<u8> {
        msg(f, s.as_bytes())
    }

    fn timestamp(secs: u64) -> Vec<u8> {
        msg(5, &msg(1, &[int(1, secs), int(2, 500_000_000)].concat()))
    }

    /// The store layout: `<root>/conversations/<id>.db` plus the sibling
    /// `conversation_summaries.db`.
    fn store(root: &Path, id: &str, steps: &[(i64, Vec<u8>)]) -> PathBuf {
        let dir = root.join("conversations");
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join(format!("{id}.db"));
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE trajectory_metadata_blob (id text, data blob, PRIMARY KEY (id));
             CREATE TABLE steps (idx integer, step_type integer NOT NULL, status integer NOT NULL DEFAULT 0, metadata blob, step_payload blob, PRIMARY KEY (idx));",
        )
        .unwrap();
        let metadata = [
            msg(1, &str(1, "file:///repo")),
            msg(2, &[int(1, 1_789_480_089), int(2, 0)].concat()),
        ]
        .concat();
        conn.execute(
            "INSERT INTO trajectory_metadata_blob (id, data) VALUES ('main', ?1)",
            rusqlite::params![metadata],
        )
        .unwrap();
        for (idx, (step_type, payload)) in steps.iter().enumerate() {
            conn.execute(
                "INSERT INTO steps (idx, step_type, status, step_payload) VALUES (?1, ?2, 3, ?3)",
                rusqlite::params![idx as i64, step_type, payload],
            )
            .unwrap();
        }
        db
    }

    fn summaries(root: &Path, rows: &[(&str, &str, &str, &str)]) {
        let conn = Connection::open(root.join("conversation_summaries.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE conversation_summaries (conversation_id text, title text NOT NULL DEFAULT '', preview text NOT NULL DEFAULT '', parent_conversation_id text, workspace_uris text NOT NULL DEFAULT '', project_id text NOT NULL DEFAULT '');",
        )
        .unwrap();
        for (id, title, preview, workspaces) in rows {
            conn.execute(
                "INSERT INTO conversation_summaries (conversation_id, title, preview, workspace_uris) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![id, title, preview, workspaces],
            )
            .unwrap();
        }
    }

    fn root_member(db: &Path) -> SessionMember {
        SessionMember {
            id: "mem-ag".into(),
            session_id: "sess-ag".into(),
            agent: Agent::Antigravity,
            source_member_id: db.file_stem().unwrap().to_str().unwrap().into(),
            relation: SessionMemberRelation::Root,
            parent_source_member_id: None,
            source_kind: "antigravity_conversation".into(),
            source_path: db.to_string_lossy().to_string(),
            cwd: None,
            started_at: None,
            last_activity_at: None,
            metadata: serde_json::json!({}),
        }
    }

    /// The two conversation shapes: the user turn (14) carries the typed
    /// prompt at `19.2`; the agent turn (15) carries visible prose at `20.3`
    /// and tool calls at `20.7`. Tool-call steps (132) and API errors (17)
    /// are observations only.
    #[test]
    fn ingests_the_conversation_and_counts_the_machine_traffic() {
        let root = temp_dir("parse");
        let user_turn = [timestamp(1_789_480_258), msg(19, &str(2, "把日报整理一下"))].concat();
        let agent_turn = [
            timestamp(1_789_480_261),
            msg(20, &str(3, "日报已整理好。")),
            msg(20, &msg(7, &str(2, "list_dir"))),
        ]
        .concat();
        let tool_call = vec![];
        let api_error = str(
            2,
            "RESOURCE_EXHAUSTED (code 429): Individual quota reached.",
        );
        store(
            &root,
            "336f551c-58e8-491b-a31f-13b362786c85",
            &[
                (14, user_turn),
                (15, agent_turn),
                (132, tool_call),
                (17, api_error),
            ],
        );
        summaries(
            &root,
            &[(
                "336f551c-58e8-491b-a31f-13b362786c85",
                "",
                "把日报整理一下",
                "[\"file:///repo\"]",
            )],
        );

        let db = root
            .join("conversations")
            .join("336f551c-58e8-491b-a31f-13b362786c85.db");
        let delta = AntigravityAdapter
            .read_member_delta(&root_member(&db), &SessionMemberCursor::default())
            .unwrap();
        let roles: Vec<(SessionMessageRole, &str)> = delta
            .messages
            .iter()
            .map(|m| (m.role, m.content.as_str()))
            .collect();
        assert_eq!(
            roles,
            vec![
                (SessionMessageRole::User, "把日报整理一下"),
                (SessionMessageRole::Assistant, "日报已整理好。"),
            ]
        );
        assert_eq!(
            delta.messages[0].source_message_id.as_deref(),
            Some("step:0"),
            "the step idx is the native id"
        );
        assert!(delta.messages[0].ts.is_some());
        match delta.stats {
            Some(crate::domain::StatsUpdate::Snapshot(s)) => {
                // 20.7 only announces the call; 132 is the execution record.
                assert_eq!(s.tool_call_count, Some(1));
                assert_eq!(s.tool_error_count, Some(1), "the 429 step");
                assert_eq!(s.compaction_count, None, "no compaction source known");
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    /// Discovery: only conversation DBs are claimed; title falls back to the
    /// first user input; a parent makes the conversation a child member.
    #[test]
    fn discovery_claims_conversations_and_maps_parents_to_children() {
        let root = temp_dir("discover");
        store(&root, "11111111-2222-4333-8444-555555555555", &[]);
        store(&root, "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee", &[]);
        summaries(
            &root,
            &[
                (
                    "11111111-2222-4333-8444-555555555555",
                    "工程项目介绍",
                    "工程项目介绍",
                    "[\"file:///repo\"]",
                ),
                ("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee", "", "", "[]"),
            ],
        );
        // Set the parent on the child row.
        let conn = Connection::open(root.join("conversation_summaries.db")).unwrap();
        conn.execute(
            "UPDATE conversation_summaries SET parent_conversation_id = '11111111-2222-4333-8444-555555555555'
             WHERE conversation_id = 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee'",
            [],
        )
        .unwrap();

        let mut found = AntigravityAdapter
            .discover_members_in(&[root], &|_| false)
            .unwrap();
        found.sort_by(|a, b| a.source_member_id.cmp(&b.source_member_id));
        assert_eq!(found.len(), 2, "one member per conversation db");
        let by_id = |id: &str| {
            found
                .iter()
                .find(|m| m.source_member_id == id)
                .unwrap_or_else(|| panic!("missing {id}"))
        };
        let child = by_id("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee");
        assert_eq!(child.kind, DiscoveredMemberKind::Child);
        assert_eq!(
            child.parent_source_member_id.as_deref(),
            Some("11111111-2222-4333-8444-555555555555")
        );
        let root_m = by_id("11111111-2222-4333-8444-555555555555");
        assert_eq!(root_m.kind, DiscoveredMemberKind::Root);
        assert_eq!(root_m.native_title.as_deref(), Some("工程项目介绍"));
        assert_eq!(root_m.cwd.as_deref(), Some("/repo"));
        assert!(root_m.started_at.is_some());
    }

    /// A member that is not a conversation DB shape is refused at discovery.
    #[test]
    fn non_conversation_db_files_are_not_claimed() {
        let root = temp_dir("shape");
        let dir = root.join("conversations");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("not-a-conversation.db"), "").unwrap();
        assert!(
            AntigravityAdapter
                .discover_members_in(&[root], &|_| false)
                .unwrap()
                .is_empty(),
            "the id must look like a conversation id"
        );
    }

    /// WAL regression: discovery must not consult the skipset. Even when the
    /// caller reports every conversation "unchanged" (main-db stats frozen),
    /// members are still re-listed.
    #[test]
    fn discovery_ignores_the_unchanged_skipset() {
        let root = temp_dir("wal");
        store(&root, "11111111-2222-4333-8444-555555555555", &[]);
        summaries(
            &root,
            &[(
                "11111111-2222-4333-8444-555555555555",
                "judul",
                "judul",
                "[]",
            )],
        );
        let found = AntigravityAdapter
            .discover_members_in(&[root], &|_| true)
            .unwrap();
        assert_eq!(found.len(), 1, "WAL stores bypass the unchanged skipset");
    }

    /// Relation-unknown regression: a conversation DB present while its
    /// summary row has not landed must not be discovered as a Root; once the
    /// row lands with a parent, the next pass attaches it as a Child.
    #[test]
    fn missing_summary_row_is_not_promoted_to_root() {
        let root = temp_dir("late-summary");
        let id = "336f551c-58e8-491b-a31f-13b362786c85";
        store(&root, id, &[]);

        let none = AntigravityAdapter
            .discover_members_in(&[root.clone()], &|_| false)
            .unwrap();
        assert_eq!(
            none.len(),
            0,
            "no summary row → relation unknown → skipped, not a speculative Root"
        );

        summaries(&root, &[(id, "", "", "[]")]);
        let mut found = AntigravityAdapter
            .discover_members_in(&[root.clone()], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 1, "summary row landed → discovered now");
        assert_eq!(found[0].kind, DiscoveredMemberKind::Root);

        let conn = Connection::open(root.join("conversation_summaries.db")).unwrap();
        conn.execute(
            "UPDATE conversation_summaries SET parent_conversation_id = ?2 WHERE conversation_id = ?1",
            rusqlite::params![id, "22222222-3333-4444-8555-666666666666"],
        )
        .unwrap();
        found = AntigravityAdapter
            .discover_members_in(&[root], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].kind,
            DiscoveredMemberKind::Child,
            "the parent is authoritative once the row exists"
        );
    }
}
