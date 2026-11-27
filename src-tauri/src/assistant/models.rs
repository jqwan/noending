use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct AssistantFile {
    pub content: String,
    pub revision: Option<String>,
    pub state: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectAssistant {
    pub guide_state: &'static str,
    pub claude_md_missing: bool,
    pub soul: AssistantFile,
    pub role: AssistantFile,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectAssistantDetail {
    #[serde(flatten)]
    pub files: ProjectAssistant,
    pub id: uuid::Uuid,
    pub workspace_path_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssistantCreate {
    pub name: String,
    pub soul_md: Option<String>,
    pub role_md: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "file", rename_all = "snake_case", deny_unknown_fields)]
pub enum AssistantPatch {
    Soul { content: String },
    Role { content: String },
}
