use crate::{agent_runtime::AgentRuntimeOverrides, domain::*};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantProfile {
    pub id: String,
    pub name: String,
    pub avatar: String,
    pub description: String,
    pub instructions: String,
    pub agent: Agent,
    pub runtime_override: Option<AgentRuntimeOverrides>,
    pub workspace_path_id: String,
    pub visibility: String,
    pub revision: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantCard {
    pub id: String,
    pub name: String,
    pub avatar: String,
    pub description: String,
    pub agent: Agent,
    pub runtime_override: Option<AgentRuntimeOverrides>,
    pub visibility: String,
    pub revision: i64,
    pub directory: String,
    pub project_id: String,
    pub directory_exists: bool,
    pub session_count: i64,
    pub project_count: i64,
    pub task_count: i64,
    pub last_activity_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantDetail {
    pub profile: AssistantProfile,
    pub project: Project,
    pub workspace_path: WorkspacePath,
    pub projects: Vec<Project>,
    pub tasks: Vec<Workstream>,
    pub session_count: i64,
    pub last_activity_at: Option<String>,
    pub instructions_state: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AssistantListQuery {
    pub query: Option<String>,
    pub scope: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantPage<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    pub end: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssistantCreate {
    pub name: String,
    pub agent: String,
    #[serde(default)]
    pub avatar: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub instructions: String,
    pub runtime_override: Option<AgentRuntimeOverrides>,
}

fn present_nullable<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Option<AgentRuntimeOverrides>>, D::Error> {
    Option::<AgentRuntimeOverrides>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AssistantPatch {
    pub name: Option<String>,
    pub avatar: Option<String>,
    pub description: Option<String>,
    pub instructions: Option<String>,
    pub agent: Option<String>,
    #[serde(
        default,
        deserialize_with = "present_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub runtime_override: Option<Option<AgentRuntimeOverrides>>,
}
