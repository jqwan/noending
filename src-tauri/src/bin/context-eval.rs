//! context-eval — run the context quality corpus through a real extractor.
//!
//! This binary owns everything the `context_eval` engine deliberately does
//! not: extractor construction (`--agent/--model/...` → `CliExtractor`),
//! argument parsing and report rendering. The engine stays extractor-
//! agnostic; dependency direction remains harness → production.
//!
//! Modes:
//! - `--extractor heuristic` — deterministic heuristic extractor. Results
//!   are additionally compared against each fixture's recorded
//!   `heuristic_expected_failed_checks` baseline (known / unexpected).
//! - `--extractor cli` — runs the user's agent CLI headlessly through the
//!   production path (exec_resolver → adapter → headless CLI →
//!   `parse_mutations`). This is a MEASUREMENT, not a regression gate: no
//!   cross-extractor baseline exists yet, so results are shown as plain
//!   passed / failed checks and semantic failures do not affect the exit
//!   code.
//!
//! Exit codes:
//! - 0 — the eval ran to completion (even with semantic check failures)
//! - 1 — infrastructure failure: bad arguments, unreadable corpus or
//!   fixtures, extractor invocation failure (missing CLI, timeout,
//!   invalid output)
//!
//! Case execution order is deterministic (corpus files sorted by path) so
//! two reports remain diffable despite model nondeterminism.

use std::path::{Path, PathBuf};

use noending::agent_runtime::{validate_runtime_overrides, AgentRuntimeOverrides};
use noending::context_eval::{
    evaluate_extractor, load_fixture, BaselineDiff, ContextQualityFixture, EvalCaseResult,
};
use noending::domain::Agent;
use noending::error::{other, Result};
use noending::sync::extractor::{CliExtractor, HeuristicExtractor};
use noending::sync::ContextExtractor;

const USAGE: &str = "\
context-eval — run the context quality corpus through a real extractor

USAGE:
  cargo run --bin context-eval -- [OPTIONS]

OPTIONS:
  --extractor <heuristic|cli>     Extractor under test (default: heuristic)
  --agent <codex|claude_code|pi>  CLI agent (cli mode; default: codex)
  --model <model>                 Model override (cli mode; omit = Agent default)
  --provider <provider>           Provider override, e.g. openai-codex (cli mode)
                                  (codex/claude_code reject this)
  --effort <effort>               Reasoning effort override (cli mode)
  --corpus <path>                 Corpus directory of *.json fixtures
                                  (default: <crate>/tests/fixtures/context_quality)
  --case <name>                   Run a single fixture by name (file stem)
  -h, --help                      Show this help

NOTES:
  cli mode reads its runtime intent from these flags only — never from the
  app's Settings — so a report is reproducible. With no --model/--provider/
  --effort, NoEnding passes no runtime flag at all and the Agent's own
  defaults apply; the extractor then reports as `cli:<agent>:agent-default`.
  heuristic mode compares results against each fixture's recorded
  heuristic_expected_failed_checks baseline and reports known failures and
  unexpected passes/failures.

  cli mode is a pure measurement: no baseline comparison exists yet, so
  semantic check failures still exit 0. Exit 1 is reserved for
  infrastructure errors: bad arguments, unreadable corpus or fixtures,
  extractor invocation failure (missing CLI, timeout, invalid output).";

const DEFAULT_CORPUS: &str = "tests/fixtures/context_quality";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtractorKind {
    Heuristic,
    Cli,
}

#[derive(Debug)]
struct Args {
    extractor: ExtractorKind,
    agent: Option<String>,
    model: Option<String>,
    provider: Option<String>,
    effort: Option<String>,
    corpus: Option<PathBuf>,
    case: Option<String>,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            extractor: ExtractorKind::Heuristic,
            agent: None,
            model: None,
            provider: None,
            effort: None,
            corpus: None,
            case: None,
        }
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(&argv) {
        eprintln!("context-eval: {e}");
        std::process::exit(1);
    }
}

fn run(argv: &[String]) -> Result<()> {
    let args = parse_args(argv)?;
    let baseline_aware = args.extractor == ExtractorKind::Heuristic;
    if baseline_aware
        && (args.agent.is_some()
            || args.model.is_some()
            || args.provider.is_some()
            || args.effort.is_some())
    {
        return Err(other(
            "--agent/--model/--provider/--effort are only valid with --extractor cli",
        ));
    }

    let extractor = build_extractor(&args)?;
    let corpus_dir = args
        .corpus
        .clone()
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join(DEFAULT_CORPUS));
    let mut fixtures = load_corpus(&corpus_dir)?;

    if let Some(case) = &args.case {
        let available: Vec<String> = fixtures.iter().map(|f| f.name.clone()).collect();
        fixtures.retain(|f| &f.name == case);
        if fixtures.is_empty() {
            return Err(other(format!(
                "case '{case}' not found in corpus; available: {}",
                available.join(", ")
            )));
        }
    }

    println!("Context Quality Eval");
    println!("Extractor: {}", extractor.name());
    println!("Corpus: {}", corpus_dir.display());
    println!();

    let mut results: Vec<EvalCaseResult> = Vec::new();
    let mut diffs: Vec<BaselineDiff> = Vec::new();
    for (idx, fixture) in fixtures.iter().enumerate() {
        eprintln!("[{}/{}] {}", idx + 1, fixtures.len(), fixture.name);
        let result = evaluate_extractor(fixture, extractor.as_ref())
            .map_err(|e| other(format!("fixture '{}': {e}", fixture.name)))?;
        eprintln!(
            "    {} / {} checks passed",
            result.passed_checks(),
            result.checks.len()
        );
        if baseline_aware {
            diffs.push(result.baseline_diff(&fixture.heuristic_expected_failed_checks));
        }
        results.push(result);
    }

    if baseline_aware {
        render_baseline_report(&results, &diffs);
    } else {
        render_measurement_report(&results);
    }
    Ok(())
}

fn parse_args(argv: &[String]) -> Result<Args> {
    let mut args = Args::default();
    let mut i = 0usize;
    while i < argv.len() {
        let flag = argv[i].as_str();
        i += 1;
        match flag {
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "--extractor" => {
                let v = take_value(flag, &mut i, argv)?;
                args.extractor = match v.as_str() {
                    "heuristic" => ExtractorKind::Heuristic,
                    "cli" => ExtractorKind::Cli,
                    bad => {
                        return Err(other(format!(
                            "unknown --extractor '{bad}' (expected heuristic | cli)"
                        )))
                    }
                };
            }
            "--agent" => args.agent = Some(take_value(flag, &mut i, argv)?),
            "--model" => args.model = Some(take_value(flag, &mut i, argv)?),
            "--provider" => args.provider = Some(take_value(flag, &mut i, argv)?),
            "--effort" => args.effort = Some(take_value(flag, &mut i, argv)?),
            "--corpus" => args.corpus = Some(PathBuf::from(take_value(flag, &mut i, argv)?)),
            "--case" => args.case = Some(take_value(flag, &mut i, argv)?),
            unexpected => {
                return Err(other(format!(
                    "unexpected argument '{unexpected}' (see --help)"
                )))
            }
        }
    }
    Ok(args)
}

fn take_value(flag: &str, i: &mut usize, argv: &[String]) -> Result<String> {
    match argv.get(*i) {
        Some(v) => {
            *i += 1;
            Ok(v.clone())
        }
        None => Err(other(format!("missing value for {flag}"))),
    }
}

fn build_extractor(args: &Args) -> Result<Box<dyn ContextExtractor>> {
    match args.extractor {
        ExtractorKind::Heuristic => Ok(Box::new(HeuristicExtractor)),
        ExtractorKind::Cli => {
            // The harness measures a runtime intent, so it takes that intent
            // from its own arguments and NEVER from the user's Settings: an
            // eval result must not depend on what happens to be stored.
            let agent_name = args.agent.clone().unwrap_or_else(|| "codex".into());
            let agent = Agent::parse(&agent_name).ok_or_else(|| {
                other(format!(
                    "unknown --agent '{agent_name}' (expected codex | claude_code | pi)"
                ))
            })?;
            let overrides = AgentRuntimeOverrides {
                model: args.model.clone(),
                provider: args.provider.clone(),
                effort: args.effort.clone(),
            };
            // Same contract as launch: an override the Agent cannot receive is
            // rejected instead of silently dropped.
            validate_runtime_overrides(agent, &overrides)?;
            Ok(Box::new(CliExtractor::new(agent, overrides.exec_options())))
        }
    }
}

/// Read every `*.json` fixture in the corpus directory, sorted by path so
/// case execution order — and therefore report shape — is deterministic.
fn load_corpus(dir: &Path) -> Result<Vec<ContextQualityFixture>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| other(format!("cannot read corpus dir {}: {e}", dir.display())))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    if paths.is_empty() {
        return Err(other(format!(
            "no .json fixtures found in {}",
            dir.display()
        )));
    }
    paths
        .iter()
        .map(|path| {
            let json = std::fs::read_to_string(path)
                .map_err(|e| other(format!("cannot read fixture {}: {e}", path.display())))?;
            load_fixture(&json).map_err(|e| other(format!("fixture {}: {e}", path.display())))
        })
        .collect()
}

/// Heuristic mode: baseline-aware rendering. The fixture's recorded
/// `heuristic_expected_failed_checks` is the extractor's historical zero
/// point, so known failures and unexpected passes/failures are meaningful.
fn render_baseline_report(results: &[EvalCaseResult], diffs: &[BaselineDiff]) {
    let mut known_failures = 0usize;
    for (result, diff) in results.iter().zip(diffs.iter()) {
        let actual_failed = result.failed_check_ids();
        known_failures += actual_failed.len() - diff.unexpected_failures.len();
        let status = if !diff.is_clean() {
            "MISMATCH"
        } else if actual_failed.is_empty() {
            "PASS"
        } else {
            "FAIL (known)"
        };
        println!("[{status}] {} [{}]", result.fixture_name, result.dimension);
        if !actual_failed.is_empty() {
            let ids: Vec<String> = actual_failed.into_iter().collect();
            println!("    failed: {}", ids.join(", "));
        }
        for id in &diff.unexpected_passes {
            println!("    unexpected pass (re-baseline): {id}");
        }
        for check in &diff.unexpected_failures {
            println!("    unexpected failure: {}: {}", check.id, check.detail);
        }
        println!();
    }

    let report = noending::context_eval::EvalReport::new(results.to_vec());
    println!("-------------------------------------------------");
    summary_line("Cases", &report.cases.len().to_string());
    summary_line("Fully passed", &report.fully_passed_cases().to_string());
    println!();
    summary_line("Checks", &report.total_checks().to_string());
    summary_line("Passed", &report.passed_checks().to_string());
    summary_line("Known failures", &known_failures.to_string());
    summary_line(
        "Unexpected failures",
        &diffs
            .iter()
            .map(|d| d.unexpected_failures.len())
            .sum::<usize>()
            .to_string(),
    );
    summary_line(
        "Unexpected passes",
        &diffs
            .iter()
            .map(|d| d.unexpected_passes.len())
            .sum::<usize>()
            .to_string(),
    );
}

/// CLI mode: pure measurement. No baseline exists for a live model, so
/// "known failures" / "unexpected passes" would be meaningless — only real
/// passed / failed checks are shown.
fn render_measurement_report(results: &[EvalCaseResult]) {
    for result in results {
        let total = result.checks.len();
        let passed = result.passed_checks();
        let failed = total - passed;
        println!(
            "[{}] {}",
            if failed == 0 { "PASS" } else { "FAIL" },
            result.fixture_name
        );
        println!("    {passed} / {total} checks");
        for check in result.checks.iter().filter(|c| !c.passed) {
            println!("    ✗ {}", check.id);
            println!("      {}", check.detail);
        }
        println!();
    }

    let report = noending::context_eval::EvalReport::new(results.to_vec());
    let total = report.total_checks();
    let passed = report.passed_checks();
    let failed = total - passed;
    let rate = if total == 0 {
        0.0
    } else {
        passed as f64 / total as f64 * 100.0
    };
    println!("-------------------------------------------------");
    summary_line("Cases", &report.cases.len().to_string());
    summary_line(
        "Fully passed",
        &format!("{} / {}", report.fully_passed_cases(), report.cases.len()),
    );
    println!();
    summary_line("Checks", &total.to_string());
    summary_line("Passed", &passed.to_string());
    summary_line("Failed", &failed.to_string());
    summary_line("Pass rate", &format!("{rate:.1}%"));
}

fn summary_line(label: &str, value: &str) {
    println!("{:<20}{:>6}", label, value);
}
