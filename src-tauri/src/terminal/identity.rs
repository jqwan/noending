//! Per-terminal native-session notifications from Agent lifecycle hooks.
//!
//! Nothing is inferred from terminal output or message text. Each embedded
//! launch gets its own authenticated loopback listener and temporary, explicit
//! CLI resources. Antigravity uses a dedicated global plugin, gated by
//! per-launch environment; existing user configuration is never replaced.

mod antigravity;
mod claude;

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::activity::{ActivitySignal, ActivityState};
use crate::adapters::AgentCommand;
use crate::domain::Agent;
use crate::error::{other, Result};

const HELPER_FLAG: &str = "--noending-identity-hook";
// The bridge carries only identity/activity metadata. Native hook input also
// contains entire assistant replies or tool results and needs a separate bound.
const MAX_EVENT_BYTES: u64 = 128 * 1024;
const MAX_HOOK_INPUT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PENDING_EVENTS: usize = 128;
const IO_TIMEOUT: Duration = Duration::from_millis(400);
// Windows PowerShell 5.1 can inherit a legacy stdin code page. Codex/Claude
// send UTF-8 JSON: decoding it first as an OEM code page can even consume JSON
// backslashes following Chinese text, invalidating an otherwise valid Stop.
#[cfg(any(windows, test))]
const POWERSHELL_HOOK_INPUT: &str = "$neHookInput = [System.IO.StreamReader]::new(\
     [Console]::OpenStandardInput(), [System.Text.UTF8Encoding]::new($false)).ReadToEnd();";
#[cfg(any(windows, test))]
const WINDOWS_HOOK_ENV: &str = "NOENDING_TERMINAL_HOOK";
const ACK_TIMEOUT: Duration = Duration::from_secs(2);
const CLAUDE_RECORD_SETTLE_STEPS: usize = 16;
const CLAUDE_RECORD_SETTLE_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeIdentityEvent {
    pub native_id: String,
    pub cwd: Option<String>,
    pub source_path: Option<String>,
    pub source: Option<String>,
    #[serde(default)]
    pub activity: Option<ActivitySignal>,
    #[serde(default)]
    pub input: Option<NativeInput>,
}

/// A native input identity schedules history ingestion; no message content crosses the bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeInput {
    pub id: Option<String>,
}

pub type IdentityCallback = Arc<dyn Fn(NativeIdentityEvent) + Send + Sync>;

#[derive(Serialize, Deserialize)]
struct Envelope {
    token: String,
    terminal_id: String,
    event: NativeIdentityEvent,
    #[serde(default)]
    active_record: Option<PathBuf>,
}

#[derive(Default)]
struct BridgeState {
    stopped: bool,
    pending: VecDeque<PendingEvent>,
    callback: Option<IdentityCallback>,
}

struct PendingEvent {
    event: NativeIdentityEvent,
    acknowledgement: TcpStream,
    active_record: Option<PathBuf>,
}

/// Keep this alive with the terminal record. The listener buffers events until
/// `activate`, because an Agent can call its startup hook before registration.
pub struct IdentityBridge {
    state: Arc<Mutex<BridgeState>>,
    resource_dir: PathBuf,
}

impl IdentityBridge {
    pub fn activate(&self, callback: IdentityCallback) {
        if let Ok(mut state) = self.state.lock() {
            state.callback = Some(callback);
        }
    }
}

impl Drop for IdentityBridge {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            state.stopped = true;
            state.pending.clear();
            state.callback = None;
        }
        // A callback can remove its own terminal. Never join its thread here.
        let _ = std::fs::remove_dir_all(&self.resource_dir);
    }
}

pub struct PreparedIdentityBridge {
    pub command: AgentCommand,
    pub bridge: IdentityBridge,
    pub environment: Vec<(String, String)>,
}

/// Add lifecycle observation to supported CLIs without changing their prompt
/// argument or replacing the user's hooks/extensions.
pub fn prepare(
    command: &AgentCommand,
    agent: Agent,
    terminal_id: &str,
) -> Result<Option<PreparedIdentityBridge>> {
    let root = (agent == Agent::Antigravity)
        .then(|| {
            crate::platform::paths::resolve_antigravity_cli_dir()
                .ok_or_else(|| other("无法定位 Antigravity CLI 配置目录"))
        })
        .transpose()?;
    prepare_at(command, agent, terminal_id, root.as_deref())
}

fn prepare_at(
    command: &AgentCommand,
    agent: Agent,
    terminal_id: &str,
    antigravity_root: Option<&Path>,
) -> Result<Option<PreparedIdentityBridge>> {
    if !matches!(
        agent,
        Agent::ClaudeCode | Agent::Codex | Agent::Pi | Agent::Antigravity
    ) {
        return Ok(None);
    }
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    let token = crate::storage::new_id();
    let resource_dir = std::env::temp_dir().join(format!("noending-identity-{token}"));
    create_private_dir(&resource_dir)?;
    let state = Arc::new(Mutex::new(BridgeState::default()));
    let bridge = IdentityBridge {
        state: state.clone(),
        resource_dir,
    };

    let mut environment = Vec::new();
    let args = match agent {
        Agent::Antigravity => {
            let root = antigravity_root.ok_or_else(|| other("Antigravity CLI root missing"))?;
            antigravity::install_plugin(root, &std::env::current_exe()?)?;
            environment.push(antigravity::environment(
                address,
                &token,
                terminal_id,
                root,
            )?);
            Vec::new()
        }
        Agent::ClaudeCode => {
            let plugin = bridge.resource_dir.join("claude-plugin");
            create_private_dir(&plugin)?;
            create_private_dir(&plugin.join(".claude-plugin"))?;
            create_private_dir(&plugin.join("hooks"))?;
            write_private(
                &plugin.join(".claude-plugin/plugin.json"),
                &serde_json::to_vec(&serde_json::json!({
                    "name": "noending-terminal-identity",
                    "version": "1.0.0"
                }))?,
            )?;
            // Keep the established shell form: older Claude versions do not
            // understand exec-form `args` and would otherwise launch the GUI
            // executable without the helper flag.
            let hook = serde_json::json!({
                "type": "command",
                "command": hook_command(address, &token, terminal_id, "claude_code", &bridge.resource_dir, &mut environment)?,
                "timeout": 3
            });
            let hooks = serde_json::json!({ "hooks": {
                "SessionStart": [{ "hooks": [hook] }],
                "Stop": [{ "hooks": [hook] }],
                "UserPromptSubmit": [{ "hooks": [hook] }],
                "PermissionRequest": [{ "hooks": [hook] }],
                "PostToolUse": [{ "hooks": [hook] }],
                "PostToolUseFailure": [{ "hooks": [hook] }],
                "StopFailure": [{ "hooks": [hook] }],
                "Notification": [{ "matcher": "idle_prompt", "hooks": [hook] }]
            }});
            write_private(
                &plugin.join("hooks/hooks.json"),
                &serde_json::to_vec(&hooks)?,
            )?;
            vec!["--plugin-dir".into(), plugin.to_string_lossy().into_owned()]
        }
        Agent::Pi => {
            let extension = bridge.resource_dir.join("identity-extension.mjs");
            let script = pi_extension(address, &token, terminal_id)?;
            write_private(&extension, script.as_bytes())?;
            vec![
                "--extension".into(),
                extension.to_string_lossy().into_owned(),
            ]
        }
        Agent::Codex => {
            let hook = hook_command(
                address,
                &token,
                terminal_id,
                "codex",
                &bridge.resource_dir,
                &mut environment,
            )?;
            // Codex 0.160.1 dispatches queued SessionStart hooks when the
            // next user turn begins. Idle TUI switches therefore become
            // known on that next turn; they are never inferred from output.
            // Codex discovers hooks independently in every active config
            // layer. SessionFlags add a source; lower user/project layers keep
            // their hooks even though ordinary config arrays are overridden.
            codex_hook_args(&hook, cfg!(windows))?
        }
        _ => unreachable!("supported agents checked above"),
    };

    let mut observed = command.clone();
    // CLI options precede the adapter's args. The first prompt remains the
    // final argv element, including the adapter's `--` separator.
    observed.args.splice(0..0, args);
    let terminal_id = terminal_id.to_string();
    std::thread::Builder::new()
        .name(format!("terminal-identity-{terminal_id}"))
        .spawn(move || receive_events(listener, state, token, terminal_id, agent))?;
    Ok(Some(PreparedIdentityBridge {
        command: observed,
        bridge,
        environment,
    }))
}

/// PowerShell 5.1's legacy native argument passing removes embedded double
/// quotes, including through npm's .cmd shim. Our Windows hook command uses
/// EncodedCommand, so these controlled strings can use TOML literal quotes.
/// Do not change the decoded values: Codex fingerprints the parsed handlers.
fn codex_cli_toml_string(value: &str, windows: bool) -> Result<String> {
    if windows {
        if value.contains(['\'', '\r', '\n']) {
            return Err(other("Codex hook 配置无法编码为单行 TOML 字符串"));
        }
        Ok(format!("'{value}'"))
    } else {
        Ok(serde_json::to_string(value)?)
    }
}

fn codex_hook_args(hook: &str, windows: bool) -> Result<Vec<String>> {
    let group = format!(
        "[{{hooks=[{{type={},command={},timeout=3,async=false}}]}}]",
        codex_cli_toml_string("command", windows)?,
        codex_cli_toml_string(hook, windows)?
    );
    let mut args = Vec::new();
    for event in CODEX_EVENTS {
        args.extend(["-c".into(), format!("hooks.{event}={group}")]);
    }
    args.extend(["-c".into(), codex_trust_override(hook, windows)?]);
    Ok(args)
}

const CODEX_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PermissionRequest",
    "PostToolUse",
    "Stop",
    "Interrupt",
];

fn codex_trust_override(command: &str, windows: bool) -> Result<String> {
    // These fingerprints are Codex's own versioned hook identity contract
    // (codex-rs/hooks/lib.rs and config/fingerprint.rs, rust-v0.160.1).
    // Trust only the handlers introduced by this launch. Never use the
    // global bypass flag, which would also enable unrelated untrusted hooks.
    let source = if windows {
        r"C:\<session-flags>\config.toml"
    } else {
        "/<session-flags>/config.toml"
    };
    let mut entries = Vec::new();
    for event in [
        "session_start",
        "user_prompt_submit",
        "permission_request",
        "post_tool_use",
        "stop",
        "interrupt",
    ] {
        // Keep keys lexically ordered, including the nested handler object:
        // Codex hashes canonical compact JSON after normalization through TOML.
        let normalized = serde_json::json!({
            "event_name": event,
            "hooks": [{"async":false,"command":command,"timeout":3,"type":"command"}]
        });
        let hash = format!(
            "sha256:{:x}",
            Sha256::digest(serde_json::to_vec(&normalized)?)
        );
        let key = codex_cli_toml_string(&format!("{source}:{event}:0:0"), windows)?;
        entries.push(format!(
            "{key}={{enabled=true,trusted_hash={}}}",
            codex_cli_toml_string(&hash, windows)?
        ));
    }
    // The CLI's dotted override parser does not understand quoted keys that
    // themselves contain dots. Put complete keys inside one inline table.
    if windows {
        // Whitespace makes PowerShell quote the whole native argument. Keep
        // it even though TOML permits a compact form: CMD must not interpret
        // the angle brackets in <session-flags> as input/output redirection.
        Ok(format!("hooks.state={{ {} }}", entries.join(", ")))
    } else {
        Ok(format!("hooks.state={{{}}}", entries.join(",")))
    }
}

fn create_private_dir(path: &Path) -> std::io::Result<()> {
    let builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    let builder = {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = builder;
        builder.mode(0o700);
        builder
    };
    builder.create(path)
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(bytes)
}

fn hook_command(
    address: SocketAddr,
    token: &str,
    terminal_id: &str,
    agent: &str,
    _resource_dir: &Path,
    _environment: &mut Vec<(String, String)>,
) -> Result<String> {
    let executable = std::env::current_exe()?;
    let executable = executable.to_string_lossy();
    #[cfg(unix)]
    {
        let executable = crate::platform::launcher::shell_quote(&executable);
        Ok(format!(
            "{executable} {HELPER_FLAG} {address} {token} {terminal_id} {agent}"
        ))
    }
    #[cfg(windows)]
    {
        let (command, environment) = windows_hook_command(
            &executable,
            address,
            token,
            terminal_id,
            agent,
            _resource_dir,
        )?;
        _environment.push(environment);
        Ok(command)
    }
}

/// Keep the script out of Codex's repeated `-c hooks.*` arguments: the npm
/// .cmd entry point has an 8191-character limit. Only a constant bootstrap is
/// encoded; the per-terminal environment locates the private script, which
/// stays alive with IdentityBridge and preserves UTF-8 input under PS 5.1.
#[cfg(any(windows, test))]
fn windows_hook_command(
    executable: &str,
    address: SocketAddr,
    token: &str,
    terminal_id: &str,
    agent: &str,
    resource_dir: &Path,
) -> Result<(String, (String, String))> {
    use crate::platform::launcher::ps_quote;
    use base64::Engine;
    let script = format!(
        "\u{feff}$OutputEncoding = [System.Text.UTF8Encoding]::new($false); \
         {POWERSHELL_HOOK_INPUT} \
         $neHookInput | & {} {HELPER_FLAG} {} {} {} {}; \
         exit $LASTEXITCODE",
        ps_quote(executable),
        ps_quote(&address.to_string()),
        ps_quote(token),
        ps_quote(terminal_id),
        ps_quote(agent),
    );
    let path = resource_dir.join("identity-hook.ps1");
    write_private(&path, script.as_bytes())?;
    // No quoting-dependent path or metadata crosses CMD/native argument
    // parsing. This also works if Codex selects a Unix-like hook shell.
    let bootstrap = format!("& $env:{WINDOWS_HOOK_ENV}; exit $LASTEXITCODE");
    let utf16: Vec<u8> = bootstrap
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
    Ok((
        format!("powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand {encoded}"),
        (WINDOWS_HOOK_ENV.into(), path.to_string_lossy().into_owned()),
    ))
}

fn pi_extension(address: SocketAddr, token: &str, terminal_id: &str) -> Result<String> {
    let settings = serde_json::json!({
        "host": address.ip().to_string(),
        "port": address.port(),
        "token": token,
        "terminal_id": terminal_id,
    });
    Ok(format!(
        r#"import net from "node:net";
const connection = {settings};
export default function(pi) {{
  let turn = 0;
  let failed = false;
  const report = (source, ctx, activity = null, input = null) => new Promise((resolve) => {{
    const native_id = ctx.sessionManager.getSessionId();
    if (!native_id) {{ resolve(); return; }}
    const event = {{ native_id, cwd: ctx.sessionManager.getCwd(),
      source_path: ctx.sessionManager.getSessionFile() ?? null, source, activity, input }};
    const socket = net.createConnection({{host: connection.host, port: connection.port}});
    socket.setTimeout(400);
    socket.on("connect", () => socket.end(JSON.stringify({{
      token: connection.token, terminal_id: connection.terminal_id, event
    }}) + "\n"));
    // Drain the acknowledgement so Node can deliver end/close before the
    // awaited lifecycle callback returns. An unread ACK can strand the promise.
    socket.resume();
    socket.on("timeout", () => socket.destroy());
    socket.on("error", () => socket.destroy());
    socket.on("close", () => resolve());
  }});
  pi.on("session_start", (event, ctx) => report(event.reason, ctx));
  pi.on("agent_start", (_event, ctx) => {{
    failed = false;
    return report("agent_start", ctx, {{state: "working", turn_id: String(++turn), starts_turn: true}});
  }});
  pi.on("message_start", (event, ctx) => {{
    if (event.message.role !== "user") return;
    return report("message_start", ctx, null, {{
      id: event.message.timestamp == null ? `input:${{turn}}` : String(event.message.timestamp)
    }});
  }});
  pi.on("ui_prompt_start", (_event, ctx) => report("ui_prompt_start", ctx,
    {{state: "waiting", turn_id: String(turn)}}));
  pi.on("ui_prompt_end", (_event, ctx) => report("ui_prompt_end", ctx,
    {{state: "working", turn_id: String(turn)}}));
  pi.on("agent_end", (event, ctx) => {{
    const last = event.messages.filter(m => m.role === "assistant").at(-1);
    failed = last?.stopReason === "error";
    return report("agent_end", ctx);
  }});
  pi.on("agent_settled", (event, ctx) => {{
    return report("agent_settled", ctx, {{state: event.aborted ? "interrupted"
      : failed ? "failed" : "completed",
      turn_id: String(turn)}});
  }});
}}
"#
    ))
}

fn receive_events(
    listener: TcpListener,
    state: Arc<Mutex<BridgeState>>,
    token: String,
    terminal_id: String,
    agent: Agent,
) {
    let mut prompts = claude::InputTracker::default();
    loop {
        if state.lock().map_or(true, |state| state.stopped) {
            return;
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                if let Ok(envelope) = read_envelope(&mut stream) {
                    if envelope.token == token
                        && envelope.terminal_id == terminal_id
                        && valid_native_id(&envelope.event.native_id)
                    {
                        if let Ok(mut state) = state.lock() {
                            // Keep the latest identity if a CLI changes often
                            // before registration; never grow without a bound.
                            if state.pending.len() == MAX_PENDING_EVENTS {
                                state.pending.pop_front();
                            }
                            state.pending.push_back(PendingEvent {
                                event: envelope.event,
                                acknowledgement: stream,
                                active_record: envelope.active_record,
                            });
                        }
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => return,
        }
        // Only this worker invokes the callback, so buffered and live changes
        // stay ordered. Do not hold state while application code runs.
        loop {
            let next = state.lock().ok().and_then(|mut state| {
                let callback = state.callback.clone()?;
                state.pending.pop_front().map(|event| (callback, event))
            });
            match next {
                Some((callback, mut pending)) => {
                    // Claude startup hooks can finish after the CLI has
                    // switched again. Validate at consumption too, so a
                    // delayed old socket cannot restore an obsolete identity.
                    if agent == Agent::ClaudeCode {
                        consume_claude_event(&state, &callback, &mut pending, &mut prompts);
                    } else {
                        callback(pending.event);
                        // Stop hooks finish after the last identity callback,
                        // so process exit cannot dispose the bridge first.
                        let _ = pending.acknowledgement.write_all(b"ok\n");
                    }
                }
                None => break,
            }
        }
        let callback = state.lock().ok().and_then(|s| s.callback.clone());
        if let Some(callback) = callback {
            prompts.poll(&callback);
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

fn consume_claude_event(
    state: &Mutex<BridgeState>,
    callback: &IdentityCallback,
    pending: &mut PendingEvent,
    prompts: &mut claude::InputTracker,
) {
    let Some(path) = pending.active_record.as_ref() else {
        let _ = pending.acknowledgement.write_all(b"ok\n");
        return;
    };
    // Capture the transcript boundary before acknowledging the hook. Claude
    // writes the actual user turn only after this hook returns.
    if active_record_event(path, &pending.event)
        .is_some_and(|current| current.native_id == pending.event.native_id)
    {
        prompts.arm(&pending.event, path);
    }
    let input = pending.event.input.take();
    let mut observed = active_record_event(path, &pending.event);
    if let Some(event) = observed.as_ref() {
        callback(event.clone());
    }
    // An in-session resume hook can run before Claude finishes updating its
    // asynchronous PID record. Let that hook return before waiting for the
    // native write. Stop's current identity was already refreshed above.
    let _ = pending.acknowledgement.write_all(b"ok\n");
    if observed
        .as_ref()
        .is_some_and(|event| event.native_id == pending.event.native_id)
    {
        return;
    }
    // This is bounded settling of one native notification, not a persistent
    // watcher. A stale trigger always reads the current record and therefore
    // cannot restore its old ID, even if a later switch has already completed.
    for _ in 0..CLAUDE_RECORD_SETTLE_STEPS {
        if state.lock().map_or(true, |state| state.stopped) {
            return;
        }
        std::thread::sleep(CLAUDE_RECORD_SETTLE_INTERVAL);
        if let Some(event) = active_record_event(path, &pending.event) {
            let reached = event.native_id == pending.event.native_id;
            if reached && input.is_some() {
                let mut trigger = pending.event.clone();
                trigger.input = input.clone();
                prompts.arm(&trigger, path);
            }
            if observed.as_ref() != Some(&event) {
                callback(event.clone());
                observed = Some(event);
            }
            if reached {
                return;
            }
        }
    }
}

fn read_envelope(stream: &mut TcpStream) -> Result<Envelope> {
    // accept() can inherit the listener's nonblocking flag. A notification
    // may span multiple packets, so read it with a bounded blocking timeout.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    let mut bytes = Vec::new();
    stream.take(MAX_EVENT_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_EVENT_BYTES {
        return Err(other("identity notification exceeds size limit"));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn valid_native_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)
}

fn hook_event(value: serde_json::Value) -> Option<NativeIdentityEvent> {
    let name = value.get("hook_event_name")?.as_str()?;
    if !matches!(
        name,
        "SessionStart"
            | "Stop"
            | "UserPromptSubmit"
            | "PermissionRequest"
            | "PostToolUse"
            | "PostToolUseFailure"
            | "StopFailure"
            | "Interrupt"
            | "Notification"
    ) || value.get("is_sidechain").and_then(|v| v.as_bool()) == Some(true)
        || value.get("is_subagent").and_then(|v| v.as_bool()) == Some(true)
        || value
            .get("agent_id")
            .and_then(|v| v.as_str())
            .is_some_and(|id| !id.is_empty())
    {
        return None;
    }
    let native_id = value.get("session_id")?.as_str()?.to_string();
    if !valid_native_id(&native_id) {
        return None;
    }
    let text = |name: &str| {
        value
            .get(name)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Some(NativeIdentityEvent {
        native_id,
        cwd: text("cwd"),
        source_path: text("transcript_path"),
        source: text("source").or_else(|| Some(name.to_string())),
        activity: hook_activity(name, &value),
        input: (name == "UserPromptSubmit")
            .then(|| {
                value["prompt"].as_str().map(|_prompt| NativeInput {
                    // Native turn/prompt IDs can be shared by several steering
                    // inputs. Give this hook invocation its own input event ID.
                    id: Some(crate::storage::new_id()),
                })
            })
            .flatten(),
    })
}

fn hook_activity(name: &str, value: &serde_json::Value) -> Option<ActivitySignal> {
    let state = match name {
        "UserPromptSubmit" | "PostToolUse" | "PostToolUseFailure" => ActivityState::Working,
        "PermissionRequest" => ActivityState::Waiting,
        "StopFailure" => ActivityState::Failed,
        "Interrupt" => ActivityState::Interrupted,
        "Notification" if value["notification_type"] == "idle_prompt" => ActivityState::Idle,
        "Stop" => {
            let background = value["background_tasks"]
                .as_array()
                .is_some_and(|tasks| !tasks.is_empty());
            if background {
                ActivityState::Working
            } else {
                ActivityState::Completed
            }
        }
        _ => return None,
    };
    Some(ActivitySignal {
        state,
        turn_id: value["turn_id"].as_str().map(str::to_string),
        starts_turn: name == "UserPromptSubmit",
    })
}

/// Called before Tauri startup. Hook helpers do not open the app/NoEnding database or
/// run an Agent. A stale/closed listener is harmless to the Agent lifecycle.
pub fn run_hook_helper_if_requested() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    let arguments = match args.next().as_deref() {
        Some(antigravity::HELPER_FLAG) => {
            let source = args.next().unwrap_or_default();
            // Always return the neutral protocol response, even for a stale
            // listener or a normal CLI launched outside NoEnding.
            println!("{}", antigravity::response(&source));
            match antigravity::helper_arguments(&source) {
                Some(arguments) => arguments,
                None => return Some(0),
            }
        }
        Some(HELPER_FLAG) => args.collect(),
        _ => return None,
    };
    if let Err(error) = run_hook_helper(arguments, std::io::stdin().lock()) {
        eprintln!("[noending] identity notification: {error}");
    }
    Some(0)
}

fn run_hook_helper(args: Vec<String>, input: impl Read) -> Result<()> {
    let is_antigravity = args.get(3).is_some_and(|agent| agent == "antigravity");
    if args.len() != if is_antigravity { 6 } else { 4 } {
        return Err(other("invalid identity helper arguments"));
    }
    let address: SocketAddr = args[0]
        .parse()
        .map_err(|_| other("invalid identity listener address"))?;
    if !address.ip().is_loopback() {
        return Err(other("identity listener must be on loopback"));
    }
    let mut bytes = Vec::new();
    input
        .take(MAX_HOOK_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_HOOK_INPUT_BYTES {
        return Err(other("native hook input exceeds size limit"));
    }
    let value = serde_json::from_slice(&bytes)?;
    let event = if args[3] == "antigravity" {
        antigravity::hook_event(value, &args[4], Path::new(&args[5]))
    } else {
        hook_event(value)
    };
    let Some(event) = event else {
        return Ok(());
    };
    let active_record = if args[3] == "claude_code" {
        let Some(path) = claude_active_record() else {
            return Ok(());
        };
        Some(path)
    } else {
        None
    };
    let mut stream = TcpStream::connect_timeout(&address, IO_TIMEOUT)?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    serde_json::to_writer(
        &mut stream,
        &Envelope {
            token: args[1].clone(),
            terminal_id: args[2].clone(),
            event,
            active_record,
        },
    )?;
    stream.write_all(b"\n")?;
    stream.shutdown(Shutdown::Write)?;
    stream.set_read_timeout(Some(ACK_TIMEOUT))?;
    let mut acknowledgement = [0; 3];
    stream.read_exact(&mut acknowledgement)?;
    if &acknowledgement != b"ok\n" {
        return Err(other("invalid identity acknowledgement"));
    }
    Ok(())
}

fn claude_active_record() -> Option<PathBuf> {
    if matches!(
        std::env::var("CLAUDE_CODE_SESSION_KIND").as_deref(),
        Ok("bg" | "daemon" | "daemon-worker")
    ) {
        return None;
    }
    // Claude Code 2.1.285 injects its own process ID into every hook's env.
    // This avoids confusing hook helpers, npm shims, or Windows PTY wrappers
    // with the native interactive process.
    let pid = std::env::var("CLAUDE_PID").ok()?.parse::<u32>().ok()?;
    let root = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".claude")))?;
    let path = root.join("sessions").join(format!("{pid}.json"));
    // CLAUDE_CONFIG_DIR may be relative to the Agent cwd, while the bridge
    // lives in the app process. The startup record may not yet exist.
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    Some(std::fs::canonicalize(&absolute).unwrap_or(absolute))
}

fn active_record_event(path: &Path, trigger: &NativeIdentityEvent) -> Option<NativeIdentityEvent> {
    let pid = path.file_stem()?.to_str()?.parse::<u64>().ok()?;
    let record: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    if record.get("pid").and_then(|v| v.as_u64()) != Some(pid)
        || record.get("kind").and_then(|v| v.as_str()) != Some("interactive")
    {
        return None;
    }
    let native_id = record.get("sessionId")?.as_str()?;
    if !valid_native_id(native_id) {
        return None;
    }
    // The process record, not the triggering hook, is authoritative: startup
    // hooks run in the background, and fork can describe an inactive copy.
    // Never attach an old transcript path to a newer active native session.
    Some(NativeIdentityEvent {
        native_id: native_id.to_string(),
        cwd: record
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        source_path: (native_id == trigger.native_id)
            .then(|| trigger.source_path.clone())
            .flatten(),
        source: Some("active_record".into()),
        // An obsolete notification can reveal the current identity, but its
        // turn state must never be assigned to that different conversation.
        input: (native_id == trigger.native_id)
            .then(|| trigger.input.clone())
            .flatten(),
        activity: (native_id == trigger.native_id)
            .then(|| trigger.activity.clone())
            .flatten(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn command() -> AgentCommand {
        AgentCommand {
            program: "agent".into(),
            args: vec!["--".into(), "first user prompt\n$(literal)".into()],
            cwd: None,
        }
    }

    #[test]
    fn fragmented_identity_notification_waits_for_the_complete_payload() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        // Complete connection setup before testing the nonblocking read.
        let (mut server, _) = listener.accept().unwrap();
        // macOS and Windows can inherit the listener's nonblocking mode.
        // Force that mode so the same transport contract is tested everywhere.
        server.set_nonblocking(true).unwrap();
        let payload = serde_json::to_vec(&Envelope {
            token: "token".into(),
            terminal_id: "terminal".into(),
            event: NativeIdentityEvent {
                native_id: "native".into(),
                cwd: None,
                source_path: None,
                source: Some("resume".into()),
                input: None,
                activity: None,
            },
            active_record: None,
        })
        .unwrap();
        let split = payload.len() / 2;
        client.write_all(&payload[..split]).unwrap();
        let (sender, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            sender.send(read_envelope(&mut server)).unwrap();
        });
        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(25)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        client.write_all(&payload[split..]).unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        let envelope = receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert_eq!(envelope.event.native_id, "native");
        worker.join().unwrap();
    }

    #[test]
    fn identity_injection_preserves_original_arguments_and_cleans_up_resources() {
        for agent in [Agent::ClaudeCode, Agent::Codex, Agent::Pi] {
            let original = command();
            let prepared = prepare(&original, agent, "terminal").unwrap().unwrap();
            assert_eq!(prepared.command.args.last(), original.args.last());
            assert_eq!(
                &prepared.command.args[prepared.command.args.len() - 2..],
                original.args.as_slice()
            );
            assert_eq!(prepared.command.program, original.program);
            assert!(prepared.bridge.resource_dir.exists());
            #[cfg(windows)]
            if matches!(agent, Agent::Codex | Agent::ClaudeCode) {
                assert_eq!(prepared.environment.len(), 1);
                assert_eq!(prepared.environment[0].0, WINDOWS_HOOK_ENV);
                assert!(Path::new(&prepared.environment[0].1).is_file());
            }
            let dir = prepared.bridge.resource_dir.clone();
            drop(prepared);
            assert!(!dir.exists());
        }
        assert!(prepare(&command(), Agent::ZCode, "terminal")
            .unwrap()
            .is_none());
    }

    #[test]
    fn windows_codex_hooks_fit_cmd_limit_and_retain_all_trusted_events() {
        use base64::Engine;
        let directory = std::env::temp_dir().join(crate::storage::new_id());
        create_private_dir(&directory).unwrap();
        // A long, Unicode installation path must not be multiplied by six.
        let executable = format!(
            r"C:\Users\中文\{}\NoEnding's app.exe",
            "long-dir\\".repeat(20)
        );
        let (hook, (name, path)) = windows_hook_command(
            &executable,
            "127.0.0.1:54321".parse().unwrap(),
            "token",
            "terminal",
            "codex",
            &directory,
        )
        .unwrap();
        assert_eq!(name, "NOENDING_TERMINAL_HOOK");
        let script = std::fs::read_to_string(path).unwrap();
        assert!(
            script.starts_with('\u{feff}'),
            "PS 5.1 must read non-ASCII paths as UTF-8"
        );
        assert!(script.contains("NoEnding''s app.exe"));
        let encoded = hook.split_whitespace().last().unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        let bootstrap = String::from_utf16(
            &bytes
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert_eq!(
            bootstrap,
            "& $env:NOENDING_TERMINAL_HOOK; exit $LASTEXITCODE"
        );
        let mut args = codex_hook_args(&hook, true).unwrap();
        args.extend([
            "resume".into(),
            "12345678-1234-1234-1234-123456789012".into(),
        ]);
        let command = AgentCommand {
            program: r"C:\Users\中文\AppData\Roaming\npm\codex.cmd".into(),
            args: args.clone(),
            cwd: Some(directory.clone()),
        };
        // Budget the PS invocation too, including literal quoting, with room
        // for cmd.exe and the npm shim's node/script paths below 8191.
        let length = crate::platform::launcher::render_ps(&command)
            .encode_utf16()
            .count();
        assert!(
            length < 6000,
            "Windows Codex resume command is {length} characters"
        );
        let overrides = args[..args.len() - 2]
            .chunks_exact(2)
            .map(|pair| {
                assert_eq!(pair[0], "-c");
                pair[1].as_str()
            })
            .collect::<Vec<_>>()
            .join("\n");
        let config: serde_json::Value = toml_edit::de::from_str(&overrides).unwrap();
        for (event, key) in [
            ("SessionStart", "session_start"),
            ("UserPromptSubmit", "user_prompt_submit"),
            ("PermissionRequest", "permission_request"),
            ("PostToolUse", "post_tool_use"),
            ("Stop", "stop"),
            ("Interrupt", "interrupt"),
        ] {
            assert_eq!(
                config["hooks"][event][0]["hooks"][0],
                serde_json::json!({"type":"command","command":hook,"timeout":3,"async":false})
            );
            let normalized = format!(
                r#"{{"event_name":"{key}","hooks":[{{"async":false,"command":{},"timeout":3,"type":"command"}}]}}"#,
                serde_json::to_string(&hook).unwrap()
            );
            let fingerprint = format!("sha256:{:x}", Sha256::digest(normalized.as_bytes()));
            assert_eq!(
                config["hooks"]["state"][format!(r"C:\<session-flags>\config.toml:{key}:0:0")],
                serde_json::json!({"enabled":true,"trusted_hash":fingerprint})
            );
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn unix_codex_resume_preserves_hooks_and_arguments_through_the_login_shell() {
        use std::os::unix::fs::PermissionsExt;
        let directory = std::env::temp_dir().join(format!(
            "noending quote's 中文 {}",
            crate::storage::new_id()
        ));
        create_private_dir(&directory).unwrap();
        let executable = directory.join("codex");
        std::fs::write(&executable, "#!/bin/sh\nprintf '%s\\0' \"$PWD\" \"$@\"\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let original = AgentCommand {
            program: executable.to_string_lossy().into_owned(),
            args: vec!["resume".into(), "native-session-id".into()],
            cwd: Some(directory.clone()),
        };
        let prepared = prepare(&original, Agent::Codex, "terminal")
            .unwrap()
            .unwrap();
        let wrapped = super::super::spawn::wrapped_command(&prepared.command);
        let argv = wrapped.get_argv();
        let output = std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let expected = std::iter::once(directory.to_string_lossy().into_owned())
            .chain(prepared.command.args.clone())
            .map(|arg| format!("{arg}\0"))
            .collect::<String>();
        assert_eq!(output.stdout, expected.as_bytes());
        let config: serde_json::Value = toml_edit::de::from_str(
            &prepared.command.args[..14]
                .chunks_exact(2)
                .map(|pair| pair[1].as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        assert_eq!(
            config["hooks"]["SessionStart"][0]["hooks"][0]["type"],
            "command"
        );
        assert_eq!(config["hooks"]["Stop"][0]["hooks"][0]["timeout"], 3);
        assert_eq!(prepared.command.args[14..], ["resume", "native-session-id"]);
        std::fs::remove_dir_all(directory).unwrap();
    }

    // Windows PowerShell 5.1 is an OS prerequisite; missing it fails this test.
    // Node is provided by the frontend development/CI environment.
    #[cfg(windows)]
    #[test]
    fn windows_codex_cmd_shim_preserves_resume_hooks_and_utf8_input() {
        use std::os::windows::process::CommandExt;
        let directory = std::env::temp_dir().join(format!(
            "noending quote's 中文 {}",
            crate::storage::new_id()
        ));
        create_private_dir(&directory).unwrap();
        let node = std::process::Command::new("node")
            .args(["-p", "process.execPath"])
            .creation_flags(0x08000000)
            .output()
            .expect("Node is required to exercise the npm .cmd protocol");
        assert!(node.status.success());
        let node = String::from_utf8(node.stdout).unwrap();
        // The helper receives the real generated PS script's argv and stdin.
        std::fs::write(directory.join("helper.cjs"), r#"
let input = ''; process.stdin.setEncoding('utf8');
process.stdin.on('data', chunk => input += chunk);
process.stdin.on('end', () => console.log(JSON.stringify({args:process.argv.slice(2),input:JSON.parse(input)})));
"#).unwrap();
        std::fs::write(directory.join("codex.cjs"), r#"
const args=process.argv.slice(2);
const hook=args.find(arg=>arg.startsWith('hooks.SessionStart=')).match(/command='([^']+)'/)[1];
const result=require('node:child_process').spawnSync(hook,{shell:true,input:process.env.NOENDING_TEST_HOOK_INPUT,encoding:'utf8',timeout:5000,windowsHide:true});
console.log(JSON.stringify({args,status:result.status,output:result.stdout,error:result.stderr}));
"#).unwrap();
        for name in ["helper", "codex"] {
            std::fs::write(
                directory.join(format!("{name}.cmd")),
                format!(
                    "@echo off\r\n\"{}\" \"%~dp0{name}.cjs\" %*\r\n",
                    node.trim()
                ),
            )
            .unwrap();
        }
        let (hook, (name, path)) = windows_hook_command(
            &directory.join("helper.cmd").to_string_lossy(),
            "127.0.0.1:54321".parse().unwrap(),
            "token",
            "terminal",
            "codex",
            &directory,
        )
        .unwrap();
        let mut args = codex_hook_args(&hook, true).unwrap();
        args.extend(["resume".into(), "native-session-id".into()]);
        let cmd = AgentCommand {
            program: directory.join("codex.cmd").to_string_lossy().into_owned(),
            args: args.clone(),
            cwd: Some(directory.clone()),
        };
        let wrapped = super::super::spawn::wrapped_command(&cmd);
        let argv = wrapped.get_argv();
        let input = serde_json::json!({"hook_event_name":"Stop","session_id":"native","last_assistant_message":"完成了\"OK\"，下一行\nC:\\工作\\项目"});
        let output = std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .env(name, path)
            .env(
                "NOENDING_TEST_HOOK_INPUT",
                serde_json::to_string(&input).unwrap(),
            )
            .creation_flags(0x08000000)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["args"], serde_json::json!(args));
        assert_eq!(result["status"], 0, "{result}");
        let helper: serde_json::Value =
            serde_json::from_str(result["output"].as_str().unwrap().trim()).unwrap();
        assert_eq!(
            helper["args"],
            serde_json::json!([
                "--noending-identity-hook",
                "127.0.0.1:54321",
                "token",
                "terminal",
                "codex"
            ])
        );
        assert_eq!(helper["input"], input);
        std::fs::remove_dir_all(directory).unwrap();
    }

    // Windows PowerShell 5.1 is an OS prerequisite; missing it fails this test.
    #[cfg(windows)]
    #[test]
    fn windows_hook_decodes_utf8_json_under_a_legacy_console_code_page() {
        use base64::Engine;
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        let input = serde_json::to_vec(&serde_json::json!({
            "hook_event_name":"Stop", "session_id":"native",
            "last_assistant_message":"完成了\"OK\"，下一行\nC:\\工作\\项目"
        }))
        .unwrap();
        let expected = base64::engine::general_purpose::STANDARD.encode(&input);
        let script = format!(
            "[Console]::SetIn([System.IO.StreamReader]::new(\
             [Console]::OpenStandardInput(), [System.Text.Encoding]::GetEncoding(936))); \
             {POWERSHELL_HOOK_INPUT} \
             $actual = $neHookInput; \
             $expected = [System.Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{expected}')); \
             if ($actual -ceq $expected) {{ [Console]::Write('ok') }} else {{ exit 1 }}"
        );
        let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
        let mut child = Command::new("powershell.exe")
            .creation_flags(0x08000000) // Hooks must also work without an attached console.
            .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Windows PowerShell is required to check hook input encoding");
        child.stdin.take().unwrap().write_all(&input).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"ok");
    }

    #[test]
    fn helper_reports_completion_for_large_native_stop_payload_without_forwarding_the_answer() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            loop {
                match listener.accept() {
                    Ok((mut socket, _)) => {
                        let event = read_envelope(&mut socket).unwrap();
                        socket.write_all(b"ok\n").unwrap();
                        return Some(event);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if std::time::Instant::now() >= deadline {
                            return None;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("hook listener failed: {error}"),
                }
            }
        });
        let input = serde_json::to_vec(&serde_json::json!({
            "hook_event_name":"Stop", "session_id":"native", "turn_id":"turn",
            "last_assistant_message":"private-answer ".repeat(12000)
        }))
        .unwrap();
        let result = run_hook_helper(
            vec![
                address.to_string(),
                "token".into(),
                "terminal".into(),
                "codex".into(),
            ],
            input.as_slice(),
        );
        let received = worker.join().unwrap();
        assert!(result.is_ok(), "{result:?}");
        let received = received.expect("completion notification must reach the terminal");
        assert_eq!(received.terminal_id, "terminal");
        assert_eq!(
            received.event.activity.as_ref().unwrap().state,
            ActivityState::Completed
        );
        assert!(!serde_json::to_string(&received.event)
            .unwrap()
            .contains("private-answer"));
    }

    /// Node is a prerequisite (also used by the app frontend and installed in CI).
    /// Execute the generated extension against the real authenticated listener.
    #[test]
    fn pi_extension_activity_roundtrips_without_stalling_or_premature_completion() {
        use std::process::{Command, Stdio};
        let prepared = prepare(&command(), Agent::Pi, "terminal").unwrap().unwrap();
        let (tx, rx) = mpsc::channel();
        prepared.bridge.activate(Arc::new(move |event| {
            tx.send(event).unwrap();
        }));
        let script = r#"
const {pathToFileURL} = await import('node:url');
const {default: extension} = await import(pathToFileURL(process.argv[1]).href);
const handlers = new Map();
extension({on(name, handler) { handlers.set(name, handler); }});
const ctx = {sessionManager: {getSessionId:()=>"native", getCwd:()=>"/fixture", getSessionFile:()=>null}};
const run = (name, event={}) => handlers.get(name)(event, ctx);
await run("session_start", {reason:"resume"});
await run("agent_start");
await run("ui_prompt_start");
await run("ui_prompt_end");
await run("agent_end", {messages:[{role:"assistant", stopReason:"error"}]});
await run("agent_end", {messages:[{role:"assistant", stopReason:"stop"}]});
await run("agent_settled", {aborted:false});
await run("agent_start");
await run("agent_end", {messages:[{role:"assistant", stopReason:"error"}]});
await run("agent_settled", {aborted:false});
await run("agent_start");
await run("agent_settled", {aborted:true});
await run("message_start", {message:{role:"assistant", content:[{type:"text", text:"ignored"}]}});
await run("message_start", {message:{role:"user", timestamp:42, content:[{type:"text", text:"  中文\r\nnext 😀  "}]}});
"#;
        let mut child = Command::new("node")
            .args(["--input-type=module", "--eval", script])
            .arg(&prepared.command.args[1])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Node is required to verify the Pi extension protocol");
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if std::time::Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Pi lifecycle callbacks stalled on the bridge acknowledgement");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let events: Vec<_> = (0..13)
            .map(|_| rx.recv_timeout(Duration::from_secs(1)).unwrap())
            .collect();
        use ActivityState::*;
        let states: Vec<_> = events
            .iter()
            .map(|e| e.activity.as_ref().map(|s| s.state))
            .collect();
        assert_eq!(
            states,
            vec![
                None,
                Some(Working),
                Some(Waiting),
                Some(Working),
                None,
                None,
                Some(Completed),
                Some(Working),
                None,
                Some(Failed),
                Some(Working),
                Some(Interrupted),
                None
            ]
        );
        assert_eq!(
            events[12].input,
            Some(NativeInput {
                id: Some("42".into()),
            })
        );
        assert!(!serde_json::to_string(&events[12]).unwrap().contains("中文"));
        assert!(events[..12].iter().all(|event| event.input.is_none()));
        assert!(events.iter().all(|event| event.native_id == "native"));
        assert_ne!(
            events[1].activity.as_ref().unwrap().turn_id,
            events[7].activity.as_ref().unwrap().turn_id
        );
    }

    #[test]
    fn input_hooks_schedule_ingestion_without_exposing_prompt_content() {
        let payload = serde_json::json!({"hook_event_name":"UserPromptSubmit","session_id":"native","prompt":"private text"});
        let first = hook_event(payload.clone()).unwrap();
        let second = hook_event(payload).unwrap();
        assert!(first.input.is_some());
        assert_ne!(first.input, second.input);
        assert!(!serde_json::to_string(&first)
            .unwrap()
            .contains("private text"));
        assert!(hook_event(
            serde_json::json!({"hook_event_name":"PostToolUse","session_id":"native"})
        )
        .unwrap()
        .input
        .is_none());
    }

    #[test]
    fn hook_activity_distinguishes_prompt_permission_completion_and_failure() {
        let parse = |name, fields: serde_json::Value| {
            let mut input = serde_json::json!({"hook_event_name": name, "session_id":"native", "turn_id":"turn"});
            input
                .as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            hook_event(input).unwrap().activity.unwrap()
        };
        let start = parse("UserPromptSubmit", serde_json::json!({}));
        assert_eq!(start.state, ActivityState::Working);
        assert!(start.starts_turn);
        assert_eq!(start.turn_id.as_deref(), Some("turn"));
        for (name, expected) in [
            ("PermissionRequest", ActivityState::Waiting),
            ("PostToolUse", ActivityState::Working),
            ("Stop", ActivityState::Completed),
            ("StopFailure", ActivityState::Failed),
            ("Interrupt", ActivityState::Interrupted),
        ] {
            let event = parse(name, serde_json::json!({}));
            assert_eq!(event.state, expected);
            assert!(!event.starts_turn);
        }
        assert_eq!(
            parse(
                "Stop",
                serde_json::json!({"background_tasks":[{"id":"task"}]})
            )
            .state,
            ActivityState::Working
        );
    }

    #[test]
    fn native_hook_input_ignores_subagents_and_never_transports_message_content() {
        let event = hook_event(serde_json::json!({
            "hook_event_name": "Stop", "session_id": "native",
            "transcript_path": "/source", "last_assistant_message": "private"
        }))
        .unwrap();
        assert_eq!(event.native_id, "native");
        assert!(!serde_json::to_string(&event).unwrap().contains("private"));
        for extra in [
            serde_json::json!({"hook_event_name":"SubagentStop"}),
            serde_json::json!({"agent_id":"child"}),
            serde_json::json!({"is_sidechain":true}),
        ] {
            let mut value = serde_json::json!({
                "hook_event_name": "SessionStart", "session_id": "child"
            });
            value
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            assert!(hook_event(value).is_none());
        }
    }

    #[test]
    fn startup_events_buffer_until_activation_and_repeat_events_are_kept() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(BridgeState::default()));
        let resource_dir = std::env::temp_dir().join(crate::storage::new_id());
        create_private_dir(&resource_dir).unwrap();
        let bridge = IdentityBridge {
            state: state.clone(),
            resource_dir,
        };
        std::thread::spawn(move || {
            receive_events(
                listener,
                state,
                "token".into(),
                "terminal".into(),
                Agent::Codex,
            )
        });
        let input = |source| {
            serde_json::to_vec(&serde_json::json!({
                "hook_event_name":"SessionStart", "session_id":"native",
                "source":source, "cwd":"/cwd"
            }))
            .unwrap()
        };
        let mut helpers = Vec::new();
        for (index, source) in ["startup", "resume"].into_iter().enumerate() {
            let bytes = input(source);
            helpers.push(std::thread::spawn(move || {
                run_hook_helper(
                    vec![
                        address.to_string(),
                        "token".into(),
                        "terminal".into(),
                        "codex".into(),
                    ],
                    bytes.as_slice(),
                )
                .unwrap();
            }));
            let deadline = std::time::Instant::now() + Duration::from_secs(1);
            while bridge.state.lock().unwrap().pending.len() != index + 1 {
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let (sender, receiver) = mpsc::channel();
        bridge.activate(Arc::new(move |event| {
            sender.send(event).unwrap();
        }));
        let first = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        let second = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(first.source.as_deref(), Some("startup"));
        assert_eq!(second.source.as_deref(), Some("resume"));
        assert_eq!(first.native_id, second.native_id);
        for helper in helpers {
            helper.join().unwrap();
        }
    }

    fn write_record(path: &Path, native_id: &str, kind: &str) {
        std::fs::write(
            path,
            serde_json::to_vec(&serde_json::json!({
                "pid":123, "sessionId":native_id,"kind":kind,"cwd":"/native-cwd"
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn claude_native_record_rejects_background_and_obsolete_transcript_paths() {
        let temp = std::env::temp_dir().join(crate::storage::new_id());
        create_private_dir(&temp).unwrap();
        let path = temp.join("123.json");
        let trigger = NativeIdentityEvent {
            native_id: "old".into(),
            cwd: Some("/old-cwd".into()),
            source_path: Some("/old-transcript".into()),
            source: Some("fork".into()),
            input: None,
            activity: Some(ActivitySignal {
                state: ActivityState::Completed,
                turn_id: Some("old-turn".into()),
                starts_turn: false,
            }),
        };
        write_record(&path, "current", "interactive");
        let current = active_record_event(&path, &trigger).unwrap();
        assert_eq!(current.native_id, "current");
        assert_eq!(current.cwd.as_deref(), Some("/native-cwd"));
        assert_eq!(current.source.as_deref(), Some("active_record"));
        assert_eq!(current.source_path, None);
        assert_eq!(current.activity, None);
        for kind in ["bg", "daemon", "daemon-worker"] {
            write_record(&path, "current", kind);
            assert!(active_record_event(&path, &trigger).is_none());
        }
        write_record(&path, "current", "interactive");
        assert!(active_record_event(&temp.join("999.json"), &trigger).is_none());
        std::fs::rename(&path, temp.join("999.json")).unwrap();
        assert!(active_record_event(&temp.join("999.json"), &trigger).is_none());
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn claude_resume_settles_native_write_after_ack_and_late_hooks_read_current_id() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(BridgeState::default()));
        let resource_dir = std::env::temp_dir().join(crate::storage::new_id());
        create_private_dir(&resource_dir).unwrap();
        let path = resource_dir.join("123.json");
        write_record(&path, "before", "interactive");
        let bridge = IdentityBridge {
            state: state.clone(),
            resource_dir,
        };
        std::thread::spawn(move || {
            receive_events(
                listener,
                state,
                "token".into(),
                "terminal".into(),
                Agent::ClaudeCode,
            )
        });
        let (sender, receiver) = mpsc::channel();
        bridge.activate(Arc::new(move |event| {
            sender.send(event).unwrap();
        }));
        let notify = |native_id: &str, source: &str| {
            let mut stream = TcpStream::connect(address).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            serde_json::to_writer(
                &mut stream,
                &Envelope {
                    token: "token".into(),
                    terminal_id: "terminal".into(),
                    event: NativeIdentityEvent {
                        native_id: native_id.into(),
                        cwd: None,
                        source_path: Some(format!("/{native_id}")),
                        source: Some(source.into()),
                        input: None,
                        activity: None,
                    },
                    active_record: Some(path.clone()),
                },
            )
            .unwrap();
            stream.shutdown(Shutdown::Write).unwrap();
            let mut ack = [0; 3];
            stream.read_exact(&mut ack).unwrap();
            assert_eq!(&ack, b"ok\n");
        };
        notify("after", "resume");
        assert_eq!(
            receiver
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .native_id,
            "before"
        );
        // The Agent can complete this native write only after its hook exits.
        write_record(&path, "after", "interactive");
        let resumed = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(resumed.native_id, "after");
        assert_eq!(resumed.source_path.as_deref(), Some("/after"));
        notify("before", "startup");
        let late = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(late.native_id, "after");
        assert_eq!(late.source_path, None);
        drop(bridge);
    }
}
