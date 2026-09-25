//! LaunchIntent, Resume Delta and Storage/Domain consistency tests
//! (Issues #5, #6, #8).
//!
//! Refactored for the single-Owner model (方案 §14–§21): a Session has at most
//! one Owner Workstream, so an intent's `owner_workstream_id` is inherited by
//! the matched Session verbatim. There are no primary/related roles and no
//! multi-workstream bundles.

use noending::domain::{
    launch_status, Agent, ContextDelivery, LaunchIntent, Session, SourceCursor,
};
use noending::launcher::LaunchWorkspace;
use noending::storage::{new_id, now, Db};
use noending::{context, launcher};

fn open_db(tag: &str) -> Db {
    let dir = std::env::temp_dir().join(format!("noending-launch-{}-{}", tag, new_id()));
    Db::open(&dir.join("test.db")).unwrap()
}

fn ws_row(db: &Db, title: &str) -> noending::domain::Workstream {
    let w = noending::domain::Workstream {
        id: new_id(),
        title: title.into(),
        description: String::new(),
        lifecycle: "active".into(),
        visibility: "normal".into(),
        created_at: now(),
        updated_at: now(),
    };
    db.upsert_workstream(&w).unwrap();
    w
}

fn project_row(db: &Db, name: &str) -> noending::domain::Project {
    let p = noending::domain::Project {
        id: new_id(),
        name: name.into(),
        description: String::new(),
        git_id: None,
        name_customized: false,
        created_at: now(),
        updated_at: now(),
    };
    db.upsert_project(&p).unwrap();
    p
}

fn session_row(db: &Db, agent: Agent, started_at: Option<String>, cwd: Option<String>) -> Session {
    let raw = std::env::temp_dir().join(format!("noending-raw-{}.jsonl", new_id()));
    let _ = std::fs::write(&raw, "");
    let s = Session {
        id: new_id(),
        agent,
        agent_session_id: format!("as-{}", new_id()),
        title: None,
        cwd,
        workspace_path_id: None,
        project_id: None,
        owner_workstream_id: None,
        raw_path: raw.to_string_lossy().to_string(),
        parent_agent_session_id: None,
        started_at: started_at.clone(),
        last_activity_at: started_at,
        trashed_at: None,
    };
    db.upsert_session(&s).unwrap();
    s
}

/// A pending New Session intent carrying the user's single Owner selection.
fn pending_intent(db: &Db, agent: Agent, owner: Option<&str>, cwd: Option<String>) -> LaunchIntent {
    let i = LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent,
        owner_workstream_id: owner.map(str::to_string),
        cwd,
        context_bundle_markdown: None,
        context_bundle_revisions: None,
        process_id: None,
        launched_at: now(),
        matched_session_id: None,
        status: launch_status::PENDING.into(),
        note: String::new(),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_launch_intent(&i).unwrap();
    i
}

/// The Session's current Owner, straight from the store.
fn owner_of(db: &Db, session_id: &str) -> Option<String> {
    db.get_session(session_id)
        .unwrap()
        .unwrap()
        .owner_workstream_id
}

// ---------------------------------------------------------------------------
// Issue #5: LaunchIntent
// ---------------------------------------------------------------------------

/// Launch → discovery → Owner: a pending intent matches a newly discovered
/// session and the user's explicit Workstream selection becomes the Session's
/// Owner Workstream. This is also the crash-recovery path: the intent was
/// persisted before the "crash", the session is discovered by a later reconcile.
#[test]
fn pending_intent_matches_new_session_and_sets_owner() {
    let db = open_db("intent-match");
    let ws_a = ws_row(&db, "Workstream A");

    // user picks A in the New Session dialog, then NoEnding launches
    let intent = pending_intent(&db, Agent::Codex, Some(&ws_a.id), None);

    // the agent CLI creates its session; we discover it afterwards
    let session = session_row(&db, Agent::Codex, Some(now()), None);
    assert!(
        launcher::try_match_launch_intents_in(&db, &session, &LaunchWorkspace::default()).unwrap(),
        "the fresh session must claim the pending intent"
    );

    // the explicit selection becomes the single Owner Workstream
    assert_eq!(
        owner_of(&db, &session.id).as_deref(),
        Some(ws_a.id.as_str())
    );
    let owned = db.sessions_for_workstream(&ws_a.id).unwrap();
    assert_eq!(owned.len(), 1, "exactly one owned Session");
    assert_eq!(owned[0].id, session.id);

    let intent = db.get_launch_intent(&intent.id).unwrap().unwrap();
    assert_eq!(intent.status, launch_status::MATCHED);
    assert_eq!(
        intent.matched_session_id.as_deref(),
        Some(session.id.as_str())
    );
}

/// Zero selected workstreams is a valid state: the intent matches but the
/// Session stays unowned, and no classification adds an owner behind the
/// user's back.
#[test]
fn contextless_launch_matches_but_stays_unowned() {
    let db = open_db("intent-zero");
    let _ws = ws_row(&db, "Unrelated");
    let intent = pending_intent(&db, Agent::Pi, None, None);

    let session = session_row(&db, Agent::Pi, Some(now()), None);
    assert!(
        launcher::try_match_launch_intents_in(&db, &session, &LaunchWorkspace::default()).unwrap()
    );
    assert_eq!(owner_of(&db, &session.id), None);
    assert_eq!(
        db.get_launch_intent(&intent.id).unwrap().unwrap().status,
        launch_status::MATCHED
    );
}

/// Several similarly-plausible candidates → ambiguous, never a silent guess.
#[test]
fn ambiguous_candidates_wait_for_the_user() {
    let db = open_db("intent-ambiguous");
    let ws = ws_row(&db, "WS");
    // two intents launched at nearly the same time for the same agent
    let i1 = pending_intent(&db, Agent::ClaudeCode, Some(&ws.id), None);
    let i2 = pending_intent(&db, Agent::ClaudeCode, Some(&ws.id), None);
    let _ = (&i1, &i2);

    let session = session_row(&db, Agent::ClaudeCode, Some(now()), None);
    assert!(
        !launcher::try_match_launch_intents_in(&db, &session, &LaunchWorkspace::default()).unwrap(),
        "must not silently pick one"
    );
    let ambiguous = db
        .list_launch_intents(&[launch_status::AMBIGUOUS], 10)
        .unwrap();
    assert!(
        !ambiguous.is_empty(),
        "the best candidate is marked ambiguous"
    );
    assert_eq!(
        owner_of(&db, &session.id),
        None,
        "no owner before the user resolves"
    );

    // user resolves manually
    launcher::apply_match(
        &db,
        &ambiguous[0],
        &session,
        &launcher::LaunchWorkspace::default(),
    )
    .unwrap();
    let resolved = db.get_launch_intent(&ambiguous[0].id).unwrap().unwrap();
    assert_eq!(resolved.status, launch_status::MATCHED);
    assert_eq!(owner_of(&db, &session.id).as_deref(), Some(ws.id.as_str()));
}

/// Wrong agent / stale sessions never match; stale intents expire.
#[test]
fn stale_intents_expire_and_wrong_agent_never_matches() {
    let db = open_db("intent-expire");
    let ws = ws_row(&db, "WS");
    let intent = pending_intent(&db, Agent::Codex, Some(&ws.id), None);

    // a Claude session cannot claim a Codex intent
    let claude_session = session_row(&db, Agent::ClaudeCode, Some(now()), None);
    assert!(!launcher::try_match_launch_intents_in(
        &db,
        &claude_session,
        &LaunchWorkspace::default()
    )
    .unwrap());

    // expire pending intents that are older than the TTL
    db.update_launch_intent(&intent.id, launch_status::PENDING, None, "")
        .unwrap();
    db.write()
        .execute(
            "UPDATE launch_intents SET launched_at = ?2 WHERE id = ?1",
            rusqlite::params![
                intent.id,
                (chrono::Utc::now() - chrono::Duration::hours(48)).to_rfc3339()
            ],
        )
        .unwrap();
    let expired = launcher::expire_stale_launch_intents(&db).unwrap();
    assert_eq!(expired, 1);
    assert_eq!(
        db.get_launch_intent(&intent.id).unwrap().unwrap().status,
        launch_status::EXPIRED
    );

    // ...and an old session cannot claim a fresh intent either
    let old_session = session_row(
        &db,
        Agent::Codex,
        Some((chrono::Utc::now() - chrono::Duration::hours(48)).to_rfc3339()),
        None,
    );
    let fresh = pending_intent(&db, Agent::Codex, Some(&ws.id), None);
    assert!(
        !launcher::try_match_launch_intents_in(&db, &old_session, &LaunchWorkspace::default())
            .unwrap()
    );
    assert_eq!(
        db.get_launch_intent(&fresh.id).unwrap().unwrap().status,
        launch_status::PENDING
    );
}

/// Resume uses the stored Owner; discovery never re-derives it.
#[test]
fn resume_does_not_reguess_owner() {
    let db = open_db("resume-no-guess");
    let ws = ws_row(&db, "WS");
    let s = session_row(&db, Agent::Codex, Some(now()), None);
    db.set_session_owner(&s.id, Some(&ws.id)).unwrap();

    // No pending intent exists, so the discovery path has nothing to match:
    // the Owner is a stored fact and must survive untouched.
    assert!(!launcher::try_match_launch_intents_in(&db, &s, &LaunchWorkspace::default()).unwrap());
    assert_eq!(owner_of(&db, &s.id).as_deref(), Some(ws.id.as_str()));
}

// ---------------------------------------------------------------------------
// Issue #6: resume delta
// ---------------------------------------------------------------------------

fn seed_context(db: &Db, ws_id: &str, titles: &[&str]) -> Vec<noending::domain::ContextItem> {
    titles
        .iter()
        .map(|t| {
            noending::sync::create_item(
                db,
                ws_id,
                "constraint",
                t,
                &format!("content of {}", t),
                "user_edit",
                "user_edit",
                &[],
                None,
                "user",
            )
            .unwrap()
        })
        .collect()
}

#[test]
fn resume_requires_session_and_first_delivery_is_full_context() {
    let db = open_db("resume-first");
    let ws = ws_row(&db, "resume ws");
    // a goal makes the "minimal core reminder" meaningful
    noending::sync::create_item(
        &db,
        &ws.id,
        "goal",
        "完成上下文同步机制",
        "覆盖增量读取与合并",
        "user_explicit",
        "user_edit",
        &[],
        None,
        "user",
    )
    .unwrap();
    seed_context(&db, &ws.id, &["约束一", "约束二"]);
    let s = session_row(&db, Agent::Codex, Some(now()), None);

    // resume without a session is rejected
    assert!(context::build_bundle(
        &db,
        "resume",
        None,
        Some(ws.id.as_str()),
        context::ContextDeliveryLevel::Balanced
    )
    .is_err());

    // first resume (no delivery yet): full context is delivered
    let bundle = context::build_bundle(
        &db,
        "resume",
        Some(&s),
        Some(ws.id.as_str()),
        context::ContextDeliveryLevel::Balanced,
    )
    .unwrap();
    assert!(bundle.markdown.contains("约束一"));
    assert!(bundle.markdown.contains("约束二"));

    // record the delivery: every section's revision id
    let delivered: Vec<String> = bundle
        .sections
        .iter()
        .filter_map(|sec| sec.revision_id.clone())
        .collect();
    assert!(!delivered.is_empty());
    db.record_delivery(&ContextDelivery {
        id: new_id(),
        session_id: s.id.clone(),
        workstream_id: ws.id.clone(),
        bundle_id: bundle.bundle_id.clone(),
        delivered_revisions: delivered,
        delivered_conflicts: vec![],
        delivered_at: now(),
    })
    .unwrap();

    // no changes since delivery → only the minimal reminder, no full re-dump
    let unchanged = context::build_bundle(
        &db,
        "resume",
        Some(&s),
        Some(ws.id.as_str()),
        context::ContextDeliveryLevel::Balanced,
    )
    .unwrap();
    assert!(unchanged.markdown.contains("Current Task Reminder"));
    assert!(
        !unchanged.markdown.contains("约束二"),
        "unchanged items are not repeated"
    );
}

#[test]
fn resume_delta_shows_changes_and_disappearances() {
    let db = open_db("resume-delta");
    let ws = ws_row(&db, "delta ws");
    let items = seed_context(&db, &ws.id, &["决策甲", "约束乙"]);
    let s = session_row(&db, Agent::Codex, Some(now()), None);

    // deliver everything
    let bundle = context::build_bundle(
        &db,
        "resume",
        Some(&s),
        Some(ws.id.as_str()),
        context::ContextDeliveryLevel::Balanced,
    )
    .unwrap();
    let delivered: Vec<String> = bundle
        .sections
        .iter()
        .filter_map(|sec| sec.revision_id.clone())
        .collect();
    db.record_delivery(&ContextDelivery {
        id: new_id(),
        session_id: s.id.clone(),
        workstream_id: ws.id.clone(),
        bundle_id: bundle.bundle_id.clone(),
        delivered_revisions: delivered,
        delivered_conflicts: vec![],
        delivered_at: now(),
    })
    .unwrap();

    // 1. resolve one item (disappearance), 2. add a new item
    db.apply_status_change(&items[1].id, "resolved", "user", "已完成", None, &[])
        .unwrap();
    noending::sync::create_item(
        &db,
        &ws.id,
        "decision",
        "新决定：切换构建工具",
        "使用新的构建工具",
        "agent_inferred",
        "session_event",
        &["session-event:e9".into()],
        None,
        "sync:heuristic",
    )
    .unwrap();

    let delta = context::build_bundle(
        &db,
        "resume",
        Some(&s),
        Some(ws.id.as_str()),
        context::ContextDeliveryLevel::Balanced,
    )
    .unwrap();
    assert!(
        delta.markdown.contains("新决定：切换构建工具"),
        "new item appears"
    );
    assert!(delta.markdown.contains("Changed Since Your Last Activity"));
    assert!(
        delta.markdown.contains("约束乙"),
        "resolved item name appears…"
    );
    assert!(
        delta.markdown.contains("Resolved / Superseded"),
        "…in the disappeared section"
    );
    // the unchanged item is not repeated in full
    assert!(!delta
        .sections
        .iter()
        .any(|sec| sec.title == "决策甲" && sec.kind == "constraint"));
}

// ---------------------------------------------------------------------------
// Issue #8: storage / domain consistency
// ---------------------------------------------------------------------------

/// The list_sessions parameter bug: filtering by agent ONLY used to bind
/// ?2 with a single parameter and fail. All four filter combinations work.
///
/// The Project half goes through the authoritative chain (§1.10): the Session is
/// a member because its `workspace_path_id` says so, not because the derived
/// cache happens to hold a value.
#[test]
fn list_sessions_all_filter_combinations() {
    let db = open_db("session-filter");
    let p = project_row(&db, "P");
    let s_codex = session_row(&db, Agent::Codex, Some(now()), None);
    let _s_pi = session_row(&db, Agent::Pi, Some(now()), None);
    let path_id = db
        .tx(|tx| {
            noending::storage::workspace::insert_workspace_path_conn(
                tx,
                &noending::workspace::normalize_path("/list-filter/p").unwrap(),
                &p.id,
            )
        })
        .unwrap();
    db.write()
        .execute(
            "UPDATE sessions SET workspace_path_id = ?2 WHERE id = ?1",
            rusqlite::params![s_codex.id, path_id],
        )
        .unwrap();

    let agent_only = db
        .list_sessions(noending::storage::SessionFilter {
            scope: Default::default(),
            project_id: None,
            agent: Some(Agent::Codex),
        })
        .unwrap();
    assert_eq!(agent_only.len(), 1, "agent-only filter (the old ?N bug)");
    assert_eq!(agent_only[0].agent, Agent::Codex);

    let project_only = db
        .list_sessions(noending::storage::SessionFilter {
            scope: Default::default(),
            project_id: Some(p.id.clone()),
            agent: None,
        })
        .unwrap();
    assert_eq!(project_only.len(), 1);

    let both = db
        .list_sessions(noending::storage::SessionFilter {
            scope: Default::default(),
            project_id: Some(p.id.clone()),
            agent: Some(Agent::Codex),
        })
        .unwrap();
    assert_eq!(both.len(), 1);

    let none = db
        .list_sessions(noending::storage::SessionFilter::default())
        .unwrap();
    assert_eq!(none.len(), 2);
}

/// Automatic workstream discovery must allow project_id = NULL.
#[test]
fn auto_created_workstream_may_have_no_project() {
    let db = open_db("auto-ws");
    let ctx = noending::sync::MergeContext {
        run_id: new_id(),
        runtime: "heuristic".into(),
    };
    let m = noending::sync::ContextMutation::CreateWorkstream {
        title: "自动发现的新工作流".into(),
        reason: "会话中出现新的长期主题".into(),
    };
    let applied = db
        .tx(|tx| noending::sync::merge::MergeEngine.apply(tx, &m, &ctx))
        .unwrap();
    assert!(applied);
    let all = db.list_workstreams(None).unwrap();
    let _auto = all
        .iter()
        .find(|w| w.title == "自动发现的新工作流")
        .expect("created");
}

/// get_event_by_ref resolves stable event ids and rejects unknown refs.
#[test]
fn event_ref_roundtrip() {
    let db = open_db("event-ref");
    let s = session_row(&db, Agent::Codex, Some(now()), None);
    let source = noending::domain::SourceCursorUpdate {
        file_identity: "dev:1:ino:3".into(),
        generation: 0,
        byte_offset: 10,
        last_seen_size: 10,
        mtime: None,
        start_byte_offset: 10,
        prefix_hash: String::new(),
    };
    let stored = db
        .append_source_events(
            &s.id,
            &[noending::domain::ParsedEvent {
                source_event_id: None,
                source_position: "line:1".into(),
                ts: Some("t".into()),
                kind: "user_message".into(),
                text: Some("ref target".into()),
                metadata: serde_json::json!({}),
            }],
            &source,
            "/x.jsonl",
        )
        .unwrap();
    let ev = &stored[0];

    // stable reference
    let by_id = db
        .get_event_by_ref(&format!("session-event:{}", ev.id))
        .unwrap()
        .unwrap();
    assert_eq!(by_id.id, ev.id);
    // unknown ref → None, never fabricated
    assert!(db
        .get_event_by_ref("session-event:missing")
        .unwrap()
        .is_none());
}

/// A fresh cursor row exists per session with sane defaults.
#[test]
fn source_cursor_roundtrip() {
    let db = open_db("cursor-roundtrip");
    let s = session_row(&db, Agent::Codex, Some(now()), None);
    let c = SourceCursor {
        session_id: s.id.clone(),
        source_file_identity: "unix:dev:1:ino:42".into(),
        generation: 3,
        byte_offset: 900,
        last_seen_size: 1000,
        mtime: Some(1234.5),
        prefix_hash: "abc123".into(),
        identity_tail_hash: "deadbeef".into(),
        last_sequence: 7,
    };
    db.set_source_cursor(&c).unwrap();
    let back = db.get_source_cursor(&s.id).unwrap();
    assert_eq!(back.source_file_identity, c.source_file_identity);
    assert_eq!(back.generation, 3);
    assert_eq!(back.byte_offset, 900);
    assert_eq!(back.prefix_hash, "abc123");
    assert_eq!(back.identity_tail_hash, "deadbeef");
    assert_eq!(back.last_sequence, 7);
    assert_eq!(db.get_cursor(&s.id).unwrap(), 7);
}

/// A New Session launched with injected context RECEIVED that context:
/// apply_match must record the intent's delivery snapshot, so the first
/// resume computes a true delta instead of re-sending the full context.
#[test]
fn launch_intent_match_records_delivery_snapshot() {
    let db = open_db("intent-delivery");
    let ws = ws_row(&db, "delivery ws");
    let items = seed_context(&db, &ws.id, &["已交付约束"]);
    assert!(!items.is_empty());
    // create_item returns the in-memory item; the head revision lives in DB
    let head_rev = |item_id: &str| {
        db.get_item(item_id)
            .unwrap()
            .unwrap()
            .current_revision_id
            .unwrap()
    };
    let s = session_row(&db, Agent::Codex, Some(now()), None);

    // intent snapshot: what the launched session actually received
    let intent = LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent: Agent::Codex,
        owner_workstream_id: Some(ws.id.clone()),
        cwd: None,
        context_bundle_markdown: Some("full bundle".into()),
        context_bundle_revisions: Some(
            serde_json::json!({
                "bundle_id": "bundle-1",
                "by_workstream": { ws.id.clone(): [head_rev(&items[0].id)] }
            })
            .to_string(),
        ),
        process_id: None,
        launched_at: now(),
        matched_session_id: None,
        status: launch_status::PENDING.into(),
        note: String::new(),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_launch_intent(&intent).unwrap();

    assert!(launcher::apply_match(&db, &intent, &s, &launcher::LaunchWorkspace::default()).is_ok());

    // the Owner is established explicitly…
    assert_eq!(owner_of(&db, &s.id).as_deref(), Some(ws.id.as_str()));

    // …and the delivery recorded per workstream from the snapshot
    let deliveries = db.latest_deliveries(&s.id).unwrap();
    let d = deliveries
        .iter()
        .find(|d| d.workstream_id == ws.id)
        .expect("delivery recorded");
    assert_eq!(d.bundle_id, "bundle-1");
    assert_eq!(
        d.delivered_revisions,
        vec![head_rev(&items[0].id)],
        "only this workstream's revisions are attributed to it"
    );

    // first resume: the delivered item is NOT re-sent in full
    let bundle = context::build_bundle(
        &db,
        "resume",
        Some(&s),
        Some(ws.id.as_str()),
        context::ContextDeliveryLevel::Balanced,
    )
    .unwrap();
    assert!(
        !bundle.markdown.contains("已交付约束"),
        "already-delivered content must not be re-sent, got: {}",
        bundle.markdown
    );
}

/// The token budget filters SECTIONS before rendering, so `bundle.sections`
/// describes exactly what the markdown delivered. A delivery snapshot is
/// derived from sections — if budget truncation happened after rendering,
/// context the agent never received would be recorded as "delivered" and
/// the next resume would skip it as a false delta.
#[test]
fn token_budget_limits_sections_to_actually_delivered_content() {
    let db = open_db("budget");
    let ws = ws_row(&db, "budget ws");

    // two small core items, then a dozen large ones: whatever the exact
    // cut point, some sections must fit and some must not
    // (create_item returns the in-memory item without its head pointer —
    // re-read from the DB to get the persisted revision id)
    let head_rev = |item_id: &str| -> String {
        db.get_item(item_id)
            .unwrap()
            .unwrap()
            .current_revision_id
            .unwrap()
    };
    let mut named: Vec<(String, String)> = Vec::new();
    // items render newest-first (updated_at DESC): create the LARGE ones
    // first so the two small ones sort to the top and fit the budget
    for i in 0..12 {
        let title = format!("大条目{i:02}");
        let item = noending::sync::create_item(
            &db,
            &ws.id,
            "constraint",
            &title,
            &"很长的上下文内容。".repeat(80),
            "user_edit",
            "user_edit",
            &[],
            None,
            "user",
        )
        .unwrap();
        named.push((title, head_rev(&item.id)));
    }
    for title in ["预算内约束A", "预算内约束B"] {
        let item = noending::sync::create_item(
            &db,
            &ws.id,
            "constraint",
            title,
            &format!("content of {}", title),
            "user_edit",
            "user_edit",
            &[],
            None,
            "user",
        )
        .unwrap();
        named.push((title.into(), head_rev(&item.id)));
    }

    let policy = context::ContextDeliveryPolicy {
        enabled: true,
        new_token_budget: 200,
        resume_token_budget: 200,
        new_extended_limit: 20,
        resume_first_delivery_extended_limit: 10,
        conflict_limit: 10,
    };
    let bundle = context::build_bundle_with_policy(
        &db,
        "new",
        None,
        Some(ws.id.as_str()),
        policy,
        "test_custom",
    )
    .unwrap();

    assert!(
        bundle.approx_tokens <= 200,
        "budget respected, got {}",
        bundle.approx_tokens
    );
    let delivered: Vec<String> = bundle
        .sections
        .iter()
        .filter_map(|s| s.revision_id.clone())
        .collect();
    assert!(
        delivered.len() < named.len(),
        "budget actually excluded sections ({}/{} delivered)",
        delivered.len(),
        named.len()
    );
    assert!(!delivered.is_empty(), "some sections still fit the budget");
    assert!(
        bundle.markdown.contains("… (上下文因预算被截断)"),
        "truncation is visible in the delivered markdown"
    );

    // the invariant that matters for delivery snapshots: a section is
    // claimed as delivered ⟺ its content is actually in the markdown
    for (title, rev) in &named {
        let is_delivered = delivered.contains(rev);
        assert_eq!(
            is_delivered,
            bundle.markdown.contains(title.as_str()),
            "section '{}' delivered/markdown mismatch",
            title
        );
    }
}

// ---------------------------------------------------------------------------
// Context Delivery Level tests
// ---------------------------------------------------------------------------

#[test]
fn context_delivery_level_default_and_roundtrip() {
    let db = open_db("delivery-level-setting");

    // 1. Unset → Base Experience default is Off (nothing is injected until the
    // user chooses a level; the v11 migration pins an explicit 'off' row for
    // databases that predate it, and a missing row still means Off).
    let lvl = noending::commands::context_delivery_level_of(&db).unwrap();
    assert_eq!(lvl, context::ContextDeliveryLevel::Off);
    db.delete_setting(noending::commands::CONTEXT_DELIVERY_LEVEL_KEY)
        .unwrap();
    assert_eq!(
        noending::commands::context_delivery_level_of(&db).unwrap(),
        context::ContextDeliveryLevel::Off,
        "a missing row must never re-enable injection"
    );

    // 2. Persisted compact → Compact
    db.set_setting(noending::commands::CONTEXT_DELIVERY_LEVEL_KEY, "compact")
        .unwrap();
    let lvl = noending::commands::context_delivery_level_of(&db).unwrap();
    assert_eq!(lvl, context::ContextDeliveryLevel::Compact);

    // 3. String parser checks & illegal values
    assert_eq!(
        context::ContextDeliveryLevel::parse("off"),
        Some(context::ContextDeliveryLevel::Off)
    );
    assert_eq!(
        context::ContextDeliveryLevel::parse("compact"),
        Some(context::ContextDeliveryLevel::Compact)
    );
    assert_eq!(
        context::ContextDeliveryLevel::parse("balanced"),
        Some(context::ContextDeliveryLevel::Balanced)
    );
    assert_eq!(
        context::ContextDeliveryLevel::parse("detailed"),
        Some(context::ContextDeliveryLevel::Detailed)
    );
    assert_eq!(context::ContextDeliveryLevel::parse("invalid"), None);
    assert_eq!(context::ContextDeliveryLevel::parse(""), None);
}

#[test]
fn context_delivery_policy_monotonicity() {
    let off = context::ContextDeliveryLevel::Off.policy();
    assert!(!off.enabled);
    assert_eq!(off.new_token_budget, 0);
    assert_eq!(off.resume_token_budget, 0);

    let compact = context::ContextDeliveryLevel::Compact.policy();
    let balanced = context::ContextDeliveryLevel::Balanced.policy();
    let detailed = context::ContextDeliveryLevel::Detailed.policy();

    assert!(compact.enabled && balanced.enabled && detailed.enabled);

    // strictly increasing budgets
    assert!(compact.new_token_budget < balanced.new_token_budget);
    assert!(balanced.new_token_budget < detailed.new_token_budget);
    assert_eq!(balanced.new_token_budget, 4000);

    assert!(compact.resume_token_budget < balanced.resume_token_budget);
    assert!(balanced.resume_token_budget < detailed.resume_token_budget);
    assert_eq!(balanced.resume_token_budget, 3000);

    // strictly increasing item limits
    assert!(compact.new_extended_limit < balanced.new_extended_limit);
    assert!(balanced.new_extended_limit < detailed.new_extended_limit);

    assert!(
        compact.resume_first_delivery_extended_limit
            < balanced.resume_first_delivery_extended_limit
    );
    assert!(
        balanced.resume_first_delivery_extended_limit
            < detailed.resume_first_delivery_extended_limit
    );

    assert!(compact.conflict_limit < balanced.conflict_limit);
    assert!(balanced.conflict_limit < detailed.conflict_limit);
}

#[test]
fn context_delivery_off_produces_empty_bundle() {
    let db = open_db("delivery-off-bundle");
    let ws = ws_row(&db, "off ws");
    seed_context(&db, &ws.id, &["重要目标", "关键约束"]);

    // New bundle with Off
    let bundle = context::build_bundle(
        &db,
        "new",
        None,
        Some(ws.id.as_str()),
        context::ContextDeliveryLevel::Off,
    )
    .unwrap();

    assert!(bundle.sections.is_empty());
    assert!(bundle.markdown.is_empty());
    assert_eq!(bundle.approx_tokens, 0);
    assert_eq!(bundle.delivery_level, "off");

    // Resume bundle with Off
    let s = session_row(&db, Agent::Codex, Some(now()), None);
    let r_bundle = context::build_bundle(
        &db,
        "resume",
        Some(&s),
        Some(ws.id.as_str()),
        context::ContextDeliveryLevel::Off,
    )
    .unwrap();

    assert!(r_bundle.sections.is_empty());
    assert!(r_bundle.markdown.is_empty());
    assert_eq!(r_bundle.approx_tokens, 0);
    assert_eq!(r_bundle.delivery_level, "off");
}

/// Invariant: New Session launched with Context Delivery = Off must STILL
/// record the Owner Workstream and set it on discovery, but must NEVER record
/// a ContextDelivery snapshot.
#[test]
fn launch_intent_with_off_sets_owner_but_no_delivery() {
    let db = open_db("intent-off-delivery");
    let ws = ws_row(&db, "ws-off");
    seed_context(&db, &ws.id, &["约束"]);
    let s = session_row(&db, Agent::Codex, Some(now()), None);

    // In Off mode, Launcher creates intent without context markdown / revisions
    let intent = LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent: Agent::Codex,
        owner_workstream_id: Some(ws.id.clone()),
        cwd: None,
        context_bundle_markdown: None,
        context_bundle_revisions: None,
        process_id: None,
        launched_at: now(),
        matched_session_id: None,
        status: launch_status::PENDING.into(),
        note: "已关联 Workstream；Context Delivery 已关闭，本次未注入 Context。".into(),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_launch_intent(&intent).unwrap();

    assert!(launcher::apply_match(&db, &intent, &s, &launcher::LaunchWorkspace::default()).is_ok());

    // The Owner Workstream is preserved!
    assert_eq!(owner_of(&db, &s.id).as_deref(), Some(ws.id.as_str()));

    // But NO ContextDelivery is recorded!
    let deliveries = db.latest_deliveries(&s.id).unwrap();
    assert!(
        deliveries.is_empty(),
        "Off mode must not create a ContextDelivery row"
    );
}

/// P0 Invariant: Balanced → Off → Balanced
/// When delivery is turned Off, interim revisions created during Off phase
/// are NOT marked as delivered. When switched back to Balanced, the next
/// resume DELTA must deliver all those interim revisions!
#[test]
fn balanced_off_balanced_preserves_revisions_in_delta() {
    let db = open_db("balanced-off-balanced");
    let ws = ws_row(&db, "ws-lifecycle");
    let s = session_row(&db, Agent::Codex, Some(now()), None);

    let head_rev = |item_id: &str| {
        db.get_item(item_id)
            .unwrap()
            .unwrap()
            .current_revision_id
            .unwrap()
    };

    // Phase 1: Balanced delivery. Agent receives Revision 1.
    let item1 = &seed_context(&db, &ws.id, &["初始约束 1"])[0];
    let rev1 = head_rev(&item1.id);

    db.record_delivery(&ContextDelivery {
        id: new_id(),
        session_id: s.id.clone(),
        workstream_id: ws.id.clone(),
        bundle_id: "bundle-initial".into(),
        delivered_revisions: vec![rev1.clone()],
        delivered_conflicts: vec![],
        delivered_at: now(),
    })
    .unwrap();

    // Phase 2: User sets Context Delivery = Off.
    // In the meantime, two new items (Revision 2 & 3) are added.
    let item2 = &seed_context(&db, &ws.id, &["中期决策 2"])[0];
    let item3 = &seed_context(&db, &ws.id, &["中期架构 3"])[0];
    let _rev2 = head_rev(&item2.id);
    let _rev3 = head_rev(&item3.id);

    // During Off, a Resume occurs.
    let off_bundle = context::build_bundle(
        &db,
        "resume",
        Some(&s),
        Some(ws.id.as_str()),
        context::ContextDeliveryLevel::Off,
    )
    .unwrap();
    assert_eq!(off_bundle.delivery_level, "off");
    assert!(off_bundle.markdown.is_empty());
    // Crucial: Launcher in Off mode does NOT call db.record_delivery.
    // Latest delivery snapshot is still at Phase 1 (rev1).
    let deliveries = db.latest_deliveries(&s.id).unwrap();
    let last = deliveries
        .iter()
        .find(|d| d.workstream_id == ws.id)
        .unwrap();
    assert_eq!(last.delivered_revisions, vec![rev1.clone()]);

    // Phase 3: User turns Context Delivery back to Balanced.
    // Next Resume occurs.
    let balanced_bundle = context::build_bundle(
        &db,
        "resume",
        Some(&s),
        Some(ws.id.as_str()),
        context::ContextDeliveryLevel::Balanced,
    )
    .unwrap();

    // Both interim revisions MUST be present in the delta!
    assert!(
        balanced_bundle.markdown.contains("中期决策 2"),
        "Interim revision 2 must be delivered in delta after re-enabling Balanced"
    );
    assert!(
        balanced_bundle.markdown.contains("中期架构 3"),
        "Interim revision 3 must be delivered in delta after re-enabling Balanced"
    );
    // Revision 1 was already delivered in Phase 1, so it shouldn't be in delta
    assert!(
        !balanced_bundle.markdown.contains("初始约束 1"),
        "Revision 1 was delivered previously, must not be repeated"
    );
}

/// P0: Delivery snapshots are cumulative (Agent-Known State). Consecutive
/// resumes without changes must NEVER re-send items as spurious deltas.
#[test]
fn resume_preserves_cumulative_known_state_without_spurious_deltas() {
    let db = open_db("cumulative-state");
    let ws = ws_row(&db, "cumulative ws");

    let goal = noending::sync::create_item(
        &db,
        &ws.id,
        "goal",
        "核心目标",
        "目标描述",
        "user_explicit",
        "user_explicit",
        &[],
        None,
        "user",
    )
    .unwrap();
    let state = noending::sync::create_item(
        &db,
        &ws.id,
        "current_state",
        "当前状态",
        "状态描述",
        "user_explicit",
        "user_explicit",
        &[],
        None,
        "user",
    )
    .unwrap();
    let constraint = noending::sync::create_item(
        &db,
        &ws.id,
        "constraint",
        "核心约束",
        "必须向后兼容",
        "user_explicit",
        "user_explicit",
        &[],
        None,
        "user",
    )
    .unwrap();
    let decision = noending::sync::create_item(
        &db,
        &ws.id,
        "decision",
        "重大决定",
        "采用 SQLite",
        "user_explicit",
        "user_explicit",
        &[],
        None,
        "user",
    )
    .unwrap();

    let head_rev = |id: &str| {
        db.get_item(id)
            .unwrap()
            .unwrap()
            .current_revision_id
            .unwrap()
    };

    let all_revs = vec![
        head_rev(&goal.id),
        head_rev(&state.id),
        head_rev(&constraint.id),
        head_rev(&decision.id),
    ];

    let s = session_row(&db, Agent::Codex, Some(now()), None);

    // Initial delivery snapshot (e.g. from New Session launch)
    let d0 = ContextDelivery {
        id: new_id(),
        session_id: s.id.clone(),
        workstream_id: ws.id.clone(),
        bundle_id: "bundle-initial".into(),
        delivered_revisions: all_revs.clone(),
        delivered_conflicts: vec![],
        delivered_at: now(),
    };
    db.record_delivery(&d0).unwrap();

    // Resume round 1: No changes in DB
    let b1 = context::build_bundle(
        &db,
        "resume",
        Some(&s),
        Some(ws.id.as_str()),
        context::ContextDeliveryLevel::Balanced,
    )
    .unwrap();
    // Round 1 only has reminders, no deltas
    assert!(!b1.sections.iter().any(|sec| sec.kind == "delta"));

    // Compute cumulative delivery for round 1
    let d1 =
        launcher::compute_cumulative_delivery(&db, &s.id, &ws.id, &b1, Some(&d0), &ws.id).unwrap();
    // Invariant: The cumulative snapshot must STILL contain all 4 revisions!
    assert_eq!(
        d1.delivered_revisions.len(),
        4,
        "Round 1 cumulative delivery must retain all known revisions"
    );
    for r in &all_revs {
        assert!(d1.delivered_revisions.contains(r));
    }
    db.record_delivery(&d1).unwrap();

    // Resume round 2: Still no changes in DB
    let b2 = context::build_bundle(
        &db,
        "resume",
        Some(&s),
        Some(ws.id.as_str()),
        context::ContextDeliveryLevel::Balanced,
    )
    .unwrap();

    // Prior bug: In round 2, Constraint and Decision were treated as undelivered
    // and emitted as deltas!
    // With cumulative snapshots, there MUST be NO deltas!
    let deltas: Vec<&context::ContextSection> = b2
        .sections
        .iter()
        .filter(|sec| sec.kind == "delta")
        .collect();
    assert!(
        deltas.is_empty(),
        "No spurious deltas allowed on second resume without changes, but got: {:?}",
        deltas
    );
    assert!(!b2.markdown.contains("Changed Since Your Last Activity"));
}

/// P1: Conflicts truncated by token budget must not be permanently lost,
/// and delivered conflicts must be deduplicated across consecutive resumes.
#[test]
fn conflict_not_lost_when_truncated_by_budget() {
    let db = open_db("conflict-budget");
    let ws = ws_row(&db, "conflict ws");
    let item = noending::sync::create_item(
        &db,
        &ws.id,
        "constraint",
        "约束条目",
        "内容",
        "user_explicit",
        "user_explicit",
        &[],
        None,
        "user",
    )
    .unwrap();

    let conflict_id = new_id();
    db.insert_conflict(&noending::domain::ContextConflict {
        id: conflict_id.clone(),
        workstream_id: ws.id.clone(),
        left_item_id: item.id.clone(),
        right_item_id: None,
        conflict_type: "authority".into(),
        status: "open".into(),
        resolution: None,
        created_at: now(),
        updated_at: now(),
        left_revision_id: None,
        right_revision_id: None,
        candidate_snapshot_json: None,
    })
    .unwrap();

    let s = session_row(&db, Agent::Codex, Some(now()), None);

    // Initial delivery where conflict is truncated by a tiny budget (e.g. 5 tokens)
    let tiny_policy = context::ContextDeliveryPolicy {
        enabled: true,
        new_token_budget: 5,
        resume_token_budget: 5,
        new_extended_limit: 0,
        resume_first_delivery_extended_limit: 0,
        conflict_limit: 1,
    };
    let b_tiny = context::build_bundle_with_policy(
        &db,
        "resume",
        Some(&s),
        Some(ws.id.as_str()),
        tiny_policy,
        "custom",
    )
    .unwrap();
    // Budget truncated the conflict section
    assert!(!b_tiny
        .sections
        .iter()
        .any(|sec| sec.conflict_id.as_deref() == Some(&conflict_id)));

    // Cumulative delivery computed from truncated bundle
    let d_tiny =
        launcher::compute_cumulative_delivery(&db, &s.id, &ws.id, &b_tiny, None, &ws.id).unwrap();
    assert!(
        !d_tiny.delivered_conflicts.contains(&conflict_id),
        "Truncated conflict must not be recorded as delivered"
    );
    db.record_delivery(&d_tiny).unwrap();

    // Next resume with normal Balanced budget: the conflict was never delivered, so it must be delivered now!
    let b_normal = context::build_bundle(
        &db,
        "resume",
        Some(&s),
        Some(ws.id.as_str()),
        context::ContextDeliveryLevel::Balanced,
    )
    .unwrap();
    assert!(
        b_normal
            .sections
            .iter()
            .any(|sec| sec.conflict_id.as_deref() == Some(&conflict_id)),
        "Undelivered conflict must appear in normal resume"
    );

    // Record delivery of b_normal
    let d_normal =
        launcher::compute_cumulative_delivery(&db, &s.id, &ws.id, &b_normal, Some(&d_tiny), &ws.id)
            .unwrap();
    assert!(
        d_normal.delivered_conflicts.contains(&conflict_id),
        "Delivered conflict must be tracked"
    );
    db.record_delivery(&d_normal).unwrap();

    // Subsequent resume: conflict already delivered, must not be repeated
    let b_after = context::build_bundle(
        &db,
        "resume",
        Some(&s),
        Some(ws.id.as_str()),
        context::ContextDeliveryLevel::Balanced,
    )
    .unwrap();
    assert!(
        !b_after
            .sections
            .iter()
            .any(|sec| sec.conflict_id.as_deref() == Some(&conflict_id)),
        "Delivered conflict must not be re-delivered"
    );
}

/// P1: Filtering of core items must happen BEFORE taking extended_limit,
/// preventing core items from starving extended items in Compact/Balanced modes.
#[test]
fn extended_items_filtering_order_not_starved_by_core_items() {
    let db = open_db("extended-starve");
    let ws = ws_row(&db, "starve ws");

    // Seed 5 core items first
    for kind in [
        "goal",
        "current_state",
        "constraint",
        "decision",
        "open_question",
    ] {
        noending::sync::create_item(
            &db,
            &ws.id,
            kind,
            &format!("核心条目 {}", kind),
            "内容",
            "user_explicit",
            "user_explicit",
            &[],
            None,
            "user",
        )
        .unwrap();
    }

    // Seed 1 extended item
    noending::sync::create_item(
        &db,
        &ws.id,
        "key_fact",
        "核心架构事实：使用 Tokio 运行时",
        "详细说明",
        "user_explicit",
        "user_explicit",
        &[],
        None,
        "user",
    )
    .unwrap();

    // In Compact mode (new_extended_limit = 1):
    // Prior bug: items.iter().take(1) took the first item (core item) and skipped it, producing 0 extended items.
    // Fixed: filter(!CORE).take(1) takes the extended item!
    let bundle = context::build_bundle(
        &db,
        "new",
        None,
        Some(ws.id.as_str()),
        context::ContextDeliveryLevel::Compact,
    )
    .unwrap();

    assert!(
        bundle.markdown.contains("使用 Tokio 运行时"),
        "Compact mode must include extended items without starvation by core items"
    );
    assert!(bundle.sections.iter().any(|s| s.kind == "key_fact"));
}

/// P3: Builder validation must enforce that resume mode requires a session,
/// even when Context Delivery is Off.
#[test]
fn builder_requires_session_in_resume_mode_even_when_off() {
    let db = open_db("resume-off-valid");
    let ws = ws_row(&db, "off ws");

    let err = context::build_bundle(
        &db,
        "resume",
        None,
        Some(ws.id.as_str()),
        context::ContextDeliveryLevel::Off,
    );
    assert!(
        err.is_err(),
        "Resume mode without session must error even in Off mode"
    );
    assert!(err
        .unwrap_err()
        .to_string()
        .contains("Resume 模式必须提供 Session"));
}

/// P1: A deleted item must produce a `gone` section with "被删除" on the next resume,
/// and once delivered, it is removed from the Agent-Known State.
#[test]
fn resume_emits_gone_for_deleted_item() {
    let db = open_db("resume-gone-deleted");
    let ws = ws_row(&db, "deleted ws");
    let item = noending::sync::create_item(
        &db,
        &ws.id,
        "constraint",
        "将被删除的约束",
        "约束内容",
        "user_explicit",
        "user_explicit",
        &[],
        None,
        "user",
    )
    .unwrap();
    let rev_id = db
        .get_item(&item.id)
        .unwrap()
        .unwrap()
        .current_revision_id
        .unwrap();

    let s = session_row(&db, Agent::Codex, Some(now()), None);

    // 1. Initial delivery: agent receives the item
    let d0 = ContextDelivery {
        id: new_id(),
        session_id: s.id.clone(),
        workstream_id: ws.id.clone(),
        bundle_id: "bundle-0".into(),
        delivered_revisions: vec![rev_id.clone()],
        delivered_conflicts: vec![],
        delivered_at: now(),
    };
    db.record_delivery(&d0).unwrap();

    // 2. User deletes the item
    db.apply_status_change(&item.id, "deleted", "user", "已由用户彻底删除", None, &[])
        .unwrap();

    // 3. Next resume: must produce a `gone` section mentioning "被删除"
    let b1 = context::build_bundle(
        &db,
        "resume",
        Some(&s),
        Some(ws.id.as_str()),
        context::ContextDeliveryLevel::Balanced,
    )
    .unwrap();

    let gone_sec = b1
        .sections
        .iter()
        .find(|sec| sec.kind == "gone")
        .expect("must produce gone section for deleted item");
    assert!(
        gone_sec.content.contains("被删除"),
        "Gone section content must say '被删除', got: {}",
        gone_sec.content
    );
    assert!(b1.markdown.contains("被删除"));

    // 4. Record cumulative delivery: revision is removed from known_revisions
    let d1 =
        launcher::compute_cumulative_delivery(&db, &s.id, &ws.id, &b1, Some(&d0), &ws.id).unwrap();
    assert!(
        !d1.delivered_revisions.contains(&rev_id),
        "Deleted item revision must be removed from delivered_revisions"
    );
    db.record_delivery(&d1).unwrap();

    // 5. Subsequent resume without changes: `gone` is NOT repeated
    let b2 = context::build_bundle(
        &db,
        "resume",
        Some(&s),
        Some(ws.id.as_str()),
        context::ContextDeliveryLevel::Balanced,
    )
    .unwrap();
    assert!(
        !b2.sections.iter().any(|sec| sec.kind == "gone"),
        "Delivered gone section must not be repeated"
    );
}

/// P1: If a `gone` section is truncated by token budget, the revision must
/// NOT be prematurely removed from Agent-Known State; it must remain until delivered.
#[test]
fn gone_truncated_by_budget_preserves_revision_in_snapshot() {
    let db = open_db("gone-truncated");
    let ws = ws_row(&db, "gone trunc ws");
    let item = noending::sync::create_item(
        &db,
        &ws.id,
        "constraint",
        "约束",
        "内容",
        "user_explicit",
        "user_explicit",
        &[],
        None,
        "user",
    )
    .unwrap();
    let rev_id = db
        .get_item(&item.id)
        .unwrap()
        .unwrap()
        .current_revision_id
        .unwrap();

    let s = session_row(&db, Agent::Codex, Some(now()), None);

    let d0 = ContextDelivery {
        id: new_id(),
        session_id: s.id.clone(),
        workstream_id: ws.id.clone(),
        bundle_id: "bundle-0".into(),
        delivered_revisions: vec![rev_id.clone()],
        delivered_conflicts: vec![],
        delivered_at: now(),
    };
    db.record_delivery(&d0).unwrap();

    // Item becomes deleted
    db.apply_status_change(&item.id, "deleted", "user", "删除", None, &[])
        .unwrap();

    // Resume with tiny budget where gone section is truncated
    let tiny_policy = context::ContextDeliveryPolicy {
        enabled: true,
        new_token_budget: 5,
        resume_token_budget: 5,
        new_extended_limit: 0,
        resume_first_delivery_extended_limit: 0,
        conflict_limit: 0,
    };
    let b_tiny = context::build_bundle_with_policy(
        &db,
        "resume",
        Some(&s),
        Some(ws.id.as_str()),
        tiny_policy,
        "custom",
    )
    .unwrap();
    assert!(!b_tiny.sections.iter().any(|sec| sec.kind == "gone"));

    // Cumulative delivery computed from truncated bundle must KEEP rev_id
    let d_tiny =
        launcher::compute_cumulative_delivery(&db, &s.id, &ws.id, &b_tiny, Some(&d0), &ws.id)
            .unwrap();
    assert!(
        d_tiny.delivered_revisions.contains(&rev_id),
        "Truncated gone section must NOT remove revision from delivery snapshot"
    );
    db.record_delivery(&d_tiny).unwrap();

    // Next resume with normal budget: gone section appears again!
    let b_normal = context::build_bundle(
        &db,
        "resume",
        Some(&s),
        Some(ws.id.as_str()),
        context::ContextDeliveryLevel::Balanced,
    )
    .unwrap();
    assert!(b_normal.sections.iter().any(|sec| sec.kind == "gone"));
}

/// P1: A resolved conflict must produce a `conflict_resolved` section on resume,
/// and only after actual delivery is the conflict ID removed from delivered_conflicts.
#[test]
fn resume_emits_conflict_resolved_when_conflict_closed() {
    let db = open_db("conflict-resolved");
    let ws = ws_row(&db, "conflict resolved ws");
    let item = noending::sync::create_item(
        &db,
        &ws.id,
        "constraint",
        "冲突约束",
        "内容",
        "user_explicit",
        "user_explicit",
        &[],
        None,
        "user",
    )
    .unwrap();

    let conflict_id = new_id();
    db.insert_conflict(&noending::domain::ContextConflict {
        id: conflict_id.clone(),
        workstream_id: ws.id.clone(),
        left_item_id: item.id.clone(),
        right_item_id: None,
        conflict_type: "authority".into(),
        status: "open".into(),
        resolution: None,
        created_at: now(),
        updated_at: now(),
        left_revision_id: None,
        right_revision_id: None,
        candidate_snapshot_json: None,
    })
    .unwrap();

    let s = session_row(&db, Agent::Codex, Some(now()), None);

    // Initial delivery: agent knows the open conflict
    let d0 = ContextDelivery {
        id: new_id(),
        session_id: s.id.clone(),
        workstream_id: ws.id.clone(),
        bundle_id: "bundle-0".into(),
        delivered_revisions: vec![],
        delivered_conflicts: vec![conflict_id.clone()],
        delivered_at: now(),
    };
    db.record_delivery(&d0).unwrap();

    // Conflict is resolved by user
    db.update_conflict_status(&conflict_id, "resolved", Some("双方已达成共识并合并"))
        .unwrap();

    // Next resume: must produce a `conflict_resolved` section
    let b1 = context::build_bundle(
        &db,
        "resume",
        Some(&s),
        Some(ws.id.as_str()),
        context::ContextDeliveryLevel::Balanced,
    )
    .unwrap();

    let res_sec = b1
        .sections
        .iter()
        .find(|sec| sec.kind == "conflict_resolved")
        .expect("must produce conflict_resolved section");
    assert_eq!(res_sec.conflict_id.as_deref(), Some(conflict_id.as_str()));
    assert!(res_sec.content.contains("双方已达成共识并合并"));
    assert!(b1.markdown.contains("Resolved Conflicts"));

    // Record delivery: conflict ID is removed from delivered_conflicts
    let d1 =
        launcher::compute_cumulative_delivery(&db, &s.id, &ws.id, &b1, Some(&d0), &ws.id).unwrap();
    assert!(
        !d1.delivered_conflicts.contains(&conflict_id),
        "Resolved conflict must be removed from delivered_conflicts after delivery"
    );
    db.record_delivery(&d1).unwrap();

    // Subsequent resume without changes: conflict_resolved is NOT repeated
    let b2 = context::build_bundle(
        &db,
        "resume",
        Some(&s),
        Some(ws.id.as_str()),
        context::ContextDeliveryLevel::Balanced,
    )
    .unwrap();
    assert!(
        !b2.sections
            .iter()
            .any(|sec| sec.kind == "conflict_resolved"),
        "Delivered conflict_resolved must not be repeated"
    );
}

/// P1: If `conflict_resolved` is truncated by token budget, the conflict ID
/// must remain in `delivered_conflicts` until delivered.
#[test]
fn conflict_resolved_truncated_by_budget_retains_conflict_id() {
    let db = open_db("conflict-res-trunc");
    let ws = ws_row(&db, "conflict res trunc ws");
    let item = noending::sync::create_item(
        &db,
        &ws.id,
        "constraint",
        "约束",
        "内容",
        "user_explicit",
        "user_explicit",
        &[],
        None,
        "user",
    )
    .unwrap();

    let conflict_id = new_id();
    db.insert_conflict(&noending::domain::ContextConflict {
        id: conflict_id.clone(),
        workstream_id: ws.id.clone(),
        left_item_id: item.id.clone(),
        right_item_id: None,
        conflict_type: "authority".into(),
        status: "open".into(),
        resolution: None,
        created_at: now(),
        updated_at: now(),
        left_revision_id: None,
        right_revision_id: None,
        candidate_snapshot_json: None,
    })
    .unwrap();

    let s = session_row(&db, Agent::Codex, Some(now()), None);

    let d0 = ContextDelivery {
        id: new_id(),
        session_id: s.id.clone(),
        workstream_id: ws.id.clone(),
        bundle_id: "bundle-0".into(),
        delivered_revisions: vec![],
        delivered_conflicts: vec![conflict_id.clone()],
        delivered_at: now(),
    };
    db.record_delivery(&d0).unwrap();

    // Conflict resolved
    db.update_conflict_status(&conflict_id, "resolved", Some("已解决"))
        .unwrap();

    // Resume with tiny budget where conflict_resolved is truncated
    let tiny_policy = context::ContextDeliveryPolicy {
        enabled: true,
        new_token_budget: 5,
        resume_token_budget: 5,
        new_extended_limit: 0,
        resume_first_delivery_extended_limit: 0,
        conflict_limit: 0,
    };
    let b_tiny = context::build_bundle_with_policy(
        &db,
        "resume",
        Some(&s),
        Some(ws.id.as_str()),
        tiny_policy,
        "custom",
    )
    .unwrap();
    assert!(!b_tiny
        .sections
        .iter()
        .any(|sec| sec.kind == "conflict_resolved"));

    // Cumulative delivery computed from truncated bundle must KEEP conflict_id
    let d_tiny =
        launcher::compute_cumulative_delivery(&db, &s.id, &ws.id, &b_tiny, Some(&d0), &ws.id)
            .unwrap();
    assert!(
        d_tiny.delivered_conflicts.contains(&conflict_id),
        "Truncated conflict_resolved must NOT remove conflict from delivery snapshot"
    );
    db.record_delivery(&d_tiny).unwrap();

    // Next resume with normal budget: conflict_resolved appears and is delivered
    let b_normal = context::build_bundle(
        &db,
        "resume",
        Some(&s),
        Some(ws.id.as_str()),
        context::ContextDeliveryLevel::Balanced,
    )
    .unwrap();
    assert!(b_normal
        .sections
        .iter()
        .any(|sec| sec.kind == "conflict_resolved"));
}

/// P2: apply_match must record deliveries for a bundle that only has conflicts
/// (even if it has 0 revisions).
#[test]
fn apply_match_handles_conflict_only_bundle() {
    let db = open_db("match-conflict-only");
    let ws = ws_row(&db, "WS");
    let s = session_row(&db, Agent::Codex, Some(now()), None);

    let conflict_id = new_id();

    let intent = LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent: Agent::Codex,
        owner_workstream_id: Some(ws.id.clone()),
        cwd: None,
        context_bundle_markdown: Some("bundle".into()),
        context_bundle_revisions: Some(
            serde_json::json!({
                "bundle_id": "b-test",
                "conflicts_by_workstream": {
                    ws.id.clone(): [conflict_id.clone()]
                }
            })
            .to_string(),
        ),
        process_id: None,
        launched_at: now(),
        matched_session_id: None,
        status: launch_status::PENDING.into(),
        note: String::new(),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_launch_intent(&intent).unwrap();

    launcher::apply_match(&db, &intent, &s, &launcher::LaunchWorkspace::default()).unwrap();

    let deliveries = db.latest_deliveries(&s.id).unwrap();
    let d = deliveries
        .iter()
        .find(|d| d.workstream_id == ws.id)
        .expect("a bundle with only conflicts must still have ContextDelivery recorded");
    assert_eq!(d.delivered_conflicts, vec![conflict_id]);
    assert!(d.delivered_revisions.is_empty());
}

#[test]
fn prepare_new_does_not_create_intent_or_delivery_or_file() {
    let db = open_db("prep-new-no-side-effects");
    // This test is about prepare's side-effect contract, which needs a bundle
    // to exist: opt into delivery explicitly (Off is the shipped default).
    noending::settings::set_context_delivery_level(&db, context::ContextDeliveryLevel::Balanced)
        .unwrap();
    let ws = ws_row(&db, "test ws");
    seed_context(&db, &ws.id, &["约束 A", "约束 B"]);

    let tmp_dir = std::env::temp_dir().join(format!("noending-launcher-{}", new_id()));
    let launcher = launcher::SessionLauncher {
        runtime_dir: tmp_dir.clone(),
    };

    let prepared = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            Some(ws.id.as_str()),
            None,
            &LaunchWorkspace::default(),
        )
        .unwrap();

    // 1. PreparedLaunch captures the exact bundle & fingerprint
    assert_eq!(prepared.mode, "new");
    assert_eq!(
        prepared.owner_workstream_id.as_deref(),
        Some(ws.id.as_str())
    );
    assert!(!prepared.state_fingerprint.is_empty());
    assert_eq!(
        prepared.bundle.workstream_id.as_deref(),
        Some(ws.id.as_str())
    );
    assert!(prepared.bundle.sections.iter().any(|s| s.title == "约束 A"));

    // 2. INVARIANT: No LaunchIntent created
    let intents = db.list_launch_intents(&[], 100).unwrap();
    assert!(
        intents.is_empty(),
        "prepare_new must not insert LaunchIntent"
    );

    // 3. INVARIANT: No context file written to disk
    let bundle_dir = tmp_dir.join("context-bundles");
    assert!(
        !bundle_dir.exists(),
        "prepare_new must not write context file"
    );
}

#[test]
fn prepare_resume_uses_current_owner_without_committing_delivery() {
    let db = open_db("prep-resume-no-side-effects");
    // Needs a delivered bundle to assert on: opt into delivery explicitly.
    noending::settings::set_context_delivery_level(&db, context::ContextDeliveryLevel::Balanced)
        .unwrap();
    let ws1 = ws_row(&db, "ws1");
    let ws2 = ws_row(&db, "ws2");
    seed_context(&db, &ws1.id, &["约束 1"]);
    seed_context(&db, &ws2.id, &["约束 2"]);

    let session = session_row(&db, Agent::Codex, Some(now()), None);
    // ws1 is the Session's current Owner
    db.set_session_owner(&session.id, Some(&ws1.id)).unwrap();

    let tmp_dir = std::env::temp_dir().join(format!("noending-launcher-{}", new_id()));
    let launcher = launcher::SessionLauncher {
        runtime_dir: tmp_dir.clone(),
    };

    let prepared = launcher
        .prepare_resume_in(&db, &session.id, &LaunchWorkspace::default())
        .unwrap();

    assert_eq!(prepared.mode, "resume");
    assert_eq!(
        prepared.owner_workstream_id.as_deref(),
        Some(ws1.id.as_str())
    );
    assert_eq!(
        prepared.bundle.workstream_id.as_deref(),
        Some(ws1.id.as_str())
    );
    assert!(prepared.bundle.sections.iter().any(|s| s.title == "约束 1"));
    // ws2 is not the Owner, so its context must not be assembled in
    assert!(!prepared.bundle.sections.iter().any(|s| s.title == "约束 2"));

    // INVARIANT: the Owner is unchanged (prepare only reads it)
    assert_eq!(owner_of(&db, &session.id).as_deref(), Some(ws1.id.as_str()));

    // INVARIANT: No delivery snapshot recorded
    let deliveries = db.latest_deliveries(&session.id).unwrap();
    assert!(deliveries.is_empty());

    // INVARIANT: No context file written
    let bundle_dir = tmp_dir.join("context-bundles");
    assert!(!bundle_dir.exists());
}

#[test]
fn state_fingerprint_stale_detection_on_context_change() {
    let db = open_db("stale-context-detection");
    let ws = ws_row(&db, "test ws");
    seed_context(&db, &ws.id, &["初始约束"]);

    let tmp_dir = std::env::temp_dir().join(format!("noending-launcher-{}", new_id()));
    let launcher = launcher::SessionLauncher {
        runtime_dir: tmp_dir,
    };

    let prepared = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            Some(ws.id.as_str()),
            None,
            &LaunchWorkspace::default(),
        )
        .unwrap();

    // Context changes in the background (e.g. new item added)
    seed_context(&db, &ws.id, &["新插入的约束"]);

    // Attempting to launch with stale prepared launch MUST fail with stale error
    let err = launcher
        .launch_prepared_in(&db, &prepared, &LaunchWorkspace::default())
        .unwrap_err();
    assert!(
        err.to_string().contains("stale"),
        "expected stale error, got: {}",
        err
    );
}

/// Preview-Launch Identity covers the Runtime override intent too: an
/// override edited while the Preview is open must invalidate the preview,
/// never be silently adopted or silently ignored.
#[test]
fn state_fingerprint_stale_detection_on_runtime_override_change() {
    let db = open_db("stale-runtime-detection");
    let ws = ws_row(&db, "test ws");
    seed_context(&db, &ws.id, &["初始约束"]);

    let launcher = launcher::SessionLauncher {
        runtime_dir: std::env::temp_dir(),
    };
    let prepared = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            Some(ws.id.as_str()),
            None,
            &LaunchWorkspace::default(),
        )
        .unwrap();
    assert!(prepared.runtime.is_default());

    let fingerprint_of = |db: &Db| {
        launcher::compute_state_fingerprint(
            db,
            "new",
            None,
            prepared.owner_workstream_id.as_deref(),
            prepared.delivery_level,
            Agent::Codex,
            &LaunchWorkspace::default(),
        )
        .unwrap()
    };
    assert_eq!(fingerprint_of(&db), prepared.state_fingerprint);

    noending::agent_runtime::set_runtime_overrides(
        &db,
        Agent::Codex,
        &noending::agent_runtime::AgentRuntimeOverrides {
            model: Some("gpt-5.6-sol".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_ne!(
        fingerprint_of(&db),
        prepared.state_fingerprint,
        "a runtime override changed after Preview: the fingerprint must change"
    );

    // Overrides are per Agent: another Agent's configuration is not this
    // launch's state.
    noending::agent_runtime::set_runtime_overrides(
        &db,
        Agent::Pi,
        &noending::agent_runtime::AgentRuntimeOverrides {
            model: Some("qwen/qwen3.8-27b".into()),
            provider: Some("lmstudio".into()),
            effort: None,
        },
    )
    .unwrap();
    assert_eq!(
        fingerprint_of(&db),
        launcher::compute_state_fingerprint(
            &db,
            "new",
            None,
            prepared.owner_workstream_id.as_deref(),
            prepared.delivery_level,
            Agent::Codex,
            &LaunchWorkspace::default()
        )
        .unwrap()
    );
}

/// Prepare freezes the stored intent instead of resolving a default, so the
/// argv Launch renders is exactly what Preview described.
#[test]
fn prepared_launch_freezes_the_runtime_override_intent() {
    let db = open_db("prepared-runtime-intent");
    let ws = ws_row(&db, "test ws");
    seed_context(&db, &ws.id, &["约束"]);

    noending::agent_runtime::set_runtime_overrides(
        &db,
        Agent::Codex,
        &noending::agent_runtime::AgentRuntimeOverrides {
            model: Some("gpt-5.6-sol".into()),
            effort: Some("high".into()),
            provider: None,
        },
    )
    .unwrap();

    let launcher = launcher::SessionLauncher {
        runtime_dir: std::env::temp_dir(),
    };
    let prepared = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            Some(ws.id.as_str()),
            None,
            &LaunchWorkspace::default(),
        )
        .unwrap();
    assert_eq!(prepared.runtime.model.as_deref(), Some("gpt-5.6-sol"));
    assert_eq!(prepared.runtime.effort.as_deref(), Some("high"));
    assert_eq!(
        prepared.runtime.intent_summary(),
        "model=gpt-5.6-sol,effort=high"
    );

    // The frozen intent feeds the same conversion New / Resume / exec use.
    let opts = prepared.runtime.exec_options();
    let args = noending::adapters::adapter_for(Agent::Codex)
        .build_new_command(
            &noending::platform::exec_resolver::AgentInstallation {
                agent: Agent::Codex,
                executable_path: "/usr/local/bin/codex".into(),
                version: None,
                source: "test".into(),
                last_verified_at: String::new(),
            },
            &opts,
            None,
            None,
        )
        .unwrap()
        .args;
    assert!(args
        .windows(2)
        .any(|w| w[0] == "-m" && w[1] == "gpt-5.6-sol"));
    assert!(args.iter().any(|a| a == "model_reasoning_effort=\"high\""));

    // And clearing the override later does not retroactively change the
    // already-prepared intent — it just makes the preview stale.
    noending::agent_runtime::set_runtime_overrides(&db, Agent::Codex, &Default::default()).unwrap();
    assert_eq!(prepared.runtime.model.as_deref(), Some("gpt-5.6-sol"));
}

#[test]
fn state_fingerprint_stale_detection_on_delivery_snapshot_change() {
    let db = open_db("stale-delivery-detection");
    let ws = ws_row(&db, "test ws");
    seed_context(&db, &ws.id, &["约束"]);

    let s = session_row(&db, Agent::Codex, Some(now()), None);
    db.set_session_owner(&s.id, Some(&ws.id)).unwrap();

    let tmp_dir = std::env::temp_dir().join(format!("noending-launcher-{}", new_id()));
    let launcher = launcher::SessionLauncher {
        runtime_dir: tmp_dir,
    };

    let prepared = launcher
        .prepare_resume_in(&db, &s.id, &LaunchWorkspace::default())
        .unwrap();

    // Background process advances delivery snapshot
    let delivery = ContextDelivery {
        id: new_id(),
        session_id: s.id.clone(),
        workstream_id: ws.id.clone(),
        bundle_id: "other-bundle".into(),
        delivered_revisions: vec!["some-rev".into()],
        delivered_conflicts: vec![],
        delivered_at: now(),
    };
    db.record_delivery(&delivery).unwrap();

    let err = launcher
        .launch_prepared_in(&db, &prepared, &LaunchWorkspace::default())
        .unwrap_err();
    assert!(
        err.to_string().contains("stale"),
        "expected stale error, got: {}",
        err
    );
}

#[test]
fn state_fingerprint_stale_detection_on_delivery_level_change() {
    let db = open_db("stale-delivery-level");
    // The transition under test is Balanced → Off, so the starting level has to
    // be chosen explicitly now that Off is the default.
    noending::settings::set_context_delivery_level(&db, context::ContextDeliveryLevel::Balanced)
        .unwrap();
    let ws = ws_row(&db, "test ws");
    seed_context(&db, &ws.id, &["约束"]);

    let tmp_dir = std::env::temp_dir().join(format!("noending-launcher-{}", new_id()));
    let launcher = launcher::SessionLauncher {
        runtime_dir: tmp_dir,
    };

    let prepared = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            Some(ws.id.as_str()),
            None,
            &LaunchWorkspace::default(),
        )
        .unwrap();
    assert_eq!(
        prepared.delivery_level,
        context::ContextDeliveryLevel::Balanced
    );

    // Global setting changes to Off
    noending::settings::set_context_delivery_level(&db, context::ContextDeliveryLevel::Off)
        .unwrap();

    let err = launcher
        .launch_prepared_in(&db, &prepared, &LaunchWorkspace::default())
        .unwrap_err();
    assert!(
        err.to_string().contains("stale"),
        "expected stale error on delivery level change, got: {}",
        err
    );
}

#[test]
fn prepared_bundle_identity_preserved_and_deterministic() {
    let db = open_db("bundle-identity");
    let ws = ws_row(&db, "test ws");
    seed_context(&db, &ws.id, &["约束 1", "约束 2"]);

    let tmp_dir = std::env::temp_dir().join(format!("noending-launcher-{}", new_id()));
    let launcher = launcher::SessionLauncher {
        runtime_dir: tmp_dir,
    };

    let p1 = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            Some(ws.id.as_str()),
            None,
            &LaunchWorkspace::default(),
        )
        .unwrap();

    // Fingerprint recomputed against unchanged state is identical
    let current_fp = launcher::compute_state_fingerprint(
        &db,
        "new",
        None,
        p1.owner_workstream_id.as_deref(),
        p1.delivery_level,
        p1.agent,
        &LaunchWorkspace::default(),
    )
    .unwrap();
    assert_eq!(p1.state_fingerprint, current_fp);
    // Nothing was overridden, so the frozen runtime intent is all-default.
    assert!(p1.runtime.is_default());
    assert_eq!(p1.runtime.intent_summary(), "agent-default");

    // Off mode produces empty bundle markdown & sections
    noending::settings::set_context_delivery_level(&db, context::ContextDeliveryLevel::Off)
        .unwrap();
    let p_off = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            Some(ws.id.as_str()),
            None,
            &LaunchWorkspace::default(),
        )
        .unwrap();
    assert_eq!(p_off.delivery_level, context::ContextDeliveryLevel::Off);
    assert!(p_off.bundle.sections.is_empty());
    assert!(p_off.bundle.markdown.is_empty());
    assert_eq!(p_off.bundle.approx_tokens, 0);
}

#[test]
fn prepared_launch_single_use_atomic_consumption() {
    let db = open_db("single-use-prep");
    let ws = ws_row(&db, "test ws");
    let launcher = launcher::SessionLauncher {
        runtime_dir: std::env::temp_dir(),
    };
    let prepared = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            Some(ws.id.as_str()),
            None,
            &LaunchWorkspace::default(),
        )
        .unwrap();

    let map = std::sync::Mutex::new(std::collections::HashMap::new());
    map.lock()
        .unwrap()
        .insert(prepared.id.clone(), prepared.clone());

    // 1. First consume succeeds
    let first = noending::commands::consume_prepared_launch(&map, &prepared.id);
    assert!(first.is_ok());
    assert_eq!(first.unwrap().id, prepared.id);

    // 2. Second consume fails immediately with "已被使用或已过期"
    let second = noending::commands::consume_prepared_launch(&map, &prepared.id);
    assert!(second.is_err());
    let err_msg = second.unwrap_err().to_string();
    assert!(
        err_msg.contains("已过期") || err_msg.contains("已被使用"),
        "expected consumed error, got: {}",
        err_msg
    );
}

#[test]
fn prepared_launch_concurrent_consumption_is_exclusive() {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    let db = open_db("concurrent-prep");
    let ws = ws_row(&db, "test ws");
    let launcher = launcher::SessionLauncher {
        runtime_dir: std::env::temp_dir(),
    };
    let prepared = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            Some(ws.id.as_str()),
            None,
            &LaunchWorkspace::default(),
        )
        .unwrap();

    let map = Arc::new(Mutex::new(HashMap::new()));
    map.lock()
        .unwrap()
        .insert(prepared.id.clone(), prepared.clone());

    let num_threads = 8;
    let mut handles = Vec::new();

    for _ in 0..num_threads {
        let map_clone = Arc::clone(&map);
        let pid = prepared.id.clone();
        handles.push(std::thread::spawn(move || {
            noending::commands::consume_prepared_launch(&map_clone, &pid)
        }));
    }

    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let successes = results.iter().filter(|r| r.is_ok()).count();
    let failures = results.iter().filter(|r| r.is_err()).count();

    // INVARIANT: Exactly one consumer succeeds; all other 7 fail.
    assert_eq!(successes, 1, "exactly 1 thread must succeed in consuming");
    assert_eq!(failures, num_threads - 1, "all other threads must fail");
}

#[test]
fn prepared_launch_lazy_ttl_cleanup() {
    use std::collections::HashMap;
    use std::sync::Mutex;

    let db = open_db("ttl-cleanup-prep");
    let ws = ws_row(&db, "test ws");
    let launcher = launcher::SessionLauncher {
        runtime_dir: std::env::temp_dir(),
    };
    let mut stale_prepared = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            Some(ws.id.as_str()),
            None,
            &LaunchWorkspace::default(),
        )
        .unwrap();
    // Simulate an old timestamp: 35 minutes ago
    let old_ts = chrono::Utc::now() - chrono::Duration::seconds(35 * 60);
    stale_prepared.prepared_at = old_ts.to_rfc3339();

    let fresh_prepared = launcher
        .prepare_new_in(
            &db,
            Agent::Codex,
            Some(ws.id.as_str()),
            None,
            &LaunchWorkspace::default(),
        )
        .unwrap();

    let map = Mutex::new(HashMap::new());
    {
        let mut guard = map.lock().unwrap();
        guard.insert(stale_prepared.id.clone(), stale_prepared.clone());
        guard.insert(fresh_prepared.id.clone(), fresh_prepared.clone());
    }

    // 1. Attempting to consume stale prepared fails because TTL pruned it
    let stale_res = noending::commands::consume_prepared_launch(&map, &stale_prepared.id);
    assert!(stale_res.is_err());

    // 2. Fresh prepared launch is still present and can be consumed
    let fresh_res = noending::commands::consume_prepared_launch(&map, &fresh_prepared.id);
    assert!(fresh_res.is_ok());
    assert_eq!(fresh_res.unwrap().id, fresh_prepared.id);
}
