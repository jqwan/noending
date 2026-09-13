//! PlatformLauncher: how an `AgentCommand` actually reaches the user.
//!
//! Agent Adapters only produce a declarative command (program, args, cwd,
//! context file). Whether that becomes a macOS Terminal tab, a Windows
//! Terminal window, or a direct child process is decided here, per platform.

use serde::Serialize;

use crate::adapters::AgentCommand;
use crate::error::{other, Result};

#[derive(Debug, Clone, Serialize)]
pub struct LaunchOutcome {
    pub launched_via: String,
    pub command_line: String,
    pub pid: Option<u32>,
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    fn shell_quote(s: &str) -> String {
        format!("'{}'", s.replace('\'', "'\\''"))
    }

    pub fn launch(cmd: &AgentCommand) -> Result<LaunchOutcome> {
        let mut parts: Vec<String> = vec![shell_quote(&cmd.program)];
        for a in &cmd.args {
            parts.push(shell_quote(a));
        }
        let mut script = String::new();
        if let Some(cwd) = &cmd.cwd {
            script.push_str(&format!("cd {} && ", shell_quote(&cwd.to_string_lossy())));
        }
        script.push_str(&parts.join(" "));
        script.push_str("; exec $SHELL");

        // AppleScript string escaping
        let escaped = script.replace('\\', "\\\\").replace('"', "\\\"");
        let osascript = format!(
            "tell application \"Terminal\"\nactivate\ndo script \"{}\"\nend tell",
            escaped
        );

        let child = std::process::Command::new("osascript")
            .arg("-e")
            .arg(&osascript)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        let pid = child.id();
        let out = child.wait_with_output()?;
        if !out.status.success() {
            return Err(other(format!(
                "osascript 启动 Terminal 失败: {}",
                String::from_utf8_lossy(&out.stderr)
            )));
        }
        Ok(LaunchOutcome {
            launched_via: "macOS Terminal".into(),
            command_line: script,
            pid: Some(pid),
        })
    }

    pub fn open_uri(uri: &str) -> Result<()> {
        std::process::Command::new("open")
            .arg(uri)
            .status()
            .map_err(|e| other(format!("open {} 失败: {}", uri, e)))?;
        Ok(())
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use super::*;

    fn ps_quote(s: &str) -> String {
        // single-quoted PowerShell string literal
        format!("'{}'", s.replace('\'', "''"))
    }

    pub fn launch(cmd: &AgentCommand) -> Result<LaunchOutcome> {
        let mut parts: Vec<String> = vec![&cmd.program];
        parts.extend(cmd.args.iter().map(|a| a.to_string()));
        let mut inner = String::new();
        if let Some(cwd) = &cmd.cwd {
            inner.push_str(&format!("Set-Location {}; ", ps_quote(&cwd.to_string_lossy())));
        }
        inner.push_str(&parts.join(" "));

        // Prefer Windows Terminal when present; fall back to PowerShell.
        let has_wt = std::process::Command::new("where")
            .arg("wt.exe")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        let (via, script) = if has_wt {
            (
                "Windows Terminal",
                format!(
                    "wt.exe -d {} powershell -NoExit -Command {}",
                    cmd.cwd
                        .as_ref()
                        .map(|c| ps_quote(&c.to_string_lossy()))
                        .unwrap_or_else(|| "'.'".into()),
                    ps_quote(&inner)
                ),
            )
        } else {
            (
                "PowerShell",
                format!("Start-Process powershell -ArgumentList '-NoExit','-Command',{}", ps_quote(&inner)),
            )
        };

        let child = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .stdin(std::process::Stdio::null())
            .spawn()?;
        let pid = child.id();
        Ok(LaunchOutcome {
            launched_via: via.into(),
            command_line: inner,
            pid: Some(pid),
        })
    }

    pub fn open_uri(uri: &str) -> Result<()> {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", uri])
            .status()
            .map_err(|e| other(format!("start {} 失败: {}", uri, e)))?;
        Ok(())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod imp {
    use super::*;

    pub fn launch(cmd: &AgentCommand) -> Result<LaunchOutcome> {
        let mut child = std::process::Command::new(&cmd.program)
            .args(&cmd.args)
            .current_dir(cmd.cwd.as_deref().unwrap_or(std::env::current_dir()?.as_path()))
            .spawn()?;
        let pid = child.id();
        Ok(LaunchOutcome {
            launched_via: "direct process (linux)".into(),
            command_line: cmd.program.clone(),
            pid: Some(pid),
        })
    }

    pub fn open_uri(uri: &str) -> Result<()> {
        std::process::Command::new("xdg-open")
            .arg(uri)
            .status()
            .map_err(|e| other(format!("xdg-open {} 失败: {}", uri, e)))?;
        Ok(())
    }
}

pub use imp::{launch, open_uri};
#[allow(unused_imports)]
pub use imp::launch as launch_command;
