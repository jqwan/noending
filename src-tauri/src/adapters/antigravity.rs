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
//! CLI and IDE discovery included — is one enable in Settings.
//!
//! The `steps` table is the event log, ordered by `idx`; payloads are
//! protobuf with no published schema. Field paths verified against the real
//! corpus:
//! - 14 user turn: prompt at `payload.19.2` (fallback `19.3.1`), timestamp
//!   `5.1`; CLI workspace URI at `19.12.12` when the summary/metadata lack it;
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
//! The `gen_metadata` table (model served, usage) is NOT read: it carried only
//! per-generation provenance and usage, both retired.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::adapters::{
    ms_epoch_to_rfc3339, AgentCommand, DesktopResume, DiscoveredMember, DiscoveredMemberKind,
    ExecOptions, MemberReadDelta, ParsedLine, ResumeRoute, SessionMessageRole,
};
use crate::domain::{Agent, Session, SourceAvailability, SourceCursor};
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

/// Only explicit workspace facts are paths; prose and tool arguments are not.
fn workspace_path(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.starts_with("file:") {
        tauri::Url::parse(raw)
            .ok()?
            .to_file_path()
            .ok()?
            .to_str()
            .map(String::from)
    } else {
        Path::new(raw).is_absolute().then(|| raw.to_string())
    }
}

/// The CLI can leave both summary and trajectory workspaces empty while later
/// user steps carry the workspace URI at `19.12.12` (verified in the CLI store).
fn user_workspace(payload: &[u8]) -> Option<String> {
    let turn = pb_sub(payload, 19)?;
    let context = pb_sub(turn, 12)?;
    workspace_path(&pb_str(context, 12)?)
}

/// Whether a native lifecycle notification belongs to a confirmed root.
pub(crate) fn is_root_conversation(store_root: &Path, conversation_id: &str) -> bool {
    // Hooks also fire for subagents. Unknown relations must wait for a later
    // native event, just as discovery waits for the summaries row.
    open_read_only(&store_root.join("conversation_summaries.db"))
        .ok()
        .and_then(|conn| {
            conn.query_row(
                "SELECT parent_conversation_id FROM conversation_summaries WHERE conversation_id = ?1",
                [conversation_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .ok()
        })
        .is_some_and(|parent| parent.is_none_or(|id| id.trim().is_empty()))
}

/// One conversation DB's facts for discovery: start time from its metadata
/// blob, title / parent from the summaries store, and cwd from the summary,
/// metadata or CLI user-step context, in that order.
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
        eprintln!(
            "[discover] skip {}: no conversation_summaries row yet (relation unknown)",
            db_path.display()
        );
        return Ok(None);
    };

    // `workspace_uris` is a JSON array of `file:///…` URIs; the metadata
    // blob's workspace message says the same thing.
    let mut cwd = serde_json::from_str::<Vec<String>>(&workspaces)
        .ok()
        .and_then(|uris| uris.iter().find_map(|uri| workspace_path(uri)))
        .or_else(|| {
            metadata
                .as_deref()
                .and_then(|m| pb_sub(m, 1))
                .and_then(|w| pb_str(w, 1))
                .and_then(|uri| workspace_path(&uri))
        });

    let parent = parent.filter(|p| !p.trim().is_empty());
    let is_root = parent.is_none();
    let mut first_user_text = None;
    let mut first_agent_text = None;
    // There is no fixed step cutoff: metadata and tool announcements may
    // precede visible text, and the workspace may first appear on a later turn.
    let mut stmt = conn.prepare(
        "SELECT step_type, step_payload FROM steps WHERE step_type IN (14, 15) ORDER BY idx",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))?;
    for row in rows {
        let (step_type, payload) = row?;
        if step_type == 14 {
            if cwd.is_none() {
                cwd = user_workspace(&payload);
            }
            if is_root && first_user_text.is_none() {
                first_user_text =
                    user_text(&payload).map(|text| crate::adapters::truncate_text(&text, 400));
            }
        } else if is_root && first_agent_text.is_none() {
            let text = agent_turn(&payload).join("\n\n");
            if !text.trim().is_empty() {
                first_agent_text = Some(crate::adapters::truncate_text(text.trim(), 400));
            }
        }
        if cwd.is_some() && (!is_root || (first_user_text.is_some() && first_agent_text.is_some()))
        {
            break;
        }
    }
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
        first_user_text,
        first_agent_text,
        metadata: serde_json::json!({}),
    }))
}

/// One step row's contribution: conversation text on the root only.
/// `idx` is the step's stable native id.
fn parse_step(idx: i64, step_type: i64, payload: &[u8], is_root: bool) -> Option<ParsedLine> {
    match step_type {
        14 => {
            let text = user_text(payload)?;
            if !is_root {
                return None;
            }
            Some(ParsedLine::message_only(parsed_message(
                idx,
                SessionMessageRole::User,
                text,
                payload,
            )))
        }
        15 => {
            let prose = agent_turn(payload);
            let text = prose.join("\n\n").trim().to_string();
            if text.is_empty() || !is_root {
                return None;
            }
            Some(ParsedLine::message_only(parsed_message(
                idx,
                SessionMessageRole::Assistant,
                text,
                payload,
            )))
        }
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
            let mut paths: Vec<PathBuf> = if root.is_file() {
                vec![root.clone()]
            } else {
                let Ok(rd) = std::fs::read_dir(&dir) else {
                    continue;
                };
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("db"))
                    .collect()
            };
            paths.sort();
            for path in paths {
                match parse_member(&path, summaries.as_ref(), source_kind) {
                    Ok(Some(m)) => out.push(m),
                    Ok(None) => {}
                    Err(e) => eprintln!("[discover] skip {}: {}", path.display(), e),
                }
            }
        }
        Ok(out)
    }

    fn read_session_delta(
        &self,
        session: &Session,
        cursor: &SourceCursor,
    ) -> Result<MemberReadDelta> {
        let path = PathBuf::from(&session.source_path);
        let identity = crate::adapters::file_identity(&path);
        let conn = open_read_only(&path)?;
        let tx = conn.unchecked_transaction()?;
        let is_root = true; // every stored session is its root source

        let mut stmt = tx.prepare("SELECT idx, step_type, step_payload FROM steps ORDER BY idx")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Vec<u8>>(2)?,
            ))
        })?;

        let mut messages = Vec::new();
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
            if let Some(parsed) = parse_step(idx, step_type, &payload, is_root) {
                if let Some(m) = parsed.message {
                    if m.content.trim().is_empty() {
                        continue;
                    }
                    messages.push(m);
                }
            }
        }
        drop(stmt);

        let source = crate::adapters::sqlite_replay_cursor_update(
            &path,
            cursor,
            crate::adapters::SqliteMemberFacts {
                position: max_idx,
                activity_epoch_secs: max_step_secs,
            },
        )?;
        if source.file_identity != identity {
            return Err(other("Antigravity 来源在读取期间被替换，请重试"));
        }
        tx.commit()?;
        Ok(MemberReadDelta {
            messages,
            source: Some(source),
            complete_snapshot: true,
        })
    }

    fn inspect_session_source(&self, session: &Session) -> Result<SourceAvailability> {
        Ok(crate::adapters::inspect_file_source(Path::new(
            &session.source_path,
        )))
    }

    fn build_new_command(
        &self,
        install: &crate::platform::exec_resolver::AgentInstallation,
        opts: &ExecOptions,
        cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        // Bare `agy` opens the interactive TUI. With a first message, its
        // interactive-prompt flag runs that turn and keeps the TUI open;
        // `--prompt` would run print mode and exit instead.
        let mut args = runtime_args(opts);
        if let Some(message) = opts.initial_message() {
            args.extend(["--prompt-interactive".into(), message.into()]);
        }
        Ok(AgentCommand {
            program: install.executable_path.clone(),
            args,
            cwd: cwd.map(|p| p.to_path_buf()),
        })
    }

    fn build_resume_command(
        &self,
        install: &crate::platform::exec_resolver::AgentInstallation,
        opts: &ExecOptions,
        agent_session_id: &str,
        cwd: Option<&Path>,
        _source_path: Option<&str>,
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

    fn desktop_app_name(&self) -> Option<&'static str> {
        Some("Antigravity")
    }

    /// The open-method configuration for this Agent's TWO source formats,
    /// keyed on the member's `source_kind` (not on the Agent): a CLI-store
    /// conversation resumes through `agy --conversation` (the terminal
    /// route), but an IDE-store conversation has NO binding in the CLI's
    /// registry — resuming one opens a blank session that fails at the first
    /// message — so Continue activates the desktop app; with the app absent
    /// there is no viable route at all. `antigravity_conversation` is the
    /// pre-split kind (legacy rows): their store is re-derived from the path.
    fn continue_route(&self, session: &Session) -> ResumeRoute {
        match session.source_kind.as_str() {
            "antigravity_cli_conversation" => ResumeRoute::Terminal,
            "antigravity_ide_conversation" | "antigravity_conversation" => {
                if session.source_kind == "antigravity_conversation"
                    && session.source_path.contains("antigravity-cli")
                {
                    return ResumeRoute::Terminal;
                }
                if !self.desktop_app_present() {
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
            uuid::Uuid::new_v4()
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

    fn repo_uri() -> &'static str {
        if cfg!(windows) {
            "file:///C:/repo"
        } else {
            "file:///repo"
        }
    }

    fn repo_workspaces() -> String {
        serde_json::json!([repo_uri()]).to_string()
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
            msg(1, &str(1, repo_uri())),
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

    fn root_session(db: &Path) -> Session {
        Session {
            id: "sess-ag".into(),
            agent: Agent::Antigravity,
            root_agent_session_id: db.file_stem().unwrap().to_str().unwrap().into(),
            title: None,
            cwd: None,
            workspace_path_id: None,
            project_id: None,
            tracked_workstream_ids: vec![],
            forked_from_session_id: None,
            started_at: None,
            last_activity_at: None,
            last_conversation_at: None,
            archived_at: None,
            source_kind: "antigravity_ide_conversation".into(),
            source_path: db.to_string_lossy().to_string(),
            metadata: serde_json::json!({}),
            source_file_identity: String::new(),
            source_generation: 0,
            source_byte_offset: 0,
            source_last_seen_size: 0,
            source_mtime: None,
            source_prefix_hash: String::new(),
            source_tail_hash: String::new(),
            fact_generation: 0,
            latest_message_seq: 0,
        }
    }

    /// User turn 14 → `19.2`; agent turn 15 → its prose (`20.1`, else `20.3`);
    /// tool calls (132) and API errors (17) are observations only.
    #[test]
    fn ingests_the_conversation_and_excludes_machine_traffic() {
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
                &repo_workspaces(),
            )],
        );

        let db = root
            .join("conversations")
            .join("336f551c-58e8-491b-a31f-13b362786c85.db");
        let delta = AntigravityAdapter
            .read_session_delta(&root_session(&db), &crate::domain::SourceCursor::default())
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
    }

    /// Discovery: only conversation DBs are claimed; title falls back to the
    /// first user input; a parent makes the conversation a child member.
    #[test]
    fn discovery_claims_conversations_and_maps_parents_to_children() {
        let root = temp_dir("discover");
        // Timestamped native steps supply visible text independently of titles.
        let steps = [
            (
                14,
                [timestamp(1_789_480_100), msg(19, &str(2, "分析代码性能"))].concat(),
            ),
            (
                15,
                [
                    timestamp(1_789_480_105),
                    msg(20, &str(1, "已完成性能分析。")),
                ]
                .concat(),
            ),
        ];
        store(&root, "11111111-2222-4333-8444-555555555555", &steps);
        store(&root, "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee", &steps);
        summaries(
            &root,
            &[
                (
                    "11111111-2222-4333-8444-555555555555",
                    "工程项目介绍",
                    "工程项目介绍",
                    &repo_workspaces(),
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
        assert_eq!(
            root_m.cwd.as_deref(),
            Some(if cfg!(windows) { "C:\\repo" } else { "/repo" })
        );
        assert!(root_m.started_at.is_some());
        assert_eq!(root_m.first_user_text.as_deref(), Some("分析代码性能"));
        assert_eq!(root_m.first_agent_text.as_deref(), Some("已完成性能分析。"));
        assert_eq!(child.first_user_text, None);
        assert_eq!(child.first_agent_text, None);
    }

    #[test]
    fn native_cli_notification_ingests_only_its_conversation_with_history_disabled() {
        let base = temp_dir("native-notice");
        let root = base.join("antigravity-cli");
        let wanted = "11111111-2222-4333-8444-555555555555";
        let neighbor = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
        let child = "12345678-1234-4234-8234-123456789012";
        let steps = [(14, msg(19, &str(2, "真实首条消息")))];
        for id in [wanted, neighbor, child] {
            store(&root, id, &steps);
        }
        let conversation =
            Connection::open(root.join("conversations").join(format!("{wanted}.db"))).unwrap();
        conversation
            .execute(
                "UPDATE trajectory_metadata_blob SET data = ?1 WHERE id = 'main'",
                rusqlite::params![msg(2, &int(1, 1_789_480_089))],
            )
            .unwrap();
        summaries(
            &root,
            &[
                (wanted, "目标会话", "", "[]"),
                (neighbor, "历史会话", "", "[]"),
                (child, "子会话", "", "[]"),
            ],
        );
        Connection::open(root.join("conversation_summaries.db")).unwrap().execute(
            "UPDATE conversation_summaries SET parent_conversation_id = ?1 WHERE conversation_id = ?2",
            rusqlite::params![wanted, child],
        ).unwrap();
        let db = crate::storage::Db::open(&base.join("noending.db")).unwrap();
        let source = db
            .add_ingest_source(Agent::Antigravity, &root.to_string_lossy(), false)
            .unwrap();
        let transcript = root
            .join("brain")
            .join(wanted)
            .join(".system_generated/logs/transcript_full.jsonl");
        let result = crate::ingestion::reconcile_native(
            &db,
            Agent::Antigravity,
            wanted,
            Some(&transcript.to_string_lossy()),
            &|_| {},
        )
        .unwrap();
        assert_eq!(result, (1, 1));
        let session = db
            .find_session_by_root_agent_id(Agent::Antigravity, wanted)
            .unwrap()
            .unwrap();
        assert_eq!(session.source_kind, "antigravity_cli_conversation");
        assert_eq!(session.cwd, None);
        // After initial binding, later native steps may supply the first cwd.
        let workspace = base.join("late workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let uri = tauri::Url::from_file_path(&workspace).unwrap().to_string();
        let payload = msg(19, &[str(2, "继续处理"), msg(12, &str(12, &uri))].concat());
        conversation
            .execute(
                "INSERT INTO steps (idx, step_type, step_payload) VALUES (1, 14, ?1)",
                rusqlite::params![payload],
            )
            .unwrap();
        Connection::open(root.join("conversation_summaries.db"))
            .unwrap()
            .execute(
                "UPDATE conversation_summaries SET title = '新标题' WHERE conversation_id = ?1",
                rusqlite::params![wanted],
            )
            .unwrap();
        assert_eq!(
            crate::ingestion::reconcile_native(&db, Agent::Antigravity, wanted, None, &|_| {})
                .unwrap(),
            (1, 1)
        );
        let updated = db.get_session(&session.id).unwrap().unwrap();
        assert_eq!(updated.cwd.as_deref(), workspace.to_str());
        assert_eq!(updated.title.as_deref(), Some("新标题"));
        assert!(db
            .find_session_by_root_agent_id(Agent::Antigravity, neighbor)
            .unwrap()
            .is_none());
        assert_eq!(
            crate::ingestion::reconcile_native(&db, Agent::Antigravity, child, None, &|_| {})
                .unwrap(),
            (0, 0)
        );
        assert!(
            db.find_session_by_root_agent_id(Agent::Antigravity, child)
                .unwrap()
                .is_none(),
            "child notifications cannot create logical sessions"
        );
        assert!(!db.get_ingest_source(&source.id).unwrap().unwrap().enabled);
        let one_file = AntigravityAdapter
            .discover_members_in(
                &[root.join("conversations").join(format!("{wanted}.db"))],
                &|_| false,
            )
            .unwrap();
        assert_eq!(one_file.len(), 1);
        assert_eq!(one_file[0].source_member_id, wanted);
        drop(conversation);
        drop(db);
        std::fs::remove_dir_all(&base).unwrap();
    }

    fn reconcile_store(db: &crate::storage::Db, root: &Path) -> i64 {
        let source = crate::domain::IngestSource {
            id: "ag-test-source".into(),
            agent: Agent::Antigravity,
            path: root.to_string_lossy().into_owned(),
            enabled: true,
            origin: crate::domain::ingest_origin::USER.into(),
            created_at: "2026-10-10T00:00:00Z".into(),
        };
        crate::ingestion::reconcile_source(db, &source, &|_| {})
            .unwrap()
            .1
    }

    /// CLI workspace facts can first appear after the first complete exchange.
    /// Reconcile must admit late messages and repair an existing NULL cwd.
    #[test]
    fn reconcile_recovers_late_cli_messages_and_workspace() {
        let base = temp_dir("late-workspace");
        let root = base.join("antigravity-cli");
        let id = "55555555-6666-4777-8888-999999999999";
        let mut steps = vec![(23, vec![]); 60];
        steps.push((14, msg(19, &str(2, "分析代码性能"))));
        steps.push((15, msg(20, &str(1, "已完成性能分析。"))));
        let path = store(&root, id, &steps);
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "UPDATE trajectory_metadata_blob SET data = ?1 WHERE id = 'main'",
            rusqlite::params![msg(2, &int(1, 1_789_480_089))],
        )
        .unwrap();
        summaries(&root, &[(id, "性能分析", "预览", "")]);
        let db = crate::storage::Db::open(&base.join("noending.db")).unwrap();
        let found = AntigravityAdapter
            .discover_members_in(std::slice::from_ref(&root), &|_| false)
            .unwrap();
        assert_eq!(found[0].first_user_text.as_deref(), Some("分析代码性能"));
        assert_eq!(
            found[0].first_agent_text.as_deref(),
            Some("已完成性能分析。")
        );
        assert_eq!(reconcile_store(&db, &root), 2);
        let before = db
            .find_session_by_root_agent_id(Agent::Antigravity, id)
            .unwrap()
            .unwrap();
        assert_eq!(before.cwd, None);
        assert_eq!(before.source_kind, "antigravity_cli_conversation");

        let workspace = base.join("工作目录 with spaces");
        std::fs::create_dir_all(&workspace).unwrap();
        let uri = tauri::Url::from_file_path(&workspace).unwrap().to_string();
        let payload = msg(19, &[str(2, "继续分析"), msg(12, &str(12, &uri))].concat());
        conn.execute(
            "INSERT INTO steps (idx, step_type, step_payload) VALUES (62, 14, ?1)",
            rusqlite::params![payload],
        )
        .unwrap();
        assert_eq!(reconcile_store(&db, &root), 1);
        let after = db.get_session(&before.id).unwrap().unwrap();
        assert_eq!(after.cwd.as_deref(), workspace.to_str());
        let messages = db.get_messages(&after.id, None, 10).unwrap();
        assert_eq!(
            messages
                .iter()
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>(),
            ["分析代码性能", "已完成性能分析。", "继续分析"]
        );
        assert_eq!(reconcile_store(&db, &root), 0);
        drop(conn);
        drop(db);
        std::fs::remove_dir_all(base).unwrap();
    }

    /// Native titles/previews and tool-only parts do not prove visible messages.
    #[test]
    fn reconcile_requires_visible_messages_even_with_a_summary() {
        let root = temp_dir("empty-summary");
        let id = "55555555-6666-4777-8888-999999999999";
        let path = store(&root, id, &[(15, msg(20, &msg(7, &str(2, "list_dir"))))]);
        summaries(&root, &[(id, "命名的空会话", "预览", &repo_workspaces())]);
        let db = crate::storage::Db::open(&root.join("noending.db")).unwrap();
        assert_eq!(reconcile_store(&db, &root), 0);
        assert!(db
            .find_session_by_root_agent_id(Agent::Antigravity, id)
            .unwrap()
            .is_none());

        // An assistant-only visible answer is sufficient; no user text is invented.
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO steps (idx, step_type, step_payload) VALUES (1, 15, ?1)",
            rusqlite::params![msg(20, &str(1, "可见回答"))],
        )
        .unwrap();
        let found = AntigravityAdapter
            .discover_members_in(std::slice::from_ref(&root), &|_| false)
            .unwrap();
        assert_eq!(found[0].first_user_text, None);
        assert_eq!(found[0].first_agent_text.as_deref(), Some("可见回答"));
        assert_eq!(reconcile_store(&db, &root), 1);
        drop(conn);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn discovery_prefers_summary_then_metadata_over_step_workspace() {
        let root = temp_dir("workspace-precedence");
        let id = "55555555-6666-4777-8888-999999999999";
        let summary_path = root.join("summary workspace");
        let step_path = root.join("step workspace");
        let summary_uri = tauri::Url::from_file_path(&summary_path)
            .unwrap()
            .to_string();
        let step_uri = tauri::Url::from_file_path(&step_path).unwrap().to_string();
        store(
            &root,
            id,
            &[(
                14,
                msg(
                    19,
                    &[str(2, "检查目录"), msg(12, &str(12, &step_uri))].concat(),
                ),
            )],
        );
        summaries(
            &root,
            &[(
                id,
                "",
                "",
                &serde_json::json!(["", summary_uri]).to_string(),
            )],
        );
        let discover = || {
            AntigravityAdapter
                .discover_members_in(std::slice::from_ref(&root), &|_| false)
                .unwrap()
                .remove(0)
        };
        assert_eq!(discover().cwd.as_deref(), summary_path.to_str());

        let conn = Connection::open(root.join("conversation_summaries.db")).unwrap();
        conn.execute(
            "UPDATE conversation_summaries SET workspace_uris = '[]'",
            [],
        )
        .unwrap();
        assert_eq!(
            discover().cwd.as_deref(),
            Some(if cfg!(windows) { "C:\\repo" } else { "/repo" })
        );
        drop(conn);
        std::fs::remove_dir_all(root).unwrap();
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
    /// caller reports every conversation "unchanged" (main-db size/mtime
    /// frozen), members are still re-listed.
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

    /// The cursor must be WAL-aware: a conversation that keeps executing tool
    /// calls writes only into `<id>.db-wal` while the main `.db` file stays
    /// byte-identical, and that MUST still advance the session's cursor and
    /// `last_activity_at` through `source_changed`.
    #[test]
    fn wal_only_write_advances_activity() {
        use crate::storage::Db;

        let root = temp_dir("wal-activity");
        let id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
        // An untimed tool step only: the conversation carries no step
        // timestamp, so the activity time is the store's own (file/WAL)
        // mtime — exactly the coordinate a WAL-only write moves.
        store(&root, id, &[(132, vec![])]);
        summaries(&root, &[(id, "", "", "[]")]);

        let db_path = root.join("conversations").join(format!("{id}.db"));

        let db = Db::open(&root.join("noending.db")).unwrap();
        let (session_id, _) = db
            .upsert_logical_session(
                Agent::Antigravity,
                id,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                "antigravity_ide_conversation",
                &db_path.to_string_lossy(),
                &serde_json::json!({}),
            )
            .unwrap();
        let session = root_session(&db_path);

        // Pass 1: ingested through the commit path.
        let delta = AntigravityAdapter
            .read_session_delta(&session, &crate::domain::SourceCursor::default())
            .unwrap();
        db.commit_ingest(&session_id, &delta.messages, delta.source.as_ref().unwrap())
            .unwrap();
        let activity_before = db
            .get_session(&session_id)
            .unwrap()
            .unwrap()
            .last_activity_at;

        // WAL-only write: keep a writer open so the append never checkpoints,
        // and verify the main `.db` file is byte-for-byte unchanged.
        let writer = Connection::open(&db_path).unwrap();
        writer.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        let (size_before, mtime_before) = {
            let meta = std::fs::metadata(&db_path).unwrap();
            (meta.len(), meta.modified().unwrap())
        };
        writer
            .execute(
                "INSERT INTO steps (idx, step_type, status, step_payload) VALUES (1, 132, 3, ?1)",
                rusqlite::params![Vec::<u8>::new()],
            )
            .unwrap();
        let meta = std::fs::metadata(&db_path).unwrap();
        assert_eq!(
            meta.len(),
            size_before,
            "the main db file must be untouched"
        );
        assert_eq!(meta.modified().unwrap(), mtime_before);

        // Pass 2: the logical cursor must move anyway.
        let cursor = db
            .get_session(&session_id)
            .unwrap()
            .unwrap()
            .source_cursor();
        let delta = AntigravityAdapter
            .read_session_delta(&session, &cursor)
            .unwrap();
        let source = delta.source.unwrap();
        assert_ne!(
            source.byte_offset, cursor.byte_offset,
            "a WAL-only append must move the logical cursor"
        );
        assert_ne!(source.mtime, cursor.mtime);
        db.commit_ingest(&session_id, &delta.messages, &source)
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
        let session = root_session(&db_path);

        // Pass 1: ingested; activity is the step's own time.
        let delta = AntigravityAdapter
            .read_session_delta(&session, &crate::domain::SourceCursor::default())
            .unwrap();
        db.commit_ingest(&session_id, &delta.messages, delta.source.as_ref().unwrap())
            .unwrap();
        let activity_before = db
            .get_session(&session_id)
            .unwrap()
            .unwrap()
            .last_activity_at;

        // A 0-byte `-wal` appears (an open creates one); no step is added.
        let wal = std::path::PathBuf::from(format!("{}-wal", db_path.display()));
        std::fs::write(&wal, b"").unwrap();

        // Pass 2: the session cursor is unmoved by container noise, and
        // the commit leaves the session's activity alone.
        let cursor = db
            .get_session(&session_id)
            .unwrap()
            .unwrap()
            .source_cursor();
        let delta = AntigravityAdapter
            .read_session_delta(&session, &cursor)
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
        db.commit_ingest(&session_id, &delta.messages, &source)
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
