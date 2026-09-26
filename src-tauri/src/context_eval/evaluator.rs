//! Evaluation engine: runs corpus fixtures through the production pipeline
//! and compares outcomes against gold.
//!
//! - Layer A — [`run_domain_golden`]: `recorded_model_output` →
//!   `parse_mutations` → `MergeEngine` → exact domain state assertions.
//! - Layer B — [`evaluate_extractor`]: `input.events` + candidates +
//!   existing items → any [`ContextExtractor`] → same `MergeEngine` →
//!   structural gold checks with stable ids.
//!
//! The engine is deliberately extractor-agnostic: it accepts a
//! `&dyn ContextExtractor` and knows nothing about Codex / Claude / Pi
//! configuration, CLI installation discovery, models, argument parsing or
//! console rendering — all of that lives with the caller (tests today, the
//! future `context-eval` binary later). Dependency direction is one-way:
//! eval harness → production; production never depends on the harness.
//!
//! Layer A gold enforcement asserts inside the run (exact domain state is
//! the contract); infrastructure failures surface as `Err`. Layer B never
//! panics on outcomes — it returns every check so callers can compare,
//! report and enforce baselines themselves.

use std::collections::{BTreeSet, HashMap};

use crate::context::resolve_core_context;
use crate::domain::{
    Agent, ContextItem, ContextItemRevision, Session, SessionMemberRelation, SessionMessage,
    SessionMessageRole, Workstream,
};
use crate::error::{other, Result};
use crate::storage::{new_id, now, Db};
use crate::sync::extractor::{collect_prompt_inputs, parse_mutations, PromptMessageRef};
use crate::sync::merge::MergeEngine;
use crate::sync::{ContextExtractor, ContextMutation, MergeContext};

use super::corpus::{
    ContextItemExpectation, ContextQualityFixture, StructuralMutationExpectation,
    CONFLICTS_CHECK_ID,
};

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

/// One named, individually pass/fail-able gold check for a single eval run.
/// Failed check ids form the `actual_failed_checks` set that baseline
/// enforcement compares against the fixture's recorded expectation.
#[derive(Debug, Clone)]
pub struct CheckResult {
    pub id: String,
    pub passed: bool,
    /// Human-readable evidence; non-empty when the check failed.
    pub detail: String,
}

/// Result of evaluating one fixture through one extractor (Layer B).
#[derive(Debug, Clone)]
pub struct EvalCaseResult {
    pub fixture_name: String,
    pub dimension: String,
    pub description: String,
    pub checks: Vec<CheckResult>,
}

impl EvalCaseResult {
    pub fn passed_checks(&self) -> usize {
        self.checks.iter().filter(|c| c.passed).count()
    }

    /// Ids of all failed checks — the `actual_failed_checks` set.
    pub fn failed_check_ids(&self) -> BTreeSet<String> {
        self.checks
            .iter()
            .filter(|c| !c.passed)
            .map(|c| c.id.clone())
            .collect()
    }

    /// Criterion-level baseline comparison: set equality between the actual
    /// failed checks and `expected_failed_checks`. The symmetric differences
    /// separate regression (new failures) from improvement (now passing);
    /// only a clean diff means the baseline still describes the extractor.
    pub fn baseline_diff(&self, expected_failed_checks: &[String]) -> BaselineDiff {
        let expected: BTreeSet<&str> = expected_failed_checks.iter().map(|s| s.as_str()).collect();
        let actual = self.failed_check_ids();
        let actual_refs: BTreeSet<&str> = actual.iter().map(|s| s.as_str()).collect();
        BaselineDiff {
            unexpected_failures: actual_refs
                .difference(&expected)
                .filter_map(|id| self.checks.iter().find(|c| c.id.as_str() == *id).cloned())
                .collect(),
            unexpected_passes: expected
                .difference(&actual_refs)
                .map(|id| (*id).to_string())
                .collect(),
        }
    }
}

/// Symmetric difference between a recorded baseline and the actual failed
/// check set of a run.
#[derive(Debug, Default)]
pub struct BaselineDiff {
    /// Failed now but not in the baseline: the extractor regressed — or the
    /// baseline must consciously move.
    pub unexpected_failures: Vec<CheckResult>,
    /// In the baseline but passing now: re-baseline in the same commit.
    pub unexpected_passes: Vec<String>,
}

impl BaselineDiff {
    pub fn is_clean(&self) -> bool {
        self.unexpected_failures.is_empty() && self.unexpected_passes.is_empty()
    }
}

/// Aggregated results over a corpus run. Data only — rendering and
/// enforcement policy are the caller's job.
#[derive(Debug, Default)]
pub struct EvalReport {
    pub cases: Vec<EvalCaseResult>,
}

impl EvalReport {
    pub fn new(cases: Vec<EvalCaseResult>) -> Self {
        Self { cases }
    }

    pub fn total_checks(&self) -> usize {
        self.cases.iter().map(|c| c.checks.len()).sum()
    }

    pub fn passed_checks(&self) -> usize {
        self.cases.iter().map(|c| c.passed_checks()).sum()
    }

    pub fn fully_passed_cases(&self) -> usize {
        self.cases
            .iter()
            .filter(|c| c.failed_check_ids().is_empty())
            .count()
    }
}

// ---------------------------------------------------------------------------
// Corpus setup
// ---------------------------------------------------------------------------

/// Database, session, conversation messages and prompt references
/// materialized from one fixture, ready to be driven by the Layer A / Layer B
/// runners.
pub struct FixtureEnv {
    pub db: Db,
    pub session: Session,
    pub session_messages: Vec<SessionMessage>,
    /// The single Owner Workstream this fixture routes to (方案 §19): the
    /// first workstream of the fixture. A fixture with more than one
    /// workstream no longer describes a legal Session — a Session has at most
    /// one Owner — so only the first is ever the routing target.
    pub owner_workstream_id: String,
    pub ref_map: Vec<PromptMessageRef>,
}

fn open_eval_db(fixture_name: &str) -> Result<Db> {
    let dir = std::env::temp_dir().join(format!("noending-eval-{}-{}", fixture_name, new_id()));
    Db::open(&dir.join("test.db"))
        .map_err(|e| other(format!("fixture {fixture_name}: open eval db failed: {e}")))
}

/// Materialize the fixture's domain state (workstreams, initial context,
/// session, events) into a fresh temporary database.
pub fn setup_fixture(fixture: &ContextQualityFixture) -> Result<FixtureEnv> {
    let name = &fixture.name;
    let db = open_eval_db(name)?;

    for ws in &fixture.input.workstreams {
        let w = Workstream {
            id: ws.id.clone(),
            title: ws.title.clone(),
            description: ws.description.clone(),
            lifecycle: crate::domain::workstream_lifecycle::ACTIVE.into(),
            visibility: "normal".into(),
            created_at: now(),
            updated_at: now(),
        };
        db.upsert_workstream(&w).map_err(|e| {
            other(format!(
                "fixture {name}: upsert workstream {} failed: {e}",
                ws.id
            ))
        })?;
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
            crate::storage::insert_item_conn(tx, &item, &rev)?;
            Ok(())
        })
        .map_err(|e| {
            other(format!(
                "fixture {name}: insert initial item {} failed: {e}",
                it.id
            ))
        })?;
    }

    let agent = match fixture.input.session.agent.to_lowercase().as_str() {
        "codex" => Agent::Codex,
        "claude_code" | "claude" => Agent::ClaudeCode,
        "pi" => Agent::Pi,
        "qoder" | "qcoder" => Agent::Qoder,
        "autoclaw" | "openclaw" => Agent::AutoClaw,
        "workbuddy" | "work_buddy" => Agent::WorkBuddy,
        "dsh" | "deepseek_harness" => Agent::Dsh,
        "zcode" | "z_code" => Agent::ZCode,
        _ => Agent::Codex,
    };
    let root_agent_session_id = format!("as-{name}");
    let (session_id, _) = db
        .upsert_logical_session_unchecked(
            agent,
            &root_agent_session_id,
            Some(&fixture.input.session.title),
            None,
            None,
            None,
            Some(&now()),
            Some(&now()),
        )
        .map_err(|e| other(format!("fixture {name}: upsert session failed: {e}")))?;
    let root_member_id = db
        .upsert_session_member(
            &session_id,
            agent,
            &root_agent_session_id,
            SessionMemberRelation::Root,
            None,
            "eval_fixture",
            &format!("/tmp/{name}.jsonl"),
            None,
            Some(&now()),
            Some(&now()),
            &serde_json::json!({}),
        )
        .map_err(|e| other(format!("fixture {name}: upsert root member failed: {e}")))?;

    // The harness drives extraction directly with the fixture's single
    // workstream id; the row itself is not consulted for routing.
    let session = Session {
        id: session_id.clone(),
        agent,
        root_agent_session_id,
        title: Some(fixture.input.session.title.clone()),
        cwd: None,
        workspace_path_id: None,
        project_id: None,
        owner_workstream_id: None,
        forked_from_session_id: None,
        started_at: Some(now()),
        last_activity_at: Some(now()),
        last_conversation_at: None,
        trashed_at: None,
    };

    // Prompt-local short refs follow the fixture's declared sequence numbers;
    // they are positional labels into the recorded prompt, never message
    // identity (that stays `ev-<fixture>-<seq>` / `session-message:<id>`).
    // Messages are seeded through the production commit path so the fixture
    // conversation satisfies the same invariants a real ingest does.
    let mut session_messages = Vec::new();
    let mut ref_map = Vec::new();
    for fe in &fixture.input.events {
        let ev_id = format!("ev-{name}-{}", fe.sequence);
        let short_ref = format!("#{}", fe.sequence);
        let role = if fe.kind == "user_message" {
            SessionMessageRole::User
        } else {
            SessionMessageRole::Assistant
        };
        let stored = db
            .commit_member_ingest(
                &session_id,
                &root_member_id,
                &[crate::domain::ParsedSessionMessage {
                    source_message_id: Some(ev_id.clone()),
                    source_position: format!("line:{}", fe.sequence),
                    ts: None,
                    role,
                    content: fe.text.clone(),
                }],
                None,
                &crate::domain::SourceCursorUpdate {
                    file_identity: format!("eval-{name}"),
                    generation: 0,
                    byte_offset: fe.sequence as u64,
                    last_seen_size: fe.sequence as u64,
                    mtime: None,
                    start_byte_offset: 0,
                    prefix_hash: String::new(),
                },
            )
            .map_err(|e| other(format!("fixture {name}: seed message failed: {e}")))?;
        let stored = stored
            .last()
            .cloned()
            .ok_or_else(|| other(format!("fixture {name}: seeded message deduped away")))?;
        ref_map.push(PromptMessageRef {
            short_ref,
            message_id: stored.id.clone(),
            sequence: stored.sequence,
            role,
        });
        session_messages.push(stored);
    }

    let owner_workstream_id = fixture
        .input
        .workstreams
        .first()
        .map(|w| w.id.clone())
        .ok_or_else(|| other(format!("fixture {name}: 没有 Workstream")))?;

    Ok(FixtureEnv {
        db,
        session,
        session_messages,
        owner_workstream_id,
        ref_map,
    })
}

// ---------------------------------------------------------------------------
// Layer A — Domain Golden (recorded model output → exact domain state)
// ---------------------------------------------------------------------------

pub fn run_domain_golden(fixture: &ContextQualityFixture) -> Result<Db> {
    let FixtureEnv {
        db,
        session,
        ref_map,
        owner_workstream_id,
        ..
    } = setup_fixture(fixture)?;
    let name = &fixture.name;

    let parse_output = parse_mutations(
        &fixture.recorded_model_output,
        &ref_map,
        &owner_workstream_id,
        &session,
    )
    .map_err(|e| other(format!("fixture {name}: parse recorded output failed: {e}")))?;
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
        run_id: format!("eval-run-{name}"),
        runtime: "eval-harness".into(),
        workstream_id: owner_workstream_id.clone(),
    };

    db.tx(|tx| {
        for m in &mutations {
            merger.apply(tx, m, &merge_ctx)?;
        }
        Ok(())
    })
    .map_err(|e| {
        other(format!(
            "fixture {name}: merge recorded mutations failed: {e}"
        ))
    })?;

    for (ws_id, expected_items) in &fixture.gold.effective_context {
        let core_sections = resolve_core_context(&db, ws_id).map_err(|e| {
            other(format!(
                "fixture {name}: resolve core for {ws_id} failed: {e}"
            ))
        })?;
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
            .map_err(|e| other(format!("fixture {name}: conflicts query failed: {e}")))?;
        total_open_conflicts += conflicts.len();
    }
    assert_eq!(
        total_open_conflicts, fixture.gold.conflicts_count,
        "Open conflicts count mismatch for fixture {name}",
    );

    Ok(db)
}

// ---------------------------------------------------------------------------
// Layer B — Extraction Eval (real extractor → criterion-level structural gold)
// ---------------------------------------------------------------------------

/// Run one fixture through any extractor and the production merge engine,
/// then evaluate every structural gold check. Never panics on outcomes —
/// baseline enforcement (e.g. `EvalCaseResult::baseline_diff`) is the
/// caller's policy.
pub fn evaluate_extractor(
    fixture: &ContextQualityFixture,
    extractor: &dyn ContextExtractor,
) -> Result<EvalCaseResult> {
    let name = &fixture.name;
    let env = setup_fixture(fixture)?;

    // Same snapshot boundary as a production SyncJob: prompt inputs are read
    // up front, extraction itself never touches the database.
    let inputs = collect_prompt_inputs(&env.db, &env.owner_workstream_id).map_err(|e| {
        other(format!(
            "fixture {name}: snapshot prompt inputs failed: {e}"
        ))
    })?;
    let message_refs: Vec<&SessionMessage> = env.session_messages.iter().collect();
    let out = extractor
        .extract(
            &env.session,
            &message_refs,
            &env.owner_workstream_id,
            &inputs,
        )
        .map_err(|e| {
            other(format!(
                "fixture {name}: extractor '{}' failed: {e}",
                extractor.name()
            ))
        })?;

    let merger = MergeEngine;
    let merge_ctx = MergeContext {
        run_id: format!("eval-run-{name}-{}", extractor.name()),
        runtime: "eval-harness".into(),
        workstream_id: env.owner_workstream_id.clone(),
    };
    env.db
        .tx(|tx| {
            for m in &out.mutations {
                merger.apply(tx, m, &merge_ctx)?;
            }
            Ok(())
        })
        .map_err(|e| {
            other(format!(
                "fixture {name}: merge extracted mutations failed: {e}"
            ))
        })?;

    Ok(evaluate_extractor_gold(fixture, &env, &out.mutations))
}

fn evaluate_extractor_gold(
    fixture: &ContextQualityFixture,
    env: &FixtureEnv,
    mutations: &[ContextMutation],
) -> EvalCaseResult {
    let gold = &fixture.extractor_gold;
    let name = &fixture.name;
    let mut checks = Vec::new();

    let seq_to_event_id: HashMap<i64, String> = env
        .session_messages
        .iter()
        .map(|m| (m.sequence, m.id.clone()))
        .collect();
    let views: Vec<MutationView<'_>> = mutations.iter().map(mutation_view).collect();

    for exp in &gold.required_mutations {
        let matched = views
            .iter()
            .find(|v| mutation_matches(v, exp, &seq_to_event_id));
        checks.push(CheckResult {
            id: format!("required:{}", exp.id),
            passed: matched.is_some(),
            detail: match matched {
                Some(_) => String::new(),
                None => {
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
                    format!(
                        "no mutation matches ({}); {extracted}",
                        describe_expectation(exp)
                    )
                }
            },
        });
    }

    for exp in &gold.forbidden_mutations {
        let violated = views
            .iter()
            .find(|v| mutation_matches(v, exp, &seq_to_event_id));
        checks.push(CheckResult {
            id: format!("forbidden:{}", exp.id),
            passed: violated.is_none(),
            detail: match violated {
                None => String::new(),
                Some(v) => format!(
                    "produced {} matching ({})",
                    describe_mutation(v),
                    describe_expectation(exp)
                ),
            },
        });
    }

    for (ws_id, expected) in &gold.required_context {
        let effective = effective_core(&env.db, name, ws_id);
        for exp in expected {
            let present = effective.iter().any(|(k, t)| context_matches(k, t, exp));
            checks.push(CheckResult {
                id: format!("required_context:{}", exp.id),
                passed: present,
                detail: if present {
                    String::new()
                } else {
                    format!(
                        "missing in {ws_id}: ({}); effective: {:?}",
                        describe_context_expectation(exp),
                        effective
                    )
                },
            });
        }
    }

    for (ws_id, forbidden) in &gold.forbidden_context {
        let effective = effective_core(&env.db, name, ws_id);
        for exp in forbidden {
            let violation = effective
                .iter()
                .find(|(k, t)| context_matches(k, t, exp))
                .map(|(k, t)| format!("(\"{k}\", \"{t}\")"));
            checks.push(CheckResult {
                id: format!("forbidden_context:{}", exp.id),
                passed: violation.is_none(),
                detail: match violation {
                    None => String::new(),
                    Some(found) => format!(
                        "{found} present in {ws_id}, matches ({})",
                        describe_context_expectation(exp)
                    ),
                },
            });
        }
    }

    for exp in &gold.required_item_status {
        let actual = env
            .db
            .get_item(&exp.item_id)
            .unwrap_or_else(|e| panic!("fixture {name}: get_item {} failed: {e}", exp.item_id))
            .map(|i| i.status);
        let satisfied = actual.as_deref() == Some(exp.status.as_str());
        checks.push(CheckResult {
            id: format!("item_status:{}", exp.id),
            passed: satisfied,
            detail: if satisfied {
                String::new()
            } else {
                format!(
                    "item {} must have status '{}', actual {}",
                    exp.item_id,
                    exp.status,
                    actual
                        .map(|s| format!("'{s}'"))
                        .unwrap_or_else(|| "<missing>".into())
                )
            },
        });
    }

    let mut open_conflicts = 0;
    for ws in &fixture.input.workstreams {
        open_conflicts += env
            .db
            .conflicts_for_workstream(&ws.id, false)
            .unwrap_or_else(|e| panic!("fixture {name}: conflicts query failed: {e}"))
            .len();
    }
    checks.push(CheckResult {
        id: CONFLICTS_CHECK_ID.to_string(),
        passed: open_conflicts == gold.conflicts_count,
        detail: if open_conflicts == gold.conflicts_count {
            String::new()
        } else {
            format!(
                "expected {} open conflicts, actual {}",
                gold.conflicts_count, open_conflicts
            )
        },
    });

    EvalCaseResult {
        fixture_name: fixture.name.clone(),
        dimension: fixture.dimension.clone(),
        description: fixture.description.clone(),
        checks,
    }
}

// ---------------------------------------------------------------------------
// Structural matching helpers
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
        let want = format!("session-message:{event_id}");
        if !v.source_refs.iter().any(|r| r == &want) {
            return false;
        }
    }
    true
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

fn effective_core(db: &Db, fixture_name: &str, ws_id: &str) -> Vec<(String, String)> {
    resolve_core_context(db, ws_id)
        .unwrap_or_else(|e| panic!("fixture {fixture_name}: resolve core for {ws_id} failed: {e}"))
        .into_iter()
        .map(|s| (s.kind, s.title))
        .collect()
}
