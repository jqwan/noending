use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct ProjectAssistant {
    pub agents_md: String,
    pub revision: Option<String>,
    pub agents_md_state: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectAssistantDetail {
    #[serde(flatten)]
    pub file: ProjectAssistant,
    pub workspace_path_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssistantCreate {
    pub name: String,
    #[serde(default)]
    pub agents_md: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssistantPatch {
    pub agents_md: String,
}
