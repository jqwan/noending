//! The data command surface is shared by CLI and MCP; no desktop is needed
//! until a caller requests a new native terminal.
use super::*;
use crate::error::Result;
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssistantId {
    pub id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssistantUpdate {
    pub id: String,
    pub revision: i64,
    pub patch: AssistantPatch,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssistantResponsibilities {
    pub id: String,
    pub revision: i64,
    pub project_ids: Vec<String>,
    pub task_ids: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssistantArchive {
    pub id: String,
    pub revision: i64,
    pub archived: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssistantPrepare {
    pub id: String,
    pub revision: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssistantSessions {
    pub id: String,
    #[serde(default)]
    pub query: AssistantListQuery,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssistantForSession {
    pub session: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssistantStart {
    pub id: String,
    #[serde(default)]
    pub task_ids: Vec<String>,
    pub initial_message: Option<String>,
    pub runtime_override: Option<crate::agent_runtime::AgentRuntimeOverrides>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum AssistantCommand {
    List(AssistantListQuery),
    Get(AssistantId),
    Create(AssistantCreate),
    Update(AssistantUpdate),
    Responsibilities(AssistantResponsibilities),
    Archive(AssistantArchive),
    Prepare(AssistantPrepare),
    Sessions(AssistantSessions),
    ForSession(AssistantForSession),
}
impl AssistantCommand {
    pub(crate) fn is_write(&self) -> bool {
        !matches!(
            self,
            Self::List(_) | Self::Get(_) | Self::Sessions(_) | Self::ForSession(_)
        )
    }
}
pub(crate) fn dispatch_command(
    db: &crate::storage::Db,
    home: &crate::workspace::home::NoEndingHome,
    layer: &crate::workspace::wiring::WorkspaceLayer,
    command: AssistantCommand,
) -> Result<Value> {
    use AssistantCommand::*;
    Ok(match command {
        List(query) => serde_json::to_value(list_profiles(db, query)?)?,
        Get(p) => serde_json::to_value(detail(db, &p.id)?)?,
        Create(p) => serde_json::to_value(create_profile(db, home, layer, p)?)?,
        Update(p) => serde_json::to_value(update_profile(db, home, &p.id, p.revision, p.patch)?)?,
        Responsibilities(p) => serde_json::to_value(set_responsibilities(
            db,
            home,
            &p.id,
            p.revision,
            &p.project_ids,
            &p.task_ids,
        )?)?,
        Archive(p) => serde_json::to_value(set_archived(db, home, &p.id, p.revision, p.archived)?)?,
        Prepare(p) => serde_json::to_value(prepare_profile_files(db, home, &p.id, p.revision)?)?,
        Sessions(p) => serde_json::to_value(list_profile_sessions(db, &p.id, p.query)?)?,
        ForSession(p) => {
            db.get_session(&p.session)?
                .ok_or_else(|| crate::error::other("会话不存在"))?;
            let profile =
                crate::storage::assistant_profiles::profile_for_session(&db.read(), &p.session)?;
            json!({"session_id":p.session,"assistant":profile.map(|p|p.id).map(|id|detail(db,&id)).transpose()?})
        }
    })
}
