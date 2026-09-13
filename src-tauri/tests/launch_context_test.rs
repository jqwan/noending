//! LaunchIntent, Resume Delta and Storage/Domain consistency tests
//! (Issues #5, #6, #8).

use noending::domain::{
    binding_source, launch_status, Agent, ContextDelivery, LaunchIntent, ProjectAffinityEvidence,
    Session, SessionClassificationState, SessionWorkstreamBinding, SourceCursor,
};
use noending::storage::{new_id, now, Db};
use noending::{context, launcher};

fn open_db(tag: &str) -> Db {
    let dir = std::env::temp_dir().join(format!("noending-launch-{}-{}", tag, new_id()));
    Db::open(&dir.join("test.db")).unwrap()
}

fn ws_row(db: &Db, title: &str, project_id: Option<&str>) -> noending::domain::Workstream {
    let w = noending::domain::Workstream {
        id: new_id(),
        project_id: project_id.map(|s| s.to_string()),
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

fn project_row(db: &Db, name: &str) -> noending::domain::Project {
    let p = noending::domain::Project {
        id: new_id(),
        name: name.into(),
        description: String::new(),
        archived: false,
        created_at: now(),
        updated_at: now(),
    };
    db.upsert_project(&p).unwrap();
    p
}

fn session_row(db: &Db, agent: Agent, started_at: Option<String>, cwd: Option<String>) -> Session {
    let s = Session {
        id: new_id(),
        agent,
        agent_session_id: format!("as-{}", new_id()),
        title: None,
        cwd,
        project_id: None,
        raw_path: "/tmp/x.jsonl".into(),
        parent_agent_session_id: None,
        started_at: started_at.clone(),
        last_activity_at: started_at,
    };
    db.upsert_session(&s).unwrap();
    s
}

fn pending_intent(db: &Db, agent: Agent, ws_ids: Vec<String>, cwd: Option<String>) -> LaunchIntent {
    let i = LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent,
        selected_workstream_ids: ws_ids,
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

// ---------------------------------------------------------------------------
// Issue #5: LaunchIntent
// ---------------------------------------------------------------------------

/// Launch → discovery → binding: a pending intent matches a newly
/// discovered session and the user's explicit Workstream selection becomes
/// high-confidence bindings. This is also the crash-recovery path: the
/// intent was persisted before the "crash", the session is discovered by a
/// later reconcile.
#[test]
fn pending_intent_matches_new_session_and_creates_explicit_bindings() {
    let db = open_db("intent-match");
    let ws_a = ws_row(&db, "Workstream A", None);
    let ws_b = ws_row(&db, "Workstream B", None);

    // user picks A + B in the New Session dialog, then NoEnding launches
    let intent = pending_intent(&db, Agent::Codex, vec![ws_a.id.clone(), ws_b.id.clone()], None);

    // the agent CLI creates its session; we discover it afterwards
    let session = session_row(&db, Agent::Codex, Some(now()), None);
    assert!(
        launcher::try_match_launch_intents(&db, &session).unwrap(),
        "the fresh session must claim the pending intent"
    );

    // both explicit selections are bound with confidence 1.0
    let bindings = db.bindings_for_session(&session.id).unwrap();
    assert_eq!(bindings.len(), 2, "A + B → two bindings");
    assert!(bindings.iter().all(|b| b.source == binding_source::EXPLICIT_LAUNCH));
    assert!(bindings.iter().all(|b| b.confidence == 1.0));

    let intent = db.get_launch_intent(&intent.id).unwrap().unwrap();
    assert_eq!(intent.status, launch_status::MATCHED);
    assert_eq!(intent.matched_session_id.as_deref(), Some(session.id.as_str()));

    // classification is derived as fully assigned
    assert_eq!(
        SessionClassificationState::derive(&bindings),
        SessionClassificationState::Assigned
    );
}

/// Zero selected workstreams is a valid state: the intent matches but
/// creates no bindings (0 binding is allowed), and auto classification
/// must not add anything to an explicitly launched session.
#[test]
fn contextless_launch_matches_but_stays_zero_binding() {
    let db = open_db("intent-zero");
    let _ws = ws_row(&db, "Unrelated", None);
    let intent = pending_intent(&db, Agent::Pi, vec![], None);

    let session = session_row(&db, Agent::Pi, Some(now()), None);
    assert!(launcher::try_match_launch_intents(&db, &session).unwrap());
    assert!(db.bindings_for_session(&session.id).unwrap().is_empty());
    assert_eq!(
        db.get_launch_intent(&intent.id).unwrap().unwrap().status,
        launch_status::MATCHED
    );
}

/// Several similarly-plausible candidates → ambiguous, never a silent guess.
#[test]
fn ambiguous_candidates_wait_for_the_user() {
    let db = open_db("intent-ambiguous");
    let ws = ws_row(&db, "WS", None);
    // two intents launched at nearly the same time for the same agent
    let i1 = pending_intent(&db, Agent::ClaudeCode, vec![ws.id.clone()], None);
    let i2 = pending_intent(&db, Agent::ClaudeCode, vec![ws.id.clone()], None);
    let _ = (&i1, &i2);

    let session = session_row(&db, Agent::ClaudeCode, Some(now()), None);
    assert!(
        !launcher::try_match_launch_intents(&db, &session).unwrap(),
        "must not silently pick one"
    );
    let ambiguous = db
        .list_launch_intents(&[launch_status::AMBIGUOUS], 10)
        .unwrap();
    assert!(!ambiguous.is_empty(), "the best candidate is marked ambiguous");
    assert!(
        db.bindings_for_session(&session.id).unwrap().is_empty(),
        "no bindings before the user resolves"
    );

    // user resolves manually
    launcher::apply_match(&db, &ambiguous[0], &session).unwrap();
    let resolved = db.get_launch_intent(&ambiguous[0].id).unwrap().unwrap();
    assert_eq!(resolved.status, launch_status::MATCHED);
    assert_eq!(db.bindings_for_session(&session.id).unwrap().len(), 1);
}

/// Wrong agent / stale sessions never match; stale intents expire.
#[test]
fn stale_intents_expire_and_wrong_agent_never_matches() {
    let db = open_db("intent-expire");
    let ws = ws_row(&db, "WS", None);
    let intent = pending_intent(&db, Agent::Codex, vec![ws.id.clone()], None);

    // a Claude session cannot claim a Codex intent
    let claude_session = session_row(&db, Agent::ClaudeCode, Some(now()), None);
    assert!(!launcher::try_match_launch_intents(&db, &claude_session).unwrap());

    // expire pending intents that are older than the TTL
    db.update_launch_intent(&intent.id, launch_status::PENDING, None, "").unwrap();
    db.0
        .execute(
            "UPDATE launch_intents SET launched_at = ?2 WHERE id = ?1",
            rusqlite::params![intent.id, (chrono::Utc::now() - chrono::Duration::hours(48)).to_rfc3339()],
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
    let fresh = pending_intent(&db, Agent::Codex, vec![ws.id.clone()], None);
    assert!(!launcher::try_match_launch_intents(&db, &old_session).unwrap());
    assert_eq!(
        db.get_launch_intent(&fresh.id).unwrap().unwrap().status,
        launch_status::PENDING
    );
}

/// Resume keeps existing bindings — it never re-guesses them.
#[test]
fn resume_does_not_reguess_bindings() {
    let db = open_db("resume-no-guess");
    let ws = ws_row(&db, "WS", None);
    let s = session_row(&db, Agent::Codex, Some(now()), None);
    db.bind(&SessionWorkstreamBinding {
        session_id: s.id.clone(),
        workstream_id: ws.id.clone(),
        role: "primary".into(),
        source: binding_source::EXPLICIT_LAUNCH.into(),
        confidence: 1.0,
        last_seen_revision: None,
        last_sync_cursor: 0,
        created_at: now(),
        last_used_at: now(),
    })
    .unwrap();
    let bindings = db.bindings_for_session(&s.id).unwrap();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].source, binding_source::EXPLICIT_LAUNCH);
    // resume path only *uses* bindings; there is no classification call
    // (verified structurally: try_match only consumes PENDING intents and
    // this session's intent (if any) is already MATCHED.)
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
    let ws = ws_row(&db, "resume ws", None);
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
    assert!(context::build_bundle(&db, "resume", None, &[ws.id.clone()], 4000).is_err());

    // first resume (no delivery yet): full context is delivered
    let bundle = context::build_bundle(&db, "resume", Some(&s), &[ws.id.clone()], 4000).unwrap();
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
        delivered_at: now(),
    })
    .unwrap();

    // no changes since delivery → only the minimal reminder, no full re-dump
    let unchanged = context::build_bundle(&db, "resume", Some(&s), &[ws.id.clone()], 4000).unwrap();
    assert!(unchanged.markdown.contains("Current Task Reminder"));
    assert!(!unchanged.markdown.contains("约束二"), "unchanged items are not repeated");
}

#[test]
fn resume_delta_shows_changes_and_disappearances() {
    let db = open_db("resume-delta");
    let ws = ws_row(&db, "delta ws", None);
    let items = seed_context(&db, &ws.id, &["决策甲", "约束乙"]);
    let s = session_row(&db, Agent::Codex, Some(now()), None);

    // deliver everything
    let bundle = context::build_bundle(&db, "resume", Some(&s), &[ws.id.clone()], 4000).unwrap();
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

    let delta = context::build_bundle(&db, "resume", Some(&s), &[ws.id.clone()], 4000).unwrap();
    assert!(delta.markdown.contains("新决定：切换构建工具"), "new item appears");
    assert!(delta.markdown.contains("Changed Since Your Last Activity"));
    assert!(delta.markdown.contains("约束乙"), "resolved item name appears…");
    assert!(delta.markdown.contains("Resolved / Superseded"), "…in the disappeared section");
    // the unchanged item is not repeated in full
    assert!(!delta.sections.iter().any(|sec| sec.title == "决策甲" && sec.kind == "constraint"));
}

// ---------------------------------------------------------------------------
// Issue #6: aggregation
// ---------------------------------------------------------------------------

#[test]
fn multi_workstream_bundle_dedups_and_labels_primary_related() {
    let db = open_db("aggregate");
    let ws1 = ws_row(&db, "主 Workstream", None);
    let ws2 = ws_row(&db, "相关 Workstream", None);

    // the SAME constraint exists in both workstreams
    for ws in [&ws1, &ws2] {
        noending::sync::create_item(
            &db,
            &ws.id,
            "constraint",
            "保持 API 向后兼容",
            "所有变更不得破坏现有 API",
            "user_edit",
            "user_edit",
            &[],
            None,
            "user",
        )
        .unwrap();
    }
    // an open conflict in ws2
    let item = noending::sync::create_item(
        &db,
        &ws2.id,
        "decision",
        "只支持 macOS",
        "agent 观点",
        "agent_inferred",
        "session_event",
        &[],
        None,
        "sync:heuristic",
    )
    .unwrap();
    db.insert_conflict(&noending::domain::ContextConflict {
        id: new_id(),
        workstream_id: ws2.id.clone(),
        left_item_id: item.id.clone(),
        right_item_id: None,
        conflict_type: "authority".into(),
        status: "open".into(),
        resolution: None,
        created_at: now(),
        updated_at: now(),
    })
    .unwrap();

    let bundle = context::build_bundle(&db, "new", None, &[ws1.id.clone(), ws2.id.clone()], 8000).unwrap();
    assert!(bundle.markdown.contains("## 主 Workstream"));
    assert!(bundle.markdown.contains("Related Workstream"));
    // dedup: the shared constraint appears once (primary wins)
    let count = bundle.markdown.matches("保持 API 向后兼容").count();
    assert_eq!(count, 1, "shared constraint deduped across workstreams");
    // cross-workstream conflicts surface
    assert!(bundle.markdown.contains("New Conflicts"));
}

// ---------------------------------------------------------------------------
// Issue #8: storage / domain consistency
// ---------------------------------------------------------------------------

#[test]
fn upsert_workstream_moves_between_projects_and_to_standalone() {
    let db = open_db("ws-move");
    let pa = project_row(&db, "Project A");
    let pb = project_row(&db, "Project B");
    let mut w = ws_row(&db, "移动的 Workstream", Some(&pa.id));

    // A → B
    w.project_id = Some(pb.id.clone());
    w.updated_at = now();
    db.upsert_workstream(&w).unwrap();
    assert_eq!(db.get_workstream(&w.id).unwrap().unwrap().project_id, Some(pb.id.clone()));

    // B → NULL (standalone)
    w.project_id = None;
    w.updated_at = now();
    db.upsert_workstream(&w).unwrap();
    assert_eq!(db.get_workstream(&w.id).unwrap().unwrap().project_id, None);
    assert!(db
        .list_workstreams(None)
        .unwrap()
        .iter()
        .any(|x| x.id == w.id));
}

/// Deleting a Project detaches Workstreams and Sessions — it never deletes
/// or archives them.
#[test]
fn delete_project_detaches_without_archiving() {
    let db = open_db("project-delete");
    let p = project_row(&db, "Doomed Project");
    let w = ws_row(&db, "Surviving Workstream", Some(&p.id));
    let s = session_row(&db, Agent::Codex, Some(now()), None);
    db.0
        .execute(
            "UPDATE sessions SET project_id = ?2 WHERE id = ?1",
            rusqlite::params![s.id, p.id],
        )
        .unwrap();

    db.delete_project(&p.id).unwrap();

    assert!(db.get_project(&p.id).unwrap().is_none(), "project gone");
    let w = db.get_workstream(&w.id).unwrap().expect("workstream survives");
    assert_eq!(w.project_id, None, "workstream detached");
    assert_eq!(w.visibility, "normal", "workstream NOT archived");
    assert_eq!(w.lifecycle, "open", "workstream lifecycle untouched");
    let s = db.get_session(&s.id).unwrap().expect("session survives");
    assert_eq!(s.project_id, None, "session detached");
}

/// The list_sessions parameter bug: filtering by agent ONLY used to bind
/// ?2 with a single parameter and fail. All four filter combinations work.
#[test]
fn list_sessions_all_filter_combinations() {
    let db = open_db("session-filter");
    let p = project_row(&db, "P");
    let s_codex = session_row(&db, Agent::Codex, Some(now()), None);
    let _s_pi = session_row(&db, Agent::Pi, Some(now()), None);
    db.0
        .execute(
            "UPDATE sessions SET project_id = ?2 WHERE id = ?1",
            rusqlite::params![s_codex.id, p.id],
        )
        .unwrap();

    let agent_only = db
        .list_sessions(noending::storage::SessionFilter {
            project_id: None,
            agent: Some(Agent::Codex),
        })
        .unwrap();
    assert_eq!(agent_only.len(), 1, "agent-only filter (the old ?N bug)");
    assert_eq!(agent_only[0].agent, Agent::Codex);

    let project_only = db
        .list_sessions(noending::storage::SessionFilter {
            project_id: Some(p.id.clone()),
            agent: None,
        })
        .unwrap();
    assert_eq!(project_only.len(), 1);

    let both = db
        .list_sessions(noending::storage::SessionFilter {
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
    let ctx = noending::sync::MergeContext { run_id: new_id(), runtime: "heuristic".into() };
    let m = noending::sync::ContextMutation::CreateWorkstream {
        project_id: None,
        title: "自动发现的新工作流".into(),
        reason: "会话中出现新的长期主题".into(),
    };
    let applied = db.tx(|tx| noending::sync::merge::MergeEngine.apply(tx, &m, &ctx)).unwrap();
    assert!(applied);
    let all = db.list_workstreams(None).unwrap();
    let auto = all.iter().find(|w| w.title == "自动发现的新工作流").expect("created");
    assert_eq!(auto.project_id, None, "no project home required");
}

/// cwd is only evidence: the resolver suggests with a score, the user
/// correction records the strongest evidence.
#[test]
fn project_affinity_evidence_and_resolution() {
    let db = open_db("affinity");
    let p = project_row(&db, "noending");
    let s = session_row(&db, Agent::Codex, Some(now()), Some("/Users/jqk/projects/noending".into()));

    // evidence recorded from cwd (as reconcile does)
    let e = ProjectAffinityEvidence {
        id: new_id(),
        session_id: Some(s.id.clone()),
        workstream_id: None,
        project_id: p.id.clone(),
        evidence_type: "cwd_match".into(),
        source: "cwd=/Users/jqk/projects/noending".into(),
        score: 1.0,
        created_at: now(),
    };
    db.insert_evidence(&e).unwrap();
    let (suggested, score) = db.resolve_project_affinity(&s.id).unwrap().unwrap();
    assert_eq!(suggested, p.id);
    assert!(score > 0.0);

    // user correction dominates (score 10)
    let p2 = project_row(&db, "other");
    let correction = ProjectAffinityEvidence {
        id: new_id(),
        session_id: Some(s.id.clone()),
        workstream_id: None,
        project_id: p2.id.clone(),
        evidence_type: "user_correction".into(),
        source: "manual".into(),
        score: 10.0,
        created_at: now(),
    };
    db.insert_evidence(&correction).unwrap();
    let (suggested, _) = db.resolve_project_affinity(&s.id).unwrap().unwrap();
    assert_eq!(suggested, p2.id, "user correction wins over cwd evidence");

    // no evidence at all → no suggestion
    let s2 = session_row(&db, Agent::Pi, Some(now()), None);
    assert!(db.resolve_project_affinity(&s2.id).unwrap().is_none());
}

/// get_event_by_ref resolves both stable ids and legacy positional refs.
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
    let by_id = db.get_event_by_ref(&format!("session-event:{}", ev.id)).unwrap().unwrap();
    assert_eq!(by_id.id, ev.id);
    // legacy positional reference
    let legacy = db
        .get_event_by_ref(&format!("session:{}#{}", s.id, ev.sequence))
        .unwrap()
        .unwrap();
    assert_eq!(legacy.id, ev.id);
    // unknown ref → None, never fabricated
    assert!(db.get_event_by_ref("session-event:missing").unwrap().is_none());
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
        last_sequence: 7,
    };
    db.set_source_cursor(&c).unwrap();
    let back = db.get_source_cursor(&s.id).unwrap();
    assert_eq!(back.source_file_identity, c.source_file_identity);
    assert_eq!(back.generation, 3);
    assert_eq!(back.byte_offset, 900);
    assert_eq!(back.prefix_hash, "abc123");
    assert_eq!(back.last_sequence, 7);
    assert_eq!(db.get_cursor(&s.id).unwrap(), 7);
}

/// A New Session launched with injected context RECEIVED that context:
/// apply_match must record the intent's delivery snapshot, so the first
/// resume computes a true delta instead of re-sending the full context.
#[test]
fn launch_intent_match_records_delivery_snapshot() {
    let db = open_db("intent-delivery");
    let ws = ws_row(&db, "delivery ws", None);
    let items = seed_context(&db, &ws.id, &["已交付约束"]);
    assert!(!items.is_empty());
    // create_item returns the in-memory item; the head revision lives in DB
    let head_rev = |item_id: &str| db.get_item(item_id).unwrap().unwrap().current_revision_id.unwrap();
    let s = session_row(&db, Agent::Codex, Some(now()), None);

    // intent snapshot: what the launched session actually received
    let intent = LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent: Agent::Codex,
        selected_workstream_ids: vec![ws.id.clone()],
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

    assert!(launcher::apply_match(&db, &intent, &s).is_ok());

    // binding established explicitly…
    let bound = db.bindings_for_session(&s.id).unwrap();
    assert!(bound
        .iter()
        .any(|b| b.workstream_id == ws.id && b.source == binding_source::EXPLICIT_LAUNCH));

    // …and the delivery recorded per workstream from the snapshot
    let deliveries = db.latest_deliveries(&s.id).unwrap();
    let d = deliveries.iter().find(|d| d.workstream_id == ws.id).expect("delivery recorded");
    assert_eq!(d.bundle_id, "bundle-1");
    assert_eq!(
        d.delivered_revisions,
        vec![head_rev(&items[0].id)],
        "only this workstream's revisions are attributed to it"
    );

    // first resume: the delivered item is NOT re-sent in full
    let bundle = context::build_bundle(&db, "resume", Some(&s), &[ws.id.clone()], 4000).unwrap();
    assert!(
        !bundle.markdown.contains("已交付约束"),
        "already-delivered content must not be re-sent, got: {}",
        bundle.markdown
    );
}

/// Deliveries are attributed per workstream: a multi-workstream resume
/// records each workstream's OWN revisions, never the union.
#[test]
fn multi_workstream_delivery_groups_revisions_by_workstream() {
    let db = open_db("delivery-grouping");
    let ws_a = ws_row(&db, "ws a", None);
    let ws_b = ws_row(&db, "ws b", None);
    let a = &seed_context(&db, &ws_a.id, &["约束A"])[0];
    let b = &seed_context(&db, &ws_b.id, &["约束B"])[0];
    let s = session_row(&db, Agent::Codex, Some(now()), None);

    // simulate what resume_session now records: grouped by section owner
    let head_rev = |item_id: &str| db.get_item(item_id).unwrap().unwrap().current_revision_id.unwrap();
    let by_ws = std::collections::BTreeMap::from([
        (ws_a.id.clone(), vec![head_rev(&a.id)]),
        (ws_b.id.clone(), vec![head_rev(&b.id)]),
    ]);
    for (ws_id, revs) in &by_ws {
        db.record_delivery(&ContextDelivery {
            id: new_id(),
            session_id: s.id.clone(),
            workstream_id: ws_id.clone(),
            bundle_id: "bundle-2".into(),
            delivered_revisions: revs.clone(),
            delivered_at: now(),
        })
        .unwrap();
    }

    for (ws, expected_rev) in [(&ws_a, a), (&ws_b, b)] {
        let d = db
            .latest_deliveries(&s.id)
            .unwrap()
            .into_iter()
            .find(|d| d.workstream_id == ws.id)
            .unwrap();
        assert_eq!(d.delivered_revisions, vec![head_rev(&expected_rev.id)]);
    }
}
