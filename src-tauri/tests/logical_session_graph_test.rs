//! Logical Session graph resolution end-to-end tests (重构方案 §10/§11/§13 and
//! the §32.3–§32.5/§32.10 test matrix): discovery batch → root/child/side
//! resolution → diagnostics → atomic member commit → LaunchIntent root-only.
//!
//! Fixtures are Codex rollouts (plain JSONL): the one adapter whose sources
//! express child threads (`thread_source=subagent`), side threads
//! (`guardian_review`) and forked pages (`history_base`) — so one fixture
//! family covers the whole graph vocabulary.

use std::path::{Path, PathBuf};

use noending::adapters::all_adapters;
use noending::domain::{
    Agent, LaunchIntent, ParsedSessionMessage, SessionMemberRelation, SessionMessageRole,
};
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
    let engine = noending::sync::SyncEngine::default();
    ingestion::reconcile_with_engine(db, &engine, &LaunchWorkspace::default(), &|_| {}).unwrap()
}

const ROOT_ID: &str = "019f135a-621c-76a1-a76c-7c71021847aa";
const CHILD_ID: &str = "019fbbd3-47be-78f2-89fd-9deec2e4c6d9";
const SIDE_ID: &str = "019fccd3-47be-78f2-89fd-9deec2e4c6d9";
const FORK_ID: &str = "01a0c943-53b8-7e82-8f84-0c2b33da8801";

#[test]
fn logical_root_creation_rolls_back_if_root_member_cannot_be_written() {
    let db = open_db("root-transaction");
    db.write()
        .execute_batch(
            "CREATE TRIGGER reject_root_member BEFORE INSERT ON session_members
             WHEN NEW.relation = 'root'
             BEGIN SELECT RAISE(ABORT, 'test root insert failure'); END;",
        )
        .unwrap();

    assert!(db
        .upsert_logical_root(
            Agent::Codex,
            ROOT_ID,
            None,
            None,
            None,
            None,
            None,
            None,
            "test",
            "/tmp/root",
            None,
            &serde_json::json!({}),
        )
        .is_err());
    assert!(db
        .find_session_by_root_agent_id(Agent::Codex, ROOT_ID)
        .unwrap()
        .is_none());
}

// ---------------------------------------------------------------------------
// §32.3 — child/side resolution and diagnostics
// ---------------------------------------------------------------------------

/// A child discovered with no root anywhere: no Logical Session is created
/// (no root, no session), the child is a diagnostic — not a fake root.
#[test]
fn a_child_without_a_root_is_a_diagnostic_not_a_session() {
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
    let diags = db.list_ingestion_diagnostics(1).unwrap();
    assert_eq!(diags.len(), 1, "the orphan child is recorded: {diags:?}");
    assert_eq!(diags[0].source_member_id.as_deref(), Some(CHILD_ID));
}

/// The root shows up in a LATER batch: one Logical Session, the child (and a
/// side) attach to it, and the diagnostic from the earlier pass is removed.
/// Scan order never decides identity (§10.2).
#[test]
fn a_root_discovered_later_attaches_the_child_and_resolves_the_diagnostic() {
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
    assert_eq!(db.list_ingestion_diagnostics(1).unwrap().len(), 1);

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
    let members = db.members_for_session(&sessions[0].id).unwrap();
    let relations: std::collections::HashMap<&str, &str> = members
        .iter()
        .map(|m| (m.source_member_id.as_str(), m.relation.as_str()))
        .collect();
    assert_eq!(relations.get(ROOT_ID), Some(&"root"));
    assert_eq!(relations.get(CHILD_ID), Some(&"child"));
    assert_eq!(relations.get(SIDE_ID), Some(&"side"));
    assert!(
        db.list_ingestion_diagnostics(1).unwrap().is_empty(),
        "resolved members leave no diagnostic behind"
    );
}

/// Child transcript text never becomes conversation (§6/§26.1); only the
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
    // Every stored message points at the ROOT member.
    let root_member = db
        .root_member_for_session(&sessions[0].id)
        .unwrap()
        .unwrap();
    assert!(messages.iter().all(|m| m.member_id == root_member.id));
}

/// A child whose cwd differs never moves the Session's physical location
/// (§4.1/§18): Session.cwd is Root authority only.
#[test]
fn a_child_cwd_never_moves_the_session() {
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
        "child cwd is execution fact on the member row, never Session.cwd"
    );
    let child = db
        .find_member_by_source_id(Agent::Codex, CHILD_ID)
        .unwrap()
        .unwrap();
    assert_eq!(child.cwd.as_deref(), Some("/repo-b-child"));
}

/// Child activity moves `last_activity_at` (the whole graph's activity) but
/// never `last_conversation_at` (root conversation only) (§4.1).
#[test]
fn child_activity_moves_last_activity_but_not_last_conversation() {
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
    let conversation_before = before.last_conversation_at.clone();
    let child = db
        .find_member_by_source_id(Agent::Codex, CHILD_ID)
        .unwrap()
        .unwrap();
    let cursor_before = db.get_member_cursor(&child.id).unwrap();

    // A no-change read is not source activity.
    let engine = noending::sync::SyncEngine::default();
    ingestion::ingest_and_sync_session(&db, &engine, &before).unwrap();
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
    let cursor_after = db.get_member_cursor(&child.id).unwrap();
    assert_eq!(
        after.last_conversation_at, conversation_before,
        "child chatter is not conversation"
    );
    assert!(cursor_after.byte_offset > cursor_before.byte_offset);
    assert!(after.last_activity_at > before.last_activity_at);
}

#[test]
fn trash_freezes_discovery_until_restore() {
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
    let root_member = db.root_member_for_session(&session.id).unwrap().unwrap();
    let before_cursor = db.get_member_cursor(&root_member.id).unwrap();
    let before_members = db.members_for_session(&session.id).unwrap();
    noending::lifecycle::trash_session(&db, &session.id).unwrap();

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

    let frozen = db.get_session(&session.id).unwrap().unwrap();
    assert_eq!(frozen.cwd, session.cwd);
    assert_eq!(frozen.last_activity_at, session.last_activity_at);
    assert_eq!(
        db.members_for_session(&session.id)
            .unwrap()
            .iter()
            .map(|m| m.id.clone())
            .collect::<Vec<_>>(),
        before_members
            .iter()
            .map(|m| m.id.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        db.get_member_cursor(&root_member.id).unwrap().byte_offset,
        before_cursor.byte_offset
    );

    noending::lifecycle::restore_session(&db, &session.id).unwrap();
    reconcile(&db);
    assert_eq!(
        db.get_session(&session.id).unwrap().unwrap().cwd.as_deref(),
        Some("/repo-after-restore")
    );
    assert!(db
        .find_member_by_source_id(Agent::Codex, CHILD_ID)
        .unwrap()
        .is_some());
    assert!(db.get_member_cursor(&root_member.id).unwrap().byte_offset > before_cursor.byte_offset);
}

// ---------------------------------------------------------------------------
// §32.4 — Fork
// ---------------------------------------------------------------------------

/// A forked page becomes its OWN Logical Session with the fork source recorded
/// — and the source session's Owner is NOT inherited (§2.2/§17.3).
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
    // Both members are `root` relations of their own sessions.
    let fork_member = db
        .find_member_by_source_id(Agent::Codex, FORK_ID)
        .unwrap()
        .unwrap();
    assert_eq!(fork_member.relation, SessionMemberRelation::Root);
    assert_eq!(fork_member.session_id, fork.id);

    // Now give the PARENT an owner via the explicit user door and re-reconcile:
    // ownership is decided once, by explicit action or a matched intent —
    // discovery never copies it onto the fork (§2.2).
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
        lifecycle: "active".into(),
        visibility: "normal".into(),
        created_at: now(),
        updated_at: now(),
    }
}

/// §17.1 — LaunchIntent matching is ROOT-only: a child/side discovery can
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
        context_bundle_markdown: None,
        context_bundle_revisions: None,
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
        context_bundle_markdown: None,
        context_bundle_revisions: None,
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

#[test]
fn owner_assignment_replays_pending_context_on_reconcile() {
    let root_dir = temp_root("owner-replay");
    write_rollout(
        &root_dir,
        &rollout_name(ROOT_ID),
        &[
            meta_line(ROOT_ID, serde_json::json!({})),
            message_line(1, "m1", "user", "请记住这个重要约定"),
            message_line(2, "m2", "assistant", "我会记住"),
        ],
    );
    let db = open_db("owner-replay");
    enable_codex_source(&db, &root_dir);
    reconcile(&db);
    let session = db
        .find_session_by_root_agent_id(Agent::Codex, ROOT_ID)
        .unwrap()
        .unwrap();
    assert_eq!(
        db.get_context_state(&session.id)
            .unwrap()
            .processed_message_sequence,
        0
    );

    let owner = support_workstream("ws-owner-replay");
    db.upsert_workstream(&owner).unwrap();
    db.set_session_owner(&session.id, Some(&owner.id)).unwrap();
    noending::settings::set_context_intelligence_enabled(&db, true).unwrap();
    reconcile(&db);

    assert_eq!(
        db.get_context_state(&session.id)
            .unwrap()
            .processed_message_sequence,
        db.get_messages(&session.id, None, 100)
            .unwrap()
            .last()
            .unwrap()
            .sequence
    );
}

// ---------------------------------------------------------------------------
// §32.5 — ingestion atomicity
// ---------------------------------------------------------------------------

/// A Trash racing a member commit: the whole delta is refused — no messages,
/// no stats, no cursor move (§13: trash racing ingestion commits either all
/// of it or none of it).
#[test]
fn a_trash_racing_the_commit_takes_nothing() {
    let db = open_db("trash-race");
    let (session, member_id, _) = seed_root(&db);

    let delta = vec![ParsedSessionMessage {
        source_message_id: Some("m1".into()),
        source_position: "line:1".into(),
        ts: None,
        role: SessionMessageRole::User,
        content: "并发期间的提问".into(),
    }];
    noending::lifecycle::trash_session(&db, &session.id).unwrap();
    // The racy delta would advance the cursor to 99 if it were accepted.
    let stored = db
        .commit_member_ingest(&session.id, &member_id, &delta, None, &seed_update(1, 99))
        .unwrap();
    assert!(stored.is_empty(), "trashed session takes nothing");
    assert_eq!(
        db.message_count(&session.id).unwrap(),
        1,
        "the seed message stays; the racy delta added nothing"
    );
    let cursor = db.get_member_cursor(&member_id).unwrap();
    assert_eq!(
        cursor.byte_offset, 40,
        "the cursor stayed exactly where the seed left it"
    );
}

/// A member that moved to another session while its delta was being prepared:
/// the stale commit is rejected in full (§13.2).
#[test]
fn a_stale_commit_after_topology_correction_is_rejected() {
    let db = open_db("topology-race");
    let (session, member_id, _) = seed_root(&db);

    // Topology correction: the member is re-pointed at another session.
    let (other_id, _) = db
        .upsert_logical_session(
            Agent::Codex,
            "other-root",
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
    db.upsert_session_member(
        &other_id,
        Agent::Codex,
        "moved-member",
        SessionMemberRelation::Child,
        None,
        "codex_rollout",
        "/tmp/moved",
        None,
        None,
        None,
        &serde_json::json!({}),
    )
    .unwrap();
    // The correction moves OUR member: simulate by re-upserting it under the
    // other session (same identity).
    db.upsert_session_member(
        &other_id,
        Agent::Codex,
        &db.get_member(&member_id).unwrap().unwrap().source_member_id,
        SessionMemberRelation::Child,
        None,
        "codex_rollout",
        "/tmp/moved",
        None,
        None,
        None,
        &serde_json::json!({}),
    )
    .unwrap();

    let delta = vec![ParsedSessionMessage {
        source_message_id: Some("m1".into()),
        source_position: "line:1".into(),
        ts: None,
        role: SessionMessageRole::User,
        content: "过期提交的提问".into(),
    }];
    let stored = db
        .commit_member_ingest(&session.id, &member_id, &delta, None, &seed_update(1, 40))
        .unwrap();
    assert!(stored.is_empty(), "the stale commit stores nothing");
    assert_eq!(
        db.message_count(&session.id).unwrap(),
        1,
        "the seed message stays; the stale delta added nothing"
    );
}

/// Append duplicate: the same messages committed again store nothing new
/// (identity dedup, §14).
#[test]
fn duplicate_commits_dedup() {
    let db = open_db("dedup");
    let (session, member_id, first) = seed_root(&db);
    assert_eq!(first.len(), 1);
    let first_seq = first[0].sequence;

    // Same source again (a full re-scan: start at genesis with a fresh
    // generation) — the identical message dedups, nothing appends. Every
    // identity input (id, role, ts, content) must equal the seed's.
    let again = db
        .commit_member_ingest(
            &session.id,
            &member_id,
            &[noending::domain::ParsedSessionMessage {
                source_message_id: Some("m1".into()),
                source_position: "line:1".into(),
                ts: Some("2026-09-20T13:01:48Z".into()),
                role: SessionMessageRole::User,
                content: "第一次的提问".into(),
            }],
            None,
            &seed_update(1, 40),
        )
        .unwrap();
    assert!(again.is_empty(), "identical content stores nothing");
    assert_eq!(db.message_count(&session.id).unwrap(), 1);
    let messages = db.get_messages(&session.id, None, 10).unwrap();
    assert_eq!(messages[0].sequence, first_seq, "sequence never re-issued");
}

/// A source rewrite (new generation, full rescan): the old conversation is
/// retained, identical messages dedup, genuinely new ones append — and a
/// stats snapshot replaces the previous counters atomically (§14/§7.3).
#[test]
fn a_full_rescan_keeps_history_and_replaces_the_stats_snapshot() {
    let db = open_db("rescan");
    let (session, member_id, _) = seed_root(&db);

    // Generation 1 rescan: the same user message (dedup) plus a new reply.
    let stored = db
        .commit_member_ingest(
            &session.id,
            &member_id,
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
            None,
            &seed_update(1, 80),
        )
        .unwrap();
    assert_eq!(stored.len(), 1, "only the new reply appends");
    assert_eq!(db.message_count(&session.id).unwrap(), 2);

    // The snapshot replaces the counters wholesale.
    db.commit_member_ingest(
        &session.id,
        &member_id,
        &[],
        Some(noending::domain::StatsUpdate::Snapshot(
            noending::domain::SessionMemberStatsSnapshot {
                tool_call_count: 7,
                tool_error_count: 0,
                compaction_count: 1,
                side_activity_count: 2,
            },
        )),
        &seed_update(1, 80),
    )
    .unwrap();
    let stats = db.get_member_stats(&member_id).unwrap().unwrap();
    assert_eq!(stats.tool_call_count, Some(7));
    assert_eq!(stats.compaction_count, Some(1));
    assert_eq!(stats.side_activity_count, Some(2));
}

#[test]
fn empty_full_scan_replaces_old_stats_with_zeroes() {
    let db = open_db("zero-stats-rescan");
    let (session, member_id, _) = seed_root(&db);
    db.commit_member_ingest(
        &session.id,
        &member_id,
        &[],
        Some(noending::domain::StatsUpdate::Snapshot(
            noending::domain::SessionMemberStatsSnapshot {
                tool_call_count: 7,
                tool_error_count: 3,
                compaction_count: 2,
                side_activity_count: 4,
            },
        )),
        &seed_update(1, 40),
    )
    .unwrap();
    let source = seed_update(2, 40);
    let stats = noending::adapters::stats_update_from(
        &noending::domain::MemberObservation::default(),
        &source,
    );
    db.commit_member_ingest(&session.id, &member_id, &[], stats, &source)
        .unwrap();

    let stats = db.get_member_stats(&member_id).unwrap().unwrap();
    assert_eq!(stats.tool_call_count, Some(0));
    assert_eq!(stats.tool_error_count, Some(0));
    assert_eq!(stats.compaction_count, Some(0));
    assert_eq!(stats.side_activity_count, Some(0));
}

#[test]
fn older_history_from_a_rescan_cannot_move_conversation_time_backwards() {
    let db = open_db("conversation-monotonic");
    let (session, member_id, _) = seed_root(&db);
    let before = db
        .get_session(&session.id)
        .unwrap()
        .unwrap()
        .last_conversation_at;
    let stored = db
        .commit_member_ingest(
            &session.id,
            &member_id,
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
            None,
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

/// §32.5 — the core's last line of defense: messages handed to a CHILD member
/// are an error, never silently stored (§6).
#[test]
fn a_child_member_cannot_write_conversation() {
    let db = open_db("child-write");
    let (session, _, _) = seed_root(&db);
    let child_member = db
        .upsert_session_member(
            &session.id,
            Agent::Codex,
            CHILD_ID,
            SessionMemberRelation::Child,
            Some(ROOT_ID),
            "codex_rollout",
            "/tmp/child",
            None,
            None,
            None,
            &serde_json::json!({}),
        )
        .unwrap();
    let delta = vec![ParsedSessionMessage {
        source_message_id: Some("c1".into()),
        source_position: "line:1".into(),
        ts: None,
        role: SessionMessageRole::Assistant,
        content: "子成员试图写会话".into(),
    }];
    let err = db
        .commit_member_ingest(
            &session.id,
            &child_member,
            &delta,
            None,
            &seed_update(0, 10),
        )
        .unwrap_err();
    assert!(
        err.to_string().contains("root"),
        "the refusal names the invariant: {err}"
    );
    assert_eq!(db.message_count(&session.id).unwrap(), 1, "only the seed");
}

// ---------------------------------------------------------------------------
// §32.10 — diagnostics stay out of everything
// ---------------------------------------------------------------------------

/// A repeat offender becomes visible at observation_count >= 2, and resolving
/// the member removes it. Diagnostics never surface in search.
#[test]
fn diagnostics_are_repeat_visible_and_resolvable() {
    let root_dir = temp_root("diag");
    write_rollout(
        &root_dir,
        &rollout_name(CHILD_ID),
        &[meta_line(
            CHILD_ID,
            serde_json::json!({"thread_source": "subagent", "parent_thread_id": ROOT_ID}),
        )],
    );
    let db = open_db("diag");
    enable_codex_source(&db, &root_dir);

    reconcile(&db);
    reconcile(&db);
    let visible = db.list_ingestion_diagnostics(2).unwrap();
    assert_eq!(visible.len(), 1, "two sightings make it visible");
    assert_eq!(visible[0].observation_count, 2);
    let reason = visible[0].reason.clone();

    // Diagnostics are not searchable documents (§11: no Search).
    let hits = noending::search::search(&db, &reason, 20).unwrap();
    assert!(
        hits.is_empty(),
        "a diagnostic must never surface in search: {hits:?}"
    );

    // The root arrives → attach → the diagnostic is gone.
    write_rollout(
        &root_dir,
        &rollout_name(ROOT_ID),
        &[
            meta_line(ROOT_ID, serde_json::json!({})),
            message_line(1, "m1", "user", "提问"),
        ],
    );
    reconcile(&db);
    assert!(db.list_ingestion_diagnostics(1).unwrap().is_empty());
}

/// Discovery never produces a member for an adapter that cannot identify one:
/// the roster still works end to end (a smoke check over every adapter with a
/// fabricated impossible source — nothing may panic).
#[test]
fn every_adapter_inspects_sources_without_panicking() {
    let db = open_db("inspect-all");
    let (session, _, _) = seed_root(&db);
    for adapter in all_adapters() {
        let member = noending::domain::SessionMember {
            id: new_id(),
            session_id: session.id.clone(),
            agent: adapter.agent(),
            source_member_id: "no-such-member".into(),
            relation: SessionMemberRelation::Root,
            parent_source_member_id: None,
            source_kind: "test".into(),
            source_path: "/tmp/definitely-not-here".into(),
            cwd: None,
            started_at: None,
            last_activity_at: None,
            metadata: serde_json::json!({}),
        };
        // A missing FILE is Missing for file adapters; a missing RECORD in a
        // store adapter is only answerable when the store itself exists —
        // either way, no panic and never a false Present.
        let verdict = adapter.inspect_member_source(&member).unwrap();
        assert_ne!(verdict, noending::domain::SourceAvailability::Present);
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Seed a logical session + root member + one message, the way production
/// would after discovery (via the same storage APIs ingestion uses).
fn seed_root(
    db: &Db,
) -> (
    noending::domain::Session,
    String,
    Vec<noending::domain::SessionMessage>,
) {
    let (session_id, _) = db
        .upsert_logical_session(Agent::Codex, ROOT_ID, None, None, None, None, None, None)
        .unwrap();
    let member_id = db
        .upsert_session_member(
            &session_id,
            Agent::Codex,
            ROOT_ID,
            SessionMemberRelation::Root,
            None,
            "codex_rollout",
            "/repo-a/rollout.jsonl",
            Some("/repo-a"),
            None,
            None,
            &serde_json::json!({}),
        )
        .unwrap();
    let stored = db
        .commit_member_ingest(
            &session_id,
            &member_id,
            &[ParsedSessionMessage {
                source_message_id: Some("m1".into()),
                source_position: "line:1".into(),
                ts: Some("2026-09-20T13:01:48Z".into()),
                role: SessionMessageRole::User,
                content: "第一次的提问".into(),
            }],
            None,
            &seed_update(0, 40),
        )
        .unwrap();
    let session = db.get_session(&session_id).unwrap().unwrap();
    (session, member_id, stored)
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
