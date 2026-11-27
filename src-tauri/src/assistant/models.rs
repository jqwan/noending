use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct AssistantFile {
    pub content: String,
    pub revision: Option<String>,
    pub state: &'static str,
}

#[derive(Serialize)]
pub(crate) struct AssistantDirectoryEntry {
    pub id: uuid::Uuid,
    pub name: String,
}

#[derive(Serialize)]
pub(crate) struct AssistantCard {
    pub id: uuid::Uuid,
    pub name: String,
    pub intro: String,
}

#[derive(Serialize)]
pub(crate) struct AssistantPrivateProfile {
    pub id: uuid::Uuid,
    pub files: ProjectAssistant,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectAssistant {
    pub guide_is_default: bool,
    pub claude_md_missing: bool,
    pub agents: AssistantFile,
    pub soul: AssistantFile,
    pub role: AssistantFile,
    pub user: AssistantFile,
    pub intro: AssistantFile,
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
    Agents { content: String },
    Soul { content: String },
    Role { content: String },
}
