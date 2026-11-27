use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct AgentFile {
    pub content: String,
    pub revision: Option<String>,
    pub state: &'static str,
}

#[derive(Serialize)]
pub(crate) struct Agent {
    pub id: uuid::Uuid,
    pub name: String,
}

#[derive(Serialize)]
pub(crate) struct AgentCard {
    #[serde(flatten)]
    pub agent: Agent,
    pub intro: String,
}

#[derive(Serialize)]
pub(crate) struct AgentPrivateProfile {
    pub id: uuid::Uuid,
    pub files: ProjectAgent,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectAgent {
    pub guide_is_default: bool,
    pub claude_md_missing: bool,
    pub agents: AgentFile,
    pub soul: AgentFile,
    pub role: AgentFile,
    pub user: AgentFile,
    pub intro: AgentFile,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectAgentDetail {
    #[serde(flatten)]
    pub files: ProjectAgent,
    pub id: uuid::Uuid,
    pub workspace_path_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCreate {
    pub name: String,
    pub soul_md: Option<String>,
    pub role_md: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "file", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentPatch {
    Agents { content: String },
    Soul { content: String },
    Role { content: String },
}
