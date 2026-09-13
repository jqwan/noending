//! Agent Adapter layer.
//!
//! Each adapter owns: session directory layout, file format, session id,
//! optional environment info, resume parameters, new-session invocation.
//! Upper layers must never reference `~/.codex` / `~/.claude` / `~/.pi`
//! directly — that is PlatformPaths' job.

pub mod claude;
pub mod codex;
pub mod pi;

use std::path::PathBuf;

use crate::domain::{Agent, Session};
use crate::error::Result;
use crate::platform::exec_resolver::AgentInstallation;

/// Declarative command produced by an adapter; execution is delegated to
/// the PlatformLauncher so OS differences stay out of adapters.
#[derive(Debug, Clone)]
pub struct AgentCommand {
    pub program: String,
    pub args: Vec<String>,
    /// Context text to hand to the agent as initial prompt / follow-up.
    pub prompt: Option<String>,
    pub cwd: Option<PathBuf>,
}

/// One discovered external session before ingestion.
#[derive(Debug, Clone)]
pub struct DiscoveredSession {
    pub agent: Agent,
    pub agent_session_id: String,
    pub path: PathBuf,
    pub cwd: Option<String>,
    pub started_at: Option<String>,
    pub last_activity_at: Option<String>,
    pub first_user_text: Option<String>,
    pub parent_agent_session_id: Option<String>,
}

/// Incremental read handle: adapter returns only events after a sequence.
pub struct ReadDelta {
    pub events: Vec<crate::domain::SessionEvent>,
    pub last_sequence: i64,
    pub file_size: i64,
}

/// Options for a headless ("exec"/print-mode) invocation of the agent CLI.
/// The Workspace Assistant uses these to run intelligence through the
/// user's already-authenticated agent CLIs.
#[derive(Debug, Clone, Default)]
pub struct ExecOptions {
    pub model: Option<String>,
    pub provider: Option<String>, // pi: --provider (e.g. openai-codex, lmstudio)
    pub effort: Option<String>,   // codex model_reasoning_effort / pi --thinking
}

pub trait AgentAdapter: Send + Sync {
    fn agent(&self) -> Agent;

    /// CLI detect() via ExecutableResolver + data-dir presence.
    fn detect(&self) -> Option<AgentInstallation>;

    /// Where sessions live, for diagnostics.
    fn session_root(&self) -> Option<PathBuf>;

    fn discover_sessions(&self) -> Result<Vec<DiscoveredSession>>;

    /// Read events with sequence > `from`, appending normalized events.
    /// Must detect append / truncate / rewrite via file size heuristics.
    fn read_delta(&self, session: &Session, from_sequence: i64) -> Result<ReadDelta>;

    /// Build the command line for a New Session. `context_file` is None when
    /// the user chose to start without any Workstream Context — adapters then
    /// launch the plain CLI with no injected prompt.
    fn build_new_command(
        &self,
        install: &AgentInstallation,
        context_file: Option<&std::path::Path>,
        cwd: Option<&std::path::Path>,
    ) -> Result<AgentCommand>;

    fn build_resume_command(
        &self,
        install: &AgentInstallation,
        agent_session_id: &str,
        context_file: Option<&std::path::Path>,
        cwd: Option<&std::path::Path>,
    ) -> Result<AgentCommand>;

    /// Non-interactive one-shot run (codex exec / claude -p / pi -p).
    /// The returned command is executed by platform::exec_runner.
    fn build_exec_command(
        &self,
        install: &AgentInstallation,
        opts: &ExecOptions,
        prompt: &str,
    ) -> Result<AgentCommand>;
}

/// Shared helper: `$(cat file)` keeps shell quoting safe for large bundles.
/// Returns None (and no prompt) when there is no context file.
pub fn prompt_from_context_file(context_file: Option<&std::path::Path>) -> Option<String> {
    context_file.map(|p| format!("\"$(cat '{}')\"", p.to_string_lossy()))
}

pub fn all_adapters() -> Vec<Box<dyn AgentAdapter>> {
    vec![
        Box::new(codex::CodexAdapter),
        Box::new(claude::ClaudeAdapter),
        Box::new(pi::PiAdapter),
    ]
}

pub fn adapter_for(agent: Agent) -> &'static dyn AgentAdapter {
    use std::sync::OnceLock;
    static REGISTRY: OnceLock<Vec<Box<dyn AgentAdapter>>> = OnceLock::new();
    let reg = REGISTRY.get_or_init(all_adapters);
    for a in reg {
        if a.agent() == agent {
            // Safe: registry outlives 'static and elements are never removed.
            return unsafe { &*(a.as_ref() as *const dyn AgentAdapter) };
        }
    }
    unreachable!("adapter for {:?} missing", agent)
}

/// Shared JSONL helpers -------------------------------------------------

pub fn read_jsonl_lines(path: &std::path::Path) -> Result<Vec<(usize, String)>> {
    let data = std::fs::read(path)?;
    let text = String::from_utf8_lossy(&data);
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if !line.trim().is_empty() {
            out.push((i, line.to_string()));
        }
    }
    Ok(out)
}

pub fn truncate_text(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{}…", t)
    }
}

/// Session display title from the first meaningful user text.
pub fn title_from_text(text: &str) -> Option<String> {
    let t = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if t.is_empty() {
        None
    } else {
        Some(truncate_text(&t, 80))
    }
}
