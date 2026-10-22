//! Standalone console commands sharing the desktop business modules.
mod agents;
mod arguments;
pub(crate) mod extraction_settings;
mod installation;
mod messages;
mod receipts;
mod search;
pub(crate) mod service;
mod summaries;
pub(crate) mod transport;
mod watcher;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
pub use watcher::{watch_changes, ChangeWatcher};

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
            AppError::Io(error) if error.kind() == std::io::ErrorKind::Interrupted => "cancelled",
            AppError::Io(_) => "io_error",
            AppError::Json(_) => "invalid_json",
            AppError::Other(_) => "operation_rejected",
        };
        Self::new(code, &error)
    }
}
impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        Self::new(
            if e.kind() == std::io::ErrorKind::Interrupted {
                "cancelled"
            } else {
                "io_error"
            },
            e,
        )
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
pub(crate) fn resolve_home(
    explicit: Option<&Path>,
) -> CliResult<crate::workspace::home::NoEndingHome> {
    use crate::workspace::home::{BootstrapPointer, HomeRequest, NoEndingHome, StartupInputs};
    let inputs = StartupInputs::from_environment();
    let pointer = inputs.pointer_path.as_ref().map(|p| {
        let (pointer, note) = BootstrapPointer::load(p);
        if let Some(note) = note {
            eprintln!("[noending] {note}");
        }
        pointer
    });
    // CLI follows the current Home; only desktop startup applies a pending relocation.
    let home = NoEndingHome::resolve(&HomeRequest {
        explicit: explicit.or(inputs.explicit.as_deref()),
        bootstrap: pointer.as_ref().and_then(|p| p.current_home.as_deref()),
        user_home: inputs.user_home.as_deref(),
        style: None,
    })?;
    home.ensure_dirs()?;
    Ok(home)
}
pub fn run(args: Vec<String>) -> i32 {
    // A stdio server owns stdout for the lifetime of its connection; never
    // print the one-shot CLI envelope after it closes, including on errors.
    let invocation = crate::platform::cancellation::install()
        .map_err(CliError::from)
        .and_then(|_| arguments::parse(&args));
    let result = match invocation {
        Ok(arguments::Invocation::Mcp { home }) => return crate::mcp::run(home.as_deref()),
        other => other.and_then(execute_invocation),
    };
    let code = if result.is_ok() { 0 } else { 1 };
    println!("{}", envelope(result));
    code
}
#[cfg(test)]
pub(crate) fn execute(args: &[String]) -> CliResult<Value> {
    arguments::parse(args).and_then(execute_invocation)
}
fn execute_invocation(invocation: arguments::Invocation) -> CliResult<Value> {
    match invocation {
        arguments::Invocation::Help(value) => Ok(value),
        arguments::Invocation::Mcp { .. } => Err(CliError::new(
            "invalid_arguments",
            "MCP 必须通过独立进程运行",
        )),
        arguments::Invocation::Install { directory, action } => {
            installation::run(&directory, &action)
        }
        arguments::Invocation::Request {
            home,
            connection,
            command,
            request_id,
        } => {
            let result = (|| {
                if let Some(connection) = connection {
                    return transport::request(&connection, command);
                }
                let home = resolve_home(home.as_deref())?;
                let service = service::Service::open(home)?;
                service.dispatch(serde_json::from_value(command)?)
            })();
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

/// Managed private entry tied to this Home, without changing PATH.
pub fn install_workspace_entry(
    directory: &std::path::Path,
    home: &std::path::Path,
    exe: &std::path::Path,
) -> CliResult<serde_json::Value> {
    installation::run_for_home(directory, "update", Some(home), Some(exe))
}
