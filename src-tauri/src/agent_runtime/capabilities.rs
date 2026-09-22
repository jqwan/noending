//! Which runtime fields each Agent can override, and how.
//!
//! Capabilities are static knowledge about the Agent CLIs — they never
//! contain user configuration, and discovery results must not be conflated
//! with them (`ModelCatalog` is advisory UI help; this is the contract the
//! backend enforces).

use serde::Serialize;

use crate::domain::Agent;
use crate::error::{other, Result};

use super::AgentRuntimeOverrides;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeFieldCapability {
    /// NoEnding must neither offer nor accept an override for this field.
    Unsupported,
    /// Can be set explicitly, but candidates cannot be reliably listed.
    FreeForm,
    /// NoEnding has suggested values; completeness is not guaranteed.
    Suggested,
    /// Candidates can be fetched from the Agent dynamically.
    Discoverable,
}

impl RuntimeFieldCapability {
    pub fn supports_override(self) -> bool {
        !matches!(self, RuntimeFieldCapability::Unsupported)
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct AgentRuntimeCapabilities {
    pub model: RuntimeFieldCapability,
    pub provider: RuntimeFieldCapability,
    pub effort: RuntimeFieldCapability,
}

pub fn capabilities_of(agent: Agent) -> AgentRuntimeCapabilities {
    use RuntimeFieldCapability::{Discoverable, Suggested, Unsupported};
    match agent {
        // codex exec -m <model> -c model_reasoning_effort=<effort>
        Agent::Codex => AgentRuntimeCapabilities {
            model: Discoverable,
            provider: Unsupported,
            effort: Suggested,
        },
        // claude -p --model <model> --effort <level>; no provider switch
        Agent::ClaudeCode => AgentRuntimeCapabilities {
            model: Suggested,
            provider: Unsupported,
            effort: Suggested,
        },
        // pi -p --provider <p> --model <m> --thinking <level>
        Agent::Pi => AgentRuntimeCapabilities {
            model: Discoverable,
            provider: Discoverable,
            effort: Suggested,
        },
        // Qoder is IDE-hosted: no CLI, so there is no argument to override
        // anything with. Every field must read as unsupported, otherwise the
        // Settings UI would offer a control that can never take effect.
        Agent::Qoder => AgentRuntimeCapabilities {
            model: Unsupported,
            provider: Unsupported,
            effort: Unsupported,
        },
        // AutoClaw's own `--model` / `--thinking` flags exist, but NoEnding
        // cannot launch it, so it must not offer an override either.
        Agent::AutoClaw => AgentRuntimeCapabilities {
            model: Unsupported,
            provider: Unsupported,
            effort: Unsupported,
        },
    }
}

/// Reject overrides the Agent cannot receive.
///
/// Without this, a user could believe they were running configuration A
/// while the Agent silently never saw the argument.
pub fn validate_runtime_overrides(agent: Agent, overrides: &AgentRuntimeOverrides) -> Result<()> {
    let caps = capabilities_of(agent);
    let o = overrides.normalized();

    for (field, capability, value) in [
        ("model", caps.model, o.model.as_deref()),
        ("provider", caps.provider, o.provider.as_deref()),
        ("effort", caps.effort, o.effort.as_deref()),
    ] {
        if value.is_some() && !capability.supports_override() {
            return Err(other(format!(
                "{} 不支持覆盖 {}，该配置不会被传给 Agent CLI",
                agent.display_name(),
                field
            )));
        }
    }
    Ok(())
}
