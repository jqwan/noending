//! Session lifecycle — Trash / Restore / permanent LOCAL delete.
//!
//! Trash freezes a session: no member ingestion commits against it, it leaves
//! search, cannot resume, and never touches the Agent source file (NoEnding never
//! deletes an Agent-owned source). Restore keeps the same id, Owner, messages,
//! cursors and Context frontier, then reindexes. Permanent delete is a
//! NoEnding-LOCAL purge (no job, no filesystem step, no crash recovery), allowed
//! only for a TRASHED session whose ROOT source the adapter freshly confirmed
//! Missing; it removes exactly the session-owned rows, redacts Context provenance
//! in place, and leaves no tombstone.
//!
//! Adapter verdicts are driven with REAL files via `inspect_file_source`; only
//! temp dirs are used, never the real ~/.codex, ~/.claude or ~/.pi.

use noending::domain::diagnostic_kind;
use noending::domain::{
    Agent, LaunchIntent, MemberStatsDelta, SessionMemberRelation, SessionMessageRole,
    SourceAvailability, StatsUpdate,
};
use noending::lifecycle;
use noending::search;
use noending::storage::{diagnostic_key, new_id, now, Db};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

mod support;

// fixtures

fn unique_dir(tag: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "noending-lifecycle-{}-{}-{}",
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

fn count(db: &Db, sql: &str, param: &str) -> i64 {
    db.read()
        .query_row(sql, [param], |r| r.get::<_, i64>(0))
        .unwrap()
}

/// A temp directory removed when the test ends.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        Self(unique_dir(tag))
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A Logical Session with a root member whose source is a REAL file at
/// `dir/session.jsonl` — the file the adapter verdicts read, and the file the
/// purge must never touch. Pass `source_kind = Directory` to get the
/// deterministic Unavailable verdict instead (a non-regular file).
enum SourceKind {
    File,
    Directory,
}

fn seeded_session(
    db: &Db,
    dir: &TempDir,
    source: SourceKind,
) -> (noending::domain::Session, String) {
    let root_id = format!("root-{}", new_id());
    let source_path = match source {
        SourceKind::File => {
            let file = dir.path().join("session.jsonl");
            std::fs::write(&file, "agent-owned transcript fixture\n").unwrap();
            file
        }
        // A directory: symlink_metadata succeeds, is_file() is false → the
        // adapter answers Unavailable (any doubt ≠ missing,).
        SourceKind::Directory => dir.path().join("source-dir"),
    };
    if matches!(source, SourceKind::Directory) {
        std::fs::create_dir_all(&source_path).unwrap();
    }
    let s = support::ensure_session(db, format!("s-{}", new_id()), Agent::Codex, &root_id);
    let member = support::ensure_root_member(
        db,
        &s.id,
        Agent::Codex,
        &root_id,
        &source_path.to_string_lossy(),
    );
    (s, member)
}

/// Commit a one-message batch through the production path and index it.
fn commit_message(
    db: &Db,
    session_id: &str,
    member_id: &str,
    text: &str,
    offset: u64,
) -> Vec<noending::domain::SessionMessage> {
    let messages = vec![support::parsed_message(
        format!("m-{}", new_id()),
        SessionMessageRole::User,
        text,
    )];
    let stored = db
        .commit_member_ingest(
            session_id,
            member_id,
            &messages,
            None,
            &noending::domain::SourceCursorUpdate {
                file_identity: format!("identity-{}", offset),
                generation: 1,
                byte_offset: offset,
                last_seen_size: offset,
                mtime: None,
                start_byte_offset: if offset == 100 { 0 } else { 100 },
                prefix_hash: String::new(),
            },
        )
        .unwrap();
    if !stored.is_empty() {
        db.index_new_messages(&stored).unwrap();
    }
    stored
}

fn workstream(db: &Db, title: &str) -> noending::domain::Workstream {
    let w = support::workstream(format!("ws-{}", new_id()), title);
    db.upsert_workstream(&w).unwrap();
    w
}

fn set_owner(db: &Db, session_id: &str, ws_id: &str) {
    noending::workspace::session::set_session_owner(db, session_id, Some(ws_id)).unwrap();
}

/// Commit the Session Context frontier through the v1 commit path — what an
/// explicit "更新摘要" writes. Only the frontier is asserted here, so the
/// summary fields stay at their defaults.
fn set_context_frontier(db: &Db, session_id: &str, seq: i64) {
    db.tx(|tx| {
        noending::storage::commit_session_context_conn(
            tx,
            session_id,
            &noending::domain::SessionContextFields::default(),
            0,
            0,
            seq,
        )
    })
    .unwrap();
}

/// The Session Context's processed-through frontier, or 0 when the Session has
/// never had a summary generated.
fn context_frontier(db: &Db, session_id: &str) -> i64 {
    db.get_session_context(session_id)
        .unwrap()
        .map(|c| c.processed_through_seq)
        .unwrap_or(0)
}

fn launch_intent(db: &Db, session_id: &str, agent: Agent) {
    db.insert_launch_intent(&LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent,
        owner_workstream_id: None,
        cwd: None,
        process_id: Some(1),
        launched_at: now(),
        matched_session_id: Some(session_id.to_string()),
        status: "matched".into(),
        note: String::new(),
        created_at: now(),
        updated_at: now(),
    })
    .unwrap();
}

/// A context item whose head revision points at one of the session's messages
/// (current provenance spelling).
fn context_item_pointing_at(
    db: &Db,
    ws_id: &str,
    session_id: &str,
) -> (String, noending::domain::SessionMessage) {
    let messages = db.get_messages(session_id, None, 10).unwrap();
    let message = messages.first().expect("a committed message").clone();
    let message_ref = format!("session-message:{}", message.id);
    let item = noending::sync::create_item(
        db,
        ws_id,
        "decision",
        "Use SQLite",
        "storage decision from the session",
        "agent_statement",
        "session_message",
        &[message_ref],
        "agent",
    )
    .unwrap();
    (item.id, message)
}

// lifecycle trash / restore

#[test]
fn trash_freezes_the_session_and_keeps_every_fact() {
    let db = open_db("trash-freezes");
    let dir = TempDir::new("trash-freezes-src");
    let (s, member) = seeded_session(&db, &dir, SourceKind::File);
    let source_file = {
        let m = db.get_member(&member).unwrap().unwrap();
        PathBuf::from(&m.source_path)
    };
    commit_message(&db, &s.id, &member, "决定使用 SQLite 存储", 100);
    let ws = workstream(&db, "WS");
    set_owner(&db, &s.id, &ws.id);
    set_context_frontier(&db, &s.id, 1);

    let cursor_before = db.get_member_cursor(&member).unwrap();
    let trashed = lifecycle::trash_session(&db, &s.id).unwrap();
    assert!(
        trashed.trashed_at.is_some(),
        "the flip is visible on the row"
    );
    assert!(trashed.is_trashed());

    // The Agent source is untouched — NoEnding never deletes it.
    assert!(
        source_file.exists(),
        "trash must never touch the Agent source"
    );

    // NoEnding data untouched: messages, owner, cursor, frontier all survive.
    assert_eq!(db.message_count(&s.id).unwrap(), 1);
    assert_eq!(
        db.get_session(&s.id).unwrap().unwrap().owner_workstream_id,
        Some(ws.id.clone()),
        "Trash keeps the Owner Workstream (Trash Session → Owner 保留)"
    );
    let cursor_after = db.get_member_cursor(&member).unwrap();
    assert_eq!(
        cursor_before.byte_offset, cursor_after.byte_offset,
        "cursor frozen"
    );
    assert_eq!(
        cursor_before.identity_tail_hash,
        cursor_after.identity_tail_hash
    );
    assert_eq!(
        context_frontier(&db, &s.id),
        1,
        "the Context frontier is member-lifecycle-independent"
    );
}

#[test]
fn trash_is_hidden_from_list_projections_and_cards() {
    let db = open_db("list-scope");
    let dir = TempDir::new("list-scope-src");
    let (s, _member) = seeded_session(&db, &dir, SourceKind::File);
    let ws = workstream(&db, "WS");
    set_owner(&db, &s.id, &ws.id);

    let active = |db: &Db, scope| {
        db.list_sessions(noending::storage::SessionFilter {
            scope,
            ..Default::default()
        })
        .unwrap()
    };
    use noending::domain::SessionListScope as Scope;
    assert!(active(&db, Scope::Active).iter().any(|x| x.id == s.id));

    let (count_before, latest_before, _) = db.workstream_session_stats(&ws.id).unwrap();
    assert_eq!(count_before, 1);
    assert!(latest_before.is_some());

    lifecycle::trash_session(&db, &s.id).unwrap();

    assert!(
        !active(&db, Scope::Active).iter().any(|x| x.id == s.id),
        "trash hidden from the default list"
    );
    let trash = active(&db, Scope::Trash);
    assert!(trash.iter().any(|x| x.id == s.id));
    let all = active(&db, Scope::All);
    assert!(all.iter().any(|x| x.id == s.id));

    // Workstream cards no longer count the trashed session.
    let (count_after, latest_after, _) = db.workstream_session_stats(&ws.id).unwrap();
    assert_eq!(count_after, 0);
    assert!(latest_after.is_none());
}

///  — an in-flight member batch that tries to commit after a Trash
/// stores NOTHING: no message, no stats, no cursor move. A Restore resumes
/// from the untouched cursor and picks up what was suppressed.
#[test]
fn inflight_ingest_cannot_commit_after_trash_and_resumes_after_restore() {
    let db = open_db("inflight-guard");
    let dir = TempDir::new("inflight-src");
    let (s, member) = seeded_session(&db, &dir, SourceKind::File);
    commit_message(&db, &s.id, &member, "first round message", 100);
    let cursor_before = db.get_member_cursor(&member).unwrap();

    // T1: the user trashes while the next batch is in flight…
    lifecycle::trash_session(&db, &s.id).unwrap();

    // …T2: the staged batch tries to commit and is rejected.
    let stored = commit_message(&db, &s.id, &member, "second round message", 200);
    assert!(stored.is_empty(), "a trashed session takes no messages");

    assert_eq!(db.message_count(&s.id).unwrap(), 1);
    let cursor_after = db.get_member_cursor(&member).unwrap();
    assert_eq!(
        cursor_before.byte_offset, cursor_after.byte_offset,
        "cursor frozen"
    );
    assert_eq!(
        cursor_before.identity_tail_hash, cursor_after.identity_tail_hash,
        "the identity chain tail is frozen too"
    );

    // Restore → the next commit resumes from the untouched cursor.
    lifecycle::restore_session(&db, &s.id).unwrap();
    let stored = commit_message(&db, &s.id, &member, "second round message", 200);
    assert_eq!(
        stored.len(),
        1,
        "the suppressed batch commits after restore"
    );
    assert_eq!(db.message_count(&s.id).unwrap(), 2);
}

/// Restore keeps the same id/Owner/messages/cursors/frontier and
/// reindexes the conversation.
#[test]
fn restore_keeps_identity_data_and_reindexes() {
    let db = open_db("restore-keeps");
    let dir = TempDir::new("restore-src");
    let (s, member) = seeded_session(&db, &dir, SourceKind::File);
    let stored = commit_message(&db, &s.id, &member, "the sqlite decision message", 100);
    let ws = workstream(&db, "WS");
    set_owner(&db, &s.id, &ws.id);
    set_context_frontier(&db, &s.id, 1);
    let cursor_before = db.get_member_cursor(&member).unwrap();

    lifecycle::trash_session(&db, &s.id).unwrap();
    let restored = lifecycle::restore_session(&db, &s.id).unwrap();

    // : Trash → Restore keeps the same identity and every fact.
    assert_eq!(restored.id, s.id);
    assert!(restored.trashed_at.is_none());
    assert_eq!(restored.owner_workstream_id, Some(ws.id));
    let messages = db.get_messages(&s.id, None, 100).unwrap();
    assert_eq!(
        messages.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        stored.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        "the same messages, same app-owned ids"
    );
    let cursor_after = db.get_member_cursor(&member).unwrap();
    assert_eq!(cursor_before.byte_offset, cursor_after.byte_offset);
    assert_eq!(
        context_frontier(&db, &s.id),
        1,
        "the frontier was never touched"
    );

    // The conversation is searchable again (reindex inside the restore tx).
    let hits = search::search(&db, "sqlite", 20).unwrap();
    assert!(
        hits.iter()
            .any(|h| h.kind == "message" && h.ref_id == stored[0].id),
        "restore reindexes the messages: {:?}",
        hits.iter().map(|h| h.ref_id.clone()).collect::<Vec<_>>()
    );
}

/// Resume is refused for a trashed session; the launcher gate is the
/// single funnel every resume entry goes through.
#[test]
fn trashed_session_cannot_resume() {
    let db = open_db("no-resume");
    let dir = TempDir::new("no-resume-src");
    let (s, _member) = seeded_session(&db, &dir, SourceKind::File);
    lifecycle::trash_session(&db, &s.id).unwrap();

    let launcher = noending::launcher::SessionLauncher {
        runtime_dir: unique_dir("no-resume-runtime"),
    };
    let workspace = noending::launcher::LaunchWorkspace {
        default_workspace: None,
    };
    let err = launcher
        .prepare_resume_in(&db, &s.id, &workspace)
        .unwrap_err();
    assert!(err.to_string().contains("回收站"), "unexpected: {err}");
}

/// An ACTIVE session with a freshly Missing root must NOT be purgeable —
/// whatever the source state, only Trash heads toward deletion (row 1).
#[test]
fn active_session_is_never_purgeable_even_when_root_is_missing() {
    let db = open_db("active-no-purge");
    let dir = TempDir::new("active-purge-src");
    let (s, _member) = seeded_session(&db, &dir, SourceKind::File);
    let source_file = {
        let root = db.root_member_for_session(&s.id).unwrap().unwrap();
        PathBuf::from(&root.source_path)
    };

    // The source disappears (user rm, sync tool) while the session is ACTIVE.
    std::fs::remove_file(&source_file).unwrap();
    assert_eq!(
        lifecycle::root_source_status(&db, &db.get_session(&s.id).unwrap().unwrap()).unwrap(),
        Some(SourceAvailability::Missing),
        "fixture sanity: the adapter confirms the root missing"
    );

    let err = lifecycle::get_session_local_delete_preview(&db, &s.id).unwrap_err();
    assert!(err.to_string().contains("回收站"), "unexpected: {err}");
    assert!(
        lifecycle::permanently_delete_session(&db, &s.id).is_err(),
        "an active session is never purgeable"
    );
    assert!(db.get_session(&s.id).unwrap().is_some());
}

/// row 2 — Trash + Root Present refuses the purge.
#[test]
fn trashed_with_root_present_refuses_the_purge() {
    let db = open_db("present-refuses");
    let dir = TempDir::new("present-src");
    let (s, _member) = seeded_session(&db, &dir, SourceKind::File);
    let source_file = {
        let root = db.root_member_for_session(&s.id).unwrap().unwrap();
        PathBuf::from(&root.source_path)
    };
    lifecycle::trash_session(&db, &s.id).unwrap();

    let preview = lifecycle::get_session_local_delete_preview(&db, &s.id).unwrap();
    assert_eq!(preview.root_source_status, SourceAvailability::Present);
    assert!(
        !preview.can_permanently_delete,
        "a present source is living data — no purge offered"
    );
    let err = lifecycle::permanently_delete_session(&db, &s.id).unwrap_err();
    assert!(err.to_string().contains("missing"), "unexpected: {err}");

    assert!(
        db.get_session(&s.id).unwrap().is_some(),
        "nothing was purged"
    );
    assert!(source_file.exists(), "the file was never touched");
}

/// row 3 — Trash + Root Unavailable refuses: any doubt ≠ missing.
/// The deterministic Unavailable fixture is a DIRECTORY at the source path
/// (`inspect_file_source`: non-regular file → Unavailable).
#[test]
fn trashed_with_root_unavailable_refuses_the_purge() {
    let db = open_db("unavailable-refuses");
    let dir = TempDir::new("unavailable-src");
    let (s, _member) = seeded_session(&db, &dir, SourceKind::Directory);
    let source_path = {
        let root = db.root_member_for_session(&s.id).unwrap().unwrap();
        PathBuf::from(&root.source_path)
    };
    assert!(source_path.is_dir(), "fixture sanity");
    lifecycle::trash_session(&db, &s.id).unwrap();

    let preview = lifecycle::get_session_local_delete_preview(&db, &s.id).unwrap();
    assert_eq!(preview.root_source_status, SourceAvailability::Unavailable);
    assert!(!preview.can_permanently_delete);

    let err = lifecycle::permanently_delete_session(&db, &s.id).unwrap_err();
    assert!(err.to_string().contains("missing"), "unexpected: {err}");
    assert!(db.get_session(&s.id).unwrap().is_some());
    assert!(source_path.is_dir(), "nothing on disk was touched");
}

/// rows 4-5 — the full purge: Trash + fresh Missing, execute re-checks
/// the source freshly, and ONE transaction removes exactly the session-owned
/// rows while redacting provenance in place. Content, authority and actor
/// survive; another session's identical-looking context is untouched.
#[test]
fn permanent_delete_purges_local_rows_only() {
    let db = open_db("full-purge");
    let dir = TempDir::new("purge-src");
    let (s, member) = seeded_session(&db, &dir, SourceKind::File);
    let source_file = {
        let root = db.root_member_for_session(&s.id).unwrap().unwrap();
        PathBuf::from(&root.source_path)
    };
    commit_message(&db, &s.id, &member, "决定使用 SQLite 存储", 100);
    let ws = workstream(&db, "Surviving WS");
    set_owner(&db, &s.id, &ws.id);
    let (item_id, _) = context_item_pointing_at(&db, &ws.id, &s.id);
    launch_intent(&db, &s.id, Agent::Codex);
    // A diagnostic describing exactly this session's root member.
    let root_source_id = {
        let root = db.root_member_for_session(&s.id).unwrap().unwrap();
        root.source_member_id
    };
    db.upsert_ingestion_diagnostic(
        Agent::Codex,
        diagnostic_kind::UNRESOLVED_SESSION_MEMBER,
        Some(&root_source_id),
        None,
        Some(source_file.to_string_lossy().as_ref()),
        "fixture",
        &serde_json::json!({}),
    )
    .unwrap();
    // Stats via the production delta path.
    db.commit_member_ingest(
        &s.id,
        &member,
        &[],
        Some(StatsUpdate::Delta(MemberStatsDelta {
            tool_call_count: Some(3),
            ..Default::default()
        })),
        &noending::domain::SourceCursorUpdate {
            file_identity: "identity".into(),
            generation: 1,
            byte_offset: 100,
            last_seen_size: 100,
            mtime: None,
            start_byte_offset: 100,
            prefix_hash: String::new(),
        },
    )
    .unwrap();
    assert!(
        db.get_member_stats(&member).unwrap().is_some(),
        "fixture sanity"
    );
    set_context_frontier(&db, &s.id, 1);

    // …another session's context must NOT be redacted.
    let other_dir = TempDir::new("purge-other-src");
    let (other, other_member) = seeded_session(&db, &other_dir, SourceKind::File);
    commit_message(&db, &other.id, &other_member, "另一个会话的消息", 100);
    let (other_item, _) = context_item_pointing_at(&db, &ws.id, &other.id);

    // Trashed + root missing → purge allowed.
    lifecycle::trash_session(&db, &s.id).unwrap();
    std::fs::remove_file(&source_file).unwrap();

    let preview = lifecycle::get_session_local_delete_preview(&db, &s.id).unwrap();
    assert_eq!(preview.root_source_status, SourceAvailability::Missing);
    assert!(preview.can_permanently_delete);
    assert_eq!(preview.counts.message_count, 1);
    assert_eq!(preview.counts.member_count, 1);
    assert!(preview.counts.session_context_count >= 1);
    assert_eq!(preview.counts.launch_intent_count, 1);
    assert!(
        preview.counts.context_revision_redaction_count >= 1,
        "the item revision pointing at this session is counted: {:?}",
        preview.counts
    );

    let result = lifecycle::permanently_delete_session(&db, &s.id).unwrap();
    assert!(result.purged);
    assert!(result.redacted_revisions >= 1);

    // Every session-owned row is gone ('s fixed order).
    assert!(db.get_session(&s.id).unwrap().is_none(), "session row gone");
    assert!(db.root_member_for_session(&s.id).unwrap().is_none());
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM session_members WHERE session_id = ?",
            &s.id
        ),
        0,
        "members gone"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM session_messages WHERE session_id = ?",
            &s.id
        ),
        0,
        "conversation gone"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM session_member_cursors WHERE member_id = ?",
            &member
        ),
        0,
        "member cursors gone"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM session_member_stats WHERE member_id = ?",
            &member
        ),
        0,
        "member stats gone"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM session_contexts WHERE session_id = ?",
            &s.id
        ),
        0,
        "session summary gone"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM session_context_revisions WHERE session_id = ?",
            &s.id
        ),
        0,
        "summary revision history gone"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM launch_intents WHERE matched_session_id = ?",
            &s.id
        ),
        0,
        "matched launch history gone"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM ingestion_diagnostics WHERE diagnostic_key = ?",
            &diagnostic_key(
                Agent::Codex,
                diagnostic_kind::UNRESOLVED_SESSION_MEMBER,
                Some(&root_source_id)
            )
        ),
        0,
        "diagnostics of this session's members are gone"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM search_index WHERE parent_id = ?",
            &s.id
        ),
        0,
        "no FTS row survives the purge"
    );

    // Surviving context: content preserved, provenance redacted.
    let rev_id: String = db
        .read()
        .query_row(
            "SELECT current_revision_id FROM context_items WHERE id = ?1",
            [&item_id],
            |r| r.get(0),
        )
        .unwrap();
    let source = db.get_context_revision_source(&rev_id).unwrap().unwrap();
    assert_eq!(source.source_type.as_deref(), Some("deleted_session"));
    assert!(source.source_ref.is_none(), "the message ref dies");
    assert!(source.session_id.is_none(), "no session may resolve");
    let rev = db.get_revision(&rev_id).unwrap().unwrap();
    assert_eq!(rev.title, "Use SQLite", "the title survives");
    assert_eq!(
        rev.content, "storage decision from the session",
        "the content survives"
    );
    // The redaction keeps authority/actor — the provenance the authority
    // resolver reads is scrubbed of refs only.
    let metadata: serde_json::Value = rev.metadata;
    assert_eq!(
        metadata["provenance"]["authority"], "agent_statement",
        "authority survives the redaction"
    );
    assert_eq!(metadata["provenance"]["actor"], "agent", "actor survives");
    assert_eq!(
        metadata["provenance"]["source_type"], "deleted_session",
        "the redaction is recorded in the provenance too"
    );

    // The OTHER session's identical-looking context is untouched, its source
    // file still on disk, and the Workstream survives.
    let other_rev_id: String = db
        .read()
        .query_row(
            "SELECT current_revision_id FROM context_items WHERE id = ?1",
            [&other_item],
            |r| r.get(0),
        )
        .unwrap();
    let other_source = db
        .get_context_revision_source(&other_rev_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        other_source.source_type.as_deref(),
        Some("session_message"),
        "only the dying session's provenance is redacted"
    );
    assert!(db.get_session(&other.id).unwrap().is_some());
    assert!(db.get_workstream(&ws.id).unwrap().is_some());
}

/// row 5 — execute takes the source verdict FRESHLY: a file that
/// reappeared between preview and execute aborts the purge.
#[test]
fn execute_rechecks_the_source_and_a_reappearing_file_blocks_the_purge() {
    let db = open_db("fresh-recheck");
    let dir = TempDir::new("recheck-src");
    let (s, _member) = seeded_session(&db, &dir, SourceKind::File);
    let source_file = {
        let root = db.root_member_for_session(&s.id).unwrap().unwrap();
        PathBuf::from(&root.source_path)
    };

    lifecycle::trash_session(&db, &s.id).unwrap();
    std::fs::remove_file(&source_file).unwrap();
    let preview = lifecycle::get_session_local_delete_preview(&db, &s.id).unwrap();
    assert!(preview.can_permanently_delete, "preview saw Missing");

    // The user restores the file from backup before confirming.
    std::fs::write(&source_file, "the source came back\n").unwrap();

    let err = lifecycle::permanently_delete_session(&db, &s.id).unwrap_err();
    assert!(err.to_string().contains("missing"), "unexpected: {err}");
    assert!(
        db.get_session(&s.id).unwrap().is_some(),
        "nothing was purged"
    );
    assert!(
        source_file.exists(),
        "the reappeared file is never deleted by a refusal"
    );
}

/// row 6 — only the ROOT source is the deletion authority: a child
/// member's source may well still exist while the root is gone, and the purge
/// proceeds — without ever touching that child file.
#[test]
fn child_source_may_survive_when_root_is_missing() {
    let db = open_db("child-source");
    let dir = TempDir::new("child-src");
    let (s, member) = seeded_session(&db, &dir, SourceKind::File);
    let root_file = {
        let root = db.root_member_for_session(&s.id).unwrap().unwrap();
        PathBuf::from(&root.source_path)
    };
    commit_message(&db, &s.id, &member, "root conversation", 100);

    // A child member with its own, still-existing source file.
    let child_file = dir.path().join("child.jsonl");
    std::fs::write(&child_file, "child transcript\n").unwrap();
    db.upsert_session_member(
        &s.id,
        Agent::Codex,
        "child-source-member",
        SessionMemberRelation::Child,
        Some(&s.root_agent_session_id),
        "subagent_file",
        &child_file.to_string_lossy(),
        None,
        None,
        None,
        &serde_json::json!({}),
    )
    .unwrap();

    lifecycle::trash_session(&db, &s.id).unwrap();
    std::fs::remove_file(&root_file).unwrap();

    let result = lifecycle::permanently_delete_session(&db, &s.id).unwrap();
    assert!(
        result.purged,
        "the root is the authority — the purge proceeds"
    );
    assert!(db.get_session(&s.id).unwrap().is_none());
    assert!(
        child_file.exists(),
        "the child's Agent source file is NEVER touched — there is no filesystem step"
    );
}

/// no tombstone: after a purge the root identity is free, and a
/// reappearing source may be re-ingested as a NEW session with no Owner.
#[test]
fn a_purged_root_may_be_reingested_as_a_new_session() {
    let db = open_db("no-tombstone");
    let dir = TempDir::new("tombstone-src");
    let (s, member) = seeded_session(&db, &dir, SourceKind::File);
    let root_id = s.root_agent_session_id.clone();
    let source_file = {
        let root = db.root_member_for_session(&s.id).unwrap().unwrap();
        PathBuf::from(&root.source_path)
    };
    commit_message(&db, &s.id, &member, "the original conversation", 100);
    let ws = workstream(&db, "WS");
    set_owner(&db, &s.id, &ws.id);

    lifecycle::trash_session(&db, &s.id).unwrap();
    std::fs::remove_file(&source_file).unwrap();
    lifecycle::permanently_delete_session(&db, &s.id).unwrap();

    assert!(
        db.find_session_by_root_agent_id(Agent::Codex, &root_id)
            .unwrap()
            .is_none(),
        "the identity is free — nothing remembers the old session"
    );

    // The source comes back (backup restore): discovery re-ingests it as a
    // brand-new Logical Session.
    std::fs::write(&source_file, "the source came back\n").unwrap();
    let (s2, is_new) = db
        .upsert_logical_session_unchecked(
            Agent::Codex,
            &root_id,
            Some("fresh start"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
    assert!(is_new, "rediscovery creates a fresh lifecycle");
    assert_ne!(s2, s.id);
    let stored = db.get_session(&s2).unwrap().unwrap();
    assert!(
        stored.owner_workstream_id.is_none(),
        "no old Owner resurrects"
    );
}

// Review P1-1: search stays lifecycle-authoritative across restarts

/// The startup backfill must not re-index what a Trash unindexed: only ACTIVE
/// sessions are ever filled in, so the recycle bin cannot leak back into
/// search after a restart.
#[test]
fn startup_backfill_skips_trashed_sessions() {
    let db = open_db("backfill-trashed");
    let dir_a = TempDir::new("backfill-a-src");
    let dir_b = TempDir::new("backfill-b-src");
    let (active, active_member) = seeded_session(&db, &dir_a, SourceKind::File);
    let (trashed, trashed_member) = seeded_session(&db, &dir_b, SourceKind::File);
    let active_stored = commit_message(
        &db,
        &active.id,
        &active_member,
        "unique active marker goals",
        100,
    );
    let trashed_stored = commit_message(
        &db,
        &trashed.id,
        &trashed_member,
        "unique trashed marker goals",
        100,
    );

    lifecycle::trash_session(&db, &trashed.id).unwrap();
    // 模拟重启：startup backfill 不得把回收站加回来。
    db.backfill_search_index().unwrap();

    let hits = search::search(&db, "goals", 20).unwrap();
    assert!(
        hits.iter()
            .any(|h| h.kind == "message" && h.ref_id == active_stored[0].id),
        "the active session stays searchable"
    );
    assert!(
        !hits
            .iter()
            .any(|h| h.kind == "message" && h.ref_id == trashed_stored[0].id),
        "the trashed session must not be re-indexed by the startup backfill"
    );

    // Restore 之后回到索引 —— 生命周期与索引同进退。
    lifecycle::restore_session(&db, &trashed.id).unwrap();
    let hits = search::search(&db, "goals", 20).unwrap();
    assert!(
        hits.iter()
            .any(|h| h.kind == "message" && h.ref_id == trashed_stored[0].id),
        "restore brings the conversation back into search"
    );
}

/// A stale index row whose session is gone entirely (or trashed) never
/// surfaces: the read-side guard is the lifecycle authority in search.
#[test]
fn search_filters_stale_rows_of_dead_sessions() {
    let db = open_db("stale-rows");
    db.write()
        .execute(
            "INSERT INTO search_index (kind, ref_id, parent_id, title, body)
             VALUES ('message', 'stale:1', 'sess-gone', '', 'unique stale marker text')",
            [],
        )
        .unwrap();

    let hits = search::search(&db, "unique stale marker", 20).unwrap();
    assert!(
        !hits.iter().any(|h| h.ref_id == "stale:1"),
        "stale rows must never surface: {:?}",
        hits.iter().map(|h| h.ref_id.clone()).collect::<Vec<_>>()
    );
}
