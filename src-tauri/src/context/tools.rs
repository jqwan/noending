//! Run-scoped CLI bridge. The app owns query scopes and commits; the helper never
//! opens SQLite. Connections disappear with the extraction, including on error.
use super::summary::{self, BatchReply, ExtractionFiles, SessionReply};
use crate::error::{other, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

const MAX_REQUEST: u64 = 4 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(5);
const SKILL: &str = include_str!("../../skills/noending-extraction/SKILL.md");
const PROTOCOL: &str = include_str!("../../skills/noending-extraction/references/protocol.md");

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Connection {
    version: u8,
    directory: std::path::PathBuf,
    token: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
    token: String,
    command: Command,
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Inputs,
    Read {
        name: String,
        cursor: usize,
        limit: usize,
    },
    Search {
        name: String,
        query: String,
        cursor: usize,
        limit: usize,
    },
}

struct Bridge<'a> {
    db: &'a crate::storage::Db,
    files: &'a ExtractionFiles,
    token: String,
    // Pagination acknowledgement proves delivery, not semantic understanding.
    delivered: Mutex<HashMap<String, HashSet<usize>>>,
}
impl Bridge<'_> {
    fn dispatch(&self, request: Request) -> Result<Value> {
        if request.token != self.token {
            return Err(other("当前提取连接无效"));
        }
        match request.command {
            Command::Inputs => {
                let inputs = self.files.inputs.lock().unwrap();
                let mut names: Vec<_> = inputs
                    .iter()
                    .map(|(name, input)| {
                        Ok(json!({"name":name,"record_count":input.count(&self.db.read())?}))
                    })
                    .collect::<Result<_>>()?;
                names.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
                Ok(json!({"inputs":names}))
            }
            Command::Read {
                name,
                cursor,
                limit,
            } => self.page(&name, cursor, limit, None),
            Command::Search {
                name,
                query,
                cursor,
                limit,
            } => {
                if query.trim().is_empty() {
                    return Err(other("搜索内容不能为空"));
                }
                self.page(&name, cursor, limit, Some(&query))
            }
        }
    }
    fn page(&self, name: &str, cursor: usize, limit: usize, query: Option<&str>) -> Result<Value> {
        if !(1..=50).contains(&limit) {
            return Err(other("limit 必须在 1 到 50 之间"));
        }
        let inputs = self.files.inputs.lock().unwrap();
        let input = inputs
            .get(name)
            .ok_or_else(|| other("输入不在本次提取范围内"))?;
        let page = input.page(self.db, self.files, cursor, limit, query)?;
        if query.is_none() {
            self.delivered
                .lock()
                .unwrap()
                .entry(name.into())
                .or_default()
                .extend(
                    page["records"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|r| r["cursor"].as_u64().unwrap() as usize),
                );
        }
        Ok(page)
    }

    fn validate(&self, reply: &Value) -> Result<()> {
        let dir_name = self.files.dir.file_name().unwrap().to_string_lossy();
        let (kind, id) = dir_name
            .split_once('-')
            .ok_or_else(|| other("提取目标无效"))?;
        let raw = serde_json::to_string(reply)?;
        let mut excluded = HashSet::new();
        let mut skipped = HashSet::new();
        let mut failed = false;
        let (target_skipped, preference_failed) = if kind == "session" {
            match summary::parse_session_reply(&raw)? {
                SessionReply::Completed { preferences } => (
                    false,
                    matches!(
                        preferences,
                        super::preferences::PreferenceReply::Failed { .. }
                    ),
                ),
                SessionReply::Skipped { preferences, .. } => {
                    skipped.insert(id.to_string());
                    (
                        true,
                        matches!(
                            preferences,
                            super::preferences::PreferenceReply::Failed { .. }
                        ),
                    )
                }
                SessionReply::Failed { .. } => {
                    failed = true;
                    (false, true)
                }
            }
        } else {
            let (errors, ids, target_skipped, pref) = match summary::parse_batch_reply(&raw)? {
                BatchReply::Completed {
                    skipped_session_ids,
                    preferences,
                } => (vec![], skipped_session_ids, false, Some(preferences)),
                BatchReply::Partial {
                    failed_sessions,
                    skipped_session_ids,
                    preferences,
                } => (
                    failed_sessions,
                    skipped_session_ids,
                    false,
                    Some(preferences),
                ),
                BatchReply::Skipped {
                    skipped_session_ids,
                    preferences,
                    ..
                } => (vec![], skipped_session_ids, true, Some(preferences)),
                BatchReply::Failed { .. } => {
                    failed = true;
                    (vec![], vec![], false, None)
                }
            };
            for item in errors {
                if item.error.trim().is_empty() || !excluded.insert(item.session_id) {
                    return Err(other("失败会话无效或重复"));
                }
            }
            for sid in ids {
                if excluded.contains(&sid) || !skipped.insert(sid) {
                    return Err(other("跳过会话重复"));
                }
            }
            (
                target_skipped,
                pref.is_none_or(|p| {
                    matches!(p, super::preferences::PreferenceReply::Failed { .. })
                }),
            )
        };
        let inputs = self.files.inputs.lock().unwrap();
        let targets: HashSet<_> = inputs
            .keys()
            .filter_map(|name| {
                name.strip_prefix("session-")
                    .and_then(|s| s.strip_suffix(".jsonl"))
            })
            .collect();
        if excluded
            .iter()
            .chain(skipped.iter())
            .any(|id| !targets.contains(id.as_str()))
        {
            return Err(other("未知会话 ID"));
        }
        let target = self.files.batch_result(kind, id);
        if failed || target_skipped {
            self.files.require_absent(&target)?;
        } else {
            self.files.read_result(&target)?;
        }
        if !failed {
            for sid in &targets {
                let result = self.files.session_result(sid);
                if excluded.contains(*sid) || skipped.contains(*sid) {
                    self.files.require_absent(&result)?;
                } else {
                    self.files.read_result(&result)?;
                }
            }
            let delivered = self.delivered.lock().unwrap();
            for (name, input) in inputs.iter() {
                let failed_input = excluded.iter().any(|sid| {
                    name == &format!("session-{sid}.jsonl")
                        || name == &format!("preferences-session-{sid}.jsonl")
                });
                if failed_input
                    || (preference_failed
                        && (name == "preferences.input.json"
                            || name.starts_with("preferences-session-")))
                {
                    continue;
                }
                if delivered.get(name).map_or(0, HashSet::len) != input.count(&self.db.read())? {
                    return Err(other(format!("输入尚未完整读取：{name}")));
                }
            }
        }
        Ok(())
    }
}

fn private_write(path: &Path, content: &str) -> Result<()> {
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

/// The Agent runs on the caller thread. Only scoped requests touch the bridge;
/// Filesystem IPC works inside Agent workspace sandboxes without network access.
/// All durable writes remain in the existing snapshot-guarded transaction.
pub(super) fn run(
    db: &crate::storage::Db,
    files: &ExtractionFiles,
    requirements: &str,
    invoke: impl FnOnce(&str) -> Result<String>,
) -> Result<String> {
    let directory = files.dir.join("requests");
    std::fs::create_dir_all(&directory)?;
    let connection = Connection {
        version: 1,
        directory,
        token: crate::storage::new_id(),
    };
    private_write(&connection.directory.join("active"), &connection.token)?;
    let connection_path = files.dir.join("connection.json");
    private_write(&connection_path, &serde_json::to_string(&connection)?)?;
    let skill_dir = files.dir.join("noending-extraction");
    std::fs::create_dir_all(skill_dir.join("references"))?;
    private_write(&skill_dir.join("SKILL.md"), SKILL)?;
    private_write(&skill_dir.join("references/protocol.md"), PROTOCOL)?;
    private_write(&skill_dir.join("references/requirements.md"), requirements)?;
    let exe = std::env::current_exe()?;
    let prompt=format!("使用提取 skill：{}。先完整读取该 SKILL.md，再按其中的流程查询材料并生成结果。\nCLI 前缀：{} extraction --connection {}\n提取要求：{}\n最终回复必须是协议规定的单个状态 JSON 对象，不带 Markdown 代码块或其他说明。后端在调用正常结束后校验结果文件并入库。",serde_json::to_string(&skill_dir.join("SKILL.md"))?,executable_command(&exe),shell_quote(&connection_path),serde_json::to_string(&skill_dir.join("references/requirements.md"))?);
    let bridge = Bridge {
        db,
        files,
        token: connection.token,
        delivered: Mutex::default(),
    };
    let stopped = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !stopped.load(Ordering::Acquire) {
                if let Ok(entries) = std::fs::read_dir(&connection.directory) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().and_then(|e| e.to_str()) != Some("request") {
                            continue;
                        }
                        let result = (|| -> Result<Value> {
                            let meta = std::fs::symlink_metadata(&path)?;
                            if !meta.file_type().is_file() || meta.len() > MAX_REQUEST {
                                return Err(other("请求过长或不是普通文件"));
                            }
                            bridge.dispatch(serde_json::from_str(&std::fs::read_to_string(&path)?)?)
                        })();
                        let response = match result {
                            Ok(data) => json!({"schema_version":1,"ok":true,"data":data}),
                            Err(error) => json!({"ok":false,"error":error.to_string()}),
                        };
                        let output = path.with_extension("response");
                        let temp = path.with_extension("tmp");
                        if private_write(&temp, &response.to_string()).is_ok() {
                            let _ = std::fs::rename(temp, output);
                        }
                        let _ = std::fs::remove_file(path);
                    }
                }
                std::thread::park_timeout(Duration::from_millis(10));
            }
            let _ = std::fs::remove_file(connection.directory.join("active"));
        });
        // Stop even if a test runner panics. No query worker can outlive its scope.
        struct Stop<'a>(&'a AtomicBool);
        impl Drop for Stop<'_> {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let _stop = Stop(&stopped);
        let raw = invoke(&prompt)?;
        let reply: Value = serde_json::from_str(raw.trim()).map_err(super::invalid)?;
        bridge.validate(&reply).map_err(|error| match error {
            crate::error::AppError::Context(_) => error,
            other => super::invalid(other),
        })?;
        Ok(raw)
    })
}

fn executable_command(path: &Path) -> String {
    #[cfg(windows)]
    {
        format!("& {}", shell_quote(path))
    }
    #[cfg(not(windows))]
    {
        shell_quote(path)
    }
}
pub(super) fn preview(requirements: &str) -> String {
    format!("{SKILL}\n\n{PROTOCOL}\n\n## 当前提取要求\n\n{requirements}")
}
#[cfg(test)]
pub(super) fn input_available(path: &Path) -> bool {
    let connection = path.parent().unwrap().join("connection.json");
    let name = path.file_name().unwrap().to_string_lossy();
    request(&connection, Command::Inputs).unwrap()["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["name"].as_str() == Some(name.as_ref()))
}
fn shell_quote(path: &Path) -> String {
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
fn request(path: &Path, command: Command) -> Result<Value> {
    let connection: Connection = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    if connection.version != 1 {
        return Err(other("提取连接协议版本不匹配"));
    }
    let active = connection.directory.join("active");
    if std::fs::read_to_string(&active).ok().as_deref() != Some(connection.token.as_str()) {
        return Err(other("提取连接已结束"));
    }
    let name = crate::storage::new_id();
    let input = connection.directory.join(format!("{name}.request"));
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
    let start = std::time::Instant::now();
    let response = loop {
        match std::fs::read_to_string(&output) {
            Ok(text) => {
                let _ = std::fs::remove_file(&output);
                break serde_json::from_str::<Value>(&text)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !active.exists() || start.elapsed() > IO_TIMEOUT {
                    let _ = std::fs::remove_file(&input);
                    return Err(other("提取连接已结束或请求超时"));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    };
    if response["ok"] != true {
        return Err(other(response["error"].as_str().unwrap_or("查询失败")));
    }
    Ok(response["data"].clone())
}

/// Same executable, a small explicit command surface; no app startup or DB open.
pub fn run_cli_if_requested() -> Option<i32> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("extraction") {
        return None;
    }
    let result = cli(&args[1..]);
    match result {
        Ok(data) => {
            println!("{}", json!({"schema_version":1,"ok":true,"data":data}));
            Some(0)
        }
        Err(error) => {
            println!(
                "{}",
                json!({"schema_version":1,"ok":false,"error":{"code":"extraction_request_failed","message":error.to_string()}})
            );
            Some(1)
        }
    }
}
fn cli(args: &[String]) -> Result<Value> {
    if args == ["--help"] {
        return Ok(
            json!({"commands":["inputs","read","search"],"connection":"使用当前提取运行提供的 --connection 路径","writes":"只读查询，不提供 SQL 或业务编辑"}),
        );
    }
    let mut options = HashMap::new();
    let mut action = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if let Some(key) = arg.strip_prefix("--") {
            let value = args.get(index + 1).ok_or_else(|| other("参数缺少值"))?;
            if value.starts_with("--") || options.insert(key, value.as_str()).is_some() {
                return Err(other("参数无效或重复"));
            }
            index += 2;
        } else {
            if action.replace(arg.as_str()).is_some() {
                return Err(other("多余参数"));
            }
            index += 1;
        }
    }
    let connection = options
        .remove("connection")
        .ok_or_else(|| other("缺少 --connection"))?;
    let command = match action {
        Some("inputs") => Command::Inputs,
        Some("read") | Some("search") => {
            let name = options
                .remove("input")
                .ok_or_else(|| other("缺少 --input"))?
                .to_string();
            let cursor = options
                .remove("cursor")
                .unwrap_or("0")
                .parse::<usize>()
                .map_err(|_| other("游标无效"))?;
            let limit = options
                .remove("limit")
                .unwrap_or("8")
                .parse::<usize>()
                .map_err(|_| other("limit 无效"))?;
            if action == Some("search") {
                Command::Search {
                    name,
                    cursor,
                    limit,
                    query: options
                        .remove("query")
                        .ok_or_else(|| other("缺少 --query"))?
                        .into(),
                }
            } else {
                Command::Read {
                    name,
                    cursor,
                    limit,
                }
            }
        }
        _ => return Err(other("支持 inputs、read、search")),
    };
    if !options.is_empty() {
        return Err(other("未知参数"));
    }
    request(Path::new(connection), command)
}

#[cfg(test)]
pub(super) fn read_input(path: &Path) -> Vec<Value> {
    let connection = path.parent().unwrap().join("connection.json");
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    let mut rows = Vec::new();
    let mut cursor = 0;
    loop {
        let page = request(
            &connection,
            Command::Read {
                name: name.clone(),
                cursor,
                limit: 50,
            },
        )
        .unwrap();
        rows.extend(
            page["records"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["record"].clone()),
        );
        match page["next_cursor"].as_u64() {
            Some(next) => cursor = next as usize,
            None => break,
        }
    }
    rows
}
#[cfg(test)]
pub(super) fn read_all_for_test(dir: &Path) {
    let inputs = request(&dir.join("connection.json"), Command::Inputs).unwrap();
    for item in inputs["inputs"].as_array().unwrap() {
        read_input(&dir.join(item["name"].as_str().unwrap()));
    }
}
#[cfg(test)]
mod tests;
