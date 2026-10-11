//! Editable summary instructions. File/response contracts stay in `summary`.
use crate::error::{other, Result};
use crate::storage::Db;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

const SETTINGS_KEY: &str = "summary_prompt_templates";
const SESSION_REQUIREMENTS: &str = r#"摘要正文使用 Markdown，建议不超过 2000 字符（含空格与标点）。精简但保留关键事实，不凑字数，不靠直接截断压缩正文。
通常组织为一到三个短段落，每段围绕一个主要意思。优先呈现当前最重要的结论或状态，再补充必要背景、未解决问题和后续行动，不按消息时间顺序复述过程。
使用少量加粗突出关键结论、重要约束或阻塞，不整段加粗。内容复杂时可以用简短小标题，但不强制固定栏目；只有并列信息确实更适合列表时才使用列表，不机械拆成条目。
保留讨论目标、必要背景、关键结论和影响后续工作的具体信息。删除寒暄、重复内容、无关细节和过程流水账。路径、命令、配置项等用行内代码，不复制大段代码或日志。
区分用户要求、已确认事实、Agent 建议、已完成事项和待验证结果。不把计划写成完成，不把建议写成决定，不自行推断缺失的信息。
新消息明确修正或推翻旧内容时，更新对应表述；保留仍然有效的信息与重要的未解决问题，不因出现较早而遗漏。
将当前摘要与新消息重新整合为完整摘要，不简单追加。结果应独立可读，避免“同上”“之前那个方案”等依赖原文的表达。沿用会话的主要语言，保留必要的技术术语。"#;

const TASK_REQUIREMENTS: &str = "使用 Markdown，建议不超过 1000 字符（含空格与标点）。精简但保留关键事实，不凑字数。\n从任务整体呈现目标、当前进展、关键结果、有效决定与约束、未解决问题和下一步；只写有依据的内容，不固定栏目。\n结论在前，以短段落为主，适量加粗；需要时使用简短小标题或列表。综合会话成果，不逐个复述，不写过程流水账。\n已有摘要仅供参考，根据材料修正失效表述，保留有效信息。区分要求、建议、决定、计划、已完成与待验证，不虚构进展，不自行裁定未解决的分歧。沿用材料的主要语言，路径、命令和配置项用行内代码。";
const PROJECT_REQUIREMENTS: &str = "使用 Markdown，建议不超过 1000 字符（含空格与标点）。精简但保留关键事实，不凑字数。\n从项目整体呈现用途与目标、当前进展、关键成果、有效决定和约束、未解决问题及下一步；只写材料支持的内容，不固定栏目。\n结论在前，以短段落为主，适量加粗，需要时使用简短小标题或列表。综合跨目录和会话的成果，不逐个复述任务或会话，不写过程流水账。\n把已有摘要与当前材料整合为完整摘要，修正已失效的表述，保留有效信息。区分要求、建议、决定、计划、已完成与待验证，不虚构进展，不自行裁定未解决分歧。任务背景不代表任务已完成。沿用材料的主要语言，路径、命令和配置项用行内代码。";

const PREFERENCE_REQUIREMENTS: &str = r#"在生成摘要的同时，检查本次读取的用户原始消息是否包含需要新增、修改或删除的长期偏好或项目准则。只提取用户明确表达、适合后续持续适用的要求，一条表达一个要求，正文简明、独立、可执行。
不得仅凭已有摘要推断偏好，不将 Agent 建议、普通事实、当前进度、临时操作或单次任务要求保存为偏好。无法确定长期适用性或作用范围时不新增。
跨项目的习惯和要求归入用户偏好；明确针对某项目的约定归入该项目。不能仅因消息出现在项目内，就提升为用户偏好。只使用输入列出的项目 ID。
对照已有条目去重，已有条目已能表达相同要求时不新增；用户明确补充或纠正已有要求时优先修改。仅在用户明确撤销或明确用新要求取代旧要求时删除，不因未再次提及或不适合当前任务而删除。
保留原有适用条件，不扩大范围，不加入推测。不得新增近义条目变相改写锁定条目，无法消解的冲突不变更。没有符合条件的变更时输出空 changes 数组。
现有偏好仅用于去重、识别变更对象，不作为摘要写作或提取约束，锁定只表示 Agent 不能修改或删除该条目。"#;

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
            session: SESSION_REQUIREMENTS.into(),
            task: TASK_REQUIREMENTS.into(),
            project: PROJECT_REQUIREMENTS.into(),
            preferences: PREFERENCE_REQUIREMENTS.into(),
        }
    }
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
