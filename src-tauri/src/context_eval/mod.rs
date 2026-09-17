//! Context Evaluation Harness — reusable infrastructure for measuring
//! context extraction and domain evolution quality over a versioned corpus
//! (`tests/fixtures/context_quality/`).
//!
//! Consumers:
//! - `tests/context_quality_test.rs` — every CI run, heuristic extractor
//! - a future `context-eval` binary — manual / release-gate runs with CLI
//!   extractors. The engine accepts any `&dyn ContextExtractor` and knows
//!   nothing about Codex / Claude / Pi configuration, CLI installation
//!   discovery, models, argument parsing or console rendering; all of that
//!   lives with the caller.
//!
//! Dependency direction is one-way: eval harness → production (extractor,
//! merge engine, core resolver). Production never depends on the harness.
//!
//! Two layers over one shared corpus; domain correctness and extraction
//! intelligence are measured separately:
//!
//! Layer A — Domain Golden (deterministic):
//!   recorded_model_output
//!          ↓ parse_mutations (ref validation, authority derivation, schema checks)
//!          ↓ MergeEngine (AuthorityPolicy, supersede, conflict)
//!          ↓ exact authoritative domain state (`gold`)
//!   It proves the domain is correct *given a legal extractor output*. It
//!   says nothing about whether a real extractor would find the right facts.
//!   Layer A stays exact: fixed input justifies exact titles and lineage.
//!
//! Layer B — Extraction Eval (a real extractor):
//!   input.events + candidate workstreams + existing items
//!          ↓ ContextExtractor::extract
//!          ↓ same MergeEngine
//!          ↓ structural gold (`extractor_gold`)
//!   Layer B stays semantic: expectations are named checks over required /
//!   forbidden mutations (op family, routing, kind, evidence citations) and
//!   required / forbidden effective context — never exact wording or
//!   mutation counts, so a model upgrade must not produce waves of false
//!   failures.
//!
//! Criterion-level regression guard:
//!   Every extractor_gold expectation has a stable check id; families are
//!   `required:*`, `forbidden:*`, `required_context:*`,
//!   `forbidden_context:*`, `item_status:*` (end state of non-core items
//!   such as todos, which never enter the core projection), and the implicit
//!   `conflicts:count`. A fixture's `heuristic_expected_failed_checks`
//!   records the exact set of checks the extractor is known to fail.
//!   CI enforces set equality via `EvalCaseResult::baseline_diff`:
//!     fewer failures   → unexpected improvement → red (remove the check
//!                        from the baseline in the same commit)
//!     more failures    → regression              → red (fix the extractor
//!                        or consciously re-baseline)
//!     same failure set → green
//!   A known-bad case can no longer silently degrade, and partial progress
//!   is visible in the baseline instead of hiding behind a per-case xfail.

pub mod corpus;
pub mod evaluator;

pub use corpus::{
    load_fixture, ContextItemExpectation, ContextQualityFixture, ExpectedCoreItem,
    ExpectedMutation, ExtractorGold, FixtureEvent, FixtureGold, FixtureInitialItem, FixtureInput,
    FixtureSession, FixtureWorkstream, ItemStatusExpectation, StructuralMutationExpectation,
    CONFLICTS_CHECK_ID,
};
pub use evaluator::{
    evaluate_extractor, run_domain_golden, setup_fixture, BaselineDiff, CheckResult,
    EvalCaseResult, EvalReport, FixtureEnv,
};
