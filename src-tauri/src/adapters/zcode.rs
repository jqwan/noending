//! ZCode Adapter: the session store is a **live SQLite database**,
//! `~/.zcode/cli/db/db.sqlite` (WAL, mutated in place).
//!
//! This is the one agent with no transcript file at all, so none of the file
//! machinery applies: no byte offsets, no prefix fingerprints, no append
//! detection. Three tables carry the conversation, opened read-only (NoEnding
//! never writes, migrates or VACUUMs): `session(id, parent_id, directory, title,
//! time_created, time_updated, time_archived, task_type, …)` where `directory` is
//! the cwd; `message(id, session_id, data, sequence)` with `data.role` and
//! `data.time.created` / `time.completed`; and `part(id, message_id, session_id,
//! data, sequence)` whose `data.type` is `text` / `reasoning` / `tool` /
//! `step-start` / `step-finish` / `timeline` / `compaction` / `file`.
//!
//! Usage is in none of those rows: it lives in `turn_usage`, ZCode's per-turn
//! rollup, read separately and summed. (`model_usage` is the per-request table,
//! which a retried request would repeat — the rollup cannot double-count an
//! attempt.) The store states no cost.
//!
//! Member mapping: every live session row is a member; a row with a `parent_id`
//! is a CHILD member of that parent. The source cannot express a genuine user
//! fork, so no member is ever a ForkRoot — only an explicit fork marker would
//! change that, and an adapter test pins the rule.
//!
//! Ingested is exactly the prose: the `text` parts of a message joined in
//! `sequence` order, under the message's own id, and only for the ROOT member.
//! Reasoning, tool traffic, step markers, timeline and file references are
//! counted, never stored; a `compaction` part is a compaction observation. The
//! user's prompt needs no unwrapping — the environment snapshot lives in
//! `message.data.contextSnapshot` and leaves `text` clean.
//!
//! Three decisions worth recording:
//! - **`role` is not who wrote it.** `data.semantics.{origin,kind}` is what
//!   separates a human turn from the runtime talking to the model: under
//!   `role:"user"` sit reminders and compaction summaries, which reading `role`
//!   would ingest as prose and then use to title the session. Only
//!   `real_user`/`user_prompt` is a human turn.
//! - **Only settled assistant messages.** A row is inserted when generation
//!   starts and rewritten as it streams, and storage dedups by id — so reading a
//!   half-written reply would freeze a truncated answer forever. A reply is read
//!   only once `time.completed` exists; user messages never carry it.
//! - **Archived sessions are left alone.** `time_archived` is ZCode's own
//!   "retired from the list" verdict, and inventing sessions the app itself
//!   hides is the failure the design does not tolerate.
//!
//! There is no launchable CLI (`~/.zcode/cli` is its own data dir, not a
//! user-facing command), so `detect()` never succeeds and no command is built.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::adapters::{
    ms_epoch_to_rfc3339, AgentCommand, DesktopResume, DiscoveredMember, DiscoveredMemberKind,
    ExecOptions, MemberObservation, MemberReadDelta, ResumeRoute,
};
use crate::domain::{
    Agent, ParsedSessionMessage, SessionMember, SessionMemberCursor, SessionMessageRole,
    SourceAvailability,
};
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

/// Tool-call observations one message's parts contribute.
fn observation_of(parts: &[Value]) -> MemberObservation {
    let mut obs = MemberObservation::default();
    for p in parts {
        match p.pointer("/data/type").and_then(|t| t.as_str()) {
            Some("tool") => obs.tool_calls += 1,
            _ => {}
        }
    }
    obs
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

/// The session's usage, read per turn from `turn_usage` — ZCode's own
/// per-turn rollup of `model_usage` (the per-request rows, which a retried
/// request would repeat), so it is the authoritative total and cannot
/// double-count an attempt. Returns one ledger event per turn reporting
/// tokens or requests. Token totals are queried from these events.
/// `cache_read_input_tokens` is the cached axis here, matching how every
/// other adapter maps it. The model comes from the read's own message rows
/// when they name exactly ONE (the common session); a mixed-model session
/// attributes nothing rather than guessing. The channel is used only when
/// every settled generation names the same model/channel pair.
fn usage_of(
    conn: &Connection,
    session_id: &str,
    model: Option<&str>,
    provider: Option<&str>,
) -> Result<Vec<crate::adapters::UsageEvent>> {
    let mut stmt = conn
        .prepare(
            "SELECT turn_id, started_at, input_tokens, output_tokens,
                    reasoning_tokens, cache_read_input_tokens, model_request_count
             FROM turn_usage WHERE session_id = ?1 ORDER BY started_at",
        )
        .map_err(|e| other(format!("查询 ZCode turn_usage 失败: {e}")))?;
    let rows = stmt
        .query_map([session_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
            ))
        })
        .map_err(|e| other(format!("查询 ZCode turn_usage 失败: {e}")))?;

    let mut events = Vec::new();
    for row in rows {
        let (turn_id, started_at, input, output, reasoning, cached, request_count) =
            row.map_err(|e| other(format!("读取 ZCode turn_usage 失败: {e}")))?;
        let (input_raw, output, reasoning, cached) = (
            input.max(0) as u64,
            output.max(0) as u64,
            reasoning.max(0) as u64,
            cached.max(0) as u64,
        );
        if input_raw + output + reasoning + cached == 0 && request_count <= 0 {
            continue;
        }
        // The store's `input_tokens` INCLUDES `cache_read_input_tokens` (the
        // row's `computed_total_tokens == input_tokens + output_tokens`, which
        // only adds up with the cache inside). Store the fresh part.
        let input = input_raw.saturating_sub(cached);
        events.push(crate::adapters::UsageEvent {
            key: Some(format!("turn:{turn_id}")),
            category: crate::adapters::UsageCategory::Conversation,
            model: model.map(str::to_string),
            provider: provider.map(str::to_string),
            ts: ms_epoch_to_rfc3339(started_at),
            input_tokens: input,
            output_tokens: output,
            cached_tokens: cached,
            reasoning_tokens: reasoning,

            // The store's own request tally: one turn's tokens may cover many
            // underlying requests (retries and tool loops included).
            request_count: request_count.max(0) as u64,
        });
    }
    Ok(events)
}

/// The conversation message one row contributes (root members only), plus its
/// observations. A reply is only read once generation finished: the row
/// appears when streaming starts and is rewritten in place, so an early read
/// would freeze a truncated answer under its permanent message id.
fn message_of(
    message: &Value,
    parts: &[Value],
    is_root: bool,
) -> (Option<ParsedSessionMessage>, MemberObservation) {
    let mut observation = observation_of(parts);
    let id = message.get("id").and_then(|i| i.as_str()).unwrap_or("");
    let data = message.get("data").unwrap_or(&Value::Null);
    let kind = match message_kind(data) {
        Some("assistant_response") if !is_settled(data) => None,
        other => other,
    };
    let Some(kind) = kind else {
        return (None, observation);
    };
    let text = text_of(parts);
    if text.trim().is_empty() {
        return (None, observation);
    }
    let role = if kind == "user_prompt" {
        SessionMessageRole::User
    } else {
        SessionMessageRole::Assistant
    };
    match role {
        SessionMessageRole::User => observation.user_messages = 1,
        SessionMessageRole::Assistant => observation.assistant_messages = 1,
    }
    // Every settled assistant response's own `data` carries the actual
    // generation identity at the top level: `modelID` / `providerID`, older
    // rows spelling them `modelId` / `providerId`. Read together with the
    // settled guard above: provenance is only trusted once `time.completed`
    // exists, and rows without the fields stay NULL.
    let (provider, model) = if kind == "assistant_response" {
        (
            data.get("providerID")
                .or_else(|| data.get("providerId"))
                .and_then(|p| p.as_str())
                .map(String::from),
            data.get("modelID")
                .or_else(|| data.get("modelId"))
                .and_then(|m| m.as_str())
                .map(String::from),
        )
    } else {
        (None, None)
    };
    if !is_root {
        return (None, observation);
    }
    let message = ParsedSessionMessage {
        source_message_id: Some(id.to_string()),
        source_position: format!("msg:{id}"),
        ts: data
            .pointer("/time/created")
            .and_then(|t| t.as_i64())
            .and_then(ms_epoch_to_rfc3339),
        role,
        content: text,
        provider,
        model,
    };
    (Some(message), observation)
}

impl ZCodeAdapter {
    fn parse_session_row(row: &rusqlite::Row) -> rusqlite::Result<(DiscoveredMember, String)> {
        let id: String = row.get(0)?;
        let parent_id: Option<String> = row.get(1)?;
        let directory: Option<String> = row.get(2)?;
        let time_created: Option<i64> = row.get(3)?;
        let time_updated: Option<i64> = row.get(4)?;
        let title: Option<String> = row.get(5)?;
        let title_source: Option<String> = row.get(6)?;
        let task_type: Option<String> = row.get(7)?;
        // ZCode says where each title came from. `generated` is a real title;
        // `first_input` is ZCode truncating the first input — the same naive
        // derivation as ours and often worse, so it is skipped and our own
        // derivation stands.
        let native_title = title
            .filter(|t| !t.trim().is_empty() && title_source.as_deref() != Some("first_input"));
        // `task_type` is the source's EXPLICIT member marker: a user fork is
        // its own continuable session (the parent is provenance only), a
        // selection side chat is the user's own little session, and the
        // child spellings are the parent's internal execution. Reading only
        // `parent_id` made a forked session's whole conversation silently
        // disappear — a child contributes observations, never conversation.
        let parent_id = parent_id.filter(|p| !p.is_empty());
        let (kind, parent) = match (parent_id.as_deref(), task_type.as_deref()) {
            (_, Some("fork")) | (_, Some("selection_side_chat")) => {
                (DiscoveredMemberKind::ForkRoot, parent_id)
            }
            // `subagent_child` / `workflow_child` / `nested_workflow_child`
            // are the known child spellings; an unknown marker still means
            // "internal execution of the parent" until proven otherwise.
            (Some(p), _) => (DiscoveredMemberKind::Child, Some(p.to_string())),
            (None, _) => (DiscoveredMemberKind::Root, None),
        };
        let is_root = kind.is_logical_root();
        Ok((
            DiscoveredMember {
                agent: Agent::ZCode,
                source_member_id: id.clone(),
                kind,
                parent_source_member_id: parent.clone(),
                root_hint: parent,
                source_kind: "zcode_store_record".into(),
                // Filled by the caller, which knows the database path.
                source_path: PathBuf::new(),
                cwd: directory.filter(|d| !d.is_empty()),
                started_at: time_created.and_then(ms_epoch_to_rfc3339),
                last_activity_at: time_updated.and_then(ms_epoch_to_rfc3339),
                native_title: if is_root { native_title } else { None },
                first_user_text: None,
                first_agent_text: None,
                metadata: serde_json::json!({ "task_type": task_type }),
            },
            id,
        ))
    }

    /// The first human turn — the fallback when the session carries no title.
    /// The first `real_user` prompt, not the first `role:"user"` row (which is a
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

    /// The first settled agent reply — the last resort for a title.
    /// Settled only: the row appears when streaming starts and is rewritten in
    /// place, so an early read would name the session after a half sentence.
    fn first_agent_text(conn: &Connection, session_id: &str) -> Result<Option<String>> {
        for (message, parts) in conversation(conn, session_id)? {
            let data = message.get("data").unwrap_or(&Value::Null);
            if message_kind(data) != Some("assistant_response") || !is_settled(data) {
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

    /// Always `None`: ZCode is a desktop app, not a CLI.
    fn detect(&self) -> Option<AgentInstallation> {
        None
    }

    fn discover_members_in(
        &self,
        roots: &[PathBuf],
        _unchanged: &dyn Fn(&Path) -> bool,
    ) -> Result<Vec<DiscoveredMember>> {
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
            // `unchanged` is deliberately NOT consulted: it is keyed by source
            // path, and every ZCode member shares this one path, so its verdict
            // would be "SOME member on this path is ingested", not "all of
            // them". The store also commits in WAL mode, so rows can appear in
            // `db.sqlite-wal` while the main file's size and mtime stay
            // identical. Reading all live sessions every pass costs one indexed
            // query each and is absorbed by message-identity dedup.
            let conn = open_read_only(&db)?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, parent_id, directory, time_created, time_updated, title, title_source, task_type
                     FROM session WHERE time_archived IS NULL ORDER BY time_created",
                )
                .map_err(|e| other(format!("查询 ZCode session 失败: {e}")))?;
            let rows = stmt
                .query_map([], Self::parse_session_row)
                .map_err(|e| other(format!("查询 ZCode session 失败: {e}")))?;
            for row in rows {
                let (mut member, id) =
                    row.map_err(|e| other(format!("读取 ZCode session 失败: {e}")))?;
                member.source_path = db.clone();
                if member.kind.is_logical_root() {
                    member.first_user_text = Self::first_user_text(&conn, &id)?;
                    member.first_agent_text = Self::first_agent_text(&conn, &id)?;
                }
                out.push(member);
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
        let mut messages = Vec::new();
        let mut observation = MemberObservation::default();
        let mut identities = std::collections::HashSet::new();
        for (message, parts) in conversation(&conn, &member.source_member_id)? {
            let data = &message["data"];
            if message_kind(data) == Some("assistant_response") && is_settled(data) {
                identities.insert((
                    data.get("modelID")
                        .or_else(|| data.get("modelId"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    data.get("providerID")
                        .or_else(|| data.get("providerId"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                ));
            }
            let (m, obs) = message_of(&message, &parts, is_root);
            observation.add_activity(&obs);
            if let Some(m) = m {
                messages.push(m);
            }
        }
        // Usage is the store's own per-turn rollup, not derivable from the
        // message rows, so it is read separately and summed onto the same
        // snapshot — one ledger event per spending turn.
        let models: std::collections::HashSet<&str> = identities
            .iter()
            .filter_map(|(model, _)| model.as_deref())
            .collect();
        let single_model = if models.len() == 1 {
            models.iter().next().copied()
        } else {
            None
        };
        // No per-turn join key exists between rollups and message identities.
        // Mixed or missing identities must never inherit the last channel.
        let single_provider = if identities.len() == 1 && single_model.is_some() {
            identities
                .iter()
                .next()
                .and_then(|(_, provider)| provider.as_deref())
        } else {
            None
        };
        let usage_events = usage_of(
            &conn,
            &member.source_member_id,
            single_model,
            single_provider,
        )?;

        // Per-member facts, never container stats: every thread in the store
        // shares one `db.sqlite` (+wal), so the file's size and mtime move
        // whenever ANY session writes — keyed on them, one session's update
        // would churn every member's cursor and stamp a shared mtime onto all
        // their `last_activity_at`. The thread's own message count is its
        // content position; the store's own `time_updated` is its real
        // activity time. Every replay is a full scan → stats SNAPSHOT, and
        // message identity absorbs the re-read.
        let (count, time_updated): (i64, Option<i64>) = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM message WHERE session_id = ?1),
                        (SELECT time_updated FROM session WHERE id = ?1)",
                [&member.source_member_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| other(format!("查询 ZCode 会话事实失败: {e}")))?;
        let source = crate::adapters::sqlite_replay_cursor_update(
            &path,
            cursor,
            crate::adapters::SqliteMemberFacts {
                position: count,
                activity_epoch_secs: time_updated.map(|ms| ms as f64 / 1000.0),
            },
        )?;
        Ok(MemberReadDelta {
            stats: crate::adapters::stats_update_from(
                &observation,
                &source,
                crate::adapters::StatsCapabilities::TOOL_CALLS,
            ),
            messages,
            source: Some(source),
            complete_snapshot: true,
            next_active_provider: None,
            next_active_model: None,
            usage_events,
        })
    }

    /// The store decides: the member's record is present → Present, gone →
    /// Missing, and a store that cannot be opened is NEVER missing, only
    /// Unavailable.
    fn inspect_member_source(&self, member: &SessionMember) -> Result<SourceAvailability> {
        let path = PathBuf::from(&member.source_path);
        let Ok(conn) = open_read_only(&path) else {
            return Ok(SourceAvailability::Unavailable);
        };
        // Three verdicts, strictly: the record is there → Present; the store
        // opened fine and does NOT hold the record → Missing (the shared-store
        // form of confirmed absence); anything the query could not answer
        // (broken store, locked, wrong shape) → Unavailable — an unreadable
        // store must never read as an absent source.
        let record = match conn.query_row(
            "SELECT 1 FROM session WHERE id = ?1",
            [&member.source_member_id],
            |r| r.get::<_, i64>(0),
        ) {
            Ok(_) => Some(()),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(_) => return Ok(SourceAvailability::Unavailable),
        };
        if record.is_some() {
            Ok(SourceAvailability::Present)
        } else {
            Ok(SourceAvailability::Missing)
        }
    }

    fn build_new_command(
        &self,
        _install: &AgentInstallation,
        _opts: &ExecOptions,
        _cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        Err(other("ZCode 是桌面应用，没有可启动的 CLI"))
    }

    fn build_resume_command(
        &self,
        _install: &AgentInstallation,
        _opts: &ExecOptions,
        _agent_session_id: &str,
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

    /// The registered scheme only offers `oauth/callback` and
    /// `workspace/open?path=` — no session route — so Continue can only
    /// activate the app. (The bundle's embedded `zcode.cjs` runs `--help`
    /// but is not usable standalone: it expects the desktop host.)
    /// App absent → refuse.
    fn desktop_app_name(&self) -> Option<&'static str> {
        Some("ZCode")
    }

    fn continue_route(&self, _member: &SessionMember) -> ResumeRoute {
        if !crate::platform::paths::app_bundle_present("ZCode") {
            return ResumeRoute::Refused("未找到 ZCode 桌面应用，无法继续该会话".into());
        }
        ResumeRoute::Desktop(DesktopResume {
            uri: "zcode://".into(),
            note: "将打开 ZCode 桌面应用（应用内不定位到该会话）".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::AgentAdapter;
    use crate::domain::{SessionMemberRelation, StatsUpdate};

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
                time_updated integer not null, data text not null, sequence integer);
             CREATE TABLE turn_usage (
                session_id text not null references session(id) on delete cascade,
                turn_id text not null,
                trace_id text, user_message_id text,
                status text not null check(status in ('running', 'completed', 'error', 'cancelled')),
                started_at integer not null,
                first_model_start_at integer, first_token_at integer, completed_at integer,
                duration_ms integer, time_to_first_token_ms integer,
                model_request_count integer not null default 0,
                model_retry_count integer not null default 0,
                tool_call_count integer not null default 0,
                tool_error_count integer not null default 0,
                input_tokens integer not null default 0,
                output_tokens integer not null default 0,
                reasoning_tokens integer not null default 0,
                cache_creation_input_tokens integer not null default 0,
                cache_read_input_tokens integer not null default 0,
                computed_total_tokens integer not null default 0,
                retryable integer not null default 0 check(retryable in (0, 1)),
                cancelled_by_user integer not null default 0 check(cancelled_by_user in (0, 1)),
                context_exceeded integer not null default 0 check(context_exceeded in (0, 1)),
                error_type text, error_code text,
                primary key(session_id, turn_id));",
        )
        .unwrap();
        path
    }

    fn open(path: &Path) -> Connection {
        Connection::open(path).unwrap()
    }

    fn session_row(conn: &Connection, id: &str, parent: Option<&str>, dir: &str, created: i64) {
        session_row_titled(conn, id, parent, dir, created, "app title", "generated");
    }

    fn session_row_titled(
        conn: &Connection,
        id: &str,
        parent: Option<&str>,
        dir: &str,
        created: i64,
        title: &str,
        title_source: &str,
    ) {
        conn.execute(
            "INSERT INTO session
               (id, project_id, parent_id, slug, directory, title, version,
                title_source, time_created, time_updated)
             VALUES (?1, 'p', ?2, ?1, ?3, ?4, '1', ?5, ?6, ?6)",
            rusqlite::params![id, parent, dir, title, title_source, created],
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

    /// One settled turn's usage, as the store's own rollup writes it.
    fn usage_row(
        conn: &Connection,
        session: &str,
        turn: &str,
        input: i64,
        output: i64,
        reasoning: i64,
        cache_read: i64,
    ) {
        conn.execute(
            "INSERT INTO turn_usage
               (session_id, turn_id, status, started_at, input_tokens, output_tokens,
                reasoning_tokens, cache_read_input_tokens)
             VALUES (?1, ?2, 'completed', 0, ?3, ?4, ?5, ?6)",
            rusqlite::params![session, turn, input, output, reasoning, cache_read],
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

    fn member_of(db: &Path, id: &str, relation: SessionMemberRelation) -> SessionMember {
        SessionMember {
            id: "mem-zcode".into(),
            session_id: "sess-zcode".into(),
            agent: Agent::ZCode,
            source_member_id: id.into(),
            relation,
            parent_source_member_id: None,
            source_kind: "zcode_store_record".into(),
            source_path: db.to_string_lossy().to_string(),
            cwd: None,
            started_at: None,
            last_activity_at: None,
            metadata: serde_json::json!({}),
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

    /// A `parent_id` makes a live row a CHILD member of that parent, never a
    /// fork: the source cannot express one, so the child carries no title.
    #[test]
    fn discovery_lists_live_sessions_and_maps_parents_to_children() {
        let root = unique_dir("discover");
        let db = store(&root);
        let conn = open(&db);
        session_row(&conn, "root-child", Some("main"), "/repo/sub", 300);
        session_row(&conn, "main", None, "/repo", 200);
        session_row(&conn, "retired", None, "/repo/old", 100);
        archive(&conn, "retired", 400);
        drop(conn);

        let found = ZCodeAdapter
            .discover_members_in(&[root], &|_| false)
            .unwrap();
        // Newest first: the store's `ORDER BY time_created`, and the archived
        // row is ZCode's own "hidden" verdict — NoEnding does not resurrect it.
        let by_id: std::collections::HashMap<&str, &DiscoveredMember> = found
            .iter()
            .map(|m| (m.source_member_id.as_str(), m))
            .collect();
        assert_eq!(found.len(), 2, "the archived row stays hidden");
        let child = by_id["root-child"];
        assert_eq!(child.kind, DiscoveredMemberKind::Child);
        assert_eq!(child.parent_source_member_id.as_deref(), Some("main"));
        assert_eq!(child.cwd.as_deref(), Some("/repo/sub"));
        assert_eq!(child.native_title, None, "a child never carries a title");
        let main = by_id["main"];
        assert_eq!(main.kind, DiscoveredMemberKind::Root);
        assert_eq!(
            main.native_title.as_deref(),
            Some("app title"),
            "the session's own title column outranks anything derived from the messages"
        );
    }

    /// A `first_input` title is ZCode's own naive truncation of the first input
    /// — the same thing we would derive, and often worse. It is not a title the
    /// Agent thought about, so the derived one stands.
    #[test]
    fn a_first_input_title_is_not_treated_as_native() {
        let root = unique_dir("title-source");
        let db = store(&root);
        let conn = open(&db);
        session_row_titled(
            &conn,
            "s",
            None,
            "/repo",
            100,
            "Explore the codebase at /Users/jqk/proje",
            "first_input",
        );
        message_row(&conn, "m2", "s", 1, prompt());
        part_row(&conn, "p2", "m2", "s", 0, text_part("只看这一句"));
        drop(conn);

        let found = ZCodeAdapter
            .discover_members_in(&[root], &|_| false)
            .unwrap();
        assert_eq!(found[0].native_title, None);
        assert_eq!(found[0].first_user_text.as_deref(), Some("只看这一句"));
    }

    #[test]
    fn the_title_is_the_first_real_prompt_not_a_reminder() {
        let root = unique_dir("title");
        let db = store(&root);
        let conn = open(&db);
        session_row(&conn, "s", None, "/repo", 100);
        // The runtime speaks first and much more often: a reminder would have
        // supplied the title if `role` were trusted.
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
            .discover_members_in(&[root], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].first_user_text.as_deref(), Some("真正的问题"));
    }

    /// reminders, reasoning and tool traffic are observed, not stored;
    /// only the `text` parts of a settled root conversation become messages.
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
            .read_member_delta(
                &member_of(&db, "s", SessionMemberRelation::Root),
                &SessionMemberCursor::default(),
            )
            .unwrap();
        let got: Vec<(SessionMessageRole, &str, Option<&str>)> = delta
            .messages
            .iter()
            .map(|m| (m.role, m.content.as_str(), m.source_message_id.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                (SessionMessageRole::User, "人写的", Some("m3")),
                (SessionMessageRole::Assistant, "回答\n尾部", Some("m4")),
            ]
        );
        match delta.stats {
            Some(StatsUpdate::Snapshot(s)) => {
                assert_eq!(s.tool_call_count, Some(1), "the tool part is observed");
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
        // The cursor stays in the member's own coordinates — the thread's
        // message count — never the container file's size.
        assert_eq!(delta.source.unwrap().byte_offset, 4);
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

        let member = member_of(&db, "s", SessionMemberRelation::Root);
        let first = ZCodeAdapter
            .read_member_delta(&member, &SessionMemberCursor::default())
            .unwrap();
        let ids = |d: &MemberReadDelta| -> Vec<String> {
            d.messages
                .iter()
                .filter_map(|m| m.source_message_id.clone())
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
        // is fine: message identity is the message id, so the store keeps it once.
        let cursor =
            SessionMemberCursor::from_update(member.id.as_str(), first.source.as_ref().unwrap());
        let second = ZCodeAdapter.read_member_delta(&member, &cursor).unwrap();
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
        // the `compaction` PART is the structural marker.
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
            .read_member_delta(
                &member_of(&db, "s", SessionMemberRelation::Root),
                &SessionMemberCursor::default(),
            )
            .unwrap();
        assert!(delta.messages.is_empty());
        match delta.stats {
            Some(StatsUpdate::Snapshot(s)) => {
                assert_eq!(s.user_message_count, Some(0));
                assert_eq!(s.assistant_message_count, Some(0));
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
        assert!(delta.usage_events.is_empty());
    }

    /// Usage comes from the store's own per-turn rollup and adds up across the
    /// session's turns; the message rows never carry it.
    #[test]
    fn usage_sums_the_turn_rollup() {
        let root = unique_dir("usage");
        let db = store(&root);
        let conn = open(&db);
        session_row(&conn, "s", None, "/repo", 100);
        usage_row(&conn, "s", "t1", 100, 10, 3, 40);
        usage_row(&conn, "s", "t2", 8459, 64, 0, 7000);
        drop(conn);

        let delta = ZCodeAdapter
            .read_member_delta(
                &member_of(&db, "s", SessionMemberRelation::Root),
                &SessionMemberCursor::default(),
            )
            .unwrap();
        let usage = crate::adapters::test_usage_tokens(&delta.usage_events);
        match delta.stats {
            Some(StatsUpdate::Snapshot(_)) => {
                assert_eq!(usage[0], 1519,
                    "fresh input: (100−40) + (8459−7000); the store's input column includes the cache"
                );
                assert_eq!(usage[1], 74);
                assert_eq!(usage[3], 3);
                assert_eq!(usage[2], 7040, "cache_read_input_tokens");
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
        // One ledger event per spending turn, keyed on the store's own turn id.
        assert_eq!(delta.usage_events.len(), 2);
        assert_eq!(
            delta
                .usage_events
                .iter()
                .map(|e| e.key.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("turn:t1"), Some("turn:t2")]
        );
        assert_eq!(
            delta
                .usage_events
                .iter()
                .map(|e| e.input_tokens)
                .sum::<u64>(),
            1519,
            "event totals contain the fresh input from every turn"
        );
    }

    #[test]
    fn cache_creation_is_ignored_without_losing_request_only_turns() {
        let root = unique_dir("usage-no-cache-creation");
        let db = store(&root);
        let conn = open(&db);
        session_row(&conn, "s", None, "/repo", 100);
        usage_row(&conn, "s", "requests", 0, 0, 0, 0);
        usage_row(&conn, "s", "empty", 0, 0, 0, 0);
        conn.execute(
            "UPDATE turn_usage SET cache_creation_input_tokens = 12345",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE turn_usage SET model_request_count = 3 WHERE turn_id = 'requests'",
            [],
        )
        .unwrap();
        let events = usage_of(&conn, "s", None, None).unwrap();
        assert_eq!(crate::adapters::test_usage_tokens(&events), [0; 4]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].key.as_deref(), Some("turn:requests"));
        assert_eq!(events[0].request_count, 3);
        drop(conn);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The per-turn events carry the session's model only when the read's
    /// own assistant messages agree on ONE; a mixed-model session attributes
    /// nothing rather than guessing.
    #[test]
    fn turn_events_carry_the_single_model_or_none() {
        let root = unique_dir("usage-model");
        let db_path = store(&root);
        let conn = open(&db_path);
        session_row(&conn, "s", None, "/repo", 100);
        // Two assistant replies, both naming GLM-5.3 (different key spellings
        // the store has been seen to write).
        let mut r1 = reply(true);
        r1["modelID"] = serde_json::json!("GLM-5.3");
        message_row(&conn, "m1", "s", 1, r1);
        part_row(&conn, "p1", "m1", "s", 1, text_part("回答一"));
        let mut r2 = reply(true);
        r2["modelId"] = serde_json::json!("GLM-5.3");
        message_row(&conn, "m2", "s", 2, r2);
        part_row(&conn, "p2", "m2", "s", 1, text_part("回答二"));
        usage_row(&conn, "s", "t1", 100, 10, 3, 40);
        usage_row(&conn, "s", "t2", 200, 20, 0, 0);
        drop(conn);

        let delta = ZCodeAdapter
            .read_member_delta(
                &member_of(&db_path, "s", SessionMemberRelation::Root),
                &SessionMemberCursor::default(),
            )
            .unwrap();
        assert!(delta
            .usage_events
            .iter()
            .all(|e| e.model.as_deref() == Some("GLM-5.3")));

        // A second session mixing two models: the turns stay unattributed.
        let conn = open(&db_path);
        session_row(&conn, "s2", None, "/repo", 100);
        let mut a = reply(true);
        a["modelID"] = serde_json::json!("GLM-5.3");
        message_row(&conn, "a1", "s2", 1, a);
        part_row(&conn, "a1p", "a1", "s2", 1, text_part("一"));
        let mut b = reply(true);
        b["modelID"] = serde_json::json!("GLM-5.3-Flash");
        message_row(&conn, "a2", "s2", 2, b);
        part_row(&conn, "a2p", "a2", "s2", 1, text_part("二"));
        usage_row(&conn, "s2", "u1", 50, 5, 0, 0);
        drop(conn);

        let delta = ZCodeAdapter
            .read_member_delta(
                &member_of(&db_path, "s2", SessionMemberRelation::Root),
                &SessionMemberCursor::default(),
            )
            .unwrap();
        assert_eq!(delta.usage_events.len(), 1);
        assert_eq!(delta.usage_events[0].model, None, "mixed models: no guess");
    }

    /// No ZCode member is ever a ForkRoot: the source cannot express a genuine
    /// user fork, and only an explicit fork marker would change that.
    #[test]
    fn task_type_decides_the_member_kind() {
        let root = unique_dir("task-type");
        let db = store(&root);
        let conn = open(&db);
        session_row(&conn, "main", None, "/repo", 100);
        conn.execute(
            "UPDATE session SET task_type = 'fork' WHERE id = 'main'",
            [],
        )
        .unwrap();
        session_row(&conn, "branched", Some("main"), "/repo", 200);
        conn.execute(
            "UPDATE session SET task_type = 'subagent_child' WHERE id = 'branched'",
            [],
        )
        .unwrap();
        session_row(&conn, "side-chat", Some("main"), "/repo", 300);
        conn.execute(
            "UPDATE session SET task_type = 'selection_side_chat' WHERE id = 'side-chat'",
            [],
        )
        .unwrap();
        drop(conn);
        let found = ZCodeAdapter
            .discover_members_in(&[root], &|_| false)
            .unwrap();
        let kind_of = |id: &str| {
            found
                .iter()
                .find(|m| m.source_member_id == id)
                .map(|m| m.kind)
                .unwrap()
        };
        // A genuine fork — even with a parent — is its own continuable
        // session; swallowing it as a child erased its whole conversation.
        assert_eq!(kind_of("main"), DiscoveredMemberKind::ForkRoot);
        assert_eq!(
            kind_of("branched"),
            DiscoveredMemberKind::Child,
            "subagent execution stays internal"
        );
        assert_eq!(
            kind_of("side-chat"),
            DiscoveredMemberKind::ForkRoot,
            "a selection side chat is the user's own session"
        );
    }

    /// For a shared store: the record's absence is a confirmed Missing; an
    /// unopenable store is only ever Unavailable.
    #[test]
    fn inspect_reads_the_record_not_the_file() {
        let root = unique_dir("inspect");
        let db = store(&root);
        let conn = open(&db);
        session_row(&conn, "alive", None, "/repo", 100);
        drop(conn);

        assert_eq!(
            ZCodeAdapter
                .inspect_member_source(&member_of(&db, "alive", SessionMemberRelation::Root))
                .unwrap(),
            SourceAvailability::Present
        );
        assert_eq!(
            ZCodeAdapter
                .inspect_member_source(&member_of(&db, "purged", SessionMemberRelation::Root))
                .unwrap(),
            SourceAvailability::Missing
        );

        // A file that exists but is not a usable store is Unavailable.
        let broken = root.join("broken.sqlite");
        std::fs::write(&broken, b"not a database").unwrap();
        assert_eq!(
            ZCodeAdapter
                .inspect_member_source(&member_of(&broken, "s", SessionMemberRelation::Root))
                .unwrap(),
            SourceAvailability::Unavailable
        );
    }

    /// A settled assistant response's own data carries its generation identity
    /// in either field spelling; a row without the fields stays NULL.
    #[test]
    fn settled_assistant_responses_carry_their_model_and_provider() {
        let root = unique_dir("prov");
        let db = store(&root);
        let conn = open(&db);
        session_row(&conn, "s", None, "/repo", 100);
        let mut pascal = reply(true);
        pascal["modelID"] = serde_json::json!("GLM-5.3");
        pascal["providerID"] = serde_json::json!("builtin:bigmodel");
        message_row(&conn, "m1", "s", 1, pascal);
        part_row(&conn, "p1", "m1", "s", 1, text_part("回答一"));
        let mut camel = reply(true);
        camel["modelId"] = serde_json::json!("GLM-5.3-Flash");
        camel["providerId"] = serde_json::json!("bigmodel-api");
        message_row(&conn, "m2", "s", 2, camel);
        part_row(&conn, "p2", "m2", "s", 1, text_part("回答二"));
        message_row(&conn, "m3", "s", 3, reply(true));
        part_row(&conn, "p3", "m3", "s", 1, text_part("回答三"));
        drop(conn);

        let delta = ZCodeAdapter
            .read_member_delta(
                &member_of(&db, "s", SessionMemberRelation::Root),
                &SessionMemberCursor::default(),
            )
            .unwrap();
        let assistant: Vec<(Option<&str>, Option<&str>)> = delta
            .messages
            .iter()
            .filter(|m| m.role == SessionMessageRole::Assistant)
            .map(|m| (m.provider.as_deref(), m.model.as_deref()))
            .collect();
        assert_eq!(
            assistant,
            vec![
                (Some("builtin:bigmodel"), Some("GLM-5.3")),
                (Some("bigmodel-api"), Some("GLM-5.3-Flash")),
                (None, None),
            ]
        );
    }

    /// The store holds every thread in ONE db file: another thread's write
    /// moves the container's stats but must not move THIS thread's cursor or
    /// advance its `last_activity_at` — one session's update must never churn
    /// the whole agent's sessions.
    #[test]
    fn another_threads_write_does_not_churn_this_thread() {
        use crate::storage::Db;

        let root = unique_dir("shared-store");
        let db_path = store(&root);
        let conn = open(&db_path);
        session_row(&conn, "a", None, "/repo-a", 100);
        session_row(&conn, "b", None, "/repo-b", 200);
        message_row(&conn, "b1", "b", 0, prompt());
        part_row(&conn, "bp1", "b1", "b", 0, text_part("B 的提问"));
        drop(conn);

        let db = Db::open(&root.join("noending.db")).unwrap();
        let (session_id, _) = db
            .upsert_logical_session_unchecked(
                Agent::ZCode,
                "b",
                Some("B"),
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
                Agent::ZCode,
                "b",
                SessionMemberRelation::Root,
                None,
                "zcode_store_record",
                &db_path.to_string_lossy(),
                None,
                None,
                None,
                &serde_json::json!({}),
            )
            .unwrap();
        let member_b = member_of(&db_path, "b", SessionMemberRelation::Root);

        // Pass 1: thread B ingested; its activity is its own time_updated.
        let delta = ZCodeAdapter
            .read_member_delta(&member_b, &SessionMemberCursor::default())
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
            &[],
        )
        .unwrap();
        let activity_before = db
            .get_session(&session_id)
            .unwrap()
            .unwrap()
            .last_activity_at;

        // Thread A writes: a new message plus a bumped time_updated. The
        // container file's stats move; thread B's facts do not.
        let conn = open(&db_path);
        message_row(&conn, "a1", "a", 0, prompt());
        conn.execute(
            "UPDATE session SET time_updated = ?1 WHERE id = 'a'",
            rusqlite::params![5_100],
        )
        .unwrap();
        drop(conn);

        // Pass 2: B's re-read sees an unchanged source, and the commit must
        // not touch its activity.
        let cursor = db.get_member_cursor(&member_id).unwrap();
        let delta = ZCodeAdapter.read_member_delta(&member_b, &cursor).unwrap();
        let source = delta.source.unwrap();
        assert_eq!(
            source.byte_offset, cursor.byte_offset,
            "B's content position is untouched by A's write"
        );
        assert_eq!(
            source.mtime, cursor.mtime,
            "B's activity time is its own, not the container's"
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
            &[],
        )
        .unwrap();
        let activity_after = db
            .get_session(&session_id)
            .unwrap()
            .unwrap()
            .last_activity_at;
        assert_eq!(
            activity_after, activity_before,
            "another thread's write must not churn this session's activity"
        );
    }
    #[test]
    fn usage_channel_requires_unambiguous_settled_generation_identity() {
        let root = unique_dir("usage-channel");
        let db = store(&root);
        let conn = open(&db);
        session_row(&conn, "s", None, "/repo", 100);
        let mut generation = reply(true);
        generation["modelID"] = serde_json::json!("glm-test");
        generation["providerID"] = serde_json::json!("302ai");
        message_row(&conn, "m1", "s", 1, generation.clone());
        usage_row(&conn, "s", "t1", 100, 10, 0, 0);
        let member = member_of(&db, "s", SessionMemberRelation::Child);
        let read = || {
            ZCodeAdapter
                .read_member_delta(&member, &SessionMemberCursor::default())
                .unwrap()
        };
        let first = read();
        assert!(first.messages.is_empty());
        assert_eq!(first.usage_events[0].provider.as_deref(), Some("302ai"));
        assert_eq!(first.usage_events[0].model.as_deref(), Some("glm-test"));
        generation["providerID"] = serde_json::json!("zai");
        message_row(&conn, "m2", "s", 2, generation);
        assert_eq!(
            read().usage_events[0].provider,
            None,
            "mixed channels have no per-turn join"
        );
        conn.execute("DELETE FROM message WHERE id='m2'", [])
            .unwrap();
        message_row(&conn, "m3", "s", 3, reply(true));
        assert_eq!(
            read().usage_events[0].provider,
            None,
            "an unknown generation must not inherit a channel"
        );
    }
}
