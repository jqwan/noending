use super::{transport, CliError, CliResult};
use crate::{
    context,
    storage::{self, Db},
    workspace::{home::NoEndingHome, wiring::WorkspaceLayer},
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Kind {
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
pub(super) enum Command {
    ExecuteOnce {
        request_id: String,
        command: Box<Command>,
    },
    Receipt {
        id: String,
    },
    Status,
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
    },
    EditSummary {
        kind: Kind,
        id: String,
        revision: i64,
        content: String,
    },
    Extract {
        kind: Kind,
        id: String,
        regenerate: bool,
    },
    Preferences {
        project: Option<String>,
    },
    AddPreference {
        project: Option<String>,
        content: String,
    },
    UpdatePreference {
        id: String,
        content: String,
    },
    DeletePreference {
        id: String,
    },
    CreateTask {
        title: String,
        description: String,
        paths: Vec<String>,
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
    TaskPaths {
        id: String,
    },
    AddTaskPath {
        id: String,
        path: String,
    },
    RemoveTaskPath {
        id: String,
        path_id: String,
    },
    ReorderTaskPaths {
        id: String,
        path_ids: Vec<String>,
    },
    Attach {
        task: String,
        session: String,
    },
    Detach {
        task: String,
        session: String,
    },
    Jobs,
    Job {
        id: String,
    },
}
impl Command {
    pub(super) fn is_write(&self) -> bool {
        matches!(
            self,
            Self::EditSummary { .. }
                | Self::Extract { .. }
                | Self::AddPreference { .. }
                | Self::UpdatePreference { .. }
                | Self::DeletePreference { .. }
                | Self::CreateTask { .. }
                | Self::UpdateTask { .. }
                | Self::Archive { .. }
                | Self::RenameProject { .. }
                | Self::RegisterPath { .. }
                | Self::AddTaskPath { .. }
                | Self::RemoveTaskPath { .. }
                | Self::ReorderTaskPaths { .. }
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

struct Receipt {
    command_hash: [u8; 32],
    result: Option<CliResult<Value>>,
    size: usize,
}
pub struct Server {
    stop: Arc<AtomicBool>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    connection: transport::Connection,
    discovery: PathBuf,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown();
    }
}
impl Server {
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Release);
        self.connection.deactivate();
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
        if std::fs::read_to_string(&self.discovery)
            .ok()
            .and_then(|s| serde_json::from_str::<transport::Connection>(&s).ok())
            .is_some_and(|c| c.token == self.connection.token)
        {
            let _ = std::fs::remove_file(&self.discovery);
        }
        let _ = std::fs::remove_dir_all(&self.connection.directory);
    }
}
pub(super) struct Service {
    pub db: Arc<Db>,
    pub home: NoEndingHome,
    pub layer: Arc<WorkspaceLayer>,
    jobs: Mutex<BTreeMap<String, Value>>,
    receipts: Mutex<BTreeMap<String, Receipt>>,
    changed: Arc<dyn Fn() + Send + Sync>,
}
pub fn start(
    db: Arc<Db>,
    home: NoEndingHome,
    layer: Arc<WorkspaceLayer>,
    changed: impl Fn() + Send + Sync + 'static,
) -> CliResult<Server> {
    let discovery = home.runtime_dir.join("cli.json");
    let directory = home.runtime_dir.join("cli").join(storage::new_id());
    let connection = transport::Connection::create(directory)?;
    connection.publish(&discovery)?;
    let service = Arc::new(Service {
        db,
        home,
        layer,
        jobs: Mutex::default(),
        receipts: Mutex::default(),
        changed: Arc::new(changed),
    });
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    let worker_connection = connection.clone();
    let worker = std::thread::spawn(move || {
        while !worker_stop.load(Ordering::Acquire) {
            transport::serve_pending(&worker_connection, |value| {
                let command = serde_json::from_value(value)
                    .map_err(|e| CliError::new("invalid_command", e))?;
                service.dispatch(command)
            });
            // This listener is idle for most of the app's lifetime. Extraction
            // has its own faster, short-lived listener for repeated Agent reads.
            std::thread::park_timeout(std::time::Duration::from_millis(100));
        }
    });
    Ok(Server {
        stop,
        worker: Mutex::new(Some(worker)),
        connection,
        discovery,
    })
}
impl Service {
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
    pub(super) fn dispatch(self: &Arc<Self>, command: Command) -> CliResult<Value> {
        match command {
            Command::ExecuteOnce {
                request_id,
                command,
            } => self.execute_once(&request_id, *command),
            Command::Receipt { id } => {
                let receipts = self.receipts.lock().unwrap();
                let receipt = receipts.get(&id).ok_or_else(|| {
                    CliError::new("not_found", "请求不存在（请求记录仅在本次应用运行中保留）")
                })?;
                Ok(
                    json!({"request_id":id,"status":if receipt.result.is_some(){"completed"}else{"running"},"response":receipt.result.clone().map(super::envelope)}),
                )
            }
            Command::Status => Ok(
                json!({"connected":true,"version":env!("CARGO_PKG_VERSION"),"home":self.home.root,"capabilities":["search","session","task","project","preference","summary","job","request"],"running_jobs":self.jobs.lock().unwrap().values().filter(|j|j["status"]=="running").count()}),
            ),
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
                        json!({"task":self.db.get_workstream(&id)?,"paths":crate::workspace::workstream::list_workstream_path_views(&self.db,&id)?,"projects":crate::workspace::workstream::projects_for_workstream(&self.db,&id)?,"sessions_command":format!("ne session list --task {id} --scope all")}),
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
            Command::Summary { kind, id } => self.summary(kind, &id),
            Command::EditSummary {
                kind,
                id,
                revision,
                content,
            } => {
                self.require(kind, &id)?;
                let revision =
                    context::edit_summary(&self.db, kind.name(), &id, revision, &content)?;
                (self.changed)();
                Ok(json!({"kind":kind,"id":id,"revision":revision}))
            }
            Command::Extract {
                kind,
                id,
                regenerate,
            } => {
                self.require(kind, &id)?;
                let job = storage::new_id();
                {
                    let mut jobs = self.jobs.lock().unwrap();
                    if jobs.values().any(|j| {
                        j["kind"] == kind.name() && j["target_id"] == id && j["status"] == "running"
                    }) {
                        return Err(CliError::new("busy", "此目标正在提取"));
                    }
                    // Keep at most 100 completed jobs, never evict a running job.
                    if jobs.len() >= 100 {
                        if let Some(old) = jobs
                            .iter()
                            .filter(|(_, v)| v["status"] != "running")
                            .min_by_key(|(_, v)| v["finished_at"].as_str().unwrap_or(""))
                            .map(|(k, _)| k.clone())
                        {
                            jobs.remove(&old);
                        } else {
                            return Err(CliError::new("busy", "当前提取作业过多"));
                        }
                    }
                    jobs.insert(job.clone(),json!({"job_id":job,"kind":kind,"target_id":id,"regenerate":regenerate,"status":"running","started_at":storage::now()}));
                }
                let service = self.clone();
                let worker_job = job.clone();
                std::thread::spawn(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        service.extract(kind, &id, regenerate)
                    }))
                    .unwrap_or_else(|_| Err(CliError::new("internal_error", "摘要提取异常终止")));
                    let mut jobs = service.jobs.lock().unwrap();
                    if let Some(job) = jobs.get_mut(&worker_job) {
                        job["finished_at"] = json!(storage::now());
                        match result {
                            Ok(value) => {
                                job["status"] = json!("completed");
                                job["result"] = value;
                            }
                            Err(error) => {
                                job["status"] = json!("failed");
                                job["error"] = json!(error);
                            }
                        }
                    }
                    drop(jobs);
                    (service.changed)();
                });
                Ok(json!({"job_id":job,"status":"running"}))
            }
            Command::Preferences { project } => {
                if let Some(id) = &project {
                    self.require(Kind::Project, id)?;
                }
                Ok(json!({"items":storage::preferences::list_items(&self.db,project.as_deref())?}))
            }
            Command::AddPreference { project, content } => {
                if let Some(id) = &project {
                    self.require(Kind::Project, id)?;
                }
                let value = storage::preferences::save_item(
                    &self.db,
                    None,
                    project.as_deref(),
                    &content,
                    false,
                )?;
                (self.changed)();
                Ok(json!(value))
            }
            Command::UpdatePreference { id, content } => {
                self.change_preference(&id, Some(&content))
            }
            Command::DeletePreference { id } => self.change_preference(&id, None),
            Command::CreateTask {
                title,
                description,
                paths,
            } => {
                let report = crate::workspace::workstream::create_workstream(
                    &self.db,
                    &*self.layer,
                    &title,
                    &description,
                    &paths,
                )?;
                (self.changed)();
                Ok(json!({"task":report.workstream,"paths":report.paths}))
            }
            Command::UpdateTask {
                id,
                title,
                description,
            } => {
                self.require(Kind::Task, &id)?;
                let task = crate::workspace::workstream::update_workstream_fields(
                    &self.db,
                    &id,
                    title.as_deref(),
                    description.as_deref(),
                )?;
                (self.changed)();
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
                (self.changed)();
                Ok(value)
            }
            Command::RenameProject { id, name } => {
                self.require(Kind::Project, &id)?;
                let project = crate::workspace::project::rename_project(&self.db, &id, &name)?;
                (self.changed)();
                Ok(json!({"project":project}))
            }
            Command::RegisterPath { path } => {
                let path = self.register_path(&path)?;
                let project = self.db.get_project(&path.project_id)?;
                (self.changed)();
                Ok(json!({"path":path,"project":project}))
            }
            Command::TaskPaths { id } => {
                self.require(Kind::Task, &id)?;
                Ok(
                    json!({"items":crate::workspace::workstream::list_workstream_path_views(&self.db,&id)?}),
                )
            }
            Command::AddTaskPath { id, path } => {
                self.require(Kind::Task, &id)?;
                let registered = self.register_path(&path)?;
                let path = crate::workspace::workstream::add_registered_workstream_path(
                    &self.db,
                    &id,
                    &registered.id,
                )?;
                (self.changed)();
                Ok(json!({"path":path}))
            }
            Command::RemoveTaskPath { id, path_id } => {
                self.require(Kind::Task, &id)?;
                let row = self
                    .db
                    .list_workstream_paths(&id)?
                    .into_iter()
                    .find(|p| p.workspace_path_id == path_id)
                    .ok_or_else(|| CliError::new("scope_mismatch", "该目录未关联此任务"))?;
                crate::workspace::workstream::remove_workstream_path(&self.db, &id, &row.id)?;
                (self.changed)();
                Ok(json!({"task_id":id,"workspace_path_id":path_id,"removed":true}))
            }
            Command::ReorderTaskPaths { id, path_ids } => {
                self.require(Kind::Task, &id)?;
                let paths = crate::workspace::workstream::reorder_workstream_paths(
                    &self.db, &id, &path_ids,
                )?;
                (self.changed)();
                Ok(json!({"items":paths}))
            }
            Command::Attach { task, session } => {
                self.require(Kind::Task, &task)?;
                self.require(Kind::Session, &session)?;
                let session =
                    crate::workspace::session::set_session_owner(&self.db, &session, Some(&task))?;
                (self.changed)();
                Ok(session_value(&session))
            }
            Command::Detach { task, session } => {
                self.require(Kind::Task, &task)?;
                self.require(Kind::Session, &session)?;
                // Compare within the write transaction so a newer reassignment
                // can never be cleared by a stale detach request.
                let mut conn = self.db.write();
                let tx = conn.transaction()?;
                let owner: Option<String> = tx.query_row(
                    "SELECT owner_workstream_id FROM sessions WHERE id=?1",
                    [&session],
                    |r| r.get(0),
                )?;
                if owner.as_deref() != Some(&task) {
                    return Err(CliError::new("scope_mismatch", "会话不属于此任务"));
                }
                // Owner mutation is the existing storage service; release the
                // transaction only after applying the same business operation.
                storage::set_session_owner_conn(&tx, &session, None)?;
                tx.commit()?;
                drop(conn);
                (self.changed)();
                Ok(json!({"session_id":session,"task_id":null}))
            }
            Command::Jobs => {
                Ok(json!({"items":self.jobs.lock().unwrap().values().cloned().collect::<Vec<_>>()}))
            }
            Command::Job { id } => self.jobs.lock().unwrap().get(&id).cloned().ok_or_else(|| {
                CliError::new("not_found", "作业不存在（作业记录仅在本次应用运行中保留）")
            }),
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
        if !command.is_write()
            || id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(CliError::new(
                "invalid_arguments",
                "请求编号需为 1～128 个字母、数字、短横线或下划线，且只支持写操作",
            ));
        }
        use sha2::{Digest, Sha256};
        let payload: [u8; 32] = Sha256::digest(serde_json::to_vec(&command)?).into();
        {
            let mut receipts = self.receipts.lock().unwrap();
            if let Some(receipt) = receipts.get(id) {
                if receipt.command_hash != payload {
                    return Err(CliError::new(
                        "request_conflict",
                        "此请求编号已用于其他操作",
                    ));
                }
                return receipt.result.clone().unwrap_or_else(|| {
                    Err(CliError::new(
                        "request_running",
                        "请求仍在执行，请查询 request show",
                    ))
                });
            }
            // Never evict a receipt and risk executing a reused ID twice.
            if receipts.len() >= 1000
                || receipts.values().map(|r| r.size).sum::<usize>() >= 64 * 1024 * 1024
            {
                return Err(CliError::new(
                    "request_limit",
                    "本次运行的写请求记录已满，请重启应用后继续",
                ));
            }
            receipts.insert(
                id.into(),
                Receipt {
                    command_hash: payload,
                    result: None,
                    size: 0,
                },
            );
        }
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.dispatch(command)))
                .unwrap_or_else(|_| {
                    Err(CliError::new(
                        "internal_error",
                        "操作异常终止，请查询业务状态",
                    ))
                });
        let mut receipts = self.receipts.lock().unwrap();
        let receipt = receipts.get_mut(id).unwrap();
        receipt.size = serde_json::to_vec(&super::envelope(result.clone()))?.len();
        receipt.result = Some(result.clone());
        result
    }
    fn summary(&self, kind: Kind, id: &str) -> CliResult<Value> {
        self.require(kind, id)?;
        Ok(match kind {
            Kind::Session => {
                let view = context::session_context_view(&self.db, id)?;
                json!({"kind":kind,"id":id,"summary":view.fields.map(|f|f.summary),"revision":view.revision,"updated_at":view.updated_at,"pending":view.pending,"ingest_generation":view.ingest_generation,"processed_through_seq":view.processed_through_seq,"latest_message_seq":view.latest_message_seq})
            }
            Kind::Task => {
                let view = context::workstream_context_view(&self.db, id)?;
                json!({"kind":kind,"id":id,"summary":view.summary,"revision":view.context_revision,"updated_at":view.updated_at,"pending":view.pending,"pending_sessions":view.pending_sessions})
            }
            Kind::Project => {
                let view = context::project_context_view(&self.db, id)?;
                json!({"kind":kind,"id":id,"summary":view.summary,"revision":view.context_revision,"updated_at":view.updated_at,"pending":view.pending,"pending_sessions":view.pending_sessions})
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
    fn change_preference(&self, id: &str, content: Option<&str>) -> CliResult<Value> {
        let mut conn = self.db.write();
        let tx = conn.transaction()?;
        let current = storage::preferences::get(&tx, id)?
            .ok_or_else(|| CliError::new("not_found", "偏好不存在"))?;
        if current.locked {
            return Err(CliError::new("locked", "偏好已锁定，请在界面解锁后编辑"));
        }
        let result = if let Some(content) = content {
            json!(storage::preferences::update(&tx, id, content, false)?)
        } else {
            tx.execute("DELETE FROM preferences WHERE id=?1", [id])?;
            json!({"deleted":id})
        };
        tx.commit()?;
        drop(conn);
        (self.changed)();
        Ok(result)
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
            Kind::Session=>"SELECT s.id FROM sessions s LEFT JOIN workspace_paths wp ON wp.id=s.workspace_path_id WHERE s.id>?1 AND (?2 IS NULL OR wp.project_id=?2) AND (?3 IS NULL OR s.owner_workstream_id=?3) AND (?4 IS NULL OR s.agent=?4) AND (?5='all' OR (?5='archived' AND s.archived_at IS NOT NULL) OR (?5='unarchived' AND s.archived_at IS NULL)) AND (?7 IS NULL OR instr(lower(s.title),lower(?7))>0) AND (?8 IS NULL OR COALESCE(wp.canonical_path,s.cwd)=?8 COLLATE NOCASE) ORDER BY s.id LIMIT ?6",
            Kind::Task=>"SELECT s.id FROM workstreams s WHERE s.id>?1 AND (?2 IS NULL OR EXISTS(SELECT 1 FROM workstream_paths p JOIN workspace_paths wp ON wp.id=p.workspace_path_id WHERE p.workstream_id=s.id AND wp.project_id=?2)) AND (?5='all' OR (?5='archived' AND s.visibility='archived') OR (?5='unarchived' AND s.visibility='normal')) AND ?3 IS NULL AND ?4 IS NULL AND (?7 IS NULL OR instr(lower(s.title),lower(?7))>0) AND (?8 IS NULL OR EXISTS(SELECT 1 FROM workstream_paths p JOIN workspace_paths wp ON wp.id=p.workspace_path_id WHERE p.workstream_id=s.id AND wp.canonical_path=?8 COLLATE NOCASE)) ORDER BY s.id LIMIT ?6",
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
    json!({"id":s.id,"title":s.title,"agent":s.agent,"working_directory":s.cwd,"project_id":s.project_id,"task_id":s.owner_workstream_id,"archived_at":s.archived_at,"started_at":s.started_at,"last_activity_at":s.last_activity_at,"last_conversation_at":s.last_conversation_at})
}
