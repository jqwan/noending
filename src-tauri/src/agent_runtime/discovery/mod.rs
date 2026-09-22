//! Model Discovery — advisory only.
//!
//! Discovery exists to help the user pick an override. It never participates
//! in default resolution: an Agent left at *Agent default* launches fine even
//! when every discovery call in this module fails. Consequently discovery
//! returns a value, not a `Result`, and every failure path folds into
//! `warnings` + `ModelCatalog::Unavailable`.

pub mod claude;
pub mod codex;
pub mod pi;

use serde::Serialize;

use crate::adapters::AgentCommand;
use crate::domain::Agent;
use crate::error::Result;

use super::AgentRuntimeCapabilities;

/// Discovery must never block a launch or a Settings render for long.
pub const DISCOVERY_TIMEOUT_SECS: u64 = 20;

#[derive(Debug, Clone, Serialize)]
pub struct ModelOption {
    pub id: String,
    pub display_name: Option<String>,
    /// Pi only; Codex and Claude Code have no provider dimension.
    pub provider: Option<String>,
    pub supported_efforts: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum ModelCatalog {
    /// Fetched from the Agent CLI, so it reflects what this install offers.
    Dynamic(Vec<ModelOption>),
    /// NoEnding's own suggestions; explicitly NOT the account's full model list.
    Suggested(Vec<ModelOption>),
    Unavailable,
}

impl ModelCatalog {
    pub fn options(&self) -> Vec<ModelOption> {
        match self {
            ModelCatalog::Dynamic(v) | ModelCatalog::Suggested(v) => v.clone(),
            ModelCatalog::Unavailable => vec![],
        }
    }

    /// Stable tag for the UI DTO: the user must be able to tell a real catalog
    /// from a suggestion list from a failure.
    pub fn source(&self) -> &'static str {
        match self {
            ModelCatalog::Dynamic(_) => "dynamic",
            ModelCatalog::Suggested(_) => "suggested",
            ModelCatalog::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentRuntimeDiscovery {
    pub capabilities: AgentRuntimeCapabilities,
    /// Catalog entries flattened for the UI; `model_source` says what they are.
    pub models: Vec<ModelOption>,
    /// `dynamic` | `suggested` | `unavailable` — the UI must not present a
    /// suggestion list as if it were the Agent's real catalog.
    pub model_source: &'static str,
    /// Effort levels the CLI documents, independent of any model override.
    pub effort_levels: Vec<String>,
    pub warnings: Vec<String>,
}

/// What the user may set for one Agent: capabilities (contract), a model
/// catalog (advice), and the effort vocabulary.
///
/// Timing log (方案 §17): agent + duration + outcome only — never CLI output,
/// tokens, credentials or environment. Slow CLIs show up as long durations.
pub fn discover_runtime_options(agent: Agent) -> AgentRuntimeDiscovery {
    let started = std::time::Instant::now();
    eprintln!("[agent-runtime] {} discovery started", agent.as_str());
    let capabilities = super::capabilities_of(agent);
    let effort_levels = effort_levels_for(agent);
    let (models, warnings) = match agent {
        Agent::Codex => codex::discover(),
        Agent::ClaudeCode => claude::discover(),
        Agent::Pi => pi::discover(),
        // No CLI to ask. `Unavailable` is the honest answer and the UI already
        // renders it as "无法获取模型列表" rather than as a broken install.
        Agent::Qoder => (ModelCatalog::Unavailable, vec![]),
    };
    let model_source = models.source();
    if model_source == "unavailable" {
        eprintln!(
            "[agent-runtime] {} discovery failed after {}ms",
            agent.as_str(),
            started.elapsed().as_millis()
        );
    } else {
        eprintln!(
            "[agent-runtime] {} discovery completed in {}ms",
            agent.as_str(),
            started.elapsed().as_millis()
        );
    }
    AgentRuntimeDiscovery {
        capabilities,
        models: models.options(),
        model_source,
        effort_levels,
        warnings,
    }
}

/// Effort vocabulary the CLI documents. Static on purpose: the effort picker
/// must work even when the model catalog could not be fetched.
pub fn effort_levels_for(agent: Agent) -> Vec<String> {
    let levels: &[&str] = match agent {
        Agent::Codex => codex::EFFORT_LEVELS,
        Agent::ClaudeCode => claude::EFFORT_LEVELS,
        Agent::Pi => pi::EFFORT_LEVELS,
        Agent::Qoder => &[],
    };
    levels.iter().map(|s| s.to_string()).collect()
}

/// Run a read-only CLI probe. Failures stay failures: the caller decides how
/// to degrade (warning + Unavailable), never the launcher.
pub(crate) fn run_cli(agent: Agent, args: &[&str]) -> Result<String> {
    let install = crate::platform::exec_resolver::resolve(agent)?;
    let cmd = AgentCommand {
        program: install.executable_path.clone(),
        args: args.iter().map(|s| s.to_string()).collect(),
        cwd: None, // probes read the Agent's own global config, not a repo
    };
    let out = crate::platform::exec_runner::run_headless(&cmd, DISCOVERY_TIMEOUT_SECS)?;
    Ok(out.stdout)
}

pub(crate) fn warn(text: impl ToString) -> Vec<String> {
    vec![text.to_string()]
}
