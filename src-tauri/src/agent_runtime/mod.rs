//! Agent Runtime Configuration.
//!
//! Invariant: **Agent owns defaults, NoEnding owns overrides.**
//!
//! NoEnding never parses an Agent's own configuration (`~/.codex/config.toml`,
//! `~/.claude/settings.json`, `~/.pi/…`, project config, env, profiles,
//! managed policy) and never computes an "effective default model". A field
//! left as `None` means *Agent default* and is implemented by **not passing
//! the corresponding CLI argument at all** — default is a launch intent, not
//! a resolved value.
//!
//! Model Discovery lives beside this module and is advisory only: it helps
//! the user pick an override, and it can never fail a launch or participate
//! in default resolution.

pub mod capabilities;
pub mod store;

use serde::{Deserialize, Serialize};

use crate::adapters::ExecOptions;
use crate::domain::Agent;
use crate::error::Result;
use crate::storage::Db;

pub use capabilities::{
    capabilities_of, validate_runtime_overrides, AgentRuntimeCapabilities, RuntimeFieldCapability,
};
pub use store::{get_runtime_overrides, set_runtime_overrides};

/// Explicit user overrides for one Agent. `None` = Agent default = no CLI argument.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentRuntimeOverrides {
    pub model: Option<String>,
    pub provider: Option<String>,
    pub effort: Option<String>,
}

fn clean(v: Option<&str>) -> Option<String> {
    v.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

impl AgentRuntimeOverrides {
    /// Drop blank values so `Some("")` can never mean anything other than
    /// *Agent default* on either side of the storage boundary.
    pub fn normalized(&self) -> AgentRuntimeOverrides {
        AgentRuntimeOverrides {
            model: clean(self.model.as_deref()),
            provider: clean(self.provider.as_deref()),
            effort: clean(self.effort.as_deref()),
        }
    }

    pub fn is_default(&self) -> bool {
        let o = self.normalized();
        o.model.is_none() && o.provider.is_none() && o.effort.is_none()
    }

    /// The single conversion every consumer shares: New Session, Resume,
    /// Context Eval and the Assistant all reach the CLI through this.
    pub fn exec_options(&self) -> ExecOptions {
        let o = self.normalized();
        ExecOptions {
            model: o.model,
            provider: o.provider,
            effort: o.effort,
        }
    }
}

/// Resolve the runtime `ExecOptions` for an Agent from stored overrides.
///
/// There is no default resolution here: an unset override yields an
/// `ExecOptions` field of `None`, which adapters render as no CLI argument.
pub fn runtime_exec_options(db: &Db, agent: Agent) -> Result<ExecOptions> {
    let overrides = get_runtime_overrides(db, agent)?;
    validate_runtime_overrides(agent, &overrides)?;
    Ok(overrides.exec_options())
}
