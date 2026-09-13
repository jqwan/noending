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

            assert_eq!(
                ingest(&db, adapter, &s),
                3,
                "initial ingest stores all events"
            );
            let all = db.get_events(&s.id, None, 100).unwrap();
            assert_eq!(all.len(), 3);
            assert_eq!(
                all.iter().map(|e| e.sequence).collect::<Vec<_>>(),
                vec![1, 2, 3],
                "sequences are app-assigned and dense on first ingest"
            );
            assert!(all.iter().all(|e| e.source_generation == 0));
            assert!(
                all.iter().all(|e| !e.id.is_empty()),
                "every event has a stable id"
            );

            let c = db.get_source_cursor(&s.id).unwrap();
            assert_eq!(c.generation, 0);
            assert_eq!(
                c.byte_offset as usize,
                std::fs::metadata(&file).unwrap().len() as usize
            );
            assert!(!c.source_file_identity.is_empty());

            // ---- append: only the delta is stored ----
            std::thread::sleep(std::time::Duration::from_millis(20));
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&file)
                .unwrap();
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

            assert_eq!(
                ingest(&db, adapter, &s),
                2,
                "compacted content is new events"
            );
            let all = db.get_events(&s.id, None, 100).unwrap();
            assert_eq!(all.len(), 7, "old history is preserved, never overwritten");
            assert_eq!(
                all.iter().map(|e| e.sequence).collect::<Vec<_>>(),
                vec![1, 2, 3, 4, 5, 6, 7],
                "sequences keep increasing after truncate; no reuse"
            );
            assert_eq!(all.iter().filter(|e| e.source_generation == 1).count(), 2);

            // ---- rescan of identical content dedups ----
            assert_eq!(
                ingest(&db, adapter, &s),
                0,
                "identical rescan never duplicates"
            );

            // ---- file replacement (new identity) ----
            std::thread::sleep(std::time::Duration::from_millis(20));
            std::fs::remove_file(&file).unwrap();
            std::fs::write(
                &file,
                format!("{}\n", $user_line("user", "brand new session file content")),
            )
            .unwrap();
            assert_eq!(
                ingest(&db, adapter, &s),
                1,
                "replaced file is ingested fresh"
            );
            let c = db.get_source_cursor(&s.id).unwrap();
            assert_eq!(c.generation, 2, "file replacement bumps the generation");
            let all = db.get_events(&s.id, None, 100).unwrap();
            assert_eq!(all.len(), 8);
        }
    };
}

identity_suite!(
    codex_truncate_rewrite_dedup,
    Agent::Codex,
    noending::adapters::codex::CodexAdapter,
    codex_line,
    codex_line
);
identity_suite!(
    claude_truncate_rewrite_dedup,
    Agent::ClaudeCode,
    noending::adapters::claude::ClaudeAdapter,
    claude_line,
    claude_line
);
identity_suite!(
    pi_truncate_rewrite_dedup,
    Agent::Pi,
    noending::adapters::pi::PiAdapter,
    pi_line,
    pi_line
);

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
    let s = session_row(&db, Agent::Pi, &file);
    assert_eq!(ingest(&db, adapter, &s), 1);

    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(&file, format!("{}\n", after)).unwrap();

    assert_eq!(
        ingest(&db, adapter, &s),
        1,
        "same-size rewrite adds the new text"
    );
    let all = db.get_events(&s.id, None, 100).unwrap();
    assert_eq!(all.len(), 2, "history preserved");
    assert_eq!(all[1].source_generation, 1);
    let c = db.get_source_cursor(&s.id).unwrap();
    assert_eq!(c.generation, 1);
}

/// Event identity semantics (chained adjacency for fallback events):
/// - identical re-scan from the same chain position → dedup;
/// - the same content at a DIFFERENT chain position (a genuinely repeated
///   user message like "继续") stays distinct;
/// - native agent event ids dedup regardless of position;
/// - genuinely new content always inserts.
#[test]
fn identity_dedup_follows_chain_semantics() {
    let db = open_db("chain-identity");
    let dir = unique_dir("chain-identity");
    let s = session_row(&db, Agent::Codex, &dir.join("x.jsonl"));

    let mk = |pos: &str, text: &str| noending::domain::ParsedEvent {
        source_event_id: None,
        source_position: pos.into(),
        ts: Some("t".into()),
        kind: "user_message".into(),
        text: Some(text.into()),
        metadata: serde_json::json!({}),
    };
    let mk_native = |pos: &str, id: &str| noending::domain::ParsedEvent {
        source_event_id: Some(id.into()),
        source_position: pos.into(),
        ts: Some("t".into()),
        kind: "user_message".into(),
        text: Some("same text".into()),
        metadata: serde_json::json!({}),
    };
    // start_byte_offset == 0 → full re-scan → chain restarts at genesis.
    let rescan = |gen: i64| noending::domain::SourceCursorUpdate {
        file_identity: "unix:dev:1:ino:2".into(),
        generation: gen,
        byte_offset: 10,
        last_seen_size: 10,
        mtime: None,
        start_byte_offset: 0,
        prefix_hash: String::new(),
    };
    // start_byte_offset > 0 → append → chain continues from the last event.
    let append = |gen: i64| noending::domain::SourceCursorUpdate {
        file_identity: "unix:dev:1:ino:2".into(),
        generation: gen,
        byte_offset: 20,
        last_seen_size: 20,
        mtime: None,
        start_byte_offset: 10,
        prefix_hash: String::new(),
    };

    // first sight of the content
    let first = db
        .append_source_events(&s.id, &[mk("line:1", "same text")], &rescan(0), "/x.jsonl")
        .unwrap();
    assert_eq!(first.len(), 1);

    // identical re-scan (same chain position, later generation) → dedup
    let again = db
        .append_source_events(&s.id, &[mk("line:9", "same text")], &rescan(1), "/x.jsonl")
        .unwrap();
    assert_eq!(again.len(), 0, "identical rescan never duplicates");

    // the SAME content appended after another event is a DIFFERENT logical
    // event ("继续" sent twice must not collapse)
    let second_msg = db
        .append_source_events(&s.id, &[mk("line:2", "same text")], &append(0), "/x.jsonl")
        .unwrap();
    assert_eq!(second_msg.len(), 1, "repeated user message stays distinct");

    // native agent event ids dedup regardless of chain position
    let native1 = db
        .append_source_events(
            &s.id,
            &[mk_native("line:3", "native-1")],
            &append(0),
            "/x.jsonl",
        )
        .unwrap();
    assert_eq!(native1.len(), 1);
    let native2 = db
        .append_source_events(
            &s.id,
            &[mk_native("line:4", "native-1")],
            &append(0),
            "/x.jsonl",
        )
        .unwrap();
    assert_eq!(native2.len(), 0, "native id dedups across positions");

    // genuinely new content still inserts
    let third = db
        .append_source_events(
            &s.id,
            &[mk("line:5", "different text")],
            &append(0),
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
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap();
    use std::io::Write;
    let rest = format!(
        "age\":{{\"role\":\"user\",\"content\":[{{\"type\":\"text\",\"text\":\"now complete\"}}]}},\"timestamp\":\"2026-09-13T10:00:01Z\"}}\n"
    );
    write!(f, "{}", rest).unwrap();
    drop(f);
    assert_eq!(ingest(&db, adapter, &s), 1, "completed line is picked up");
}

/// A rewrite that GROWS the file must not be mistaken for an append: the
/// stored prefix fingerprint no longer matches, so the read is a rewrite
/// (generation bump + full rescan), never a mid-file partial read.
#[test]
fn rewrite_grow_is_detected_not_treated_as_append() {
    let db = open_db("rewrite-grow");
    let dir = unique_dir("rewrite-grow");
    let file = dir.join("s.jsonl");
    let adapter: &dyn AgentAdapter = &noending::adapters::pi::PiAdapter;

    let old = format!(
        "{}\n{}\n",
        pi_line("user", "old first message aaaaaaaaaaaaaaaaaaaaaa"),
        pi_line("assistant", "old second message bbbbbbbbbbbbbbbbbbbbb")
    );
    std::fs::write(&file, &old).unwrap();
    let s = session_row(&db, Agent::Pi, &file);
    assert_eq!(ingest(&db, adapter, &s), 2);
    let offset_before = db.get_source_cursor(&s.id).unwrap().byte_offset;

    // rewrite with MORE content than before: size grew, prefix changed
    std::thread::sleep(std::time::Duration::from_millis(50));
    let new = format!(
        "{}\n{}\n{}\n",
        pi_line("user", "rewritten first message xxxxxxxxxxxxxxxxxxxx"),
        pi_line("assistant", "rewritten second message yyyyyyyyyyyyyyyy"),
        pi_line("user", "rewritten third message zzzzzzzzzzzzzzzzzzzzz")
    );
    assert!(new.len() > old.len(), "fixture must grow");
    std::fs::write(&file, &new).unwrap();

    // If this were misread as an append from offset_before, the parse would
    // start mid-line and drop the rewritten head. It must be a full rescan.
    assert_eq!(
        ingest(&db, adapter, &s),
        3,
        "all rewritten lines are new events"
    );
    let all = db.get_events(&s.id, None, 100).unwrap();
    assert_eq!(
        all.len(),
        5,
        "history preserved, rewritten content appended"
    );
    assert_eq!(
        all.iter().map(|e| e.sequence).collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5]
    );
    assert_eq!(
        all[2].source_generation, 1,
        "rewritten content carries the new generation"
    );
    assert!(all[2].text.as_deref().unwrap().contains("rewritten first"));
    let c = db.get_source_cursor(&s.id).unwrap();
    assert_eq!(c.generation, 1, "generation bumped exactly once");
    assert!(c.byte_offset > offset_before);
}

/// After a compact + dedup re-scan, the identity chain must continue from
/// the CURRENT source tail (tracked on the cursor), not from the event
/// store's last row: the store keeps newer history the source no longer
/// has (append-only), and chaining an append from the store tail would make
/// that append invisible to the next full re-scan (duplicated as "new").
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
    let s = session_row(&db, Agent::Pi, &file);
    assert_eq!(ingest(&db, adapter, &s), 4);
    let store_tail_before = db.get_source_cursor(&s.id).unwrap().identity_tail_hash;
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
        db.conn()
            .query_row(
                "SELECT source_identity_hash FROM session_events WHERE session_id = ?1 AND text = ?2",
                rusqlite::params![s.id, t],
                |r| r.get(0),
            )
            .unwrap()
    };
    let tail = db.get_source_cursor(&s.id).unwrap().identity_tail_hash;
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
    assert_eq!(ingest(&db, adapter, &s), 1, "E is new");
    let e_hash = hash_of(&text('E'));

    // full re-scan of A,B,E: E must dedup — it did NOT chain from the
    // stale store tail (the bug would duplicate it here)
    db.reset_session_source_cursor(&s.id).unwrap();
    assert_eq!(
        ingest(&db, adapter, &s),
        0,
        "E survives a full re-scan without duplication"
    );
    assert_eq!(db.get_events(&s.id, None, 100).unwrap().len(), 5);
    assert_eq!(
        db.get_source_cursor(&s.id).unwrap().identity_tail_hash,
        e_hash
    );
}

/// Re-ingest must NEVER delete the event store: ids survive, SourceReferences
/// in context revisions stay valid, and a full re-scan dedups to zero new rows.
#[test]
fn reingest_preserves_event_ids_and_dedups() {
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
    let s = session_row(&db, Agent::Pi, &file);
    assert_eq!(ingest(&db, adapter, &s), 2);
    let ids_before: Vec<String> = db
        .get_events(&s.id, None, 100)
        .unwrap()
        .into_iter()
        .map(|e| e.id)
        .collect();

    // simulate 重新入库: rewind the read cursor, keep the event store
    db.reset_session_source_cursor(&s.id).unwrap();
    assert_eq!(
        ingest(&db, adapter, &s),
        0,
        "re-scan of unchanged source adds nothing"
    );

    let all = db.get_events(&s.id, None, 100).unwrap();
    let ids_after: Vec<String> = all.iter().map(|e| e.id.clone()).collect();
    assert_eq!(
        ids_after, ids_before,
        "event ids (and SourceReferences) survive re-ingest"
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
    assert_eq!(db.get_events(&s.id, None, 100).unwrap().len(), 3);
}

/// The append-only Event Store is NOT a linear chain of the current source:
/// a pre-chain database that went through rewrite/compact keeps superseded
/// history (store A B C D for a source that is now A B D). Recomputing hashes
/// in store order would give D the wrong chain position (H(C,D) instead of
/// H(B,D)) and the next re-scan would duplicate it. The v4 migration instead
/// stamps legacy rows with a claimable alias and re-identifies them from the
/// REAL source on the next full re-scan: count, event ids and uniqueness all
/// survive. Covers BOTH upgrade sources: a v2 store (content hashes) and a
/// store already rewritten by the shipped-v3 store-order recompute.
#[test]
fn migration_v4_claims_diverged_legacy_store_from_real_source() {
    let text = |c: char| {
        format!(
            "legacy event {c} {}{}{}{}{}{}{}{}{}{}",
            c, c, c, c, c, c, c, c, c, c
        )
    };
    let mk = |t: &str| noending::domain::ParsedEvent {
        source_event_id: None,
        source_position: "line:1".into(),
        ts: Some("t".into()),
        kind: "user_message".into(),
        text: Some(t.into()),
        metadata: serde_json::json!({}),
    };
    let rescan = noending::domain::SourceCursorUpdate {
        file_identity: "unix:dev:1:ino:9".into(),
        generation: 0,
        byte_offset: 10,
        last_seen_size: 10,
        mtime: None,
        start_byte_offset: 0,
        prefix_hash: String::new(),
    };
    // chained identity the CURRENT source A,B,D would produce
    let h = |prev: &str, t: &str| {
        noending::storage::event_identity_hash(prev, None, "user_message", Some("t"), Some(t))
    };
    let h_a = h(noending::storage::IDENTITY_GENESIS, &text('A'));
    let h_b = h(&h_a, &text('B'));
    let h_d = h(&h_b, &text('D'));

    for (regress_to, label) in [(2i64, "v2 content hashes"), (3, "v3 store-order recompute")] {
        let dir = unique_dir(&format!("identity-migration-{}", regress_to));
        let path = dir.join("test.db");

        // build the diverged store A B C D plus a cursor that believes the
        // store tail is the source tail — what a v2 app / the shipped v3
        // migration leaves behind
        let (session_id, ids_before, d_id) = {
            let db = Db::open(&path).unwrap();
            let s = session_row(&db, Agent::Pi, &dir.join("x.jsonl"));
            let events: Vec<noending::domain::SessionEvent> = ['A', 'B', 'C', 'D']
                .iter()
                .enumerate()
                .map(|(i, c)| noending::domain::SessionEvent {
                    id: new_id(),
                    session_id: s.id.clone(),
                    sequence: i as i64 + 1,
                    source_event_id: None,
                    source_generation: 0,
                    source_position: format!("line:{}", i + 1),
                    ts: Some("t".into()),
                    kind: "user_message".into(),
                    text: Some(text(*c)),
                    raw_ref: format!("/x.jsonl#line:{}", i + 1),
                    metadata: serde_json::json!({}),
                })
                .collect();
            let d_id = events[3].id.clone();
            let ids_before: Vec<String> = events.iter().map(|e| e.id.clone()).collect();
            db.append_events(&events).unwrap();
            db.set_source_cursor(&noending::domain::SourceCursor {
                session_id: s.id.clone(),
                source_file_identity: "unix:dev:1:ino:9".into(),
                generation: 0,
                byte_offset: 999,
                last_seen_size: 999,
                mtime: None,
                prefix_hash: "stale-prefix".into(),
                identity_tail_hash: "store-tail-not-source-tail".into(),
                last_sequence: 4,
            })
            .unwrap();
            (s.id, ids_before, d_id)
        };

        // regress the database the way the pre-v4 world left it
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            if regress_to == 2 {
                // real ≤ v2 fallback identities: content-only hashes
                let rows: Vec<(String, String, Option<String>, Option<String>)> = {
                    let mut st = conn
                        .prepare("SELECT id, kind, ts, text FROM session_events")
                        .unwrap();
                    let rows = st
                        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                        .unwrap()
                        .collect::<std::result::Result<Vec<_>, _>>()
                        .unwrap();
                    rows
                };
                for (id, kind, ts, t) in rows {
                    conn.execute(
                        "UPDATE session_events SET source_identity_hash = ?2 WHERE id = ?1",
                        rusqlite::params![
                            id,
                            noending::storage::legacy_fallback_identity_hash(
                                &kind,
                                ts.as_deref(),
                                t.as_deref()
                            )
                        ],
                    )
                    .unwrap();
                }
            }
            // regress_to == 3: keep the chained store-order hashes the
            // shipped v3 recompute produced (append_events chains exactly
            // like it did)
            conn.pragma_update(None, "user_version", regress_to)
                .unwrap();
        }

        // reopen: v4 stamps the alias column and rewinds cursors
        let db = Db::open(&path).unwrap();
        let version: i64 = db
            .conn()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 4, "{}: schema advanced", label);
        let cursor = db.get_source_cursor(&session_id).unwrap();
        assert_eq!(
            cursor.identity_tail_hash, "",
            "{}: cursor rewound for a source-driven re-scan",
            label
        );

        // the real source is A B D (C was compacted away): the re-scan must
        // claim D onto H(B,D) instead of duplicating it
        let s = db.get_session(&session_id).unwrap().unwrap();
        let stored = db
            .append_source_events(
                &s.id,
                &[mk(&text('A')), mk(&text('B')), mk(&text('D'))],
                &rescan,
                "/x.jsonl",
            )
            .unwrap();
        assert_eq!(
            stored.len(),
            0,
            "{}: A,B dedup and D is claimed — nothing new",
            label
        );
        assert_eq!(
            db.event_count(&s.id).unwrap(),
            4,
            "{}: count unchanged",
            label
        );

        let ids_after: Vec<String> = db
            .get_events(&s.id, None, 100)
            .unwrap()
            .into_iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(ids_after.len(), 4, "{}: no duplicate D", label);
        assert_eq!(
            ids_after.iter().filter(|id| **id == d_id).count(),
            1,
            "{}: D keeps its original event id (SourceReferences stay valid)",
            label
        );
        for id in &ids_before {
            assert!(ids_after.contains(id), "{}: id {} preserved", label, id);
        }

        // the cursor now carries the REAL source-chain tail H(A,B,D)
        let tail = db.get_source_cursor(&s.id).unwrap().identity_tail_hash;
        assert_eq!(
            tail, h_d,
            "{}: tail follows the source chain, not the store",
            label
        );
    }
}
