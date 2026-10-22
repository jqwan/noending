//! Logical Session resolution end-to-end: discovery batch → session
//! resolution → atomic ingest commit.
//!
//! Fixtures are Codex rollouts (plain JSONL), the one adapter whose sources
//! express child threads (`thread_source=subagent`), side threads
//! (`guardian_review`) and forked pages (`history_base`) — so one fixture family
//! covers the whole source-shape vocabulary.

mod support;
use std::path::{Path, PathBuf};

use noending::adapters::all_adapters;
use noending::domain::{Agent, ParsedSessionMessage, Session, SessionMessageRole};
use noending::ingestion;
use noending::storage::{new_id, now, Db};
use noending_lib as noending;

fn temp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "noending-graph-{}-{}-{}",
        tag,
        std::process::id(),
        new_id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A Codex session-meta envelope. Extra fields (thread_source,
/// parent_thread_id, history_base) ride in the payload. Timestamps are
/// `now` as the fixture timestamp.
fn meta_line(id: &str, extra: serde_json::Value) -> String {
    let mut payload = serde_json::json!({
        "session_id": id, "id": id,
        "timestamp": chrono_now(),
        "cwd": "/repo-a",
    });
    if let (Some(base), Some(extra)) = (payload.as_object_mut(), extra.as_object()) {
        for (k, v) in extra {
            base.insert(k.clone(), v.clone());
        }
    }
    serde_json::json!({
        "timestamp": chrono_now(), "ordinal": 0,
        "type": "session_meta", "payload": payload,
    })
    .to_string()
}

fn chrono_now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn message_line(ordinal: usize, id: &str, role: &str, text: &str) -> String {
    serde_json::json!({
        "timestamp": "2026-09-20T13:01:48.000Z", "ordinal": ordinal,
        "type": "response_item",
        "payload": {
            "type": "message", "id": id, "role": role,
            "content": [{ "type": "input_text", "text": text }],
        },
    })
    .to_string()
}

fn rollout_name(id: &str) -> String {
    format!("rollout-2026-09-20T21-01-47-{id}.jsonl")
}

fn forked_rollout_name(base: &str, own: &str) -> String {
    format!("rollout-2026-09-22T21-17-07-{base}_{own}.jsonl")
}

fn write_rollout(root: &Path, name: &str, lines: &[String]) -> PathBuf {
    let p = root.join(name);
    std::fs::write(&p, lines.join("\n") + "\n").unwrap();
    p
}

fn open_db(tag: &str) -> Db {
    let dir = std::env::temp_dir().join(format!("noending-graph-db-{tag}-{}", new_id()));
    let db = Db::open(&dir.join("t.db")).unwrap();
    db.write()
        .execute("DELETE FROM ingest_sources", [])
        .unwrap();
    db
}

/// Register exactly one Codex ingest source pointing at `root`.
fn register_codex_source(db: &Db, root: &Path) {
    db.add_ingest_source(Agent::Codex, &root.to_string_lossy())
        .unwrap();
}

fn reconcile(db: &Db) -> (usize, i64) {
    ingestion::reconcile_all(db, &|_| {}).unwrap()
}

/// Default and custom roots both participate in automatic incremental sync.
#[test]
fn all_registered_sources_sync_and_pick_up_appended_messages() {
    use std::io::Write;
    let root = temp_root("首次同步-用户");
    let db = Db::open(&root.join("test.db")).unwrap();
    // Isolate fixtures from the developer machine's default history roots.
    db.write()
        .execute("DELETE FROM ingest_sources", [])
        .unwrap();
    let codex = root.join("codex");
    let other = root.join("codex-other");
    let pi = root.join("pi");
    for dir in [&codex, &other, &pi] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let source = db
        .add_ingest_source(Agent::Codex, &codex.to_string_lossy())
        .unwrap();
    db.add_ingest_source(Agent::Codex, &other.to_string_lossy())
        .unwrap();
    db.add_ingest_source(Agent::Pi, &pi.to_string_lossy())
        .unwrap();
    // A fresh computer normally has configured defaults for agents that have
    // never written history. Missing roots must not block the valid sources.
    for agent in Agent::all() {
        if !matches!(agent, Agent::Codex | Agent::Pi) {
            db.add_ingest_source(
                *agent,
                &root.join("absent").join(agent.as_str()).to_string_lossy(),
            )
            .unwrap();
        }
    }
    let sessions = codex.join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let first = write_rollout(
        &sessions,
        &rollout_name(ROOT_ID),
        &[
            meta_line(ROOT_ID, serde_json::json!({"cwd":codex})),
            message_line(1, "m1", "user", "Codex first"),
        ],
    );
    write_rollout(
        &other,
        &rollout_name(FORK_ID),
        &[
            meta_line(FORK_ID, serde_json::json!({"cwd":other})),
            message_line(1, "other-m1", "user", "Other root"),
        ],
    );
    let pi_path = pi.join("session.jsonl");
    std::fs::write(&pi_path, format!("{}\n{}\n",
        serde_json::json!({"type":"session","id":"pi-native","cwd":pi,"version":3,"timestamp":"2026-09-18T12:40:00Z"}),
        serde_json::json!({"type":"message","id":"pi-m1","parentId":"pi-native","timestamp":"2026-09-18T12:40:03Z","message":{"role":"user","content":[{"type":"text","text":"Pi first"}]}})
    )).unwrap();
    db.write()
        .execute(
            "UPDATE ingest_sources SET origin = 'default' WHERE id = ?1",
            [&source.id],
        )
        .unwrap();
    let report = ingestion::reconcile_all_report(&db, &|_| {}).unwrap();
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!((report.discovered, report.messages), (3, 3));
    let codex_session = db
        .find_session_by_root_agent_id(Agent::Codex, ROOT_ID)
        .unwrap()
        .unwrap();
    let pi_session = db
        .find_session_by_root_agent_id(Agent::Pi, "pi-native")
        .unwrap()
        .unwrap();
    assert_eq!(
        db.get_messages(&codex_session.id, None, 10).unwrap()[0].content,
        "Codex first"
    );
    assert_eq!(
        db.get_messages(&pi_session.id, None, 10).unwrap()[0].content,
        "Pi first"
    );
    assert_eq!(
        ingestion::reconcile_all_report(&db, &|_| {})
            .unwrap()
            .messages,
        0
    );
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(first)
            .unwrap(),
        "{}",
        message_line(2, "m2", "assistant", "Codex appended")
    )
    .unwrap();
    writeln!(std::fs::OpenOptions::new().append(true).open(pi_path).unwrap(), "{}",serde_json::json!({"type":"message","id":"pi-m2","parentId":"pi-m1","timestamp":"2026-09-18T12:40:05Z","message":{"role":"assistant","content":[{"type":"text","text":"Pi appended"}]}})).unwrap();
    let report = ingestion::reconcile_all_report(&db, &|_| {}).unwrap();
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(report.messages, 2);
    assert_eq!(db.message_count(&codex_session.id).unwrap(), 2);
    assert_eq!(db.message_count(&pi_session.id).unwrap(), 2);
    assert_eq!(
        db.find_session_by_root_agent_id(Agent::Codex, ROOT_ID)
            .unwrap()
            .unwrap()
            .id,
        codex_session.id
    );
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

const ROOT_ID: &str = "019f135a-621c-76a1-a76c-7c71021847aa";
const CHILD_ID: &str = "019fbbd3-47be-78f2-89fd-9deec2e4c6d9";
const SIDE_ID: &str = "019fccd3-47be-78f2-89fd-9deec2e4c6d9";
const FORK_ID: &str = "01a0c943-53b8-7e82-8f84-0c2b33da8801";

#[test]
fn logical_session_create_and_refresh_roll_back_when_search_indexing_fails() {
    for existing in [false, true] {
        let db = open_db("root-transaction");
        if existing {
            db.upsert_logical_session(
                Agent::Codex,
                ROOT_ID,
                Some("original title"),
                None,
                Some("/original"),
                None,
                None,
                None,
                None,
                "test",
                "/original/source",
                &serde_json::json!({"original": true}),
            )
            .unwrap();
        }
        let before = db
            .find_session_by_root_agent_id(Agent::Codex, ROOT_ID)
            .unwrap();
        // The session INSERT/UPDATE succeeds before indexing reaches the missing table.
        db.write()
            .execute_batch("DROP TABLE search_index;")
            .unwrap();
        assert!(db
            .upsert_logical_session(
                Agent::Codex,
                ROOT_ID,
                Some("changed title"),
                None,
                Some("/changed"),
                None,
                None,
                None,
                None,
                "changed",
                "/changed/source",
                &serde_json::json!({"changed": true}),
            )
            .is_err());
        let after = db
            .find_session_by_root_agent_id(Agent::Codex, ROOT_ID)
            .unwrap();
        assert_eq!(
            serde_json::to_value(after).unwrap(),
            serde_json::to_value(before).unwrap()
        );
    }
}

// child/side sources: recognized, then silently skipped

/// The root shows up in a LATER batch: one Logical Session keyed by the
/// root identity. Scan order never decides identity, and the child/side
/// rollouts seen along the way are silently skipped, never attached.
#[test]
fn a_root_discovered_in_a_later_batch_still_becomes_the_session() {
    let root_dir = temp_root("late-root");
    write_rollout(
        &root_dir,
        &rollout_name(CHILD_ID),
        &[
            meta_line(
                CHILD_ID,
                serde_json::json!({"thread_source": "subagent", "parent_thread_id": ROOT_ID}),
            ),
            message_line(1, "m1", "assistant", "子任务的结论"),
        ],
    );
    let db = open_db("late-root");
    register_codex_source(&db, &root_dir);
    reconcile(&db);
    assert!(db.list_sessions(Default::default()).unwrap().is_empty());

    // The batch with BOTH root and side arrives (root last in the walk order
    // is fine — the resolver is order-free).
    write_rollout(
        &root_dir,
        &rollout_name(ROOT_ID),
        &[
            meta_line(ROOT_ID, serde_json::json!({})),
            message_line(1, "m1", "user", "父会话的提问"),
            message_line(2, "m2", "assistant", "父会话的回答"),
        ],
    );
    write_rollout(
        &root_dir,
        &rollout_name(SIDE_ID),
        &[meta_line(
            SIDE_ID,
            serde_json::json!({"thread_source": "guardian_review", "parent_thread_id": ROOT_ID}),
        )],
    );
    reconcile(&db);

    let sessions = db.list_sessions(Default::default()).unwrap();
    assert_eq!(sessions.len(), 1, "exactly one Logical Session");
    assert_eq!(sessions[0].root_agent_session_id, ROOT_ID);
    assert!(
        db.find_session_by_root_agent_id(Agent::Codex, CHILD_ID)
            .unwrap()
            .is_none(),
        "a skipped child never becomes a session"
    );
    assert!(
        db.find_session_by_root_agent_id(Agent::Codex, SIDE_ID)
            .unwrap()
            .is_none(),
        "a skipped side thread never becomes a session"
    );
}

/// Child transcript text never becomes conversation; only the
/// root's user/assistant prose is stored.
#[test]
fn child_transcript_text_never_becomes_conversation() {
    let root_dir = temp_root("child-text");
    write_rollout(
        &root_dir,
        &rollout_name(ROOT_ID),
        &[
            meta_line(ROOT_ID, serde_json::json!({})),
            message_line(1, "m1", "user", "父会话的提问"),
            message_line(2, "m2", "assistant", "父会话的回答"),
        ],
    );
    write_rollout(
        &root_dir,
        &rollout_name(CHILD_ID),
        &[
            meta_line(
                CHILD_ID,
                serde_json::json!({"thread_source": "subagent", "parent_thread_id": ROOT_ID}),
            ),
            message_line(1, "c1", "user", "子任务的提示"),
            message_line(2, "c2", "assistant", "子任务的结论"),
        ],
    );
    let db = open_db("child-text");
    register_codex_source(&db, &root_dir);
    reconcile(&db);

    let sessions = db.list_sessions(Default::default()).unwrap();
    let messages = db.get_messages(&sessions[0].id, None, 100).unwrap();
    let contents: Vec<&str> = messages.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(
        contents,
        vec!["父会话的提问", "父会话的回答"],
        "child prose stays out of the conversation: {contents:?}"
    );
    // Every stored message belongs to the root's own session.
    assert!(messages.iter().all(|m| m.session_id == sessions[0].id));
    assert!(
        db.find_session_by_root_agent_id(Agent::Codex, CHILD_ID)
            .unwrap()
            .is_none(),
        "a skipped child never becomes a session"
    );
}

#[test]
fn a_missing_root_does_not_fail_reconcile_and_resumes_when_it_returns() {
    let root_dir = temp_root("missing-root");
    let root_path = write_rollout(
        &root_dir,
        &rollout_name(ROOT_ID),
        &[
            meta_line(ROOT_ID, serde_json::json!({})),
            message_line(1, "m1", "user", "最初的提问"),
        ],
    );
    let db = open_db("missing-root");
    register_codex_source(&db, &root_dir);
    reconcile(&db);

    let session = db
        .find_session_by_root_agent_id(Agent::Codex, ROOT_ID)
        .unwrap()
        .unwrap();
    let cursor = db
        .get_session(&session.id)
        .unwrap()
        .unwrap()
        .source_cursor();

    std::fs::remove_file(&root_path).unwrap();
    for _ in 0..2 {
        let report = ingestion::reconcile_all_report(&db, &|_| {}).unwrap();
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        assert_eq!(db.message_count(&session.id).unwrap(), 1);
        assert_eq!(
            db.get_session(&session.id)
                .unwrap()
                .unwrap()
                .source_cursor()
                .byte_offset,
            cursor.byte_offset,
            "a missing source must not advance its cursor"
        );
    }

    // The source returns and GREW: the stored session resumes from its cursor.
    std::fs::write(
        &root_path,
        format!(
            "{}\n{}\n{}\n",
            meta_line(ROOT_ID, serde_json::json!({})),
            message_line(1, "m1", "user", "最初的提问"),
            message_line(2, "m2", "assistant", "后续回答")
        ),
    )
    .unwrap();
    let report = ingestion::reconcile_all_report(&db, &|_| {}).unwrap();
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(db.message_count(&session.id).unwrap(), 2);
    assert!(
        db.get_session(&session.id)
            .unwrap()
            .unwrap()
            .source_cursor()
            .byte_offset
            > cursor.byte_offset,
        "the returning file must be ingested"
    );

    // A path that exists but is not a readable file is still an error.
    std::fs::remove_file(&root_path).unwrap();
    std::fs::create_dir(&root_path).unwrap();
    let report = ingestion::reconcile_all_report(&db, &|_| {}).unwrap();
    assert!(!report.failures.is_empty());
    let source = db
        .list_ingest_sources()
        .unwrap()
        .into_iter()
        .find(|source| source.path == root_dir.to_string_lossy())
        .unwrap();
    assert!(ingestion::reconcile_source(&db, &source, &|_| {}).is_err());
    db.remove_ingest_source(&source.id).unwrap();
    let removed_report = ingestion::reconcile_all_report(&db, &|_| {}).unwrap();
    assert!(
        removed_report.failures.is_empty(),
        "removed sources must not be retried"
    );
    assert_eq!(db.message_count(&session.id).unwrap(), 2);
}

/// A child rollout whose cwd differs is invisible: it never becomes a
/// session, so its execution cwd can never reach any Session — Session.cwd
/// is Root authority only.
#[test]
fn a_child_rollout_never_moves_the_session() {
    let root_dir = temp_root("child-cwd");
    write_rollout(
        &root_dir,
        &rollout_name(ROOT_ID),
        &[
            meta_line(ROOT_ID, serde_json::json!({})),
            message_line(1, "m1", "user", "提问"),
        ],
    );
    // The child's rollout records a DIFFERENT cwd in its meta.
    write_rollout(
        &root_dir,
        &rollout_name(CHILD_ID),
        &[{
            let mut payload = serde_json::json!({
                "session_id": CHILD_ID, "id": CHILD_ID,
                "timestamp": "2026-09-20T13:00:32.388Z",
                "cwd": "/repo-b-child",
            });
            payload["thread_source"] = serde_json::json!("subagent");
            payload["parent_thread_id"] = serde_json::json!(ROOT_ID);
            serde_json::json!({
                "timestamp": "2026-09-20T13:01:47.832Z", "ordinal": 0,
                "type": "session_meta", "payload": payload,
            })
            .to_string()
        }],
    );
    let db = open_db("child-cwd");
    register_codex_source(&db, &root_dir);
    reconcile(&db);

    let sessions = db.list_sessions(Default::default()).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        sessions[0].cwd.as_deref(),
        Some("/repo-a"),
        "child cwd is execution fact, never Session.cwd — and the child has no row at all"
    );
    assert!(db
        .find_session_by_root_agent_id(Agent::Codex, CHILD_ID)
        .unwrap()
        .is_none());
}

/// A growing child transcript touches nothing: skipped at discovery, it never
/// becomes a session, so neither `last_activity_at` nor `last_conversation_at`
/// may move for it.
#[test]
fn a_growing_child_transcript_touches_nothing() {
    let root_dir = temp_root("child-activity");
    write_rollout(
        &root_dir,
        &rollout_name(ROOT_ID),
        &[
            meta_line(ROOT_ID, serde_json::json!({})),
            message_line(1, "m1", "user", "提问"),
        ],
    );
    write_rollout(
        &root_dir,
        &rollout_name(CHILD_ID),
        &[meta_line(
            CHILD_ID,
            serde_json::json!({"thread_source": "subagent", "parent_thread_id": ROOT_ID}),
        )],
    );
    let db = open_db("child-activity");
    register_codex_source(&db, &root_dir);
    reconcile(&db);
    let session_id = db.list_sessions(Default::default()).unwrap()[0].id.clone();
    let before = db.get_session(&session_id).unwrap().unwrap();

    // A no-change read is not source activity.
    ingestion::ingest_session(&db, &before).unwrap();
    assert_eq!(
        db.get_session(&session_id)
            .unwrap()
            .unwrap()
            .last_activity_at,
        before.last_activity_at
    );

    // The child's source GROWS (a new reply lands in the child transcript) —
    // same-size-mtime would be skipped, so append real bytes.
    let child_path = root_dir.join(rollout_name(CHILD_ID));
    let mut body = std::fs::read_to_string(&child_path).unwrap();
    body.push_str(&message_line(9, "c9", "assistant", "子任务又说话了"));
    body.push('\n');
    std::fs::write(&child_path, body).unwrap();

    reconcile(&db);

    let after = db.get_session(&session_id).unwrap().unwrap();
    assert_eq!(
        after.last_conversation_at, before.last_conversation_at,
        "child chatter is not conversation"
    );
    assert_eq!(
        after.last_activity_at, before.last_activity_at,
        "a skipped child is not activity either"
    );
    assert!(db
        .find_session_by_root_agent_id(Agent::Codex, CHILD_ID)
        .unwrap()
        .is_none());
}

#[test]
fn trash_does_not_freeze_discovery() {
    let root_dir = temp_root("trash-freeze");
    let root_path = rollout_name(ROOT_ID);
    write_rollout(
        &root_dir,
        &root_path,
        &[
            meta_line(ROOT_ID, serde_json::json!({})),
            message_line(1, "m1", "user", "提问"),
        ],
    );
    let db = open_db("trash-freeze");
    register_codex_source(&db, &root_dir);
    reconcile(&db);
    let session = db.list_sessions(Default::default()).unwrap()[0].clone();
    let before_cursor = db
        .get_session(&session.id)
        .unwrap()
        .unwrap()
        .source_cursor();
    noending::lifecycle::archive_session(&db, &session.id).unwrap();

    write_rollout(
        &root_dir,
        &root_path,
        &[
            meta_line(ROOT_ID, serde_json::json!({"cwd": "/repo-after-restore"})),
            message_line(1, "m1", "user", "提问"),
        ],
    );
    write_rollout(
        &root_dir,
        &rollout_name(CHILD_ID),
        &[meta_line(
            CHILD_ID,
            serde_json::json!({"thread_source": "subagent", "parent_thread_id": ROOT_ID}),
        )],
    );
    reconcile(&db);

    // The recycle bin is a filter, not a freeze: the trashed Session keeps
    // following its source while it sits there.
    let current = db.get_session(&session.id).unwrap().unwrap();
    assert!(current.is_archived(), "discovery never Restores a Session");
    assert_eq!(current.cwd.as_deref(), Some("/repo-after-restore"));
    assert!(current.last_activity_at > session.last_activity_at);
    assert!(
        db.find_session_by_root_agent_id(Agent::Codex, CHILD_ID)
            .unwrap()
            .is_none(),
        "a child rollout discovered while trashed is skipped like any other"
    );
    assert!(
        db.get_session(&session.id)
            .unwrap()
            .unwrap()
            .source_cursor()
            .byte_offset
            > before_cursor.byte_offset
    );

    // Restore changes nothing about the facts, it only makes them visible again.
    noending::lifecycle::restore_session(&db, &session.id).unwrap();
    reconcile(&db);
    assert_eq!(
        db.get_session(&session.id).unwrap().unwrap().cwd.as_deref(),
        Some("/repo-after-restore")
    );
}

// Fork

/// A forked page becomes its OWN Logical Session with the fork source recorded
/// — and the source session's Owner is NOT inherited.
#[test]
fn a_fork_root_is_a_new_session_that_never_inherits_the_owner() {
    let root_dir = temp_root("fork");
    write_rollout(
        &root_dir,
        &rollout_name(ROOT_ID),
        &[
            meta_line(ROOT_ID, serde_json::json!({})),
            message_line(1, "m1", "user", "原始会话的提问"),
        ],
    );
    write_rollout(
        &root_dir,
        &forked_rollout_name(ROOT_ID, FORK_ID),
        &[
            // A forked page's meta still names the thread it forked FROM.
            meta_line(
                ROOT_ID,
                serde_json::json!({
                    "history_base": {"thread_id": ROOT_ID, "end_ordinal_exclusive": 90},
                }),
            ),
            message_line(90, "f1", "user", "fork 里的新提问"),
        ],
    );
    let db = open_db("fork");
    register_codex_source(&db, &root_dir);
    reconcile(&db);

    let sessions = db.list_sessions(Default::default()).unwrap();
    assert_eq!(sessions.len(), 2, "root + fork: {sessions:?}");
    let parent = sessions
        .iter()
        .find(|s| s.root_agent_session_id == ROOT_ID)
        .unwrap();
    let fork = sessions
        .iter()
        .find(|s| s.root_agent_session_id == FORK_ID)
        .unwrap();
    assert_eq!(
        fork.forked_from_session_id.as_deref(),
        Some(parent.id.as_str()),
        "fork provenance points at the source Logical Session"
    );

    // Now give the PARENT an owner via the explicit user door and re-reconcile:
    // ownership is decided once, by explicit action or a matched intent —
    // discovery never copies it onto the fork.
    let owner = support_workstream("ws-owner");
    db.upsert_workstream(&owner).unwrap();
    support::track(&db, &parent.id, Some(&owner.id)).unwrap();
    reconcile(&db);
    let fork = db.get_session(&fork.id).unwrap().unwrap();
    assert!(
        fork.tracked_workstream_ids.is_empty(),
        "a fork never inherits the source session's Owner"
    );
}

fn support_workstream(id: &str) -> noending::domain::Workstream {
    noending::domain::Workstream {
        id: id.into(),
        title: "所属任务".into(),
        description: String::new(),
        visibility: "normal".into(),
        created_at: now(),
        updated_at: now(),
    }
}

// ingestion atomicity

/// The detail-page preview query: user turns and each turn's FINAL reply
/// only, newest first capped to the limit, oldest-first output.
#[test]
fn recent_turn_messages_keep_the_conversation_skeleton() {
    let db = open_db("recent-turns");
    let (session, _) = seed_root(&db);
    let msg = |id: &str, role: SessionMessageRole, text: &str| ParsedSessionMessage {
        source_message_id: Some(id.into()),
        ts: None,
        role,
        content: text.into(),
    };
    db.commit_ingest(
        &session.id,
        &[
            msg("u2", SessionMessageRole::User, "追问"),
            msg("a-mid", SessionMessageRole::Assistant, "中间输出"),
            msg("a3", SessionMessageRole::Assistant, "第二轮回答"),
            msg("u3", SessionMessageRole::User, "再问"),
            msg("a-mid2", SessionMessageRole::Assistant, "又一处中间"),
            msg("a4", SessionMessageRole::Assistant, "第三轮回答"),
        ],
        &seed_update(1, 500),
    )
    .unwrap();

    let turns = db.recent_turn_messages(&session.id, 4).unwrap();
    let texts: Vec<&str> = turns.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(
        texts,
        vec!["追问", "第二轮回答", "再问", "第三轮回答"],
        "intermediates drop out; the newest `limit` survive, oldest first"
    );

    // A page of 1 takes only the newest turn message.
    let one = db.recent_turn_messages(&session.id, 1).unwrap();
    assert_eq!(one[0].content, "第三轮回答");

    // The detail API and history reader share the window DTO. A replaced
    // snapshot keeps internal sequences distinct from current ordinals.
    let preview: Vec<_> = db
        .message_window(&session.id, None, 4)
        .unwrap()
        .messages
        .into_iter()
        .map(noending::commands::session_workspace::SessionWindowMessage::from)
        .map(|m| serde_json::to_value(m).unwrap())
        .collect();
    assert_eq!(
        preview
            .iter()
            .map(|m| m["ordinal"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [1, 3, 4, 6]
    );
    assert_eq!(
        preview
            .iter()
            .map(|m| m["sequence"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [2, 4, 5, 7]
    );
}

/// An append that continues the same turn demotes the formerly-final reply.
#[test]
fn appending_to_a_turn_demotes_the_formerly_final_reply() {
    let db = open_db("turn-final");
    let (session, _) = seed_root(&db);
    let msg = |id: &str, role: SessionMessageRole, text: &str| ParsedSessionMessage {
        source_message_id: Some(id.into()),
        ts: None,
        role,
        content: text.into(),
    };
    let skeleton = |db: &Db| -> Vec<String> {
        db.recent_turn_messages(&session.id, 8)
            .unwrap()
            .into_iter()
            .map(|message| message.content)
            .collect()
    };

    let turn = vec![
        msg("u1", SessionMessageRole::User, "问题"),
        msg("a1", SessionMessageRole::Assistant, "中间输出"),
        msg("a2", SessionMessageRole::Assistant, "最终回答"),
        msg("u2", SessionMessageRole::User, "追问"),
        msg("a3", SessionMessageRole::Assistant, "第二轮回答"),
    ];
    db.commit_ingest(&session.id, &turn, &seed_update(1, 500))
        .unwrap();
    assert_eq!(
        skeleton(&db),
        vec!["问题", "最终回答", "追问", "第二轮回答"]
    );

    // The turn continues: the appended reply takes the final slot and a3
    // becomes an intermediate.
    db.commit_ingest(
        &session.id,
        &[msg("a4", SessionMessageRole::Assistant, "补充")],
        &noending::domain::SourceCursorUpdate {
            start_byte_offset: 500,
            ..seed_update(1, 700)
        },
    )
    .unwrap();
    assert_eq!(skeleton(&db), vec!["问题", "最终回答", "追问", "补充"]);
}

#[test]
fn older_history_from_a_rescan_cannot_move_conversation_time_backwards() {
    let db = open_db("conversation-monotonic");
    let (session, _) = seed_root(&db);
    let before = db
        .get_session(&session.id)
        .unwrap()
        .unwrap()
        .last_conversation_at;
    let stored = db
        .commit_ingest(
            &session.id,
            &[
                ParsedSessionMessage {
                    source_message_id: Some("m1".into()),
                    ts: Some("2026-09-20T13:01:48Z".into()),
                    role: SessionMessageRole::User,
                    content: "第一次的提问".into(),
                },
                ParsedSessionMessage {
                    source_message_id: Some("old".into()),
                    ts: Some("2020-01-01T00:00:00Z".into()),
                    role: SessionMessageRole::Assistant,
                    content: "旧历史".into(),
                },
            ],
            &seed_update(2, 80),
        )
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].ts.as_deref(), Some("2020-01-01T00:00:00Z"));
    assert_eq!(
        db.get_session(&session.id)
            .unwrap()
            .unwrap()
            .last_conversation_at,
        before
    );
}

#[test]
fn a_new_untimestamped_message_uses_the_source_mtime_for_conversation_time() {
    let db = open_db("conversation-mtime-fallback");
    let (session, _) = seed_root(&db);
    let mut source = seed_update(1, 80);
    source.mtime = Some(1_893_456_000.0);
    let stored = db
        .commit_ingest(
            &session.id,
            &[
                ParsedSessionMessage {
                    source_message_id: Some("m2".into()),
                    ts: Some("2027-01-01T00:00:00Z".into()),
                    role: SessionMessageRole::Assistant,
                    content: "带时间戳的新回复".into(),
                },
                ParsedSessionMessage {
                    source_message_id: Some("m3".into()),
                    ts: None,
                    role: SessionMessageRole::Assistant,
                    content: "没有时间戳的新回复".into(),
                },
            ],
            &source,
        )
        .unwrap();
    assert_eq!(stored.len(), 2);
    assert_eq!(
        db.get_session(&session.id)
            .unwrap()
            .unwrap()
            .last_conversation_at
            .as_deref(),
        Some("2030-01-01T00:00:00+00:00")
    );
}

/// Discovery never panics for an adapter that cannot identify a source:
/// the roster still works end to end (a smoke check over every adapter with
/// a fabricated impossible source — no panic, never a false Present).
#[test]
fn every_adapter_inspects_sources_without_panicking() {
    for adapter in all_adapters() {
        let session = Session {
            id: new_id(),
            agent: adapter.agent(),
            root_agent_session_id: "no-such-source".into(),
            title: None,
            tracked_workstream_ids: vec![],
            cwd: None,
            workspace_path_id: None,
            project_id: None,
            forked_from_session_id: None,
            started_at: None,
            last_activity_at: None,
            last_conversation_at: None,
            archived_at: None,
            source_kind: "test".into(),
            source_path: "/tmp/definitely-not-here".into(),
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
        };
        // A missing FILE is Missing for file adapters; a missing RECORD in a
        // store adapter is only answerable when the store itself exists —
        // either way, no panic and never a false Present.
        let verdict = adapter.inspect_session_source(&session).unwrap();
        assert_ne!(verdict, noending::domain::SourceAvailability::Present);
    }
}

// helpers

/// Seed a logical session + one message, the way production would after
/// discovery (via the same storage APIs ingestion uses).
fn seed_root(
    db: &Db,
) -> (
    noending::domain::Session,
    Vec<noending::domain::SessionMessage>,
) {
    let (session_id, _) = db
        .upsert_logical_session(
            Agent::Codex,
            ROOT_ID,
            None,
            Some("/repo-a"),
            None,
            None,
            None,
            None,
            None,
            "codex_rollout",
            "/repo-a/rollout.jsonl",
            &serde_json::json!({}),
        )
        .unwrap();
    let stored = db
        .commit_ingest(
            &session_id,
            &[ParsedSessionMessage {
                source_message_id: Some("m1".into()),
                ts: Some("2026-09-20T13:01:48Z".into()),
                role: SessionMessageRole::User,
                content: "第一次的提问".into(),
            }],
            &seed_update(0, 40),
        )
        .unwrap();
    let session = db.get_session(&session_id).unwrap().unwrap();
    (session, stored)
}

fn seed_update(generation: i64, byte_offset: u64) -> noending::domain::SourceCursorUpdate {
    noending::domain::SourceCursorUpdate {
        file_identity: format!("test-identity-{generation}"),
        generation,
        byte_offset,
        last_seen_size: byte_offset,
        mtime: None,
        start_byte_offset: 0,
        prefix_hash: String::new(),
    }
}

const OTHER_ROOT_ID: &str = "01a0d943-53b8-7e82-8f84-0c2b33da8802";

/// A stored Logical Session is never re-homed: when the source later claims
/// the same session id as another session's subagent, the claim is a skipped
/// child — the stored session survives untouched, and the new root still
/// resolves as its own session.
#[test]
fn a_stored_root_is_never_re_homed_as_a_child() {
    let root_dir = temp_root("topo-guard");
    write_rollout(
        &root_dir,
        &rollout_name(ROOT_ID),
        &[
            meta_line(ROOT_ID, serde_json::json!({})),
            message_line(1, "m1", "user", "第一问"),
        ],
    );
    let db = open_db("topo-guard");
    register_codex_source(&db, &root_dir);
    reconcile(&db);
    let session_a = db
        .find_session_by_root_agent_id(Agent::Codex, ROOT_ID)
        .unwrap()
        .unwrap();

    // Pass 2: the same session id is re-claimed as a subagent of a new root.
    write_rollout(
        &root_dir,
        &rollout_name(ROOT_ID),
        &[meta_line(
            ROOT_ID,
            serde_json::json!({"thread_source": "subagent", "parent_thread_id": OTHER_ROOT_ID}),
        )],
    );
    write_rollout(
        &root_dir,
        &rollout_name(OTHER_ROOT_ID),
        &[
            meta_line(OTHER_ROOT_ID, serde_json::json!({})),
            message_line(1, "m1", "user", "新根的提问"),
        ],
    );
    reconcile(&db);

    // Session A survives, still keyed by its root identity — no ghost.
    let stored = db
        .find_session_by_root_agent_id(Agent::Codex, ROOT_ID)
        .unwrap()
        .unwrap();
    assert_eq!(stored.id, session_a.id, "no re-home");
    // The new root still resolves as its own session.
    assert!(db
        .find_session_by_root_agent_id(Agent::Codex, OTHER_ROOT_ID)
        .unwrap()
        .is_some());
}

/// Title policy across repeated ingests: the app's native title writes
/// through (a rename follows), the text-derived fallback only fills an
/// EMPTY slot — it never overwrites, and a later ingest without a native
/// title never downgrades the stored one back to the fallback.
#[test]
fn a_native_title_renames_and_a_fallback_title_only_fills() {
    let db = open_db("title-policy");
    let refresh = |db: &Db, native: Option<&str>, fallback: Option<&str>| {
        db.upsert_logical_session(
            Agent::Codex,
            "title-policy-root",
            native,
            fallback,
            Some("/repo-a"),
            None,
            None,
            None,
            None,
            "codex_rollout",
            "/repo-a/rollout.jsonl",
            &serde_json::json!({}),
        )
        .unwrap();
        db.find_session_by_root_agent_id(Agent::Codex, "title-policy-root")
            .unwrap()
            .unwrap()
            .title
    };

    // No native yet: the fallback fills the empty slot.
    assert_eq!(
        refresh(&db, None, Some("首条用户文本")).as_deref(),
        Some("首条用户文本")
    );
    // A new fallback never overwrites — not even its own kind.
    assert_eq!(
        refresh(&db, None, Some("另一条兜底")).as_deref(),
        Some("首条用户文本")
    );
    // The app names the thread: native replaces the fallback.
    assert_eq!(
        refresh(&db, Some("模型起的名字"), Some("首条用户文本")).as_deref(),
        Some("模型起的名字")
    );
    // The app renames: the rename follows.
    assert_eq!(
        refresh(&db, Some("改名之后"), None).as_deref(),
        Some("改名之后")
    );
    // An ingest without a native title keeps the stored one — no downgrade
    // back to the fallback.
    assert_eq!(
        refresh(&db, None, Some("首条用户文本")).as_deref(),
        Some("改名之后")
    );
}

/// Cold storage is a representation change, not a new conversation or activity.
#[test]
fn codex_compression_roundtrip_preserves_identity_context_and_activity() {
    use noending::adapters::AgentAdapter;
    use noending::domain::{SessionContextFields, SourceAvailability};
    let root = temp_root("compressed-roundtrip");
    let mut meta: serde_json::Value =
        serde_json::from_str(&meta_line(ROOT_ID, serde_json::json!({}))).unwrap();
    meta["timestamp"] = serde_json::json!("2026-09-20T13:00:00Z");
    let body = [
        meta.to_string(),
        message_line(1, "m1", "user", "保留我的消息"),
        message_line(2, "m2", "assistant", "已有回答"),
    ]
    .join("\n")
        + "\n";
    let plain = root.join(rollout_name(ROOT_ID));
    let compressed = plain.with_extension("jsonl.zst");
    std::fs::write(&plain, &body).unwrap();
    let db = open_db("compressed-roundtrip");
    register_codex_source(&db, &root);
    assert_eq!(reconcile(&db).1, 2);
    let before = db
        .find_session_by_root_agent_id(Agent::Codex, ROOT_ID)
        .unwrap()
        .unwrap();
    let original_ids: Vec<_> = db
        .get_messages(&before.id, None, 100)
        .unwrap()
        .iter()
        .map(|m| m.id.clone())
        .collect();
    db.tx(|tx| {
        noending::storage::context_repo::commit_session_context_conn(
            tx,
            &before.id,
            &SessionContextFields {
                summary: noending_lib::domain::Summary::single("已有摘要"),
                ..Default::default()
            },
            0,
            before.fact_generation,
            2,
        )
    })
    .unwrap();
    let adapter = noending::adapters::codex::CodexAdapter;
    std::fs::write(&compressed, zstd::encode_all(body.as_bytes(), 3).unwrap()).unwrap();
    std::fs::remove_file(&plain).unwrap();
    // Before any discovery, resume/source actions already find the compressed sibling.
    assert_eq!(adapter.session_source_path(&before), compressed);
    assert_eq!(
        adapter.inspect_session_source(&before).unwrap(),
        SourceAvailability::Present
    );
    assert_eq!(ingestion::ingest_session(&db, &before).unwrap(), 0);
    assert_eq!(reconcile(&db).1, 0);
    let cold = db.get_session(&before.id).unwrap().unwrap();
    assert_eq!(cold.source_path, compressed.to_string_lossy());
    assert_eq!(cold.fact_generation, before.fact_generation);
    assert_eq!(cold.last_activity_at, before.last_activity_at);
    assert_eq!(cold.last_conversation_at, before.last_conversation_at);
    let summary = db.get_session_context(&before.id).unwrap().unwrap();
    assert_eq!(
        summary.fields.summary,
        noending_lib::domain::Summary::single("已有摘要")
    );
    assert_eq!(summary.ingest_generation, before.fact_generation);
    assert_eq!(summary.processed_through_seq, 2);
    // A completed raw-byte cursor means the unchanged archive is not decoded again.
    assert!(adapter
        .read_session_delta(&cold, &cold.source_cursor())
        .unwrap()
        .source
        .is_none());
    std::fs::write(&compressed, zstd::encode_all(body.as_bytes(), 9).unwrap()).unwrap();
    assert_eq!(reconcile(&db).1, 0);
    let repacked = db.get_session(&before.id).unwrap().unwrap();
    assert_eq!(repacked.fact_generation, before.fact_generation);
    assert_eq!(repacked.last_activity_at, before.last_activity_at);
    assert_eq!(
        db.get_messages(&before.id, None, 100)
            .unwrap()
            .iter()
            .map(|m| m.id.clone())
            .collect::<Vec<_>>(),
        original_ids
    );
    // Native resume expands the file; both spellings can coexist briefly.
    std::fs::write(&plain, &body).unwrap();
    assert_eq!(adapter.session_source_path(&cold), plain);
    let found = adapter
        .discover_members_in(&[root.clone()], &|_| false)
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].source_path, plain);
    assert_eq!(reconcile(&db).1, 0);
    let restored = db.get_session(&before.id).unwrap().unwrap();
    assert_eq!(restored.fact_generation, before.fact_generation);
    assert_eq!(restored.last_activity_at, before.last_activity_at);
    use std::io::Write;
    let mut new_message: serde_json::Value =
        serde_json::from_str(&message_line(3, "m3", "user", "恢复后新消息")).unwrap();
    new_message["timestamp"] = serde_json::json!("2026-09-21T13:00:00Z");
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(&plain)
            .unwrap(),
        "{new_message}"
    )
    .unwrap();
    assert_eq!(reconcile(&db).1, 1);
    assert_eq!(db.list_sessions(Default::default()).unwrap().len(), 1);
    let updated = db.get_session(&before.id).unwrap().unwrap();
    assert!(updated.last_activity_at > before.last_activity_at);
    assert_eq!(
        db.get_messages(&before.id, None, 100)
            .unwrap()
            .last()
            .unwrap()
            .content,
        "恢复后新消息"
    );
}

#[test]
fn compressed_codex_discovery_append_and_damage_preserve_current_history() {
    use noending::adapters::AgentAdapter;
    let root = temp_root("compressed-discovery");
    let compressed = root.join(rollout_name(ROOT_ID) + ".zst");
    // Codex CLI's native header does not require the desktop-only ordinal.
    let mut meta: serde_json::Value =
        serde_json::from_str(&meta_line(ROOT_ID, serde_json::json!({}))).unwrap();
    meta.as_object_mut().unwrap().remove("ordinal");
    let body = [meta.to_string(), message_line(1, "m1", "user", "压缩历史")].join("\n") + "\n";
    std::fs::write(&compressed, zstd::encode_all(body.as_bytes(), 3).unwrap()).unwrap();
    let db = open_db("compressed-discovery");
    register_codex_source(&db, &root);
    assert_eq!(reconcile(&db).1, 1);
    let session = db
        .find_session_by_root_agent_id(Agent::Codex, ROOT_ID)
        .unwrap()
        .unwrap();
    use std::io::Write;
    let tail = message_line(2, "m2", "assistant", "追加压缩帧") + "\n";
    std::fs::OpenOptions::new()
        .append(true)
        .open(&compressed)
        .unwrap()
        .write_all(&zstd::encode_all(tail.as_bytes(), 3).unwrap())
        .unwrap();
    assert_eq!(reconcile(&db).1, 1);
    let before = db.get_session(&session.id).unwrap().unwrap();
    let messages = db.get_messages(&session.id, None, 100).unwrap();
    assert_eq!(
        messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>(),
        ["压缩历史", "追加压缩帧"]
    );
    let valid = std::fs::read(&compressed).unwrap();
    for damaged in [&valid[..valid.len() - 5], b"not a zstd frame".as_slice()] {
        std::fs::write(&compressed, damaged).unwrap();
        let report = ingestion::reconcile_all_report(&db, &|_| {}).unwrap();
        assert!(!report.failures.is_empty());
        let after = db.get_session(&session.id).unwrap().unwrap();
        assert_eq!(after.source_byte_offset, before.source_byte_offset);
        assert_eq!(after.fact_generation, before.fact_generation);
        assert_eq!(
            db.get_messages(&session.id, None, 100)
                .unwrap()
                .iter()
                .map(|m| &m.id)
                .collect::<Vec<_>>(),
            messages.iter().map(|m| &m.id).collect::<Vec<_>>()
        );
    }
    std::fs::write(&compressed, valid).unwrap();
    assert_eq!(reconcile(&db).1, 0);
    std::fs::remove_file(&compressed).unwrap();
    assert_eq!(
        noending::adapters::codex::CodexAdapter
            .inspect_session_source(&session)
            .unwrap(),
        noending::domain::SourceAvailability::Missing
    );
}

#[test]
fn full_sync_rechecks_plain_and_compressed_content_even_when_metadata_is_unchanged() {
    for compressed in [false, true] {
        let root = temp_root("force-full-content");
        let db = open_db("force-full-content");
        let source = db
            .add_ingest_source(Agent::Codex, &root.to_string_lossy())
            .unwrap();
        let path = root.join(format!(
            "{}{}",
            rollout_name(ROOT_ID),
            if compressed { ".zst" } else { "" }
        ));
        let mut meta: serde_json::Value =
            serde_json::from_str(&meta_line(ROOT_ID, serde_json::json!({}))).unwrap();
        // Stable input keeps the compressed-size fixture independent of the clock.
        meta["timestamp"] = serde_json::json!("2026-09-20T13:01:48.000Z");
        meta["payload"]["timestamp"] = meta["timestamp"].clone();
        let meta = meta.to_string();
        let body =
            |text: &str| [meta.clone(), message_line(1, "m1", "user", text)].join("\n") + "\n";
        let encode = |text: &str| {
            if compressed {
                zstd::encode_all(text.as_bytes(), 3).unwrap()
            } else {
                text.as_bytes().to_vec()
            }
        };
        let before = encode(&body("AAAAAA"));
        let replacement = encode(&body("BBBBBB"));
        assert_eq!(
            before.len(),
            replacement.len(),
            "the fixture changes content without changing file size"
        );
        std::fs::write(&path, before).unwrap();
        reconcile(&db);
        let session = db
            .find_session_by_root_agent_id(Agent::Codex, ROOT_ID)
            .unwrap()
            .unwrap();
        let old = db.get_messages(&session.id, None, 10).unwrap()[0].clone();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::write(&path, replacement).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        assert_eq!(
            ingestion::reconcile_source(&db, &source, &|_| {})
                .unwrap()
                .1,
            0
        );
        assert_eq!(
            db.get_messages(&session.id, None, 10).unwrap()[0].content,
            "AAAAAA"
        );

        assert_eq!(
            ingestion::reingest_source(&db, &source, &|_| {}).unwrap().1,
            1
        );
        let current = db.get_messages(&session.id, None, 10).unwrap();
        assert_eq!(current[0].content, "BBBBBB");
        assert!(db
            .get_message_by_ref(&format!("session-message:{}", old.id))
            .unwrap()
            .is_none());
        let stored: i64 = db
            .read()
            .query_row(
                "SELECT COUNT(*) FROM session_messages WHERE session_id=?1",
                [&session.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, 1);
        let before_repeat = db.get_session(&session.id).unwrap().unwrap();
        assert_eq!(
            ingestion::reingest_source(&db, &source, &|_| {}).unwrap().1,
            0
        );
        assert_eq!(
            db.get_messages(&session.id, None, 10).unwrap()[0].id,
            current[0].id
        );
        let after_repeat = db.get_session(&session.id).unwrap().unwrap();
        assert_eq!(after_repeat.fact_generation, before_repeat.fact_generation);
        assert_eq!(
            after_repeat.source_generation,
            before_repeat.source_generation
        );
    }
}

#[test]
fn source_retries_cover_old_sessions_outside_a_bounded_preview() {
    let root = temp_root("retry-old-session");
    let db = open_db("retry-old-session");
    let source = db
        .add_ingest_source(Agent::Codex, &root.to_string_lossy())
        .unwrap();
    let path = write_rollout(
        &root,
        &rollout_name(ROOT_ID),
        &[
            meta_line(ROOT_ID, serde_json::json!({})),
            message_line(1, "m1", "user", "last reliable conversation"),
        ],
    );
    reconcile(&db);
    let session = db
        .find_session_by_root_agent_id(Agent::Codex, ROOT_ID)
        .unwrap()
        .unwrap();
    let old_message = db.get_messages(&session.id, None, 10).unwrap()[0].clone();
    db.tx(|tx| {
        for i in 0..501 {
            tx.execute(
                "INSERT INTO sessions (id, agent, root_agent_session_id, title,
                    started_at, source_kind, source_path)
                 VALUES (?1, 'codex', ?1, 'newer conversation', '2099-01-01T00:00:00Z',
                    'codex_rollout', '/outside-configured-source')",
                [format!("newer-{i}")],
            )?;
        }
        Ok(())
    })
    .unwrap();
    let all = db.list_session_items(Default::default(), None).unwrap();
    assert_eq!(all.len(), 502);
    assert!(all.iter().any(|s| s.id == session.id));
    let preview = db.list_session_items(Default::default(), Some(25)).unwrap();
    assert_eq!(preview.len(), 25);
    assert!(preview.iter().all(|s| s.id != session.id));

    // Discovery cannot identify a broken header, but the persisted source
    // still needs a retry and must report failure in both synchronization modes.
    std::fs::write(&path, "{broken\n").unwrap();
    assert!(ingestion::reconcile_source(&db, &source, &|_| {}).is_err());
    assert!(ingestion::reingest_source(&db, &source, &|_| {}).is_err());
    assert_eq!(
        db.get_messages(&session.id, None, 10).unwrap()[0].id,
        old_message.id
    );
    let after = db.get_session(&session.id).unwrap().unwrap();
    assert_eq!(after.source_prefix_hash, session.source_prefix_hash);
    assert_eq!(after.source_byte_offset, session.source_byte_offset);
}

#[test]
fn a_failed_full_sync_preserves_messages_and_cursor_after_discovery() {
    for compressed in [false, true] {
        let root = temp_root("failed-full-after-discovery");
        let db = open_db("failed-full-after-discovery");
        let source = db
            .add_ingest_source(Agent::Codex, &root.to_string_lossy())
            .unwrap();
        let path = root.join(format!(
            "{}{}",
            rollout_name(ROOT_ID),
            if compressed { ".zst" } else { "" }
        ));
        let meta = meta_line(ROOT_ID, serde_json::json!({}));
        let valid = [
            meta.clone(),
            message_line(1, "m1", "user", "reliable history"),
        ]
        .join("\n")
            + "\n";
        let write = |body: &str| {
            let bytes = if compressed {
                zstd::encode_all(body.as_bytes(), 3).unwrap()
            } else {
                body.as_bytes().to_vec()
            };
            std::fs::write(&path, bytes).unwrap();
        };
        write(&valid);
        reconcile(&db);
        let before = db
            .find_session_by_root_agent_id(Agent::Codex, ROOT_ID)
            .unwrap()
            .unwrap();
        let old = db.get_messages(&before.id, None, 10).unwrap()[0].clone();
        let broken = [
            meta,
            message_line(1, "m2", "user", "unconfirmed replacement"),
            "{broken}".into(),
        ]
        .join("\n")
            + "\n";
        // Runs after discovery has decoded a valid snapshot, before ingestion
        // consumes it: cached content must be revalidated against the source.
        assert!(ingestion::reingest_source(&db, &source, &|_| write(&broken)).is_err());
        let after = db.get_session(&before.id).unwrap().unwrap();
        assert_eq!(db.get_messages(&before.id, None, 10).unwrap()[0].id, old.id);
        assert_eq!(
            (
                after.fact_generation,
                after.source_byte_offset,
                after.source_last_seen_size,
                after.source_prefix_hash,
                after.source_tail_hash
            ),
            (
                before.fact_generation,
                before.source_byte_offset,
                before.source_last_seen_size,
                before.source_prefix_hash,
                before.source_tail_hash
            )
        );
        let stored: i64 = db
            .read()
            .query_row(
                "SELECT COUNT(*) FROM session_messages WHERE session_id=?1",
                [&before.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, 1, "a failed full scan cannot leave orphan rows");
    }
}
