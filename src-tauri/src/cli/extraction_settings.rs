//! Partial edits reuse the same extraction configuration validation as the UI.
use super::{CliError, CliResult};
use crate::{domain::Agent, storage::Db, sync::extractor::ExtractionSettings};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Patch {
    pub agent: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub effort: Option<String>,
}
impl Patch {
    pub fn is_empty(&self) -> bool {
        self.agent.is_none()
            && self.model.is_none()
            && self.provider.is_none()
            && self.effort.is_none()
    }
}
pub(super) fn update(db: &Db, patch: Patch) -> CliResult<ExtractionSettings> {
    if patch.is_empty() {
        return Err(CliError::new(
            "invalid_arguments",
            "需要提供 Agent、模型、提供方或推理强度",
        ));
    }
    let mut settings = ExtractionSettings::from_settings(db)?;
    if let Some(agent) = patch.agent {
        let agent = if agent == "none" {
            agent
        } else {
            Agent::parse(&agent)
                .ok_or_else(|| CliError::new("invalid_arguments", "未知的 Agent"))?
                .as_str()
                .to_string()
        };
        let previous = Agent::parse(&settings.agent)
            .map(|a| a.as_str())
            .unwrap_or(&settings.agent);
        if agent != previous {
            settings.runtime = Default::default();
        }
        settings.agent = agent;
    }
    if let Some(value) = patch.model {
        settings.runtime.model = Some(value);
    }
    if let Some(value) = patch.provider {
        settings.runtime.provider = Some(value);
    }
    if let Some(value) = patch.effort {
        settings.runtime.effort = Some(value);
    }
    if settings.agent == "none" && !settings.runtime.normalized().is_default() {
        return Err(CliError::new(
            "invalid_arguments",
            "停用提取时不能指定运行参数",
        ));
    }
    settings
        .exec_options()
        .map_err(|error| CliError::new("invalid_arguments", error))?;
    Ok(settings.save(db)?)
}
