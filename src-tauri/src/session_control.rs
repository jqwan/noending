//! Home-scoped desktop control. Data-only CLI/MCP commands remain standalone.
use crate::{
    agent_runtime::AgentRuntimeOverrides,
    cli::{receipts, service::Command, transport, CliError, CliResult},
    workspace::home::NoEndingHome,
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs::File,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tauri::Manager;

pub(crate) const CALLER_TERMINAL_ENV: &str = "NOENDING_TERMINAL_ID";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StartSession {
    pub agent: String,
    pub cwd: Option<String>,
    #[serde(default)]
    pub tasks: Vec<String>,
    pub message: Option<String>,
    pub runtime: Option<AgentRuntimeOverrides>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlCommand {
    List { session: Option<String> },
    Identity { claimed: Option<String> },
    Start(StartSession),
}
impl ControlCommand {
    pub(crate) fn is_write(&self) -> bool {
        !matches!(self, Self::List { .. } | Self::Identity { .. })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    command: ControlCommand,
    request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sender_terminal: Option<String>,
}

fn endpoint(home: &NoEndingHome) -> PathBuf {
    home.runtime_dir.join("desktop-control.json")
}
fn lease_path(home: &NoEndingHome) -> PathBuf {
    home.runtime_dir.join("desktop-control.lock")
}
fn unavailable() -> CliError {
    CliError::new(
        "desktop_unavailable",
        "会话终端控制需要同一数据目录的 NoEnding 桌面应用正在运行；其他数据命令无需应用运行",
    )
}

/// The desktop owns side-effect receipts, so a lost CLI connection cannot cause
/// a second process launch or message submission on a retry.
pub(crate) fn request(
    home: &NoEndingHome,
    command: ControlCommand,
    request_id: Option<String>,
) -> CliResult<Value> {
    let lease = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(lease_path(home))
        .map_err(|_| unavailable())?;
    match crate::platform::file_lock::try_lock(&lease) {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        Ok(()) => return Err(unavailable()),
        Err(error) => return Err(error.into()),
    }
    transport::request_with_timeout(
        &endpoint(home),
        serde_json::to_value(Request {
            command,
            request_id,
            sender_terminal: std::env::var(CALLER_TERMINAL_ENV)
                .ok()
                .filter(|s| !s.is_empty()),
        })?,
        Duration::from_secs(30),
    )
}

pub struct DesktopHost {
    connection: transport::Connection,
    endpoint: PathBuf,
    stopped: Arc<AtomicBool>,
    _lease: File,
    _access: crate::workspace::home::HomeAccess,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl DesktopHost {
    pub fn start(
        home: &NoEndingHome,
        dispatch: impl Fn(ControlCommand, Option<String>) -> CliResult<Value> + Send + 'static,
    ) -> CliResult<Self> {
        let access = home.acquire_access()?;
        let lease = crate::platform::file_lock::exclusive(&lease_path(home)).map_err(|_| {
            CliError::new("desktop_already_running", "此数据目录已有桌面会话控制服务")
        })?;
        let connection = transport::Connection::create(
            home.runtime_dir
                .join(format!("desktop-control-{}", crate::storage::new_id())),
        )?;
        let endpoint = endpoint(home);
        connection.publish(&endpoint)?;
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stop = stopped.clone();
        let worker_connection = connection.clone();
        let worker_home = home.clone();
        let mut host = Self {
            connection,
            endpoint,
            stopped,
            _lease: lease,
            _access: access,
            worker: None,
        };
        host.worker = Some(
            std::thread::Builder::new()
                .name("desktop-session-control".into())
                .spawn(move || {
                    while !worker_stop.load(Ordering::Acquire) {
                        transport::serve_pending(&worker_connection, |value| {
                            if worker_stop.load(Ordering::Acquire) {
                                return Err(unavailable());
                            }
                            let request: Request = serde_json::from_value(value)?;
                            if request.command.is_write() {
                                let id = request.request_id.ok_or_else(|| {
                                    CliError::new("invalid_arguments", "会话写操作需要请求编号")
                                })?;
                                let command = Command::SessionControl {
                                    command: request.command.clone(),
                                };
                                let payload = if let Some(sender) = &request.sender_terminal {
                                    serde_json::to_vec(&(command, sender))?
                                } else {
                                    serde_json::to_vec(&command)?
                                };
                                receipts::execute(&worker_home, &id, &payload, || {
                                    dispatch(request.command, request.sender_terminal)
                                })
                            } else if request.request_id.is_some() {
                                Err(CliError::new("invalid_arguments", "查询操作不接受请求编号"))
                            } else {
                                dispatch(request.command, request.sender_terminal)
                            }
                        });
                        std::thread::sleep(Duration::from_millis(50));
                    }
                })?,
        );
        Ok(host)
    }
}
impl Drop for DesktopHost {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.connection.deactivate();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if std::fs::read_to_string(&self.endpoint)
            .ok()
            .and_then(|s| serde_json::from_str::<transport::Connection>(&s).ok())
            .is_some_and(|value| value.token == self.connection.token)
        {
            let _ = std::fs::remove_file(&self.endpoint);
        }
        let _ = std::fs::remove_dir_all(&self.connection.directory);
    }
}

pub(crate) fn sender_session(
    registry: &crate::terminal::TerminalRegistry,
    db: &crate::storage::Db,
    terminal: Option<&str>,
    claimed: Option<&str>,
) -> CliResult<Option<String>> {
    if let Some(terminal) = terminal {
        registry.bind_discovered(db);
        let current = registry
            .attach(terminal)
            .map_err(|_| {
                CliError::new(
                    "sender_unavailable",
                    "来源终端已关闭或不属于此 NoEnding Home",
                )
            })?
            .summary;
        if !current.live {
            return Err(CliError::new("sender_unavailable", "来源终端已退出"));
        }
        let session = current.session_id.ok_or_else(|| {
            CliError::new(
                "sender_unbound",
                "来源终端尚未绑定会话，请等待会话摄入后再发送",
            )
        })?;
        if claimed.is_some_and(|id| id != session) {
            return Err(CliError::new(
                "sender_identity_changed",
                "来源会话与当前终端绑定不一致",
            ));
        }
        Ok(Some(session))
    } else if let Some(session) = claimed {
        if db.get_session(session)?.is_none() {
            return Err(CliError::new("sender_not_found", "来源会话不存在"));
        }
        Ok(Some(session.into()))
    } else {
        Ok(None)
    }
}

pub(crate) fn dispatch(
    app: &tauri::AppHandle,
    command: ControlCommand,
    sender_terminal: Option<&str>,
) -> CliResult<Value> {
    let registry = app.state::<crate::terminal::TerminalRegistry>();
    let state = app.state::<crate::commands::AppState>();
    match command {
        ControlCommand::List { session } => Ok(
            json!({"items":registry.list().into_iter().filter(|t|session.as_ref().is_none_or(|s|t.session_id.as_ref()==Some(s))).collect::<Vec<_>>()}),
        ),
        ControlCommand::Identity { claimed } => Ok(
            json!({"session_id":sender_session(&registry,&state.db,sender_terminal,claimed.as_deref())?}),
        ),
        ControlCommand::Start(options) => {
            if let Some(cwd) = &options.cwd {
                if !std::path::Path::new(cwd).is_absolute() || !std::path::Path::new(cwd).is_dir() {
                    return Err(CliError::new(
                        "invalid_arguments",
                        "cwd 必须是已存在的绝对目录",
                    ));
                }
            }
            let message = options.message.filter(|s| !s.trim().is_empty());
            let result = crate::commands::launch_embedded_new(
                app.clone(),
                state,
                registry,
                options.agent,
                options.tasks,
                options.cwd,
                message,
                options.runtime,
            )?;
            Ok(json!({"launch":result,"session_id":Value::Null,"binding":"pending"}))
        }
    }
}
