//! Default instructions live in `prompts/summary` and are embedded at build time.
//! Settings store only user overrides; `summary` assembles runtime requirements.
use crate::error::{other, Result};
use crate::storage::Db;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

const SETTINGS_KEY: &str = "summary_prompt_templates";
const SESSION_REQUIREMENTS: &str = include_str!("../../prompts/summary/session.md");
const TASK_REQUIREMENTS: &str = include_str!("../../prompts/summary/task.md");
const PROJECT_REQUIREMENTS: &str = include_str!("../../prompts/summary/project.md");
const SUMMARY_SYNTHESIS: &str = include_str!("../../prompts/summary/synthesis.md");
const SUMMARY_LAYOUT: &str = include_str!("../../prompts/summary/layout.md");
const PREFERENCE_REQUIREMENTS: &str = include_str!("../../prompts/summary/preferences.md");

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptKind {
    Session,
    Task,
    Project,
    Preferences,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptTemplates {
    pub session: String,
    pub task: String,
    pub project: String,
    pub preferences: String,
}
impl Default for PromptTemplates {
    fn default() -> Self {
        Self {
            session: default_summary(SESSION_REQUIREMENTS),
            task: default_summary(TASK_REQUIREMENTS),
            project: default_summary(PROJECT_REQUIREMENTS),
            preferences: PREFERENCE_REQUIREMENTS.trim_end().into(),
        }
    }
}
fn default_summary(requirements: &str) -> String {
    [requirements, SUMMARY_SYNTHESIS, SUMMARY_LAYOUT]
        .map(str::trim_end)
        .join("\n")
}
impl PromptTemplates {
    pub fn validate(&self) -> Result<()> {
        for text in [&self.session, &self.task, &self.project, &self.preferences] {
            if text.trim().is_empty() {
                return Err(other("摘要要求模板不能为空"));
            }
        }
        Ok(())
    }
    fn get(&self, kind: PromptKind) -> &str {
        match kind {
            PromptKind::Session => &self.session,
            PromptKind::Task => &self.task,
            PromptKind::Project => &self.project,
            PromptKind::Preferences => &self.preferences,
        }
    }
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Overrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preferences: Option<String>,
}
impl Overrides {
    fn effective(self) -> PromptTemplates {
        let default = PromptTemplates::default();
        PromptTemplates {
            session: self.session.unwrap_or(default.session),
            task: self.task.unwrap_or(default.task),
            project: self.project.unwrap_or(default.project),
            preferences: self.preferences.unwrap_or(default.preferences),
        }
    }
}
#[derive(Serialize)]
pub struct PromptSettings {
    pub templates: PromptTemplates,
    pub defaults: PromptTemplates,
}
fn read(conn: &Connection) -> Result<Overrides> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key=?1",
            [SETTINGS_KEY],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|s| serde_json::from_str(&s).map_err(Into::into))
        .transpose()
        .map(|value| value.unwrap_or_default())
}
pub fn templates(db: &Db) -> Result<PromptTemplates> {
    let templates = read(&db.read())?.effective();
    templates.validate()?;
    Ok(templates)
}
pub fn settings(db: &Db) -> Result<PromptSettings> {
    Ok(PromptSettings {
        templates: templates(db)?,
        defaults: PromptTemplates::default(),
    })
}
/// One setting entry stores only custom instructions, never copies of defaults.
/// Read/modify/write in a transaction prevents two editors overwriting each other.
pub fn save(db: &Db, kind: PromptKind, template: Option<&str>) -> Result<PromptSettings> {
    let template = template.map(str::trim);
    if template.is_some_and(str::is_empty) {
        return Err(other("摘要要求模板不能为空"));
    }
    db.tx(|tx| {
        let mut overrides = read(tx)?;
        let default = PromptTemplates::default();
        let custom = template.filter(|text| *text != default.get(kind)).map(str::to_owned);
        match kind {
            PromptKind::Session => overrides.session = custom,
            PromptKind::Task => overrides.task = custom,
            PromptKind::Project => overrides.project = custom,
            PromptKind::Preferences => overrides.preferences = custom,
        }
        if overrides.session.is_none() && overrides.task.is_none() && overrides.project.is_none() && overrides.preferences.is_none() {
            tx.execute("DELETE FROM settings WHERE key=?1", [SETTINGS_KEY])?;
        } else {
            tx.execute(
                "INSERT INTO settings(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=?2",
                params![SETTINGS_KEY, serde_json::to_string(&overrides)?],
            )?;
        }
        Ok(PromptSettings { templates: overrides.effective(), defaults: default })
    })
}
