//! Native customization inventory. Native files remain authoritative; listing never
//! executes an Agent, connects an MCP server, or rewrites configuration.
mod files;
mod inventory;
mod packages;
mod skills;

use crate::{
    domain::Agent,
    error::{other, Result},
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub use inventory::location_revision;
pub use inventory::{detail, list, remove_mcp, save_mcp};
pub use packages::package_action;
pub use skills::set_skill_state;
pub use skills::{install_skill, link_skill, remove_skill};

pub const AGENTS: [Agent; 4] = [
    Agent::Codex,
    Agent::ClaudeCode,
    Agent::Pi,
    Agent::Antigravity,
];
pub(crate) static MUTATION: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Scope {
    pub project_id: Option<String>,
    pub workspace_path_id: Option<String>,
}

/// Environment is captured once, never changed for an operation or a test.
#[derive(Clone)]
pub struct Roots {
    pub home: PathBuf,
    pub codex: PathBuf,
    pub claude: PathBuf,
    pub pi: PathBuf,
}

impl Roots {
    pub fn current() -> Result<Self> {
        let home =
            crate::platform::paths::resolve_home().ok_or_else(|| other("无法确定用户目录"))?;
        let override_or = |key: &str, default: PathBuf| {
            std::env::var_os(key)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or(default)
        };
        Ok(Self {
            codex: override_or("CODEX_HOME", home.join(".codex")),
            claude: override_or("CLAUDE_CONFIG_DIR", home.join(".claude")),
            pi: override_or("PI_CODING_AGENT_DIR", home.join(".pi/agent")),
            home,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Location {
    pub id: String,
    pub kind: String,
    pub agent: Option<Agent>,
    pub scope: String,
    pub path: String,
    pub label: String,
    pub writable: bool,
    pub package: Option<String>,
    pub package_enabled: Option<bool>,
    #[serde(skip)]
    pub section: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Resource {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub description: String,
    pub path: String,
    pub locations: Vec<String>,
    pub entry_paths: Vec<String>,
    pub enabled: Option<bool>,
    pub revision: String,
    pub managed: bool,
    pub error: Option<String>,
    pub skill_controls: Vec<SkillControl>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillControl {
    pub agent: Agent,
    pub scope: String,
    pub enabled: Option<bool>,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Inventory {
    pub resources: Vec<Resource>,
    pub locations: Vec<Location>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResourceDetail {
    pub resource: Resource,
    pub content: String,
    pub config: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OperationResult {
    pub message: String,
}

pub(crate) fn key(value: impl AsRef<[u8]>) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(value.as_ref()))
}

pub(crate) fn location(
    kind: &str,
    agent: Option<Agent>,
    scope: &str,
    path: PathBuf,
    label: &str,
) -> Location {
    let path = path.to_string_lossy().into_owned();
    Location {
        id: key(format!("{kind}:{agent:?}:{scope}:{path}")),
        kind: kind.into(),
        agent,
        scope: scope.into(),
        path,
        label: label.into(),
        writable: true,
        package: None,
        section: Vec::new(),
        package_enabled: None,
    }
}

pub(crate) fn check_revision(actual: &str, expected: &str) -> Result<()> {
    if actual != expected {
        return Err(other("配置已被修改，请刷新后重试"));
    }
    Ok(())
}
