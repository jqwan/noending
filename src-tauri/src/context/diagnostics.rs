//! Privacy-safe operation logging for explicit Context extraction.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use chrono::{DateTime, Days, Utc};
use serde::Serialize;

use crate::adapters::ExecOptions;
use crate::domain::{Agent, ContextUpdateError};
use crate::error::{AppError, ContextUpdateFailure};
use crate::workspace::home::NoEndingHome;

const LOG_SUBDIR: &str = "context-extraction";
static LOG_WRITE_LOCK: Mutex<()> = Mutex::new(());

pub fn runtime_dir(home: &NoEndingHome) -> PathBuf {
    home.runtime_dir.join(LOG_SUBDIR)
}

pub fn prepare_runtime_dir(home: &NoEndingHome) -> std::io::Result<PathBuf> {
    let path = runtime_dir(home);
    fs::create_dir_all(&path)?;
    Ok(path)
}

pub fn log_dir(home: &NoEndingHome) -> PathBuf {
    home.logs_dir.join(LOG_SUBDIR)
}

pub struct ContextOperation {
    pub operation_id: String,
    target_type: &'static str,
    target_id: String,
    home: Option<NoEndingHome>,
    started_at: DateTime<Utc>,
    started: Instant,
    stage: &'static str,
    agent: Option<String>,
    model_override: Option<String>,
    cli_invoked: bool,
    cli_exit_code: Option<i32>,
    cli_start_error: Option<&'static str>,
    failure: Option<(&'static str, &'static str)>,
}

impl ContextOperation {
    pub fn new(target_type: &'static str, target_id: &str, home: Option<&NoEndingHome>) -> Self {
        Self {
            operation_id: uuid::Uuid::new_v4().to_string(),
            target_type,
            target_id: target_id.to_string(),
            home: home.cloned(),
            started_at: Utc::now(),
            started: Instant::now(),
            stage: "snapshot",
            agent: None,
            model_override: None,
            cli_invoked: false,
            cli_exit_code: None,
            cli_start_error: None,
            failure: None,
        }
    }

    pub fn set_stage(&mut self, stage: &'static str) {
        self.stage = stage;
    }

    pub fn home(&self) -> Option<&NoEndingHome> {
        self.home.as_ref()
    }

    pub fn record_config(
        &mut self,
        raw_agent: &str,
        parsed_agent: Option<Agent>,
        opts: Option<&ExecOptions>,
    ) {
        self.agent = Some(match parsed_agent {
            Some(agent) => agent.as_str().to_string(),
            None if raw_agent == "none" => "none".into(),
            None => "invalid".into(),
        });
        if let Some(opts) = opts {
            self.model_override = Some(opts.override_summary());
        }
    }

    pub fn record_invocation_config(&mut self, agent: Agent, opts: &ExecOptions) {
        self.agent = Some(agent.as_str().to_string());
        self.model_override = Some(opts.override_summary());
    }

    pub fn set_cli_result(
        &mut self,
        spawned: bool,
        exit_code: Option<i32>,
        io_error_kind: Option<&'static str>,
    ) {
        self.cli_invoked = spawned;
        self.cli_exit_code = exit_code;
        self.cli_start_error = io_error_kind;
    }

    pub fn succeed(&mut self, outcome: &'static str) {
        self.stage = outcome;
        self.write_record(outcome, None, None);
    }

    pub fn fail(&mut self, error: &AppError) -> ContextUpdateFailure {
        let (code, message) = self
            .failure
            .unwrap_or_else(|| safe_error(error, self.stage));
        self.write_record("failed", Some(code), Some(message));
        ContextUpdateFailure {
            code: code.to_string(),
            message: message.to_string(),
            operation_id: self.operation_id.clone(),
        }
    }

    pub fn fail_with(&mut self, stage: &'static str, code: &'static str, message: &'static str) {
        self.stage = stage;
        self.failure = Some((code, message));
    }

    fn write_record(&self, outcome: &str, error_code: Option<&str>, message: Option<&str>) {
        let Some(home) = &self.home else {
            return;
        };
        let dir = log_dir(home);
        let date = self.started_at.format("%Y-%m-%d").to_string();
        let record = ContextLogRecord {
            operation_id: &self.operation_id,
            operation_type: self.target_type,
            target_id: &self.target_id,
            started_at: self.started_at.to_rfc3339(),
            finished_at: Utc::now().to_rfc3339(),
            duration_ms: self.started.elapsed().as_millis().min(u64::MAX as u128) as u64,
            agent: self.agent.as_deref(),
            model_override: self.model_override.as_deref(),
            cli_invoked: self.cli_invoked,
            cli_exit_code: self.cli_exit_code,
            cli_start_error: self.cli_start_error,
            stage: self.stage,
            outcome,
            error_code,
            message,
        };
        let _ = append_record(&dir, &date, &record);
    }
}

#[derive(Serialize)]
struct ContextLogRecord<'a> {
    operation_id: &'a str,
    operation_type: &'a str,
    target_id: &'a str,
    started_at: String,
    finished_at: String,
    duration_ms: u64,
    agent: Option<&'a str>,
    model_override: Option<&'a str>,
    cli_invoked: bool,
    cli_exit_code: Option<i32>,
    cli_start_error: Option<&'static str>,
    stage: &'a str,
    outcome: &'a str,
    error_code: Option<&'a str>,
    message: Option<&'a str>,
}

fn append_record(dir: &Path, date: &str, record: &impl Serialize) -> std::io::Result<()> {
    let _guard = LOG_WRITE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    fs::create_dir_all(dir)?;
    prune_old_logs(dir, Utc::now().date_naive());
    let path = dir.join(format!("{date}.jsonl"));
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let mut line = serde_json::to_vec(record)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    line.push(b'\n');
    file.write_all(&line)
}

fn prune_old_logs(dir: &Path, today: chrono::NaiveDate) {
    let Some(cutoff) = today.checked_sub_days(Days::new(14)) else {
        return;
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(date) = name.strip_suffix(".jsonl") else {
            continue;
        };
        let Ok(date) = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d") else {
            continue;
        };
        if date < cutoff {
            let _ = fs::remove_file(path);
        }
    }
}

fn safe_error(error: &AppError, stage: &str) -> (&'static str, &'static str) {
    if let AppError::Context(context) = error {
        return match context {
            ContextUpdateError::AiUnavailable(_) => (
                "ai_unavailable",
                "未配置可用的 Context Agent，请检查 Assistant Agent 设置。",
            ),
            ContextUpdateError::ModelCallFailed(_) => (
                "model_call_failed",
                "Agent 模型调用失败，请检查 CLI 状态后重试。",
            ),
            ContextUpdateError::InvalidOutput(_) => (
                "invalid_output",
                "Agent 返回内容不符合 Context 格式，请重试。",
            ),
            ContextUpdateError::ConcurrencyConflict(_) => {
                ("stale_snapshot", "内容已变化，请重新更新。")
            }
            ContextUpdateError::InputTooLarge(_) => {
                ("input_too_large", "待处理内容超过单次更新上限。")
            }
            ContextUpdateError::Storage(_) => ("storage_failed", "保存 Context 失败，请重试。"),
        };
    }
    match error {
        AppError::Io(_) => ("io_failed", "准备 Context 更新时发生文件错误，请重试。"),
        AppError::Db(_) | AppError::Json(_) => {
            ("storage_failed", "读取或保存 Context 失败，请重试。")
        }
        AppError::Other(_) if stage == "database_commit" => {
            ("storage_failed", "保存 Context 失败，请重试。")
        }
        AppError::Other(_) if stage == "snapshot" => {
            ("target_unavailable", "找不到更新目标或无法读取其当前状态。")
        }
        _ => ("update_failed", "Context 更新失败，请重试。"),
    }
}

pub fn cli_failure(
    kind: crate::sync::extractor::ContextCallFailureKind,
) -> (&'static str, &'static str) {
    use crate::sync::extractor::ContextCallFailureKind as K;
    match kind {
        K::AgentUnavailable => (
            "cli_unavailable",
            "找不到所选 Agent CLI，请检查 Agent 是否已安装。",
        ),
        K::UnsupportedAgent => (
            "agent_unsupported",
            "Context 提取支持 Codex、Claude Code 和 Pi，请更改 Assistant Agent。",
        ),
        K::CommandInvalid => (
            "runtime_dir_unavailable",
            "无法使用 Context 专用运行目录，请检查 NoEnding Home。",
        ),
        K::Start => ("cli_start_failed", "Agent CLI 无法启动，请检查安装和权限。"),
        K::Wait => ("cli_wait_failed", "等待 Agent CLI 完成时发生错误，请重试。"),
        K::Exit => ("cli_failed", "Agent CLI 执行失败，请检查 CLI 状态后重试。"),
        K::Timeout => ("cli_timeout", "Agent CLI 超时，请重试或检查 CLI 状态。"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::home::NoEndingHome;

    fn home(root: &Path) -> NoEndingHome {
        NoEndingHome::new(root.to_str().unwrap(), None).unwrap()
    }

    #[test]
    fn custom_home_controls_context_runtime_and_logs() {
        let base = std::env::temp_dir().join(format!("context-home-{}", uuid::Uuid::new_v4()));
        let home = home(&base.join("custom"));
        let runtime = prepare_runtime_dir(&home).unwrap();
        assert_eq!(
            home.root.file_name().and_then(|name| name.to_str()),
            Some("custom")
        );
        assert_eq!(runtime, home.runtime_dir.join(LOG_SUBDIR));
        assert!(runtime.is_dir());
        assert_eq!(log_dir(&home), home.logs_dir.join(LOG_SUBDIR));
    }

    #[test]
    fn logging_failure_is_best_effort_and_does_not_fail_operation() {
        let base = std::env::temp_dir().join(format!("context-log-{}", uuid::Uuid::new_v4()));
        let home = home(&base);
        fs::create_dir_all(&home.logs_dir).unwrap();
        fs::write(&home.logs_dir.join(LOG_SUBDIR), "not a directory").unwrap();
        let mut op = ContextOperation::new("session", "session-test", Some(&home));
        op.succeed("updated");
        assert_eq!(op.operation_id.len(), 36);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn error_log_has_stage_and_operation_metadata_without_prompt_or_cli_output() {
        let base = std::env::temp_dir().join(format!("context-safe-log-{}", uuid::Uuid::new_v4()));
        let home = home(&base);
        let mut op = ContextOperation::new("session", "session-safe-test", Some(&home));
        op.agent = Some("codex".into());
        op.model_override = Some("agent-default".into());
        op.set_cli_result(true, Some(2), None);
        op.set_stage("output_validation");
        op.fail_with(
            "output_validation",
            "invalid_output",
            "Agent 返回内容不符合 Context 格式，请重试。",
        );
        let failure = op.fail(&crate::error::other(
            "prompt: secret; output: secret; stderr: secret",
        ));
        assert_eq!(failure.operation_id, op.operation_id);
        let file = log_dir(&home).join(format!("{}.jsonl", op.started_at.format("%Y-%m-%d")));
        let line = fs::read_to_string(file).unwrap();
        assert!(line.contains("\"operation_type\":\"session\""));
        assert!(line.contains("\"target_id\":\"session-safe-test\""));
        assert!(line.contains("\"stage\":\"output_validation\""));
        assert!(line.contains("\"cli_invoked\":true"));
        assert!(line.contains("\"cli_exit_code\":2"));
        assert!(line.contains("\"error_code\":\"invalid_output\""));
        assert!(!line.contains("secret"));
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn database_commit_failure_is_logged_as_a_controlled_storage_error() {
        let base = std::env::temp_dir().join(format!("context-db-error-{}", uuid::Uuid::new_v4()));
        let home = home(&base);
        let mut op = ContextOperation::new("workstream", "workstream-safe-test", Some(&home));
        op.set_stage("database_commit");
        let failure = op.fail(&crate::error::other("private database value"));
        assert_eq!(failure.code, "storage_failed");
        assert_eq!(failure.operation_id, op.operation_id);
        let file = log_dir(&home).join(format!("{}.jsonl", op.started_at.format("%Y-%m-%d")));
        let line = fs::read_to_string(file).unwrap();
        assert!(line.contains("\"stage\":\"database_commit\""));
        assert!(line.contains("\"error_code\":\"storage_failed\""));
        assert!(!line.contains("private database value"));
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn daily_log_prunes_only_own_files_older_than_fourteen_days() {
        let base = std::env::temp_dir().join(format!("context-prune-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&base).unwrap();
        fs::write(base.join("2020-01-01.jsonl"), "old").unwrap();
        fs::write(base.join("keep.txt"), "other").unwrap();
        let today = chrono::NaiveDate::from_ymd_opt(2026, 9, 26).unwrap();
        prune_old_logs(&base, today);
        assert!(!base.join("2020-01-01.jsonl").exists());
        assert!(base.join("keep.txt").exists());
        let _ = fs::remove_dir_all(base);
    }
}
