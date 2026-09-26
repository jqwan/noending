//! Context integrity tests — the explicit-update half of the pipeline.
//!
//! Every Workstream mutation goes through `MergeEngine::apply` inside ONE
//! caller-owned transaction: any failure rolls the whole update back (mutations,
//! conflict rows and head revisions together); retrying a batch applies it
//! exactly once, and re-applying an identical mutation is a deterministic skip.
//! Facts and Context are separate lifecycles, so an update never moves the fact
//! frontier. Agents never silently overwrite user items (they persist
//! ContextConflicts), every status change leaves an audit revision, and a
//! mutation naming a Workstream outside the run's Owner is skipped, never written.

use noending::domain::{Agent, ParsedSessionMessage, Session, SessionMessage, SessionMessageRole};
use noending::storage::{new_id, Db};
use noending::sync::merge::MergeEngine;
use noending::sync::{create_item, ContextMutation, MergeContext};

mod support;

fn open_db(tag: &str) -> Db {
    let dir = std::env::temp_dir().join(format!("noending-integrity-{}-{}", tag, new_id()));
    Db::open(&dir.join("test.db")).unwrap()
}

/// The run context for an explicit update of `ws`. `runtime` is recorded as
/// `created_by = sync:<runtime>` on everything the merge engine writes.
fn ctx(ws: &str) -> MergeContext {
    MergeContext {
        runtime: "heuristic".into(),
        workstream_id: ws.to_string(),
    }
}

/// A Logical Session + ROOT member seeded through the production commit path,
/// with one stored user message per text.
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
        "user",
    )
    .unwrap()
}

fn agent_item(db: &Db, ws_id: &str, title: &str, content: &str) -> noending::domain::ContextItem {
    create_item(
        db,
        ws_id,
        "note",
        title,
        content,
        "agent_inferred",
        "session_message",
        &[],
        "sync:heuristic",
    )
    .unwrap()
}

fn add_note(ws_id: &str, title: &str, content: &str) -> ContextMutation {
    ContextMutation::Add {
        workstream_id: ws_id.to_string(),
        item_kind: "note".into(),
        title: title.into(),
        content: content.into(),
        source_refs: vec![],
        authority: "agent_inferred".into(),
    }
}

// Transactions

/// A DB failure anywhere in the update's transaction must roll back the whole
/// batch: earlier mutations and their conflict rows disappear together.
#[test]
fn mutation_failure_rolls_back_entire_run() {
    let db = open_db("rollback");
    let ws = ws_row(&db, "ws-rollback", "real workstream");
    let c = ctx(&ws.id);
    let m = add_note(&ws.id, "first mutation", "ok");

    let result = db.tx(|tx| {
        MergeEngine.apply(tx, &m, &c)?;
        // FK violation: there is no such Session to write Context for.
        tx.execute(
            "INSERT INTO session_contexts (session_id, updated_at) VALUES ('missing-session', 't')",
            [],
        )?;
        Ok(())
    });
    assert!(
        result.is_err(),
        "the FK violation must fail the transaction"
    );

    assert!(
        db.items_for_workstream(&ws.id, true).unwrap().is_empty(),
        "the applied mutation must NOT survive the rollback"
    );
    assert_eq!(
        db.conflicts_for_workstream(&ws.id, false).unwrap().len(),
        0,
        "no conflict row survives either"
    );
}

/// Retrying after a rollback applies the batch exactly once; re-applying an
/// identical mutation is a deterministic skip (dedup).
#[test]
fn retry_after_rollback_applies_once_and_reapply_is_deduped() {
    let db = open_db("retry");
    let ws = ws_row(&db, "ws-retry", "retry ws");
    let m = add_note(&ws.id, "only good mutation", "fine");

    // 1st attempt fails after the mutation was applied
    let c = ctx(&ws.id);
    let failed = db.tx(|tx| {
        MergeEngine.apply(tx, &m, &c)?;
        tx.execute(
            "INSERT INTO session_contexts (session_id, updated_at) VALUES ('missing-session', 't')",
            [],
        )?;
        Ok(())
    });
    assert!(failed.is_err());
    assert!(
        db.items_for_workstream(&ws.id, true).unwrap().is_empty(),
        "the failed attempt left nothing behind"
    );

    // retry succeeds exactly once
    let c2 = ctx(&ws.id);
    db.tx(|tx| {
        MergeEngine.apply(tx, &m, &c2)?;
        Ok(())
    })
    .unwrap();
    assert_eq!(db.items_for_workstream(&ws.id, true).unwrap().len(), 1);

    // applying the same mutation again is a deterministic skip (dedup)
    let c3 = ctx(&ws.id);
    let applied_again = db
        .tx(|tx| {
            let applied = MergeEngine.apply(tx, &m, &c3)?;
            Ok(applied as i32)
        })
        .unwrap();
    assert_eq!(applied_again, 0, "identical re-apply is a no-op");
    assert_eq!(db.items_for_workstream(&ws.id, true).unwrap().len(), 1);
}

/// Facts and Context are independent lifecycles: ingestion advances the fact
/// frontier, an explicit update writes Context — neither touches the other.
#[test]
fn facts_and_context_are_separate_lifecycles() {
    let db = open_db("cursors");
    let (s, member_id, _stored) = session_with_messages(
        &db,
        "cursors-root",
        &["我们决定使用 PostgreSQL 作为主数据库，不再使用 SQLite 存储业务数据"],
    );
    let ws = ws_row(&db, "ws-cursors", "cursor ws");
    // Routing is the Session's single Owner Workstream.
    db.set_session_owner(&s.id, Some(&ws.id)).unwrap();

    // facts advanced through the production commit path
    assert_eq!(
        db.get_session_ingest_state(&s.id)
            .unwrap()
            .latest_message_seq,
        1
    );
    assert_eq!(db.message_projection_ids(&s.id).unwrap().len(), 1);
    assert!(
        db.get_member_cursor(&member_id).unwrap().byte_offset > 0,
        "member (read) cursor advanced"
    );

    // no Context exists yet — only an explicit update would write it
    assert!(db.get_session_context(&s.id).unwrap().is_none());
    assert!(db.workstream_frontiers(&ws.id).unwrap().is_empty());

    // a Context write never moves the fact frontier
    let c = ctx(&ws.id);
    db.tx(|tx| {
        MergeEngine.apply(tx, &add_note(&ws.id, "from the run", "body"), &c)?;
        Ok(())
    })
    .unwrap();
    assert_eq!(db.items_for_workstream(&ws.id, true).unwrap().len(), 1);
    assert_eq!(
        db.get_session_ingest_state(&s.id)
            .unwrap()
            .latest_message_seq,
        1,
        "the fact frontier is unchanged by a Context write"
    );
    assert!(
        db.get_session_context(&s.id).unwrap().is_none(),
        "a Workstream mutation does not author a Session summary"
    );
}

// Authority policy integration

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
    let c = ctx(&ws.id);
    let applied = db.tx(|tx| MergeEngine.apply(tx, &m, &c)).unwrap();
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

    let c = ctx(&ws.id);

    let sup = ContextMutation::Supersede {
        item_id: item.id.clone(),
        title: "改用 Postgres".into(),
        content: "agent wants postgres".into(),
        source_refs: vec![],
        authority: "agent_inferred".into(),
    };
    assert!(db.tx(|tx| MergeEngine.apply(tx, &sup, &c)).unwrap());
    let after = db.get_item(&item.id).unwrap().unwrap();
    assert_eq!(after.status, "active", "user_explicit not superseded");
    assert_eq!(after.current_revision_id, original_head);

    let res = ContextMutation::Resolve {
        item_id: item.id.clone(),
        source_refs: vec![],
    };
    assert!(db.tx(|tx| MergeEngine.apply(tx, &res, &c)).unwrap());
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
    let item = agent_item(&db, &ws.id, "初步猜测", "guess v1");

    let c = ctx(&ws.id);
    let update = ContextMutation::Update {
        item_id: item.id.clone(),
        title: "初步猜测（修订）".into(),
        content: "guess v2".into(),
        source_refs: vec!["session-message:e-new".into()],
        authority: "agent_inferred".into(),
    };
    assert!(db.tx(|tx| MergeEngine.apply(tx, &update, &c)).unwrap());
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
    assert!(db.tx(|tx| MergeEngine.apply(tx, &sup, &c)).unwrap());
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

    db.apply_status_change(&item.id, "resolved", "user", "用户标记完成", &[])
        .unwrap();
    let after = db.get_item(&item.id).unwrap().unwrap();
    assert_eq!(after.status, "resolved");

    db.apply_status_change(&item.id, "deleted", "user", "用户删除", &[])
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

// Head integrity: an applied Update must persist its revision BEFORE the
// item head points at it. A dangling current_revision_id breaks the
// workstream page (revision JOIN yields NULL).

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
    let ws = ws_row(&db, "ws-head", "head integrity");
    let c = ctx(&ws.id);

    let add = ContextMutation::Add {
        workstream_id: ws.id.clone(),
        item_kind: "current_state".into(),
        title: "initial state".into(),
        content: "v1".into(),
        source_refs: vec!["session-message:a".into()],
        authority: "agent_inferred".into(),
    };
    MergeEngine.apply(&db.write(), &add, &c).unwrap();

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
    assert!(MergeEngine.apply(&db.write(), &update, &c).unwrap());

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
    let ws = ws_row(&db, "ws-head-dedup", "head integrity dedup");
    let c = ctx(&ws.id);

    let add = ContextMutation::Add {
        workstream_id: ws.id.clone(),
        item_kind: "issue".into(),
        title: "页面假死".into(),
        content: "v1".into(),
        source_refs: vec![],
        authority: "agent_inferred".into(),
    };
    MergeEngine.apply(&db.write(), &add, &c).unwrap();
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
    assert!(MergeEngine.apply(&db.write(), &add_again, &c).unwrap());

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

// Owner boundary: `update` / `supersede` / `resolve` name their target by
// `item_id`, and that id may come from model output echoing an id found anywhere
// in the transcript — so a mutation naming a Workstream outside the run's Owner
// must be SKIPPED, never written. The routing decision the user just made wins.

#[test]
fn mutation_outside_the_owner_is_skipped_not_written() {
    let db = open_db("owner-boundary");
    let ws_owner = ws_row(&db, "ws-owner", "the run's owner");
    let ws_other = ws_row(&db, "ws-other", "someone else's workstream");
    let item = agent_item(&db, &ws_other.id, "别的 Workstream 的条目", "body");
    let original_head = item.current_revision_id.clone();

    // The run may only write `ws_owner`.
    let c = ctx(&ws_owner.id);

    let add = add_note(&ws_other.id, "越界新增", "should never land");
    assert!(
        !db.tx(|tx| MergeEngine.apply(tx, &add, &c)).unwrap(),
        "an Add for another Workstream is skipped"
    );

    let update = ContextMutation::Update {
        item_id: item.id.clone(),
        title: "越界改写".into(),
        content: "should never land".into(),
        source_refs: vec![],
        authority: "agent_inferred".into(),
    };
    assert!(
        !db.tx(|tx| MergeEngine.apply(tx, &update, &c)).unwrap(),
        "an Update naming another Workstream's item is skipped"
    );

    let sup = ContextMutation::Supersede {
        item_id: item.id.clone(),
        title: "越界取代".into(),
        content: "should never land".into(),
        source_refs: vec![],
        authority: "agent_inferred".into(),
    };
    assert!(!db.tx(|tx| MergeEngine.apply(tx, &sup, &c)).unwrap());

    let res = ContextMutation::Resolve {
        item_id: item.id.clone(),
        source_refs: vec![],
    };
    assert!(!db.tx(|tx| MergeEngine.apply(tx, &res, &c)).unwrap());

    let after = db.get_item(&item.id).unwrap().unwrap();
    assert_eq!(after.status, "active", "the other item is untouched");
    assert_eq!(after.current_revision_id, original_head);
    assert_eq!(
        db.items_for_workstream(&ws_other.id, true).unwrap().len(),
        1
    );
    assert!(db
        .items_for_workstream(&ws_owner.id, true)
        .unwrap()
        .is_empty());
    assert_eq!(
        db.conflicts_for_workstream(&ws_other.id, false)
            .unwrap()
            .len(),
        0,
        "a skipped mutation writes neither item nor conflict"
    );
}

/// A trashed Session refuses an explicit summary update outright (the guard
/// runs before any model call), and its facts stay frozen.
#[test]
fn trashed_session_refuses_explicit_update() {
    let db = open_db("trash-reject");
    let (s, _member, stored) = session_with_messages(
        &db,
        "trash-reject-root",
        &["我们决定使用 PostgreSQL 作为主数据库，不再使用 SQLite 存储业务数据"],
    );
    let ws = ws_row(&db, "ws-trash", "trashed routing");
    db.set_session_owner(&s.id, Some(&ws.id)).unwrap();

    noending::lifecycle::trash_session(&db, &s.id).unwrap();

    let home_dir =
        std::env::temp_dir().join(format!("noending-integrity-context-home-{}", new_id()));
    let home =
        noending::workspace::home::NoEndingHome::new(home_dir.to_str().unwrap(), None).unwrap();
    let outcome = noending::context::update_session(&db, &s.id, Some(&home));
    assert!(
        outcome.is_err(),
        "a trashed session refuses an explicit summary update"
    );
    assert_eq!(
        db.message_count(&s.id).unwrap(),
        stored.len() as i64,
        "the messages themselves were never deleted"
    );
    assert!(db.get_session_context(&s.id).unwrap().is_none());
    assert!(db.items_for_workstream(&ws.id, true).unwrap().is_empty());
}
