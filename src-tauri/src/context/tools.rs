//! Run-scoped CLI bridge. The extraction Agent submits through the same scope
//! as an autonomous session. A process exit can only report what was saved.
use super::{
    interactive::Scope,
    prompts::PromptTemplates,
    summary::{RunDirectory, Submission, TIME_RULES},
};
use crate::error::{other, Result};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::Value;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

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
    Submit {
        submission: Submission,
    },
}

pub(super) fn run(
    db: &crate::storage::Db,
    extraction: &mut Scope,
    home: &crate::workspace::home::NoEndingHome,
    root: &Path,
    activity: &mut crate::operations::Operation,
    invoke: impl FnOnce(&str, &Path) -> Result<()>,
) -> Result<()> {
    let files = RunDirectory::create(root, extraction.kind())?;
    let dir = &files.0;
    let connection =
        crate::cli::transport::Connection::create(dir.join("requests")).map_err(other)?;
    let connection_path = dir.join("connection.json");
    connection.publish(&connection_path).map_err(other)?;
    let skill_dir = dir.join("noending");
    std::fs::create_dir_all(skill_dir.join("references"))?;
    for (relative, content) in crate::skills::BUNDLED_FILES {
        crate::cli::transport::private_write(&skill_dir.join(relative), content).map_err(other)?;
    }
    let templates = PromptTemplates::default();
    let requirements = match extraction.kind() {
        "session" => templates.session,
        "task" => templates.task,
        _ => format!(
            "项目材料包括目录关联会话（含归档会话）及任务背景。\n## 会话摘要\n{}\n## 项目摘要\n{}",
            templates.session, templates.project
        ),
    };
    let requirements_path = skill_dir.join("references/requirements.md");
    crate::cli::transport::private_write(
        &requirements_path,
        &format!("{TIME_RULES}\n{requirements}"),
    )
    .map_err(other)?;
    let exe = crate::cli::executable()?;
    #[cfg(windows)]
    let executable = format!("& {}", crate::cli::shell_quote(&exe));
    #[cfg(not(windows))]
    let executable = crate::cli::shell_quote(&exe);
    let prompt = format!(
        "使用 NoEnding 技能执行后台提取。先读取 {}，再按技能查询材料并主动调用 submit 保存。\nCLI 前缀：{executable} --connection {} extraction\n提取要求：{}\n按提交回执报告保存结果。",
        serde_json::to_string(&skill_dir.join("SKILL.md"))?,
        crate::cli::shell_quote(&connection_path),
        serde_json::to_string(&requirements_path)?,
    );
    let stopped = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let stop_flag = &stopped;
        scope.spawn(move || {
            while !stop_flag.load(Ordering::Acquire) {
                crate::cli::transport::serve_pending(&connection, |command| {
                    let command = serde_json::from_value(command)
                        .map_err(|e| crate::cli::CliError::new("invalid_command", e))?;
                    let result = match command {
                        Command::Inputs => extraction.describe(db),
                        Command::Read {
                            name,
                            cursor,
                            limit,
                        } => extraction.read(db, &name, cursor, limit, None),
                        Command::Search {
                            name,
                            query,
                            cursor,
                            limit,
                        } => extraction.read(db, &name, cursor, limit, Some(&query)),
                        Command::Submit { submission } => {
                            let result = extraction.submit(db, home, submission);
                            if result.is_ok() {
                                if let Some(progress) = extraction.project_summary_progress() {
                                    // The checkpoint has already committed. A status file
                                    // failure must not change the submission receipt.
                                    let _ = activity.set_project_summary_progress(progress);
                                }
                            }
                            result
                        }
                    };
                    result.map_err(Into::into)
                });
                std::thread::park_timeout(Duration::from_millis(10));
            }
            connection.deactivate();
        });
        struct Stop<'a>(&'a AtomicBool);
        impl Drop for Stop<'_> {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let _stop = Stop(&stopped);
        invoke(&prompt, dir)
    })
}

#[cfg(test)]
pub(super) fn request(path: &Path, command: Value) -> crate::cli::CliResult<Value> {
    crate::cli::transport::request(&path.join("connection.json"), command)
}

#[cfg(test)]
pub(super) fn read_input(path: &Path) -> Vec<Value> {
    let name = path.file_name().unwrap().to_string_lossy();
    let dir = path.parent().unwrap();
    let mut rows = Vec::new();
    let mut cursor = 0;
    loop {
        let page = request(
            dir,
            serde_json::json!({"action":"read","name":name,"cursor":cursor,"limit":50}),
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
            None => return rows,
        }
    }
}

#[cfg(test)]
pub(super) fn input_available(path: &Path) -> bool {
    let name = path.file_name().unwrap().to_string_lossy();
    request(
        path.parent().unwrap(),
        serde_json::json!({"action":"inputs"}),
    )
    .unwrap()["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|input| input["name"].as_str() == Some(name.as_ref()))
}

#[cfg(test)]
mod tests;
