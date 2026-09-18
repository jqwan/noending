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

/// Retired Assistant runtime keys. `assistant.agent` is NOT part of this:
/// which Agent the built-in Assistant runs on is a product choice, while
/// model / provider / effort are now owned by Settings → Agents (§14).
const LEGACY_KEYS: [&str; 3] = ["assistant.model", "assistant.provider", "assistant.effort"];

/// One-shot migration of the retired Assistant runtime keys (§8): read once,
/// keep what the Agent can actually receive, write it as that Agent's
/// override, then drop the legacy keys so they carry no meaning any more.
///
/// A row the runtime settings already own always wins, and there is no
/// double write: after this run the legacy keys are gone. Returns the Agent
/// whose overrides were created, so callers can report an actual migration.
pub fn migrate_legacy_assistant_runtime(db: &Db) -> Result<Option<Agent>> {
    let legacy = AgentRuntimeOverrides {
        model: db.get_setting(LEGACY_KEYS[0])?,
        provider: db.get_setting(LEGACY_KEYS[1])?,
        effort: db.get_setting(LEGACY_KEYS[2])?,
    };
    let mut migrated = None;
    if let Some(agent) = db
        .get_setting("assistant.agent")?
        .and_then(|s| Agent::parse(&s))
    {
        let caps = super::capabilities_of(agent);
        let keep = |v: Option<String>, cap: super::RuntimeFieldCapability| {
            if cap == super::RuntimeFieldCapability::Unsupported {
                None
            } else {
                v
            }
        };
        let candidate = AgentRuntimeOverrides {
            model: keep(legacy.model.clone(), caps.model),
            provider: keep(legacy.provider.clone(), caps.provider),
            effort: keep(legacy.effort.clone(), caps.effort),
        };
        if !candidate.is_default() && db.get_setting(&key(agent))?.is_none() {
            set_runtime_overrides(db, agent, &candidate)?;
            migrated = Some(agent);
        }
    }
    for k in LEGACY_KEYS {
        db.delete_setting(k)?;
    }
    Ok(migrated)
}
