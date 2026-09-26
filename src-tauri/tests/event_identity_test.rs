//! Message Identity & Source mutation tests (Issue #1) — Logical Session model.
//!
//! Fixture-driven: real temp JSONL files per agent adapter, exercising
//! append / no-change / truncate+rewrite / same-size rewrite / file
//! replacement, and verifying that:
//! - sequences are app-assigned, monotonic, never reused;
//! - history is never overwritten by later source states (append-only);
//! - re-scans of identical content never duplicate messages;
//! - file replacement is recognized as a new generation.
//!
//! Identity now lives on the ROOT MEMBER: reads commit through
//! `Db::commit_member_ingest`, which chains `message_identity_hash` from
//! IDENTITY_GENESIS on a full re-scan (`start_byte_offset == 0`) and from the
//! member cursor's `identity_tail_hash` on an append. The cursor — not the
//! session — owns the read position (§8.1).

use noending::adapters::AgentAdapter;
use noending::domain::{
    Agent, MemberStatsDelta, ParsedSessionMessage, Session, SessionMemberRelation,
    SessionMessageRole, SourceCursorUpdate, StatsUpdate,
};
use noending::storage::{new_id, Db};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

mod support;

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

// ---- per-source write strategies -----------------------------------------
//
// Plain JSONL is written as text; dsh's transcript is zstd, appended one
// complete frame per write batch (方案 §37.8). Both must pass the same suite.

fn write_plain(file: &std::path::Path, body: &str) {
    std::fs::write(file, body).unwrap();
}

fn append_plain(file: &std::path::Path, extra: &str) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().append(true).open(file).unwrap();
    write!(f, "{extra}").unwrap();
}

fn frame(body: &str) -> Vec<u8> {
    zstd::encode_all(body.as_bytes(), 3).unwrap()
}

fn write_framed(file: &std::path::Path, body: &str) {
    std::fs::write(file, frame(body)).unwrap();
}

fn append_framed(file: &std::path::Path, extra: &str) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().append(true).open(file).unwrap();
    f.write_all(&frame(extra)).unwrap();
}

/// A Logical Session + its ROOT member, pointed at a fixture transcript file.
/// The member identity is the session's root Resume identity, exactly as
/// discovery creates it (§10.1).
fn fixture_session(
    db: &Db,
    agent: Agent,
    root_agent_session_id: &str,
    path: &std::path::Path,
) -> (Session, String) {
    let s = support::ensure_session(db, new_id(), agent, root_agent_session_id);
    let member_id = support::ensure_root_member(
        db,
        &s.id,
        agent,
        root_agent_session_id,
        &path.to_string_lossy(),
    );
    (s, member_id)
}

/// A parsed root-conversation message with a stable synthetic id, so a rescan
/// of the same logical line hashes identically (dedup).
fn msg(
    source_message_id: impl Into<String>,
    role: SessionMessageRole,
    content: impl Into<String>,
) -> ParsedSessionMessage {
    ParsedSessionMessage {
        source_message_id: Some(source_message_id.into()),
        source_position: String::new(),
        ts: Some("2026-09-13T10:00:00Z".into()),
        role,
        content: content.into(),
    }
}

/// A full re-scan source state (`start_byte_offset == 0` → the identity chain
/// restarts from genesis).
fn rescan(gen: i64) -> SourceCursorUpdate {
    SourceCursorUpdate {
        file_identity: "unix:dev:1:ino:2".into(),
        generation: gen,
        byte_offset: 10,
        last_seen_size: 10,
        mtime: None,
        start_byte_offset: 0,
        prefix_hash: String::new(),
    }
}

/// An append source state (`start_byte_offset > 0` → the chain continues from
/// the cursor's identity tail).
fn append(gen: i64) -> SourceCursorUpdate {
    SourceCursorUpdate {
        file_identity: "unix:dev:1:ino:2".into(),
        generation: gen,
        byte_offset: 20,
        last_seen_size: 20,
        mtime: None,
        start_byte_offset: 10,
        prefix_hash: String::new(),
    }
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

fn workbuddy_line(role: &str, text: &str) -> String {
    // WorkBuddy: append-only JSONL with no session header; the content block
    // is typed by role (`input_text` / `output_text`) and the timestamp is
    // epoch millis. The deterministic id keeps rescans hash-identical.
    let id = format!("wb-{}-{}", role, text);
    let block = if role == "assistant" {
        "output_text"
    } else {
        "input_text"
    };
    format!(
        r#"{{"id":"{id}","timestamp":1783137449113,"type":"message","role":"{role}","content":[{{"type":"{block}","text":"{text}"}}],"sessionId":"s1","cwd":"/repo"}}"#
    )
}

fn qoder_line(role: &str, text: &str) -> String {
    // Qoder = Claude's shape + a sessionId; the deterministic uuid keeps
    // rescans hash-identical so dedup can be observed.
    let uuid = format!("u-{}-{}", role, text);
    format!(
        r#"{{"type":"{role}","uuid":"{uuid}","timestamp":"2026-09-13T10:00:00Z","sessionId":"qs","cwd":"/repo","message":{{"role":"{role}","content":[{{"type":"text","text":"{text}"}}]}}}}"#
    )
}

fn dsh_line(role: &str, text: &str) -> String {
    // dsh records the writer's own contiguous `seq`; a per-line counter keeps
    // the fixture faithful (ids are unique and never reused) so the suite
    // exercises the id-based dedup production relies on.
    static SEQ: AtomicU64 = AtomicU64::new(1);
    let seq = SEQ.fetch_add(1, Ordering::SeqCst);
    let (vtype, data) = if role == "user" {
        (
            "user/message",
            format!(r#"{{"content":[{{"type":"text","text":"{text}"}}],"role":"user"}}"#),
        )
    } else {
        (
            "assistant/message",
            format!(
                r#"{{"turn":1,"step":1,"message":{{"role":"assistant","content":[{{"type":"text","text":"{text}"}}]}}}}"#
            ),
        )
    };
    format!(r#"{{"type":"{vtype}","seq":{seq},"time":1783137449113,"data":{data}}}"#)
}

/// One ingest round-trip exactly as production does it: read the MEMBER's
/// delta from its own source, commit atomically through the one ingest path.
fn ingest(db: &Db, adapter: &dyn AgentAdapter, session: &Session, member_id: &str) -> usize {
    let member = db.get_member(member_id).unwrap().expect("member row");
    let cursor = db.get_member_cursor(member_id).unwrap();
    let delta = adapter.read_member_delta(&member, &cursor).unwrap();
    let source = delta.source.clone().expect("source state");
    let stored = db
        .commit_member_ingest(
            &session.id,
            member_id,
            &delta.messages,
            delta.stats,
            &source,
        )
        .unwrap();
    stored.len()
}

macro_rules! identity_suite {
    ($fn_name:ident, $agent:expr, $adapter:expr, $user_line:expr, $asst_line:expr, $write:expr, $append:expr) => {
        #[test]
        fn $fn_name() {
            let db = open_db(stringify!($fn_name));
            let dir = unique_dir(stringify!($fn_name));
            let file = dir.join("session.jsonl");
            let adapter: &dyn AgentAdapter = &$adapter;
            let write: fn(&std::path::Path, &str) = $write;
            let append: fn(&std::path::Path, &str) = $append;

            // ---- initial ingest: 3 lines ----
            write(
                &file,
                &format!(
                    "{}\n{}\n{}\n",
                    $user_line("user", "first user message about goals"),
                    $asst_line("assistant", "first assistant reply"),
                    $user_line("user", "second user message")
                ),
            );
            let (s, member_id) = fixture_session(&db, $agent, stringify!($fn_name), &file);

            assert_eq!(
                ingest(&db, adapter, &s, &member_id),
                3,
                "initial ingest stores all messages"
            );
            let all = db.get_messages(&s.id, None, 100).unwrap();
            assert_eq!(all.len(), 3);
            assert_eq!(
                all.iter().map(|e| e.sequence).collect::<Vec<_>>(),
                vec![1, 2, 3],
                "sequences are app-assigned and dense on first ingest"
            );
            assert!(all.iter().all(|e| e.source_generation == 0));
            assert!(
                all.iter().all(|e| !e.id.is_empty()),
                "every message has a stable id"
            );
            assert!(
                all.iter().all(|e| e.member_id == member_id),
                "conversation rows belong to the ROOT member"
            );

            let c = db.get_member_cursor(&member_id).unwrap();
            assert_eq!(c.generation, 0);
            assert_eq!(
                c.byte_offset as usize,
                std::fs::metadata(&file).unwrap().len() as usize
            );
            assert!(!c.source_file_identity.is_empty());
            assert!(
                !c.identity_tail_hash.is_empty(),
                "the cursor tracks the source identity chain tail"
            );

            // ---- append: only the delta is stored ----
            std::thread::sleep(std::time::Duration::from_millis(20));
            append(
                &file,
                &format!(
                    "{}\n{}\n",
                    $asst_line("assistant", "appended assistant reply"),
                    $user_line("user", "appended user message")
                ),
            );

            assert_eq!(
                ingest(&db, adapter, &s, &member_id),
                2,
                "append stores only new lines"
            );
            let all = db.get_messages(&s.id, None, 100).unwrap();
            assert_eq!(all.len(), 5);
            assert_eq!(
                all.iter().map(|e| e.sequence).collect::<Vec<_>>(),
                vec![1, 2, 3, 4, 5],
                "sequence continues monotonically, line numbers are never reused as identity"
            );

            // ---- no change: re-read is a no-op ----
            assert_eq!(
                ingest(&db, adapter, &s, &member_id),
                0,
                "unchanged file adds nothing"
            );

            // ---- truncate + rewrite with different content ----
            std::thread::sleep(std::time::Duration::from_millis(20));
            write(
                &file,
                &format!(
                    "{}\n{}\n",
                    $user_line("user", "compacted summary of everything"),
                    $asst_line("assistant", "post-compact state")
                ),
            );

            assert_eq!(
                ingest(&db, adapter, &s, &member_id),
                2,
                "compacted content is new messages"
            );
            let all = db.get_messages(&s.id, None, 100).unwrap();
            assert_eq!(all.len(), 7, "old history is preserved, never overwritten");
            assert_eq!(
                all.iter().map(|e| e.sequence).collect::<Vec<_>>(),
                vec![1, 2, 3, 4, 5, 6, 7],
                "sequences keep increasing after truncate; no reuse"
            );
            assert_eq!(all.iter().filter(|e| e.source_generation == 1).count(), 2);

            // ---- rescan of identical content dedups ----
            assert_eq!(
                ingest(&db, adapter, &s, &member_id),
                0,
                "identical rescan never duplicates"
            );

            // ---- file replacement (new identity) ----
            std::thread::sleep(std::time::Duration::from_millis(20));
            std::fs::remove_file(&file).unwrap();
            write(
                &file,
                &format!("{}\n", $user_line("user", "brand new session file content")),
            );
            assert_eq!(
                ingest(&db, adapter, &s, &member_id),
                1,
                "replaced file is ingested fresh"
            );
            let c = db.get_member_cursor(&member_id).unwrap();
            assert_eq!(c.generation, 2, "file replacement bumps the generation");
            let all = db.get_messages(&s.id, None, 100).unwrap();
            assert_eq!(all.len(), 8);
        }
    };
}

identity_suite!(
    codex_truncate_rewrite_dedup,
    Agent::Codex,
    noending::adapters::codex::CodexAdapter,
    codex_line,
    codex_line,
    write_plain,
    append_plain
);
identity_suite!(
    claude_truncate_rewrite_dedup,
    Agent::ClaudeCode,
    noending::adapters::claude::ClaudeAdapter,
    claude_line,
    claude_line,
    write_plain,
    append_plain
);
identity_suite!(
    pi_truncate_rewrite_dedup,
    Agent::Pi,
    noending::adapters::pi::PiAdapter,
    pi_line,
    pi_line,
    write_plain,
    append_plain
);
identity_suite!(
    qoder_truncate_rewrite_dedup,
    Agent::Qoder,
    noending::adapters::qoder::QoderAdapter,
    qoder_line,
    qoder_line,
    write_plain,
    append_plain
);
identity_suite!(
    workbuddy_truncate_rewrite_dedup,
    Agent::WorkBuddy,
    noending::adapters::workbuddy::WorkBuddyAdapter,
    workbuddy_line,
    workbuddy_line,
    write_plain,
    append_plain
);
identity_suite!(
    dsh_truncate_rewrite_dedup,
    Agent::Dsh,
    noending::adapters::dsh::DshAdapter,
    dsh_line,
    dsh_line,
    write_framed,
    append_framed
);
// ZCode has no entry: its source is a live SQLite store, and this suite works
// by mutating a transcript file under the adapter's own reader. Its cursor and
// identity behaviour is covered by the adapter's unit tests instead (方案
// §37.10).

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
    assert_eq!(
        before.len(),
        after.len(),
        "fixture lines must be byte-equal in length"
    );

    std::fs::write(&file, format!("{}\n", before)).unwrap();
    let (s, member_id) = fixture_session(&db, Agent::Pi, "same-size-root", &file);
    assert_eq!(ingest(&db, adapter, &s, &member_id), 1);

    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(&file, format!("{}\n", after)).unwrap();

    assert_eq!(
        ingest(&db, adapter, &s, &member_id),
        1,
        "same-size rewrite adds the new text"
    );
    let all = db.get_messages(&s.id, None, 100).unwrap();
    assert_eq!(all.len(), 2, "history preserved");
    assert_eq!(all[1].source_generation, 1);
    let c = db.get_member_cursor(&member_id).unwrap();
    assert_eq!(c.generation, 1);
}

/// Message identity semantics (chained adjacency for id-less messages):
/// - identical re-scan from the same chain position → dedup;
/// - the same content at a DIFFERENT chain position (a genuinely repeated
///   user message like "继续") stays distinct;
/// - native agent message ids dedup regardless of position;
/// - genuinely new content always inserts.
#[test]
fn identity_dedup_follows_chain_semantics() {
    let db = open_db("chain-identity");
    let dir = unique_dir("chain-identity");
    let (s, member_id) = fixture_session(
        &db,
        Agent::Codex,
        "chain-identity-root",
        &dir.join("x.jsonl"),
    );

    let mk = |pos: &str, text: &str| ParsedSessionMessage {
        source_message_id: None,
        source_position: pos.into(),
        ts: Some("t".into()),
        role: SessionMessageRole::User,
        content: text.into(),
    };
    let mk_native = |pos: &str, id: &str| ParsedSessionMessage {
        source_message_id: Some(id.into()),
        source_position: pos.into(),
        ts: Some("t".into()),
        role: SessionMessageRole::User,
        content: "same text".into(),
    };

    // first sight of the content
    let first = db
        .commit_member_ingest(
            &s.id,
            &member_id,
            &[mk("line:1", "same text")],
            None,
            &rescan(0),
        )
        .unwrap();
    assert_eq!(first.len(), 1);

    // identical re-scan (same chain position, later generation) → dedup
    let again = db
        .commit_member_ingest(
            &s.id,
            &member_id,
            &[mk("line:9", "same text")],
            None,
            &rescan(1),
        )
        .unwrap();
    assert_eq!(again.len(), 0, "identical rescan never duplicates");

    // the SAME content appended after another event is a DIFFERENT logical
    // message ("继续" sent twice must not collapse): the append chains from
    // the cursor's identity tail, so the second occurrence links to a
    // different predecessor.
    let second_msg = db
        .commit_member_ingest(
            &s.id,
            &member_id,
            &[mk("line:2", "same text")],
            None,
            &append(0),
        )
        .unwrap();
    assert_eq!(second_msg.len(), 1, "repeated user message stays distinct");

    // native agent message ids dedup regardless of chain position
    let native1 = db
        .commit_member_ingest(
            &s.id,
            &member_id,
            &[mk_native("line:3", "native-1")],
            None,
            &append(0),
        )
        .unwrap();
    assert_eq!(native1.len(), 1);
    let native2 = db
        .commit_member_ingest(
            &s.id,
            &member_id,
            &[mk_native("line:4", "native-1")],
            None,
            &append(0),
        )
        .unwrap();
    assert_eq!(native2.len(), 0, "native id dedups across positions");

    // genuinely new content still inserts
    let third = db
        .commit_member_ingest(
            &s.id,
            &member_id,
            &[mk("line:5", "different text")],
            None,
            &append(0),
        )
        .unwrap();
    assert_eq!(third.len(), 1);
}

/// A stats-only batch (no messages) advances the member cursor's read
/// position but must NOT move the identity tail: only messages advance the
/// chain, so the next append keeps chaining from the last real message (§8.1).
#[test]
fn stats_only_batch_advances_cursor_but_keeps_identity_tail() {
    let db = open_db("stats-only-tail");
    let dir = unique_dir("stats-only-tail");
    let (s, member_id) =
        fixture_session(&db, Agent::Codex, "stats-only-root", &dir.join("x.jsonl"));

    let first = db
        .commit_member_ingest(
            &s.id,
            &member_id,
            &[msg("m-1", SessionMessageRole::User, "hello")],
            None,
            &rescan(0),
        )
        .unwrap();
    assert_eq!(first.len(), 1);
    let tail = db.get_member_cursor(&member_id).unwrap().identity_tail_hash;
    assert!(!tail.is_empty(), "a message batch sets the identity tail");

    // stats-only batch: the read moved, no message was parsed
    let advanced = SourceCursorUpdate {
        file_identity: "unix:dev:1:ino:2".into(),
        generation: 0,
        byte_offset: 200,
        last_seen_size: 200,
        mtime: None,
        start_byte_offset: 100,
        prefix_hash: "prefix".into(),
    };
    let stored = db
        .commit_member_ingest(
            &s.id,
            &member_id,
            &[],
            Some(StatsUpdate::Delta(MemberStatsDelta {
                tool_call_count: Some(2),
                ..Default::default()
            })),
            &advanced,
        )
        .unwrap();
    assert!(stored.is_empty(), "a stats-only batch stores no messages");

    let c = db.get_member_cursor(&member_id).unwrap();
    assert_eq!(c.byte_offset, 200, "the read position moved");
    assert_eq!(
        c.identity_tail_hash, tail,
        "stats-only batches keep the identity tail"
    );
    let stats = db.get_member_stats(&member_id).unwrap().expect("stats row");
    assert_eq!(stats.tool_call_count, Some(2));
    assert_eq!(db.message_count(&s.id).unwrap(), 1);

    // the next real append still chains from the preserved tail: an id-less
    // message appended after the stats batch links to the recorded tail as
    // its predecessor, landing as a genuinely new chain position.
    let appended = db
        .commit_member_ingest(
            &s.id,
            &member_id,
            &[ParsedSessionMessage {
                source_message_id: None,
                source_position: String::new(),
                ts: Some("2026-09-13T10:00:00Z".into()),
                role: SessionMessageRole::User,
                content: "hello".into(),
            }],
            None,
            &append(0),
        )
        .unwrap();
    assert_eq!(appended.len(), 1, "an append is a new chain position");
    assert_ne!(
        db.get_member_cursor(&member_id).unwrap().identity_tail_hash,
        tail,
        "the tail advanced to the appended message"
    );
}

/// Only the ROOT member may produce Conversation rows: an adapter handing
/// child text to the conversation is a bug, and the commit must reject the
/// whole batch — nothing stored, nothing moved (§6 / §13.3).
#[test]
fn child_member_messages_are_rejected_not_stored() {
    let db = open_db("child-guard");
    let dir = unique_dir("child-guard");
    let (s, _root_member) =
        fixture_session(&db, Agent::Codex, "child-guard-root", &dir.join("x.jsonl"));
    let child_id = db
        .upsert_session_member(
            &s.id,
            Agent::Codex,
            "child-guard-root:subagent:1",
            SessionMemberRelation::Child,
            Some("child-guard-root"),
            "test_child",
            "/tmp/child.jsonl",
            None,
            None,
            None,
            &serde_json::json!({}),
        )
        .unwrap();

    let attempt = db.commit_member_ingest(
        &s.id,
        &child_id,
        &[msg(
            "c-1",
            SessionMessageRole::User,
            "child prose that must not land",
        )],
        None,
        &rescan(0),
    );
    assert!(
        attempt.is_err(),
        "child-with-messages must be rejected at commit"
    );
    assert_eq!(
        db.message_count(&s.id).unwrap(),
        0,
        "the rejected batch stored nothing"
    );

    // observations without messages stay legal for a child (stats surface §7)
    let observations = db.commit_member_ingest(
        &s.id,
        &child_id,
        &[],
        Some(StatsUpdate::Delta(MemberStatsDelta {
            tool_call_count: Some(1),
            ..Default::default()
        })),
        &rescan(0),
    );
    assert!(
        observations.is_ok(),
        "a stats-only child batch is a legal observation"
    );
}

/// The conversation store is append-only across truncation, and the Context
/// frontier is a separate lifecycle: a source truncation (compaction) never
/// deletes old messages, never reuses sequences, and never moves the
/// processed frontier that Sync consumed ("conversation ends, context
/// doesn't", §8.2).
#[test]
fn truncate_preserves_history_and_leaves_the_context_frontier_alone() {
    let db = open_db("truncate-frontier");
    let dir = unique_dir("truncate-frontier");
    let (s, member_id) = fixture_session(
        &db,
        Agent::Codex,
        "truncate-frontier-root",
        &dir.join("x.jsonl"),
    );

    let first = db
        .commit_member_ingest(
            &s.id,
            &member_id,
            &[
                msg("t-1", SessionMessageRole::User, "decision one"),
                msg("t-2", SessionMessageRole::Assistant, "reply one"),
                msg("t-3", SessionMessageRole::User, "decision two"),
            ],
            None,
            &rescan(0),
        )
        .unwrap();
    assert_eq!(first.len(), 3);

    // Sync consumed the first message
    db.set_processed_message_sequence(&s.id, 1).unwrap();

    // the source truncates to a compaction summary: a full re-scan at a new
    // generation appends the summary, dedups nothing, deletes nothing
    let compacted = db
        .commit_member_ingest(
            &s.id,
            &member_id,
            &[msg(
                "t-4",
                SessionMessageRole::User,
                "compacted summary of it all",
            )],
            None,
            &rescan(1),
        )
        .unwrap();
    assert_eq!(compacted.len(), 1, "the summary is new evidence");

    let all = db.get_messages(&s.id, None, 100).unwrap();
    assert_eq!(
        all.iter().map(|m| m.sequence).collect::<Vec<_>>(),
        vec![1, 2, 3, 4],
        "sequences keep increasing across a truncate; none reused"
    );
    let ids_before: Vec<String> = first.iter().map(|m| m.id.clone()).collect();
    let ids_after: Vec<String> = all[..3].iter().map(|m| m.id.clone()).collect();
    assert_eq!(
        ids_after, ids_before,
        "pre-truncation message ids survive untouched"
    );
    assert_eq!(
        db.get_context_state(&s.id)
            .unwrap()
            .processed_message_sequence,
        1,
        "member cursors and the Context frontier are separate lifecycles"
    );
}

/// After a compact + dedup re-scan, the identity chain must continue from
/// the CURRENT source tail (tracked on the member cursor), not from the
/// message store's last row: the store keeps newer history the source no
/// longer has (append-only), and chaining an append from the store tail would
/// make that append invisible to the next full re-scan (duplicated as "new").
#[test]
fn append_after_compact_chains_from_source_tail_not_store_tail() {
    let db = open_db("compact-tail");
    let dir = unique_dir("compact-tail");
    let file = dir.join("s.jsonl");
    let adapter: &dyn AgentAdapter = &noending::adapters::pi::PiAdapter;

    let text = |c: char| {
        format!(
            "event {c} {}{}{}{}{}{}{}{}{}{}",
            c, c, c, c, c, c, c, c, c, c
        )
    };
    std::fs::write(
        &file,
        format!(
            "{}\n{}\n{}\n{}\n",
            pi_line("user", &text('A')),
            pi_line("user", &text('B')),
            pi_line("user", &text('C')),
            pi_line("user", &text('D'))
        ),
    )
    .unwrap();
    let (s, member_id) = fixture_session(&db, Agent::Pi, "compact-tail-root", &file);
    assert_eq!(ingest(&db, adapter, &s, &member_id), 4);
    let store_tail_before = db.get_member_cursor(&member_id).unwrap().identity_tail_hash;
    assert!(
        !store_tail_before.is_empty(),
        "cursor tracks the source chain tail"
    );

    // the source is compacted to A,B: the rescan dedups to zero new rows,
    // but the cursor tail must move BACK to B's identity hash
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(
        &file,
        format!(
            "{}\n{}\n",
            pi_line("user", &text('A')),
            pi_line("user", &text('B'))
        ),
    )
    .unwrap();
    assert_eq!(
        ingest(&db, adapter, &s, &member_id),
        0,
        "compacted prefix dedups"
    );

    let hash_of = |t: &str| -> String {
        db.read()
            .query_row(
                "SELECT source_identity_hash FROM session_messages
                 WHERE session_id = ?1 AND content = ?2",
                rusqlite::params![s.id, t],
                |r| r.get(0),
            )
            .unwrap()
    };
    let tail = db.get_member_cursor(&member_id).unwrap().identity_tail_hash;
    assert_ne!(
        tail, store_tail_before,
        "tail follows the source, not the store"
    );
    assert_eq!(
        tail,
        hash_of(&text('B')),
        "tail == B's chain hash, though the store still ends at D"
    );

    // append E: must chain from B (the source tail), not from D
    std::thread::sleep(std::time::Duration::from_millis(20));
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap();
    use std::io::Write;
    writeln!(f, "{}", pi_line("user", &text('E'))).unwrap();
    drop(f);
    assert_eq!(ingest(&db, adapter, &s, &member_id), 1, "E is new");
    let e_hash = hash_of(&text('E'));

    // full re-scan of A,B,E: E must dedup — it did NOT chain from the
    // stale store tail (the bug would duplicate it here)
    db.reset_member_cursors(&s.id).unwrap();
    assert_eq!(
        ingest(&db, adapter, &s, &member_id),
        0,
        "E survives a full re-scan without duplication"
    );
    assert_eq!(db.get_messages(&s.id, None, 100).unwrap().len(), 5);
    assert_eq!(
        db.get_member_cursor(&member_id).unwrap().identity_tail_hash,
        e_hash
    );
}

/// Re-ingest must NEVER delete the message store: ids survive,
/// SourceReferences in context revisions stay valid, and a full re-scan
/// dedups to zero new rows.
#[test]
fn reingest_preserves_message_ids_and_dedups() {
    let db = open_db("reingest-keep");
    let dir = unique_dir("reingest-keep");
    let file = dir.join("s.jsonl");
    let adapter: &dyn AgentAdapter = &noending::adapters::pi::PiAdapter;

    std::fs::write(
        &file,
        format!(
            "{}\n{}\n",
            pi_line("user", "persisted message one aaaaaaaaaaaaaaaaaaaa"),
            pi_line("assistant", "persisted message two bbbbbbbbbbbbbbbbbbb")
        ),
    )
    .unwrap();
    let (s, member_id) = fixture_session(&db, Agent::Pi, "reingest-root", &file);
    assert_eq!(ingest(&db, adapter, &s, &member_id), 2);
    let ids_before: Vec<String> = db
        .get_messages(&s.id, None, 100)
        .unwrap()
        .into_iter()
        .map(|e| e.id)
        .collect();

    // simulate 重新入库: rewind the member cursors, keep the message store
    db.reset_member_cursors(&s.id).unwrap();
    assert_eq!(
        ingest(&db, adapter, &s, &member_id),
        0,
        "re-scan of unchanged source adds nothing"
    );

    let all = db.get_messages(&s.id, None, 100).unwrap();
    let ids_after: Vec<String> = all.iter().map(|e| e.id.clone()).collect();
    assert_eq!(
        ids_after, ids_before,
        "message ids (and SourceReferences) survive re-ingest"
    );
    assert_eq!(
        all.iter().map(|e| e.sequence).collect::<Vec<_>>(),
        vec![1, 2]
    );

    // new source content after the rewind still appends normally
    std::thread::sleep(std::time::Duration::from_millis(20));
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap();
    use std::io::Write;
    writeln!(
        f,
        "{}",
        pi_line("user", "fresh message after reingest ccccccccccccccc")
    )
    .unwrap();
    drop(f);
    assert_eq!(
        ingest(&db, adapter, &s, &member_id),
        1,
        "new content appends after the re-scan"
    );
    assert_eq!(db.get_messages(&s.id, None, 100).unwrap().len(), 3);
}

/// A commit racing a Trash stores NOTHING (§13.1): the trashed session takes
/// no messages, no stats and no cursor move, so a Restore resumes from the
/// untouched cursor.
#[test]
fn trashed_session_commit_stores_nothing() {
    let db = open_db("trash-guard");
    let dir = unique_dir("trash-guard");
    let (s, member_id) =
        fixture_session(&db, Agent::Codex, "trash-guard-root", &dir.join("x.jsonl"));

    let first = db
        .commit_member_ingest(
            &s.id,
            &member_id,
            &[msg("g-1", SessionMessageRole::User, "before")],
            None,
            &rescan(0),
        )
        .unwrap();
    assert_eq!(first.len(), 1);
    let cursor_before = db.get_member_cursor(&member_id).unwrap();

    noending::lifecycle::trash_session(&db, &s.id).unwrap();

    let rejected = db
        .commit_member_ingest(
            &s.id,
            &member_id,
            &[msg("g-2", SessionMessageRole::User, "raced a trash")],
            None,
            &append(0),
        )
        .unwrap();
    assert!(rejected.is_empty(), "a trashed session takes nothing");
    assert_eq!(db.message_count(&s.id).unwrap(), 1);
    assert_eq!(
        db.get_member_cursor(&member_id).unwrap().byte_offset,
        cursor_before.byte_offset,
        "the cursor did not move"
    );
}

/// Every stored message is resolvable through its stable provenance ref
/// (`session-message:<id>`), the only spelling Context revisions may cite.
#[test]
fn message_by_ref_resolves_the_app_owned_identity() {
    let db = open_db("by-ref");
    let dir = unique_dir("by-ref");
    let (s, member_id) = fixture_session(&db, Agent::Codex, "by-ref-root", &dir.join("x.jsonl"));

    let stored = db
        .commit_member_ingest(
            &s.id,
            &member_id,
            &[msg("r-1", SessionMessageRole::User, "cited evidence")],
            None,
            &rescan(0),
        )
        .unwrap();
    assert_eq!(stored.len(), 1);

    let found = db
        .get_message_by_ref(&format!("session-message:{}", stored[0].id))
        .unwrap()
        .expect("ref resolves");
    assert_eq!(found.id, stored[0].id);
    assert_eq!(found.content, "cited evidence");
    assert_eq!(
        db.get_message_by_ref("session-message:missing")
            .unwrap()
            .is_none(),
        true
    );
}
