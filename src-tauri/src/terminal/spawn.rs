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
        #[cfg(windows)]
        crate::platform::conpty::ensure_loaded()?;
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
        self.slave.spawn_command(cmd.clone()).map_err(|e| {
            other(format!(
                "在终端中启动 Agent 失败: {}",
                spawn_error_detail(&e.to_string())
            ))
        })
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

/// portable-pty's Windows errors can embed the full command line, including
/// the user's first message inside our wrapper script. Keep only the actual
/// failure reason from those known formats; ordinary OS errors remain useful.
fn spawn_error_detail(error: &str) -> &str {
    if error.starts_with("CreateProcessW `") {
        // The script itself can contain `failed:` (or this whole delimiter),
        // so only the final boundary introduces the real OS error.
        return error
            .rsplit_once("` failed: ")
            .map(|(_, reason)| reason)
            .filter(|reason| !reason.trim().is_empty())
            .unwrap_or("CreateProcessW 失败");
    }
    if error.starts_with("invalid encoding for command line argument ") {
        return "命令参数包含无效编码";
    }
    error
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

    let mut script = String::from(
        "$OutputEncoding = [Console]::InputEncoding = [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)\n",
    );
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

    let shell = std::env::var_os("SystemRoot")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"))
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let mut wrapped = portable_pty::CommandBuilder::new(shell);
    wrapped.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command"]);
    wrapped.arg(script);
    wrapped
}

/// portable-pty 0.9 kills the PowerShell wrapper alone on Windows. Its Agent
/// and descendants must share the existing close/exit lifetime contract.
#[cfg(windows)]
pub(crate) fn process_tree_killer(
    child: &(dyn portable_pty::Child + Send + Sync),
) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
    #[derive(Debug)]
    struct TreeKiller {
        pid: u32,
        fallback: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    }
    impl portable_pty::ChildKiller for TreeKiller {
        fn kill(&mut self) -> std::io::Result<()> {
            use std::os::windows::process::CommandExt;
            let taskkill = std::env::var_os("SystemRoot")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"))
                .join("System32/taskkill.exe");
            let status = std::process::Command::new(taskkill)
                .args(["/PID", &self.pid.to_string(), "/T", "/F"])
                .creation_flags(0x08000000) // CREATE_NO_WINDOW
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            if status.is_ok_and(|status| status.success()) {
                Ok(())
            } else {
                self.fallback.kill()
            }
        }

        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(Self {
                pid: self.pid,
                fallback: self.fallback.clone_killer(),
            })
        }
    }
    match child.process_id() {
        Some(pid) => Box::new(TreeKiller {
            pid,
            fallback: child.clone_killer(),
        }),
        None => child.clone_killer(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_error_detail_redacts_windows_command_lines_and_keeps_the_os_reason() {
        let reason = "The system cannot find the file specified. (os error 2)";
        let error = format!(
            "CreateProcessW `\"powershell -Command & 'codex' '--' '用户首条消息 failed: fake ` failed: still user text'\"` in cwd `Some(\"C:\\\\work\")` failed: {reason}"
        );
        assert_eq!(spawn_error_detail(&error), reason);
        assert_eq!(
            spawn_error_detail("CreateProcessW `用户首条消息` unexpected failure"),
            "CreateProcessW 失败"
        );
        assert_eq!(
            spawn_error_detail("CreateProcessW `用户首条消息` in cwd `None` failed: "),
            "CreateProcessW 失败"
        );
    }

    #[test]
    fn spawn_error_detail_redacts_invalid_arguments_and_preserves_other_failures() {
        assert_eq!(
            spawn_error_detail(
                "invalid encoding for command line argument \"用户首条消息\\0script\""
            ),
            "命令参数包含无效编码"
        );
        for error in [
            "Permission denied (os error 13)",
            "No such file or directory (os error 2)",
            "failed to resolve home dir",
        ] {
            assert_eq!(spawn_error_detail(error), error);
        }
    }

    #[test]
    #[cfg(windows)]
    fn windows_wrapper_preserves_unicode_arguments_cwd_and_utf8_output() {
        use std::os::windows::process::CommandExt;
        let cwd =
            std::env::temp_dir().join(format!("noending quote's {}", crate::storage::new_id()));
        std::fs::create_dir_all(&cwd).unwrap();
        let cmd = AgentCommand {
            program: "Write-Output".into(),
            args: vec!["中文，你好 😀 '$literal'".into()],
            cwd: Some(cwd.clone()),
        };
        let wrapped = wrapped_command(&cmd);
        let argv = wrapped.get_argv();
        let output = std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .creation_flags(0x08000000)
            .output()
            .unwrap();
        let cwd_cmd = AgentCommand {
            program: "powershell".into(),
            args: vec![
                "-NoProfile".into(),
                "-Command".into(),
                "[Console]::Write((Get-Location).Path)".into(),
            ],
            cwd: Some(cwd.clone()),
        };
        let wrapped_cwd = wrapped_command(&cwd_cmd);
        let cwd_argv = wrapped_cwd.get_argv();
        let cwd_output = std::process::Command::new(&cwd_argv[0])
            .args(&cwd_argv[1..])
            .creation_flags(0x08000000)
            .output()
            .unwrap();
        // PowerShell expands 8.3 paths (RUNNER~1 on hosted Windows). Compare
        // directory identity while the fixture still exists, not path spelling.
        let expected_cwd = std::fs::canonicalize(&cwd).unwrap();
        let actual_cwd =
            std::fs::canonicalize(String::from_utf8(cwd_output.stdout.clone()).unwrap());
        std::fs::remove_dir_all(&cwd).unwrap();
        assert!(output.status.success(), "{:?}", output.stderr);
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim(),
            "中文，你好 😀 '$literal'"
        );
        assert!(cwd_output.status.success(), "{:?}", cwd_output.stderr);
        assert_eq!(actual_cwd.unwrap(), expected_cwd);
    }

    #[test]
    #[cfg(unix)]
    fn unix_wrapper_quotes_every_argument_into_a_login_shell() {
        let cwd =
            std::env::temp_dir().join(format!("noending quote's {}", crate::storage::new_id()));
        std::fs::create_dir_all(&cwd).unwrap();
        let literal_args = [
            "thread with space",
            "it's literal",
            "line one\nline two",
            "$(printf injected) $HOME `printf injected`",
            "中文",
        ];
        let cmd = AgentCommand {
            program: "/bin/sh".into(),
            args: ["-c", r#"printf '%s\0' "$PWD" "$@""#, "--"]
                .into_iter()
                .chain(literal_args)
                .map(str::to_string)
                .collect(),
            cwd: Some(cwd.clone()),
        };
        let wrapped = wrapped_command(&cmd);
        let argv = wrapped.get_argv();
        assert_eq!(argv[0], std::ffi::OsString::from(login_shell()));
        assert_eq!(&argv[1..3], ["-l", "-c"]);
        // Execute the generated script without user-specific login startup files.
        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&argv[3])
            .output()
            .unwrap();
        std::fs::remove_dir_all(&cwd).unwrap();
        assert!(output.status.success(), "{:?}", output.stderr);
        let expected = std::iter::once(cwd.to_string_lossy().as_ref())
            .chain(literal_args)
            .map(|arg| format!("{arg}\0"))
            .collect::<String>();
        assert_eq!(output.stdout, expected.as_bytes());
    }
}
