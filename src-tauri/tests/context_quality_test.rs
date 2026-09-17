//! Context Quality Evaluation Harness & Golden Fixtures (Context Quality v0.1).
//!
//! Evaluates the end-to-end extraction and domain evolution pipeline:
//!   Session transcript events
//!          ↓
//!   parse_mutations (authority derivation, ref mapping, schema validation)
//!          ↓
//!   MergeEngine (AuthorityPolicy, supersede, update, resolve, conflict)
//!          ↓
//!   authoritative domain state (resolve_core_context, active conflicts, status)
//!
//! Measures quality across 5 dimensions:
//!   1. Extraction precision (conversational filler ignored)
//!   2. Extraction recall (critical facts captured)
//!   3. Evolution correctness (update vs supersede vs resolve)
//!   4. Workstream classification (proper routing in multi-workstream environments)
//!   5. Authority & conflict quality (user constraints protected from agent overwrite)

use std::collections::HashMap;

use noending::domain::{
    Agent, ContextItem, ContextItemRevision, Session, SessionEvent, Workstream,
};
use noending::storage::{new_id, now, Db};
use noending::sync::extractor::{parse_mutations, PromptEventRef};
use noending::sync::merge::MergeEngine;
use noending::sync::{ContextMutation, MergeContext};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct FixtureWorkstream {
    id: String,
    title: String,
    description: String,
}

#[derive(Debug, Deserialize)]
struct FixtureInitialItem {
    id: String,
    workstream_id: String,
    kind: String,
    title: String,
    content: String,
    authority: String,
    status: String,
}

#[derive(Debug, Deserialize)]
struct FixtureSession {
    agent: String,
    title: String,
}

#[derive(Debug, Deserialize)]
struct FixtureEvent {
    sequence: i64,
    kind: String,
    text: String,
}

#[derive(Debug, Deserialize)]
struct ExpectedMutation {
    op: String,
    #[serde(default)]
    workstream_id: Option<String>,
    #[serde(default)]
    item_kind: Option<String>,
    #[serde(default)]
    item_id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    authority: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExpectedCoreItem {
    kind: String,
    title: String,
}

#[derive(Debug, Deserialize)]
struct FixtureExpected {
    mutations: Vec<ExpectedMutation>,
    effective_core: HashMap<String, Vec<ExpectedCoreItem>>,
    conflicts_count: usize,
}

#[derive(Debug, Deserialize)]
struct ContextQualityFixture {
    name: String,
    #[allow(dead_code)]
    description: String,
    #[allow(dead_code)]
    dimension: String,
    workstreams: Vec<FixtureWorkstream>,
    initial_context: Vec<FixtureInitialItem>,
    session: FixtureSession,
    events: Vec<FixtureEvent>,
    mock_model_output: String,
    expected: FixtureExpected,
}

fn open_test_db(tag: &str) -> Db {
    let dir = std::env::temp_dir().join(format!("noending-eval-{}-{}", tag, new_id()));
    Db::open(&dir.join("test.db")).expect("open eval db")
}

fn run_quality_fixture(fixture_json: &str) -> Db {
    let fixture: ContextQualityFixture =
        serde_json::from_str(fixture_json).expect("valid fixture JSON");
    let db = open_test_db(&fixture.name);

    // 1. Insert candidate workstreams
    for ws in &fixture.workstreams {
        let w = Workstream {
            id: ws.id.clone(),
            project_id: None,
            title: ws.title.clone(),
            description: ws.description.clone(),
            lifecycle: "open".into(),
            visibility: "normal".into(),
            default_cwd: None,
            created_at: now(),
            updated_at: now(),
        };
        db.upsert_workstream(&w).expect("upsert workstream");
    }

    // 2. Insert initial context items
    for it in &fixture.initial_context {
        db.tx(|tx| {
            let item = ContextItem {
                id: it.id.clone(),
                workstream_id: it.workstream_id.clone(),
                kind: it.kind.clone(),
                status: it.status.clone(),
                authority: it.authority.clone(),
                created_by: "setup".into(),
                current_revision_id: None,
                supersedes_item_id: None,
                created_at: now(),
                updated_at: now(),
            };
            let rev_id = format!("rev-init-{}", it.id);
            let rev = ContextItemRevision {
                id: rev_id.clone(),
                item_id: it.id.clone(),
                title: it.title.clone(),
                content: it.content.clone(),
                metadata: serde_json::json!({}),
                source_type: Some("user_edit".into()),
                source_ref: None,
                sync_run_id: None,
                created_at: now(),
            };
            noending::storage::insert_item_conn(tx, &item, &rev)?;
            Ok(())
        })
        .expect("insert initial context item");
    }

    // 3. Create session and build prompt events + ref map
    let session = Session {
        id: format!("sess-{}", fixture.name),
        agent: match fixture.session.agent.to_lowercase().as_str() {
            "codex" => Agent::Codex,
            "claude_code" | "claude" => Agent::ClaudeCode,
            "pi" => Agent::Pi,
            _ => Agent::Codex,
        },
        agent_session_id: format!("as-{}", fixture.name),
        title: Some(fixture.session.title.clone()),
        cwd: None,
        project_id: None,
        raw_path: format!("/tmp/{}.jsonl", fixture.name),
        parent_agent_session_id: None,
        started_at: Some(now()),
        last_activity_at: Some(now()),
    };
    db.upsert_session(&session).expect("upsert session");

    let mut session_events = Vec::new();
    let mut ref_map = Vec::new();
    for fe in &fixture.events {
        let ev_id = format!("ev-{}-{}", fixture.name, fe.sequence);
        let short_ref = format!("#{}", fe.sequence);
        session_events.push(SessionEvent {
            id: ev_id.clone(),
            session_id: session.id.clone(),
            sequence: fe.sequence,
            source_event_id: None,
            source_generation: 0,
            source_position: format!("line:{}", fe.sequence),
            ts: None,
            kind: fe.kind.clone(),
            text: Some(fe.text.clone()),
            raw_ref: format!("test#line:{}", fe.sequence),
            metadata: serde_json::json!({}),
        });
        ref_map.push(PromptEventRef {
            short_ref,
            event_id: ev_id,
            sequence: fe.sequence,
            kind: fe.kind.clone(),
        });
    }

    // 4. Run extractor parse_mutations
    let candidate_ids: Vec<String> = fixture.workstreams.iter().map(|w| w.id.clone()).collect();
    let parse_output = parse_mutations(
        &fixture.mock_model_output,
        &ref_map,
        &candidate_ids,
        &session,
    )
    .expect("parse mutations must succeed");

    let mutations = parse_output.mutations;

    // 5. Assert mutations match expected
    assert_eq!(
        mutations.len(),
        fixture.expected.mutations.len(),
        "Expected {} mutations, got {}",
        fixture.expected.mutations.len(),
        mutations.len()
    );

    for (actual, expected) in mutations.iter().zip(fixture.expected.mutations.iter()) {
        match (actual, expected.op.as_str()) {
            (
                ContextMutation::Add {
                    workstream_id,
                    item_kind,
                    title,
                    authority,
                    ..
                },
                "add",
            ) => {
                if let Some(exp_ws) = &expected.workstream_id {
                    assert_eq!(workstream_id, exp_ws);
                }
                if let Some(exp_kind) = &expected.item_kind {
                    assert_eq!(item_kind, exp_kind);
                }
                if let Some(exp_title) = &expected.title {
                    assert_eq!(title, exp_title);
                }
                if let Some(exp_auth) = &expected.authority {
                    assert_eq!(authority, exp_auth);
                }
            }
            (
                ContextMutation::Update {
                    item_id,
                    title,
                    authority,
                    ..
                },
                "update",
            ) => {
                if let Some(exp_id) = &expected.item_id {
                    assert_eq!(item_id, exp_id);
                }
                if let Some(exp_title) = &expected.title {
                    assert_eq!(title, exp_title);
                }
                if let Some(exp_auth) = &expected.authority {
                    assert_eq!(authority, exp_auth);
                }
            }
            (
                ContextMutation::Supersede {
                    item_id,
                    title,
                    authority,
                    ..
                },
                "supersede",
            ) => {
                if let Some(exp_id) = &expected.item_id {
                    assert_eq!(item_id, exp_id);
                }
                if let Some(exp_title) = &expected.title {
                    assert_eq!(title, exp_title);
                }
                if let Some(exp_auth) = &expected.authority {
                    assert_eq!(authority, exp_auth);
                }
            }
            (ContextMutation::Resolve { item_id, .. }, "resolve") => {
                if let Some(exp_id) = &expected.item_id {
                    assert_eq!(item_id, exp_id);
                }
            }
            _ => panic!(
                "Mutation op mismatch: actual {:?}, expected op {}",
                actual, expected.op
            ),
        }
    }

    // 6. Commit mutations via MergeEngine within a transaction
    let merger = MergeEngine;
    let merge_ctx = MergeContext {
        run_id: format!("eval-run-{}", fixture.name),
        runtime: "eval-harness".into(),
    };

    db.tx(|tx| {
        for m in &mutations {
            merger.apply(tx, m, &merge_ctx)?;
        }
        Ok(())
    })
    .expect("merger apply must commit cleanly");

    // 7. Assert authoritative domain state (resolve_core_context)
    for (ws_id, expected_items) in &fixture.expected.effective_core {
        let core_sections =
            noending::context::resolve_core_context(&db, ws_id).expect("resolve_core_context");
        let actual_items: Vec<_> = core_sections.iter().map(|s| (&s.kind, &s.title)).collect();

        assert_eq!(
            actual_items.len(),
            expected_items.len(),
            "Effective core items count mismatch for ws {}. Actual: {:?}, Expected: {:?}",
            ws_id,
            actual_items,
            expected_items
        );

        for (actual, expected) in actual_items.iter().zip(expected_items.iter()) {
            assert_eq!(*actual.0, expected.kind, "Kind mismatch in ws {}", ws_id);
            assert_eq!(*actual.1, expected.title, "Title mismatch in ws {}", ws_id);
        }
    }

    // 8. Assert conflicts count
    let mut total_open_conflicts = 0;
    for ws in &fixture.workstreams {
        let conflicts = db
            .conflicts_for_workstream(&ws.id, false)
            .expect("conflicts_for_workstream");
        total_open_conflicts += conflicts.len();
    }
    assert_eq!(
        total_open_conflicts, fixture.expected.conflicts_count,
        "Open conflicts count mismatch for fixture {}",
        fixture.name
    );

    db
}

#[test]
fn fixture_goal_evolution() {
    let json = include_str!("fixtures/context_quality/goal_evolution.json");
    let db = run_quality_fixture(json);

    let item = db.get_item("goal-initial").unwrap().unwrap();
    assert_eq!(item.status, "active");
    let rev_id = item.current_revision_id.expect("has revision");
    let rev = db.get_revision(&rev_id).unwrap().unwrap();
    assert!(rev.title.contains("完善生产级认证系统"));
}

#[test]
fn fixture_decision_supersede() {
    let json = include_str!("fixtures/context_quality/decision_supersede.json");
    let db = run_quality_fixture(json);

    let old = db.get_item("dec-websocket").unwrap().unwrap();
    assert_eq!(old.status, "superseded");

    let active_items = db.items_for_workstream("ws-realtime", false).unwrap();
    assert_eq!(active_items.len(), 1);
    assert_eq!(
        active_items[0].0.supersedes_item_id.as_deref(),
        Some("dec-websocket"),
        "Supersedes lineage pointer must be preserved"
    );
}

#[test]
fn fixture_constraint_conflict() {
    let json = include_str!("fixtures/context_quality/constraint_conflict.json");
    let db = run_quality_fixture(json);

    let conflicts = db.conflicts_for_workstream("ws-storage", false).unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].left_item_id, "const-migration");
    assert!(conflicts[0].candidate_snapshot_json.is_some());

    let item = db.get_item("const-migration").unwrap().unwrap();
    assert_eq!(item.authority, "user_explicit");
    assert_eq!(item.status, "active");
}

#[test]
fn fixture_todo_resolution() {
    let json = include_str!("fixtures/context_quality/todo_resolution.json");
    let db = run_quality_fixture(json);

    let item = db.get_item("todo-truncate").unwrap().unwrap();
    assert_eq!(item.status, "resolved");

    let active = db.items_for_workstream("ws-ingestion", false).unwrap();
    assert_eq!(
        active.len(),
        0,
        "Resolved item must not appear in active items query"
    );

    let all = db.items_for_workstream("ws-ingestion", true).unwrap();
    assert_eq!(
        all.len(),
        1,
        "Resolved item must still be present when include_resolved=true"
    );
}

#[test]
fn fixture_multi_workstream_classification() {
    let json = include_str!("fixtures/context_quality/multi_workstream_classification.json");
    let db = run_quality_fixture(json);

    let launcher_items = db.items_for_workstream("ws-launcher", false).unwrap();
    assert_eq!(launcher_items.len(), 1);

    let sync_items = db.items_for_workstream("ws-sync", false).unwrap();
    assert_eq!(
        sync_items.len(),
        0,
        "Non-target workstream must remain clean"
    );
}
