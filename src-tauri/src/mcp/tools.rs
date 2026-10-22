//! MCP contracts translate typed arguments into the shared business commands.
use crate::cli::{
    extraction_settings::Patch,
    service::{Command, Kind},
    CliError, CliResult,
};
use rmcp::{
    model::{Tool, ToolAnnotations},
    schemars::{self, JsonSchema},
};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};

pub(super) struct Spec {
    pub tool: Tool,
    pub write: bool,
    decode: Box<dyn Fn(Value) -> CliResult<Command> + Send + Sync>,
}
impl Spec {
    fn new<T: DeserializeOwned + JsonSchema>(
        name: &'static str,
        description: &'static str,
        write: bool,
        open_world: bool,
        convert: impl Fn(T) -> Command + Send + Sync + 'static,
    ) -> Self {
        let mut schema = schemars::schema_for!(T).to_value();
        if write {
            schema["properties"]["request_id"] = json!({
                "type":"string", "minLength":1, "maxLength":128,
                "pattern":"^[A-Za-z0-9_-]+$",
                "description":"Optional durable write ID. Reuse the same ID and arguments when retrying; changing the ID can repeat the operation. Generated if omitted."
            });
        }
        let mut tool = Tool::new(name, description, schema.as_object().unwrap().clone());
        tool.output_schema = Some(std::sync::Arc::new(
            json!({
                "type":"object", "required":["schema_version","ok"],
                "properties":{
                    "schema_version":{"type":"integer","const":1},
                    "ok":{"type":"boolean"},
                    "data":{},
                    "error":{"type":"object","required":["code","message"],"properties":{
                        "code":{"type":"string"},"message":{"type":"string"},"details":{}
                    }},
                    "request_id":{"type":"string"}
                }
            })
            .as_object()
            .unwrap()
            .clone(),
        ));
        tool.annotations = Some(
            ToolAnnotations::new()
                .read_only(!write)
                .destructive(write)
                .open_world(open_world),
        );
        Self {
            tool,
            write,
            decode: Box::new(move |value| {
                serde_json::from_value(value)
                    .map(&convert)
                    .map_err(|error| CliError::new("invalid_arguments", error))
            }),
        }
    }
    pub fn command(&self, mut arguments: Value) -> CliResult<(Command, Option<String>)> {
        let object = arguments
            .as_object_mut()
            .ok_or_else(|| CliError::new("invalid_arguments", "参数必须是对象"))?;
        let request_id = if self.write {
            match object.remove("request_id") {
                None => Some(crate::storage::new_id()),
                Some(Value::String(id))
                    if !id.is_empty()
                        && id.len() <= 128
                        && id
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') =>
                {
                    Some(id)
                }
                _ => {
                    return Err(CliError::new(
                        "invalid_arguments",
                        "request_id 需为 1～128 个字母、数字、短横线或下划线",
                    ))
                }
            }
        } else {
            None
        };
        Ok(((self.decode)(arguments)?, request_id))
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Id {
    id: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Target {
    kind: Kind,
    id: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Directory {
    /// Existing absolute directory; always pass the Agent's target directory.
    /// The MCP server's startup directory is not assumed to be the project.
    path: String,
}

#[derive(Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
enum Scope {
    #[default]
    Unarchived,
    Archived,
    All,
}
impl Scope {
    fn name(self) -> String {
        match self {
            Self::Unarchived => "unarchived",
            Self::Archived => "archived",
            Self::All => "all",
        }
        .into()
    }
}
fn list_limit() -> usize {
    20
}
fn message_limit() -> usize {
    8
}
fn first_ordinal() -> i64 {
    1
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct List {
    kind: Kind,
    /// Project ID. Supported for sessions and tasks only.
    project: Option<String>,
    /// Task ID. Supported for sessions only.
    task: Option<String>,
    /// Agent identifier. Supported for sessions only; see agent_list.
    agent: Option<String>,
    #[serde(default)]
    scope: Scope,
    title: Option<String>,
    /// Exact absolute workspace path filter.
    path: Option<String>,
    cursor: Option<String>,
    /// Page size, 1 to 50. Reuse filters with next_cursor.
    #[serde(default = "list_limit")]
    #[schemars(range(min = 1, max = 50))]
    limit: usize,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Search {
    query: String,
    project: Option<String>,
    task: Option<String>,
    agent: Option<String>,
    #[serde(default)]
    scope: Scope,
    cursor: Option<String>,
    #[serde(default = "list_limit")]
    #[schemars(range(min = 1, max = 50))]
    limit: usize,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Messages {
    /// NoEnding session ID, not an Agent's native session ID.
    id: String,
    query: Option<String>,
    /// Inclusive message ordinal, starting at 1.
    #[serde(default = "first_ordinal")]
    #[schemars(range(min = 1))]
    from: i64,
    to: Option<i64>,
    cursor: Option<String>,
    /// Number of message parts, 1 to 50. Join parts in order for long messages.
    #[serde(default = "message_limit")]
    #[schemars(range(min = 1, max = 50))]
    limit: usize,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Summary {
    kind: Kind,
    id: String,
    /// One-based item number; requires revision. Omit both to read the summary.
    item: Option<usize>,
    revision: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SummarySearch {
    query: String,
    kind: Option<Kind>,
    project: Option<String>,
    #[serde(default)]
    scope: Scope,
    cursor: Option<String>,
    #[serde(default = "list_limit")]
    #[schemars(range(min = 1, max = 50))]
    limit: usize,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SummarySet {
    kind: Kind,
    id: String,
    /// Expected current revision from summary_get. Stale edits are rejected.
    revision: i64,
    content: crate::domain::Summary,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Extract {
    kind: Kind,
    id: String,
    /// False updates from new input. True explicitly regenerates from history.
    #[serde(default)]
    regenerate: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TaskCreate {
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    projects: Vec<String>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TaskUpdate {
    id: String,
    title: Option<String>,
    description: Option<String>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TaskProjects {
    id: String,
    /// Complete desired project ID list. An empty list removes all associations.
    projects: Vec<String>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Tracking {
    task: String,
    session: String,
    tracked: bool,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Archive {
    kind: Kind,
    id: String,
    archived: bool,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Rename {
    id: String,
    name: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SyncArgs {
    /// Sync a single source. Mutually exclusive with session; omit both for all sources.
    source: Option<String>,
    /// Refresh a single ingested session.
    session: Option<String>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Config {
    /// codex, claude_code, pi, or none. Changing Agent clears previous overrides.
    agent: Option<String>,
    /// Empty string clears the override and uses the Agent default.
    model: Option<String>,
    provider: Option<String>,
    effort: Option<String>,
}

pub(super) fn catalog() -> Vec<Spec> {
    vec![
        Spec::new("status", "Read NoEnding Home, version and capabilities. Does not start the desktop app or synchronize.", false, false, |_: Empty| Command::Status),
        Spec::new("agent_list", "Query local Agent CLI availability and extraction capabilities. Does not verify login or model access.", false, false, |_: Empty| Command::Agents),
        Spec::new("project_resolve", "Resolve an existing absolute directory to a registered project, without registration or synchronization. Always pass the target path explicitly.", false, false, |p: Directory| Command::ResolveProject { path:p.path }),
        Spec::new("object_list", "List sessions, tasks or projects with filters and pagination. Returns metadata; read summaries separately.", false, false, |p: List| Command::List {kind:p.kind,project:p.project,task:p.task,scope:p.scope.name(),agent:p.agent,title:p.title,path:p.path,cursor:p.cursor,limit:p.limit}),
        Spec::new("object_get", "Read one session, task or project by NoEnding ID. Project details include paths; task details include associated projects.", false, false, |p: Target| Command::Show {kind:p.kind,id:p.id}),
        Spec::new("session_search", "Search ingested session titles and message bodies. Returns evidence locations; use session_messages for full text and context.", false, false, |p: Search| Command::Search {query:p.query,project:p.project,task:p.task,scope:p.scope.name(),agent:p.agent,cursor:p.cursor,limit:p.limit}),
        Spec::new("session_messages", "Read or search a session's stored history by ordinal and cursor. Long messages are split into ordered parts. Does not synchronize source files.", false, false, |p: Messages| Command::Messages {id:p.id,cursor:p.cursor,query:p.query,from:p.from,to:p.to,limit:p.limit}),
        Spec::new("summary_get", "Read a structured summary and its revision, or one item using item plus revision. Does not trigger extraction.", false, false, |p: Summary| Command::Summary {kind:p.kind,id:p.id,item:p.item,revision:p.revision}),
        Spec::new("summary_search", "Search summary items across sessions, tasks and projects. Results include kind, object ID, item number and revision for summary_get.", false, false, |p: SummarySearch| Command::SearchSummaries {query:p.query,kind:p.kind.map(|k| serde_json::to_value(k).unwrap().as_str().unwrap().to_owned()),project:p.project,scope:p.scope.name(),cursor:p.cursor,limit:p.limit}),
        Spec::new("summary_set", "Replace a summary with structured items using the expected revision. Preserve existing title_locked flags and locked titles; descriptions/details remain editable. Does not invoke an Agent.", true, false, |p: SummarySet| Command::EditSummary {kind:p.kind,id:p.id,revision:p.revision,content:serde_json::to_string(&p.content).expect("Summary contains only JSON fields")}),
        Spec::new("summary_update", "Explicitly run the configured Agent to update a summary; waits for extraction and persistence. Set regenerate only when requested. Inspect partial failures in the result.", true, true, |p: Extract| Command::Extract {kind:p.kind,id:p.id,regenerate:p.regenerate}),
        Spec::new("task_create", "Create a task with optional description and project associations. Does not launch a session.", true, false, |p: TaskCreate| Command::CreateTask {title:p.title,description:p.description,projects:p.projects}),
        Spec::new("task_update", "Edit a task title or description. Supply at least one field; omitted fields are preserved.", true, false, |p: TaskUpdate| Command::UpdateTask {id:p.id,title:p.title,description:p.description}),
        Spec::new("task_projects_set", "Replace all project associations of a task. An empty projects list removes all associations.", true, false, |p: TaskProjects| Command::SetTaskProjects {id:p.id,projects:p.projects}),
        Spec::new("task_session_set", "Enable or disable a task's tracking of an existing session. Does not launch or message an Agent.", true, false, |p: Tracking| if p.tracked { Command::Attach {task:p.task,session:p.session} } else { Command::Detach {task:p.task,session:p.session} }),
        Spec::new("object_archive", "Archive or restore a session, task or project. Does not permanently delete it.", true, false, |p: Archive| Command::Archive {kind:p.kind,id:p.id,archived:p.archived}),
        Spec::new("project_rename", "Rename a registered project without moving its directories.", true, false, |p: Rename| Command::RenameProject {id:p.id,name:p.name}),
        Spec::new("path_register", "Register an existing absolute workspace directory and resolve/create its project using NoEnding's workspace rules.", true, false, |p: Directory| Command::RegisterPath {path:p.path}),
        Spec::new("source_list", "List configured session ingestion sources. Does not scan their contents.", false, false, |_: Empty| Command::Sources),
        Spec::new("sync", "Explicitly synchronize all sources, one source, or one session. Waits for ingestion; does not invoke a model. Partial failures retain successful updates.", true, false, |p: SyncArgs| Command::Sync {source:p.source,session:p.session}),
        Spec::new("summary_config_get", "Read extraction Agent, model, provider and reasoning overrides.", false, false, |_: Empty| Command::ExtractionSettings),
        Spec::new("summary_config_set", "Patch extraction configuration without invoking a model. At least one field is required. Empty runtime fields clear overrides; agent none disables extraction. Check agent_list for local availability.", true, false, |p: Config| Command::ConfigureExtraction {patch:Patch {agent:p.agent,model:p.model,provider:p.provider,effort:p.effort}}),
        Spec::new("request_get", "Inspect a durable write receipt by its request ID after a timeout or disconnect. Running/interrupted requests must not be blindly repeated with a new ID.", false, false, |p: Id| Command::Receipt {id:p.id}),
    ]
}
