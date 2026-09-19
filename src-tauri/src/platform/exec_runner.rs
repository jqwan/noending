//! Headless child-process execution.
//!
//! Two layers live here:
//! * [`run`] / [`run_with_env`] — the generic mechanism: structured program +
//!   literal args + an explicit `cwd` + a hard timeout. No shell, no invented
//!   working directory.
//! * [`run_headless`] — the Agent-CLI policy on top of it (a bad exit becomes an
//!   error the UI can show).
//!
//! The Workspace Assistant runs its intelligence through the very same
//! agent CLIs the user already has authenticated — `codex exec`,
//! `claude -p`, `pi -p --no-session` — instead of requiring separate
//! LLM API keys. This module runs those commands as child processes
//! with a hard timeout and returns stdout.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::adapters::AgentCommand;
use crate::error::{other, Result};

pub const DEFAULT_EXEC_TIMEOUT_SECS: u64 = 120;

pub struct HeadlessOutput {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
    /// `None` when the child never reported an exit status, i.e. it was killed
    /// at the deadline. Callers that must distinguish "failed" from "hung"
    /// (the WorkspaceResolver collapses both into `GitDetection::Unavailable`)
    /// need this, so it is part of the payload rather than an error variant.
    pub exit_code: Option<i32>,
}

fn drain(mut reader: impl Read, buf: &mut String) {
    let mut tmp = String::new();
    let _ = reader.read_to_string(&mut tmp);
    buf.push_str(&tmp);
}

/// Run a program headlessly with a hard timeout.
///
/// The mechanism behind [`run_headless`], extracted so it is not locked to the
/// adapter namespace (方案 §42.3-M11): the WorkspaceResolver has to run `git`
/// in a *specific* directory, and the old `run_headless` fell back to a
/// hardcoded `/tmp` when `cwd` was `None` — wrong on Windows, and wrong for any
/// caller that cares which directory it is asking about.
///
/// Contract differences from `run_headless`, deliberate:
/// * `cwd: None` means "inherit this process's directory" — no invented path.
/// * A non-zero exit and a timeout are both `Ok(HeadlessOutput)`: an exit code
///   is an *answer*. Only a child we could not start or wait for is `Err`, so
///   callers that must degrade gracefully (Git detection) never see a user
///   visible error for a path that simply is not a repository.
/// * Stdout/stderr are drained on background threads so large outputs cannot
///   deadlock the pipe; the child is killed when the deadline passes.
pub fn run(
    program: &Path,
    args: &[&str],
    cwd: Option<&Path>,
    timeout_secs: u64,
) -> Result<HeadlessOutput> {
    run_with_env(program, args, cwd, timeout_secs, &[])
}

/// [`run`] plus per-child environment.
///
/// Git detection needs `GIT_OPTIONAL_LOCKS=0` / `GIT_TERMINAL_PROMPT=0`
/// (方案 §42.3-M9); keeping them out of `run`'s signature leaves the Agent
/// path (which must inherit the user's environment verbatim) untouched.
pub fn run_with_env(
    program: &Path,
    args: &[&str],
    cwd: Option<&Path>,
    timeout_secs: u64,
    env: &[(&str, &str)],
) -> Result<HeadlessOutput> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    for (key, value) in env {
        command.env(key, value);
    }

    let mut child: Child = command
        .spawn()
        .map_err(|e| other(format!("启动 {} 失败: {}", program.display(), e)))?;

    let mut stdout_pipe = child.stdout.take().ok_or_else(|| other("no stdout"))?;
    let mut stderr_pipe = child.stderr.take().ok_or_else(|| other("no stderr"))?;
    let out_handle = std::thread::spawn(move || {
        let mut s = String::new();
        drain(&mut stdout_pipe, &mut s);
        s
    });
    let err_handle = std::thread::spawn(move || {
        let mut s = String::new();
        drain(&mut stderr_pipe, &mut s);
        s
    });

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(e) => return Err(other(format!("等待进程失败: {}", e))),
        }
    };

    let stdout = out_handle.join().unwrap_or_default();
    let stderr = err_handle.join().unwrap_or_default();

    Ok(match status {
        Some(st) => HeadlessOutput {
            stdout,
            stderr,
            success: st.success(),
            exit_code: Some(st.code().unwrap_or(-1)),
        },
        None => HeadlessOutput {
            stdout,
            stderr,
            success: false,
            exit_code: None,
        },
    })
}

/// Run an AgentCommand headlessly, turning a bad exit into an error.
///
/// Agent CLIs are a user-visible action, so "claude exited 1" must reach the
/// UI; the generic [`run`] deliberately does not decide that.
pub fn run_headless(cmd: &AgentCommand, timeout_secs: u64) -> Result<HeadlessOutput> {
    let args: Vec<&str> = cmd.args.iter().map(String::as_str).collect();
    let out = run(
        Path::new(&cmd.program),
        &args,
        cmd.cwd.as_deref(),
        timeout_secs,
    )?;
    match out.exit_code {
        Some(_) if out.success => Ok(out),
        Some(code) => Err(other(format!(
            "agent CLI 退出码 {}：{}",
            code,
            last_lines(&out.stderr, 3)
        ))),
        None => Err(other(format!(
            "agent CLI 超时（{}s）：{}",
            timeout_secs,
            last_lines(&out.stderr, 3)
        ))),
    }
}

fn last_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join(" | ")
}

/// Strip headless-CLI noise from stdout: codex prints a `tokens used`
/// footer, pi/claude may pad with blank lines.
pub fn clean_exec_stdout(raw: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    for line in raw.lines() {
        let t = line.trim();
        if t == "tokens used" || t.starts_with("--------") {
            break; // codex footer begins here
        }
        lines.push(line);
    }
    let joined = lines.join("\n");
    joined.trim().to_string()
}
