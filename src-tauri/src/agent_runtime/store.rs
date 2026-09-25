//! Persistence for NoEnding-owned runtime overrides.
//!
//! Only explicit overrides are stored. The settings row is deleted once every
//! field returns to *Agent default*, so "no row" and "all fields null" mean
//! exactly the same thing. Agent native defaults and model catalogs are
//! dynamic and are never persisted here.

use crate::domain::Agent;
use crate::error::{other, Result};
use crate::storage::Db;

use super::AgentRuntimeOverrides;

const KEY_PREFIX: &str = "agent.runtime.";

fn key(agent: Agent) -> String {
    format!("{}{}", KEY_PREFIX, agent.as_str())
}

pub fn get_runtime_overrides(db: &Db, agent: Agent) -> Result<AgentRuntimeOverrides> {
    let Some(raw) = db.get_setting(&key(agent))? else {
        return Ok(AgentRuntimeOverrides::default());
    };
    let parsed: AgentRuntimeOverrides = serde_json::from_str(&raw).map_err(|e| {
        other(format!(
            "{} 的运行时配置已损坏（{}）：{}。请在 Settings → Agents 重新设置。",
            agent.display_name(),
            key(agent),
            e
        ))
    })?;
    Ok(parsed.normalized())
}

pub fn set_runtime_overrides(
    db: &Db,
    agent: Agent,
    overrides: &AgentRuntimeOverrides,
) -> Result<()> {
    super::validate_runtime_overrides(agent, overrides)?;
    let normalized = overrides.normalized();
    if normalized.is_default() {
        return db.delete_setting(&key(agent));
    }
    let raw = serde_json::to_string(&normalized)
        .map_err(|e| other(format!("运行时配置序列化失败: {}", e)))?;
    db.set_setting(&key(agent), &raw)
}
