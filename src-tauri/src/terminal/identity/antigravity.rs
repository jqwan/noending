//! Antigravity CLI's named hooks have no per-launch configuration flag.
//! Keep one neutral plugin in the global customization root; listener credentials live
//! exclusively in each PTY's environment, never in the shared hook file.

use super::*;

pub(super) const HELPER_FLAG: &str = "--noending-antigravity-hook";
const ENVIRONMENT: &str = "NOENDING_ANTIGRAVITY_IDENTITY";
const PLUGIN: &str = "noending-terminal-identity";
static INSTALL_LOCK: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize)]
struct Target {
    address: SocketAddr,
    token: String,
    terminal_id: String,
    store_root: PathBuf,
}

pub(super) fn environment(
    address: SocketAddr,
    token: &str,
    terminal_id: &str,
    store_root: &Path,
) -> Result<(String, String)> {
    Ok((
        ENVIRONMENT.into(),
        serde_json::to_string(&Target {
            address,
            token: token.into(),
            terminal_id: terminal_id.into(),
            store_root: store_root.into(),
        })?,
    ))
}

pub(super) fn helper_arguments(source: &str) -> Option<Vec<String>> {
    arguments(&std::env::var(ENVIRONMENT).ok()?, source)
}

fn arguments(environment: &str, source: &str) -> Option<Vec<String>> {
    if !matches!(source, "PreInvocation" | "Stop") {
        return None;
    }
    let target: Target = serde_json::from_str(environment).ok()?;
    Some(vec![
        target.address.to_string(),
        target.token,
        target.terminal_id,
        "antigravity".into(),
        source.into(),
        target.store_root.to_string_lossy().into_owned(),
    ])
}

pub(super) fn response(source: &str) -> &'static str {
    if source == "Stop" {
        r#"{"decision":"stop"}"#
    } else {
        "{}"
    }
}

pub(super) fn hook_event(
    value: serde_json::Value,
    source: &str,
    store_root: &Path,
) -> Option<NativeIdentityEvent> {
    if !matches!(source, "PreInvocation" | "Stop") {
        return None;
    }
    let native_id = value.get("conversationId")?.as_str()?;
    uuid::Uuid::parse_str(native_id).ok()?;
    // CLI and IDE hooks have the same shape. Require this CLI's explicit
    // transcript location and native root relation before changing identity.
    let transcript = Path::new(value.get("transcriptPath")?.as_str()?);
    let logs = store_root
        .join("brain")
        .join(native_id)
        .join(".system_generated/logs");
    // agy sends the full transcript in lifecycle payloads; its exported
    // transcript is a separate file in the same conversation directory.
    let valid_transcript = transcript == logs.join("transcript_full.jsonl")
        || transcript == logs.join("transcript.jsonl");
    if !valid_transcript
        || !crate::adapters::antigravity::is_root_conversation(store_root, native_id)
    {
        return None;
    }
    let cwd = value
        .get("workspacePaths")?
        .as_array()?
        .iter()
        .filter_map(|value| value.as_str())
        .find(|path| Path::new(path).is_absolute())
        .map(str::to_string);
    Some(NativeIdentityEvent {
        native_id: native_id.into(),
        cwd,
        source_path: Some(transcript.to_string_lossy().into_owned()),
        source: Some(source.into()),
        activity: match source {
            "PreInvocation" => Some(ActivitySignal {
                state: ActivityState::Working,
                turn_id: None,
                starts_turn: true,
            }),
            "Stop" => Some(ActivitySignal {
                state: match value["terminationReason"].as_str() {
                    Some("error" | "max_steps_exceeded") => ActivityState::Failed,
                    Some("model_stop") if value["fullyIdle"] == true => ActivityState::Completed,
                    _ if value["fullyIdle"] == false => ActivityState::Working,
                    _ => ActivityState::Idle,
                },
                turn_id: None,
                starts_turn: false,
            }),
            _ => None,
        },
    })
}

pub(super) fn install_plugin(store_root: &Path, executable: &Path) -> Result<()> {
    let _guard = INSTALL_LOCK
        .lock()
        .map_err(|_| other("identity plugin lock poisoned"))?;
    // The CLI data store's plugins can appear in the hook listing without
    // participating in model execution. agy resolves executable customizations
    // from ~/.gemini/config, beside ~/.gemini/antigravity-cli.
    let plugin = store_root
        .with_file_name("config")
        .join("plugins")
        .join(PLUGIN);
    // Own a separate package; never merge or replace the user's hooks.json,
    // settings.json, import manifest, or other plugins.
    std::fs::create_dir_all(&plugin)?;
    let marker = plugin.join(".noending-managed");
    if !marker.exists() {
        if plugin.join("plugin.json").exists() || plugin.join("hooks.json").exists() {
            return Err(other(format!(
                "Antigravity 插件目录已被占用: {}",
                plugin.display()
            )));
        }
        write_private(&marker, b"noending-terminal-identity-v1\n")?;
    }
    let hooks = serde_json::json!({ PLUGIN: {
        "PreInvocation": [{"type":"command", "command": command(executable, "PreInvocation"), "timeout":3}],
        "Stop": [{"type":"command", "command": command(executable, "Stop"), "timeout":3}]
    }});
    replace_owned(
        &plugin.join("hooks.json"),
        &serde_json::to_vec_pretty(&hooks)?,
    )?;
    // Publish the manifest last, so a new package is never discovered without
    // its handlers. Repeated launches update only this package's executable.
    replace_owned(
        &plugin.join("plugin.json"),
        &serde_json::to_vec_pretty(&serde_json::json!({
            "name":PLUGIN,
            "description":"Native session notifications for NoEnding embedded terminals"
        }))?,
    )?;
    let legacy = store_root.join("plugins").join(PLUGIN);
    if std::fs::read(legacy.join(".noending-managed"))
        .ok()
        .as_deref()
        == Some(b"noending-terminal-identity-v1\n")
    {
        std::fs::remove_dir_all(legacy)?;
    }
    Ok(())
}

fn replace_owned(path: &Path, bytes: &[u8]) -> Result<()> {
    if std::fs::read(path).ok().as_deref() == Some(bytes) {
        return Ok(());
    }
    let staging = path.with_extension(format!("{}.tmp", crate::storage::new_id()));
    write_private(&staging, bytes)?;
    let result = std::fs::rename(&staging, path);
    if result.is_err() {
        let _ = std::fs::remove_file(&staging);
    }
    result?;
    Ok(())
}

fn command(executable: &Path, source: &str) -> String {
    let executable = executable.to_string_lossy();
    #[cfg(unix)]
    {
        let executable = crate::platform::launcher::shell_quote(&executable);
        format!(
            "if [ -n \"${ENVIRONMENT}\" ] && [ -x {executable} ]; then {executable} {HELPER_FLAG} {source}; else printf '%s\\n' '{}'; fi",
            response(source)
        )
    }
    #[cfg(windows)]
    {
        use base64::Engine;
        let executable = crate::platform::launcher::ps_quote(&executable);
        let script = format!(
            "$OutputEncoding = [System.Text.UTF8Encoding]::new($false); \
             if ($env:{ENVIRONMENT} -and (Test-Path -LiteralPath {executable})) {{ \
             {POWERSHELL_HOOK_INPUT} \
             $neHookInput | & {executable} {HELPER_FLAG} {source}; exit $LASTEXITCODE \
             }} else {{ [Console]::WriteLine('{}') }}",
            response(source)
        );
        let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        format!("powershell.exe -NoProfile -NonInteractive -EncodedCommand {encoded}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    const ROOT_A: &str = "4425e665-16cd-4828-8b1c-e39de782e9af";
    const ROOT_B: &str = "5c031a03-f81d-4e15-b5e4-9cc45821a19d";
    const CHILD: &str = "e130ee70-2bca-4440-9c3e-ed0ac7858838";

    struct Store(PathBuf);
    impl Store {
        fn new() -> Self {
            let base =
                std::env::temp_dir().join(format!("noending-agy-{}", crate::storage::new_id()));
            create_private_dir(&base).unwrap();
            let root = base.join("antigravity-cli");
            create_private_dir(&root).unwrap();
            let conn = rusqlite::Connection::open(root.join("conversation_summaries.db")).unwrap();
            conn.execute_batch("CREATE TABLE conversation_summaries (conversation_id TEXT PRIMARY KEY, parent_conversation_id TEXT)").unwrap();
            for (id, parent) in [(ROOT_A, None), (ROOT_B, Some("  ")), (CHILD, Some(ROOT_A))] {
                conn.execute(
                    "INSERT INTO conversation_summaries VALUES (?1, ?2)",
                    rusqlite::params![id, parent],
                )
                .unwrap();
            }
            Self(root)
        }
    }
    impl Drop for Store {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
        }
    }

    fn input(root: &Path, id: &str) -> serde_json::Value {
        serde_json::json!({
            "conversationId":id,
            "workspacePaths":[root.to_string_lossy()],
            // Captured from agy 1.3.3's real PreInvocation and Stop payloads.
            "transcriptPath":root.join("brain").join(id).join(".system_generated/logs/transcript_full.jsonl"),
            "invocationNum":0,"initialNumSteps":3,
            "lastAssistantMessage":"private response"
        })
    }

    #[test]
    fn activity_waits_for_background_work_and_does_not_treat_failure_as_completion() {
        let store = Store::new();
        for (reason, idle, expected) in [
            ("model_stop", true, ActivityState::Completed),
            ("model_stop", false, ActivityState::Working),
            ("error", true, ActivityState::Failed),
            ("max_steps_exceeded", true, ActivityState::Failed),
        ] {
            let mut value = input(&store.0, ROOT_A);
            value["terminationReason"] = serde_json::json!(reason);
            value["fullyIdle"] = serde_json::json!(idle);
            assert_eq!(
                hook_event(value, "Stop", &store.0)
                    .unwrap()
                    .activity
                    .unwrap()
                    .state,
                expected
            );
        }
    }

    #[test]
    fn native_hooks_require_a_cli_root_and_ignore_children_and_unknown_relations() {
        let store = Store::new();
        for source in ["PreInvocation", "Stop"] {
            let event = hook_event(input(&store.0, ROOT_A), source, &store.0).unwrap();
            assert_eq!(event.native_id, ROOT_A);
            assert_eq!(event.cwd, Some(store.0.to_string_lossy().into_owned()));
            assert_eq!(event.source.as_deref(), Some(source));
            assert!(!serde_json::to_string(&event)
                .unwrap()
                .contains("private response"));
            assert!(hook_event(input(&store.0, CHILD), source, &store.0).is_none());
            assert!(hook_event(input(&store.0, "bad/id"), source, &store.0).is_none());
            assert!(hook_event(
                input(&store.0, "1e4ab219-b426-43e0-a8c9-e332b6570989"),
                source,
                &store.0
            )
            .is_none());
            assert!(hook_event(input(&store.0.join("ide"), ROOT_A), source, &store.0).is_none());

            let mut exported = input(&store.0, ROOT_A);
            exported["transcriptPath"] = serde_json::json!(store
                .0
                .join("brain")
                .join(ROOT_A)
                .join(".system_generated/logs/transcript.jsonl"));
            assert!(hook_event(exported, source, &store.0).is_some());

            for invalid in [
                store
                    .0
                    .join("brain")
                    .join(ROOT_B)
                    .join(".system_generated/logs/transcript_full.jsonl"),
                store
                    .0
                    .join("brain")
                    .join(ROOT_A)
                    .join(".system_generated/logs/other.jsonl"),
            ] {
                let mut input = input(&store.0, ROOT_A);
                input["transcriptPath"] = serde_json::json!(invalid);
                assert!(hook_event(input, source, &store.0).is_none());
            }
        }
        assert!(hook_event(input(&store.0, ROOT_A), "PreToolUse", &store.0).is_none());
        assert!(hook_event(input(&store.0, ROOT_B), "PreInvocation", &store.0).is_some());
        let missing = store.0.join("missing");
        assert!(hook_event(input(&missing, ROOT_A), "Stop", &missing).is_none());
        assert!(
            !missing.exists(),
            "read-only validation must not create an agent database"
        );
    }

    #[test]
    fn concurrent_terminals_route_both_native_events_to_their_own_listener() {
        let store = Store::new();
        let original = AgentCommand {
            program: "agy".into(),
            args: vec![
                "--prompt-interactive".into(),
                "first prompt\n$(literal)".into(),
            ],
            cwd: Some(store.0.clone()),
        };
        let first = prepare_at(&original, Agent::Antigravity, "terminal-a", Some(&store.0))
            .unwrap()
            .unwrap();
        let second = prepare_at(&original, Agent::Antigravity, "terminal-b", Some(&store.0))
            .unwrap()
            .unwrap();
        let (send_a, recv_a) = mpsc::channel();
        let (send_b, recv_b) = mpsc::channel();
        first.bridge.activate(Arc::new(move |event| {
            send_a.send(event).unwrap();
        }));
        second.bridge.activate(Arc::new(move |event| {
            send_b.send(event).unwrap();
        }));
        assert_eq!(first.command.args, original.args);
        assert_eq!(second.command.args, original.args);
        assert_ne!(first.environment, second.environment);
        let hooks = std::fs::read_to_string(
            store
                .0
                .with_file_name("config")
                .join("plugins")
                .join(PLUGIN)
                .join("hooks.json"),
        )
        .unwrap();
        for prepared in [&first, &second] {
            let target: Target = serde_json::from_str(&prepared.environment[0].1).unwrap();
            assert!(!hooks.contains(&target.token));
            assert!(!hooks.contains(&target.terminal_id));
        }
        for source in ["PreInvocation", "Stop"] {
            let workers: Vec<_> = [(&first, ROOT_A), (&second, ROOT_B)]
                .into_iter()
                .map(|(prepared, id)| {
                    let args = arguments(&prepared.environment[0].1, source).unwrap();
                    let bytes = serde_json::to_vec(&input(&store.0, id)).unwrap();
                    std::thread::spawn(move || run_hook_helper(args, bytes.as_slice()).unwrap())
                })
                .collect();
            let a = recv_a.recv_timeout(Duration::from_secs(2)).unwrap();
            let b = recv_b.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(a.native_id, ROOT_A);
            assert_eq!(b.native_id, ROOT_B);
            assert_eq!(a.source.as_deref(), Some(source));
            assert_eq!(b.source.as_deref(), Some(source));
            for worker in workers {
                worker.join().unwrap();
            }
        }
        assert!(recv_a.try_recv().is_err());
        assert!(recv_b.try_recv().is_err());
        let resource = first.bridge.resource_dir.clone();
        drop(first);
        assert!(!resource.exists());
        assert!(arguments("", "Stop").is_none());
        assert!(arguments(&second.environment[0].1, "PostToolUse").is_none());
    }

    #[test]
    fn plugin_install_preserves_user_configuration_and_other_packages() {
        let store = Store::new();
        let settings = store.0.join("settings.json");
        let config = store.0.with_file_name("config");
        let other = config.join("plugins/user-plugin");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(
            &settings,
            br#"{"hooks":{"user":{"enabled":false}},"model":"user-model"}"#,
        )
        .unwrap();
        std::fs::write(other.join("hooks.json"), b"user hooks").unwrap();
        let legacy = store.0.join("plugins").join(PLUGIN);
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(
            legacy.join(".noending-managed"),
            b"noending-terminal-identity-v1\n",
        )
        .unwrap();
        std::fs::write(legacy.join("hooks.json"), b"old managed hooks").unwrap();
        let before = std::fs::read(&settings).unwrap();
        install_plugin(&store.0, Path::new("first app")).unwrap();
        install_plugin(&store.0, Path::new("updated app")).unwrap();
        assert_eq!(std::fs::read(&settings).unwrap(), before);
        assert!(!legacy.exists());
        assert_eq!(
            std::fs::read(other.join("hooks.json")).unwrap(),
            b"user hooks"
        );
        let plugin = config.join("plugins").join(PLUGIN);
        let hooks: serde_json::Value =
            serde_json::from_slice(&std::fs::read(plugin.join("hooks.json")).unwrap()).unwrap();
        assert!(hooks[PLUGIN]["PreInvocation"][0]["command"].is_string());
        assert!(hooks[PLUGIN]["Stop"][0]["command"].is_string());
        std::fs::remove_file(plugin.join(".noending-managed")).unwrap();
        let before = std::fs::read(plugin.join("hooks.json")).unwrap();
        assert!(install_plugin(&store.0, Path::new("third app")).is_err());
        assert_eq!(std::fs::read(plugin.join("hooks.json")).unwrap(), before);
    }

    #[cfg(unix)]
    #[test]
    fn hooks_outside_noending_return_neutral_json_without_launching_the_app() {
        for source in ["PreInvocation", "Stop"] {
            // A shell executable would fail if invoked with our helper flag.
            let result = std::process::Command::new("/bin/sh")
                .args(["-c", &command(Path::new("/bin/sh"), source)])
                .env_remove(ENVIRONMENT)
                .output()
                .unwrap();
            assert!(result.status.success());
            let output: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
            if source == "Stop" {
                assert_eq!(output, serde_json::json!({"decision":"stop"}));
            } else {
                assert_eq!(output, serde_json::json!({}));
            }
        }
    }
}
