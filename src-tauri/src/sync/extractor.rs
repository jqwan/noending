//! Agent CLI invocation for explicit summary extraction. File and reply validation
//! live in context::summary; snapshot validation and commits live in context.
use crate::adapters::ExecOptions;
use crate::domain::Agent;
use crate::error::{other, Result};
use crate::storage::Db;

/// Assistant configuration, persisted in settings.
///
/// ONLY the agent choice: which CLI the explicit Context update runs on. Model /
/// provider / effort are owned by Settings → Agents like every other runtime.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AssistantConfig {
    pub agent: String, // codex | claude_code | pi | none
}

impl Default for AssistantConfig {
    fn default() -> Self {
        Self {
            agent: "codex".into(),
        }
    }
}

impl AssistantConfig {
    pub fn from_settings(db: &Db) -> AssistantConfig {
        AssistantConfig {
            agent: db
                .get_setting("assistant.agent")
                .ok()
                .flatten()
                .unwrap_or_else(|| AssistantConfig::default().agent),
        }
    }

    pub fn save(&self, db: &Db) -> Result<()> {
        db.set_setting("assistant.agent", &self.agent)
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextCallFailure {
    pub kind: ContextCallFailureKind,
    pub spawned: bool,
    pub exit_code: Option<i32>,
    pub io_error_kind: Option<&'static str>,
}

impl CliExtractor {
    /// Strict lookup for the interactive path: `Ok(None)` means the user set
    /// Assistant to `none`; anything else that cannot be honoured is an `Err`.
    /// A missing or invalid CLI is surfaced, never degraded to a heuristic.
    pub fn try_from_settings(db: &Db) -> Result<Option<CliExtractor>> {
        Self::try_for_agent(db, &AssistantConfig::from_settings(db).agent)
    }

    pub fn try_for_agent(db: &Db, agent: &str) -> Result<Option<CliExtractor>> {
        if agent == "none" {
            return Ok(None);
        }
        let agent = Agent::parse(agent).ok_or_else(|| other(format!("未知的 Agent: {agent}")))?;
        Ok(Some(CliExtractor::new(
            agent,
            crate::agent_runtime::runtime_exec_options(db, agent)?,
        )))
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
        if !matches!(self.agent, Agent::Codex | Agent::ClaudeCode | Agent::Pi) {
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
