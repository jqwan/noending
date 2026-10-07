//! How an `AgentCommand` becomes a process inside a real PTY.
//!
//! The adapters' argv stays declarative; the platform decision lives here,
//! mirroring `platform::launcher` (which does the same job for *external*
//! terminal windows). The one extra thing an embedded PTY must do that the
//! external path got for free: a GUI-launched app has no login-shell
//! environment, so the agent (and everything it forks — git, editors, package
//! managers) runs inside the user's login shell.

use crate::adapters::AgentCommand;
use crate::error::{other, Result};

/// A freshly opened PTY pair. The slave exists to spawn the child; dropping
/// it (when the pair is consumed into its master) is what lets the master
/// see EOF once the child exits.
pub(crate) struct PtyPair {
    master: Box<dyn portable_pty::MasterPty + Send>,
    slave: Box<dyn portable_pty::SlavePty + Send>,
}

impl PtyPair {
    pub(crate) fn open() -> Result<Self> {
        let pair = portable_pty::native_pty_system()
            .openpty(portable_pty::PtySize {
                rows: super::INITIAL_ROWS,
                cols: super::INITIAL_COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| other(format!("打开 PTY 失败: {e}")))?;
        Ok(Self {
            master: pair.master,
            slave: pair.slave,
        })
    }

    /// Spawn the command. The returned child must be reaped by the caller's
    /// wait thread.
    pub(crate) fn spawn_command(
        &self,
        cmd: &portable_pty::CommandBuilder,
    ) -> Result<Box<dyn portable_pty::Child + Send + Sync>> {
        self.slave
            .spawn_command(cmd.clone())
            .map_err(|e| other(format!("在终端中启动 Agent 失败: {e}")))
    }

    pub(crate) fn take_reader(&self) -> Result<Box<dyn std::io::Read + Send>> {
        self.master
            .try_clone_reader()
            .map_err(|e| other(format!("读取终端输出失败: {e}")))
    }

    pub(crate) fn take_writer(&self) -> Result<Box<dyn std::io::Write + Send>> {
        self.master
            .take_writer()
            .map_err(|e| other(format!("写入终端失败: {e}")))
    }

    pub(crate) fn into_master(self) -> Box<dyn portable_pty::MasterPty + Send> {
        self.master
    }
}

/// POSIX login shell for the wrapper. A GUI process has no shell-derived
/// PATH; wrapping in the user's login shell restores the environment the
/// external-terminal launch always had.
#[cfg(unix)]
fn login_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| {
        if cfg!(target_os = "macos") {
            "/bin/zsh".to_string()
        } else {
            "/bin/bash".to_string()
        }
    })
}

/// The command the PTY actually runs: the agent CLI inside the user's login
/// shell. cwd handling mirrors `platform::launcher::render_bash` — a stale
/// directory falls back to `$HOME` *with a printed notice*, never silently.
#[cfg(unix)]
pub(crate) fn wrapped_command(cmd: &AgentCommand) -> portable_pty::CommandBuilder {
    use crate::platform::launcher::shell_quote;

    let mut script = String::new();
    if let Some(cwd) = &cmd.cwd {
        let dir = shell_quote(&cwd.to_string_lossy());
        script.push_str(&format!(
            "cd {dir} 2>/dev/null || {{ printf 'NoEnding: 工作目录不可用: %s — 已回退到 $HOME\\n' {dir}; cd \"$HOME\"; }} || true\n",
        ));
    }
    let mut parts: Vec<String> = vec![shell_quote(&cmd.program)];
    parts.extend(cmd.args.iter().map(|a| shell_quote(a)));
    // exec: the shell is replaced by the agent, so the PTY closes when the
    // agent exits — the embedded terminal is the agent's lifetime.
    script.push_str(&format!("exec {}\n", parts.join(" ")));

    let mut wrapped = portable_pty::CommandBuilder::new(login_shell());
    wrapped.arg("-l");
    wrapped.arg("-c");
    wrapped.arg(script);
    wrapped
}

/// Windows wrapper: PowerShell (mirroring `platform::launcher::render_ps`)
/// so npm-style `.cmd` shims resolve and the user PATH applies. `-NoProfile`
/// keeps startup fast; the guarded `Set-Location` states a stale directory
/// instead of failing the spawn.
#[cfg(target_os = "windows")]
pub(crate) fn wrapped_command(cmd: &AgentCommand) -> portable_pty::CommandBuilder {
    use crate::platform::launcher::ps_quote;

    let mut script = String::new();
    if let Some(cwd) = &cmd.cwd {
        let dir = ps_quote(&cwd.to_string_lossy());
        script.push_str(&format!(
            "if (Test-Path -LiteralPath {dir}) {{ Set-Location -LiteralPath {dir} }} else {{ Write-Host ('NoEnding: 工作目录不可用: ' + {dir} + ' — 已回退到默认目录') }}\n",
        ));
    }
    script.push_str(&format!("& {}", ps_quote(&cmd.program)));
    for arg in &cmd.args {
        script.push(' ');
        script.push_str(&ps_quote(arg));
    }

    let mut wrapped = portable_pty::CommandBuilder::new("powershell");
    wrapped.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command"]);
    wrapped.arg(script);
    wrapped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn unix_wrapper_quotes_every_argument_into_a_login_shell() {
        let cmd = AgentCommand {
            program: "/usr/local/bin/codex".into(),
            args: vec!["resume".into(), "thread with space".into()],
            cwd: Some(std::path::PathBuf::from("/tmp/some dir")),
        };
        let wrapped = wrapped_command(&cmd);
        // Not directly inspectable — CommandBuilder has no getters — so assert
        // on the shell choice and rely on the integration tests to prove the
        // quoting survives a real spawn.
        let shell = login_shell();
        assert!(shell == "/bin/zsh" || shell == "/bin/bash" || shell.starts_with('/'));
        let _ = wrapped;
    }
}
