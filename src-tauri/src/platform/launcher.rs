//! PlatformLauncher: how an `AgentCommand` actually reaches the user.
//!
//! Agent Adapters only produce a declarative command (program, args, cwd)
//! with literal strings; whether that becomes a macOS Terminal tab, a
//! Windows PowerShell window, or a direct child process is decided here,
//! per platform.
//!
//! Implementation strategy: each platform renders the argv into a temporary
//! script (bash / PowerShell) with strict argument quoting, then opens that
//! script in the user's terminal. This keeps every character of every
//! argument intact — including quotes, `$`, `&`, backticks, newlines and
//! unicode — without adapters ever generating shell expressions.

use serde::Serialize;

use crate::adapters::AgentCommand;
use crate::error::{other, Result};

#[derive(Debug, Clone, Serialize)]
pub struct LaunchOutcome {
    pub launched_via: String,
    pub command_line: String,
    pub pid: Option<u32>,
}

/// POSIX shell quoting: wrap in single quotes, escape embedded quotes.
/// Inside single quotes every other character (space, `$`, backtick, `&`,
/// newline, unicode) is literal.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// PowerShell single-quoted string literal: embedded quotes become `''`;
/// every other character (`$`, backtick, `&`, `;`, newline, unicode) is
/// literal inside single quotes.
pub fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn script_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("noending-launch");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn script_name(kind: &str) -> String {
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S%3f");
    format!("launch-{}-{}.{}", kind, ts, if cfg!(windows) { "ps1" } else { "sh" })
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    pub fn render_bash(cmd: &AgentCommand) -> String {
        let mut script = String::from("#!/bin/bash\n");
        if let Some(cwd) = &cmd.cwd {
            script.push_str(&format!("cd {} || exit 1\n", shell_quote(&cwd.to_string_lossy())));
        }
        let mut parts: Vec<String> = vec![shell_quote(&cmd.program)];
        parts.extend(cmd.args.iter().map(|a| shell_quote(a)));
        script.push_str(&parts.join(" "));
        script.push_str("\nexec $SHELL\n");
        script
    }

    pub fn launch(cmd: &AgentCommand) -> Result<LaunchOutcome> {
        let path = script_dir().join(script_name("macos"));
        std::fs::write(&path, render_bash(cmd))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700));
        }

        // Terminal executes the script file; no command text is embedded in
        // the AppleScript beyond the (space-free-safe) quoted script path.
        let script_path = shell_quote(&path.to_string_lossy());
        let osascript = format!(
            "tell application \"Terminal\"\nactivate\ndo script {}\nend tell",
            format!("\"{}\"", script_path.replace('"', "\\\""))
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
            command_line: cmd.display(),
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

    pub fn render_ps(cmd: &AgentCommand) -> String {
        let mut script = String::new();
        if let Some(cwd) = &cmd.cwd {
            script.push_str(&format!(
                "Set-Location -LiteralPath {}\n",
                ps_quote(&cwd.to_string_lossy())
            ));
        }
        script.push_str(&format!("& {}\n", ps_quote(&cmd.program)));
        for a in &cmd.args {
            script.push_str(&format!("  {}\n", ps_quote(a)));
        }
        script
    }

    pub fn launch(cmd: &AgentCommand) -> Result<LaunchOutcome> {
        let path = script_dir().join(script_name("windows"));
        std::fs::write(&path, render_ps(cmd))?;

        // -File keeps the script path out of any command-line string
        // interpretation; the script itself carries strictly quoted argv.
        let script_arg = ps_quote(&path.to_string_lossy());
        let script = format!(
            "Start-Process powershell -ArgumentList '-NoProfile','-NoExit','-ExecutionPolicy','Bypass','-File',{}",
            script_arg
        );

        let child = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .stdin(std::process::Stdio::null())
            .spawn()?;
        let pid = child.id();
        Ok(LaunchOutcome {
            launched_via: "PowerShell".into(),
            command_line: cmd.display(),
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
            command_line: cmd.display(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_keeps_every_character_literal() {
        assert_eq!(shell_quote("plain"), "'plain'");
        assert_eq!(shell_quote("path with spaces"), "'path with spaces'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote("$(rm -rf /)"), "'$(rm -rf /)'");
        assert_eq!(shell_quote("a\nb"), "'a\nb'");
        assert_eq!(shell_quote("中文✅"), "'中文✅'");
    }

    #[test]
    fn ps_quote_keeps_every_character_literal() {
        assert_eq!(ps_quote("plain"), "'plain'");
        assert_eq!(ps_quote("C:\\Program Files\\x.exe"), "'C:\\Program Files\\x.exe'");
        assert_eq!(ps_quote("it''s"), "'it''''s'");
        assert_eq!(ps_quote("$var & `b"), "'$var & `b'");
        assert_eq!(ps_quote("a\nb"), "'a\nb'");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn bash_render_quotes_argv() {
        use super::imp::render_bash;
        let cmd = AgentCommand {
            program: "/usr/local/bin/My Agent".into(),
            args: vec!["resume".into(), "sess-1".into(), "line1\nline2 'q' $X".into()],
            cwd: Some("/tmp/some dir".into()),
        };
        let s = render_bash(&cmd);
        assert!(s.contains("cd '/tmp/some dir' || exit 1"));
        assert!(s.contains("'/usr/local/bin/My Agent' 'resume' 'sess-1' 'line1\nline2 '\\''q'\\'' $X'"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn ps_render_quotes_argv() {
        use super::imp::render_ps;
        let cmd = AgentCommand {
            program: "C:\\Program Files\\claude.exe".into(),
            args: vec!["--resume".into(), "s1".into(), "a `b $c & d".into()],
            cwd: Some("C:\\Users\\me docs".into()),
        };
        let s = render_ps(&cmd);
        assert!(s.contains("Set-Location -LiteralPath 'C:\\Users\\me docs'"));
        assert!(s.contains("& 'C:\\Program Files\\claude.exe'"));
        assert!(s.contains("'a `b $c & d'"));
    }
}
