//! Integration tests over the shared context quality corpus
//! (`tests/fixtures/context_quality/`).
//!
//! The generic evaluation engine lives in [`noending::context_eval`] — see
//! its module docs for the two-layer model and the criterion-level baseline
//! guard. This file deliberately keeps only case-specific material:
//!
//! - which fixtures exist (`include_str!`),
//! - Layer A deep assertions unique to each case (supersede lineage,
//!   conflict candidate snapshot, active/resolved item queries) that must
//!   NOT be abstracted into the evaluator's generic rules,
//! - enforcement of the heuristic baseline (panic formatting is test
//!   policy; the comparison itself is `EvalCaseResult::baseline_diff`).

use noending::context_eval::{
    evaluate_extractor, load_fixture, run_domain_golden, BaselineDiff, EvalCaseResult, EvalReport,
};
use noending::sync::extractor::HeuristicExtractor;

const FIXTURE_GOAL_EVOLUTION: &str = include_str!("fixtures/context_quality/goal_evolution.json");
const FIXTURE_DECISION_SUPERSEDE: &str =
    include_str!("fixtures/context_quality/decision_supersede.json");
const FIXTURE_TODO_RESOLUTION: &str = include_str!("fixtures/context_quality/todo_resolution.json");
const FIXTURE_CONSTRAINT_CONFLICT: &str =
    include_str!("fixtures/context_quality/constraint_conflict.json");
/// Single-owner routing: a Session has exactly one Owner Workstream, so every
/// extracted fact lands there and nowhere else.
const FIXTURE_OWNER_ROUTING: &str =
    include_str!("fixtures/context_quality/workstream_routing_reverse.json");

// ---------------------------------------------------------------------------
// Layer A — Domain Golden: exact run + case-specific deep assertions
// ---------------------------------------------------------------------------

#[test]
fn domain_goal_evolution() {
    let fixture = load_fixture(FIXTURE_GOAL_EVOLUTION).expect("valid fixture");
    let db = run_domain_golden(&fixture).expect("domain golden run");

    let item = db.get_item("goal-initial").unwrap().unwrap();
    assert_eq!(item.status, "active");
    let rev_id = item.current_revision_id.expect("has revision");
    let rev = db.get_revision(&rev_id).unwrap().unwrap();
    assert!(rev.title.contains("完善生产级认证系统"));
}

#[test]
fn domain_decision_supersede() {
    let fixture = load_fixture(FIXTURE_DECISION_SUPERSEDE).expect("valid fixture");
    let db = run_domain_golden(&fixture).expect("domain golden run");

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
    let fixture = load_fixture(FIXTURE_CONSTRAINT_CONFLICT).expect("valid fixture");
    let db = run_domain_golden(&fixture).expect("domain golden run");

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
    let fixture = load_fixture(FIXTURE_TODO_RESOLUTION).expect("valid fixture");
    let db = run_domain_golden(&fixture).expect("domain golden run");

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
fn domain_owner_routing() {
    let fixture = load_fixture(FIXTURE_OWNER_ROUTING).expect("valid fixture");
    let db = run_domain_golden(&fixture).expect("domain golden run");

    let launcher_items = db.items_for_workstream("ws-launcher", false).unwrap();
    assert_eq!(launcher_items.len(), 1);
}

// ---------------------------------------------------------------------------
// Layer B — heuristic extraction baseline (criterion-level enforcement)
// ---------------------------------------------------------------------------

fn assert_heuristic_baseline(fixture_json: &str) {
    let fixture = load_fixture(fixture_json).expect("valid fixture");
    let result = evaluate_extractor(&fixture, &HeuristicExtractor).expect("extraction eval run");
    let diff = result.baseline_diff(&fixture.heuristic_expected_failed_checks);
    assert!(
        diff.is_clean(),
        "heuristic baseline mismatch for '{}' ({}, {}):\n{}",
        result.fixture_name,
        result.dimension,
        result.description,
        format_baseline_diff(&diff)
    );
}

fn format_baseline_diff(diff: &BaselineDiff) -> String {
    let mut msg = String::new();
    if !diff.unexpected_failures.is_empty() {
        msg.push_str(
            "  new failed checks — the extractor regressed. Fix the extractor, or if this is a\n  deliberate behavior change, re-baseline consciously:\n",
        );
        for check in &diff.unexpected_failures {
            msg.push_str(&format!("    - {}: {}\n", check.id, check.detail));
        }
    }
    if !diff.unexpected_passes.is_empty() {
        msg.push_str(
            "  checks now passing — remove them from heuristic_expected_failed_checks in the\n  same commit (the baseline must track the extractor):\n",
        );
        for id in &diff.unexpected_passes {
            msg.push_str(&format!("    - {id}\n"));
        }
    }
    msg
}

#[test]
fn heuristic_goal_evolution() {
    assert_heuristic_baseline(FIXTURE_GOAL_EVOLUTION);
}

#[test]
fn heuristic_decision_supersede() {
    assert_heuristic_baseline(FIXTURE_DECISION_SUPERSEDE);
}

#[test]
fn heuristic_todo_resolution() {
    assert_heuristic_baseline(FIXTURE_TODO_RESOLUTION);
}

#[test]
fn heuristic_constraint_conflict() {
    assert_heuristic_baseline(FIXTURE_CONSTRAINT_CONFLICT);
}

#[test]
fn heuristic_owner_routing() {
    assert_heuristic_baseline(FIXTURE_OWNER_ROUTING);
}

/// The quantified criterion-level heuristic baseline over the whole corpus.
/// Run with `cargo test heuristic_baseline_report -- --nocapture` to see the
/// report; enforcement matches the per-case tests above (set equality per
/// fixture, aggregated once at the end so the full report always prints).
#[test]
fn heuristic_baseline_report() {
    let corpus: [(&str, &str); 5] = [
        ("goal_evolution", FIXTURE_GOAL_EVOLUTION),
        ("decision_supersede", FIXTURE_DECISION_SUPERSEDE),
        ("todo_resolution", FIXTURE_TODO_RESOLUTION),
        ("constraint_conflict", FIXTURE_CONSTRAINT_CONFLICT),
        ("owner_routing", FIXTURE_OWNER_ROUTING),
    ];

    let mut rows: Vec<String> = Vec::new();
    let mut known_failures = 0usize;
    let mut unexpected_failures = 0usize;
    let mut unexpected_passes = 0usize;
    let mut cases: Vec<EvalCaseResult> = Vec::new();

    for (name, json) in corpus {
        let fixture = load_fixture(json).expect("valid fixture");
        let result =
            evaluate_extractor(&fixture, &HeuristicExtractor).expect("extraction eval run");

        let diff = result.baseline_diff(&fixture.heuristic_expected_failed_checks);
        let actual_failed = result.failed_check_ids();
        known_failures += actual_failed.len() - diff.unexpected_failures.len();
        unexpected_failures += diff.unexpected_failures.len();
        unexpected_passes += diff.unexpected_passes.len();

        let status = if !diff.is_clean() {
            "MISMATCH"
        } else if actual_failed.is_empty() {
            "PASS"
        } else {
            "FAIL (known)"
        };
        rows.push(format!("  [{status}] {name} [{}]", result.dimension));
        if !actual_failed.is_empty() {
            let ids: Vec<String> = actual_failed.into_iter().collect();
            rows.push(format!("      failed: {}", ids.join(", ")));
        }
        for id in &diff.unexpected_passes {
            rows.push(format!("      unexpected pass (re-baseline): {id}"));
        }
        for check in &diff.unexpected_failures {
            rows.push(format!(
                "      unexpected failure: {}: {}",
                check.id, check.detail
            ));
        }
        cases.push(result);
    }

    let report = EvalReport::new(cases);
    println!("\nContext Quality Eval — extractor: heuristic");
    println!("=================================================");
    for row in &rows {
        println!("{row}");
    }
    println!("-------------------------------------------------");
    println!("Cases                     {:>4}", report.cases.len());
    println!(
        "Fully passed              {:>4}",
        report.fully_passed_cases()
    );
    println!();
    println!("Checks                    {:>4}", report.total_checks());
    println!("Passed                    {:>4}", report.passed_checks());
    println!("Known failures            {:>4}", known_failures);
    println!("Unexpected failures       {:>4}", unexpected_failures);
    println!("Unexpected passes         {:>4}", unexpected_passes);

    assert_eq!(
        unexpected_failures + unexpected_passes,
        0,
        "criterion-level baseline mismatch detected — see the printed report"
    );
}
