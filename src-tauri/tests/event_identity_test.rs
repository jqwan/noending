//! Message Identity & Source mutation tests — Logical Session model.
//!
//! Fixture-driven real JSONL per adapter: append / no-change / truncate+rewrite
//! / same-size rewrite / file replacement. Complete snapshots keep only the
//! current conversation, reusing unchanged message ids and removing retired
//! rows; a rewritten conversation is a new fact generation.
//!
//! Identity lives on the SESSION: `commit_ingest` chains
//! `message_identity_hash` from IDENTITY_GENESIS on a full re-scan, else from
//! the session's `source_tail_hash`. The session row — not a separate cursor
//! row — owns the read position.

use noending::adapters::AgentAdapter;
use noending::domain::{
    Agent, ParsedSessionMessage, Session, SessionMessageRole, SourceCursorUpdate,
};
use noending::storage::{new_id, Db};
use noending_lib as noending;
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

/// Inspect persisted rows independently of the current-message query so tests
/// detect obsolete messages that would otherwise remain invisible.
fn raw_rows(db: &Db, session_id: &str) -> Vec<(i64, String, i64)> {
    let conn = db.read();
    let mut stmt = conn
        .prepare(
            "SELECT sequence, content, source_generation FROM session_messages
             WHERE session_id = ?1 ORDER BY sequence",
        )
        .unwrap();
    stmt.query_map(rusqlite::params![session_id], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
    })
    .unwrap()
    .collect::<std::result::Result<Vec<_>, _>>()
    .unwrap()
}

// per-source write strategies
// Plain JSONL is written as text; dsh's transcript is zstd, appended one
// complete frame per write batch. Both must pass the same suite.

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

/// A Logical Session pointed at a fixture transcript file. The root Resume
/// identity is the source's identity, exactly as discovery creates it.
fn fixture_session(
    db: &Db,
    agent: Agent,
    root_agent_session_id: &str,
    path: &std::path::Path,
) -> Session {
    let s = support::ensure_session(db, new_id(), agent, root_agent_session_id);
    support::ensure_session_source(db, agent, root_agent_session_id, &path.to_string_lossy());
    db.get_session(&s.id).unwrap().expect("session row")
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

// per-agent JSONL fixtures

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

/// One ingest round-trip exactly as production does it: read the session's
/// delta from its own source, commit atomically through the one ingest path.
fn ingest(db: &Db, adapter: &dyn AgentAdapter, session: &Session) -> usize {
    ingest_mode(db, adapter, session, false)
}

fn ingest_mode(db: &Db, adapter: &dyn AgentAdapter, session: &Session, full: bool) -> usize {
    let mut cursor = db
        .get_session(&session.id)
        .unwrap()
        .expect("session row")
        .source_cursor();
    if full {
        cursor.byte_offset = 0;
        cursor.last_seen_size = 0;
        cursor.prefix_hash.clear();
    }
    let delta = adapter.read_session_delta(session, &cursor).unwrap();
    let Some(source) = delta.source else {
        assert!(delta.messages.is_empty());
        assert!(delta.complete_snapshot);
        return 0;
    };
    let stored = db
        .commit_ingest_snapshot(
            &session.id,
            &delta.messages,
            &source,
            delta.complete_snapshot,
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

            // initial ingest: 3 lines
            write(
                &file,
                &format!(
                    "{}\n{}\n{}\n",
                    $user_line("user", "first user message about goals"),
                    $asst_line("assistant", "first assistant reply"),
                    $user_line("user", "second user message")
                ),
            );
            let s = fixture_session(&db, $agent, stringify!($fn_name), &file);

            assert_eq!(
                ingest(&db, adapter, &s),
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
                all.iter().all(|e| e.session_id == s.id),
                "conversation rows belong to the session"
            );

            let c = db.get_session(&s.id).unwrap().unwrap().source_cursor();
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

            // append: only the delta is stored
            std::thread::sleep(std::time::Duration::from_millis(20));
            append(
                &file,
                &format!(
                    "{}\n{}\n",
                    $asst_line("assistant", "appended assistant reply"),
                    $user_line("user", "appended user message")
                ),
            );

            assert_eq!(ingest(&db, adapter, &s), 2, "append stores only new lines");
            let all = db.get_messages(&s.id, None, 100).unwrap();
            assert_eq!(all.len(), 5);
            assert_eq!(
                all.iter().map(|e| e.sequence).collect::<Vec<_>>(),
                vec![1, 2, 3, 4, 5],
                "sequence continues monotonically, line numbers are never reused as identity"
            );

            // no change: re-read is a no-op
            assert_eq!(ingest(&db, adapter, &s), 0, "unchanged file adds nothing");

            // truncate + rewrite with different content
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
                ingest(&db, adapter, &s),
                2,
                "compacted content is new messages"
            );
            // Only the complete replacement remains in the persisted store.
            let raw = raw_rows(&db, &s.id);
            assert_eq!(
                raw.len(),
                2,
                "retired history is removed after a complete snapshot"
            );
            assert_eq!(
                raw.iter().map(|(seq, _, _)| *seq).collect::<Vec<_>>(),
                vec![6, 7],
                "new messages keep the storage sequence assigned before retirement"
            );
            assert_eq!(raw.iter().filter(|(_, _, g)| *g == 1).count(), 2);
            // The CURRENT conversation is the rewritten source only.
            let conv = db.get_messages(&s.id, None, 100).unwrap();
            assert_eq!(
                conv.iter().map(|e| e.sequence).collect::<Vec<_>>(),
                vec![6, 7],
                "the effective conversation is the rewritten source"
            );

            // rescan of identical content dedups
            assert_eq!(
                ingest(&db, adapter, &s),
                0,
                "identical rescan never duplicates"
            );

            // file replacement (new identity)
            std::thread::sleep(std::time::Duration::from_millis(20));
            std::fs::remove_file(&file).unwrap();
            write(
                &file,
                &format!("{}\n", $user_line("user", "brand new session file content")),
            );
            assert_eq!(
                ingest(&db, adapter, &s),
                1,
                "replaced file is ingested fresh"
            );
            let c = db.get_session(&s.id).unwrap().unwrap().source_cursor();
            assert_eq!(c.generation, 2, "file replacement bumps the generation");
            assert_eq!(
                raw_rows(&db, &s.id).len(),
                1,
                "only the current replacement is retained"
            );
            assert_eq!(
                db.get_messages(&s.id, None, 100).unwrap().len(),
                1,
                "the current conversation is the replaced file"
            );
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
// identity behaviour is covered by the adapter's unit tests instead.

/// Same-size rewrite (size unchanged, mtime changed) must be detected as a
/// rewrite: generation bump + rescan, retired text removed, new text stored.
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
    let s = fixture_session(&db, Agent::Pi, "same-size-root", &file);
    assert_eq!(ingest(&db, adapter, &s), 1);

    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(&file, format!("{}\n", after)).unwrap();

    assert_eq!(
        ingest(&db, adapter, &s),
        1,
        "same-size rewrite adds the new text"
    );
    let conv = db.get_messages(&s.id, None, 100).unwrap();
    assert_eq!(
        conv.len(),
        1,
        "the current conversation is the rewritten text"
    );
    assert_eq!(conv[0].source_generation, 1);
    assert_eq!(
        raw_rows(&db, &s.id).len(),
        1,
        "the raw store agrees with the current conversation"
    );
    let c = db.get_session(&s.id).unwrap().unwrap().source_cursor();
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
    let s = fixture_session(
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
        .commit_ingest(&s.id, &[mk("line:1", "same text")], &rescan(0))
        .unwrap();
    assert_eq!(first.len(), 1);

    // identical re-scan (same chain position, later generation) → dedup
    let again = db
        .commit_ingest(&s.id, &[mk("line:9", "same text")], &rescan(1))
        .unwrap();
    assert_eq!(again.len(), 0, "identical rescan never duplicates");

    // the SAME content appended after another event is a DIFFERENT logical
    // message ("继续" sent twice must not collapse): the append chains from
    // the session's identity tail, so the second occurrence links to a
    // different predecessor.
    let second_msg = db
        .commit_ingest(&s.id, &[mk("line:2", "same text")], &append(0))
        .unwrap();
    assert_eq!(second_msg.len(), 1, "repeated user message stays distinct");

    // native agent message ids dedup regardless of chain position
    let native1 = db
        .commit_ingest(&s.id, &[mk_native("line:3", "native-1")], &append(0))
        .unwrap();
    assert_eq!(native1.len(), 1);
    let native2 = db
        .commit_ingest(&s.id, &[mk_native("line:4", "native-1")], &append(0))
        .unwrap();
    assert_eq!(native2.len(), 0, "native id dedups across positions");

    // genuinely new content still inserts
    let third = db
        .commit_ingest(&s.id, &[mk("line:5", "different text")], &append(0))
        .unwrap();
    assert_eq!(third.len(), 1);
}

/// A stats-only batch (no messages) advances the session cursor's read
/// position but must NOT move the identity tail: only messages advance the
/// chain, so the next append keeps chaining from the last real message.
#[test]
fn stats_only_batch_advances_cursor_but_keeps_identity_tail() {
    let db = open_db("stats-only-tail");
    let dir = unique_dir("stats-only-tail");
    let s = fixture_session(&db, Agent::Codex, "stats-only-root", &dir.join("x.jsonl"));

    let first = db
        .commit_ingest(
            &s.id,
            &[msg("m-1", SessionMessageRole::User, "hello")],
            &rescan(0),
        )
        .unwrap();
    assert_eq!(first.len(), 1);
    let tail = db
        .get_session(&s.id)
        .unwrap()
        .unwrap()
        .source_cursor()
        .identity_tail_hash;
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
    let stored = db.commit_ingest(&s.id, &[], &advanced).unwrap();
    assert!(stored.is_empty(), "a stats-only batch stores no messages");

    let c = db.get_session(&s.id).unwrap().unwrap().source_cursor();
    assert_eq!(c.byte_offset, 200, "the read position moved");
    assert_eq!(
        c.identity_tail_hash, tail,
        "stats-only batches keep the identity tail"
    );
    assert_eq!(db.message_count(&s.id).unwrap(), 1);

    // the next real append still chains from the preserved tail: an id-less
    // message appended after the stats batch links to the recorded tail as
    // its predecessor, landing as a genuinely new chain position.
    let appended = db
        .commit_ingest(
            &s.id,
            &[ParsedSessionMessage {
                source_message_id: None,
                source_position: String::new(),
                ts: Some("2026-09-13T10:00:00Z".into()),
                role: SessionMessageRole::User,
                content: "hello".into(),
            }],
            &append(0),
        )
        .unwrap();
    assert_eq!(appended.len(), 1, "an append is a new chain position");
    assert_ne!(
        db.get_session(&s.id)
            .unwrap()
            .unwrap()
            .source_cursor()
            .identity_tail_hash,
        tail,
        "the tail advanced to the appended message"
    );
}

/// A complete rewrite replaces the current-message projection and deletes
/// retired rows and their search entries in the same transaction. Old message
/// references expire; current messages and Context ownership remain intact.
#[test]
fn truncate_replaces_the_projection_and_removes_retired_history() {
    let db = open_db("truncate-frontier");
    let dir = unique_dir("truncate-frontier");
    let s = fixture_session(
        &db,
        Agent::Codex,
        "truncate-frontier-root",
        &dir.join("x.jsonl"),
    );

    let first = db
        .commit_ingest(
            &s.id,
            &[
                msg("t-1", SessionMessageRole::User, "decision one"),
                msg("t-2", SessionMessageRole::Assistant, "reply one"),
                msg("t-3", SessionMessageRole::User, "decision two"),
            ],
            &rescan(0),
        )
        .unwrap();
    assert_eq!(first.len(), 3);
    assert_eq!(
        db.get_messages(&s.id, None, 100).unwrap().len(),
        3,
        "the effective conversation is the three ingested messages"
    );

    // the source truncates to a compaction summary: a full re-scan at a new
    // generation atomically replaces the projection and retires the old rows.
    let compacted = db
        .commit_ingest(
            &s.id,
            &[msg(
                "t-4",
                SessionMessageRole::User,
                "compacted summary of it all",
            )],
            &rescan(1),
        )
        .unwrap();
    assert_eq!(compacted.len(), 1, "the summary is new evidence");

    // Retired messages drop out of search the moment the projection is
    // replaced; the replacement conversation is what stays indexed.
    assert!(
        support::search_messages(&db, "decision", 10)
            .unwrap()
            .iter()
            .all(|hit| hit.kind != "message" || hit.parent_id != s.id),
        "messages retired from the current projection must not remain searchable"
    );
    assert!(
        support::search_messages(&db, "compacted", 10)
            .unwrap()
            .iter()
            .any(|hit| hit.kind == "message" && hit.parent_id == s.id),
        "the replacement conversation must be indexed"
    );

    // Retired references expire, and the persisted store has no hidden history.
    assert_eq!(raw_rows(&db, &s.id).len(), 1);
    for m in &first {
        assert!(
            db.get_message_by_ref(&format!("session-message:{}", m.id))
                .unwrap()
                .is_none(),
            "retired message {} must no longer be addressable",
            m.id
        );
    }
}

#[test]
fn full_scan_updates_search_without_rewriting_retained_documents() {
    let db = open_db("search-diff");
    let dir = unique_dir("search-diff");
    let s = fixture_session(&db, Agent::Codex, "search-diff", &dir.join("s.jsonl"));
    let keep = msg("keep", SessionMessageRole::User, "retainedlexeme");
    let edit = msg("edit", SessionMessageRole::Assistant, "retiredlexeme");
    let first = db
        .commit_ingest(
            &s.id,
            &[
                keep.clone(),
                edit,
                msg("delete", SessionMessageRole::User, "deletedlexeme"),
            ],
            &rescan(0),
        )
        .unwrap();
    // Another session holds a later FTS rowid. Deleting and reinserting the
    // retained document cannot accidentally reuse its original rowid.
    let other = fixture_session(&db, Agent::Codex, "search-other", &dir.join("other.jsonl"));
    let foreign = db
        .commit_ingest(
            &other.id,
            &[msg("foreign", SessionMessageRole::User, "foreignlexeme")],
            &rescan(0),
        )
        .unwrap();
    let index_rows = |id: &str| -> Vec<(i64, String, String)> {
        let conn = db.read();
        let mut st = conn
            .prepare(
                "SELECT rowid, ref_id, body FROM search_index
                 WHERE kind = 'message' AND parent_id = ?1 ORDER BY ref_id",
            )
            .unwrap();
        st.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
    };
    let retained_row = index_rows(&s.id)
        .into_iter()
        .find(|(_, id, _)| id == &first[0].id)
        .unwrap();
    let foreign_rows = index_rows(&other.id);
    let updated = msg("edit", SessionMessageRole::Assistant, "updatedlexeme");
    let added = msg("new", SessionMessageRole::Assistant, "addedlexeme");
    db.commit_ingest(
        &s.id,
        &[added.clone(), keep.clone(), updated.clone()],
        &rescan(1),
    )
    .unwrap();
    let current = db.get_messages(&s.id, None, 10).unwrap();
    assert_eq!(current[1].id, first[0].id);
    let rows = index_rows(&s.id);
    assert_eq!(rows.len(), 3, "exactly one document per current message");
    assert_eq!(
        rows.iter().find(|(_, id, _)| id == &first[0].id),
        Some(&retained_row),
        "unchanged documents retain their FTS row instead of being reindexed"
    );
    assert_eq!(index_rows(&other.id), foreign_rows);
    for query in ["retiredlexeme", "deletedlexeme"] {
        let count: i64 = db
            .read()
            .query_row(
                "SELECT COUNT(*) FROM search_index WHERE search_index MATCH ?1",
                [query],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "retired text is removed from FTS itself");
        assert!(support::search_messages(&db, query, 10).unwrap().is_empty());
    }
    for (query, expected_id) in [
        ("addedlexeme", &current[0].id),
        ("retainedlexeme", &first[0].id),
        ("updatedlexeme", &current[2].id),
        ("foreignlexeme", &foreign[0].id),
    ] {
        let hits = support::search_messages(&db, query, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(&hits[0].ref_id, expected_id);
    }

    // Reordering changes the projection but none of the search documents.
    db.commit_ingest(&s.id, &[updated, keep, added], &rescan(2))
        .unwrap();
    assert_eq!(index_rows(&s.id), rows);
    db.commit_ingest(&s.id, &[], &rescan(3)).unwrap();
    assert!(index_rows(&s.id).is_empty());
    assert_eq!(index_rows(&other.id), foreign_rows);
}

/// After a compact + dedup re-scan, the identity chain must continue from
/// the CURRENT source tail (tracked on the session row), not from the
/// message store's last row. Chaining an append from a stale tail would
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
    let s = fixture_session(&db, Agent::Pi, "compact-tail-root", &file);
    assert_eq!(ingest(&db, adapter, &s), 4);
    let store_tail_before = db
        .get_session(&s.id)
        .unwrap()
        .unwrap()
        .source_cursor()
        .identity_tail_hash;
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
    assert_eq!(ingest(&db, adapter, &s), 0, "compacted prefix dedups");

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
    let tail = db
        .get_session(&s.id)
        .unwrap()
        .unwrap()
        .source_cursor()
        .identity_tail_hash;
    assert_ne!(
        tail, store_tail_before,
        "tail follows the source, not the store"
    );
    assert_eq!(
        tail,
        hash_of(&text('B')),
        "tail follows the retained B message"
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
    assert_eq!(ingest(&db, adapter, &s), 1, "E is new");
    let e_hash = hash_of(&text('E'));

    // full re-scan of A,B,E: E must dedup — it did NOT chain from the
    // stale store tail (the bug would duplicate it here)
    assert_eq!(
        ingest_mode(&db, adapter, &s, true),
        0,
        "E survives a full re-scan without duplication"
    );
    assert_eq!(
        raw_rows(&db, &s.id).len(),
        3,
        "only the current A,B,E messages are retained"
    );
    assert_eq!(
        db.get_messages(&s.id, None, 100).unwrap().len(),
        3,
        "the current conversation is the compacted A,B,E"
    );
    assert_eq!(
        db.get_session(&s.id)
            .unwrap()
            .unwrap()
            .source_cursor()
            .identity_tail_hash,
        e_hash
    );
}

/// Re-ingest reuses unchanged message ids and dedups to zero new rows.
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
    let s = fixture_session(&db, Agent::Pi, "reingest-root", &file);
    assert_eq!(ingest(&db, adapter, &s), 2);
    let ids_before: Vec<String> = db
        .get_messages(&s.id, None, 100)
        .unwrap()
        .into_iter()
        .map(|e| e.id)
        .collect();

    // Force a full read without changing the persisted cursor first.
    assert_eq!(
        ingest_mode(&db, adapter, &s, true),
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
        ingest(&db, adapter, &s),
        1,
        "new content appends after the re-scan"
    );
    assert_eq!(db.get_messages(&s.id, None, 100).unwrap().len(), 3);
}

#[test]
fn incomplete_replacement_preserves_all_facts_and_complete_replacement_retires_them() {
    let db = open_db("incomplete-replacement");
    let file = unique_dir("incomplete-replacement").join("s.jsonl");
    let s = fixture_session(&db, Agent::Pi, "incomplete", &file);
    let original = db
        .commit_ingest(
            &s.id,
            &[msg("old", SessionMessageRole::User, "reliable history")],
            &rescan(0),
        )
        .unwrap();
    let before = db.get_session(&s.id).unwrap().unwrap();
    let search_document = || -> (i64, String, String) {
        db.read()
            .query_row(
                "SELECT rowid, ref_id, body FROM search_index
                 WHERE kind = 'message' AND parent_id = ?1",
                [&s.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
    };
    let original_document = search_document();
    let replacement = [msg("new", SessionMessageRole::User, "replacement history")];
    assert!(db
        .commit_ingest_snapshot(&s.id, &replacement, &rescan(1), false)
        .unwrap()
        .is_empty());
    assert_eq!(raw_rows(&db, &s.id).len(), 1);
    assert_eq!(
        db.get_messages(&s.id, None, 10).unwrap()[0].id,
        original[0].id
    );
    let after = db.get_session(&s.id).unwrap().unwrap();
    assert_eq!(
        (
            after.fact_generation,
            after.source_byte_offset,
            after.source_prefix_hash
        ),
        (
            before.fact_generation,
            before.source_byte_offset,
            before.source_prefix_hash
        )
    );

    // A storage failure while retiring the old row rolls back the new row,
    // projection, search index and frontier as one transaction.
    db.write().execute_batch("CREATE TEMP TRIGGER refuse_retirement BEFORE DELETE ON session_messages BEGIN SELECT RAISE(ABORT, 'retirement failed'); END;").unwrap();
    assert!(db.commit_ingest(&s.id, &replacement, &rescan(1)).is_err());
    assert_eq!(raw_rows(&db, &s.id).len(), 1);
    assert_eq!(search_document(), original_document);
    assert_eq!(
        db.get_messages(&s.id, None, 10).unwrap()[0].id,
        original[0].id
    );
    assert!(support::search_messages(&db, "replacement", 10)
        .unwrap()
        .iter()
        .all(|h| h.kind != "message"));
    db.write()
        .execute_batch("DROP TRIGGER refuse_retirement")
        .unwrap();
    db.commit_ingest(&s.id, &replacement, &rescan(1)).unwrap();
    assert_eq!(raw_rows(&db, &s.id).len(), 1);
    assert_eq!(
        db.get_messages(&s.id, None, 10).unwrap()[0].content,
        "replacement history"
    );
    assert!(db
        .get_message_by_ref(&format!("session-message:{}", original[0].id))
        .unwrap()
        .is_none());
}

#[test]
fn full_scan_collects_unmapped_rows_without_rewriting_unchanged_messages() {
    let db = open_db("collect-unmapped");
    let file = unique_dir("collect-unmapped").join("s.jsonl");
    let s = fixture_session(&db, Agent::Pi, "collect-unmapped", &file);
    let messages = [msg("current", SessionMessageRole::User, "current history")];
    let current = db.commit_ingest(&s.id, &messages, &rescan(0)).unwrap();
    db.write().execute(
        "INSERT INTO session_messages(id, session_id, sequence, source_identity_hash, role, content) VALUES('unmapped', ?1, 99, 'unmapped', 'assistant', 'old orphan')", [&s.id],
    ).unwrap();
    db.write().execute("INSERT INTO search_documents(kind, ref_id, parent_id, title, body) VALUES('message', 'unmapped', ?1, '', 'old orphan')", [&s.id]).unwrap();
    let generation = db.get_session(&s.id).unwrap().unwrap().fact_generation;
    assert!(db
        .commit_ingest(&s.id, &messages, &rescan(0))
        .unwrap()
        .is_empty());
    assert_eq!(raw_rows(&db, &s.id).len(), 1);
    assert_eq!(
        db.get_messages(&s.id, None, 10).unwrap()[0].id,
        current[0].id
    );
    assert_eq!(
        db.get_session(&s.id).unwrap().unwrap().fact_generation,
        generation
    );
    let orphan_index: i64 = db
        .read()
        .query_row(
            "SELECT COUNT(*) FROM search_index WHERE ref_id='unmapped'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(orphan_index, 0);

    // An existing message without its search document is repaired once, even
    // when the full read reuses its message id and leaves the projection alone.
    db.write()
        .execute(
            "DELETE FROM search_documents WHERE kind='message' AND ref_id=?1",
            [&current[0].id],
        )
        .unwrap();
    db.commit_ingest(&s.id, &messages, &rescan(0)).unwrap();
    let hits = support::search_messages(&db, "current", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].ref_id, current[0].id);

    let changes = || {
        db.write()
            .query_row("SELECT total_changes()", [], |r| r.get::<_, i64>(0))
            .unwrap()
    };
    let before = changes();
    db.commit_ingest(&s.id, &messages, &rescan(0)).unwrap();
    assert_eq!(
        changes(),
        before,
        "unchanged full scans do not rebuild search or rewrite messages"
    );

    // An empty current projection must not count unmapped stored history.
    db.write()
        .execute(
            "DELETE FROM session_message_projection WHERE session_id=?1",
            [&s.id],
        )
        .unwrap();
    let stats = db.session_message_stats(&s.id).unwrap();
    assert_eq!((stats.user_messages, stats.assistant_messages), (0, 0));
}

#[test]
fn a_stale_read_cannot_overwrite_a_newer_cursor_messages_or_search() {
    let db = open_db("stale-read");
    let file = unique_dir("stale-read").join("s.jsonl");
    let s = fixture_session(&db, Agent::Codex, "stale-read", &file);
    let old = msg("old", SessionMessageRole::User, "originalhistory");
    db.commit_ingest(&s.id, &[old.clone()], &rescan(0)).unwrap();
    let stale = db.get_session(&s.id).unwrap().unwrap().source_cursor();
    let fresh = db
        .commit_ingest(
            &s.id,
            &[msg("new", SessionMessageRole::Assistant, "freshhistory")],
            &append(0),
        )
        .unwrap();
    let current = db.get_session(&s.id).unwrap().unwrap();
    for (messages, source) in [
        (vec![old], rescan(1)),
        (
            vec![msg("stale", SessionMessageRole::Assistant, "stalehistory")],
            append(0),
        ),
    ] {
        assert!(db
            .commit_ingest_guarded(&s.id, &messages, &source, true, Some(&stale))
            .is_err());
        assert_eq!(
            db.get_session(&s.id).unwrap().unwrap().source_cursor(),
            current.source_cursor()
        );
        assert_eq!(db.get_messages(&s.id, None, 10).unwrap().len(), 2);
        assert_eq!(
            support::search_messages(&db, "freshhistory", 10).unwrap()[0].ref_id,
            fresh[0].id
        );
        assert!(support::search_messages(&db, "stalehistory", 10)
            .unwrap()
            .is_empty());
    }
}

#[test]
fn message_parts_preserve_unicode_ranges_and_offsets_across_append_and_reorder() {
    use noending::storage::message_query::{part_count, parts};
    let db = open_db("message-parts");
    let first_chunk = format!("{}😀", "甲".repeat(1999));
    let first = msg(
        "parts-first",
        SessionMessageRole::User,
        format!("{first_chunk}乙"),
    );
    let empty = msg("parts-empty", SessionMessageRole::Assistant, "");
    let exact = msg("parts-exact", SessionMessageRole::User, "é".repeat(4000));
    let (session, _) = support::seed_conversation(
        &db,
        Agent::Codex,
        "message-parts",
        &[first.clone(), empty.clone(), exact.clone()],
    );
    {
        let conn = db.read();
        assert_eq!(part_count(&conn, &session.id, 0, 3).unwrap(), 5);
        assert_eq!(part_count(&conn, &session.id, 1, 99).unwrap(), 3);
        assert_eq!(part_count(&conn, &session.id, 99, 100).unwrap(), 0);
        let page = parts(&conn, &session.id, 0, 3, 0, 4, None).unwrap();
        assert_eq!(page.len(), 4);
        assert_eq!(page[0]["record"]["content"], first_chunk);
        assert_eq!(page[1]["record"]["content"], "乙");
        assert_eq!(page[2]["record"]["content"], "");
        assert_eq!(page[3]["record"]["content"], "é".repeat(2000));
        assert_eq!(page[3]["record"]["last_part"], false);
        let page = parts(&conn, &session.id, 1, 3, 3, 10, None).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0]["cursor"], 3);
        assert_eq!(page[0]["record"]["sequence"], 3);
        assert_eq!(page[0]["record"]["part"], 2);
        assert_eq!(page[0]["record"]["last_part"], true);
        assert_eq!(page[0]["record"]["content"], "é".repeat(2000));
        assert!(parts(&conn, &session.id, 0, 3, 6, 10, None)
            .unwrap()
            .is_empty());
        let search = parts(&conn, &session.id, 0, 3, 1, 10, Some("😀乙")).unwrap();
        assert_eq!(
            search.len(),
            2,
            "search matches the whole message, including across chunks"
        );
        assert_eq!(search[1]["record"]["content"], "乙");
    }
    let appended = msg("parts-appended", SessionMessageRole::Assistant, "追加");
    db.commit_ingest(
        &session.id,
        std::slice::from_ref(&appended),
        &SourceCursorUpdate {
            start_byte_offset: 100,
            byte_offset: 200,
            last_seen_size: 200,
            ..support::seed_source(0)
        },
    )
    .unwrap();
    {
        let conn = db.read();
        assert_eq!(part_count(&conn, &session.id, 0, 4).unwrap(), 6);
        assert_eq!(
            part_count(&conn, &session.id, 0, 3).unwrap(),
            5,
            "existing cutoffs remain stable"
        );
        let page = parts(&conn, &session.id, 3, 4, 1, 2, None).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0]["record"]["content"], "追加");
    }
    // Reorder retained messages, retire one and append a new message. Offsets
    // must follow current ordinals rather than retained storage sequences.
    let tail = msg("parts-tail", SessionMessageRole::User, "新的末尾");
    db.commit_ingest(
        &session.id,
        &[exact, empty, first, tail],
        &support::seed_source(1),
    )
    .unwrap();
    let conn = db.read();
    assert_eq!(part_count(&conn, &session.id, 0, 4).unwrap(), 6);
    let page = parts(&conn, &session.id, 2, 4, 2, 10, None).unwrap();
    assert_eq!(page.len(), 2);
    assert_eq!(page[0]["record"]["sequence"], 3);
    assert_eq!(page[0]["record"]["content"], "乙");
    assert_eq!(page[1]["record"]["sequence"], 4);
    assert_eq!(page[1]["record"]["content"], "新的末尾");
}
