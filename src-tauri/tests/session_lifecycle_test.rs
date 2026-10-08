//! Session lifecycle — Trash / Restore / permanent LOCAL delete.
//!
//! Trash is a thin filter: it decides which lists a Session appears in and
//! freezes its Context extraction. Everything else keeps following the source —
//! ingestion still commits, ownership still matches, and the Agent source
//! file is never touched (NoEnding never deletes an Agent-owned source). Restore
//! keeps the same id, Owner, messages, cursors and Context frontier, then
//! reindexes. Permanent delete is a NoEnding-LOCAL purge (no job, no filesystem
//! step, no crash recovery), allowed only for a TRASHED session whose ROOT source
//! the adapter freshly confirmed Missing; it removes exactly the session-owned
//! rows, redacts Context provenance in place, and leaves no tombstone.
//!
//! Adapter verdicts are driven with REAL files via `inspect_file_source`; only
//! temp dirs are used, never the real ~/.codex, ~/.claude or ~/.pi.

use noending::domain::{Agent, LaunchIntent, SessionMessageRole, SourceAvailability};
use noending::lifecycle;
use noending::search;
use noending::storage::{new_id, now, Db};
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

/// A Logical Session whose root source is a REAL file at `dir/session.jsonl`
/// — the file the adapter verdicts read, and the file the purge must never
/// touch. Pass `source_kind = Directory` to get the deterministic Unavailable
/// verdict instead (a non-regular file).
enum SourceKind {
    File,
    Directory,
}

fn seeded_session(db: &Db, dir: &TempDir, source: SourceKind) -> noending::domain::Session {
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
    support::ensure_session_source(db, Agent::Codex, &root_id, &source_path.to_string_lossy());
    db.find_session_by_root_agent_id(Agent::Codex, &root_id)
        .unwrap()
        .expect("seeded session")
}

/// Commit a one-message batch through the production path and index it.
fn commit_message(
    db: &Db,
    session_id: &str,
    text: &str,
    offset: u64,
) -> Vec<noending::domain::SessionMessage> {
    let messages = vec![support::parsed_message(
        format!("m-{}", new_id()),
        SessionMessageRole::User,
        text,
    )];
    let stored = db
        .commit_ingest(
            session_id,
            &messages,
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
fn archive_keeps_every_fact_and_never_touches_the_source() {
    let db = open_db("trash-freezes");
    let dir = TempDir::new("trash-freezes-src");
    let s = seeded_session(&db, &dir, SourceKind::File);
    let source_file = PathBuf::from(&s.source_path);
    commit_message(&db, &s.id, "决定使用 SQLite 存储", 100);
    let ws = workstream(&db, "WS");
    set_owner(&db, &s.id, &ws.id);
    set_context_frontier(&db, &s.id, 1);

    let cursor_before = db.get_session(&s.id).unwrap().unwrap().source_cursor();
    let trashed = lifecycle::archive_session(&db, &s.id).unwrap();
    assert!(
        trashed.archived_at.is_some(),
        "the flip is visible on the row"
    );
    assert!(trashed.is_archived());

    // The Agent source is untouched — NoEnding never deletes it.
    assert!(
        source_file.exists(),
        "trash must never touch the Agent source"
    );

    // The flip itself rewrites nothing: messages, owner, cursor and frontier
    // are all exactly as they were.
    assert_eq!(db.message_count(&s.id).unwrap(), 1);
    assert_eq!(
        db.get_session(&s.id).unwrap().unwrap().owner_workstream_id,
        Some(ws.id.clone()),
        "Trash keeps the Owner Workstream (Trash Session → Owner 保留)"
    );
    let cursor_after = db.get_session(&s.id).unwrap().unwrap().source_cursor();
    assert_eq!(
        cursor_before.byte_offset, cursor_after.byte_offset,
        "the trash flip itself does not move the cursor"
    );
    assert_eq!(
        cursor_before.identity_tail_hash,
        cursor_after.identity_tail_hash
    );
    assert_eq!(
        context_frontier(&db, &s.id),
        1,
        "the Context frontier is session-lifecycle-independent"
    );
}

#[test]
fn archive_filters_board_but_preserves_task_projections() {
    let db = open_db("list-scope");
    let dir = TempDir::new("list-scope-src");
    let s = seeded_session(&db, &dir, SourceKind::File);
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
    assert!(active(&db, Scope::Unarchived).iter().any(|x| x.id == s.id));

    let (count_before, latest_before, _) = db.workstream_session_stats(&ws.id).unwrap();
    assert_eq!(count_before, 1);
    assert!(latest_before.is_some());

    lifecycle::archive_session(&db, &s.id).unwrap();

    assert!(
        !active(&db, Scope::Unarchived).iter().any(|x| x.id == s.id),
        "trash hidden from the default list"
    );
    let trash = active(&db, Scope::Archived);
    assert!(trash.iter().any(|x| x.id == s.id));
    let all = active(&db, Scope::All);
    assert!(all.iter().any(|x| x.id == s.id));

    // Workstream cards no longer count the trashed session.
    let (count_after, latest_after, _) = db.workstream_session_stats(&ws.id).unwrap();
    assert_eq!(count_after, count_before);
    assert_eq!(latest_after, latest_before);
    assert!(db
        .sessions_for_workstream(&ws.id)
        .unwrap()
        .iter()
        .any(|row| row.id == s.id));
}

/// Trash does not stop a commit: an in-flight batch that lands after a Trash
/// stores its messages, stats and cursor move like any other.
#[test]
fn inflight_ingest_commits_after_archive() {
    let db = open_db("inflight-guard");
    let dir = TempDir::new("inflight-src");
    let s = seeded_session(&db, &dir, SourceKind::File);
    commit_message(&db, &s.id, "first round message", 100);
    let cursor_before = db.get_session(&s.id).unwrap().unwrap().source_cursor();

    // T1: the user trashes while the next batch is in flight…
    lifecycle::archive_session(&db, &s.id).unwrap();

    // …T2: the staged batch commits anyway — the source is the authority.
    let stored = commit_message(&db, &s.id, "second round message", 200);
    assert_eq!(stored.len(), 1, "a trashed session still takes its source");

    assert_eq!(db.message_count(&s.id).unwrap(), 2);
    let cursor_after = db.get_session(&s.id).unwrap().unwrap().source_cursor();
    assert!(
        cursor_before.byte_offset < cursor_after.byte_offset,
        "the cursor advanced with the committed batch"
    );

    // Restore keeps everything the trashed Session ingested meanwhile.
    lifecycle::restore_session(&db, &s.id).unwrap();
    assert_eq!(db.message_count(&s.id).unwrap(), 2);
}

/// Restore keeps the same id/Owner/messages/cursors/frontier and
/// reindexes the conversation.
#[test]
fn unarchive_keeps_identity_data_and_search() {
    let db = open_db("restore-keeps");
    let dir = TempDir::new("restore-src");
    let s = seeded_session(&db, &dir, SourceKind::File);
    let stored = commit_message(&db, &s.id, "the sqlite decision message", 100);
    let ws = workstream(&db, "WS");
    set_owner(&db, &s.id, &ws.id);
    set_context_frontier(&db, &s.id, 1);
    let cursor_before = db.get_session(&s.id).unwrap().unwrap().source_cursor();

    lifecycle::archive_session(&db, &s.id).unwrap();
    let restored = lifecycle::restore_session(&db, &s.id).unwrap();

    // : Trash → Restore keeps the same identity and every fact.
    assert_eq!(restored.id, s.id);
    assert!(restored.archived_at.is_none());
    assert_eq!(restored.owner_workstream_id, Some(ws.id));
    let messages = db.get_messages(&s.id, None, 100).unwrap();
    assert_eq!(
        messages.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        stored.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        "the same messages, same app-owned ids"
    );
    let cursor_after = db.get_session(&s.id).unwrap().unwrap().source_cursor();
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
fn archived_session_cannot_resume() {
    let db = open_db("no-resume");
    let dir = TempDir::new("no-resume-src");
    let s = seeded_session(&db, &dir, SourceKind::File);
    lifecycle::archive_session(&db, &s.id).unwrap();

    let launcher = noending::launcher::SessionLauncher {
        runtime_dir: unique_dir("no-resume-runtime"),
    };
    let workspace = noending::launcher::LaunchWorkspace {
        default_workspace: None,
    };
    let err = launcher
        .prepare_resume_in(&db, &s.id, &workspace)
        .unwrap_err();
    assert!(err.to_string().contains("已归档"), "unexpected: {err}");
}

/// An ACTIVE session with a freshly Missing root must NOT be purgeable —
/// whatever the source state, only Trash heads toward deletion (row 1).
#[test]
fn active_session_is_never_purgeable_even_when_root_is_missing() {
    let db = open_db("active-no-purge");
    let dir = TempDir::new("active-purge-src");
    let s = seeded_session(&db, &dir, SourceKind::File);
    let source_file = PathBuf::from(&s.source_path);

    // The source disappears (user rm, sync tool) while the session is ACTIVE.
    std::fs::remove_file(&source_file).unwrap();
    assert_eq!(
        lifecycle::root_source_status(&db, &db.get_session(&s.id).unwrap().unwrap()).unwrap(),
        Some(SourceAvailability::Missing),
        "fixture sanity: the adapter confirms the root missing"
    );

    let err = lifecycle::get_session_local_delete_preview(&db, &s.id).unwrap_err();
    assert!(err.to_string().contains("已归档"), "unexpected: {err}");
    assert!(
        lifecycle::permanently_delete_session(&db, &s.id).is_err(),
        "an active session is never purgeable"
    );
    assert!(db.get_session(&s.id).unwrap().is_some());
}

/// Trash alone decides: a present or unconfirmable root is purged exactly like
/// a missing one. The source file is never touched and the identity is left
/// free, so a readable source is simply rebuilt as a new Session. (The
/// deterministic Unavailable fixture is a DIRECTORY at the source path —
/// `inspect_file_source` answers Unavailable for a non-regular file.)
#[test]
fn archived_session_is_purgeable_whatever_the_root_source_says() {
    for (tag, kind, expected) in [
        ("present", SourceKind::File, SourceAvailability::Present),
        (
            "unavailable",
            SourceKind::Directory,
            SourceAvailability::Unavailable,
        ),
    ] {
        let db = open_db(&format!("purge-{tag}"));
        let dir = TempDir::new(&format!("purge-{tag}-src"));
        let s = seeded_session(&db, &dir, kind);
        let source_path = PathBuf::from(&s.source_path);
        lifecycle::archive_session(&db, &s.id).unwrap();

        let preview = lifecycle::get_session_local_delete_preview(&db, &s.id).unwrap();
        assert_eq!(preview.root_source_status, expected, "{tag}");

        let result = lifecycle::permanently_delete_session(&db, &s.id).unwrap();
        assert!(result.purged, "{tag}");
        assert!(db.get_session(&s.id).unwrap().is_none(), "{tag}");
        assert!(
            source_path.exists(),
            "{tag}: the Agent source is never touched"
        );
        assert!(
            db.find_session_by_root_agent_id(Agent::Codex, &s.root_agent_session_id)
                .unwrap()
                .is_none(),
            "{tag}: the identity is free, so a rebuild produces a NEW session"
        );
    }
}

/// rows 4-5 — the full purge: Trash + fresh Missing, execute re-checks
/// the source freshly, and ONE transaction removes exactly the session-owned
/// rows while redacting provenance in place. Content, authority and actor
/// survive; another session's identical-looking context is untouched.
#[test]
fn permanent_delete_purges_local_rows_only() {
    let db = open_db("full-purge");
    let dir = TempDir::new("purge-src");
    let s = seeded_session(&db, &dir, SourceKind::File);
    let source_file = PathBuf::from(&s.source_path);
    commit_message(&db, &s.id, "决定使用 SQLite 存储", 100);
    let ws = workstream(&db, "Surviving WS");
    set_owner(&db, &s.id, &ws.id);
    let (item_id, _) = context_item_pointing_at(&db, &ws.id, &s.id);
    launch_intent(&db, &s.id, Agent::Codex);
    // Stats via the production delta path.
    db.commit_ingest(
        &s.id,
        &[],
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
    set_context_frontier(&db, &s.id, 1);

    // …another session's context must NOT be redacted.
    let other_dir = TempDir::new("purge-other-src");
    let other = seeded_session(&db, &other_dir, SourceKind::File);
    commit_message(&db, &other.id, "另一个会话的消息", 100);
    let (other_item, _) = context_item_pointing_at(&db, &ws.id, &other.id);

    // Trashed + root missing → purge allowed.
    lifecycle::archive_session(&db, &s.id).unwrap();
    std::fs::remove_file(&source_file).unwrap();

    let preview = lifecycle::get_session_local_delete_preview(&db, &s.id).unwrap();
    assert_eq!(preview.root_source_status, SourceAvailability::Missing);
    assert_eq!(preview.counts.message_count, 1);
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

/// A source that reappeared between preview and execute does not block the
/// purge: the verdict is copy for the dialog, not a gate.
#[test]
fn a_reappearing_source_does_not_block_the_purge() {
    let db = open_db("fresh-recheck");
    let dir = TempDir::new("recheck-src");
    let s = seeded_session(&db, &dir, SourceKind::File);
    let source_file = PathBuf::from(&s.source_path);

    lifecycle::archive_session(&db, &s.id).unwrap();
    std::fs::remove_file(&source_file).unwrap();
    let preview = lifecycle::get_session_local_delete_preview(&db, &s.id).unwrap();
    assert_eq!(preview.root_source_status, SourceAvailability::Missing);

    // The user restores the file from backup before confirming.
    std::fs::write(&source_file, "the source came back\n").unwrap();

    let result = lifecycle::permanently_delete_session(&db, &s.id).unwrap();
    assert!(result.purged);
    assert!(db.get_session(&s.id).unwrap().is_none());
    assert!(
        source_file.exists(),
        "the reappeared file is never deleted by the purge"
    );
}

/// The purge has no filesystem step: a source file that is not the purged
/// session's own root source survives untouched — and so does a sibling
/// session's file.
#[test]
fn the_purge_never_touches_other_files_on_disk() {
    let db = open_db("child-source");
    let dir = TempDir::new("child-src");
    let s = seeded_session(&db, &dir, SourceKind::File);
    let root_file = PathBuf::from(&s.source_path);
    commit_message(&db, &s.id, "root conversation", 100);

    // An unrelated file in the same temp dir (the old "child transcript"
    // spot): the purge is a LOCAL row purge with no filesystem step.
    let sibling_file = dir.path().join("sibling.jsonl");
    std::fs::write(&sibling_file, "unrelated transcript\n").unwrap();

    lifecycle::archive_session(&db, &s.id).unwrap();
    std::fs::remove_file(&root_file).unwrap();

    let result = lifecycle::permanently_delete_session(&db, &s.id).unwrap();
    assert!(
        result.purged,
        "the root is the authority — the purge proceeds"
    );
    assert!(db.get_session(&s.id).unwrap().is_none());
    assert!(
        sibling_file.exists(),
        "unrelated files are NEVER touched — there is no filesystem step"
    );
}

/// no tombstone: a purge frees the root identity, and a source that is still
/// there (the usual case) is re-discovered as a NEW Session with no Owner.
#[test]
fn a_purged_root_may_be_reingested_as_a_new_session() {
    let db = open_db("no-tombstone");
    let dir = TempDir::new("tombstone-src");
    let s = seeded_session(&db, &dir, SourceKind::File);
    let root_id = s.root_agent_session_id.clone();
    let source_file = PathBuf::from(&s.source_path);
    commit_message(&db, &s.id, "the original conversation", 100);
    let ws = workstream(&db, "WS");
    set_owner(&db, &s.id, &ws.id);

    lifecycle::archive_session(&db, &s.id).unwrap();
    lifecycle::permanently_delete_session(&db, &s.id).unwrap();

    assert!(
        source_file.exists(),
        "the source was there all along — the purge never touches it"
    );
    assert!(
        db.find_session_by_root_agent_id(Agent::Codex, &root_id)
            .unwrap()
            .is_none(),
        "the identity is free — nothing remembers the old session"
    );

    // Discovery re-reads the same source: a brand-new Logical Session, no
    // Owner, no Context, no launch history.
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
fn startup_backfill_includes_archived_sessions() {
    let db = open_db("backfill-trashed");
    let dir_a = TempDir::new("backfill-a-src");
    let dir_b = TempDir::new("backfill-b-src");
    let active = seeded_session(&db, &dir_a, SourceKind::File);
    let trashed = seeded_session(&db, &dir_b, SourceKind::File);
    let active_stored = commit_message(&db, &active.id, "unique active marker goals", 100);
    let trashed_stored = commit_message(&db, &trashed.id, "unique trashed marker goals", 100);

    lifecycle::archive_session(&db, &trashed.id).unwrap();
    // 模拟重启：startup backfill 不得把已归档加回来。
    db.backfill_search_index().unwrap();

    let hits = search::search(&db, "goals", 20).unwrap();
    assert!(
        hits.iter()
            .any(|h| h.kind == "message" && h.ref_id == active_stored[0].id),
        "the active session stays searchable"
    );
    assert!(
        hits.iter()
            .any(|h| h.kind == "message" && h.ref_id == trashed_stored[0].id),
        "the archived session remains searchable after startup backfill"
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
