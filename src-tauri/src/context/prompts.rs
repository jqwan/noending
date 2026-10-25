//! Built-in summary requirements shared by delegated and current-session extraction.
use serde::{Deserialize, Serialize};

const SESSION_REQUIREMENTS: &str = include_str!("../../prompts/summary/session.md");
const TASK_REQUIREMENTS: &str = include_str!("../../prompts/summary/task.md");
const PROJECT_REQUIREMENTS: &str = include_str!("../../prompts/summary/project.md");
const SUMMARY_SYNTHESIS: &str = include_str!("../../prompts/summary/synthesis.md");
const SUMMARY_LAYOUT: &str = include_str!("../../prompts/summary/layout.md");

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptTemplates {
    pub session: String,
    pub task: String,
    pub project: String,
}
impl Default for PromptTemplates {
    fn default() -> Self {
        Self {
            session: default_summary(SESSION_REQUIREMENTS),
            task: default_summary(TASK_REQUIREMENTS),
            project: default_summary(PROJECT_REQUIREMENTS),
        }
    }
}
fn default_summary(requirements: &str) -> String {
    [requirements, SUMMARY_SYNTHESIS, SUMMARY_LAYOUT]
        .map(str::trim_end)
        .join("\n")
}
