//! Sync integrity tests (Issues #3 + #4).
//!
//! - one SyncRun = one transaction: any failure rolls back mutations, the
//!   SyncRun row and the processed cursor together;
//! - read cursor (ingested) and processed cursor (synced) are separate;
//! - completed runs are idempotent under retry (delta fingerprint);
//! - the unified AuthorityPolicy: agents never silently overwrite
//!   user_explicit / user_edit items — they create ContextConflicts;
//! - every status change leaves an audit revision.

use noending::domain::{binding_source, Agent, Session, SessionWorkstreamBinding};
use noending::storage::{
    insert_sync_run_conn, new_id, now, set_processed_sequence_conn, Db,
};
use noending::sync::merge::MergeEngine;
use noending::sync::{create_item, ContextMutation, MergeContext};

fn open_db(tag: &str) -> Db {
    let dir = std::env::temp_dir().join(format!("noending-integrity-{}-{}", tag, new_id()));
    Db::open(&dir.join("test.db")).unwrap()
}

fn session_row(db: &Db) -> Session {
    let s = Session {
        id: new_id(),
        agent: Agent::Codex,
        agent_session_id: format!("integrity-{}", new_id()),
        title: None,
        cwd: None,
        project_id: None,
        raw_path: "/tmp/x.jsonl".into(),
        parent_agent_session_id: None,
        started_at: Some(now()),
        last_activity_at: Some(now()),
    };
    db.upsert_session(&s).unwrap();
    s
}

fn ws_row(db: &Db, title: &str) -> noending::domain::Workstream {
    let w = noending::domain::Workstream {
        id: new_id(),
        project_id: None,
        title: title.into(),
        description: String::new(),
        lifecycle: "open".into(),
        visibility: "normal".into(),
        created_at: now(),
        updated_at: now(),
    };
    db.upsert_workstream(&w).unwrap();
    w
}

fn user_item(db: &Db, ws_id: &str, authority: &str, title: &str, content: &str) -> noending::domain::ContextItem {
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

/// A DB failure in the middle of the mutation batch must roll back the
/// whole run: earlier mutations, the SyncRun row and the processed cursor
/// all disappear. (Foreign-keys are ON, so an Add pointing at a missing
/// workstream reliably fails at the DB level.)
#[test]
fn mutation_failure_rolls_back_entire_run() {
    let db = open_db("rollback");
    let s = session_row(&db);
    let ws = ws_row(&db, "real workstream");

    let mutations = vec![
        ContextMutation::Add {
            workstream_id: ws.id.clone(),
            item_kind: "note".into(),
            title: "first mutation".into(),
            content: "ok".into(),
            source_refs: vec![],
            authority: "agent_inferred".into(),
        },
        ContextMutation::Add {
            workstream_id: "ws-does-not-exist".into(), // FK violation
            item_kind: "note".into(),
            title: "second mutation".into(),
            content: "boom".into(),
            source_refs: vec![],
            authority: "agent_inferred".into(),
        },
    ];

    let ctx = MergeContext { run_id: new_id(), runtime: "heuristic".into() };
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
        source_generation: 0,
    };

    let result = db.tx(|tx| {
        for m in &mutations {
            MergeEngine.apply(tx, m, &ctx)?;
        }
        insert_sync_run_conn(tx, &run)?;
        set_processed_sequence_conn(tx, &s.id, 5)?;
        Ok(())
    });
    assert!(result.is_err(), "the FK violation must fail the transaction");

    // everything rolled back
    assert!(
        db.items_for_workstream(&ws.id, true).unwrap().is_empty(),
        "the first (valid) mutation must NOT survive the rollback"
    );
    assert_eq!(db.list_sync_runs(50).unwrap().len(), 0, "no SyncRun row");
    assert_eq!(db.get_processed_sequence(&s.id).unwrap(), 0, "processed cursor unchanged");
}

/// Retrying after a rollback applies the batch exactly once; retrying an
/// already-committed delta is recognized via the fingerprint.
#[test]
fn retry_after_rollback_applies_once_and_commit_is_idempotent() {
    let db = open_db("retry");
    let _s = session_row(&db);
    let ws = ws_row(&db, "retry ws");

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

    // 1st attempt fails mid-batch (bad second mutation)
    let ctx = MergeContext { run_id: new_id(), runtime: "heuristic".into() };
    let mut mutations = mk_mutations();
    mutations.push(ContextMutation::Add {
        workstream_id: "missing-ws".into(),
        item_kind: "note".into(),
        title: "bad".into(),
        content: "x".into(),
        source_refs: vec![],
        authority: "agent_inferred".into(),
    });
    let failed = db.tx(|tx| {
        for m in &mutations {
            MergeEngine.apply(tx, m, &ctx)?;
        }
        Ok(())
    });
    assert!(failed.is_err());

    // retry with only the good mutation succeeds exactly once
    let ctx2 = MergeContext { run_id: new_id(), runtime: "heuristic".into() };
    db.tx(|tx| {
        for m in mk_mutations() {
            MergeEngine.apply(tx, &m, &ctx2)?;
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(db.items_for_workstream(&ws.id, true).unwrap().len(), 1);

    // applying the same mutation again is a deterministic skip (dedup)
    let ctx3 = MergeContext { run_id: new_id(), runtime: "heuristic".into() };
    let applied_again = db.tx(|tx| {
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

/// read_cursor and processed_cursor are independent: ingested-but-unsynced
/// events exist, survive, and are picked up by the next sync pass.
#[test]
fn read_cursor_and_processed_cursor_are_separate() {
    let db = open_db("cursors");
    let s = session_row(&db);
    let ws = ws_row(&db, "cursor ws");
    db.bind(&SessionWorkstreamBinding {
        session_id: s.id.clone(),
        workstream_id: ws.id.clone(),
        role: "primary".into(),
        source: binding_source::USER_ASSIGNED.into(),
        confidence: 1.0,
        last_seen_revision: None,
        last_sync_cursor: 0,
        created_at: now(),
        last_used_at: now(),
    })
    .unwrap();

    let parsed = |i: i64, text: &str| noending::domain::ParsedEvent {
        source_event_id: None,
        source_position: format!("line:{}", i),
        ts: Some(now()),
        kind: "user_message".into(),
        text: Some(text.into()),
        metadata: serde_json::json!({}),
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

    // ingest batch 1 (real user message so the extractor produces mutations)
    let batch1 = vec![parsed(1, "我们决定使用 PostgreSQL 作为主数据库，不再使用 SQLite 存储业务数据")];
    let stored1 = db
        .append_source_events(&s.id, &batch1, &source(50), "/tmp/x.jsonl")
        .unwrap();
    assert_eq!(stored1.len(), 1);
    assert_eq!(db.get_source_cursor(&s.id).unwrap().last_sequence, 1, "read cursor advanced");
    assert_eq!(db.get_processed_sequence(&s.id).unwrap(), 0, "processed cursor behind");

    // ingest batch 2 — still not synced (simulates a crash after ingest)
    let batch2 = vec![parsed(2, "补充约束：不能把密钥提交到代码仓库，必须使用环境变量")];
    let stored2 = db
        .append_source_events(&s.id, &batch2, &source(120), "/tmp/x.jsonl")
        .unwrap();
    assert_eq!(stored2.len(), 1);
    assert_eq!(db.get_processed_sequence(&s.id).unwrap(), 0);

    // now sync everything after the processed cursor in one run
    let pending = db.get_events(&s.id, Some(0), 10_000).unwrap();
    assert_eq!(pending.len(), 2, "both batches are pending");
    let engine = noending::sync::SyncEngine::default();
    let out = engine
        .run_session_sync(&db, &s, &pending, 0, pending.last().unwrap().sequence)
        .unwrap();
    assert!(out.applied > 0);
    assert_eq!(db.get_processed_sequence(&s.id).unwrap(), 2, "processed caught up");
    assert_eq!(
        db.items_for_workstream(&ws.id, true).unwrap().len(),
        out.applied as usize,
        "exactly the applied mutations persisted"
    );
}

/// The background sync entry processes pending events through the
/// phase-split (lock / no-lock / lock) path and advances the processed
/// cursor exactly like the blocking path.
#[test]
fn nonblocking_sync_path_processes_pending_events() {
    let db = open_db("nb-path");
    let s = session_row(&db);
    let ws = ws_row(&db, "nb ws");
    db.bind(&SessionWorkstreamBinding {
        session_id: s.id.clone(),
        workstream_id: ws.id.clone(),
        role: "primary".into(),
        source: binding_source::USER_ASSIGNED.into(),
        confidence: 1.0,
        last_seen_revision: None,
        last_sync_cursor: 0,
        created_at: now(),
        last_used_at: now(),
    })
    .unwrap();

    // one pending event (a real user message so extraction yields mutations)
    let source = noending::domain::SourceCursorUpdate {
        file_identity: "dev:1:ino:77".into(),
        generation: 0,
        byte_offset: 88,
        last_seen_size: 88,
        mtime: None,
        start_byte_offset: 88,
        prefix_hash: String::new(),
    };
    let stored = db
        .append_source_events(
            &s.id,
            &[noending::domain::ParsedEvent {
                source_event_id: None,
                source_position: "line:1".into(),
                ts: Some(now()),
                kind: "user_message".into(),
                text: Some("我们决定使用 PostgreSQL 作为主数据库，不再使用 SQLite 存储业务数据".into()),
                metadata: serde_json::json!({}),
            }],
            &source,
            "/tmp/x.jsonl",
        )
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(db.get_processed_sequence(&s.id).unwrap(), 0);

    // hand the DB to the shared Mutex<Db> exactly like the app does
    let db_lock = std::sync::Mutex::new(db);
    let engine = noending::sync::SyncEngine::default();
    let applied = engine.run_pending_sync_nonblocking(&db_lock, &s).unwrap();
    assert!(applied > 0, "pending events processed through the nb path");

    let guard = db_lock.lock().unwrap();
    assert_eq!(guard.get_processed_sequence(&s.id).unwrap(), 1, "processed cursor advanced");
    assert!(
        !guard.items_for_workstream(&ws.id, true).unwrap().is_empty(),
        "mutations were committed"
    );
}

// ---------------------------------------------------------------------------
// Issue #4: authority policy integration
// ---------------------------------------------------------------------------

#[test]
fn agent_update_of_user_item_creates_conflict_not_overwrite() {
    let db = open_db("auth-update");
    let ws = ws_row(&db, "auth ws");
    let created = user_item(&db, &ws.id, "user_edit", "保持 macOS 一等支持", "Windows 和 macOS 都是一等公民");
    let item = db.get_item(&created.id).unwrap().unwrap();
    let original_head = item.current_revision_id.clone();

    let m = ContextMutation::Update {
        item_id: item.id.clone(),
        title: "只需要 macOS".into(),
        content: "This project only needs macOS".into(),
        source_refs: vec!["session-event:e1".into()],
        authority: "agent_inferred".into(),
    };
    let ctx = MergeContext { run_id: new_id(), runtime: "heuristic".into() };
    let applied = db.tx(|tx| MergeEngine.apply(tx, &m, &ctx)).unwrap();
    assert!(applied, "the disagreement is recorded (as a conflict)");

    let after = db.get_item(&item.id).unwrap().unwrap();
    assert_eq!(after.status, "active", "user item untouched");
    assert_eq!(after.current_revision_id, original_head, "content not replaced");

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
    let ws = ws_row(&db, "supersede ws");
    let created = user_item(&db, &ws.id, "user_explicit", "数据库继续用 SQLite", "用户明确决定");
    let item = db.get_item(&created.id).unwrap().unwrap();
    let original_head = item.current_revision_id.clone();

    let ctx = MergeContext { run_id: new_id(), runtime: "heuristic".into() };

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
    assert_eq!(after.status, "active", "user_explicit not resolved by agent");

    let conflicts = db.conflicts_for_workstream(&ws.id, false).unwrap();
    assert_eq!(conflicts.len(), 2, "both attempts materialized as conflicts");
}

#[test]
fn agent_may_evolve_agent_owned_items_with_full_trail() {
    let db = open_db("auth-agent-owned");
    let ws = ws_row(&db, "agent owned ws");
    // agent-owned item
    let item = create_item(
        &db,
        &ws.id,
        "note",
        "初步猜测",
        "guess v1",
        "agent_inferred",
        "session_event",
        &["session-event:e-old".into()],
        None,
        "sync:heuristic",
    )
    .unwrap();

    let ctx = MergeContext { run_id: new_id(), runtime: "heuristic".into() };
    let update = ContextMutation::Update {
        item_id: item.id.clone(),
        title: "初步猜测（修订）".into(),
        content: "guess v2".into(),
        source_refs: vec!["session-event:e-new".into()],
        authority: "agent_inferred".into(),
    };
    assert!(db.tx(|tx| MergeEngine.apply(tx, &update, &ctx)).unwrap());
    let after = db.get_item(&item.id).unwrap().unwrap();
    assert_ne!(after.current_revision_id, item.current_revision_id, "revision advanced");

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
    let ws = ws_row(&db, "audit ws");
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

/// Automatic classification may add candidate bindings, but never touches
/// explicit ones — and explicit sources always win the upsert race.
#[test]
fn explicit_bindings_cannot_be_downgraded() {
    let db = open_db("binding-source");
    let ws = ws_row(&db, "binding ws");
    let s = session_row(&db);

    let b = SessionWorkstreamBinding {
        session_id: s.id.clone(),
        workstream_id: ws.id.clone(),
        role: "primary".into(),
        source: binding_source::EXPLICIT_LAUNCH.into(),
        confidence: 1.0,
        last_seen_revision: None,
        last_sync_cursor: 0,
        created_at: now(),
        last_used_at: now(),
    };
    db.bind(&b).unwrap();

    // automatic classification tries to take over — ignored
    let auto = SessionWorkstreamBinding {
        source: binding_source::AUTO.into(),
        confidence: 0.42,
        ..b.clone()
    };
    db.bind(&auto).unwrap();
    let bound = db.bindings_for_session(&s.id).unwrap();
    assert_eq!(bound[0].source, binding_source::EXPLICIT_LAUNCH);
    assert_eq!(bound[0].confidence, 1.0);

    // an explicit source can upgrade an automatic one
    let s2 = session_row(&db);
    db.bind(&SessionWorkstreamBinding {
        session_id: s2.id.clone(),
        workstream_id: ws.id.clone(),
        role: "related".into(),
        source: binding_source::AUTO.into(),
        confidence: 0.5,
        last_seen_revision: None,
        last_sync_cursor: 0,
        created_at: now(),
        last_used_at: now(),
    })
    .unwrap();
    db.bind(&SessionWorkstreamBinding {
        session_id: s2.id.clone(),
        workstream_id: ws.id.clone(),
        role: "primary".into(),
        source: binding_source::USER_ASSIGNED.into(),
        confidence: 1.0,
        last_seen_revision: None,
        last_sync_cursor: 0,
        created_at: now(),
        last_used_at: now(),
    })
    .unwrap();
    let bound = db.bindings_for_session(&s2.id).unwrap();
    assert_eq!(bound[0].source, binding_source::USER_ASSIGNED);

    // derived classification state
    use noending::domain::SessionClassificationState;
    assert_eq!(SessionClassificationState::derive(&[]), SessionClassificationState::Unassigned);
    assert_eq!(
        SessionClassificationState::derive(&bound),
        SessionClassificationState::Assigned
    );
    let partial = vec![SessionWorkstreamBinding { source: binding_source::AUTO.into(), ..bound[0].clone() }];
    assert_eq!(
        SessionClassificationState::derive(&partial),
        SessionClassificationState::PartiallyAssigned
    );
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
        db.get_item(item_id).unwrap().unwrap().current_revision_id.unwrap(),
        "listed head revision must be the stored head"
    );
}

#[test]
fn update_mutation_persists_revision_before_head_points_at_it() {
    let db = open_db("head-update");
    let _s = session_row(&db);
    let ws = ws_row(&db, "head integrity");
    let ctx = MergeContext { run_id: new_id(), runtime: "heuristic".into() };

    let add = ContextMutation::Add {
        workstream_id: ws.id.clone(),
        item_kind: "current_state".into(),
        title: "initial state".into(),
        content: "v1".into(),
        source_refs: vec!["session-event:a".into()],
        authority: "agent_inferred".into(),
    };
    MergeEngine.apply(&db.0, &add, &ctx).unwrap();

    let items = db.items_for_workstream(&ws.id, true).unwrap();
    let item_id = items[0].0.id.clone();
    assert_eq!(db.item_history(&item_id).unwrap().len(), 1);

    let update = ContextMutation::Update {
        item_id: item_id.clone(),
        title: "updated state".into(),
        content: "v2".into(),
        source_refs: vec!["session-event:b".into()],
        authority: "agent_inferred".into(),
    };
    assert!(MergeEngine.apply(&db.0, &update, &ctx).unwrap());

    // The new revision must exist AND be the stored head.
    let history = db.item_history(&item_id).unwrap();
    assert_eq!(history.len(), 2, "update must append a revision");
    assert_eq!(history[1].title, "updated state");
    assert_eq!(
        db.get_item(&item_id).unwrap().unwrap().current_revision_id.unwrap(),
        history[1].id,
        "head must point at a persisted revision"
    );
    head_is_resolvable(&db, &ws.id, &item_id);
}

#[test]
fn dedup_update_path_also_persists_revision() {
    let db = open_db("head-dedup");
    let _s = session_row(&db);
    let ws = ws_row(&db, "head integrity dedup");
    let ctx = MergeContext { run_id: new_id(), runtime: "heuristic".into() };

    let add = ContextMutation::Add {
        workstream_id: ws.id.clone(),
        item_kind: "issue".into(),
        title: "页面假死".into(),
        content: "v1".into(),
        source_refs: vec![],
        authority: "agent_inferred".into(),
    };
    MergeEngine.apply(&db.0, &add, &ctx).unwrap();
    let item_id = db.items_for_workstream(&ws.id, true).unwrap()[0].0.id.clone();

    // Same kind + same normalized title + different content → the dedup
    // branch updates the existing item instead of adding a new one.
    let add_again = ContextMutation::Add {
        workstream_id: ws.id.clone(),
        item_kind: "issue".into(),
        title: "页面 假死!".into(),
        content: "v2 with new facts".into(),
        source_refs: vec!["session-event:c".into()],
        authority: "agent_inferred".into(),
    };
    assert!(MergeEngine.apply(&db.0, &add_again, &ctx).unwrap());

    assert_eq!(db.items_for_workstream(&ws.id, true).unwrap().len(), 1, "deduped, not duplicated");
    let history = db.item_history(&item_id).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(db.get_item(&item_id).unwrap().unwrap().current_revision_id.unwrap(), history[1].id);
    head_is_resolvable(&db, &ws.id, &item_id);
}

// ---------------------------------------------------------------------------
// Concurrency: commit must re-verify the processed cursor inside its
// transaction (extraction runs without the lock and may take minutes).
// ---------------------------------------------------------------------------

#[test]
fn stale_commit_is_discarded_when_processed_cursor_moved() {
    let db = open_db("stale-commit");
    let s = session_row(&db);
    let ws = ws_row(&db, "stale ws");
    db.bind(&SessionWorkstreamBinding {
        session_id: s.id.clone(),
        workstream_id: ws.id.clone(),
        role: "primary".into(),
        source: binding_source::USER_ASSIGNED.into(),
        confidence: 1.0,
        last_seen_revision: None,
        last_sync_cursor: 0,
        created_at: now(),
        last_used_at: now(),
    })
    .unwrap();

    let mk = |seq: i64, text: &str| noending::domain::SessionEvent {
        id: new_id(),
        session_id: s.id.clone(),
        sequence: seq,
        source_event_id: None,
        source_generation: 0,
        source_position: format!("line:{}", seq),
        ts: Some(now()),
        kind: "user_message".into(),
        text: Some(text.into()),
        raw_ref: format!("/tmp/x.jsonl#line:{}", seq),
        metadata: serde_json::json!({}),
    };
    let events = vec![
        mk(1, "决定使用 PostgreSQL 作为主数据库，不再使用 SQLite 存储业务数据"),
        mk(2, "补充约束：不能把密钥提交到代码仓库，必须使用环境变量管理"),
    ];
    db.append_events(&events).unwrap();

    let engine = noending::sync::SyncEngine::default();
    let pre = engine
        .prepare(&db, &s, &events, 0, 2)
        .unwrap()
        .expect("prepared");

    // a concurrent run commits the same delta while "our" extraction runs
    db.set_processed_sequence(&s.id, 2).unwrap();

    let out = engine
        .commit(&db, &s, &pre, vec![], "heuristic", vec![])
        .unwrap();
    assert_eq!(out.status, "stale", "stale run must be discarded, not applied");
    assert_eq!(out.applied, 0);
    assert_eq!(
        db.get_processed_sequence(&s.id).unwrap(),
        2,
        "processed cursor must never move backwards"
    );
    assert_eq!(
        db.items_for_workstream(&ws.id, true).unwrap().len(),
        0,
        "no stale mutation may touch the workstream"
    );
}

// ---------------------------------------------------------------------------
// Auto-classification becomes a real binding inside the commit transaction.
// ---------------------------------------------------------------------------

#[test]
fn auto_classification_persists_as_binding_on_commit() {
    let db = open_db("auto-bind");
    let s = session_row(&db);
    // no bindings at all: sync must classify by keyword
    let ws = ws_row(&db, "量化系统架构");

    let event = noending::domain::SessionEvent {
        id: new_id(),
        session_id: s.id.clone(),
        sequence: 1,
        source_event_id: None,
        source_generation: 0,
        source_position: "line:1".into(),
        ts: Some(now()),
        kind: "user_message".into(),
        text: Some("我们决定量化系统的回测引擎采用向量化计算，行情数据全部走内存缓存以提升速度".into()),
        raw_ref: "/tmp/x.jsonl#line:1".into(),
        metadata: serde_json::json!({}),
    };
    db.append_events(std::slice::from_ref(&event)).unwrap();

    let engine = noending::sync::SyncEngine::default();
    let out = engine
        .run_session_sync(&db, &s, std::slice::from_ref(&event), 0, 1)
        .unwrap();
    assert_eq!(out.status, "ok");

    let bound = db.bindings_for_session(&s.id).unwrap();
    assert!(
        bound.iter().any(|b| b.workstream_id == ws.id && b.source == binding_source::AUTO),
        "auto classification must persist as a binding, got {:?}",
        bound
    );
    use noending::domain::SessionClassificationState;
    assert_eq!(
        SessionClassificationState::derive(&bound),
        SessionClassificationState::PartiallyAssigned,
        "auto-only bindings read back as partially assigned"
    );
}

// ---------------------------------------------------------------------------
// Binding source precedence: equal-confidence later flows must not rewrite
// stronger provenance, and role changes only when the binding is replaced.
// ---------------------------------------------------------------------------

#[test]
fn binding_source_precedence_beats_equal_confidence() {
    let db = open_db("bind-precedence");
    let s = session_row(&db);
    let ws = ws_row(&db, "precedence ws");
    let mk = |source: &str, role: &str| SessionWorkstreamBinding {
        session_id: s.id.clone(),
        workstream_id: ws.id.clone(),
        role: role.into(),
        source: source.into(),
        confidence: 1.0,
        last_seen_revision: None,
        last_sync_cursor: 0,
        created_at: now(),
        last_used_at: now(),
    };

    db.bind(&mk(binding_source::EXPLICIT_LAUNCH, "primary")).unwrap();
    // a later user_assigned resume at the SAME confidence must not rewrite
    // the explicit provenance (nor steal the primary role)
    db.bind(&mk(binding_source::USER_ASSIGNED, "related")).unwrap();
    let b = &db.bindings_for_session(&s.id).unwrap()[0];
    assert_eq!(b.source, binding_source::EXPLICIT_LAUNCH);
    assert_eq!(b.role, "primary");

    // automatic classification never downgrades anything explicit/user
    db.bind(&mk(binding_source::AUTO, "related")).unwrap();
    let b = &db.bindings_for_session(&s.id).unwrap()[0];
    assert_eq!(b.source, binding_source::EXPLICIT_LAUNCH);

    // equal-rank replacement still works (user_assigned over user_assigned)
    db.bind(&mk(binding_source::USER_ASSIGNED, "related")).unwrap();
    let fresh_ws = ws_row(&db, "precedence ws 2");
    db.bind(&SessionWorkstreamBinding {
        workstream_id: fresh_ws.id.clone(),
        ..mk(binding_source::USER_ASSIGNED, "related")
    })
    .unwrap();
    let b2 = db
        .bindings_for_session(&s.id)
        .unwrap()
        .into_iter()
        .find(|b| b.workstream_id == fresh_ws.id)
        .unwrap();
    assert_eq!(b2.source, binding_source::USER_ASSIGNED);
}
