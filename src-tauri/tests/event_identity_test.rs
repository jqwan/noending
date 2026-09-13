//! Event Identity & Source mutation tests (Issue #1).
//!
//! Fixture-driven: real temp JSONL files per agent adapter, exercising
//! append / no-change / truncate+rewrite / same-size rewrite / file
//! replacement, and verifying that:
//! - sequences are app-assigned, monotonic, never reused;
//! - history is never overwritten by later source states (append-only);
//! - re-scans of identical content never duplicate events;
//! - file replacement is recognized as a new generation.

use noending::adapters::AgentAdapter;
use noending::domain::{Agent, Session};
use noending::storage::{new_id, now, Db};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

fn unique_dir(tag: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "noending-identity-{}-{}-{}",
        tag,
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn open_db(tag: &str) -> Db {
    let dir = unique_dir(tag);
    Db::open(&dir.join("test.db")).unwrap()
}

fn session_row(db: &Db, agent: Agent, path: &std::path::Path) -> Session {
    let s = Session {
        id: new_id(),
        agent,
        agent_session_id: "fixture".into(),
        title: None,
        cwd: None,
        project_id: None,
        raw_path: path.to_string_lossy().to_string(),
        parent_agent_session_id: None,
        started_at: Some(now()),
        last_activity_at: Some(now()),
    };
    db.upsert_session(&s).unwrap();
    s
}

// ---- per-agent JSONL fixtures --------------------------------------------

fn codex_line(role: &str, text: &str) -> String {
    format!(
        r#"{{"type":"response_item","timestamp":"2026-09-13T10:00:00Z","payload":{{"type":"message","role":"{role}","content":[{{"type":"input_text","text":"{text}"}}]}}}}"#
    )
}

fn claude_line(role: &str, text: &str) -> String {
    // deterministic per-content uuid so rescans hash identically (dedup)
    let uuid = format!("u-{}-{}", role, text);
    format!(
        r#"{{"type":"{role}","uuid":"{uuid}","timestamp":"2026-09-13T10:00:00Z","message":{{"role":"{role}","content":[{{"type":"text","text":"{text}"}}]}}}}"#
    )
}

fn pi_line(role: &str, text: &str) -> String {
    format!(
        r#"{{"type":"message","timestamp":"2026-09-13T10:00:00Z","message":{{"role":"{role}","content":[{{"type":"text","text":"{text}"}}]}}}}"#
    )
}

/// One ingest round-trip exactly as production does it.
fn ingest(db: &Db, adapter: &dyn AgentAdapter, session: &Session) -> usize {
    let cursor = db.get_source_cursor(&session.id).unwrap();
    let delta = adapter.read_delta(session, &cursor).unwrap();
    let source = delta.source.clone().expect("source state");
    let stored = db
        .append_source_events(&session.id, &delta.events, &source, &session.raw_path)
        .unwrap();
    stored.len()
}

macro_rules! identity_suite {
    ($fn_name:ident, $agent:expr, $adapter:expr, $user_line:expr, $asst_line:expr) => {
        #[test]
        fn $fn_name() {
            let db = open_db(stringify!($fn_name));
            let dir = unique_dir(stringify!($fn_name));
            let file = dir.join("session.jsonl");
            let adapter: &dyn AgentAdapter = &$adapter;

            // ---- initial ingest: 3 lines ----
            std::fs::write(
                &file,
                format!(
                    "{}\n{}\n{}\n",
                    $user_line("user", "first user message about goals"),
                    $asst_line("assistant", "first assistant reply"),
                    $user_line("user", "second user message")
                ),
            )
            .unwrap();
            let s = session_row(&db, $agent, &file);

            assert_eq!(ingest(&db, adapter, &s), 3, "initial ingest stores all events");
            let all = db.get_events(&s.id, None, 100).unwrap();
            assert_eq!(all.len(), 3);
            assert_eq!(
                all.iter().map(|e| e.sequence).collect::<Vec<_>>(),
                vec![1, 2, 3],
                "sequences are app-assigned and dense on first ingest"
            );
            assert!(all.iter().all(|e| e.source_generation == 0));
            assert!(all.iter().all(|e| !e.id.is_empty()), "every event has a stable id");

            let c = db.get_source_cursor(&s.id).unwrap();
            assert_eq!(c.generation, 0);
            assert_eq!(c.byte_offset as usize, std::fs::metadata(&file).unwrap().len() as usize);
            assert!(!c.source_file_identity.is_empty());

            // ---- append: only the delta is stored ----
            std::thread::sleep(std::time::Duration::from_millis(20));
            let mut f = std::fs::OpenOptions::new().append(true).open(&file).unwrap();
            use std::io::Write;
            writeln!(f, "{}", $asst_line("assistant", "appended assistant reply")).unwrap();
            writeln!(f, "{}", $user_line("user", "appended user message")).unwrap();
            drop(f);

            assert_eq!(ingest(&db, adapter, &s), 2, "append stores only new lines");
            let all = db.get_events(&s.id, None, 100).unwrap();
            assert_eq!(all.len(), 5);
            assert_eq!(
                all.iter().map(|e| e.sequence).collect::<Vec<_>>(),
                vec![1, 2, 3, 4, 5],
                "sequence continues monotonically, line numbers are never reused as identity"
            );

            // ---- no change: re-read is a no-op ----
            assert_eq!(ingest(&db, adapter, &s), 0, "unchanged file adds nothing");

            // ---- truncate + rewrite with different content ----
            std::thread::sleep(std::time::Duration::from_millis(20));
            std::fs::write(
                &file,
                format!(
                    "{}\n{}\n",
                    $user_line("user", "compacted summary of everything"),
                    $asst_line("assistant", "post-compact state")
                ),
            )
            .unwrap();

            assert_eq!(ingest(&db, adapter, &s), 2, "compacted content is new events");
            let all = db.get_events(&s.id, None, 100).unwrap();
            assert_eq!(all.len(), 7, "old history is preserved, never overwritten");
            assert_eq!(
                all.iter().map(|e| e.sequence).collect::<Vec<_>>(),
                vec![1, 2, 3, 4, 5, 6, 7],
                "sequences keep increasing after truncate; no reuse"
            );
            assert_eq!(all.iter().filter(|e| e.source_generation == 1).count(), 2);

            // ---- rescan of identical content dedups ----
            assert_eq!(ingest(&db, adapter, &s), 0, "identical rescan never duplicates");

            // ---- file replacement (new identity) ----
            std::thread::sleep(std::time::Duration::from_millis(20));
            std::fs::remove_file(&file).unwrap();
            std::fs::write(
                &file,
                format!("{}\n", $user_line("user", "brand new session file content")),
            )
            .unwrap();
            assert_eq!(ingest(&db, adapter, &s), 1, "replaced file is ingested fresh");
            let c = db.get_source_cursor(&s.id).unwrap();
            assert_eq!(c.generation, 2, "file replacement bumps the generation");
            let all = db.get_events(&s.id, None, 100).unwrap();
            assert_eq!(all.len(), 8);
        }
    };
}

identity_suite!(codex_truncate_rewrite_dedup, Agent::Codex, noending::adapters::codex::CodexAdapter, codex_line, codex_line);
identity_suite!(claude_truncate_rewrite_dedup, Agent::ClaudeCode, noending::adapters::claude::ClaudeAdapter, claude_line, claude_line);
identity_suite!(pi_truncate_rewrite_dedup, Agent::Pi, noending::adapters::pi::PiAdapter, pi_line, pi_line);

/// Same-size rewrite (size unchanged, mtime changed) must be detected as a
/// rewrite: generation bump + rescan, old history intact, new text stored.
#[test]
fn same_size_rewrite_is_detected() {
    let db = open_db("same-size");
    let dir = unique_dir("same-size");
    let file = dir.join("s.jsonl");
    let adapter: &dyn AgentAdapter = &noending::adapters::pi::PiAdapter;

    let before = pi_line("user", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    let after = pi_line("user", "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    assert_eq!(before.len(), after.len(), "fixture lines must be byte-equal in length");

    std::fs::write(&file, format!("{}\n", before)).unwrap();
    let s = session_row(&db, Agent::Pi, &file);
    assert_eq!(ingest(&db, adapter, &s), 1);

    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(&file, format!("{}\n", after)).unwrap();

    assert_eq!(ingest(&db, adapter, &s), 1, "same-size rewrite adds the new text");
    let all = db.get_events(&s.id, None, 100).unwrap();
    assert_eq!(all.len(), 2, "history preserved");
    assert_eq!(all[1].source_generation, 1);
    let c = db.get_source_cursor(&s.id).unwrap();
    assert_eq!(c.generation, 1);
}

/// The unique content-identity index is the hard guarantee: inserting the
/// same content twice can never produce two rows.
#[test]
fn duplicate_content_cannot_be_inserted_twice() {
    let db = open_db("dup-insert");
    let dir = unique_dir("dup-insert");
    let s = session_row(&db, Agent::Codex, &dir.join("x.jsonl"));

    let mk = |pos: &str| noending::domain::ParsedEvent {
        source_event_id: None,
        source_position: pos.into(),
        ts: Some("t".into()),
        kind: "user_message".into(),
        text: Some("same text".into()),
        metadata: serde_json::json!({}),
    };

    let source = noending::domain::SourceCursorUpdate {
        file_identity: "unix:dev:1:ino:2".into(),
        generation: 0,
        byte_offset: 10,
        last_seen_size: 10,
        mtime: None,
    };
    let first = db
        .append_source_events(&s.id, &[mk("line:1")], &source, "/x.jsonl")
        .unwrap();
    assert_eq!(first.len(), 1);

    // same content at a DIFFERENT position / generation (e.g. after compaction)
    let source2 = noending::domain::SourceCursorUpdate {
        file_identity: source.file_identity.clone(),
        generation: 1,
        byte_offset: 10,
        last_seen_size: 10,
        mtime: None,
    };
    let second = db
        .append_source_events(&s.id, &[mk("line:9")], &source2, "/x.jsonl")
        .unwrap();
    assert_eq!(second.len(), 0, "same content in a later generation dedups");

    // genuinely new content still inserts
    let third = db
        .append_source_events(
            &s.id,
            &[noending::domain::ParsedEvent {
                source_event_id: None,
                source_position: "line:9".into(),
                ts: Some("t2".into()),
                kind: "user_message".into(),
                text: Some("different text".into()),
                metadata: serde_json::json!({}),
            }],
            &source2,
            "/x.jsonl",
        )
        .unwrap();
    assert_eq!(third.len(), 1);
}

/// Partial trailing line (agent mid-write, no trailing newline yet) is not
/// ingested until it is complete — and byte_offset reflects that.
#[test]
fn partial_trailing_line_is_not_ingested() {
    let db = open_db("partial-line");
    let dir = unique_dir("partial-line");
    let file = dir.join("s.jsonl");
    let adapter: &dyn AgentAdapter = &noending::adapters::pi::PiAdapter;

    let complete = pi_line("user", "a complete line");
    let partial = r#"{"type":"message","mess"#; // mid-write
    std::fs::write(&file, format!("{}\n{}", complete, partial)).unwrap();

    let s = session_row(&db, Agent::Pi, &file);
    assert_eq!(ingest(&db, adapter, &s), 1, "only the complete line");
    let c = db.get_source_cursor(&s.id).unwrap();
    assert_eq!(
        c.byte_offset as usize,
        complete.len() + 1,
        "cursor stops after the last complete line"
    );

    // finish the line
    std::thread::sleep(std::time::Duration::from_millis(20));
    let mut f = std::fs::OpenOptions::new().append(true).open(&file).unwrap();
    use std::io::Write;
    let rest = format!(
        "age\":{{\"role\":\"user\",\"content\":[{{\"type\":\"text\",\"text\":\"now complete\"}}]}},\"timestamp\":\"2026-09-13T10:00:01Z\"}}\n"
    );
    write!(f, "{}", rest).unwrap();
    drop(f);
    assert_eq!(ingest(&db, adapter, &s), 1, "completed line is picked up");
}
