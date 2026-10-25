use super::{CliError, CliResult};
use crate::{
    context,
    storage::Db,
    workspace::{home::NoEndingHome, wiring::WorkspaceLayer},
};
use rmcp::schemars;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    Session,
    Task,
    Project,
}
impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Task => "task",
            Self::Project => "project",
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Command {
    ExecuteOnce {
        request_id: String,
        command: Box<Command>,
    },
    Receipt {
        id: String,
    },
    MailboxList(crate::mailbox::List),
    MailboxRead(crate::mailbox::Read),
    MailboxSend(crate::mailbox::Send),
    MailboxMark(crate::mailbox::Mark),
    Status,
    SessionControl {
        command: crate::session_control::ControlCommand,
    },
    Freshness {
        kind: Option<Kind>,
        id: Option<String>,
    },
    Agents,
    ResolveProject {
        path: String,
    },
    ResolveSession {
        agent: String,
        native_id: String,
    },
    ExtractionContext {
        kind: Kind,
        id: String,
        mode: context::interactive::Mode,
    },
    ReadExtraction {
        context_id: String,
        input: String,
        cursor: usize,
        limit: usize,
        query: Option<String>,
    },
    SubmitExtraction {
        context_id: String,
        reply: Value,
        results: std::collections::HashMap<String, Value>,
    },
    ExtractionSettings,
    ConfigureExtraction {
        patch: super::extraction_settings::Patch,
    },
    Sources,
    Sync {
        source: Option<String>,
        session: Option<String>,
    },
    List {
        kind: Kind,
        project: Option<String>,
        task: Option<String>,
        scope: String,
        agent: Option<String>,
        title: Option<String>,
        path: Option<String>,
        cursor: Option<String>,
        limit: usize,
    },
    Search {
        query: String,
        project: Option<String>,
        task: Option<String>,
        scope: String,
        agent: Option<String>,
        cursor: Option<String>,
        limit: usize,
    },
    Show {
        kind: Kind,
        id: String,
    },
    Messages {
        id: String,
        cursor: Option<String>,
        query: Option<String>,
        from: i64,
        to: Option<i64>,
        limit: usize,
    },
    Summary {
        kind: Kind,
        id: String,
        item: Option<usize>,
        item_id: Option<String>,
        revision: Option<i64>,
    },
    SearchSummaries {
        query: String,
        kind: Option<String>,
        project: Option<String>,
        scope: String,
        cursor: Option<String>,
        limit: usize,
    },
    TaskItems {
        task: String,
    },
    CreateTaskItem {
        task: String,
        content: crate::workspace::task_items::CreateItem,
    },
    UpdateTaskItem {
        task: String,
        item_id: String,
        revision: i64,
        patch: crate::workspace::task_items::ItemPatch,
    },
    AppendTaskItemNote {
        task: String,
        item_id: String,
        text: String,
    },
    DeleteTaskItem {
        task: String,
        item_id: String,
        revision: i64,
    },
    Extract {
        kind: Kind,
        id: String,
        regenerate: bool,
    },
    CreateTask {
        title: String,
        description: String,
        projects: Vec<String>,
    },
    UpdateTask {
        id: String,
        title: Option<String>,
        description: Option<String>,
    },
    Archive {
        kind: Kind,
        id: String,
        archived: bool,
    },
    RenameProject {
        id: String,
        name: String,
    },
    RegisterPath {
        path: String,
    },
    TaskProjects {
        id: String,
    },
    SetTaskProjects {
        id: String,
        projects: Vec<String>,
    },
    Attach {
        task: String,
        session: String,
    },
    Detach {
        task: String,
        session: String,
    },
}
impl Command {
    pub(crate) fn is_write(&self) -> bool {
        if let Self::SessionControl { command } = self {
            return command.is_write();
        }
        matches!(
            self,
            Self::MailboxSend(_)
                | Self::MailboxMark(_)
                | Self::ConfigureExtraction { .. }
                | Self::Sync { .. }
                | Self::CreateTaskItem { .. }
                | Self::UpdateTaskItem { .. }
                | Self::AppendTaskItemNote { .. }
                | Self::DeleteTaskItem { .. }
                | Self::Extract { .. }
                | Self::SubmitExtraction { .. }
                | Self::CreateTask { .. }
                | Self::UpdateTask { .. }
                | Self::Archive { .. }
                | Self::RenameProject { .. }
                | Self::RegisterPath { .. }
                | Self::SetTaskProjects { .. }
                | Self::Attach { .. }
                | Self::Detach { .. }
        )
    }
}
fn existing_directory(path: &str) -> CliResult<&str> {
    let raw = path.trim();
    let path = std::path::Path::new(raw);
    if !path.is_absolute() || !path.is_dir() {
        return Err(CliError::new(
            "invalid_arguments",
            "需要一个已存在的绝对目录路径",
        ));
    }
    Ok(raw)
}

pub(crate) struct Service {
    pub db: Arc<Db>,
    pub home: NoEndingHome,
    pub layer: Arc<WorkspaceLayer>,
    _access: crate::workspace::home::HomeAccess,
}
impl Service {
    pub(crate) fn open(home: NoEndingHome) -> CliResult<Arc<Self>> {
        let access = home.acquire_access()?;
        let db = Arc::new(Db::open(&home.db_path)?);
        let layer = Arc::new(WorkspaceLayer::new(&home));
        Ok(Arc::new(Self {
            db,
            home,
            layer,
            _access: access,
        }))
    }

    fn require(&self, kind: Kind, id: &str) -> CliResult<()> {
        let exists = match kind {
            Kind::Session => self.db.get_session(id)?.is_some(),
            Kind::Task => self.db.get_workstream(id)?.is_some(),
            Kind::Project => self.db.get_project(id)?.is_some(),
        };
        if exists {
            Ok(())
        } else {
            Err(CliError::new(
                "not_found",
                format!("{} {id} 不存在", kind.name()),
            ))
        }
    }
    pub(crate) fn dispatch(self: &Arc<Self>, command: Command) -> CliResult<Value> {
        match command {
            Command::ExecuteOnce {
                request_id,
                command,
            } => self.execute_once(&request_id, *command),
            Command::Receipt { id } => super::receipts::show(&self.home, &id),
            Command::SessionControl { command } => {
                if command.is_write() {
                    return Err(CliError::new(
                        "invalid_arguments",
                        "会话控制写操作需要请求编号",
                    ));
                }
                crate::session_control::request(&self.home, command, None)
            }
            Command::MailboxList(p) => Ok(serde_json::to_value(
                self.db
                    .mailbox_list(&p.session, &p.state, p.cursor, p.limit)?,
            )?),
            Command::MailboxRead(p) => Ok(serde_json::to_value(
                self.db.mailbox_read(&p.session, &p.message_id)?,
            )?),
            Command::MailboxSend(p) => crate::mailbox::send(&self.db, &self.home, p),
            Command::MailboxMark(p) => crate::mailbox::mark(&self.db, &self.home, p),
            Command::Status => Ok(
                json!({"connected":true,"version":env!("CARGO_PKG_VERSION"),"home":self.home.root,"capabilities":["search","session","task","project","summary","freshness","sync","source","agent","request","session_control","mailbox"]}),
            ),
            Command::Freshness { kind, id } => {
                if kind.is_some() != id.is_some() {
                    return Err(CliError::new(
                        "invalid_arguments",
                        "kind 与 id 必须同时提供",
                    ));
                }
                if let (Some(kind), Some(id)) = (kind, id.as_deref()) {
                    self.require(kind, id)?;
                }
                Ok(serde_json::to_value(context::freshness::view(
                    &self.db,
                    kind.map(Kind::name),
                    id.as_deref(),
                )?)?)
            }
            Command::Agents => Ok(super::agents::list()),
            Command::ResolveSession { agent, native_id } => {
                let agent = crate::domain::Agent::parse(&agent)
                    .ok_or_else(|| CliError::new("invalid_arguments", "Agent 无效"))?;
                if native_id.trim().is_empty() {
                    return Err(CliError::new("invalid_arguments", "原生会话 ID 不能为空"));
                }
                let session = self
                    .db
                    .find_session_by_root_agent_id(agent, &native_id)?
                    .ok_or_else(|| {
                        CliError::new(
                            "not_found",
                            "当前 Home 未找到该 Agent 的原生会话，可能尚未同步",
                        )
                    })?;
                let mut value = session_value(&session);
                value["native_id"] = json!(session.root_agent_session_id);
                Ok(value)
            }
            Command::ExtractionContext { kind, id, mode } => {
                self.require(kind, &id)?;
                context::interactive::context(&self.db, &self.home, kind.name(), &id, mode)
            }
            Command::ReadExtraction {
                context_id,
                input,
                cursor,
                limit,
                query,
            } => context::interactive::read(
                &self.db,
                &self.home,
                &context_id,
                &input,
                cursor,
                limit,
                query.as_deref(),
            ),
            Command::SubmitExtraction {
                context_id,
                reply,
                results,
            } => context::interactive::submit(&self.db, &self.home, &context_id, reply, results),
            Command::ExtractionSettings => Ok(serde_json::to_value(
                crate::sync::extractor::ExtractionSettings::from_settings(&self.db)?,
            )?),
            Command::ConfigureExtraction { patch } => Ok(serde_json::to_value(
                super::extraction_settings::update(&self.db, patch)?,
            )?),
            Command::ResolveProject { path } => {
                existing_directory(&path)?;
                let projection = self.layer.projection();
                Ok(serde_json::to_value(
                    crate::workspace::probe::resolve_project(
                        &self.db,
                        projection.observer(),
                        projection.policy(),
                        &path,
                    )?,
                )?)
            }
            Command::Sources => Ok(json!({"items":self.db.list_ingest_sources()?})),
            Command::Sync { source, session } => {
                if source.is_some() && session.is_some() {
                    return Err(CliError::new(
                        "invalid_arguments",
                        "source 与 session 不能同时指定",
                    ));
                }
                self.sync(source.as_deref(), session.as_deref())
                    .map_err(|error| {
                        if crate::platform::cancellation::requested() {
                            CliError {
                                code: "cancelled".into(),
                                message: "同步已取消，已提交的会话数据保留".into(),
                                details: error.details,
                            }
                        } else {
                            error
                        }
                    })
            }
            Command::List {
                kind,
                project,
                task,
                scope,
                agent,
                title,
                path,
                cursor,
                limit,
            } => self.list(
                kind, project, task, scope, agent, title, path, cursor, limit,
            ),
            Command::Search {
                query,
                project,
                task,
                scope,
                agent,
                cursor,
                limit,
            } => {
                if !matches!(scope.as_str(), "unarchived" | "archived" | "all") {
                    return Err(CliError::new("invalid_arguments", "归档范围无效"));
                }
                if let Some(id) = &project {
                    self.require(Kind::Project, id)?;
                }
                if let Some(id) = &task {
                    self.require(Kind::Task, id)?;
                }
                super::search::page(
                    &self.db,
                    &query,
                    crate::search::sessions::SessionFilter {
                        project,
                        task,
                        scope,
                        agent: parse_agent(agent)?,
                    },
                    cursor.as_deref(),
                    limit,
                )
            }
            Command::Show { kind, id } => {
                self.require(kind, &id)?;
                match kind {
                    Kind::Session => Ok(session_value(&self.db.get_session(&id)?.unwrap())),
                    Kind::Task => Ok(
                        json!({"task":self.db.get_workstream(&id)?,"projects":crate::workspace::workstream::projects_for_workstream(&self.db,&id)?,"sessions_command":format!("ne session list --task {id} --scope all")}),
                    ),
                    Kind::Project => {
                        let project = self.db.get_project(&id)?.unwrap();
                        let paths = self.db.list_workspace_paths_for_project(&id)?;
                        let kind = crate::workspace::project::project_kind(
                            &project,
                            &paths
                                .iter()
                                .map(|p| p.canonical_path.clone())
                                .collect::<Vec<_>>(),
                            self.layer.projection().policy(),
                        );
                        Ok(
                            json!({"project":project,"kind":kind,"paths":paths,"tasks_command":format!("ne task list --project {id} --scope all"),"sessions_command":format!("ne session list --project {id} --scope all")}),
                        )
                    }
                }
            }
            Command::Messages {
                id,
                cursor,
                query,
                from,
                to,
                limit,
            } => super::messages::page(
                &self.db,
                &id,
                cursor.as_deref(),
                query.as_deref(),
                from,
                to,
                limit,
            ),
            Command::Summary {
                kind,
                id,
                item,
                item_id,
                revision,
            } => {
                let selected = item.is_some() || item_id.is_some();
                if selected != revision.is_some() {
                    return Err(CliError::new(
                        "invalid_arguments",
                        "按事项读取时需要 --item-id 或 --item，以及 --revision",
                    ));
                }
                self.require(kind, &id)?;
                if let Some(revision) = revision {
                    let number = context::items::resolve_number(
                        &self.db,
                        kind.name(),
                        &id,
                        revision,
                        item,
                        item_id.as_deref(),
                    )?;
                    let item =
                        context::items::read_item(&self.db, kind.name(), &id, revision, number)?;
                    Ok(
                        json!({"kind":kind,"id":id,"revision":revision,"item_number":number,"item_id":item.id,"item":item}),
                    )
                } else {
                    self.summary(kind, &id)
                }
            }
            Command::SearchSummaries {
                query,
                kind,
                project,
                scope,
                cursor,
                limit,
            } => {
                if let Some(id) = &project {
                    self.require(Kind::Project, id)?;
                }
                super::summaries::page(
                    &self.db,
                    &query,
                    context::items::SummaryFilter {
                        kind,
                        project,
                        scope,
                    },
                    cursor.as_deref(),
                    limit,
                )
            }
            Command::TaskItems { task } => {
                Ok(json!({"items": crate::workspace::task_items::list(&self.db, &task)?}))
            }
            Command::CreateTaskItem { task, content } => Ok(serde_json::to_value(
                crate::workspace::task_items::create(&self.db, &task, content)?,
            )?),
            Command::UpdateTaskItem {
                task,
                item_id,
                revision,
                patch,
            } => Ok(serde_json::to_value(crate::workspace::task_items::update(
                &self.db, &task, &item_id, revision, patch,
            )?)?),
            Command::AppendTaskItemNote {
                task,
                item_id,
                text,
            } => Ok(serde_json::to_value(
                crate::workspace::task_items::append_note(&self.db, &task, &item_id, &text)?,
            )?),
            Command::DeleteTaskItem {
                task,
                item_id,
                revision,
            } => {
                crate::workspace::task_items::delete(&self.db, &task, &item_id, revision)?;
                Ok(json!({"task_id":task,"item_id":item_id,"deleted":true}))
            }
            Command::Extract {
                kind,
                id,
                regenerate,
            } => {
                self.require(kind, &id)?;
                self.extract(kind, &id, regenerate)
            }
            Command::CreateTask {
                title,
                description,
                projects,
            } => {
                let task = crate::workspace::workstream::create_workstream(
                    &self.db,
                    &title,
                    &description,
                    &projects,
                )?;
                Ok(json!({"task":task}))
            }
            Command::UpdateTask {
                id,
                title,
                description,
            } => {
                if title.is_none() && description.is_none() {
                    return Err(CliError::new(
                        "invalid_arguments",
                        "需要提供 title 或 description",
                    ));
                }
                self.require(Kind::Task, &id)?;
                let task = crate::workspace::workstream::update_workstream_fields(
                    &self.db,
                    &id,
                    title.as_deref(),
                    description.as_deref(),
                )?;
                Ok(json!({"task":task}))
            }
            Command::Archive { kind, id, archived } => {
                self.require(kind, &id)?;
                let value = match kind {
                    Kind::Task => {
                        let task = if archived {
                            crate::workspace::workstream::archive_workstream(&self.db, &id)?
                        } else {
                            crate::workspace::workstream::restore_workstream(&self.db, &id)?
                        };
                        json!({"task":task})
                    }
                    Kind::Session => {
                        let session = if archived {
                            crate::lifecycle::archive_session(&self.db, &id)?
                        } else {
                            crate::lifecycle::restore_session(&self.db, &id)?
                        };
                        session_value(&session)
                    }
                    Kind::Project => {
                        json!({"project":crate::workspace::project::set_project_archived(&self.db, &id, archived)?})
                    }
                };
                Ok(value)
            }
            Command::RenameProject { id, name } => {
                self.require(Kind::Project, &id)?;
                let project = crate::workspace::project::rename_project(&self.db, &id, &name)?;
                Ok(json!({"project":project}))
            }
            Command::RegisterPath { path } => {
                let path = self.register_path(&path)?;
                let project = self.db.get_project(&path.project_id)?;
                Ok(json!({"path":path,"project":project}))
            }
            Command::TaskProjects { id } => {
                self.require(Kind::Task, &id)?;
                Ok(
                    json!({"items":crate::workspace::workstream::projects_for_workstream(&self.db,&id)?}),
                )
            }
            Command::SetTaskProjects { id, projects } => {
                crate::workspace::workstream::set_workstream_projects(&self.db, &id, &projects)?;
                Ok(
                    json!({"items":crate::workspace::workstream::projects_for_workstream(&self.db,&id)?}),
                )
            }
            Command::Attach { task, session } => {
                self.require(Kind::Task, &task)?;
                self.require(Kind::Session, &session)?;
                let session = crate::workspace::session::set_session_tracking(
                    &self.db, &session, &task, true,
                )?;
                Ok(session_value(&session))
            }
            Command::Detach { task, session } => {
                self.require(Kind::Task, &task)?;
                self.require(Kind::Session, &session)?;
                crate::workspace::session::set_session_tracking(&self.db, &session, &task, false)?;
                Ok(json!({"session_id":session,"task_id":task,"tracked":false}))
            }
        }
    }
    fn register_path(&self, path: &str) -> CliResult<crate::domain::WorkspacePath> {
        let projection = self.layer.projection();
        let observation = projection.observer().observe(existing_directory(path)?);
        Ok(crate::workspace::project::ensure_workspace_path_outcome(
            &self.db,
            &observation,
            projection.policy(),
        )?
        .path)
    }
    fn execute_once(self: &Arc<Self>, id: &str, command: Command) -> CliResult<Value> {
        if !command.is_write() {
            return Err(CliError::new("invalid_arguments", "请求编号只支持写操作"));
        }
        if let Command::SessionControl { command } = command {
            return crate::session_control::request(&self.home, command, Some(id.to_string()));
        }
        let payload = serde_json::to_vec(&command)?;
        super::receipts::execute(&self.home, id, &payload, || self.dispatch(command))
    }
    fn sync(&self, source: Option<&str>, session: Option<&str>) -> CliResult<Value> {
        // Production CLI owns one Home per process, like the desktop startup.
        let _ = crate::workspace::session::register_workspace_attacher(self.layer.clone());
        if let Some(id) = session {
            self.require(Kind::Session, id)?;
            let messages = crate::ingestion::refresh_session(&self.db, id)?;
            return Ok(json!({"discovered":0,"messages":messages,"failures":[]}));
        }
        if let Some(id) = source {
            let source = self
                .db
                .get_ingest_source(id)?
                .ok_or_else(|| CliError::new("not_found", "同步来源不存在"))?;
            let (discovered, messages) =
                crate::ingestion::reconcile_source(&self.db, &source, &|_| {})?;
            return Ok(json!({"discovered":discovered,"messages":messages,"failures":[]}));
        }
        let report = crate::ingestion::reconcile_all_report(&self.db, &|_| {})?;
        let value = json!({"discovered":report.discovered,"messages":report.messages,"failures":report.failures});
        if !report.failures.is_empty() {
            let mut error = CliError::new("sync_partial_failure", "部分会话来源同步失败");
            error.details = Some(value);
            return Err(error);
        }
        Ok(value)
    }
    fn summary(&self, kind: Kind, id: &str) -> CliResult<Value> {
        self.require(kind, id)?;
        Ok(match kind {
            Kind::Session => {
                let view = context::session_context_view(&self.db, id)?;
                json!({"kind":kind,"id":id,"summary":view.fields.map(|f|f.summary),"revision":view.revision,"updated_at":view.updated_at,"pending":view.pending,"freshness":view.freshness,"ingest_generation":view.ingest_generation,"processed_through_seq":view.processed_through_seq,"latest_message_seq":view.latest_message_seq})
            }
            Kind::Task => {
                let view = context::workstream_context_view(&self.db, id)?;
                json!({"kind":kind,"id":id,"summary":view.summary,"revision":view.context_revision,"updated_at":view.updated_at,"pending":view.pending,"freshness":view.freshness,"pending_sessions":view.pending_sessions})
            }
            Kind::Project => {
                let view = context::project_context_view(&self.db, id)?;
                json!({"kind":kind,"id":id,"summary":view.summary,"revision":view.context_revision,"updated_at":view.updated_at,"pending":view.pending,"freshness":view.freshness,"pending_sessions":view.pending_sessions})
            }
        })
    }
    fn extract(&self, kind: Kind, id: &str, regenerate: bool) -> CliResult<Value> {
        Ok(match (kind, regenerate) {
            (Kind::Session, false) => {
                json!(context::update_session(&self.db, id, Some(&self.home))?)
            }
            (Kind::Session, true) => {
                json!(context::regenerate_session(&self.db, id, Some(&self.home))?)
            }
            (Kind::Task, false) => {
                json!(context::update_workstream(&self.db, id, Some(&self.home))?)
            }
            (Kind::Task, true) => json!(context::regenerate_workstream(
                &self.db,
                id,
                Some(&self.home)
            )?),
            (Kind::Project, false) => {
                json!(context::update_project(&self.db, id, Some(&self.home))?)
            }
            (Kind::Project, true) => {
                json!(context::regenerate_project(&self.db, id, Some(&self.home))?)
            }
        })
    }
    fn list(
        &self,
        kind: Kind,
        project: Option<String>,
        task: Option<String>,
        scope: String,
        agent: Option<String>,
        title: Option<String>,
        path: Option<String>,
        cursor: Option<String>,
        limit: usize,
    ) -> CliResult<Value> {
        if !(1..=50).contains(&limit)
            || !matches!(scope.as_str(), "unarchived" | "archived" | "all")
        {
            return Err(CliError::new(
                "invalid_arguments",
                "无效的范围或分页大小（1～50）",
            ));
        }
        if kind == Kind::Project && (project.is_some() || task.is_some() || agent.is_some())
            || kind == Kind::Task && (task.is_some() || agent.is_some())
        {
            return Err(CliError::new(
                "invalid_arguments",
                "此列表不支持指定过滤条件",
            ));
        }
        if let Some(id) = &project {
            self.require(Kind::Project, id)?;
        }
        if let Some(id) = &task {
            self.require(Kind::Task, id)?;
        }
        if title.as_ref().is_some_and(|s| s.trim().is_empty()) {
            return Err(CliError::new("invalid_arguments", "标题筛选不能为空"));
        }
        let path = path
            .map(|s| {
                crate::workspace::identity::normalize_path(&s)
                    .ok_or_else(|| CliError::new("invalid_arguments", "--path 需要绝对工作路径"))
            })
            .transpose()?;
        let agent = parse_agent(agent)?;
        let sql=match kind {
            Kind::Session=>"SELECT s.id FROM sessions s LEFT JOIN workspace_paths wp ON wp.id=s.workspace_path_id WHERE s.id>?1 AND (?2 IS NULL OR wp.project_id=?2) AND (?3 IS NULL OR EXISTS(SELECT 1 FROM session_workstreams sw WHERE sw.session_id=s.id AND sw.workstream_id=?3)) AND (?4 IS NULL OR s.agent=?4) AND (?5='all' OR (?5='archived' AND s.archived_at IS NOT NULL) OR (?5='unarchived' AND s.archived_at IS NULL)) AND (?7 IS NULL OR instr(lower(s.title),lower(?7))>0) AND (?8 IS NULL OR COALESCE(wp.canonical_path,s.cwd)=?8 COLLATE NOCASE) ORDER BY s.id LIMIT ?6",
            Kind::Task=>"SELECT s.id FROM workstreams s WHERE s.id>?1 AND (?2 IS NULL OR EXISTS(SELECT 1 FROM workstream_projects p WHERE p.workstream_id=s.id AND p.project_id=?2)) AND (?5='all' OR (?5='archived' AND s.visibility='archived') OR (?5='unarchived' AND s.visibility='normal')) AND ?3 IS NULL AND ?4 IS NULL AND (?7 IS NULL OR instr(lower(s.title),lower(?7))>0) AND (?8 IS NULL OR EXISTS(SELECT 1 FROM workstream_projects p JOIN workspace_paths wp ON wp.project_id=p.project_id WHERE p.workstream_id=s.id AND wp.canonical_path=?8 COLLATE NOCASE)) ORDER BY s.id LIMIT ?6",
            Kind::Project=>"SELECT id FROM projects WHERE id>?1 AND ?2 IS NULL AND ?3 IS NULL AND ?4 IS NULL AND (?5='all' OR (?5='archived' AND visibility='archived') OR (?5='unarchived' AND visibility='normal')) AND (?7 IS NULL OR instr(lower(name),lower(?7))>0) AND (?8 IS NULL OR EXISTS(SELECT 1 FROM workspace_paths wp WHERE wp.project_id=projects.id AND wp.canonical_path=?8 COLLATE NOCASE)) ORDER BY id LIMIT ?6",
        };
        let sql = sql.replace(
            "COLLATE NOCASE",
            if cfg!(windows) {
                "COLLATE NOCASE"
            } else {
                "COLLATE BINARY"
            },
        );
        let ids = {
            let conn = self.db.read();
            let mut st = conn.prepare(&sql)?;
            let rows = st.query_map(
                params![
                    cursor.as_deref().unwrap_or(""),
                    project,
                    task,
                    agent,
                    scope,
                    (limit + 1) as i64,
                    title,
                    path
                ],
                |r| r.get::<_, String>(0),
            )?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let more = ids.len() > limit;
        let mut items = Vec::new();
        for id in ids.iter().take(limit) {
            match kind {
                Kind::Session => {
                    if let Some(session) = self.db.get_session(id)? {
                        items.push(session_value(&session));
                    }
                }
                Kind::Task => {
                    if let Some(task) = self.db.get_workstream(id)? {
                        items.push(json!(task));
                    }
                }
                Kind::Project => {
                    if let Some(project) = self.db.get_project(id)? {
                        items.push(json!(project));
                    }
                }
            }
        }
        Ok(json!({"items":items,"next_cursor":if more {ids.get(limit-1)} else {None},"end":!more}))
    }
}
fn parse_agent(agent: Option<String>) -> CliResult<Option<String>> {
    agent
        .map(|name| {
            crate::domain::Agent::parse(&name)
                .map(|a| a.as_str().to_string())
                .ok_or_else(|| CliError::new("invalid_arguments", "Agent 无效"))
        })
        .transpose()
}
fn session_value(s: &crate::domain::Session) -> Value {
    json!({"id":s.id,"title":s.title,"agent":s.agent,"working_directory":s.cwd,"project_id":s.project_id,"task_ids":s.tracked_workstream_ids,"archived_at":s.archived_at,"started_at":s.started_at,"last_activity_at":s.last_activity_at,"last_conversation_at":s.last_conversation_at})
}
