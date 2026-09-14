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

/// Render argv into a bash script (macOS terminal launch). Pure string
/// building, testable on every platform.
pub fn render_bash(cmd: &AgentCommand) -> String {
    let mut script = String::from("#!/bin/bash\n");
    if let Some(cwd) = &cmd.cwd {
        // A stale recorded cwd must not abort the launch (exit 1 would leave
        // the user with a dead window and no agent); fall back to $HOME.
        script.push_str(&format!(
            "cd {} 2>/dev/null || cd \"$HOME\" || true\n",
            shell_quote(&cwd.to_string_lossy())
        ));
    }
    let mut parts: Vec<String> = vec![shell_quote(&cmd.program)];
    parts.extend(cmd.args.iter().map(|a| shell_quote(a)));
    script.push_str(&parts.join(" "));
    script.push_str("\nexec $SHELL\n");
    script
}

/// Render argv into a PowerShell script (Windows terminal launch). Pure
/// string building, testable on every platform.
///
/// PowerShell parses per LINE: `& 'prog'` alone is a complete command and
/// the following quoted strings would be inert expressions, not arguments.
/// Collect argv into an array and splat it on one line. The array items are
/// comma-joined WITHOUT a trailing comma — PowerShell (unlike Rust/C) has
/// no trailing-comma tolerance in array literals: `@('a','b',)` is
/// `MissingExpressionAfterToken`.
pub fn render_ps(cmd: &AgentCommand) -> String {
    let mut script = String::new();
    if let Some(cwd) = &cmd.cwd {
        // stale cwd must not abort the launch — continue in the default dir
        script.push_str(&format!(
            "Set-Location -LiteralPath {} -ErrorAction SilentlyContinue\n",
            ps_quote(&cwd.to_string_lossy())
        ));
    }
    if !cmd.args.is_empty() {
        let items: Vec<String> = cmd
            .args
            .iter()
            .map(|a| format!("  {}", ps_quote(a)))
            .collect();
        script.push_str("$argv = @(\n");
        script.push_str(&items.join(",\n"));
        script.push_str("\n)\n");
    }
    script.push_str(&format!("& {}", ps_quote(&cmd.program)));
    if !cmd.args.is_empty() {
        script.push_str(" @argv");
    }
    script.push('\n');
    script
}

fn script_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("noending-launch");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn script_name(kind: &str) -> String {
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S%3f");
    format!(
        "launch-{}-{}.{}",
        kind,
        ts,
        if cfg!(windows) { "ps1" } else { "sh" }
    )
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

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
            .current_dir(
                cmd.cwd
                    .as_deref()
                    .unwrap_or(std::env::current_dir()?.as_path()),
            )
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

#[allow(unused_imports)]
pub use imp::launch as launch_command;
pub use imp::{launch, open_uri};

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
        assert_eq!(
            ps_quote("C:\\Program Files\\x.exe"),
            "'C:\\Program Files\\x.exe'"
        );
        assert_eq!(ps_quote("it''s"), "'it''''s'");
        assert_eq!(ps_quote("$var & `b"), "'$var & `b'");
        assert_eq!(ps_quote("a\nb"), "'a\nb'");
    }

    // Pure renderers are exercised on every platform; only the launch
    // round-trip needs the real PowerShell (windows-only, see below).
    #[test]
    fn bash_render_quotes_argv() {
        let cmd = AgentCommand {
            program: "/usr/local/bin/My Agent".into(),
            args: vec![
                "resume".into(),
                "sess-1".into(),
                "line1\nline2 'q' $X".into(),
            ],
            cwd: Some("/tmp/some dir".into()),
        };
        let s = render_bash(&cmd);
        assert!(s.contains("cd '/tmp/some dir' 2>/dev/null || cd \"$HOME\" || true"));
        assert!(
            s.contains("'/usr/local/bin/My Agent' 'resume' 'sess-1' 'line1\nline2 '\\''q'\\'' $X'")
        );
    }

    #[test]
    fn ps_render_quotes_argv_without_trailing_comma() {
        let cmd = AgentCommand {
            program: "C:\\Program Files\\claude.exe".into(),
            args: vec!["--resume".into(), "s1".into(), "a `b $c & d".into()],
            cwd: Some("C:\\Users\\me docs".into()),
        };
        let s = render_ps(&cmd);
        assert!(s.contains("Set-Location -LiteralPath 'C:\\Users\\me docs'"));
        assert!(s.contains("& 'C:\\Program Files\\claude.exe' @argv"));
        // exact array shape: comma between items, none after the last —
        // PowerShell rejects a trailing comma ("Missing expression after ',")
        assert!(s.contains("$argv = @(\n  '--resume',\n  's1',\n  'a `b $c & d'\n)\n"));
        assert!(!s.contains(",\n)"));
    }

    /// True argv round-trip: the rendered script must pass EVERY argument
    /// to the program, in order. Without the $argv splat, PowerShell parses
    /// `& 'prog'\n'arg1'\n'arg2'` as one command plus inert string
    /// expressions — the program receives NO arguments.
    #[cfg(target_os = "windows")]
    #[test]
    fn ps_rendered_script_delivers_argv_round_trip() {
        let dir = std::env::temp_dir().join(format!("noending-ps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script_path = dir.join("argv-roundtrip.ps1");
        let cmd = AgentCommand {
            program: "cmd.exe".into(),
            args: vec![
                "/c".into(),
                "echo".into(),
                "NOENDING-ARG-1".into(),
                "arg with spaces".into(),
                "NOENDING-ARG-3".into(),
            ],
            cwd: None,
        };
        std::fs::write(&script_path, render_ps(&cmd)).unwrap();

        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&script_path)
            .output()
            .expect("run powershell");
        assert!(out.status.success(), "script failed: {:?}", out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("NOENDING-ARG-1"), "argv lost: {}", stdout);
        assert!(
            stdout.contains("arg with spaces"),
            "quoted argv lost: {}",
            stdout
        );
        assert!(
            stdout.contains("NOENDING-ARG-3"),
            "argv truncated: {}",
            stdout
        );
        let _ = std::fs::remove_file(&script_path);
        let _ = std::fs::remove_dir(&dir);
    }
}
