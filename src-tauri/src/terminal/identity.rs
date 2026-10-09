//! Per-terminal native-session notifications from Agent lifecycle hooks.
//!
//! Nothing is inferred from terminal output or message text. Each embedded
//! launch gets its own authenticated loopback listener and temporary, explicit
//! CLI resources; the user's persistent configuration is never changed.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::adapters::AgentCommand;
use crate::domain::Agent;
use crate::error::{other, Result};

const HELPER_FLAG: &str = "--noending-identity-hook";
const MAX_EVENT_BYTES: u64 = 128 * 1024;
const MAX_PENDING_EVENTS: usize = 128;
const IO_TIMEOUT: Duration = Duration::from_millis(400);
const ACK_TIMEOUT: Duration = Duration::from_secs(2);
const CLAUDE_RECORD_SETTLE_STEPS: usize = 16;
const CLAUDE_RECORD_SETTLE_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeIdentityEvent {
    pub native_id: String,
    pub cwd: Option<String>,
    pub source_path: Option<String>,
    pub source: Option<String>,
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
}

/// Add lifecycle observation to supported CLIs without changing their prompt
/// argument or replacing the user's hooks/extensions.
pub fn prepare(
    command: &AgentCommand,
    agent: Agent,
    terminal_id: &str,
) -> Result<Option<PreparedIdentityBridge>> {
    if !matches!(agent, Agent::ClaudeCode | Agent::Codex | Agent::Pi) {
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

    let args = match agent {
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
                "command": hook_command(address, &token, terminal_id, "claude_code")?,
                "timeout": 3
            });
            let hooks = serde_json::json!({ "hooks": {
                "SessionStart": [{ "hooks": [hook] }],
                "Stop": [{ "hooks": [hook] }],
                "UserPromptSubmit": [{ "hooks": [hook] }]
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
            let hook = hook_command(address, &token, terminal_id, "codex")?;
            // Codex 0.160.1 dispatches queued SessionStart hooks when the
            // next user turn begins. Idle TUI switches therefore become
            // known on that next turn; they are never inferred from output.
            // Codex discovers hooks independently in every active config
            // layer. SessionFlags add a source; lower user/project layers keep
            // their hooks even though ordinary config arrays are overridden.
            let group = format!(
                "[{{hooks=[{{type=\"command\",command={},timeout=3,async=false}}]}}]",
                serde_json::to_string(&hook)?
            );
            vec![
                "-c".into(),
                format!("hooks.SessionStart={group}"),
                "-c".into(),
                format!("hooks.Stop={group}"),
                "-c".into(),
                codex_trust_override(&hook)?,
            ]
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
    }))
}

fn codex_trust_override(command: &str) -> Result<String> {
    // These fingerprints are Codex's own versioned hook identity contract
    // (codex-rs/hooks/lib.rs and config/fingerprint.rs, rust-v0.160.1).
    // Trust only the two handlers introduced by this launch. Never use the
    // global bypass flag, which would also enable unrelated untrusted hooks.
    #[cfg(windows)]
    let source = r"C:\<session-flags>\config.toml";
    #[cfg(not(windows))]
    let source = "/<session-flags>/config.toml";
    let mut entries = Vec::new();
    for event in ["session_start", "stop"] {
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
        let key = serde_json::to_string(&format!("{source}:{event}:0:0"))?;
        entries.push(format!(
            "{key}={{enabled=true,trusted_hash={}}}",
            serde_json::to_string(&hash)?
        ));
    }
    // The CLI's dotted override parser does not understand quoted keys that
    // themselves contain dots. Put complete keys inside one inline table.
    Ok(format!("hooks.state={{{}}}", entries.join(",")))
}

fn create_private_dir(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
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
        use base64::Engine;
        // Codex can execute hooks through CMD, PowerShell, or a Unix-like
        // shell. A simple outer command and an encoded PowerShell body work
        // through all three, including executable paths containing spaces.
        let executable = crate::platform::launcher::ps_quote(&executable);
        let script = format!(
            "$OutputEncoding = [System.Text.UTF8Encoding]::new($false); \
             $neHookInput = [Console]::In.ReadToEnd(); \
             $neHookInput | & {executable} {HELPER_FLAG} {address} {token} {terminal_id} {agent}; \
             exit $LASTEXITCODE"
        );
        let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
        Ok(format!(
            "powershell.exe -NoProfile -NonInteractive -EncodedCommand {encoded}"
        ))
    }
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
  const report = (source, ctx) => new Promise((resolve) => {{
    const native_id = ctx.sessionManager.getSessionId();
    if (!native_id) {{ resolve(); return; }}
    const event = {{ native_id, cwd: ctx.sessionManager.getCwd(),
      source_path: ctx.sessionManager.getSessionFile() ?? null, source }};
    const socket = net.createConnection({{host: connection.host, port: connection.port}});
    socket.setTimeout(400);
    socket.on("connect", () => socket.end(JSON.stringify({{
      token: connection.token, terminal_id: connection.terminal_id, event
    }}) + "\n"));
    socket.on("timeout", () => socket.destroy());
    socket.on("error", () => socket.destroy());
    socket.on("close", () => resolve());
  }});
  pi.on("session_start", (event, ctx) => report(event.reason, ctx));
  pi.on("agent_end", (_event, ctx) => report("agent_end", ctx));
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
                        consume_claude_event(&state, &callback, &mut pending);
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
        std::thread::sleep(Duration::from_millis(15));
    }
}

fn consume_claude_event(
    state: &Mutex<BridgeState>,
    callback: &IdentityCallback,
    pending: &mut PendingEvent,
) {
    let Some(path) = pending.active_record.as_ref() else {
        let _ = pending.acknowledgement.write_all(b"ok\n");
        return;
    };
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
    if !matches!(name, "SessionStart" | "Stop" | "UserPromptSubmit")
        || value.get("is_sidechain").and_then(|v| v.as_bool()) == Some(true)
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
    })
}

/// Called before Tauri startup. Hook helpers do not open the app/database or
/// run an Agent. A stale/closed listener is harmless to the Agent lifecycle.
pub fn run_hook_helper_if_requested() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some(HELPER_FLAG) {
        return None;
    }
    if let Err(error) = run_hook_helper(args.collect(), std::io::stdin().lock()) {
        eprintln!("[noending] identity notification: {error}");
    }
    Some(0)
}

fn run_hook_helper(args: Vec<String>, input: impl Read) -> Result<()> {
    if args.len() != 4 {
        return Err(other("invalid identity helper arguments"));
    }
    let address: SocketAddr = args[0]
        .parse()
        .map_err(|_| other("invalid identity listener address"))?;
    if !address.ip().is_loopback() {
        return Err(other("identity listener must be on loopback"));
    }
    let mut bytes = Vec::new();
    input.take(MAX_EVENT_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_EVENT_BYTES {
        return Err(other("identity notification exceeds size limit"));
    }
    let Some(event) = hook_event(serde_json::from_slice(&bytes)?) else {
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
        listener.set_nonblocking(true).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
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
    fn identity_injection_preserves_the_prompt_and_user_configuration() {
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
            let dir = prepared.bridge.resource_dir.clone();
            drop(prepared);
            assert!(!dir.exists());
        }
        assert!(prepare(&command(), Agent::Antigravity, "terminal")
            .unwrap()
            .is_none());
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
        };
        write_record(&path, "current", "interactive");
        let current = active_record_event(&path, &trigger).unwrap();
        assert_eq!(current.native_id, "current");
        assert_eq!(current.cwd.as_deref(), Some("/native-cwd"));
        assert_eq!(current.source.as_deref(), Some("active_record"));
        assert_eq!(current.source_path, None);
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
