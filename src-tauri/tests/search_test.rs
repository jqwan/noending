//! Search behavior contract (重构方案 §21/§22, doc §32.9).
//!
//! FTS5 answers token queries; a query its tokenizer cannot match still finds
//! the document through the LIKE pass. Only Logical Session documents and ROOT
//! conversation messages are indexed — and a trashed session's rows are hidden
//! by the read-side lifecycle guard, not only by the unindex-on-trash write.

use noending::domain::{Agent, SessionMessageRole};
use noending::lifecycle;
use noending::search::search;
use noending::storage::{new_id, Db};
use std::ops::Deref;

mod support;

struct TestDb {
    db: Option<Db>,
    dir: std::path::PathBuf,
}

impl Deref for TestDb {
    type Target = Db;
    fn deref(&self) -> &Self::Target {
        self.db.as_ref().unwrap()
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        self.db.take();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn open_db(tag: &str) -> TestDb {
    let dir = std::env::temp_dir().join(format!("noending-search-{tag}-{}", new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).unwrap();
    TestDb { db: Some(db), dir }
}

/// A query inside a token ("Foob" in "NoEndingFoobar") is invisible to FTS5's
/// MATCH, which compares whole tokens. The LIKE pass is what makes such a query
/// find anything at all — and it must run whenever MATCH came back empty, not
/// only when the index happens to be empty.
#[test]
fn a_query_inside_a_token_still_finds_the_document() {
    let db = open_db("fallback");
    let p = support::project("p1".into(), "NoEndingFoobar");
    db.upsert_project(&p).unwrap();
    db.index_project(&p).unwrap();

    let whole = search(&db, "NoEndingFoobar", 10).unwrap();
    assert_eq!(whole.len(), 1, "a whole token is the FTS pass");

    let substring = search(&db, "Foob", 10).unwrap();
    assert_eq!(
        substring.len(),
        1,
        "an in-token query must fall back to LIKE: {substring:?}"
    );
    assert_eq!(substring[0].ref_id, "p1");

    assert!(
        search(&db, "nothing-here", 10).unwrap().is_empty(),
        "no match anywhere is an empty result, not an error"
    );
}

/// The LIKE pass treats the query as text, not as a pattern: `_` and `%` are
/// ordinary characters in a file name or a percentage, and a query that is
/// nothing but one of them must not turn into "match everything".
#[test]
fn like_wildcards_are_matched_literally() {
    let db = open_db("wildcards");
    for (id, name) in [
        ("a", "hello_world"),
        ("b", "helloworld"),
        ("c", "progress 100%"),
        ("d", "progress complete"),
    ] {
        let p = support::project(id.into(), name);
        db.upsert_project(&p).unwrap();
        db.index_project(&p).unwrap();
    }

    let underscore = search(&db, "_", 10).unwrap();
    assert_eq!(
        underscore.iter().map(|h| &h.ref_id).collect::<Vec<_>>(),
        vec!["a"],
        "`_` is a literal underscore, not a single-character wildcard"
    );

    let percent = search(&db, "%", 10).unwrap();
    assert_eq!(
        percent.iter().map(|h| &h.ref_id).collect::<Vec<_>>(),
        vec!["c"],
        "`%` is a literal percent sign, not a wildcard"
    );
}

/// A ROOT conversation message is searchable by its content (doc §32.9), with
/// `ref_id` = the message id and `parent_id` = the session id — and there is NO
/// length filter: a short message is as findable as a long one (§21, every
/// stored message IS the curated conversation).
#[test]
fn a_root_message_is_searchable_regardless_of_length() {
    let db = open_db("message");
    let long = "决定使用 SQLite 作为本地存储，因为它是单文件、零配置且可嵌入的。";
    let short = "先做迁移";
    let (_session, _member, stored) = support::seed_conversation(
        &db,
        Agent::Codex,
        "root-search-1",
        &[
            support::parsed_message("m1", SessionMessageRole::User, long),
            support::parsed_message("m2", SessionMessageRole::User, short),
        ],
    );
    db.index_new_messages(&stored).unwrap();

    let hits = search(&db, "SQLite", 20).unwrap();
    let message_hits: Vec<_> = hits.iter().filter(|h| h.kind == "message").collect();
    assert_eq!(message_hits.len(), 1, "{hits:?}");
    assert_eq!(message_hits[0].ref_id, stored[0].id);
    assert_eq!(
        message_hits[0].parent_id, _session.id,
        "a message's parent is its session"
    );

    // No length filter: the five-character message is indexed and found.
    let hits = search(&db, "迁移", 20).unwrap();
    assert!(
        hits.iter()
            .any(|h| h.kind == "message" && h.ref_id == stored[1].id),
        "a short message must be searchable: {hits:?}"
    );

    // A child member produces no messages at all, so there is nothing to
    // leak into the index: the conversation store only ever holds root turns.
    assert_eq!(
        db.message_count(&_session.id).unwrap(),
        2,
        "only the seeded root turns exist"
    );
}

/// The Logical Session's own document (`kind = 'session'`) carries the title
/// and is searchable — this is how a session without a matching message body
/// is still found (§39).
#[test]
fn a_session_is_searchable_by_its_document() {
    let db = open_db("session-doc");
    let (session, _) = db
        .upsert_logical_session_unchecked(
            Agent::Codex,
            "root-doc-1",
            Some("quant script debug"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

    let hits = search(&db, "quant", 20).unwrap();
    let session_hits: Vec<_> = hits.iter().filter(|h| h.kind == "session").collect();
    assert_eq!(session_hits.len(), 1, "{hits:?}");
    assert_eq!(session_hits[0].ref_id, session);
    assert_eq!(session_hits[0].title, "quant script debug");

    // An untitled session is still documented, under the agent's name.
    let (untitled, _) = db
        .upsert_logical_session_unchecked(
            Agent::Codex,
            "root-doc-2",
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
    let hits = search(&db, "codex", 20).unwrap();
    assert!(
        hits.iter()
            .any(|h| h.kind == "session" && h.ref_id == untitled),
        "an untitled session falls back to the agent name as its title: {hits:?}"
    );
}

/// Lifecycle × search (doc §32.7/§32.9): trashing unindexes the session's
/// messages and its own document; restoring reindexes the same durable rows.
#[test]
fn trash_hides_a_session_and_restore_brings_it_back() {
    let db = open_db("lifecycle");
    let (_session, _member, stored) = support::seed_conversation(
        &db,
        Agent::Codex,
        "root-lifecycle-1",
        &[support::parsed_message(
            "m1",
            SessionMessageRole::User,
            "the sqlite storage decision lives here",
        )],
    );
    db.index_new_messages(&stored).unwrap();
    assert!(
        search(&db, "sqlite", 20)
            .unwrap()
            .iter()
            .any(|h| h.kind == "message"),
        "searchable before trash"
    );

    lifecycle::trash_session(&db, &_session.id).unwrap();
    let hits = search(&db, "sqlite", 20).unwrap();
    assert!(
        !hits
            .iter()
            .any(|h| h.kind == "message" && h.ref_id == stored[0].id),
        "trash unindexes the messages: {hits:?}"
    );
    assert!(
        !hits
            .iter()
            .any(|h| h.kind == "session" && h.ref_id == _session.id),
        "trash unindexes the session document too: {hits:?}"
    );

    lifecycle::restore_session(&db, &_session.id).unwrap();
    let hits = search(&db, "sqlite", 20).unwrap();
    assert!(
        hits.iter()
            .any(|h| h.kind == "message" && h.ref_id == stored[0].id),
        "restore reindexes the durable rows: {hits:?}"
    );
    // The session DOCUMENT is rebuilt as well — an untitled session's document
    // title falls back to the agent name, which is how it is findable.
    let hits = search(&db, "codex", 20).unwrap();
    assert!(
        hits.iter()
            .any(|h| h.kind == "session" && h.ref_id == _session.id),
        "restore rebuilds the session document: {hits:?}"
    );
}

/// The read-side guard is the second half of the lifecycle invariant: a stale
/// index row left behind by an interrupted write must not surface a trashed or
/// deleted session in search (review P1-1).
#[test]
fn the_read_side_guard_hides_rows_without_a_live_session() {
    let db = open_db("stale-guard");
    let (session, _member, stored) = support::seed_conversation(
        &db,
        Agent::Codex,
        "root-stale-1",
        &[support::parsed_message(
            "m1",
            SessionMessageRole::User,
            "unique stale marker text",
        )],
    );
    db.index_new_messages(&stored).unwrap();

    // The session goes to the trash: the write side unindexes, and even a
    // row that re-appears afterwards stays hidden while the trash holds.
    lifecycle::trash_session(&db, &session.id).unwrap();
    db.write()
        .execute(
            "INSERT INTO search_index (kind, ref_id, parent_id, title, body)
             VALUES ('message', 'stale-msg', ?1, '', 'unique stale marker text')",
            rusqlite::params![session.id],
        )
        .unwrap();
    db.write()
        .execute(
            "INSERT INTO search_index (kind, ref_id, parent_id, title, body)
             VALUES ('session', ?1, '', 'unique stale marker title', '')",
            rusqlite::params![session.id],
        )
        .unwrap();

    let hits = search(&db, "unique stale marker", 20).unwrap();
    assert!(
        !hits.iter().any(|h| h.ref_id == "stale-msg"),
        "a stale message row of a trashed session must never surface: {hits:?}"
    );
    assert!(
        !hits
            .iter()
            .any(|h| h.kind == "session" && h.ref_id == session.id),
        "a stale session row of a trashed session must never surface: {hits:?}"
    );

    // A row whose session is GONE entirely (purged, crashed mid-write) is
    // equally invisible.
    db.write()
        .execute(
            "INSERT INTO search_index (kind, ref_id, parent_id, title, body)
             VALUES ('message', 'stale-gone', 'sess-gone', '', 'unique stale marker text')",
            [],
        )
        .unwrap();
    let hits = search(&db, "unique stale marker", 20).unwrap();
    assert!(
        !hits.iter().any(|h| h.ref_id == "stale-gone"),
        "a row with no backing session must never surface: {hits:?}"
    );

    // Restore makes the durable rows visible again — the guard reads the
    // live lifecycle state, it does not guess from the index.
    lifecycle::restore_session(&db, &session.id).unwrap();
    let hits = search(&db, "unique stale marker", 20).unwrap();
    assert!(
        hits.iter()
            .any(|h| h.kind == "message" && h.ref_id == stored[0].id),
        "restore reindexes and the guard lets the live rows through: {hits:?}"
    );
}
