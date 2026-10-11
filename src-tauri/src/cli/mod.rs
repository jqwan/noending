//! Console entry and app-owned business commands. CLI clients never open SQLite.
mod arguments;
mod installation;
mod messages;
mod search;
mod service;
pub(crate) mod transport;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
pub use service::{start, Server};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliError {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}
pub type CliResult<T> = std::result::Result<T, CliError>;
impl CliError {
    pub(crate) fn new(code: &str, message: impl ToString) -> Self {
        Self {
            code: code.into(),
            message: message.to_string(),
            details: None,
        }
    }
}
impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for CliError {}
impl From<crate::error::AppError> for CliError {
    fn from(error: crate::error::AppError) -> Self {
        use crate::error::AppError;
        let code = match &error {
            AppError::ContextUpdateFailed(failure) => failure.code.as_str(),
            AppError::Context(crate::domain::ContextUpdateError::ConcurrencyConflict(_)) => {
                "stale_result"
            }
            AppError::Context(crate::domain::ContextUpdateError::AiUnavailable(_)) => {
                "agent_unavailable"
            }
            AppError::Context(crate::domain::ContextUpdateError::ModelCallFailed(_)) => {
                "agent_failed"
            }
            AppError::Context(crate::domain::ContextUpdateError::InvalidOutput(_)) => {
                "invalid_result"
            }
            AppError::Context(crate::domain::ContextUpdateError::Storage(_)) => "storage_error",
            AppError::Db(_) => "database_error",
            AppError::Io(_) => "io_error",
            AppError::Json(_) => "invalid_json",
            AppError::Other(_) => "operation_rejected",
        };
        Self::new(code, &error)
    }
}
impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        Self::new("io_error", e)
    }
}
impl From<serde_json::Error> for CliError {
    fn from(e: serde_json::Error) -> Self {
        Self::new("invalid_json", e)
    }
}
impl From<rusqlite::Error> for CliError {
    fn from(e: rusqlite::Error) -> Self {
        Self::new("database_error", e)
    }
}
pub(crate) fn envelope(result: CliResult<Value>) -> Value {
    match result {
        Ok(data) => json!({"schema_version":1,"ok":true,"data":data}),
        Err(error) => json!({"schema_version":1,"ok":false,"error":error}),
    }
}
fn discovery_path() -> CliResult<PathBuf> {
    use crate::workspace::home::{BootstrapPointer, HomeRequest, NoEndingHome, StartupInputs};
    let inputs = StartupInputs::from_environment();
    let pointer = inputs
        .pointer_path
        .as_ref()
        .map(|p| BootstrapPointer::load(p).0);
    let home = NoEndingHome::resolve(&HomeRequest {
        explicit: inputs.explicit.as_deref(),
        bootstrap: pointer.as_ref().and_then(|p| p.current_home.as_deref()),
        user_home: inputs.user_home.as_deref(),
        style: None,
    })?;
    Ok(home.runtime_dir.join("cli.json"))
}
pub fn run(args: Vec<String>) -> i32 {
    let result = execute(&args);
    let code = if result.is_ok() { 0 } else { 1 };
    println!("{}", envelope(result));
    code
}
pub(crate) fn execute(args: &[String]) -> CliResult<Value> {
    arguments::parse(args).and_then(|invocation| match invocation {
        arguments::Invocation::Help(value) => Ok(value),
        arguments::Invocation::Install { directory, action } => {
            installation::run(&directory, &action)
        }
        arguments::Invocation::Request {
            connection,
            command,
            wait,
            request_id,
        } => {
            let connection = connection.map(Ok).unwrap_or_else(discovery_path)?;
            let result = transport::request(&connection, command).map_err(|mut error| {
                if let Some(id) = &request_id {
                    error.details.get_or_insert_with(|| json!({}))["request_id"] = json!(id);
                }
                error
            })?;
            let result = if let Some(timeout) = wait {
                let id = result["job_id"]
                    .as_str()
                    .ok_or_else(|| CliError::new("invalid_response", "缺少作业 ID"))?;
                wait_job(&connection, id, timeout)
            } else {
                Ok(result)
            };
            result
                .map(|mut value| {
                    if let Some(id) = &request_id {
                        value["request_id"] = json!(id);
                    }
                    value
                })
                .map_err(|mut error| {
                    if let Some(id) = &request_id {
                        error.details.get_or_insert_with(|| json!({}))["request_id"] = json!(id);
                    }
                    error
                })
        }
    })
}
fn wait_job(connection: &Path, id: &str, timeout: std::time::Duration) -> CliResult<Value> {
    let started = std::time::Instant::now();
    loop {
        let job = transport::request(connection, json!({"action":"job","id":id})).map_err(
            |mut error| {
                error.details.get_or_insert_with(|| json!({}))["job_id"] = json!(id);
                error
            },
        )?;
        match job["status"].as_str() {
            Some("completed") => return Ok(job),
            Some("failed") => {
                let mut error: CliError = serde_json::from_value(job["error"].clone())?;
                error.details = Some(job);
                return Err(error);
            }
            Some("running") => {}
            _ => return Err(CliError::new("invalid_response", "作业状态无效")),
        }
        if started.elapsed() >= timeout {
            let mut error =
                CliError::new("wait_timeout", "等待超时，作业仍可通过 job show/wait 查询");
            error.details = Some(job);
            return Err(error);
        }
        std::thread::sleep(
            std::time::Duration::from_millis(200).min(timeout.saturating_sub(started.elapsed())),
        );
    }
}
pub fn run_if_requested() -> Option<i32> {
    let mut args: Vec<_> = std::env::args().skip(1).collect();
    let short = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s == "ne"))
        .unwrap_or(false);
    if args.is_empty() && !short {
        return None;
    }
    if args.first().is_some_and(|a| a == "--cli") {
        args.remove(0);
    }
    attach_parent_console();
    Some(run(args))
}
fn attach_parent_console() {
    #[cfg(windows)]
    unsafe {
        #[link(name = "kernel32")]
        extern "system" {
            fn AttachConsole(process_id: u32) -> i32;
            fn GetStdHandle(kind: u32) -> *mut std::ffi::c_void;
        }
        // Preserve redirected stdout supplied by an Agent or another process.
        // Only a GUI-subsystem entry without stdout needs the parent console.
        let stdout = GetStdHandle((-11i32) as u32);
        if stdout.is_null() || stdout as isize == -1 {
            AttachConsole(u32::MAX);
        }
    }
}
pub(crate) fn executable() -> std::io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let console = exe.with_file_name(if cfg!(windows) { "ne.exe" } else { "ne" });
    Ok(if console.is_file() { console } else { exe })
}
pub(crate) fn shell_quote(path: &Path) -> String {
    let text = path.to_string_lossy();
    #[cfg(windows)]
    {
        format!("'{}'", text.replace('\'', "''"))
    }
    #[cfg(not(windows))]
    {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}
#[cfg(test)]
mod tests;

/// Managed private entry tied to this running Home, without changing PATH.
pub fn install_workspace_entry(
    directory: &std::path::Path,
    connection: &std::path::Path,
    exe: &std::path::Path,
) -> CliResult<serde_json::Value> {
    installation::run_connected(directory, "update", Some(connection), Some(exe))
}
