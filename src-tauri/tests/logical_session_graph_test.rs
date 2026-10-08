//! Logical Session resolution end-to-end: discovery batch → session
//! resolution → atomic ingest commit → LaunchIntent root-only.
//!
//! Fixtures are Codex rollouts (plain JSONL), the one adapter whose sources
//! express child threads (`thread_source=subagent`), side threads
//! (`guardian_review`) and forked pages (`history_base`) — so one fixture family
//! covers the whole source-shape vocabulary.

use std::path::{Path, PathBuf};

use noending::adapters::all_adapters;
use noending::domain::{Agent, LaunchIntent, ParsedSessionMessage, Session, SessionMessageRole};
use noending::ingestion;
use noending::launcher::LaunchWorkspace;
use noending::storage::{new_id, now, Db};

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
/// `now` so LaunchIntent match windows stay open.
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
    Db::open(&dir.join("t.db")).unwrap()
}

/// Enable exactly one Codex ingest source pointing at `root`.
fn enable_codex_source(db: &Db, root: &Path) {
    db.add_ingest_source(Agent::Codex, &root.to_string_lossy(), true)
        .unwrap();
}

fn reconcile(db: &Db) -> (usize, i64) {
    ingestion::reconcile_all(db, &LaunchWorkspace::default(), &|_| {}).unwrap()
}

const ROOT_ID: &str = "019f135a-621c-76a1-a76c-7c71021847aa";
const CHILD_ID: &str = "019fbbd3-47be-78f2-89fd-9deec2e4c6d9";
const SIDE_ID: &str = "019fccd3-47be-78f2-89fd-9deec2e4c6d9";
const FORK_ID: &str = "01a0c943-53b8-7e82-8f84-0c2b33da8801";

#[test]
fn logical_session_creation_rolls_back_if_the_row_cannot_be_written() {
    let db = open_db("root-transaction");
    db.write()
        .execute_batch(
            "CREATE TRIGGER reject_session_insert BEFORE INSERT ON sessions
             BEGIN SELECT RAISE(ABORT, 'test session insert failure'); END;",
        )
        .unwrap();

    assert!(db
        .upsert_logical_session(
            Agent::Codex,
            ROOT_ID,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            "test",
            "/tmp/root",
            &serde_json::json!({}),
        )
        .is_err());
    assert!(db
        .find_session_by_root_agent_id(Agent::Codex, ROOT_ID)
        .unwrap()
        .is_none());
}

// child/side sources: recognized, then silently skipped

/// A child discovered with no root anywhere: no Logical Session is created
/// and nothing is recorded. Since the stats retirement a session IS its root
/// and child/side sources are invisible by design — the recognition itself
/// is what keeps codex subagent rollouts from becoming fake sessions.
#[test]
fn an_orphan_child_is_skipped_silently_not_a_session() {
    let root_dir = temp_root("orphan-child");
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
    let db = open_db("orphan-child");
    enable_codex_source(&db, &root_dir);

    reconcile(&db);

    assert!(
        db.list_sessions(Default::default()).unwrap().is_empty(),
        "no root, no Logical Session"
    );
}

/// The root shows up in a LATER batch: one Logical Session keyed by the
/// root identity. Scan order never decides identity, and the child/side
/// rollouts seen along the way are silently skipped, never attached.
#[test]
fn a_root_discovered_in_a_later_batch_still_becomes_the_session() {
    let root_dir = temp_root("late-root");
    write_rollout(
        &root_dir,
        &rollout_name(CHILD_ID),
        &[meta_line(
            CHILD_ID,
            serde_json::json!({"thread_source": "subagent", "parent_thread_id": ROOT_ID}),
        )],
    );
    let db = open_db("late-root");
    enable_codex_source(&db, &root_dir);
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
    enable_codex_source(&db, &root_dir);
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
    enable_codex_source(&db, &root_dir);
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
        let report =
            ingestion::reconcile_all_report(&db, &LaunchWorkspace::default(), &|_| {}).unwrap();
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
    let report =
        ingestion::reconcile_all_report(&db, &LaunchWorkspace::default(), &|_| {}).unwrap();
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
    let report =
        ingestion::reconcile_all_report(&db, &LaunchWorkspace::default(), &|_| {}).unwrap();
    assert!(!report.failures.is_empty());
}

/// A child rollout whose cwd differs is invisible: it never becomes a
/// session, so its execution cwd can never reach any Session — Session.cwd
/// is Root authority only.
#[test]
fn a_child_rollout_never_moves_the_session_or_becomes_a_member() {
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
    enable_codex_source(&db, &root_dir);
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
    enable_codex_source(&db, &root_dir);
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
    enable_codex_source(&db, &root_dir);
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
    enable_codex_source(&db, &root_dir);
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
    db.set_session_owner(&parent.id, Some(&owner.id)).unwrap();
    reconcile(&db);
    let fork = db.get_session(&fork.id).unwrap().unwrap();
    assert!(
        fork.owner_workstream_id.is_none(),
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

/// LaunchIntent matching is ROOT-only: a child/side discovery can
/// never claim a pending intent; the root discovery does.
#[test]
fn launch_intents_match_roots_only() {
    let root_dir = temp_root("intent");
    write_rollout(
        &root_dir,
        &rollout_name(CHILD_ID),
        &[meta_line(
            CHILD_ID,
            serde_json::json!({"thread_source": "subagent", "parent_thread_id": ROOT_ID}),
        )],
    );
    let db = open_db("intent");
    enable_codex_source(&db, &root_dir);
    let ws = support_workstream("ws-intent");
    db.upsert_workstream(&ws).unwrap();
    let intent = LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent: Agent::Codex,
        owner_workstream_id: Some(ws.id.clone()),
        cwd: Some("/repo-a".into()),
        process_id: None,
        // Within the match window of the fixture timestamps (both are now).
        launched_at: chrono_now(),
        matched_session_id: None,
        status: "pending".into(),
        note: String::new(),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_launch_intent(&intent).unwrap();

    // Pass 1: only the child. Nothing may match.
    reconcile(&db);
    let after_child = db.get_launch_intent(&intent.id).unwrap().unwrap();
    assert_eq!(
        after_child.status, "pending",
        "a child never claims an intent"
    );
    assert_eq!(after_child.matched_session_id, None);

    // Pass 2: the root arrives and claims it.
    write_rollout(
        &root_dir,
        &rollout_name(ROOT_ID),
        &[
            meta_line(ROOT_ID, serde_json::json!({})),
            message_line(1, "m1", "user", "提问"),
        ],
    );
    reconcile(&db);
    let after_root = db.get_launch_intent(&intent.id).unwrap().unwrap();
    assert_eq!(after_root.status, "matched");
    let sessions = db.list_sessions(Default::default()).unwrap();
    let root_session = sessions
        .iter()
        .find(|s| s.root_agent_session_id == ROOT_ID)
        .unwrap();
    assert_eq!(
        after_root.matched_session_id.as_deref(),
        Some(root_session.id.as_str())
    );
    assert_eq!(
        root_session.owner_workstream_id.as_deref(),
        Some(ws.id.as_str()),
        "the matched intent sets the Root session's Owner"
    );
}

#[test]
fn unchanged_root_retries_a_pending_launch_intent() {
    let root_dir = temp_root("intent-retry");
    write_rollout(
        &root_dir,
        &rollout_name(ROOT_ID),
        &[
            meta_line(ROOT_ID, serde_json::json!({})),
            message_line(1, "m1", "user", "提问"),
        ],
    );
    let db = open_db("intent-retry");
    enable_codex_source(&db, &root_dir);
    reconcile(&db);
    let session = db
        .find_session_by_root_agent_id(Agent::Codex, ROOT_ID)
        .unwrap()
        .unwrap();
    let owner = support_workstream("ws-intent-retry");
    db.upsert_workstream(&owner).unwrap();
    let intent = LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent: Agent::Codex,
        owner_workstream_id: Some(owner.id.clone()),
        cwd: Some("/repo-a".into()),
        process_id: None,
        launched_at: chrono_now(),
        matched_session_id: None,
        status: "pending".into(),
        note: String::new(),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_launch_intent(&intent).unwrap();

    reconcile(&db);

    assert_eq!(
        db.get_launch_intent(&intent.id).unwrap().unwrap().status,
        "matched"
    );
    assert_eq!(
        db.get_session(&session.id)
            .unwrap()
            .unwrap()
            .owner_workstream_id
            .as_deref(),
        Some(owner.id.as_str())
    );
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
        source_position: String::new(),
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
}

/// An append that continues the same turn demotes the formerly-final reply.
#[test]
fn turn_final_flags_track_the_projection() {
    let db = open_db("turn-final");
    let (session, _) = seed_root(&db);
    let msg = |id: &str, role: SessionMessageRole, text: &str| ParsedSessionMessage {
        source_message_id: Some(id.into()),
        source_position: String::new(),
        ts: None,
        role,
        content: text.into(),
    };
    let flags_of = |db: &Db, id: &str| -> bool {
        db.read()
            .query_row(
                "SELECT turn_final FROM session_messages WHERE source_message_id = ?1",
                [id],
                |r| r.get::<_, i64>(0),
            )
            .map(|v| v != 0)
            .unwrap()
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
    assert!(!flags_of(&db, "u1"));
    assert!(!flags_of(&db, "a1"), "the turn continues after it");
    assert!(
        flags_of(&db, "a2"),
        "the user spoke again → a2 was the final"
    );
    assert!(!flags_of(&db, "u2"));
    assert!(flags_of(&db, "a3"), "the conversation ends → a3 is final");

    // The turn continues: the appended reply takes the final flag and a3 is
    // demoted.
    db.commit_ingest(
        &session.id,
        &[msg("a4", SessionMessageRole::Assistant, "补充")],
        &seed_update(2, 700),
    )
    .unwrap();
    assert!(!flags_of(&db, "a3"), "the continuation demoted it");
    assert!(flags_of(&db, "a4"));
}

/// Append duplicate: the same messages committed again store nothing new
/// (identity dedup,).
#[test]
fn duplicate_commits_dedup() {
    let db = open_db("dedup");
    let (session, first) = seed_root(&db);
    assert_eq!(first.len(), 1);
    let first_seq = first[0].sequence;

    // Same source again (a full re-scan: start at genesis with a fresh
    // generation) — the identical message dedups, nothing appends. Every
    // identity input (id, role, ts, content) must equal the seed's.
    let again = db
        .commit_ingest(
            &session.id,
            &[noending::domain::ParsedSessionMessage {
                source_message_id: Some("m1".into()),
                source_position: "line:1".into(),
                ts: Some("2026-09-20T13:01:48Z".into()),
                role: SessionMessageRole::User,
                content: "第一次的提问".into(),
            }],
            &seed_update(1, 40),
        )
        .unwrap();
    assert!(again.is_empty(), "identical content stores nothing");
    assert_eq!(db.message_count(&session.id).unwrap(), 1);
    let messages = db.get_messages(&session.id, None, 10).unwrap();
    assert_eq!(messages[0].sequence, first_seq, "sequence never re-issued");
}

/// A source rewrite (new generation, full rescan): the old conversation is
/// retained, identical messages dedup, genuinely new ones append.
#[test]
fn a_full_rescan_keeps_history_and_replaces_the_projection() {
    let db = open_db("rescan");
    let (session, _) = seed_root(&db);

    // Generation 1 rescan: the same user message (dedup) plus a new reply.
    let stored = db
        .commit_ingest(
            &session.id,
            &[
                ParsedSessionMessage {
                    source_message_id: Some("m1".into()),
                    source_position: "line:1".into(),
                    ts: Some("2026-09-20T13:01:48Z".into()),
                    role: SessionMessageRole::User,
                    content: "第一次的提问".into(),
                },
                ParsedSessionMessage {
                    source_message_id: Some("m2".into()),
                    source_position: "line:2".into(),
                    ts: None,
                    role: SessionMessageRole::Assistant,
                    content: "重扫描后的新回答".into(),
                },
            ],
            &seed_update(1, 80),
        )
        .unwrap();
    assert_eq!(stored.len(), 1, "only the new reply appends");
    assert_eq!(db.message_count(&session.id).unwrap(), 2);
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
                    source_position: "line:1".into(),
                    ts: Some("2026-09-20T13:01:48Z".into()),
                    role: SessionMessageRole::User,
                    content: "第一次的提问".into(),
                },
                ParsedSessionMessage {
                    source_message_id: Some("old".into()),
                    source_position: "line:0".into(),
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
                    source_position: "line:2".into(),
                    ts: Some("2027-01-01T00:00:00Z".into()),
                    role: SessionMessageRole::Assistant,
                    content: "带时间戳的新回复".into(),
                },
                ParsedSessionMessage {
                    source_message_id: Some("m3".into()),
                    source_position: "line:3".into(),
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
            owner_workstream_id: None,
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
                source_position: "line:1".into(),
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
    enable_codex_source(&db, &root_dir);
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
