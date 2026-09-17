//! Context Quality Evaluation Harness — two layers over one shared corpus.
//!
//! The corpus (`tests/fixtures/context_quality/*.json`) serves both layers;
//! domain correctness and extraction intelligence are measured separately.
//!
//! Layer A — Domain Golden (deterministic, every CI run):
//!   recorded_model_output
//!          ↓ parse_mutations (ref validation, authority derivation, schema checks)
//!          ↓ MergeEngine (AuthorityPolicy, supersede, conflict)
//!          ↓ exact authoritative domain state (`gold`)
//!   It proves the domain is correct *given a legal extractor output*. It says
//!   nothing about whether a real extractor would find the right facts.
//!
//! Layer B — Extraction Eval (a real extractor over the same corpus):
//!   input.events + candidate workstreams + existing items
//!          ↓ ContextExtractor::extract (HeuristicExtractor today, Cli later)
//!          ↓ same MergeEngine
//!          ↓ structural gold (`extractor_gold`: required/forbidden mutations,
//!            required/forbidden effective context, conflict count)
//!   It measures extraction quality — recall, precision (chatter), workstream
//!   routing, evolution choice, conflict correctness — without pinning exact
//!   wording, so paraphrases do not create false failures.
//!
//! Fixtures with `heuristic_expected_failure` document the known heuristic
//! baseline: Layer B must FAIL while that marker stands. When an extractor
//! fix makes such a case pass, the test fails with an "unexpected pass"
//! message and the marker must be removed in the same commit — the recorded
//! baseline never goes stale silently.

use std::collections::HashMap;

use noending::domain::{
    Agent, ContextItem, ContextItemRevision, Session, SessionEvent, Workstream,
};
use noending::storage::{new_id, now, Db};
use noending::sync::extractor::{
    collect_prompt_inputs, parse_mutations, HeuristicExtractor, PromptEventRef,
};
use noending::sync::merge::MergeEngine;
use noending::sync::{ContextExtractor, ContextMutation, MergeContext};
use serde::Deserialize;

const FIXTURE_GOAL_EVOLUTION: &str = include_str!("fixtures/context_quality/goal_evolution.json");
const FIXTURE_DECISION_SUPERSEDE: &str =
    include_str!("fixtures/context_quality/decision_supersede.json");
const FIXTURE_TODO_RESOLUTION: &str = include_str!("fixtures/context_quality/todo_resolution.json");
const FIXTURE_CONSTRAINT_CONFLICT: &str =
    include_str!("fixtures/context_quality/constraint_conflict.json");
const FIXTURE_MULTI_WS: &str =
    include_str!("fixtures/context_quality/multi_workstream_classification.json");
const FIXTURE_MULTI_WS_REVERSE: &str =
    include_str!("fixtures/context_quality/workstream_routing_reverse.json");

// ---------------------------------------------------------------------------
// Fixture schema
// ---------------------------------------------------------------------------

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
struct FixtureInput {
    workstreams: Vec<FixtureWorkstream>,
    #[serde(default)]
    initial_context: Vec<FixtureInitialItem>,
    session: FixtureSession,
    events: Vec<FixtureEvent>,
}

/// Layer A gold: exact expectations against the recorded model output.
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
struct FixtureGold {
    mutations: Vec<ExpectedMutation>,
    #[serde(default)]
    effective_context: HashMap<String, Vec<ExpectedCoreItem>>,
    #[serde(default)]
    conflicts_count: usize,
}

/// Layer B gold: structural expectations for a real extractor. Fields left
/// out are unconstrained; wording is never pinned exactly.
#[derive(Debug, Deserialize, Default)]
struct StructuralMutationExpectation {
    /// Any-of: the mutation's op must be one of these.
    #[serde(default)]
    op: Vec<String>,
    #[serde(default)]
    workstream_id: Option<String>,
    /// Any-of.
    #[serde(default)]
    item_kind: Vec<String>,
    #[serde(default)]
    item_id: Option<String>,
    /// All-of substrings of the title.
    #[serde(default)]
    title_contains: Vec<String>,
    /// All-of substrings of the content.
    #[serde(default)]
    content_contains: Vec<String>,
    #[serde(default)]
    authority: Option<String>,
    /// The mutation must cite every one of these event sequences.
    #[serde(default)]
    source_events: Vec<i64>,
}

#[derive(Debug, Deserialize, Clone)]
struct ContextItemExpectation {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    title_contains: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
struct ExtractorGold {
    #[serde(default)]
    required_mutations: Vec<StructuralMutationExpectation>,
    #[serde(default)]
    forbidden_mutations: Vec<StructuralMutationExpectation>,
    #[serde(default)]
    required_context: HashMap<String, Vec<ContextItemExpectation>>,
    #[serde(default)]
    forbidden_context: HashMap<String, Vec<ContextItemExpectation>>,
    #[serde(default)]
    conflicts_count: usize,
}

#[derive(Debug, Deserialize)]
struct ContextQualityFixture {
    name: String,
    description: String,
    dimension: String,
    input: FixtureInput,
    recorded_model_output: String,
    gold: FixtureGold,
    extractor_gold: ExtractorGold,
    /// Documents the known heuristic limitation for this case: while present,
    /// the heuristic Layer B eval must fail. Remove in the same commit that
    /// fixes the extractor.
    #[serde(default)]
    heuristic_expected_failure: Option<String>,
}

// ---------------------------------------------------------------------------
// Shared corpus setup
// ---------------------------------------------------------------------------

struct FixtureEnv {
    fixture: ContextQualityFixture,
    db: Db,
    session: Session,
    session_events: Vec<SessionEvent>,
    candidate_ids: Vec<String>,
    ref_map: Vec<PromptEventRef>,
}

fn open_test_db(tag: &str) -> Db {
    let dir = std::env::temp_dir().join(format!("noending-eval-{}-{}", tag, new_id()));
    Db::open(&dir.join("test.db")).expect("open eval db")
}

fn setup_fixture(fixture_json: &str) -> FixtureEnv {
    let fixture: ContextQualityFixture =
        serde_json::from_str(fixture_json).expect("valid fixture JSON");
    let db = open_test_db(&fixture.name);

    for ws in &fixture.input.workstreams {
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

    for it in &fixture.input.initial_context {
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

    let session = Session {
        id: format!("sess-{}", fixture.name),
        agent: match fixture.input.session.agent.to_lowercase().as_str() {
            "codex" => Agent::Codex,
            "claude_code" | "claude" => Agent::ClaudeCode,
            "pi" => Agent::Pi,
            _ => Agent::Codex,
        },
        agent_session_id: format!("as-{}", fixture.name),
        title: Some(fixture.input.session.title.clone()),
        cwd: None,
        project_id: None,
        raw_path: format!("/tmp/{}.jsonl", fixture.name),
        parent_agent_session_id: None,
        started_at: Some(now()),
        last_activity_at: Some(now()),
    };
    db.upsert_session(&session).expect("upsert session");

    // Prompt-local short refs follow the fixture's declared sequence numbers;
    // they are positional labels into the recorded prompt, never event
    // identity (that stays `ev-<fixture>-<seq>` / `session-event:<id>`).
    let mut session_events = Vec::new();
    let mut ref_map = Vec::new();
    for fe in &fixture.input.events {
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

    let candidate_ids: Vec<String> = fixture
        .input
        .workstreams
        .iter()
        .map(|w| w.id.clone())
        .collect();

    FixtureEnv {
        fixture,
        db,
        session,
        session_events,
        candidate_ids,
        ref_map,
    }
}

// ---------------------------------------------------------------------------
// Layer A — Domain Golden (recorded model output → exact domain state)
// ---------------------------------------------------------------------------

fn run_domain_golden(env: FixtureEnv) -> Db {
    let FixtureEnv {
        fixture,
        db,
        session,
        ref_map,
        candidate_ids,
        ..
    } = env;

    let parse_output = parse_mutations(
        &fixture.recorded_model_output,
        &ref_map,
        &candidate_ids,
        &session,
    )
    .expect("parse mutations must succeed");
    let mutations = parse_output.mutations;

    assert_eq!(
        mutations.len(),
        fixture.gold.mutations.len(),
        "Expected {} mutations, got {}",
        fixture.gold.mutations.len(),
        mutations.len()
    );

    for (actual, expected) in mutations.iter().zip(fixture.gold.mutations.iter()) {
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

    for (ws_id, expected_items) in &fixture.gold.effective_context {
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

    let mut total_open_conflicts = 0;
    for ws in &fixture.input.workstreams {
        let conflicts = db
            .conflicts_for_workstream(&ws.id, false)
            .expect("conflicts_for_workstream");
        total_open_conflicts += conflicts.len();
    }
    assert_eq!(
        total_open_conflicts, fixture.gold.conflicts_count,
        "Open conflicts count mismatch for fixture {}",
        fixture.name
    );

    db
}

// ---------------------------------------------------------------------------
// Layer B — Extraction Eval (real extractor → structural gold)
// ---------------------------------------------------------------------------

/// Compact projection of a mutation for structural matching and reporting.
struct MutationView<'a> {
    op: &'a str,
    workstream_id: Option<&'a str>,
    item_kind: Option<&'a str>,
    item_id: Option<&'a str>,
    title: &'a str,
    content: &'a str,
    authority: Option<&'a str>,
    source_refs: &'a [String],
}

fn mutation_view(m: &ContextMutation) -> MutationView<'_> {
    match m {
        ContextMutation::Add {
            workstream_id,
            item_kind,
            title,
            content,
            source_refs,
            authority,
        } => MutationView {
            op: "add",
            workstream_id: Some(workstream_id),
            item_kind: Some(item_kind),
            item_id: None,
            title,
            content,
            authority: Some(authority),
            source_refs,
        },
        ContextMutation::Update {
            item_id,
            title,
            content,
            source_refs,
            authority,
        } => MutationView {
            op: "update",
            workstream_id: None,
            item_kind: None,
            item_id: Some(item_id),
            title,
            content,
            authority: Some(authority),
            source_refs,
        },
        ContextMutation::Supersede {
            item_id,
            title,
            content,
            source_refs,
            authority,
        } => MutationView {
            op: "supersede",
            workstream_id: None,
            item_kind: None,
            item_id: Some(item_id),
            title,
            content,
            authority: Some(authority),
            source_refs,
        },
        ContextMutation::Resolve {
            item_id,
            source_refs,
        } => MutationView {
            op: "resolve",
            workstream_id: None,
            item_kind: None,
            item_id: Some(item_id),
            title: "",
            content: "",
            authority: None,
            source_refs,
        },
        ContextMutation::CreateWorkstream { title, reason, .. } => MutationView {
            op: "create_workstream",
            workstream_id: None,
            item_kind: None,
            item_id: None,
            title,
            content: reason,
            authority: None,
            source_refs: &[],
        },
        ContextMutation::Conflict {
            workstream_id,
            item_id,
            title,
            content,
            source_refs,
            ..
        } => MutationView {
            op: "conflict",
            workstream_id: Some(workstream_id),
            item_kind: None,
            item_id: Some(item_id),
            title,
            content,
            authority: None,
            source_refs,
        },
    }
}

fn mutation_matches(
    v: &MutationView<'_>,
    exp: &StructuralMutationExpectation,
    seq_to_event_id: &HashMap<i64, String>,
) -> bool {
    if !exp.op.is_empty() && !exp.op.iter().any(|o| o == v.op) {
        return false;
    }
    if let Some(w) = &exp.workstream_id {
        if v.workstream_id != Some(w.as_str()) {
            return false;
        }
    }
    if !exp.item_kind.is_empty() {
        match v.item_kind {
            Some(k) if exp.item_kind.iter().any(|x| x == k) => {}
            _ => return false,
        }
    }
    if let Some(i) = &exp.item_id {
        if v.item_id != Some(i.as_str()) {
            return false;
        }
    }
    if exp.title_contains.iter().any(|s| !v.title.contains(s)) {
        return false;
    }
    if exp.content_contains.iter().any(|s| !v.content.contains(s)) {
        return false;
    }
    if let Some(a) = &exp.authority {
        if v.authority != Some(a.as_str()) {
            return false;
        }
    }
    for seq in &exp.source_events {
        let Some(event_id) = seq_to_event_id.get(seq) else {
            return false;
        };
        let want = format!("session-event:{event_id}");
        if !v.source_refs.iter().any(|r| r == &want) {
            return false;
        }
    }
    true
}

/// An expectation with zero constraints would match every mutation — a
/// fixture bug, not an eval result.
fn ensure_constrained(exp: &StructuralMutationExpectation, fixture: &str, idx: usize) {
    let constrained = !exp.op.is_empty()
        || exp.workstream_id.is_some()
        || !exp.item_kind.is_empty()
        || exp.item_id.is_some()
        || !exp.title_contains.is_empty()
        || !exp.content_contains.is_empty()
        || exp.authority.is_some()
        || !exp.source_events.is_empty();
    assert!(
        constrained,
        "fixture {fixture}: extractor_gold mutation #{idx} has no constraint and would match every mutation"
    );
}

fn describe_expectation(exp: &StructuralMutationExpectation) -> String {
    let mut parts = Vec::new();
    if !exp.op.is_empty() {
        parts.push(format!("op∈{:?}", exp.op));
    }
    if let Some(w) = &exp.workstream_id {
        parts.push(format!("ws={w}"));
    }
    if !exp.item_kind.is_empty() {
        parts.push(format!("kind∈{:?}", exp.item_kind));
    }
    if let Some(i) = &exp.item_id {
        parts.push(format!("item={i}"));
    }
    if !exp.title_contains.is_empty() {
        parts.push(format!("title⊇{:?}", exp.title_contains));
    }
    if !exp.content_contains.is_empty() {
        parts.push(format!("content⊇{:?}", exp.content_contains));
    }
    if let Some(a) = &exp.authority {
        parts.push(format!("authority={a}"));
    }
    if !exp.source_events.is_empty() {
        let refs: Vec<String> = exp.source_events.iter().map(|s| format!("#{s}")).collect();
        parts.push(format!("cites={}", refs.join(",")));
    }
    if parts.is_empty() {
        "(unconstrained)".into()
    } else {
        parts.join(" ")
    }
}

fn describe_mutation(v: &MutationView<'_>) -> String {
    let mut parts = vec![format!("op={}", v.op)];
    if let Some(w) = v.workstream_id {
        parts.push(format!("ws={w}"));
    }
    if let Some(k) = v.item_kind {
        parts.push(format!("kind={k}"));
    }
    if let Some(i) = v.item_id {
        parts.push(format!("item={i}"));
    }
    if !v.title.is_empty() {
        parts.push(format!("title={:?}", v.title));
    }
    if let Some(a) = v.authority {
        parts.push(format!("authority={a}"));
    }
    format!("({})", parts.join(" "))
}

fn context_matches(kind: &str, title: &str, exp: &ContextItemExpectation) -> bool {
    if let Some(k) = &exp.kind {
        if k != kind {
            return false;
        }
    }
    exp.title_contains.iter().all(|s| title.contains(s))
}

fn describe_context_expectation(exp: &ContextItemExpectation) -> String {
    let mut parts = Vec::new();
    if let Some(k) = &exp.kind {
        parts.push(format!("kind={k}"));
    }
    if !exp.title_contains.is_empty() {
        parts.push(format!("title⊇{:?}", exp.title_contains));
    }
    if parts.is_empty() {
        "(any core item)".into()
    } else {
        parts.join(" ")
    }
}

fn effective_core(env: &FixtureEnv, ws_id: &str) -> Vec<(String, String)> {
    noending::context::resolve_core_context(&env.db, ws_id)
        .expect("resolve_core_context")
        .into_iter()
        .map(|s| (s.kind, s.title))
        .collect()
}

/// Run one corpus case through a real extractor and the production merge
/// engine, then evaluate the structural gold. Returns human-readable
/// failures; empty = the case passes.
fn run_extractor_eval(env: &FixtureEnv, extractor: &dyn ContextExtractor) -> Vec<String> {
    // Same snapshot boundary as a production SyncJob: prompt inputs are read
    // up front, extraction itself never touches the database.
    let inputs =
        collect_prompt_inputs(&env.db, &env.candidate_ids).expect("snapshot prompt inputs");
    let event_refs: Vec<&SessionEvent> = env.session_events.iter().collect();
    let out = extractor
        .extract(&env.session, &event_refs, &env.candidate_ids, &inputs)
        .expect("extractor must succeed");

    let merger = MergeEngine;
    let merge_ctx = MergeContext {
        run_id: format!("eval-run-{}-{}", env.fixture.name, extractor.name()),
        runtime: "eval-harness".into(),
    };
    env.db
        .tx(|tx| {
            for m in &out.mutations {
                merger.apply(tx, m, &merge_ctx)?;
            }
            Ok(())
        })
        .expect("merger apply must commit cleanly");

    evaluate_extractor_gold(env, &out.mutations)
}

fn evaluate_extractor_gold(env: &FixtureEnv, mutations: &[ContextMutation]) -> Vec<String> {
    let gold = &env.fixture.extractor_gold;
    let mut failures = Vec::new();

    let seq_to_event_id: HashMap<i64, String> = env
        .session_events
        .iter()
        .map(|e| (e.sequence, e.id.clone()))
        .collect();
    let views: Vec<MutationView<'_>> = mutations.iter().map(mutation_view).collect();

    for (i, exp) in gold.required_mutations.iter().enumerate() {
        ensure_constrained(exp, &env.fixture.name, i);
        if !views
            .iter()
            .any(|v| mutation_matches(v, exp, &seq_to_event_id))
        {
            let extracted = if views.is_empty() {
                "extractor produced no mutations".to_string()
            } else {
                format!(
                    "extracted: {}",
                    views
                        .iter()
                        .map(describe_mutation)
                        .collect::<Vec<_>>()
                        .join(" | ")
                )
            };
            failures.push(format!(
                "required_mutation #{} missing ({}); {extracted}",
                i,
                describe_expectation(exp)
            ));
        }
    }

    for (i, exp) in gold.forbidden_mutations.iter().enumerate() {
        ensure_constrained(exp, &env.fixture.name, i);
        if let Some(v) = views
            .iter()
            .find(|v| mutation_matches(v, exp, &seq_to_event_id))
        {
            failures.push(format!(
                "forbidden_mutation #{} produced {} matching ({})",
                i,
                describe_mutation(v),
                describe_expectation(exp)
            ));
        }
    }

    for (ws_id, expected) in &gold.required_context {
        let effective = effective_core(env, ws_id);
        for exp in expected {
            if !effective.iter().any(|(k, t)| context_matches(k, t, exp)) {
                failures.push(format!(
                    "required_context missing in {ws_id}: ({}); effective: {:?}",
                    describe_context_expectation(exp),
                    effective
                ));
            }
        }
    }

    for (ws_id, forbidden) in &gold.forbidden_context {
        let effective = effective_core(env, ws_id);
        for exp in forbidden {
            if let Some((k, t)) = effective.iter().find(|(k, t)| context_matches(k, t, exp)) {
                failures.push(format!(
                    "forbidden_context present in {ws_id}: (\"{k}\", \"{t}\") matches ({})",
                    describe_context_expectation(exp)
                ));
            }
        }
    }

    let mut open_conflicts = 0;
    for ws in &env.fixture.input.workstreams {
        open_conflicts += env
            .db
            .conflicts_for_workstream(&ws.id, false)
            .expect("conflicts_for_workstream")
            .len();
    }
    if open_conflicts != gold.conflicts_count {
        failures.push(format!(
            "conflicts: expected {}, actual {}",
            gold.conflicts_count, open_conflicts
        ));
    }

    failures
}

fn assert_eval_outcome(fixture: &ContextQualityFixture, failures: Vec<String>) {
    match &fixture.heuristic_expected_failure {
        Some(reason) => assert!(
            !failures.is_empty(),
            "fixture '{}' now passes the heuristic eval — remove heuristic_expected_failure from \
             the fixture JSON in the same commit that fixes the extractor (documented limitation: {reason})",
            fixture.name
        ),
        None => assert!(
            failures.is_empty(),
            "heuristic extraction eval failed for '{}' ({}, {}):\n{}\n\n\
             The extractor regressed against corpus semantics — fix the extractor, do not weaken the gold.",
            fixture.name,
            fixture.description,
            fixture.dimension,
            failures.iter().map(|f| format!("  - {f}")).collect::<Vec<_>>().join("\n")
        ),
    }
}

// ---------------------------------------------------------------------------
// Layer A tests — Domain Golden
// ---------------------------------------------------------------------------

#[test]
fn domain_goal_evolution() {
    let db = run_domain_golden(setup_fixture(FIXTURE_GOAL_EVOLUTION));

    let item = db.get_item("goal-initial").unwrap().unwrap();
    assert_eq!(item.status, "active");
    let rev_id = item.current_revision_id.expect("has revision");
    let rev = db.get_revision(&rev_id).unwrap().unwrap();
    assert!(rev.title.contains("完善生产级认证系统"));
}

#[test]
fn domain_decision_supersede() {
    let db = run_domain_golden(setup_fixture(FIXTURE_DECISION_SUPERSEDE));

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
fn domain_constraint_conflict() {
    let db = run_domain_golden(setup_fixture(FIXTURE_CONSTRAINT_CONFLICT));

    let conflicts = db.conflicts_for_workstream("ws-storage", false).unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].left_item_id, "const-migration");
    assert!(conflicts[0].candidate_snapshot_json.is_some());

    let item = db.get_item("const-migration").unwrap().unwrap();
    assert_eq!(item.authority, "user_explicit");
    assert_eq!(item.status, "active");
}

#[test]
fn domain_todo_resolution() {
    let db = run_domain_golden(setup_fixture(FIXTURE_TODO_RESOLUTION));

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
fn domain_multi_workstream_classification() {
    let db = run_domain_golden(setup_fixture(FIXTURE_MULTI_WS));

    let launcher_items = db.items_for_workstream("ws-launcher", false).unwrap();
    assert_eq!(launcher_items.len(), 1);

    let sync_items = db.items_for_workstream("ws-sync", false).unwrap();
    assert_eq!(
        sync_items.len(),
        0,
        "Non-target workstream must remain clean"
    );
}

#[test]
fn domain_workstream_routing_reverse() {
    let db = run_domain_golden(setup_fixture(FIXTURE_MULTI_WS_REVERSE));

    let launcher_items = db.items_for_workstream("ws-launcher", false).unwrap();
    assert_eq!(launcher_items.len(), 1);

    let sync_items = db.items_for_workstream("ws-sync", false).unwrap();
    assert_eq!(
        sync_items.len(),
        0,
        "Sync-first candidate order must not attract the Launcher fact"
    );
}

// ---------------------------------------------------------------------------
// Layer B tests — Extraction Eval with the deterministic heuristic extractor
// ---------------------------------------------------------------------------

fn assert_heuristic_outcome(fixture_json: &str) {
    let env = setup_fixture(fixture_json);
    let failures = run_extractor_eval(&env, &HeuristicExtractor);
    assert_eval_outcome(&env.fixture, failures);
}

#[test]
fn heuristic_eval_goal_evolution() {
    assert_heuristic_outcome(FIXTURE_GOAL_EVOLUTION);
}

#[test]
fn heuristic_eval_decision_supersede() {
    assert_heuristic_outcome(FIXTURE_DECISION_SUPERSEDE);
}

#[test]
fn heuristic_eval_todo_resolution() {
    assert_heuristic_outcome(FIXTURE_TODO_RESOLUTION);
}

#[test]
fn heuristic_eval_constraint_conflict() {
    assert_heuristic_outcome(FIXTURE_CONSTRAINT_CONFLICT);
}

#[test]
fn heuristic_eval_multi_workstream_classification() {
    assert_heuristic_outcome(FIXTURE_MULTI_WS);
}

#[test]
fn heuristic_eval_workstream_routing_reverse() {
    assert_heuristic_outcome(FIXTURE_MULTI_WS_REVERSE);
}

/// The quantified heuristic baseline over the whole corpus. Run with
/// `cargo test heuristic_eval_baseline_report -- --nocapture` to see the
/// report; enforcement matches the per-case tests above.
#[test]
fn heuristic_eval_baseline_report() {
    let corpus: [(&str, &str); 6] = [
        ("goal_evolution", FIXTURE_GOAL_EVOLUTION),
        ("decision_supersede", FIXTURE_DECISION_SUPERSEDE),
        ("todo_resolution", FIXTURE_TODO_RESOLUTION),
        ("constraint_conflict", FIXTURE_CONSTRAINT_CONFLICT),
        ("multi_workstream_classification", FIXTURE_MULTI_WS),
        ("workstream_routing_reverse", FIXTURE_MULTI_WS_REVERSE),
    ];

    let mut passed = 0usize;
    let mut known_failures = 0usize;
    let mut rows: Vec<String> = Vec::new();

    for (name, json) in corpus {
        let env = setup_fixture(json);
        let failures = run_extractor_eval(&env, &HeuristicExtractor);
        match (&env.fixture.heuristic_expected_failure, failures.as_slice()) {
            (Some(reason), []) => panic!(
                "fixture '{name}' now passes the heuristic eval — remove \
                 heuristic_expected_failure from the fixture in the same commit that fixes \
                 the extractor (was: {reason})"
            ),
            (Some(reason), _) => {
                known_failures += 1;
                rows.push(format!(
                    "  FAIL (known) {name} [{}] — {reason}",
                    env.fixture.dimension
                ));
            }
            (None, []) => {
                passed += 1;
                rows.push(format!("  PASS         {name} [{}]", env.fixture.dimension));
            }
            (None, fs) => {
                let detail = fs
                    .iter()
                    .map(|f| format!("        - {f}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                rows.push(format!(
                    "  FAIL         {name} [{}]\n{detail}",
                    env.fixture.dimension
                ));
            }
        }
    }

    println!("\nContext Quality Eval — extractor: heuristic");
    println!("=================================================");
    for row in &rows {
        println!("{row}");
    }
    println!("-------------------------------------------------");
    println!(
        "  cases {} | passed {} | known failures {} | unregistered failures {}",
        corpus.len(),
        passed,
        known_failures,
        corpus.len() - passed - known_failures
    );

    assert_eq!(
        passed + known_failures,
        corpus.len(),
        "every corpus case must either pass or be a registered known failure — see the printed report"
    );
}
