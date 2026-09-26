//! Model discovery.
//!
//! The unit-level parsing lives beside the code (`agent_runtime::discovery::*`);
//! what this file pins is the contract between discovery and launch.

use noending::agent_runtime::{capabilities_of, RuntimeFieldCapability};
use noending::domain::Agent;

#[test]
fn capabilities_are_the_backend_contract_not_the_discovery_result() {
    // a field the CLI cannot receive must be Unsupported here, because
    // this — not the UI — is what rejects the override.
    let codex = capabilities_of(Agent::Codex);
    assert_eq!(codex.model, RuntimeFieldCapability::Discoverable);
    assert_eq!(codex.provider, RuntimeFieldCapability::Unsupported);

    let claude = capabilities_of(Agent::ClaudeCode);
    assert_eq!(claude.model, RuntimeFieldCapability::Suggested);
    assert_eq!(claude.provider, RuntimeFieldCapability::Unsupported);

    let pi = capabilities_of(Agent::Pi);
    assert_eq!(pi.provider, RuntimeFieldCapability::Discoverable);
    for caps in [codex, claude, pi] {
        assert!(caps.effort.supports_override());
    }
}

/// Exercises the real CLIs, so it is ignored by default:
///   cargo test --test agent_runtime_discovery_test -- --ignored --nocapture
#[test]
#[ignore]
fn real_agents_answer_discovery_or_warn() {
    // Only Agents with a CLI can be asked anything: a history-only Agent has
    // no catalog and no effort vocabulary to report.
    for agent in Agent::all()
        .iter()
        .copied()
        .filter(|a| !noending::platform::exec_resolver::cli_names(*a).is_empty())
    {
        let d = noending::agent_runtime::discover_runtime_options(agent);
        println!(
            "{:?}: source={} models={} efforts={:?} warnings={:?}",
            agent,
            d.model_source,
            d.models.len(),
            d.effort_levels,
            d.warnings
        );
        assert!(!d.effort_levels.is_empty(), "effort vocabulary is static");
        assert!(
            !d.models.is_empty() || !d.warnings.is_empty(),
            "{agent:?} discovery must never come back silently empty"
        );
        for m in &d.models {
            assert!(!m.id.is_empty());
            assert_eq!(m.provider.is_some(), agent == Agent::Pi);
        }
    }
}

/// The other half of that contract: an Agent with no CLI answers
/// `unavailable` instead of probing (and must not panic doing it).
#[test]
fn agents_without_a_cli_report_an_unavailable_catalog() {
    for agent in Agent::all()
        .iter()
        .copied()
        .filter(|a| noending::platform::exec_resolver::cli_names(*a).is_empty())
    {
        let d = noending::agent_runtime::discover_runtime_options(agent);
        assert_eq!(d.model_source, "unavailable", "{agent:?}");
        assert!(d.models.is_empty(), "{agent:?}");
        assert!(d.effort_levels.is_empty(), "{agent:?}");
        let caps = noending::agent_runtime::capabilities_of(agent);
        assert_eq!(
            caps.model,
            noending::agent_runtime::RuntimeFieldCapability::Unsupported
        );
        assert_eq!(
            caps.provider,
            noending::agent_runtime::RuntimeFieldCapability::Unsupported
        );
        assert_eq!(
            caps.effort,
            noending::agent_runtime::RuntimeFieldCapability::Unsupported
        );
    }
}
