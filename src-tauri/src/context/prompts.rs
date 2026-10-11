//! Editable summary instructions. File/response contracts stay in `summary`.
use crate::error::{other, Result};
use crate::storage::Db;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

const SETTINGS_KEY: &str = "summary_prompt_templates";
const SESSION_REQUIREMENTS: &str = r#"概括整个会话中值得继续沿用的讨论、解释、决定和成果，覆盖重要主题，让读者理解讨论了什么、得出了什么结论及其依据、实际改变了什么，以及哪些问题仍未解决。最新进度只是其中一部分；标题仅供背景参考，不限制摘要主题。
实施类内容保留关键行为变化、解决方式、必要原因及验证边界；问答类内容保留核心解释、原理和结论；设计讨论保留最终选择、重要取舍及适用条件。按材料实际内容取舍，不虚构缺失的信息，不用“已实现、已提交、测试通过”代替具体成果。
使用 Markdown，建议不超过 2000 字符（含空格与标点），不要求凑字数，不直接截断。兼顾早期与近期的重要内容，不因近期讨论更密集就只保留最后几轮。
区分用户要求、已确认事实、Agent 建议、计划、已完成和待验证结果，不把计划写成完成、建议写成决定。保留会话当时的结论与后续明确变更的关系，涉及重要版本变化时保留必要日期，不将当时结论泛化为现在仍然成立的事实。
删除寒暄、过程流水账、大段代码和日志。模型名称、执行耗时、提交哈希、测试数量等只在影响后续工作时保留。路径、命令和配置项用行内代码；沿用主要语言，保留必要技术术语，正文独立可读。
写入前核对：会话的重要主题及其具体结论是否覆盖，要求和建议是否区分，历史与现状是否混淆。摘要正文不包含核对过程。"#;

const TASK_REQUIREMENTS: &str = r#"综合归属于任务的会话材料，围绕任务目标提炼整体结论，不逐个复述会话，也不以最近一个会话的状态代表整个任务。
先从任务标题和描述识别目标与范围，再筛选会话中的相关材料；会话归属于任务不代表其中所有话题都属于任务目标。项目整体背景只保留理解该任务所必需的部分，不把任务目标改写成建设整个项目。标题和描述未明确目标时，才根据会话中已确认的任务要求概括范围，无法确定则说明。
重点检查四类信息：目标与范围（要解决的问题及边界）；成果与现状（具体变化、关键做法及尚未完成的部分）；决定与约束（已确认选择、必要原因和适用条件）；问题与后续（明确的阻碍、待验证事项及已约定行动）。这些是内容取舍依据，不是固定栏目；缺少依据或没有重要信息的类别省略，不自行生成待办，不用“已完成、测试通过”代替具体成果。
同一事项的不同结论按消息时间、适用范围和明确修正关系综合。明确删除、替换或收缩范围的变更优先于旧摘要中的笼统描述；后来的无关讨论不使旧结论失效，会话最后消息时间或摘要生成时间也不能证明其中每项事实最新。无法确认的冲突不得写成当前事实，重要且无法省略时说明不确定性。区分要求、建议、计划、已完成和待验证，不虚构进展。
使用 Markdown，建议不超过 1000 字符（含空格与标点），不直接截断、不凑字数。
删除过程流水账和无后续价值的模型、耗时、提交、测试数量等细节；保留具体结果和验证边界。沿用主要语言，路径、命令和配置项用行内代码，正文独立可读。
写入前核对：摘要是否回答了任务标题和描述中的目标，是否混入无关项目建设内容，局部进展是否误当整体结论，历史与现状、计划与完成是否区分。摘要正文不包含核对过程。"#;
const PROJECT_REQUIREMENTS: &str = r#"综合项目内会话材料，从项目整体提炼用途、能力和长期有效的结论，不逐个复述任务或会话，也不让最近一个会话的进度取代项目整体内容。
重点检查四类信息：目标与范围（项目用途及解决的需求）；成果与现状（当前重要能力及主要变化）；决定与约束（持续有效的原则、约定和限制）；问题与后续（明确的重要缺口、持续问题及已约定方向）。这些是内容取舍依据，不是固定栏目；缺少依据或没有重要信息的类别省略，不自行生成规划。优先呈现使用者可见的具体能力；内部实体、表名和实现流程只在解释关键约束时保留，避免用旧架构细节占据项目概览。
局部任务完成不代表整个项目完成。明确废弃、删除或替换的能力及设计不得再写成当前状态；历史内容只在解释关键演变时保留，并明确标注。旧材料中的未完成计划不能长期沿用为当前待办；对理解项目仍重要但无法确认是否继续的事项，说明其状态未确认。
会话摘要是经过压缩的历史记录，可能遗漏后续改造；较新的摘要未提到某功能，既不能证明它仍存在，也不能证明它已删除。综合前按候选主题核对相同功能在不同会话中的结论及替代关系，尤其是数据模型、归属关系、状态流程和提取机制。对每项准备写成当前能力的旧设计，在提供的原始消息文件中分别检索其相关名称及变更，完整读取命中消息及前后讨论并核对时间；不能只核对部分主题，也不能仅凭旧摘要名称作现状断言。
明确删除、替换、取消或“仅保留某机制”的变更，应据此更新旧功能描述，不能因旧方案更详细、出现更多次就继续写成现状。已确认的替代机制应以具体能力体现，不能只留下旧功能名称。
同一事项的新旧按消息时间、适用范围和明确修正关系判断；后来的无关讨论不推翻旧结论，会话最后消息时间或摘要生成时间不能证明其中每项事实最新。相关材料没有足够依据消解冲突时，不把存疑能力、旧待办或旧流程写成当前事实；重要且无法省略时说明状态未确认。项目摘要不是代码现状核验，不自行检查工作目录；任务背景不代表任务已完成。
使用 Markdown，建议不超过 1000 字符（含空格与标点），不直接截断、不凑字数。
删除过程流水账和无后续价值的模型、耗时、提交、测试数量等细节；保留具体结果和验证边界。沿用主要语言，路径、命令和配置项用行内代码，正文独立可读。
写入前核对：项目用途与重要能力是否清楚，局部成果是否误当项目整体状态，已退役的设计是否误写为现状，计划与完成是否区分。摘要正文不包含核对过程。"#;

const SUMMARY_SYNTHESIS: &str = r#"已有摘要代表此前的整体内容，新增消息只是补充，输出仍须是一份完整摘要。先识别已有摘要的主要主题，逐项判断保留、合并、修正或删除，再整合新增内容；不得只总结新增消息或把整份摘要替换为最近关注的话题。
优先将新信息融入已有主题；仅当其具有独立、重要的信息价值且无法自然融入已有主题时才新增主题。不在末尾追加更新记录，不按日期、批次或会话拆出重复主题。
仍然有效不等于必须保留，但删去重要旧主题应有明确取舍依据：已被纠正或取代、已失效、重复，或相对次要且不再影响整体理解。新消息没有再次提到某个主题，不是删除理由。先压缩主题内的细节，再考虑删除整个主题；不要用近期进度替代早期仍重要的解释、成果、决定和约束。
摘要是重要信息的浓缩，不是完整档案。接近建议长度上限时优先合并主题、压缩表述、删减次要信息，不靠增加主题或拉长列表收纳全部材料；也不为维持旧摘要的结构保留空泛内容。写入前检查是否存在重复主题、失效事项或可删减细节。"#;

const SUMMARY_LAYOUT: &str = r#"正文按主题使用下列 Markdown 排版，主题名称和数量由实际内容决定，不强制目标、进展、待办等固定栏目。每个主题先给一句简短的普通正文结论（不加粗），再用无序列表按点说明具体内容、必要原因或边界；说明必须分点，不写长段正文。
每点只表达一项具体事项，用一个短句，建议不超过 80 字符，必要路径或术语较长时可例外。多个独立功能、决定或验证结果拆成不同要点，不靠分号把长段落塞进一个列表项；超出整体建议长度时删减次要说明。合并重复信息，不凑条数，不让标题、结论和要点重复同一内容。信息简单时只保留一个主题和必要要点。
排版示例（替换为材料中的实际内容，保留空行，不输出示例说明或包裹正文的代码块）：
### 主题名称

一句话关键结论。

- 一项具体成果、解释或决定。
- 必要的原因、适用条件或验证边界。
- 有依据的未解决问题或后续行动；没有则省略。

继续按相同排版组织其他重要主题。"#;

const PREFERENCE_REQUIREMENTS: &str = r#"在生成摘要的同时，检查本次读取的用户原始消息是否包含需要新增、修改或删除的长期偏好或项目准则。只提取用户明确表达、适合后续持续适用的要求，一条表达一个要求，正文简明、独立、可执行。
每条只允许一个可独立执行、修改或撤销的要求，建议不超过 80 字符。修改已有条目也必须遵守：不得将不同规则追加成一段规则集；不同要求必须分别成条。省略背景、理由、实施步骤和摘要本身，保留作用对象及必要条件，例如会话摘要的长度要求不能扩大为所有摘要的长度要求。
逐条核对依据必须来自 role=user 的原始消息；role=assistant 的建议、摘要、实测报告及本次提取提示词都不能直接作偏好依据。用户明确采纳某项建议时才可提取，并保留其条件，不能把一句笼统认可扩展为采纳所有细节。没有可确认的原始依据时不变更。
不得仅凭已有摘要推断偏好，不将 Agent 建议、普通事实、当前进度、临时操作或单次任务要求保存为偏好。无法确定长期适用性或作用范围时不新增。
跨项目的习惯和要求归入用户偏好；明确针对某项目的约定归入该项目。不能仅因消息出现在项目内，就提升为用户偏好。只使用输入列出的项目 ID。
已有未锁定条目混合多个要求，且本次原始用户消息能确认各项要求时，应拆分整理：将原条目更新为其中一个要求，其余要求分别新增或复用其他独立条目。不能以“已有混合条目已经涵盖”为由保留规则集；未能核实每项依据时不拆分，锁定条目不拆分。不得仅从现有条目或摘要拆出推测性要求，不在整理时遗漏或扩大原有约束。
对照整理后的独立条目去重，已有独立条目已能表达相同要求时不新增；用户明确补充或纠正已有要求时优先修改。仅在用户明确撤销或明确用新要求取代旧要求时删除，不因未再次提及或不适合当前任务而删除。
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
            session: format!("{SESSION_REQUIREMENTS}\n{SUMMARY_SYNTHESIS}\n{SUMMARY_LAYOUT}"),
            task: format!("{TASK_REQUIREMENTS}\n{SUMMARY_SYNTHESIS}\n{SUMMARY_LAYOUT}"),
            project: format!("{PROJECT_REQUIREMENTS}\n{SUMMARY_SYNTHESIS}\n{SUMMARY_LAYOUT}"),
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
