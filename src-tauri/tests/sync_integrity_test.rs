//! Sync integrity tests (Issues #3 + #4) — Logical Session model.
//!
//! - one SyncRun = one transaction: any failure rolls back mutations, the
//!   SyncRun row and the Context frontier together;
//! - member cursors (ingested) and the Context frontier (synced) are separate
//!   lifecycles;
//! - completed runs are idempotent under retry (delta fingerprint);
//! - the unified AuthorityPolicy: agents never silently overwrite
//!   user_explicit / user_edit items — they create ContextConflicts;
//! - every status change leaves an audit revision;
//! - commit re-verifies trash / frontier / Owner inside its transaction.

use noending::domain::{Agent, ParsedSessionMessage, Session, SessionMessage, SessionMessageRole};
use noending::storage::{
    insert_sync_run_conn, new_id, now, set_processed_message_sequence_conn, Db,
};
use noending::sync::merge::MergeEngine;
use noending::sync::{create_item, ContextMutation, MergeContext};

mod support;

fn open_db(tag: &str) -> Db {
    let dir = std::env::temp_dir().join(format!("noending-integrity-{}-{}", tag, new_id()));
    Db::open(&dir.join("test.db")).unwrap()
}

/// A Logical Session + ROOT member seeded through the production commit path,
/// with one stored user message per text. The member cursor starts as an
/// append past genesis, so later batches chain from the committed tail.
fn session_with_messages(
    db: &Db,
    root_agent_session_id: &str,
    texts: &[&str],
) -> (Session, String, Vec<SessionMessage>) {
    let (s, member_id, stored) = support::seed_conversation(
        db,
        Agent::Codex,
        root_agent_session_id,
        &texts
            .iter()
            .enumerate()
            .map(|(i, t)| {
                support::parsed_message(
                    format!("msg-{root_agent_session_id}-{i}"),
                    SessionMessageRole::User,
                    *t,
                )
            })
            .collect::<Vec<ParsedSessionMessage>>(),
    );
    (s, member_id, stored)
}

fn ws_row(db: &Db, id: &str, title: &str) -> noending::domain::Workstream {
    let w = support::workstream(id.into(), title);
    db.upsert_workstream(&w).unwrap();
    w
}

fn user_item(
    db: &Db,
    ws_id: &str,
    authority: &str,
    title: &str,
    content: &str,
) -> noending::domain::ContextItem {
    create_item(
        db,
        ws_id,
        "constraint",
        title,
        content,
        authority,
        "user_edit",
        &[],
        None,
        "user",
    )
    .unwrap()
}

// ---------------------------------------------------------------------------
// Issue #3: transactions
// ---------------------------------------------------------------------------

/// A DB failure anywhere in the run's transaction must roll back the whole
/// run: earlier mutations, the SyncRun row and the Context frontier all
/// disappear.
///
/// The failure is injected at the frontier write (a Session that does not
/// exist), not by an out-of-owner mutation: since the single-owner boundary
/// landed, a mutation aimed at another Workstream is refused deterministically
/// and SKIPPED — it never reaches the FK, so it is no longer a way to fail a
/// run (that behaviour is pinned by
/// `mutations_outside_the_owner_are_skipped_not_written`).
#[test]
fn mutation_failure_rolls_back_entire_run() {
    let db = open_db("rollback");
    let (s, _member, _msgs) = session_with_messages(&db, "rollback-root", &["seed"]);
    let ws = ws_row(&db, "ws-rollback", "real workstream");

    let mutations = vec![ContextMutation::Add {
        workstream_id: ws.id.clone(),
        item_kind: "note".into(),
        title: "first mutation".into(),
        content: "ok".into(),
        source_refs: vec![],
        authority: "agent_inferred".into(),
    }];

    let ctx = MergeContext {
        run_id: new_id(),
        runtime: "heuristic".into(),
        workstream_id: ws.id.clone(),
    };
    let run = noending::domain::SyncRun {
        id: ctx.run_id.clone(),
        session_id: s.id.clone(),
        from_sequence: 0,
        to_sequence: 5,
        status: "ok".into(),
        mutations: serde_json::json!([]),
        summary: "should never persist".into(),
        error: None,
        created_at: now(),
        runtime: "heuristic".into(),
        delta_fingerprint: Some("fp".into()),
    };

    let result = db.tx(|tx| {
        for m in &mutations {
            MergeEngine.apply(tx, m, &ctx)?;
        }
        insert_sync_run_conn(tx, &run)?;
        // FK violation: there is no such Session to advance.
        set_processed_message_sequence_conn(tx, "missing-session", 5)?;
        Ok(())
    });
    assert!(
        result.is_err(),
        "the FK violation must fail the transaction"
    );

    // everything rolled back
    assert!(
        db.items_for_workstream(&ws.id, true).unwrap().is_empty(),
        "the applied mutation must NOT survive the rollback"
    );
    assert_eq!(db.list_sync_runs(50).unwrap().len(), 0, "no SyncRun row");
    assert_eq!(
        db.get_context_state(&s.id)
            .unwrap()
            .processed_message_sequence,
        0,
        "Context frontier unchanged"
    );
}

/// Retrying after a rollback applies the batch exactly once; retrying an
/// already-committed delta is recognized via the fingerprint.
#[test]
fn retry_after_rollback_applies_once_and_commit_is_idempotent() {
    let db = open_db("retry");
    let (_s, _member, _msgs) = session_with_messages(&db, "retry-root", &["seed"]);
    let ws = ws_row(&db, "ws-retry", "retry ws");

    let mk_mutations = || {
        vec![ContextMutation::Add {
            workstream_id: ws.id.clone(),
            item_kind: "note".into(),
            title: "only good mutation".into(),
            content: "fine".into(),
            source_refs: vec![],
            authority: "agent_inferred".into(),
        }]
    };

    // 1st attempt fails after the mutation was applied
    let ctx = MergeContext {
        run_id: new_id(),
        runtime: "heuristic".into(),
        workstream_id: ws.id.clone(),
    };
    let failed = db.tx(|tx| {
        for m in &mk_mutations() {
            MergeEngine.apply(tx, m, &ctx)?;
        }
        set_processed_message_sequence_conn(tx, "missing-session", 1)
    });
    assert!(failed.is_err());
    assert!(
        db.items_for_workstream(&ws.id, true).unwrap().is_empty(),
        "the failed attempt left nothing behind"
    );

    // retry with only the good mutation succeeds exactly once
    let ctx2 = MergeContext {
        run_id: new_id(),
        runtime: "heuristic".into(),
        workstream_id: ws.id.clone(),
    };
    db.tx(|tx| {
        for m in mk_mutations() {
            MergeEngine.apply(tx, &m, &ctx2)?;
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(db.items_for_workstream(&ws.id, true).unwrap().len(), 1);

    // applying the same mutation again is a deterministic skip (dedup)
    let ctx3 = MergeContext {
        run_id: new_id(),
        runtime: "heuristic".into(),
        workstream_id: ws.id.clone(),
    };
    let applied_again = db
        .tx(|tx| {
            let mut applied = 0;
            for m in mk_mutations() {
                if MergeEngine.apply(tx, &m, &ctx3)? {
                    applied += 1;
                }
            }
            Ok(applied)
        })
        .unwrap();
    assert_eq!(applied_again, 0, "identical re-apply is a no-op");
    assert_eq!(db.items_for_workstream(&ws.id, true).unwrap().len(), 1);
}

/// Member cursors and the Context frontier are independent: ingested-but-
/// unsynced messages exist, survive, and are picked up by the next sync pass.
#[test]
fn member_cursor_and_context_frontier_are_separate() {
    let db = open_db("cursors");
    let (s, member_id, _stored) = session_with_messages(
        &db,
        "cursors-root",
        &["我们决定使用 PostgreSQL 作为主数据库，不再使用 SQLite 存储业务数据"],
    );
    let ws = ws_row(&db, "ws-cursors", "cursor ws");
    // Routing is the Session's single Owner Workstream (方案 §19).
    db.set_session_owner(&s.id, Some(&ws.id)).unwrap();

    let parsed = |id: String, text: &str| ParsedSessionMessage {
        provider: None,
        model: None,
        source_message_id: Some(id),
        source_position: String::new(),
        ts: Some(now()),
        role: SessionMessageRole::User,
        content: text.into(),
    };
    let source = |offset: u64| noending::domain::SourceCursorUpdate {
        file_identity: "dev:1:ino:9".into(),
        generation: 0,
        byte_offset: offset,
        last_seen_size: offset,
        mtime: None,
        start_byte_offset: offset,
        prefix_hash: String::new(),
    };

    // ingest batch 2 (real user message so the extractor produces mutations)
    let batch2 = vec![parsed(
        format!("cursors-root-b2"),
        "补充约束：不能把密钥提交到代码仓库，必须使用环境变量",
    )];
    let stored2 = db
        .commit_member_ingest(&s.id, &member_id, &batch2, None, &source(120))
        .unwrap();
    assert_eq!(stored2.len(), 1);
    assert_eq!(
        db.get_member_cursor(&member_id).unwrap().byte_offset,
        120,
        "member (read) cursor advanced"
    );
    assert_eq!(
        db.get_context_state(&s.id)
            .unwrap()
            .processed_message_sequence,
        0,
        "Context frontier behind"
    );

    // ingest batch 3 — still not synced (simulates a crash after ingest)
    let batch3 = vec![parsed(
        format!("cursors-root-b3"),
        "下一步要实现增量读取的边界测试",
    )];
    let stored3 = db
        .commit_member_ingest(&s.id, &member_id, &batch3, None, &source(200))
        .unwrap();
    assert_eq!(stored3.len(), 1);
    assert_eq!(
        db.get_context_state(&s.id)
            .unwrap()
            .processed_message_sequence,
        0
    );

    // now sync everything after the frontier in one run
    let processed = db
        .get_context_state(&s.id)
        .unwrap()
        .processed_message_sequence;
    let pending = db.get_messages_after(&s.id, processed, 10_000).unwrap();
    assert_eq!(pending.len(), 3, "all stored messages are pending");
    let engine = noending::sync::SyncEngine::default();
    let to = pending.last().unwrap().sequence;
    let out = engine
        .run_session_sync(&db, &s, &pending, processed, to)
        .unwrap();
    assert!(out.applied > 0);
    assert_eq!(
        db.get_context_state(&s.id)
            .unwrap()
            .processed_message_sequence,
        to,
        "Context frontier caught up"
    );
    assert_eq!(
        db.items_for_workstream(&ws.id, true).unwrap().len(),
        out.applied as usize,
        "exactly the applied mutations persisted"
    );
}

/// The single ingest+sync entry processes pending messages and advances the
/// Context frontier exactly like the interactive path.
/// (`ingest_and_sync_session` IS the one reconcile path.)
#[test]
fn ingest_and_sync_path_processes_pending_messages() {
    let db = open_db("nb-path");
    // The unified path reads the member's file delta first, so the ROOT
    // member needs a real (empty) source file behind its cursor.
    let dir = std::env::temp_dir().join(format!("noending-nb-{}", new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("source.jsonl");
    std::fs::write(&file, "").unwrap();

    let s = support::ensure_session(&db, new_id(), Agent::Codex, "nb-path-root");
    let member_id = support::ensure_root_member(
        &db,
        &s.id,
        Agent::Codex,
        "nb-path-root",
        &file.to_string_lossy(),
    );
    let ws = ws_row(&db, "ws-nb", "nb ws");
    db.set_session_owner(&s.id, Some(&ws.id)).unwrap();

    // one pending message (a real user message so extraction yields mutations)
    let stored = db
        .commit_member_ingest(
            &s.id,
            &member_id,
            &[support::parsed_message(
                "nb-msg-1",
                SessionMessageRole::User,
                "我们决定使用 PostgreSQL 作为主数据库，不再使用 SQLite 存储业务数据",
            )],
            None,
            &support::seed_source(0),
        )
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(
        db.get_context_state(&s.id)
            .unwrap()
            .processed_message_sequence,
        0
    );

    // The reconcile path IS ingest_and_sync_session. Intelligence gates the
    // sync half, so the switch is part of the setup.
    noending::settings::set_context_intelligence_enabled(&db, true).unwrap();
    let engine = noending::sync::SyncEngine::default();
    let (_ingested, applied) =
        noending::ingestion::ingest_and_sync_session(&db, &engine, &s).unwrap();
    assert!(
        applied > 0,
        "pending messages processed through the sync path"
    );

    assert_eq!(
        db.get_context_state(&s.id)
            .unwrap()
            .processed_message_sequence,
        1,
        "Context frontier advanced"
    );
    assert!(
        !db.items_for_workstream(&ws.id, true).unwrap().is_empty(),
        "mutations were committed"
    );
}

// ---------------------------------------------------------------------------
// Issue #4: authority policy integration
// ---------------------------------------------------------------------------

#[test]
fn agent_update_of_user_item_creates_conflict_not_overwrite() {
    let db = open_db("auth-update");
    let ws = ws_row(&db, "ws-auth", "auth ws");
    let created = user_item(
        &db,
        &ws.id,
        "user_edit",
        "保持 macOS 一等支持",
        "Windows 和 macOS 都是一等公民",
    );
    let item = db.get_item(&created.id).unwrap().unwrap();
    let original_head = item.current_revision_id.clone();

    let m = ContextMutation::Update {
        item_id: item.id.clone(),
        title: "只需要 macOS".into(),
        content: "This project only needs macOS".into(),
        source_refs: vec!["session-message:e1".into()],
        authority: "agent_inferred".into(),
    };
    let ctx = MergeContext {
        run_id: new_id(),
        runtime: "heuristic".into(),
        workstream_id: ws.id.clone(),
    };
    let applied = db.tx(|tx| MergeEngine.apply(tx, &m, &ctx)).unwrap();
    assert!(applied, "the disagreement is recorded (as a conflict)");

    let after = db.get_item(&item.id).unwrap().unwrap();
    assert_eq!(after.status, "active", "user item untouched");
    assert_eq!(
        after.current_revision_id, original_head,
        "content not replaced"
    );

    let conflicts = db.conflicts_for_workstream(&ws.id, false).unwrap();
    assert_eq!(conflicts.len(), 1, "a ContextConflict was created");
    assert_eq!(conflicts[0].left_item_id, item.id);
    assert_eq!(conflicts[0].status, "open");

    // the agent's version exists as its own evidence item
    let right_id = conflicts[0].right_item_id.clone().unwrap();
    let right = db.get_item(&right_id).unwrap().unwrap();
    assert_eq!(right.authority, "agent_inferred");
}

#[test]
fn agent_supersede_and_resolve_of_user_items_never_apply() {
    let db = open_db("auth-supersede");
    let ws = ws_row(&db, "ws-supersede", "supersede ws");
    let created = user_item(
        &db,
        &ws.id,
        "user_explicit",
        "数据库继续用 SQLite",
        "用户明确决定",
    );
    let item = db.get_item(&created.id).unwrap().unwrap();
    let original_head = item.current_revision_id.clone();

    let ctx = MergeContext {
        run_id: new_id(),
        runtime: "heuristic".into(),
        workstream_id: ws.id.clone(),
    };

    let sup = ContextMutation::Supersede {
        item_id: item.id.clone(),
        title: "改用 Postgres".into(),
        content: "agent wants postgres".into(),
        source_refs: vec![],
        authority: "agent_inferred".into(),
    };
    assert!(db.tx(|tx| MergeEngine.apply(tx, &sup, &ctx)).unwrap());
    let after = db.get_item(&item.id).unwrap().unwrap();
    assert_eq!(after.status, "active", "user_explicit not superseded");
    assert_eq!(after.current_revision_id, original_head);

    let res = ContextMutation::Resolve {
        item_id: item.id.clone(),
        source_refs: vec![],
    };
    assert!(db.tx(|tx| MergeEngine.apply(tx, &res, &ctx)).unwrap());
    let after = db.get_item(&item.id).unwrap().unwrap();
    assert_eq!(
        after.status, "active",
        "user_explicit not resolved by agent"
    );

    let conflicts = db.conflicts_for_workstream(&ws.id, false).unwrap();
    assert_eq!(
        conflicts.len(),
        2,
        "both attempts materialized as conflicts"
    );
}

#[test]
fn agent_may_evolve_agent_owned_items_with_full_trail() {
    let db = open_db("auth-agent-owned");
    let ws = ws_row(&db, "ws-agent-owned", "agent owned ws");
    // agent-owned item
    let item = create_item(
        &db,
        &ws.id,
        "note",
        "初步猜测",
        "guess v1",
        "agent_inferred",
        "session_message",
        &["session-message:e-old".into()],
        None,
        "sync:heuristic",
    )
    .unwrap();

    let ctx = MergeContext {
        run_id: new_id(),
        runtime: "heuristic".into(),
        workstream_id: ws.id.clone(),
    };
    let update = ContextMutation::Update {
        item_id: item.id.clone(),
        title: "初步猜测（修订）".into(),
        content: "guess v2".into(),
        source_refs: vec!["session-message:e-new".into()],
        authority: "agent_inferred".into(),
    };
    assert!(db.tx(|tx| MergeEngine.apply(tx, &update, &ctx)).unwrap());
    let after = db.get_item(&item.id).unwrap().unwrap();
    assert_ne!(
        after.current_revision_id, item.current_revision_id,
        "revision advanced"
    );

    // supersede of an agent item: allowed, leaves audit revision on the old item
    let sup = ContextMutation::Supersede {
        item_id: item.id.clone(),
        title: "结论".into(),
        content: "final".into(),
        source_refs: vec![],
        authority: "agent_inferred".into(),
    };
    assert!(db.tx(|tx| MergeEngine.apply(tx, &sup, &ctx)).unwrap());
    let old = db.get_item(&item.id).unwrap().unwrap();
    assert_eq!(old.status, "superseded");

    let history = db.item_history(&item.id).unwrap();
    let audit = history
        .iter()
        .find(|r| r.source_type.as_deref() == Some("status_change"))
        .expect("status change leaves an audit revision");
    let a = &audit.metadata["audit"];
    assert_eq!(a["previous_status"], "active");
    assert_eq!(a["new_status"], "superseded");
    assert!(a["actor"].as_str().unwrap().starts_with("sync:"));

    // the superseding item links back
    let new_items = db.items_for_workstream(&ws.id, true).unwrap();
    assert!(new_items
        .iter()
        .any(|(i, _)| i.supersedes_item_id.as_deref() == Some(item.id.as_str())));
}

#[test]
fn user_status_changes_also_leave_audit() {
    let db = open_db("audit-user");
    let ws = ws_row(&db, "ws-audit", "audit ws");
    let item = user_item(&db, &ws.id, "user_edit", "t", "c");

    db.apply_status_change(&item.id, "resolved", "user", "用户标记完成", None, &[])
        .unwrap();
    let after = db.get_item(&item.id).unwrap().unwrap();
    assert_eq!(after.status, "resolved");

    db.apply_status_change(&item.id, "deleted", "user", "用户删除", None, &[])
        .unwrap();
    let after = db.get_item(&item.id).unwrap().unwrap();
    assert_eq!(after.status, "deleted");

    let history = db.item_history(&item.id).unwrap();
    let audits: Vec<_> = history
        .iter()
        .filter(|r| r.source_type.as_deref() == Some("status_change"))
        .collect();
    assert_eq!(audits.len(), 2, "both transitions audited");
    assert_eq!(audits[0].metadata["audit"]["previous_status"], "active");
    assert_eq!(audits[0].metadata["audit"]["new_status"], "resolved");
    assert_eq!(audits[1].metadata["audit"]["previous_status"], "resolved");
    assert_eq!(audits[1].metadata["audit"]["new_status"], "deleted");
}

// ---------------------------------------------------------------------------
// Head integrity: an applied Update must persist its revision BEFORE the
// item head points at it. A dangling current_revision_id breaks the
// workstream page (revision JOIN yields NULL).
// ---------------------------------------------------------------------------

fn head_is_resolvable(db: &Db, ws_id: &str, item_id: &str) {
    let items = db.items_for_workstream(ws_id, true).unwrap();
    let (_, rev) = items
        .iter()
        .find(|(i, _)| i.id == item_id)
        .expect("item present in workstream listing")
        .clone();
    assert_eq!(
        rev.id,
        db.get_item(item_id)
            .unwrap()
            .unwrap()
            .current_revision_id
            .unwrap(),
        "listed head revision must be the stored head"
    );
}

#[test]
fn update_mutation_persists_revision_before_head_points_at_it() {
    let db = open_db("head-update");
    let (_s, _member, _msgs) = session_with_messages(&db, "head-update-root", &["seed"]);
    let ws = ws_row(&db, "ws-head", "head integrity");
    let ctx = MergeContext {
        run_id: new_id(),
        runtime: "heuristic".into(),
        workstream_id: ws.id.clone(),
    };

    let add = ContextMutation::Add {
        workstream_id: ws.id.clone(),
        item_kind: "current_state".into(),
        title: "initial state".into(),
        content: "v1".into(),
        source_refs: vec!["session-message:a".into()],
        authority: "agent_inferred".into(),
    };
    MergeEngine.apply(&db.write(), &add, &ctx).unwrap();

    let items = db.items_for_workstream(&ws.id, true).unwrap();
    let item_id = items[0].0.id.clone();
    assert_eq!(db.item_history(&item_id).unwrap().len(), 1);

    let update = ContextMutation::Update {
        item_id: item_id.clone(),
        title: "updated state".into(),
        content: "v2".into(),
        source_refs: vec!["session-message:b".into()],
        authority: "agent_inferred".into(),
    };
    assert!(MergeEngine.apply(&db.write(), &update, &ctx).unwrap());

    // The new revision must exist AND be the stored head.
    let history = db.item_history(&item_id).unwrap();
    assert_eq!(history.len(), 2, "update must append a revision");
    assert_eq!(history[1].title, "updated state");
    assert_eq!(
        db.get_item(&item_id)
            .unwrap()
            .unwrap()
            .current_revision_id
            .unwrap(),
        history[1].id,
        "head must point at a persisted revision"
    );
    head_is_resolvable(&db, &ws.id, &item_id);
}

#[test]
fn dedup_update_path_also_persists_revision() {
    let db = open_db("head-dedup");
    let (_s, _member, _msgs) = session_with_messages(&db, "head-dedup-root", &["seed"]);
    let ws = ws_row(&db, "ws-head-dedup", "head integrity dedup");
    let ctx = MergeContext {
        run_id: new_id(),
        runtime: "heuristic".into(),
        workstream_id: ws.id.clone(),
    };

    let add = ContextMutation::Add {
        workstream_id: ws.id.clone(),
        item_kind: "issue".into(),
        title: "页面假死".into(),
        content: "v1".into(),
        source_refs: vec![],
        authority: "agent_inferred".into(),
    };
    MergeEngine.apply(&db.write(), &add, &ctx).unwrap();
    let item_id = db.items_for_workstream(&ws.id, true).unwrap()[0]
        .0
        .id
        .clone();

    // Same kind + same normalized title + different content → the dedup
    // branch updates the existing item instead of adding a new one.
    let add_again = ContextMutation::Add {
        workstream_id: ws.id.clone(),
        item_kind: "issue".into(),
        title: "页面 假死!".into(),
        content: "v2 with new facts".into(),
        source_refs: vec!["session-message:c".into()],
        authority: "agent_inferred".into(),
    };
    assert!(MergeEngine.apply(&db.write(), &add_again, &ctx).unwrap());

    assert_eq!(
        db.items_for_workstream(&ws.id, true).unwrap().len(),
        1,
        "deduped, not duplicated"
    );
    let history = db.item_history(&item_id).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(
        db.get_item(&item_id)
            .unwrap()
            .unwrap()
            .current_revision_id
            .unwrap(),
        history[1].id
    );
    head_is_resolvable(&db, &ws.id, &item_id);
}

// ---------------------------------------------------------------------------
// Concurrency: commit must re-verify trash, the Context frontier and the
// Owner inside its transaction (extraction runs without the lock and may
// take minutes).
// ---------------------------------------------------------------------------

#[test]
fn stale_commit_is_discarded_when_context_frontier_moved() {
    let db = open_db("stale-commit");
    let (s, _member, stored) = session_with_messages(
        &db,
        "stale-root",
        &[
            "决定使用 PostgreSQL 作为主数据库，不再使用 SQLite 存储业务数据",
            "补充约束：不能把密钥提交到代码仓库，必须使用环境变量管理",
        ],
    );
    let ws = ws_row(&db, "ws-stale", "stale ws");
    db.set_session_owner(&s.id, Some(&ws.id)).unwrap();

    let engine = noending::sync::SyncEngine::default();
    let pre = engine
        .prepare(&db, &s, &stored, 0, 2)
        .unwrap()
        .expect("prepared");

    // a concurrent run commits the same delta while "our" extraction runs
    db.set_processed_message_sequence(&s.id, 2).unwrap();

    let out = engine
        .commit(&db, &s, &pre, vec![], "heuristic", vec![])
        .unwrap();
    assert_eq!(
        out.status, "stale",
        "stale run must be discarded, not applied"
    );
    assert_eq!(out.applied, 0);
    assert_eq!(
        db.get_context_state(&s.id)
            .unwrap()
            .processed_message_sequence,
        2,
        "the Context frontier must never move backwards"
    );
    assert_eq!(
        db.items_for_workstream(&ws.id, true).unwrap().len(),
        0,
        "no stale mutation may touch the workstream"
    );
}

/// The Owner Workstream is the Context routing decision, so it is also the
/// commit-phase CAS (方案 §20). If the user re-routes the Session while
/// extraction runs (minutes, no lock held), the prepared mutations target a
/// routing that no longer exists — commit must discard the run WITHOUT
/// advancing the frontier, so the next sync prepares against the user's new
/// decision.
#[test]
fn stale_commit_when_owner_changes_during_extraction() {
    let db = open_db("stale-owner");
    let (s, _member, stored) = session_with_messages(
        &db,
        "stale-owner-root",
        &["我们决定量化系统的回测引擎采用向量化计算，行情数据全部走内存缓存以提升速度"],
    );
    let ws_a = ws_row(&db, "ws-a", "量化回测引擎");
    let ws_b = ws_row(&db, "ws-b", "前端界面重构");
    db.set_session_owner(&s.id, Some(&ws_a.id)).unwrap();

    let engine = noending::sync::SyncEngine::default();
    let pre = engine
        .prepare(&db, &s, &stored, 0, stored.last().unwrap().sequence)
        .unwrap()
        .expect("prepared");
    assert_eq!(
        pre.owner_workstream_id.as_deref(),
        Some(ws_a.id.as_str()),
        "routed to the Owner that existed at prepare time"
    );

    // the user re-routes the Session to B while "extraction" is running
    db.set_session_owner(&s.id, Some(&ws_b.id)).unwrap();

    let out = engine
        .commit(&db, &s, &pre, vec![], "heuristic", vec![])
        .unwrap();
    assert_eq!(
        out.status, "stale",
        "the user's mid-extract decision must win"
    );
    assert_eq!(out.applied, 0);
    assert_eq!(
        db.get_context_state(&s.id)
            .unwrap()
            .processed_message_sequence,
        0,
        "the frontier must not advance for a stale run"
    );
    assert!(
        db.items_for_workstream(&ws_a.id, true).unwrap().is_empty(),
        "a stale run must not write Context into the old Owner"
    );
    assert!(
        db.items_for_workstream(&ws_b.id, true).unwrap().is_empty(),
        "nor into the new one before it is re-prepared"
    );
    assert_eq!(
        db.get_session(&s.id)
            .unwrap()
            .unwrap()
            .owner_workstream_id
            .as_deref(),
        Some(ws_b.id.as_str())
    );
}

/// Clearing the Owner during extraction is a routing change like any other:
/// prepare saw Owner A, the user cleared it, so the run must be discarded
/// rather than committing context into a Workstream the Session no longer
/// belongs to (方案 §20/§21).
#[test]
fn stale_commit_when_owner_cleared_during_extraction() {
    let db = open_db("stale-owner-cleared");
    let (s, _member, _stored) = session_with_messages(&db, "stale-cleared-root", &["seed"]);
    let ws_a = ws_row(&db, "ws-cleared", "前端界面重构");
    db.set_session_owner(&s.id, Some(&ws_a.id)).unwrap();

    let engine = noending::sync::SyncEngine::default();
    let pre = engine
        .prepare(&db, &s, &[], 0, 0)
        .unwrap()
        .expect("prepared");
    assert_eq!(pre.owner_workstream_id.as_deref(), Some(ws_a.id.as_str()));

    db.set_session_owner(&s.id, None).unwrap();

    let out = engine
        .commit(&db, &s, &pre, vec![], "heuristic", vec![])
        .unwrap();
    assert_eq!(out.status, "stale");
    assert_eq!(out.applied, 0);
    assert_eq!(
        db.get_context_state(&s.id)
            .unwrap()
            .processed_message_sequence,
        0
    );
    assert!(db.items_for_workstream(&ws_a.id, true).unwrap().is_empty());
    assert!(
        db.get_session(&s.id)
            .unwrap()
            .unwrap()
            .owner_workstream_id
            .is_none(),
        "the user's clearing of the Owner is preserved"
    );
}

/// §43 / §21 — a run prepared against a Session that was trashed while its
/// extraction ran must not write message-derived context, a SyncRun, or a
/// frontier advance. The data stays frozen at the moment of trashing; a
/// Restore re-syncs from the unchanged frontier.
#[test]
fn trashed_session_rejects_commit_and_keeps_frontier_frozen() {
    let db = open_db("trash-reject");
    let (s, _member, stored) = session_with_messages(
        &db,
        "trash-reject-root",
        &["我们决定使用 PostgreSQL 作为主数据库，不再使用 SQLite 存储业务数据"],
    );
    let ws = ws_row(&db, "ws-trash", "trashed routing");
    db.set_session_owner(&s.id, Some(&ws.id)).unwrap();

    let engine = noending::sync::SyncEngine::default();
    let pre = engine
        .prepare(&db, &s, &stored, 0, stored.last().unwrap().sequence)
        .unwrap()
        .expect("prepared");

    // the user trashes the Session while "extraction" is running
    noending::lifecycle::trash_session(&db, &s.id).unwrap();

    let out = engine
        .commit(&db, &s, &pre, vec![], "heuristic", vec![])
        .unwrap();
    assert_eq!(out.status, "trashed", "the run must be discarded");
    assert_eq!(out.applied, 0);
    assert_eq!(
        db.get_context_state(&s.id)
            .unwrap()
            .processed_message_sequence,
        0,
        "the frontier stays frozen at the moment of trashing"
    );
    assert!(db.list_sync_runs(50).unwrap().is_empty(), "no SyncRun row");
    assert!(
        db.items_for_workstream(&ws.id, true).unwrap().is_empty(),
        "no context was written for a trashed session"
    );
    // the messages themselves were never deleted: Restore re-syncs from here
    assert_eq!(db.message_count(&s.id).unwrap(), stored.len() as i64);
}
