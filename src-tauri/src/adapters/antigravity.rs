//! Antigravity Adapter (Google's agentic IDE). The conversation store is one
//! live WAL SQLite database per conversation,
//! `~/.gemini/antigravity/conversations/<conversation-id>.db`; raw data is
//! read-only, always. IDE conversations are history-ingestion only.
//!
//! The official `agy` CLI launches too (New / Resume), but its conversations
//! live in a SEPARATE store, `~/.gemini/antigravity-cli/conversations/`, with
//! the same schema and its own sibling `conversation_summaries.db`. The CLI
//! cannot see IDE conversations (`agy --conversation <id>` on an IDE id fails
//! with "trajectory not found"), so a resume only works for sessions that were
//! discovered from the CLI store. Both stores are default ingest sources
//! (`agent_ingest_roots`), so covering agy-created sessions end to end —
//! LaunchIntent discovery included — is one enable in Settings.
//!
//! The `steps` table is the event log, ordered by `idx`; payloads are
//! protobuf with no published schema. Field paths verified against the real
//! corpus:
//! - 14 user turn: prompt at `payload.19.2` (fallback `19.3.1`), timestamp
//!   `5.1`;
//! - 15 agent turn: the turn's answer at `payload.20.1` per part, falling back
//!   to the model's narration at `20.3` when a part has no answer (the two are
//!   disjoint — see `agent_turn`); a `20.7` part only ANNOUNCES a tool call —
//!   not counted (132 is the execution record, same call id, 827/828 ids in
//!   both; counting both would double-count);
//! - 132 tool call → tool_call count; 23/101 metadata and inter-agent notices
//!   → ignored.
//!
//! Steps are appended once and never rewritten, so message identity is
//! `step:<idx>` and the full replay dedups exactly.
//!
//! Token usage is NOT in `steps`: it is one `gen_metadata` row per generation
//! call, whose blob's `1.4` subtree holds the counts (see `usage_of`). The rows
//! are summed onto the same replay snapshot.
//!
//! The sibling `conversation_summaries.db` holds the title (empty → the
//! `preview`, the first user input), `parent_conversation_id` and
//! `workspace_uris`. One conversation = one member; a parent makes it a CHILD
//! that contributes observations only. A conversation whose summary row has
//! not landed yet is SKIPPED — unknown is never promoted to Root, or a root
//! later re-homed as a child would leave a ghost Logical Session with no root
//! member.
//!
//! `unchanged` is deliberately not consulted: WAL writes keep the main `.db`
//! size/mtime identical, and the title/parent facts live in the second store
//! the skipset never sees. Discovery re-reads every conversation; the replay
//! dedup absorbs it. The read cursor (`sqlite_replay_cursor_update`) is in the
//! conversation's OWN coordinates — MAX(step idx) as position, the newest
//! step's time as activity — because the container's stats are noise: any
//! connection touching the conversation creates/touches a 0-byte `-wal`, and
//! keyed on that mtime, an open would read as activity. A WAL-only step append
//! still moves both facts, so `last_activity_at` keeps advancing.
//!
//! Provenance: each `gen_metadata` row names the model it served (`1.19`,
//! e.g. `gemini-3.8-flash` / `claude-opus-4-6-thinking`) and the step-input
//! boundary it ran against (`1.20`'s `last_step_index`), so assistant steps
//! carry the serving model (see `read_member_delta`). No provider field
//! exists in the source — provider stays NULL.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::adapters::{
    ms_epoch_to_rfc3339, AgentCommand, DesktopResume, DiscoveredMember, DiscoveredMemberKind,
    ExecOptions, MemberObservation, MemberReadDelta, ParsedLine, ResumeRoute, SessionMessageRole,
};
use crate::domain::{Agent, SessionMember, SessionMemberCursor, SourceAvailability};
use crate::error::{other, Result};

pub struct AntigravityAdapter;

/// The conversations directory under the data root. A root that IS the
/// conversations directory itself, or a conversation `.db` file inside it,
/// is accepted too — so a user may register any of the three spellings.
fn conversations_dir(root: &Path) -> Option<PathBuf> {
    if root.is_file() && root.extension().and_then(|e| e.to_str()) == Some("db") {
        return root.parent().map(|p| p.to_path_buf());
    }
    if root.is_dir() && root.file_name().and_then(|n| n.to_str()) == Some("conversations") {
        return Some(root.to_path_buf());
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

/// The model a generation served (`1.19`) and the step-input boundary it ran
/// against (`1.20`'s repeated metadata map entry `last_step_index` — repeated,
/// so a single-field lookup cannot reach it). A row without either attributes
/// nothing.
fn generation_model_and_boundary(data: &[u8]) -> Option<(i64, String)> {
    let chat = pb_sub(data, 1)?;
    let model = pb_str(chat, 19).filter(|m| !m.trim().is_empty())?;
    let boundary = pb_fields(chat)
        .into_iter()
        .filter_map(|(f, v)| match v {
            PbVal::Bytes(b) if f == 20 => Some(b),
            _ => None,
        })
        .find_map(|entry| {
            if pb_str(entry, 1).as_deref() == Some("last_step_index") {
                pb_str(entry, 2).and_then(|v| v.parse::<i64>().ok())
            } else {
                None
            }
        })?;
    Some((boundary, model))
}

/// `{seconds, nanos}` protobuf timestamp → RFC3339. Step envelopes keep it at
/// `payload.5.1`.
fn step_timestamp(payload: &[u8]) -> Option<String> {
    ms_epoch_to_rfc3339((step_epoch_secs(payload)? * 1000.0) as i64)
}

/// Step time in UNIX seconds from the payload's `5.1` (secs) + `5.2` (nanos).
fn step_epoch_secs(payload: &[u8]) -> Option<f64> {
    let envelope = pb_sub(payload, 5)?;
    let ts = pb_sub(envelope, 1)?;
    let secs = pb_varint(ts, 1)? as f64;
    Some(secs + pb_varint(ts, 2).unwrap_or(0) as f64 / 1e9)
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
///
/// Each `20` part carries two disjoint prose slots: `20.1` is what the turn
/// ANSWERS with, `20.3` is the model's running narration. Reading only `20.3`
/// kept the narration and dropped every final answer. Verified over one real
/// conversation (3273 steps, 1630 of them step 15): `20.1` non-empty in 20
/// records, `20.3` non-empty in 137, both in none — so the two never describe
/// the same turn and `20.1` is preferred per part. `20.8` mirrors `20.1` byte
/// for byte and must not be read too.
fn agent_turn(payload: &[u8]) -> Vec<String> {
    let mut prose = Vec::new();
    for (fnum, val) in pb_fields(payload) {
        if fnum != 20 {
            continue;
        }
        let PbVal::Bytes(part) = val else { continue };
        if let Some(text) = pb_text(part, 1).or_else(|| pb_text(part, 3)) {
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
    source_kind: &str,
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
        source_kind: source_kind.to_string(),
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

/// Token usage of ONE generation call, from the `gen_metadata` blob's `1.4`
/// subtree. Cross-validated field-by-field against a real store and tokscale's
/// independent reverse-engineering (2026-09-30): `.1` is the fixed
/// system-prompt token count (1026–1319 across observed installs) and IS
/// billable input; `.2` is newly-processed (non-cached) input — a cache hit
/// moves `.2` down while `.5` (cache read) moves up; `.9` is output (text)
/// tokens; `.10` is thinking. The earlier mapping read output from `.3` and
/// thinking from `.9`: on real rows `.3` mirrors `.10` exactly while `.9` is
/// absent, and once `.9`-bearing rows are included `.3` matches no plausible
/// quantity — it stays unread. (`1.9.10.1` is the PROMPT-SIZE ESTIMATE the
/// client shows, not billed input.) One `gen_metadata` row is one call, so
/// the rows add up.
fn usage_of(data: &[u8]) -> MemberObservation {
    let Some(usage) = pb_sub(data, 1).and_then(|m| pb_sub(m, 4)) else {
        return MemberObservation::default();
    };
    let n = |f: u32| pb_varint(usage, f).unwrap_or(0);
    MemberObservation {
        input_tokens: n(1).saturating_add(n(2)),
        output_tokens: n(9),
        cached_tokens: n(5),
        reasoning_tokens: n(10),
        ..Default::default()
    }
}

/// The responseId (`1.4.11`) identifying one generation call. Unique across
/// every observed store (3/3, 26/26, 1575/1575 distinct), so the dedup at the
/// call site is purely defensive against a future duplicate-writing version.
fn gen_response_id(data: &[u8]) -> Option<String> {
    pb_sub(data, 1)
        .and_then(|m| pb_sub(m, 4))
        .and_then(|u| pb_str(u, 11))
}

/// One step row's contribution: conversation text (root only), or execution
/// observations. `idx` is the step's stable native id.
fn parse_step(
    idx: i64,
    step_type: i64,
    payload: &[u8],
    is_root: bool,
    model: Option<&str>,
) -> Option<ParsedLine> {
    match step_type {
        14 => {
            let text = user_text(payload)?;
            let observation = MemberObservation {
                user_messages: 1,
                ..Default::default()
            };
            if !is_root {
                return Some(ParsedLine::observation_only(observation));
            }
            Some(ParsedLine {
                message: Some(parsed_message(idx, SessionMessageRole::User, text, payload)),
                observation,
            })
        }
        15 => {
            let prose = agent_turn(payload);
            let text = prose.join("\n\n").trim().to_string();
            if text.is_empty() {
                return None;
            }
            let observation = MemberObservation {
                assistant_messages: 1,
                ..Default::default()
            };
            if !is_root {
                return Some(ParsedLine::observation_only(observation));
            }
            Some(ParsedLine {
                message: Some(
                    parsed_message(idx, SessionMessageRole::Assistant, text, payload)
                        .with_provenance(None, model.map(|m| m.to_string())),
                ),
                observation,
            })
        }
        132 => Some(ParsedLine::observation_only(MemberObservation {
            tool_calls: 1,
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
        crate::platform::exec_resolver::resolve_quiet(Agent::Antigravity)
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
            // missing store means every parent relation in THIS root is
            // unknown — members wait for it (never discovered as speculative
            // roots), but other ingest roots must still be scanned.
            let summaries = dir
                .parent()
                .map(|p| p.join("conversation_summaries.db"))
                .filter(|p| p.is_file())
                .and_then(|p| {
                    Connection::open_with_flags(p, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
                });
            if summaries.is_none() {
                eprintln!(
                    "[discover] skip {}: conversation_summaries.db missing (relations unknown)",
                    dir.display()
                );
                continue;
            }
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            // The two source formats carry distinct kinds: the IDE store and
            // the `agy` CLI store are schema-identical, but Continue routes
            // them differently (terminal for the CLI store, desktop app for
            // the IDE store). Which store a root IS is decided by the root
            // itself, not re-guessed per file.
            let source_kind = if dir.to_string_lossy().contains("antigravity-cli") {
                "antigravity_cli_conversation"
            } else {
                "antigravity_ide_conversation"
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
                match parse_member(&path, summaries.as_ref(), source_kind) {
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

        // Per-generation model identity: each `gen_metadata` row names the
        // model it served (`1.19`) and the input boundary it ran against
        // (`1.20`'s `last_step_index`). A step belongs to the nearest boundary
        // strictly BELOW its idx — the boundary is the last step the
        // generation saw as input — so a turn nobody generated (the user turn
        // at the first boundary) stays unattributed. Equal boundaries keep the
        // later row: it is the later write.
        let mut gen_rows = conn
            .prepare("SELECT data FROM gen_metadata ORDER BY idx")?
            .query_map([], |r| r.get::<_, Vec<u8>>(0))?
            .filter_map(|row| generation_model_and_boundary(&row.ok()?))
            .collect::<Vec<(i64, String)>>();
        gen_rows.sort_by_key(|(boundary, _)| *boundary);
        let mut gens: Vec<(i64, String)> = Vec::new();
        for pair in gen_rows {
            if gens.last().map(|(b, _)| *b) == Some(pair.0) {
                gens.pop();
            }
            gens.push(pair);
        }
        let mut gen_cursor = 0usize;

        let mut messages = Vec::new();
        let mut observation = MemberObservation::default();
        // Per-member content facts: the newest step's own time is this
        // conversation's real activity, and MAX(idx) is its position. The
        // container's stats say nothing — a mere open of the conversation
        // creates/touches a 0-byte `-wal`, and keyed on that mtime, other
        // sessions' activity would churn this one's `last_activity_at`.
        let mut max_idx: i64 = 0;
        let mut max_step_secs: Option<f64> = None;
        for row in rows {
            let (idx, step_type, payload) = row?;
            max_idx = max_idx.max(idx);
            if let Some(secs) = step_epoch_secs(&payload) {
                max_step_secs = Some(match max_step_secs {
                    Some(m) => m.max(secs),
                    None => secs,
                });
            }
            while gen_cursor + 1 < gens.len() && gens[gen_cursor + 1].0 < idx {
                gen_cursor += 1;
            }
            let model = gens
                .get(gen_cursor)
                .filter(|(boundary, _)| *boundary < idx)
                .map(|(_, model)| model.as_str());
            if let Some(parsed) = parse_step(idx, step_type, &payload, is_root, model) {
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

        // Usage is not in `steps`: one `gen_metadata` row per generation call.
        // Every read is a full replay, so the sum becomes a snapshot.
        let mut gen_stmt = conn.prepare("SELECT data FROM gen_metadata")?;
        let gen_rows = gen_stmt.query_map([], |r| r.get::<_, Vec<u8>>(0))?;
        let mut seen_response_ids: HashSet<String> = HashSet::new();
        for row in gen_rows {
            let data = row?;
            if let Some(id) = gen_response_id(&data) {
                if !seen_response_ids.insert(id) {
                    continue;
                }
            }
            observation.add(&usage_of(&data));
        }
        drop(gen_stmt);
        drop(conn);

        let source = crate::adapters::sqlite_replay_cursor_update(
            &path,
            cursor,
            crate::adapters::SqliteMemberFacts {
                position: max_idx,
                activity_epoch_secs: max_step_secs,
            },
        )?;
        Ok(MemberReadDelta {
            stats: crate::adapters::stats_update_from(
                &observation,
                &source,
                crate::adapters::StatsCapabilities::TOOL_CALLS.with_tokens(),
            ),
            messages,
            source: Some(source),
            complete_snapshot: true,
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
        install: &crate::platform::exec_resolver::AgentInstallation,
        opts: &ExecOptions,
        cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        // Bare `agy` opens the interactive TUI in `cwd`; the conversation is
        // created on the first turn and lands in the CLI store.
        Ok(AgentCommand {
            program: install.executable_path.clone(),
            args: runtime_args(opts),
            cwd: cwd.map(|p| p.to_path_buf()),
        })
    }

    fn build_resume_command(
        &self,
        install: &crate::platform::exec_resolver::AgentInstallation,
        opts: &ExecOptions,
        agent_session_id: &str,
        cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        // `--conversation <id>` resumes by id — but only for conversations in
        // the CLI's own store; the IDE store is invisible to the CLI (see the
        // module header). The launch still starts in `cwd` when it is given.
        let mut args = vec!["--conversation".into(), agent_session_id.to_string()];
        args.extend(runtime_args(opts));
        Ok(AgentCommand {
            program: install.executable_path.clone(),
            args,
            cwd: cwd.map(|p| p.to_path_buf()),
        })
    }

    fn build_exec_command(
        &self,
        _install: &crate::platform::exec_resolver::AgentInstallation,
        _opts: &ExecOptions,
        _prompt: &str,
    ) -> Result<AgentCommand> {
        Err(other("Antigravity 尚未接入一次性执行（headless）"))
    }

    /// The open-method configuration for this Agent's TWO source formats,
    /// keyed on the member's `source_kind` (not on the Agent): a CLI-store
    /// conversation resumes through `agy --conversation` (the terminal
    /// route), but an IDE-store conversation has NO binding in the CLI's
    /// registry — resuming one opens a blank session that fails at the first
    /// message — so Continue activates the desktop app; with the app absent
    /// there is no viable route at all. `antigravity_conversation` is the
    /// pre-split kind (legacy rows): their store is re-derived from the path.
    fn desktop_app_name(&self) -> Option<&'static str> {
        Some("Antigravity")
    }

    fn continue_route(&self, member: &SessionMember) -> ResumeRoute {
        match member.source_kind.as_str() {
            "antigravity_cli_conversation" => ResumeRoute::Terminal,
            "antigravity_ide_conversation" | "antigravity_conversation" => {
                if member.source_kind == "antigravity_conversation"
                    && member.source_path.contains("antigravity-cli")
                {
                    return ResumeRoute::Terminal;
                }
                if !crate::platform::paths::app_bundle_present("Antigravity") {
                    return ResumeRoute::Refused(
                        "该会话来自 Antigravity 桌面端：agy CLI 无法读取桌面端会话（存储相互独立），且未找到 Antigravity 桌面应用，无法继续该会话"
                            .into(),
                    );
                }
                ResumeRoute::Desktop(DesktopResume {
                    uri: "antigravity://".into(),
                    note: "将打开 Antigravity 桌面应用（应用内不定位到该会话）".into(),
                })
            }
            other => ResumeRoute::Refused(format!("未识别的 Antigravity 会话源格式：{other}")),
        }
    }
}

/// `agy` flags for NoEnding's runtime-override intent. `--effort` accepts
/// low|medium|high|max and `--model` a model name; there is no provider
/// switch, and capabilities.rs keeps provider Unsupported so a value can
/// never reach this point.
fn runtime_args(opts: &ExecOptions) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(model) = &opts.model {
        args.push("--model".into());
        args.push(model.clone());
    }
    if let Some(effort) = &opts.effort {
        args.push("--effort".into());
        args.push(effort.clone());
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::AgentAdapter;
    use crate::domain::SessionMemberRelation;

    /// New runs the bare TUI; resume pins the conversation id after
    /// `--conversation` and both carry the frozen runtime intent as flags.
    #[test]
    fn launch_commands_target_the_agy_cli() {
        let install = crate::platform::exec_resolver::AgentInstallation {
            agent: Agent::Antigravity,
            executable_path: "/Users/x/.local/bin/agy".into(),
            version: None,
            source: "test".into(),
            last_verified_at: "test".into(),
        };
        let adapter = AntigravityAdapter;
        let opts = ExecOptions {
            model: Some("gemini-3-pro".into()),
            effort: Some("high".into()),
            ..Default::default()
        };

        let new_cmd = adapter
            .build_new_command(&install, &opts, Some(Path::new("/repo")))
            .unwrap();
        assert_eq!(new_cmd.program, "/Users/x/.local/bin/agy");
        assert_eq!(
            new_cmd.args,
            vec![
                "--model".to_string(),
                "gemini-3-pro".to_string(),
                "--effort".to_string(),
                "high".to_string()
            ]
        );
        assert_eq!(new_cmd.cwd.as_deref(), Some(Path::new("/repo")));

        let resume_cmd = adapter
            .build_resume_command(
                &install,
                &opts,
                "5bbd1246-5106-49e4-b399-d23faa2ede93",
                None,
            )
            .unwrap();
        assert_eq!(resume_cmd.program, "/Users/x/.local/bin/agy");
        assert_eq!(
            resume_cmd.args,
            vec![
                "--conversation".to_string(),
                "5bbd1246-5106-49e4-b399-d23faa2ede93".to_string(),
                "--model".to_string(),
                "gemini-3-pro".to_string(),
                "--effort".to_string(),
                "high".to_string(),
            ]
        );
        assert_eq!(resume_cmd.cwd, None);

        // Default overrides must produce NO flags: the Agent's own
        // configuration stays untouched and unguessed.
        let bare = adapter
            .build_new_command(&install, &ExecOptions::default(), None)
            .unwrap();
        assert!(bare.args.is_empty());
        assert_eq!(bare.cwd, None);
    }

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
             CREATE TABLE steps (idx integer, step_type integer NOT NULL, status integer NOT NULL DEFAULT 0, metadata blob, step_payload blob, PRIMARY KEY (idx));
             CREATE TABLE gen_metadata (idx integer, data blob, size integer NOT NULL DEFAULT 0, PRIMARY KEY (idx));",
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

    /// One `gen_metadata` row, shaped as the real blob nests it
    /// (`1.{4:{1,2,3,5,9,10,11}, 19:model, 20:{last_step_index}}`); the usage
    /// field values are real rows from an IDE-store conversation (idx 0: no
    /// cache hit; idx 1: cache hit). `.3` is the one quantity no reading has
    /// been established for — it is written so a future reader cannot
    /// silently "claim" it.
    #[allow(clippy::too_many_arguments)]
    fn gen_usage(
        db: &Path,
        idx: i64,
        fresh_input: u64,
        unknown_3: u64,
        cache_read: u64,
        output: u64,
        thinking: u64,
        response_id: &str,
        model: &str,
        boundary: i64,
    ) {
        let mut usage = [
            int(1, 1319),
            int(2, fresh_input),
            int(3, unknown_3),
            int(5, cache_read),
            int(9, output),
            int(10, thinking),
        ]
        .concat();
        if !response_id.is_empty() {
            usage.extend(str(11, response_id));
        }
        let mut chat = vec![msg(4, &usage), str(19, model)];
        chat.push(msg(
            20,
            &[str(1, "last_step_index"), str(2, &boundary.to_string())].concat(),
        ));
        // blob.1 = the chat node: usage subtree at `.4`, model at `.19`.
        let payload = msg(1, &chat.concat());
        let conn = Connection::open(db).unwrap();
        conn.execute(
            "INSERT INTO gen_metadata (idx, data, size) VALUES (?1, ?2, ?3)",
            rusqlite::params![idx, payload, payload.len() as i64],
        )
        .unwrap();
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
            source_kind: "antigravity_ide_conversation".into(),
            source_path: db.to_string_lossy().to_string(),
            cwd: None,
            started_at: None,
            last_activity_at: None,
            metadata: serde_json::json!({}),
        }
    }

    /// User turn 14 → `19.2`; agent turn 15 → its prose (`20.1`, else `20.3`);
    /// tool calls (132) and API errors (17) are observations only.
    #[test]
    fn ingests_the_conversation_and_counts_the_machine_traffic() {
        let root = temp_dir("parse");
        let user_turn = [timestamp(1_789_480_258), msg(19, &str(2, "把日报整理一下"))].concat();
        let agent_turn = [
            timestamp(1_789_480_261),
            msg(20, &str(3, "正在整理日报。")),
            msg(20, &str(1, "日报已整理好。")),
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
                // 同一轮里的叙述与最终回答都进：20.1 是回答，20.3 是叙述。
                (
                    SessionMessageRole::Assistant,
                    "正在整理日报。\n\n日报已整理好。"
                ),
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
                assert_eq!(s.user_message_count, Some(1));
                assert_eq!(s.assistant_message_count, Some(1));
                assert_eq!(s.compaction_count, None, "no compaction source known");
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    /// Usage is one `gen_metadata` row per generation call — outside `steps` —
    /// and the calls add up. Verified mapping: input = fixed system prompt
    /// `.1` + fresh `.2`, cache = `.5`, output = `.9`, thinking = `.10`.
    #[test]
    fn usage_sums_the_generation_calls() {
        let root = temp_dir("usage");
        let db = store(&root, "336f551c-58e8-491b-a31f-13b362786c86", &[]);
        // Boundary 999 sits above every step: no attribution, usage only.
        gen_usage(&db, 0, 17718, 138, 0, 93, 45, "", "gemini-3.8-flash", 999);
        gen_usage(&db, 1, 5962, 71, 12211, 23, 48, "", "gemini-3.8-flash", 999);

        let delta = AntigravityAdapter
            .read_member_delta(&root_member(&db), &SessionMemberCursor::default())
            .unwrap();
        match delta.stats {
            Some(crate::domain::StatsUpdate::Snapshot(s)) => {
                assert_eq!(s.input_tokens, Some(26318), "(1319+17718)+(1319+5962)");
                assert_eq!(s.output_tokens, Some(116), "1.4.9");
                assert_eq!(s.cached_tokens, Some(12211), "1.4.5");
                assert_eq!(s.reasoning_tokens, Some(93), "1.4.10");
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    /// Each generation names the model it served (`1.19`) and the input
    /// boundary it ran against (`1.20`'s `last_step_index`): an assistant
    /// step carries the model of the nearest boundary strictly below its idx,
    /// and the user turn — which no generation produced — carries none.
    #[test]
    fn assistant_steps_carry_the_serving_model_from_gen_metadata() {
        let root = temp_dir("provenance");
        let id = "336f551c-58e8-491b-a31f-13b362786c88";
        store(
            &root,
            id,
            &[
                (
                    14,
                    [timestamp(1_789_480_258), msg(19, &str(2, "问"))].concat(),
                ),
                (
                    15,
                    [timestamp(1_789_480_260), msg(20, &str(1, "答一"))].concat(),
                ),
                (132, vec![]),
                (
                    15,
                    [timestamp(1_789_480_264), msg(20, &str(1, "答二"))].concat(),
                ),
            ],
        );
        let db = root.join("conversations").join(format!("{id}.db"));
        gen_usage(&db, 0, 500, 9, 0, 30, 12, "", "gemini-3.8-flash", 0);
        gen_usage(&db, 1, 700, 9, 400, 25, 14, "", "claude-opus-4-6", 2);

        let delta = AntigravityAdapter
            .read_member_delta(&root_member(&db), &SessionMemberCursor::default())
            .unwrap();
        let got: Vec<(SessionMessageRole, Option<&str>)> = delta
            .messages
            .iter()
            .map(|m| (m.role, m.model.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                (SessionMessageRole::User, None),
                (SessionMessageRole::Assistant, Some("gemini-3.8-flash")),
                (SessionMessageRole::Assistant, Some("claude-opus-4-6")),
            ]
        );
    }

    /// A repeated responseId (a future version re-writing a row) is counted
    /// once; a row without a responseId is never deduped away.
    #[test]
    fn a_repeated_response_id_is_counted_once() {
        let root = temp_dir("usage-dedup");
        let db = store(&root, "336f551c-58e8-491b-a31f-13b362786c87", &[]);
        gen_usage(
            &db,
            0,
            17718,
            138,
            0,
            93,
            45,
            "req-1",
            "gemini-3.8-flash",
            999,
        );
        gen_usage(
            &db,
            1,
            5962,
            71,
            12211,
            23,
            48,
            "req-1",
            "gemini-3.8-flash",
            999,
        );
        gen_usage(&db, 2, 100, 5, 0, 10, 4, "", "gemini-3.8-flash", 999);

        let delta = AntigravityAdapter
            .read_member_delta(&root_member(&db), &SessionMemberCursor::default())
            .unwrap();
        match delta.stats {
            Some(crate::domain::StatsUpdate::Snapshot(s)) => {
                assert_eq!(s.input_tokens, Some(20456), "(1319+17718)+(1319+100)");
                assert_eq!(s.output_tokens, Some(103));
                assert_eq!(s.reasoning_tokens, Some(49));
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    /// The corpus shape of a final answer: a step-15 record whose only prose is
    /// `20.1`. Reading `20.3` alone dropped exactly these — the turn answered,
    /// the transcript showed nothing.
    #[test]
    fn an_answer_only_turn_is_ingested_instead_of_dropped() {
        let root = temp_dir("answer-only");
        let id = "336f551c-58e8-491b-a31f-13b362786c85";
        store(
            &root,
            id,
            &[
                (
                    14,
                    [timestamp(1_789_480_258), msg(19, &str(2, "把日报整理一下"))].concat(),
                ),
                (
                    15,
                    [timestamp(1_789_480_261), msg(20, &str(1, "日报已整理好。"))].concat(),
                ),
            ],
        );
        summaries(&root, &[(id, "", "把日报整理一下", "[\"file:///repo\"]")]);

        let db = root.join("conversations").join(format!("{id}.db"));
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
        let rowless = "336f551c-58e8-491b-a31f-13b362786c85";
        let top = "11111111-2222-4333-8444-555555555555";
        let sub = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
        store(&root, rowless, &[]);
        store(&root, top, &[]);
        store(&root, sub, &[]);
        summaries(&root, &[(top, "", "", "[]"), (sub, "", "", "[]")]);
        let conn = Connection::open(root.join("conversation_summaries.db")).unwrap();
        conn.execute(
            "UPDATE conversation_summaries SET parent_conversation_id = ?2 WHERE conversation_id = ?1",
            rusqlite::params![sub, top],
        )
        .unwrap();

        let found = AntigravityAdapter
            .discover_members_in(&[root], &|_| false)
            .unwrap();
        let by_id = |id: &str| {
            found
                .iter()
                .find(|m| m.source_member_id == id)
                .unwrap_or_else(|| panic!("missing {id}"))
        };
        // No summary row → relation unknown → skipped this round; the next
        // reconcile picks the conversation up once the row lands.
        assert!(
            found.iter().all(|m| m.source_member_id != rowless),
            "a row-less conversation must not be discovered as a speculative Root"
        );
        assert_eq!(by_id(top).kind, DiscoveredMemberKind::Root);
        assert_eq!(by_id(sub).kind, DiscoveredMemberKind::Child);
    }

    /// One unusable ingest root (no summaries store) must not stop discovery
    /// of other roots of the same agent.
    #[test]
    fn one_unusable_source_does_not_block_the_others() {
        let broken = temp_dir("multi-broken");
        std::fs::create_dir_all(broken.join("conversations")).unwrap();
        let good = temp_dir("multi-good");
        store(&good, "11111111-2222-4333-8444-555555555555", &[]);
        summaries(
            &good,
            &[(
                "11111111-2222-4333-8444-555555555555",
                "judul",
                "judul",
                "[]",
            )],
        );

        let found = AntigravityAdapter
            .discover_members_in(&[broken, good], &|_| false)
            .unwrap();
        assert_eq!(
            found.len(),
            1,
            "the healthy source is still scanned despite the broken one"
        );
        assert_eq!(
            found[0].source_member_id,
            "11111111-2222-4333-8444-555555555555"
        );
    }

    /// The data root may also be registered as the conversations directory
    /// itself or as a conversation `.db` file inside it.
    #[test]
    fn all_three_root_spellings_resolve_to_the_same_store() {
        let base = temp_dir("spellings");
        let dir = base.join("conversations");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("11111111-2222-4333-8444-555555555555.db"), "").unwrap();

        assert_eq!(
            conversations_dir(&base).as_deref(),
            Some(dir.as_path()),
            "the default data root"
        );
        assert_eq!(
            conversations_dir(&dir).as_deref(),
            Some(dir.as_path()),
            "the conversations directory itself"
        );
        assert_eq!(
            conversations_dir(&dir.join("11111111-2222-4333-8444-555555555555.db")).as_deref(),
            Some(dir.as_path()),
            "a conversation db file"
        );
    }

    /// The cursor must be WAL-aware: a child member that keeps executing tool
    /// calls writes only into `<id>.db-wal` while the main `.db` file stays
    /// byte-identical, and that MUST still advance the member and session
    /// `last_activity_at` through `source_changed`.
    #[test]
    fn wal_only_write_advances_activity() {
        use crate::storage::Db;

        let root = temp_dir("wal-activity");
        let root_id = "11111111-2222-4333-8444-555555555555";
        let child_id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
        let user_turn = [timestamp(1_789_480_258), msg(19, &str(2, "问"))].concat();
        store(&root, root_id, &[(14, user_turn)]);
        store(&root, child_id, &[(132, vec![])]);
        summaries(&root, &[(root_id, "", "", "[]"), (child_id, "", "", "[]")]);
        let conn = Connection::open(root.join("conversation_summaries.db")).unwrap();
        conn.execute(
            "UPDATE conversation_summaries SET parent_conversation_id = ?2 WHERE conversation_id = ?1",
            rusqlite::params![root_id, child_id],
        )
        .unwrap();
        drop(conn);

        let child_db = root.join("conversations").join(format!("{child_id}.db"));
        let root_db = root.join("conversations").join(format!("{root_id}.db"));

        let db = Db::open(&root.join("noending.db")).unwrap();
        let (session_id, _) = db
            .upsert_logical_session_unchecked(
                Agent::Antigravity,
                root_id,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();
        let root_member_id = db
            .upsert_session_member(
                &session_id,
                Agent::Antigravity,
                root_id,
                SessionMemberRelation::Root,
                None,
                "antigravity_ide_conversation",
                &root_db.to_string_lossy(),
                None,
                None,
                None,
                &serde_json::json!({}),
            )
            .unwrap();
        let child_member_id = db
            .upsert_session_member(
                &session_id,
                Agent::Antigravity,
                child_id,
                SessionMemberRelation::Child,
                Some(root_id),
                "antigravity_ide_conversation",
                &child_db.to_string_lossy(),
                None,
                None,
                None,
                &serde_json::json!({}),
            )
            .unwrap();

        let member = |member_id: &str, source_path: &Path, relation| SessionMember {
            id: member_id.to_string(),
            session_id: session_id.clone(),
            agent: Agent::Antigravity,
            source_member_id: source_path.file_stem().unwrap().to_str().unwrap().into(),
            relation,
            parent_source_member_id: None,
            source_kind: "antigravity_ide_conversation".into(),
            source_path: source_path.to_string_lossy().to_string(),
            cwd: None,
            started_at: None,
            last_activity_at: None,
            metadata: serde_json::json!({}),
        };
        let root_member = member(&root_member_id, &root_db, SessionMemberRelation::Root);
        let child_member = member(&child_member_id, &child_db, SessionMemberRelation::Child);

        // Pass 1: both members ingested through the commit path.
        for (m, member_id) in [
            (&root_member, &root_member_id),
            (&child_member, &child_member_id),
        ] {
            let delta = AntigravityAdapter
                .read_member_delta(m, &SessionMemberCursor::default())
                .unwrap();
            db.commit_member_ingest_with_provenance_state(
                &session_id,
                member_id,
                &delta.messages,
                delta.stats,
                delta.source.as_ref().unwrap(),
                true,
                None,
                None,
            )
            .unwrap();
        }
        let activity_before = db
            .get_session(&session_id)
            .unwrap()
            .unwrap()
            .last_activity_at;

        // WAL-only write: keep a writer open so the append never checkpoints,
        // and verify the main `.db` file is byte-for-byte unchanged.
        let writer = Connection::open(&child_db).unwrap();
        writer.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        let (size_before, mtime_before) = {
            let meta = std::fs::metadata(&child_db).unwrap();
            (meta.len(), meta.modified().unwrap())
        };
        writer
            .execute(
                "INSERT INTO steps (idx, step_type, status, step_payload) VALUES (1, 132, 3, ?1)",
                rusqlite::params![Vec::<u8>::new()],
            )
            .unwrap();
        let meta = std::fs::metadata(&child_db).unwrap();
        assert_eq!(
            meta.len(),
            size_before,
            "the main db file must be untouched"
        );
        assert_eq!(meta.modified().unwrap(), mtime_before);

        // Pass 2: the logical cursor must move anyway.
        let cursor = db.get_member_cursor(&child_member_id).unwrap();
        let delta = AntigravityAdapter
            .read_member_delta(&child_member, &cursor)
            .unwrap();
        let source = delta.source.unwrap();
        assert_ne!(
            source.byte_offset, cursor.byte_offset,
            "a WAL-only append must move the logical cursor"
        );
        assert_ne!(source.mtime, cursor.mtime);
        db.commit_member_ingest_with_provenance_state(
            &session_id,
            &child_member_id,
            &delta.messages,
            delta.stats,
            &source,
            true,
            None,
            None,
        )
        .unwrap();

        let activity_after = db
            .get_session(&session_id)
            .unwrap()
            .unwrap()
            .last_activity_at;
        let parse = |s: &Option<String>| {
            chrono::DateTime::parse_from_rfc3339(s.as_deref().unwrap()).unwrap()
        };
        assert!(
            parse(&activity_after) > parse(&activity_before),
            "a WAL-only tool execution must advance session activity: {activity_before:?} → {activity_after:?}"
        );
    }

    /// A `-wal` that is merely created or touched (any connection opening the
    /// conversation does this, at zero bytes) moves the container's mtime but
    /// NOT the conversation's content: the per-member cursor must stay put and
    /// `last_activity_at` must not churn.
    #[test]
    fn a_touched_wal_without_new_steps_does_not_advance_activity() {
        use crate::storage::Db;

        let root = temp_dir("wal-noise");
        let root_id = "11111111-2222-4333-8444-555555555555";
        let user_turn = [timestamp(1_789_480_258), msg(19, &str(2, "问"))].concat();
        let db_path = store(&root, root_id, &[(14, user_turn)]);

        let db = Db::open(&root.join("noending.db")).unwrap();
        let (session_id, _) = db
            .upsert_logical_session_unchecked(
                Agent::Antigravity,
                root_id,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();
        let member_id = db
            .upsert_session_member(
                &session_id,
                Agent::Antigravity,
                root_id,
                SessionMemberRelation::Root,
                None,
                "antigravity_ide_conversation",
                &db_path.to_string_lossy(),
                None,
                None,
                None,
                &serde_json::json!({}),
            )
            .unwrap();
        let member = SessionMember {
            id: member_id.clone(),
            session_id: session_id.clone(),
            agent: Agent::Antigravity,
            source_member_id: root_id.to_string(),
            relation: SessionMemberRelation::Root,
            parent_source_member_id: None,
            source_kind: "antigravity_ide_conversation".into(),
            source_path: db_path.to_string_lossy().to_string(),
            cwd: None,
            started_at: None,
            last_activity_at: None,
            metadata: serde_json::json!({}),
        };

        // Pass 1: ingested; activity is the step's own time.
        let delta = AntigravityAdapter
            .read_member_delta(&member, &SessionMemberCursor::default())
            .unwrap();
        db.commit_member_ingest_with_provenance_state(
            &session_id,
            &member_id,
            &delta.messages,
            delta.stats,
            delta.source.as_ref().unwrap(),
            true,
            None,
            None,
        )
        .unwrap();
        let activity_before = db
            .get_session(&session_id)
            .unwrap()
            .unwrap()
            .last_activity_at;

        // A 0-byte `-wal` appears (an open creates one); no step is added.
        let wal = std::path::PathBuf::from(format!("{}-wal", db_path.display()));
        std::fs::write(&wal, b"").unwrap();

        // Pass 2: the per-member cursor is unmoved by container noise, and
        // the commit leaves the session's activity alone.
        let cursor = db.get_member_cursor(&member_id).unwrap();
        let delta = AntigravityAdapter
            .read_member_delta(&member, &cursor)
            .unwrap();
        let source = delta.source.unwrap();
        assert_eq!(
            source.byte_offset, cursor.byte_offset,
            "no new steps, no movement"
        );
        assert_eq!(
            source.mtime, cursor.mtime,
            "the activity time is the step's own, not the touched wal's"
        );
        db.commit_member_ingest_with_provenance_state(
            &session_id,
            &member_id,
            &delta.messages,
            delta.stats,
            &source,
            true,
            None,
            None,
        )
        .unwrap();
        let activity_after = db
            .get_session(&session_id)
            .unwrap()
            .unwrap()
            .last_activity_at;
        assert_eq!(
            activity_after, activity_before,
            "container noise must not churn the session's activity"
        );
    }
}
