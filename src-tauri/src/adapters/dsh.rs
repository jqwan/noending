//! dsh Adapter (DeepSeek Harness): `$DSH_HOME/sessions/<encoded-cwd>/<id>/session.v2.jsonl.zstd`,
//! `$DSH_HOME` defaulting to `~/.dsh`.
//!
//! The only agent whose source bytes are not JSONL: every transcript is zstd,
//! and the file grows by **appending complete zstd frames** (one checksummed
//! frame per durable write batch, `packages/session/session-persistence-jsonl`).
//! Committed bytes are never rewritten — the two truncations dsh performs are
//! rollback of an in-flight append and repair of a torn tail, both of which
//! only ever remove the *last, incomplete* frame. That is why this adapter's
//! cursor still detects append / truncate / replacement exactly like the plain
//! JSONL readers do; only the coordinates differ (see below).
//!
//! Record shape (decoded):
//! - line 0 is the session header `{type:"session", version, id, createdAt,
//!   cwd, …}`; it is the only record without a `seq`;
//! - every later record is `{type, seq, time, data}` and only the conversation
//!   is ingested: `user/message`, `assistant/message`, and `compaction/*` as a
//!   boundary marker. `assistant/chunk` (v0/v1 token deltas), `tool/call`,
//!   `tool/result`, `todo/write`, `request/*`, `session/title*` and the
//!   turn/step bookkeeping are machine traffic and are dropped (§36.11).
//! - `seq` is writer-assigned, contiguous and monotonic within a generation,
//!   so it is the native event id — dedup is exact and position-independent.
//!
//! Two shapes need real work:
//! - **The user turn is not always the user's words.** dsh injects its own
//!   `user/message` records for runtime context ("Current runtime context. …"
//!   and `<system-reminder>` blocks). Ingesting those as user text would make
//!   every session start with a machine preamble, so they are dropped, exactly
//!   as codex's `<…>`/`#` injections are for the title.
//! - **Cursor coordinates.** The shared reader's offsets live in the bytes it
//!   parses, and those are decoded here, not on disk. This adapter reads the
//!   whole decoded body every time and lets `seq` dedup absorb it, while the
//!   stored cursor keeps raw-file identity/size/mtime so the reconcile
//!   pre-filter ("already ingested and stat-identical") still works.
//!
//! There is no launchable CLI: `dsh --profile <name>` needs a profile name
//! that is the user's own setup, and NoEnding cannot know it — guessing one
//! would boot the wrong tree (方案 §37.8).

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::{
    ms_epoch_to_rfc3339, AgentCommand, DiscoveredSession, ExecOptions, ParsedLine, ReadDelta,
};
use crate::domain::{Agent, ParsedEvent, Session, SourceCursor};
use crate::error::{other, Result};
use crate::platform::exec_resolver::AgentInstallation;

pub struct DshAdapter;

/// Generations dsh publishes beside each other, newest last (README of
/// `session-persistence-jsonl`: v0 has no version infix; a migration publishes
/// a NEW file and never touches the old one). A session directory can hold
/// several at once after an upgrade, and they describe the same session id —
/// so discovery must claim exactly one: the highest generation present.
fn generation_of(file_name: &str) -> Option<u8> {
    match file_name {
        "session.v2.jsonl.zstd" => Some(2),
        "session.v1.jsonl.zstd" => Some(1),
        "session.jsonl.zstd" => Some(0),
        _ => None,
    }
}

/// Does this decoded record open a dsh transcript? The header is a `session`
/// record with a `createdAt`; requiring it is what keeps a pi header (same
/// `type`/`version`/`id` shape, no `createdAt`) from ever being claimed.
fn is_session_header(v: &Value) -> bool {
    v.get("type").and_then(|t| t.as_str()) == Some("session")
        && v.get("createdAt").map(|c| c.is_number()).unwrap_or(false)
        && v.get("id").map(|i| i.is_string()).unwrap_or(false)
}

/// Text of a content-block array, keeping only human prose. Reasoning and
/// tool-call blocks are machine traffic (§36.11).
fn text_blocks(content: Option<&Value>) -> String {
    let mut parts = Vec::new();
    if let Some(arr) = content.and_then(|c| c.as_array()) {
        for item in arr {
            if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                    parts.push(t.to_string());
                }
            }
        }
    }
    parts.join("\n")
}

/// dsh's own injected context, written as a `user/message` but not written by
/// the user: the runtime snapshot and `<system-reminder>` blocks.
fn is_machine_context(text: &str) -> bool {
    text.starts_with('<') || text.starts_with("Current runtime context")
}

/// Decoded lines of a transcript, stopping early when `keep_going` says so.
/// A torn final frame ends the walk instead of erroring — the complete prefix
/// is exactly what a mid-write file has to offer, and the next read sees the
/// finished frame.
fn scan_lines(raw: &[u8], mut keep_going: impl FnMut(&str) -> bool) {
    let Ok(mut decoder) = zstd::stream::read::Decoder::new(raw) else {
        return;
    };
    let mut pending: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let n = match decoder.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        pending.extend_from_slice(&chunk[..n]);
        while let Some(end) = pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = pending.drain(..=end).collect();
            let text = String::from_utf8_lossy(&line[..line.len() - 1]);
            if !keep_going(&text) {
                return;
            }
        }
    }
    if !pending.is_empty() {
        keep_going(&String::from_utf8_lossy(&pending));
    }
}

/// The subtree that actually holds transcripts. dsh's layout is fixed —
/// `<root>/sessions/<encoded-cwd>/<id>/` — and the rest of `$DSH_HOME` is its
/// plugin tree (a `node_modules` forest): walking that is both enormous and a
/// false-positive risk, so it is never descended into.
fn session_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    roots
        .iter()
        .map(|root| {
            if root.file_name().and_then(|n| n.to_str()) == Some("sessions") {
                return root.clone();
            }
            let sessions = root.join("sessions");
            if sessions.is_dir() {
                sessions
            } else {
                root.clone()
            }
        })
        .collect()
}

fn parsed_line(v: &Value) -> Option<ParsedLine> {
    let vtype = v.get("type").and_then(|t| t.as_str())?;
    let source_event_id = v.get("seq").and_then(|s| s.as_i64()).map(|s| s.to_string());
    let (kind, text) = match vtype {
        "user/message" => {
            let text = text_blocks(v.pointer("/data/content"));
            if text.trim().is_empty() || is_machine_context(&text) {
                return None;
            }
            ("user_message", text)
        }
        "assistant/message" => {
            let text = text_blocks(v.pointer("/data/message/content"));
            if text.trim().is_empty() {
                return None;
            }
            ("assistant_message", text)
        }
        // Compaction is a boundary worth marking, but its payload is machine
        // bookkeeping: a marker, never the pruned content itself.
        t if t.starts_with("compaction/") => ("compact", "conversation compacted".into()),
        _ => return None,
    };
    Some(ParsedLine {
        kind: kind.into(),
        text: Some(text),
        source_event_id,
        metadata: serde_json::json!({ "agent": "dsh", "type": vtype }),
    })
}

impl DshAdapter {
    fn parse_session_file(path: &Path) -> Result<Option<DiscoveredSession>> {
        let raw = std::fs::read(path)?;
        let mut header: Option<Value> = None;
        let mut first_user_text: Option<String> = None;

        scan_lines(&raw, |line| {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                return true;
            };
            if header.is_none() {
                if is_session_header(&v) {
                    header = Some(v);
                }
                // The header opens the file; anything else means this is not a
                // dsh transcript, and a mid-file header is not evidence.
                return header.is_some();
            }
            if first_user_text.is_none()
                && v.get("type").and_then(|t| t.as_str()) == Some("user/message")
            {
                let text = text_blocks(v.pointer("/data/content"));
                if !text.trim().is_empty() && !is_machine_context(&text) {
                    first_user_text = Some(crate::adapters::truncate_text(&text, 400));
                }
            }
            first_user_text.is_none()
        });

        let Some(header) = header else {
            return Ok(None);
        };
        let id = header
            .get("id")
            .and_then(|i| i.as_str())
            .ok_or_else(|| other("dsh session 头缺少 id"))?;

        let meta = std::fs::metadata(path)?;
        let last_activity = meta
            .modified()
            .ok()
            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());

        Ok(Some(DiscoveredSession {
            agent: Agent::Dsh,
            agent_session_id: id.to_string(),
            path: path.to_path_buf(),
            cwd: header
                .get("cwd")
                .and_then(|c| c.as_str())
                .filter(|c| !c.is_empty())
                .map(|c| c.to_string()),
            started_at: header
                .get("createdAt")
                .and_then(|c| c.as_i64())
                .and_then(ms_epoch_to_rfc3339),
            last_activity_at: last_activity,
            first_user_text,
            // Sub-agent sessions are ordinary siblings with their own id; the
            // header names the parent, so the link is a fact, not a guess.
            parent_agent_session_id: header
                .get("parentSession")
                .and_then(|p| p.as_str())
                .map(|p| p.to_string()),
        }))
    }
}

impl crate::adapters::AgentAdapter for DshAdapter {
    fn agent(&self) -> Agent {
        Agent::Dsh
    }

    /// Always `None`: `dsh` has a CLI, but every invocation must name a profile
    /// under `$DSH_HOME/profiles` that only the user knows (方案 §37.8).
    fn detect(&self) -> Option<AgentInstallation> {
        None
    }

    fn discover_sessions_in(
        &self,
        roots: &[PathBuf],
        unchanged: &dyn Fn(&Path) -> bool,
    ) -> Result<Vec<DiscoveredSession>> {
        let mut out = Vec::new();
        let mut stack: Vec<PathBuf> = session_roots(roots);
        while let Some(dir) = stack.pop() {
            let rd = match std::fs::read_dir(&dir) {
                Ok(rd) => rd,
                Err(_) => continue,
            };
            // One session per directory: the newest published generation wins,
            // so an upgraded session is never ingested twice under one id.
            let mut newest: Option<(u8, PathBuf)> = None;
            for entry in rd.filter_map(|e| e.ok()) {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let Some(generation) = generation_of(name) else {
                    continue;
                };
                if newest
                    .as_ref()
                    .map(|(g, _)| generation > *g)
                    .unwrap_or(true)
                {
                    newest = Some((generation, p));
                }
            }
            let Some((_, path)) = newest else {
                continue;
            };
            if unchanged(&path) {
                continue;
            }
            match Self::parse_session_file(&path) {
                Ok(Some(s)) => out.push(s),
                Ok(None) => {}
                Err(e) => eprintln!("[discover] skip {}: {}", path.display(), e),
            }
        }
        Ok(out)
    }

    fn read_delta(&self, session: &Session, cursor: &SourceCursor) -> Result<ReadDelta> {
        let path = PathBuf::from(&session.raw_path);
        let raw = std::fs::read(&path)?;

        // Decoded text cannot be seeked into, so every read replays the whole
        // body and leans on event identity: each record carries the writer's
        // own `seq`, so a replay stores nothing it already has.
        let mut index = 0usize;
        let mut events: Vec<ParsedEvent> = Vec::new();
        scan_lines(&raw, |line| {
            let position = index + 1;
            index += 1;
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                return true;
            };
            let Some(p) = parsed_line(&v) else {
                return true;
            };
            events.push(ParsedEvent {
                source_event_id: p.source_event_id,
                source_position: format!("line:{position}"),
                ts: v
                    .get("time")
                    .and_then(|t| t.as_i64())
                    .and_then(ms_epoch_to_rfc3339),
                kind: p.kind,
                text: p.text,
                metadata: p.metadata,
            });
            true
        });

        Ok(ReadDelta {
            events,
            source: Some(crate::adapters::replay_cursor_update(&path, cursor, &raw)?),
        })
    }

    fn build_new_command(
        &self,
        _install: &AgentInstallation,
        _opts: &ExecOptions,
        _context_file: Option<&Path>,
        _cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        Err(other("dsh 需要指定 profile，NoEnding 无法替用户选择"))
    }

    fn build_resume_command(
        &self,
        _install: &AgentInstallation,
        _opts: &ExecOptions,
        _agent_session_id: &str,
        _context_file: Option<&Path>,
        _cwd: Option<&Path>,
    ) -> Result<AgentCommand> {
        Err(other("dsh 需要指定 profile，NoEnding 无法替用户选择"))
    }

    fn build_exec_command(
        &self,
        _install: &AgentInstallation,
        _opts: &ExecOptions,
        _prompt: &str,
    ) -> Result<AgentCommand> {
        Err(other("dsh 需要指定 profile，NoEnding 无法替用户选择"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::AgentAdapter;

    fn unique_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "noending-dsh-{}-{}-{}",
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

    fn frame(body: &str) -> Vec<u8> {
        zstd::encode_all(body.as_bytes(), 3).unwrap()
    }

    /// A session directory as dsh writes it: `<root>/<slug>/<id>/<file>`, one
    /// zstd frame per write batch.
    fn session_dir(root: &Path, id: &str, file_name: &str, frames: &[&str]) -> PathBuf {
        let dir = root.join("proj").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        let mut bytes = Vec::new();
        for f in frames {
            // One frame per write batch; every record is newline-terminated.
            bytes.extend(frame(&format!("{f}\n")));
        }
        std::fs::write(dir.join(file_name), bytes).unwrap();
        dir
    }

    fn header(id: &str, extra: &str) -> String {
        format!(
            r#"{{"type":"session","version":2,"id":"{id}","createdAt":1788969915099,"cwd":"/repo","isSeeded":false,"delegationDepth":0,"agentPreset":"standard"{extra}}}"#
        )
    }

    fn user(seq: i64, text: &str) -> String {
        format!(
            r#"{{"type":"user/message","seq":{seq},"time":1788969927205,"data":{{"content":[{{"type":"text","text":"{text}"}}],"role":"user"}}}}"#
        )
    }

    fn assistant(seq: i64, text: &str) -> String {
        format!(
            r#"{{"type":"assistant/message","seq":{seq},"time":1788969948545,"data":{{"turn":1,"step":1,"message":{{"role":"assistant","content":[{{"type":"reasoning","text":"thinking"}},{{"type":"text","text":"{text}"}}]}}}}}}"#
        )
    }

    const RUNTIME_CTX: &str = r#"{"type":"user/message","seq":1,"time":1788969927205,"data":{"content":[{"type":"text","text":"Current runtime context. This snapshot supersedes earlier runtime-context snapshots."}],"role":"user"}}"#;
    const REMINDER: &str = r#"{"type":"user/message","seq":2,"time":1788969927206,"data":{"content":[{"type":"text","text":"<system-reminder>\nworkspace instructions\n</system-reminder>"}],"role":"user"}}"#;
    const TOOL_CALL: &str = r#"{"type":"tool/call","seq":9,"time":1788969930000,"data":{"name":"read","arguments":"{}"}}"#;

    #[test]
    fn discovery_reads_header_facts_and_skips_injected_context() {
        let root = unique_dir("parse");
        let id = "session-abc";
        session_dir(
            &root,
            id,
            "session.v2.jsonl.zstd",
            &[
                &header(id, ""),
                RUNTIME_CTX,
                &user(3, "请审计这个仓库"),
                &assistant(4, "好的。"),
            ],
        );
        let found = DshAdapter
            .discover_sessions_in(&[root.clone()], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 1, "{found:#?}");
        let s = &found[0];
        assert_eq!(s.agent, Agent::Dsh);
        assert_eq!(s.agent_session_id, id);
        assert_eq!(s.cwd.as_deref(), Some("/repo"));
        // The runtime snapshot is a user/message too, but not the user's words.
        assert_eq!(s.first_user_text.as_deref(), Some("请审计这个仓库"));
        assert_eq!(
            s.started_at.as_deref(),
            Some("2026-09-09T16:05:15.099+00:00")
        );
        assert!(s.parent_agent_session_id.is_none());
    }

    #[test]
    fn the_newest_published_generation_wins() {
        let root = unique_dir("generations");
        let id = "12f75210-3fd8-4500-ab7d-014646eb1462";
        let dir = session_dir(
            &root,
            id,
            "session.v2.jsonl.zstd",
            &[
                &header(id, ""),
                &user(1, "second generation prompt"),
                &assistant(2, "second generation reply"),
            ],
        );
        // An upgraded session keeps its older files beside the new one, and
        // they describe the same id: two claims would fight over one row.
        std::fs::write(
            dir.join("session.jsonl.zstd"),
            frame(&format!(
                "{}\n{}\n",
                r#"{"type":"session","version":0,"id":"12f75210-3fd8-4500-ab7d-014646eb1462","createdAt":1788355822116,"cwd":"/repo"}"#,
                user(7, "legacy prompt")
            )),
        )
        .unwrap();

        let found = DshAdapter
            .discover_sessions_in(&[root], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(
            found[0].first_user_text.as_deref(),
            Some("second generation prompt")
        );
        assert!(
            found[0].path.ends_with("session.v2.jsonl.zstd"),
            "the newest generation is the source: {:?}",
            found[0].path
        );
    }

    #[test]
    fn ingest_keeps_the_conversation_and_drops_machine_traffic() {
        let id = "session-ingest";
        let dir = session_dir(
            &unique_dir("ingest"),
            id,
            "session.v2.jsonl.zstd",
            &[
                &header(id, ""),
                r#"{"type":"permission/preset","seq":0,"time":1,"data":{"preset":"workspace-write"}}"#,
                RUNTIME_CTX,
                REMINDER,
                &user(3, "帮我看看这个 bug"),
                TOOL_CALL,
                &assistant(5, "看完了。"),
                r#"{"type":"session/title","seq":6,"time":2,"data":{"title":"看 bug"}}"#,
                r#"{"type":"compaction/prune","seq":7,"time":3,"data":{}}"#,
            ],
        );
        let file = dir.join("session.v2.jsonl.zstd");
        let session = Session {
            id: "sess-dsh".into(),
            agent: Agent::Dsh,
            agent_session_id: id.into(),
            title: None,
            cwd: None,
            workspace_path_id: None,
            project_id: None,
            raw_path: file.to_string_lossy().to_string(),
            parent_agent_session_id: None,
            started_at: None,
            last_activity_at: None,
            trashed_at: None,
        };
        let delta = DshAdapter
            .read_delta(&session, &SourceCursor::default())
            .unwrap();
        let kinds: Vec<(&str, &str)> = delta
            .events
            .iter()
            .map(|e| (e.kind.as_str(), e.text.as_deref().unwrap_or("")))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("user_message", "帮我看看这个 bug"),
                ("assistant_message", "看完了。"),
                ("compact", "conversation compacted"),
            ],
            "got {kinds:?}"
        );
        // The writer's own seq is the native id, so dedup is exact.
        assert_eq!(delta.events[0].source_event_id.as_deref(), Some("3"));
        assert_eq!(
            delta.events[0].ts.as_deref(),
            Some("2026-09-09T16:05:27.205+00:00")
        );
    }

    #[test]
    fn subagent_sessions_keep_their_parent_link_and_own_identity() {
        let root = unique_dir("subagent");
        let child = "06a1b2c3-0000-4000-8000-000000000001";
        let parent = "session-parent";
        session_dir(
            &root,
            child,
            "session.v2.jsonl.zstd",
            &[
                &header(
                    child,
                    r#","parentSession":"session-parent","origin":"subagent","delegationDepth":1"#,
                ),
                &user(1, "delegated task"),
                &assistant(2, "done"),
            ],
        );
        let found = DshAdapter
            .discover_sessions_in(&[root], &|_| false)
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].agent_session_id, child);
        assert_eq!(
            found[0].parent_agent_session_id.as_deref(),
            Some(parent),
            "the header names the parent — the link is a fact, not a guess"
        );
    }

    /// The decoded header is pi's header shape plus `createdAt`. Only the
    /// decoded header distinguishes them, so the bytes alone never do — and
    /// the adapter must not claim a file whose header is pi's.
    #[test]
    fn a_pi_header_inside_a_zstd_file_is_not_claimed() {
        let root = unique_dir("pi-shaped");
        let dir = root.join("proj").join("p1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("session.v2.jsonl.zstd"),
            frame(r#"{"type":"session","version":3,"id":"p1","cwd":"/x"}"#),
        )
        .unwrap();
        let found = DshAdapter
            .discover_sessions_in(&[root], &|_| false)
            .unwrap();
        assert!(found.is_empty(), "{found:#?}");
    }

    /// A torn final frame (dsh mid-write) must not cost the complete prefix:
    /// the frames that finished are ingested, the partial one waits.
    #[test]
    fn a_torn_final_frame_leaves_the_complete_prefix_intact() {
        let id = "session-torn";
        let dir = session_dir(
            &unique_dir("torn"),
            id,
            "session.v2.jsonl.zstd",
            &[
                &header(id, ""),
                &user(3, "first prompt"),
                &assistant(4, "first reply"),
            ],
        );
        let file = dir.join("session.v2.jsonl.zstd");

        // Append half of the next frame: the bytes on disk are not a frame.
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&file)
                .unwrap();
            let partial = frame(&format!("{}\n", user(5, "half written")));
            f.write_all(&partial[..partial.len() / 2]).unwrap();
        }

        let session = Session {
            id: "sess-torn".into(),
            agent: Agent::Dsh,
            agent_session_id: id.into(),
            title: None,
            cwd: None,
            workspace_path_id: None,
            project_id: None,
            raw_path: file.to_string_lossy().to_string(),
            parent_agent_session_id: None,
            started_at: None,
            last_activity_at: None,
            trashed_at: None,
        };
        let delta = DshAdapter
            .read_delta(&session, &SourceCursor::default())
            .unwrap();
        let texts: Vec<&str> = delta
            .events
            .iter()
            .map(|e| e.text.as_deref().unwrap_or(""))
            .collect();
        assert_eq!(texts, vec!["first prompt", "first reply"], "{texts:?}");
    }
}
