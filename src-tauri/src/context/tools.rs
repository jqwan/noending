//! Run-scoped CLI bridge. The app owns query scopes and commits; the helper never
//! opens SQLite. Connections disappear with the extraction, including on error.
use super::summary::{self, BatchReply, ExtractionFiles, SessionReply};
use crate::error::{other, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

const SKILL: &str = include_str!("../../skills/noending-extraction/SKILL.md");
const PROTOCOL: &str = include_str!("../../skills/noending-extraction/references/protocol.md");

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

/// The Agent runs on the caller thread. Only scoped requests touch the bridge;
/// Filesystem IPC works inside Agent workspace sandboxes without network access.
/// All durable writes remain in the existing snapshot-guarded transaction.
pub(super) fn run(
    db: &crate::storage::Db,
    files: &ExtractionFiles,
    requirements: &str,
    invoke: impl FnOnce(&str) -> Result<String>,
) -> Result<String> {
    let connection =
        crate::cli::transport::Connection::create(files.dir.join("requests")).map_err(other)?;
    let connection_path = files.dir.join("connection.json");
    connection.publish(&connection_path).map_err(other)?;
    let skill_dir = files.dir.join("noending-extraction");
    std::fs::create_dir_all(skill_dir.join("references"))?;
    crate::cli::transport::private_write(&skill_dir.join("SKILL.md"), SKILL).map_err(other)?;
    crate::cli::transport::private_write(&skill_dir.join("references/protocol.md"), PROTOCOL)
        .map_err(other)?;
    crate::cli::transport::private_write(
        &skill_dir.join("references/requirements.md"),
        requirements,
    )
    .map_err(other)?;
    let exe = crate::cli::executable()?;
    let prompt=format!("使用提取 skill：{}。先完整读取该 SKILL.md，再按其中的流程查询材料并生成结果。\nCLI 前缀：{} --connection {} extraction\n提取要求：{}\n最终回复必须是协议规定的单个状态 JSON 对象，不带 Markdown 代码块或其他说明。后端在调用正常结束后校验结果文件并入库。",serde_json::to_string(&skill_dir.join("SKILL.md"))?,executable_command(&exe),shell_quote(&connection_path),serde_json::to_string(&skill_dir.join("references/requirements.md"))?);
    let bridge = Bridge {
        db,
        files,
        token: connection.token.clone(),
        delivered: Mutex::default(),
    };
    let stopped = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !stopped.load(Ordering::Acquire) {
                crate::cli::transport::serve_pending(&connection, |command| {
                    let command = serde_json::from_value(command)
                        .map_err(|e| crate::cli::CliError::new("invalid_command", e))?;
                    bridge
                        .dispatch(Request {
                            token: connection.token.clone(),
                            command,
                        })
                        .map_err(Into::into)
                });
                std::thread::park_timeout(Duration::from_millis(10));
            }
            connection.deactivate();
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
    crate::cli::shell_quote(path)
}
#[cfg(test)]
fn request(path: &Path, command: Command) -> Result<Value> {
    crate::cli::transport::request(path, serde_json::to_value(command)?).map_err(other)
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
