//! Local file IPC shared by the desktop CLI and run-scoped extraction tools.
use super::{CliError, CliResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MAX_REQUEST: u64 = 4 * 1024 * 1024;
#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct Connection {
    pub version: u8,
    pub directory: PathBuf,
    pub token: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    token: String,
    command: Value,
}
pub(crate) fn private_dir(path: &Path) -> CliResult<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
pub(crate) fn private_write(path: &Path, content: &str) -> CliResult<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(content.as_bytes())?;
    Ok(())
}
impl Connection {
    pub fn create(directory: PathBuf) -> CliResult<Self> {
        private_dir(&directory)?;
        let connection = Self {
            version: 1,
            directory,
            token: crate::storage::new_id(),
        };
        private_write(&connection.directory.join("active"), &connection.token)?;
        Ok(connection)
    }
    pub fn publish(&self, path: &Path) -> CliResult<()> {
        let temp = path.with_extension(format!("{}.tmp", self.token));
        private_write(&temp, &serde_json::to_string(self)?)?;
        // Windows rename cannot replace an existing file. Discovery is allowed
        // to be briefly unavailable; an old connection never becomes a new one.
        #[cfg(windows)]
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        std::fs::rename(temp, path)?;
        Ok(())
    }
    pub fn deactivate(&self) {
        let _ = std::fs::remove_file(self.directory.join("active"));
    }
}

pub(crate) fn serve_pending(connection: &Connection, dispatch: impl Fn(Value) -> CliResult<Value>) {
    let Ok(entries) = std::fs::read_dir(&connection.directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("request") {
            continue;
        }
        let result = (|| {
            let metadata = std::fs::symlink_metadata(&path)?;
            if !metadata.file_type().is_file() || metadata.len() > MAX_REQUEST {
                return Err(CliError::new("invalid_request", "请求过长或不是普通文件"));
            }
            let request: Request = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
            if request.token != connection.token {
                return Err(CliError::new("invalid_connection", "连接凭据无效"));
            }
            dispatch(request.command)
        })();
        let response = super::envelope(result);
        let output = path.with_extension("response");
        let temp = path.with_extension("tmp");
        if private_write(&temp, &response.to_string()).is_ok() {
            let _ = std::fs::rename(temp, output);
        }
        let _ = std::fs::remove_file(path);
    }
}

pub(crate) fn request(path: &Path, command: Value) -> CliResult<Value> {
    let text = std::fs::read_to_string(path)
        .map_err(|_| CliError::new("app_unavailable", "NoEnding 未运行或连接不可用"))?;
    let connection: Connection = serde_json::from_str(&text)?;
    if connection.version != 1 {
        return Err(CliError::new("protocol_mismatch", "CLI 连接协议版本不匹配"));
    }
    let active = connection.directory.join("active");
    if std::fs::read_to_string(&active).ok().as_deref() != Some(connection.token.as_str()) {
        return Err(CliError::new("connection_expired", "连接已结束"));
    }
    let input = connection
        .directory
        .join(format!("{}.request", crate::storage::new_id()));
    let temp = input.with_extension("tmp");
    let output = input.with_extension("response");
    private_write(
        &temp,
        &serde_json::to_string(&Request {
            token: connection.token,
            command,
        })?,
    )?;
    std::fs::rename(temp, &input)?;
    let start = Instant::now();
    let result = loop {
        match std::fs::read_to_string(&output) {
            Ok(text) => {
                break (|| {
                    let response: Value = serde_json::from_str(&text)?;
                    if response["schema_version"] != 1 {
                        return Err(CliError::new("protocol_mismatch", "CLI 响应协议版本不匹配"));
                    }
                    if response["ok"] == true {
                        Ok(response["data"].clone())
                    } else {
                        Err(serde_json::from_value(response["error"].clone())?)
                    }
                })();
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !active.exists() || start.elapsed() > Duration::from_secs(5) {
                    break Err(CliError::new(
                        "request_timeout",
                        "NoEnding 连接已结束或请求超时；写操作请查询当前状态后再决定是否重试",
                    ));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => break Err(error.into()),
        }
    };
    let _ = std::fs::remove_file(input);
    let _ = std::fs::remove_file(output);
    result
}
