//! Headless agent CLI execution (print / exec mode).
//!
//! The Workspace Assistant runs its intelligence through the very same
//! agent CLIs the user already has authenticated — `codex exec`,
//! `claude -p`, `pi -p --no-session` — instead of requiring separate
//! LLM API keys. This module runs those commands as child processes
//! with a hard timeout and returns stdout.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::adapters::AgentCommand;
use crate::error::{other, Result};

pub const DEFAULT_EXEC_TIMEOUT_SECS: u64 = 120;

pub struct HeadlessOutput {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

fn drain(mut reader: impl Read, buf: &mut String) {
    let mut tmp = String::new();
    let _ = reader.read_to_string(&mut tmp);
    buf.push_str(&tmp);
}

/// Run an AgentCommand headlessly. Stdout/stderr are drained on background
/// threads so large outputs cannot deadlock the pipe; the child is killed
/// when the deadline passes.
pub fn run_headless(cmd: &AgentCommand, timeout_secs: u64) -> Result<HeadlessOutput> {
    let mut child: Child = Command::new(&cmd.program)
        .args(&cmd.args)
        .current_dir(
            cmd.cwd
                .as_deref()
                .unwrap_or_else(|| std::path::Path::new("/tmp")),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| other(format!("启动 {} 失败: {}", cmd.program, e)))?;

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

    match status {
        Some(st) if st.success() => Ok(HeadlessOutput {
            stdout,
            stderr,
            success: true,
        }),
        Some(st) => Err(other(format!(
            "agent CLI 退出码 {}：{}",
            st.code().unwrap_or(-1),
            last_lines(&stderr, 3)
        ))),
        None => Err(other(format!(
            "agent CLI 超时（{}s）：{}",
            timeout_secs,
            last_lines(&stderr, 3)
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
