//! Agent CLI invocation for explicit summary extraction. File and reply validation
//! live in context::summary; snapshot validation and commits live in context.
use crate::adapters::ExecOptions;
use crate::domain::Agent;
use crate::error::{other, Result};
use crate::storage::Db;

pub fn supports_extraction(agent: Agent) -> bool {
    matches!(agent, Agent::Codex | Agent::ClaudeCode | Agent::Pi)
}

/// Dedicated runtime for all summary extraction.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExtractionSettings {
    pub agent: String,
    pub runtime: crate::agent_runtime::AgentRuntimeOverrides,
}

impl Default for ExtractionSettings {
    fn default() -> Self {
        Self {
            agent: "codex".into(),
            runtime: Default::default(),
        }
    }
}

impl ExtractionSettings {
    pub fn from_settings(db: &Db) -> Result<Self> {
        match db.get_setting("context.extraction")? {
            Some(raw) => {
                serde_json::from_str(&raw).map_err(|e| other(format!("摘要提取配置已损坏：{e}")))
            }
            None => Ok(Self::default()),
        }
    }

    pub fn exec_options(&self) -> Result<Option<(Agent, ExecOptions)>> {
        if self.agent == "none" {
            return Ok(None);
        }
        let agent = Agent::parse(&self.agent)
            .ok_or_else(|| other(format!("未知的 Agent: {}", self.agent)))?;
        if !supports_extraction(agent) {
            return Err(other("摘要提取仅支持 Codex、Claude Code 和 Pi"));
        }
        crate::agent_runtime::validate_runtime_overrides(agent, &self.runtime)?;
        Ok(Some((agent, self.runtime.exec_options())))
    }

    pub fn save(&self, db: &Db) -> Result<Self> {
        self.exec_options()?;
        let value = Self {
            agent: self.agent.clone(),
            runtime: if self.agent == "none" {
                Default::default()
            } else {
                self.runtime.normalized()
            },
        };
        db.set_setting(
            "context.extraction",
            &serde_json::to_string(&value).map_err(|e| other(e.to_string()))?,
        )?;
        Ok(value)
    }
}

/// Runs the explicit Context update through the user's agent CLI (headless).
pub struct CliExtractor {
    pub agent: Agent,
    pub opts: ExecOptions,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextCallFailureKind {
    AgentUnavailable,
    UnsupportedAgent,
    CommandInvalid,
    Start,
    Wait,
    Exit,
    Timeout,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextCallFailure {
    pub kind: ContextCallFailureKind,
    pub spawned: bool,
    pub exit_code: Option<i32>,
    pub io_error_kind: Option<&'static str>,
}

impl CliExtractor {
    /// A missing CLI or invalid dedicated configuration remains a failure.
    pub fn try_from_settings(db: &Db) -> Result<Option<CliExtractor>> {
        Ok(ExtractionSettings::from_settings(db)?
            .exec_options()?
            .map(|(agent, opts)| Self::new(agent, opts)))
    }

    pub fn new(agent: Agent, opts: ExecOptions) -> CliExtractor {
        CliExtractor {
            agent,
            opts,
            timeout_secs: 120,
        }
    }

    /// Runtime label recorded on written revisions.
    pub fn name(&self) -> String {
        let agent = self.agent.as_str();
        if self.opts.is_default() {
            format!("cli:{agent}:agent-default")
        } else {
            format!("cli:{agent}:override:{}", self.opts.override_summary())
        }
    }

    /// Run ONE headless Agent invocation and return cleaned stdout. Any failure
    /// (CLI missing, timeout, non-zero exit) is returned as Err — the caller
    /// must NOT fall back to a heuristic.
    pub fn run(&self, prompt: &str) -> Result<String> {
        let install = crate::platform::exec_resolver::resolve(self.agent)?;
        let adapter = crate::adapters::adapter_for(self.agent);
        let cmd = adapter.build_exec_command(&install, &self.opts, prompt)?;
        let out = crate::platform::exec_runner::run_headless(&cmd, self.timeout_secs)?;
        Ok(crate::platform::exec_runner::clean_exec_stdout(&out.stdout))
    }

    /// Run the extraction-only policy in an explicit app-owned working
    /// directory. Unlike `run`, this never calls the generic Assistant exec
    /// builder and never permits an inherited cwd.
    pub fn run_context(
        &self,
        prompt: &str,
        runtime_dir: &std::path::Path,
    ) -> std::result::Result<String, ContextCallFailure> {
        if !supports_extraction(self.agent) {
            return Err(ContextCallFailure {
                kind: ContextCallFailureKind::UnsupportedAgent,
                spawned: false,
                exit_code: None,
                io_error_kind: None,
            });
        }
        let install = crate::platform::exec_resolver::resolve(self.agent).map_err(|_| {
            ContextCallFailure {
                kind: ContextCallFailureKind::AgentUnavailable,
                spawned: false,
                exit_code: None,
                io_error_kind: None,
            }
        })?;
        let adapter = crate::adapters::adapter_for(self.agent);
        let cmd = adapter
            .build_context_extraction_command(&install, &self.opts, prompt, runtime_dir)
            .map_err(|_| ContextCallFailure {
                kind: ContextCallFailureKind::UnsupportedAgent,
                spawned: false,
                exit_code: None,
                io_error_kind: None,
            })?;
        if cmd.cwd.as_deref() != Some(runtime_dir) {
            return Err(ContextCallFailure {
                kind: ContextCallFailureKind::CommandInvalid,
                spawned: false,
                exit_code: None,
                io_error_kind: None,
            });
        }
        let out = crate::platform::exec_runner::run_context_extraction(&cmd, self.timeout_secs)
            .map_err(|failure| ContextCallFailure {
                kind: match failure.kind {
                    crate::platform::exec_runner::ContextExecFailureKind::Start => {
                        ContextCallFailureKind::Start
                    }
                    crate::platform::exec_runner::ContextExecFailureKind::Cancelled => {
                        ContextCallFailureKind::Cancelled
                    }
                    crate::platform::exec_runner::ContextExecFailureKind::Wait => {
                        ContextCallFailureKind::Wait
                    }
                    crate::platform::exec_runner::ContextExecFailureKind::Exit => {
                        ContextCallFailureKind::Exit
                    }
                    crate::platform::exec_runner::ContextExecFailureKind::Timeout => {
                        ContextCallFailureKind::Timeout
                    }
                },
                spawned: failure.spawned,
                exit_code: failure.exit_code,
                io_error_kind: failure.io_error_kind,
            })?;
        Ok(crate::platform::exec_runner::clean_exec_stdout(&out.stdout))
    }
}
