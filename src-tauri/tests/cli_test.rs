//! Exercise standalone console/desktop process entries, JSON and installed names.
use noending::{cli, storage::Db, workspace::home::NoEndingHome};
use noending_lib as noending;
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

struct Fixture {
    root: PathBuf,
    home: NoEndingHome,
    db: Option<Arc<Db>>,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "noending-cli-process-{}",
            noending::storage::new_id()
        ));
        let home = NoEndingHome::new(root.join("数据 home").to_str().unwrap(), None).unwrap();
        home.ensure_dirs().unwrap();
        let db = Arc::new(Db::open(&home.db_path).unwrap());
        Self {
            root,
            home,
            db: Some(db),
        }
    }
    fn run(&self, exe: &Path, args: &[&str]) -> (bool, Value) {
        let output = Command::new(exe)
            .args(args)
            .env("NOENDING_HOME", &self.home.root)
            .env_remove("NOENDING_TERMINAL_ID")
            .output()
            .unwrap();
        let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
            panic!(
                "{e}: stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (output.status.success(), value)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        drop(self.db.take());
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
// Let Rust apply batch-file quoting on Windows. Passing a .cmd path as an
// ordinary cmd.exe /C argument uses the wrong escaping for paths with spaces.
fn installed_command(directory: &Path, name: &str) -> Command {
    #[cfg(not(windows))]
    let path = directory.join(name);
    #[cfg(windows)]
    let path = directory.join(format!("{name}.cmd"));
    Command::new(path)
}
#[test]
fn console_and_desktop_entries_share_json_contract_without_starting_another_app() {
    let f = Fixture::new();
    for exe in [env!("CARGO_BIN_EXE_ne"), env!("CARGO_BIN_EXE_noending")] {
        let (success, help) = f.run(Path::new(exe), &["--help"]);
        assert!(success);
        assert_eq!(help["schema_version"], 1);
        assert_eq!(help["ok"], true);
        let (success, status) = f.run(Path::new(exe), &["status"]);
        assert!(success);
        assert_eq!(status["data"]["connected"], true);
        let (success, freshness) = f.run(Path::new(exe), &["freshness"]);
        assert!(success);
        assert_eq!(freshness["data"]["sessions"]["total"], 0);
        assert!(freshness["data"]["all_sources"]["checked_at"].is_null());
        let (success, error) = f.run(Path::new(exe), &["session", "delete", "unknown"]);
        assert!(!success);
        assert_eq!(error["ok"], false);
        assert_eq!(error["error"]["code"], "invalid_arguments");
    }
    // Native executable arguments retain multiline content on both platforms.
    let content = "literal $() `text`\n中文 ' quotes";
    let (_, added) = f.run(
        Path::new(env!("CARGO_BIN_EXE_ne")),
        &[
            "task",
            "create",
            "--title",
            content,
            "--request-id",
            "task-process",
        ],
    );
    assert_eq!(added["data"]["task"]["title"], content);
    let (success, repeated) = f.run(
        Path::new(env!("CARGO_BIN_EXE_noending")),
        &[
            "task",
            "create",
            "--title",
            content,
            "--request-id",
            "task-process",
        ],
    );
    assert!(success);
    assert_eq!(repeated["data"]["task"]["id"], added["data"]["task"]["id"]);
    let (success, receipt) = f.run(
        Path::new(env!("CARGO_BIN_EXE_ne")),
        &["request", "show", "task-process"],
    );
    assert!(success);
    assert_eq!(
        receipt["data"]["response"]["data"]["task"]["id"],
        added["data"]["task"]["id"]
    );
    assert_eq!(
        f.db.as_ref().unwrap().list_workstreams(None).unwrap().len(),
        1
    );
    let (success, created) = f.run(
        Path::new(env!("CARGO_BIN_EXE_ne")),
        &[
            "task",
            "create",
            "--title",
            "ProcessTask",
            "--description",
            "Clear me",
        ],
    );
    assert!(success);
    let tid = created["data"]["task"]["id"].as_str().unwrap();
    let (success, updated) = f.run(
        Path::new(env!("CARGO_BIN_EXE_noending")),
        &[
            "task",
            "update",
            tid,
            "--description",
            "",
            "--request-id",
            "process-clear",
        ],
    );
    assert!(success);
    assert_eq!(updated["data"]["request_id"], "process-clear");
    let persisted = f.db.as_ref().unwrap().get_workstream(tid).unwrap().unwrap();
    assert_eq!(persisted.title, "ProcessTask");
    assert_eq!(persisted.description, "");
}
#[test]
fn installed_ne_and_noending_commands_work_and_do_not_overwrite_existing_files() {
    let f = Fixture::new();
    let app_directory = f.root.join("app with spaces");
    std::fs::create_dir_all(&app_directory).unwrap();
    let installed_exe = app_directory.join(if cfg!(windows) { "ne.exe" } else { "ne" });
    std::fs::copy(env!("CARGO_BIN_EXE_ne"), &installed_exe).unwrap();
    let missing = f.root.join("not-installed");
    let (success, status) = f.run(
        &installed_exe,
        &["cli", "status", "--dir", missing.to_str().unwrap()],
    );
    assert!(success);
    assert!(status["data"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .all(|s| s["exists"] == false));
    assert!(!missing.exists());
    let directory = f.root.join("bin with space");
    let (success, result) = f.run(
        &installed_exe,
        &["cli", "install", "--dir", directory.to_str().unwrap()],
    );
    assert!(success);
    assert_eq!(result["data"]["commands"].as_array().unwrap().len(), 2);
    for name in ["ne", "noending"] {
        let output = installed_command(&directory, name)
            .args(["status"])
            .env("NOENDING_HOME", &f.home.root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["data"]["connected"], true);
    }
    let (success, error) = f.run(
        &installed_exe,
        &["cli", "install", "--dir", directory.to_str().unwrap()],
    );
    assert!(!success);
    assert_eq!(error["error"]["code"], "already_exists");
    let (success, status) = f.run(
        &installed_exe,
        &["cli", "status", "--dir", directory.to_str().unwrap()],
    );
    assert!(success);
    assert!(status["data"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .all(|s| s["managed"] == true && s["matches_current"] == true));
    #[cfg(not(windows))]
    let launcher = directory.join("ne");
    #[cfg(windows)]
    let launcher = directory.join("ne.cmd");
    let original = std::fs::read_to_string(&launcher).unwrap();
    let outdated = original.replacen(" --cli", " --obsolete-cli", 1);
    assert_ne!(original, outdated);
    std::fs::write(&launcher, outdated).unwrap();
    let (success, _) = f.run(
        &installed_exe,
        &["cli", "update", "--dir", directory.to_str().unwrap()],
    );
    assert!(success);
    assert_eq!(std::fs::read_to_string(&launcher).unwrap(), original);
    std::fs::write(&launcher, "unrelated command").unwrap();
    for action in ["update", "uninstall"] {
        let (success, error) = f.run(
            &installed_exe,
            &["cli", action, "--dir", directory.to_str().unwrap()],
        );
        assert!(!success);
        assert_eq!(error["error"]["code"], "unmanaged_launcher");
        assert_eq!(
            std::fs::read_to_string(&launcher).unwrap(),
            "unrelated command"
        );
    }
    std::fs::write(&launcher, &original).unwrap();
    #[cfg(unix)]
    {
        let outside = f.root.join("outside-launcher");
        std::fs::write(&outside, &original).unwrap();
        std::fs::remove_file(&launcher).unwrap();
        std::os::unix::fs::symlink(&outside, &launcher).unwrap();
        for action in ["update", "uninstall"] {
            let (success, error) = f.run(
                &installed_exe,
                &["cli", action, "--dir", directory.to_str().unwrap()],
            );
            assert!(!success);
            assert_eq!(error["error"]["code"], "unmanaged_launcher");
            assert_eq!(std::fs::read_to_string(&outside).unwrap(), original);
        }
        std::fs::remove_file(&launcher).unwrap();
        std::fs::write(&launcher, &original).unwrap();
    }
    let (success, _) = f.run(
        &installed_exe,
        &["cli", "uninstall", "--dir", directory.to_str().unwrap()],
    );
    assert!(success);
    assert!(!launcher.exists());
    let (success, status) = f.run(&installed_exe, &["status"]);
    assert!(success);
    assert_eq!(status["data"]["home"], f.home.root.to_str().unwrap());
}

#[test]
fn assistant_and_bundled_launchers_use_their_explicit_home_and_literal_arguments() {
    let mut f = Fixture::new();
    let directory = f.root.join("assistant's tools with spaces");
    cli::install_workspace_entry(
        &directory,
        &f.home.root,
        Path::new(env!("CARGO_BIN_EXE_ne")),
    )
    .unwrap();
    let mut entries = Vec::new();
    for (name, prepare) in [
        (
            "noending-memory",
            noending::skills::prepare_memory_skill
                as fn(&Db, &NoEndingHome, &Path) -> noending::error::Result<()>,
        ),
        ("noending-task", noending::skills::prepare_task_skill),
    ] {
        prepare(
            f.db.as_ref().unwrap(),
            &f.home,
            Path::new(env!("CARGO_BIN_EXE_ne")),
        )
        .unwrap();
        let skill = f.home.root.join("skills").join(name);
        let access: Value =
            serde_json::from_slice(&std::fs::read(skill.join("references/access.json")).unwrap())
                .unwrap();
        let entry = PathBuf::from(access["cli"].as_str().unwrap());
        assert!(entry.is_file());
        let status = Command::new(&entry)
            .arg("status")
            .current_dir(&f.root)
            .env("NOENDING_HOME", f.root.join("wrong-home"))
            .output()
            .unwrap();
        assert!(
            status.status.success(),
            "{}",
            String::from_utf8_lossy(&status.stderr)
        );
        let value: Value = serde_json::from_slice(&status.stdout).unwrap();
        assert_eq!(value["data"]["home"], access["home"]);
        entries.push(entry);
    }
    // Batch files cannot carry CR/LF arguments; the native entries cover those above.
    let content = "literal $() `text` 中文 ' quotes";
    let mut directories = vec![directory.as_path()];
    directories.extend(entries.iter().map(|entry| entry.parent().unwrap()));
    for directory in directories {
        for name in ["ne", "noending"] {
            let output = installed_command(&directory, name)
                .args([
                    "task",
                    "create",
                    "--title",
                    content,
                    "--request-id",
                    "assistant-launcher",
                ])
                .env("NOENDING_HOME", f.root.join("wrong-home"))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let result: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(result["data"]["task"]["title"], content);
        }
    }
    assert_eq!(
        f.db.as_ref().unwrap().list_workstreams(None).unwrap().len(),
        1
    );
    // Relocation preserves source links while rebinding the private command.
    let relocated =
        NoEndingHome::new(f.root.join("relocated home").to_str().unwrap(), None).unwrap();
    relocated.ensure_dirs().unwrap();
    drop(f.db.take());
    std::fs::remove_dir(&relocated.data_dir).unwrap();
    std::fs::rename(&f.home.data_dir, &relocated.data_dir).unwrap();
    let db = Db::open(&relocated.db_path).unwrap();
    for prepare in [
        noending::skills::prepare_memory_skill,
        noending::skills::prepare_task_skill,
    ] {
        prepare(&db, &relocated, Path::new(env!("CARGO_BIN_EXE_ne"))).unwrap();
    }
    for entry in entries {
        let status = Command::new(entry)
            .arg("status")
            .env("NOENDING_HOME", &f.home.root)
            .output()
            .unwrap();
        assert!(
            status.status.success(),
            "{}",
            String::from_utf8_lossy(&status.stderr)
        );
        let result: Value = serde_json::from_slice(&status.stdout).unwrap();
        assert_eq!(result["data"]["home"], relocated.root.to_str().unwrap());
    }
    assert_eq!(db.list_workstreams(None).unwrap().len(), 1);
    assert_eq!(
        noending::skills::load_skill_registry(&db)
            .unwrap()
            .sources
            .len(),
        2
    );
}

#[test]
fn concurrent_commands_initialize_one_home_and_replay_one_write_across_processes() {
    let f = Fixture::new();
    let home = f.root.join("fresh home");
    let children: Vec<_> = (0..6)
        .map(|_| {
            Command::new(env!("CARGO_BIN_EXE_ne"))
                .arg("--home")
                .arg(&home)
                .args([
                    "task",
                    "create",
                    "--title",
                    "One durable task",
                    "--request-id",
                    "parallel-task",
                ])
                .env("NOENDING_HOME", &f.home.root)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let mut completed = Vec::new();
    for child in children {
        let output = child.wait_with_output().unwrap();
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        if output.status.success() {
            completed.push(value["data"]["task"]["id"].as_str().unwrap().to_owned());
        } else {
            assert_eq!(value["error"]["code"], "request_running", "{value}");
        }
    }
    assert!(!completed.is_empty());
    let (success, replay) = f.run(
        Path::new(env!("CARGO_BIN_EXE_noending")),
        &[
            "--home",
            home.to_str().unwrap(),
            "task",
            "create",
            "--title",
            "One durable task",
            "--request-id",
            "parallel-task",
        ],
    );
    assert!(success, "{replay}");
    for id in completed {
        assert_eq!(replay["data"]["task"]["id"], id);
    }
    let (success, listed) = f.run(
        Path::new(env!("CARGO_BIN_EXE_ne")),
        &["--home", home.to_str().unwrap(), "task", "list"],
    );
    assert!(success, "{listed}");
    assert_eq!(listed["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(listed["data"]["items"][0]["title"], "One durable task");
    assert!(f
        .db
        .as_ref()
        .unwrap()
        .list_workstreams(None)
        .unwrap()
        .is_empty());
}

#[test]
fn explicit_sync_ingests_only_the_selected_source_then_refreshes_one_session() {
    use noending::domain::Agent;
    use serde_json::json;
    use std::io::Write;
    let f = Fixture::new();
    let root = f.root.join("codex-source");
    let cwd = f.root.join("project");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    let transcript = root.join("rollout-test.jsonl");
    let meta = json!({"type":"session_meta","payload":{"id":"cli-sync-native","cwd":cwd}});
    let message = |id: &str, text: &str| json!({"type":"response_item","payload":{"type":"message","id":id,"role":"user","content":[{"type":"input_text","text":text}]}});
    std::fs::write(
        &transcript,
        format!("{meta}\n{}\n", message("m1", "First memory")),
    )
    .unwrap();
    let source =
        f.db.as_ref()
            .unwrap()
            .add_ingest_source(Agent::Codex, root.to_str().unwrap())
            .unwrap();
    let exe = Path::new(env!("CARGO_BIN_EXE_ne"));
    let (_, before) = f.run(exe, &["session", "list"]);
    assert_eq!(before["data"]["items"], json!([]));
    let (success, sources) = f.run(exe, &["source", "list"]);
    assert!(success);
    assert!(sources["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["id"] == source.id));
    let (success, synced) = f.run(exe, &["sync", "--source", &source.id]);
    assert!(success, "{synced}");
    assert_eq!(synced["data"]["messages"], 1);
    let (_, sessions) = f.run(exe, &["session", "list"]);
    let sessions = sessions["data"]["items"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    let sid = sessions[0]["id"].as_str().unwrap();
    let (_, view) = f.run(exe, &["session", "show", sid]);
    assert_eq!(view["data"]["working_directory"], cwd.to_str().unwrap());
    assert!(view["data"]["project_id"].is_string());
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&transcript)
        .unwrap();
    writeln!(file, "{}", message("m2", "New memory")).unwrap();
    drop(file);
    let (_, old) = f.run(exe, &["session", "messages", sid]);
    assert_eq!(old["data"]["through_sequence"], 1);
    let (success, synced) = f.run(exe, &["sync", "--session", sid]);
    assert!(success, "{synced}");
    assert_eq!(synced["data"]["messages"], 1);
    let (_, latest) = f.run(exe, &["session", "messages", sid]);
    assert_eq!(latest["data"]["through_sequence"], 2);
    assert_eq!(
        latest["data"]["records"][1]["record"]["content"],
        "New memory"
    );
    let (success, missing) = f.run(exe, &["sync", "--source", "missing-source"]);
    assert!(!success);
    assert_eq!(missing["error"]["code"], "not_found");
}

#[test]
fn desktop_observer_sees_cli_commits_and_cli_preserves_an_unsupported_database() {
    use std::time::Duration;
    let f = Fixture::new();
    let (send, receive) = std::sync::mpsc::channel();
    let db = f.db.as_ref().unwrap().clone();
    let watcher = cli::watch_changes(db.clone(), move || {
        send.send(()).unwrap();
    });
    db.set_setting("local-change", "desktop").unwrap();
    assert!(receive.recv_timeout(Duration::from_millis(650)).is_err());
    let (success, added) = f.run(
        Path::new(env!("CARGO_BIN_EXE_ne")),
        &["task", "create", "--title", "Visible outside the CLI"],
    );
    assert!(success, "{added}");
    receive
        .recv_timeout(Duration::from_secs(5))
        .expect("desktop refresh after external commit");
    assert_eq!(
        db.list_workstreams(None).unwrap()[0].title,
        "Visible outside the CLI"
    );
    watcher.shutdown();
    let other = f.root.join("unsupported-home");
    std::fs::create_dir_all(other.join("data")).unwrap();
    let path = other.join("data/noending.db");
    let unsupported = rusqlite::Connection::open(&path).unwrap();
    unsupported
        .pragma_update(
            None,
            "application_id",
            noending::storage::DATABASE_APPLICATION_ID,
        )
        .unwrap();
    unsupported
        .pragma_update(
            None,
            "user_version",
            noending::storage::DATABASE_FORMAT_VERSION + 1,
        )
        .unwrap();
    unsupported
        .execute_batch("CREATE TABLE preserved(value TEXT); INSERT INTO preserved VALUES('keep');")
        .unwrap();
    drop(unsupported);
    let bytes = std::fs::read(&path).unwrap();
    let (success, result) = f.run(
        Path::new(env!("CARGO_BIN_EXE_ne")),
        &["--home", other.to_str().unwrap(), "status"],
    );
    assert!(!success);
    assert_eq!(result["ok"], false);
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

/// Python 3 is an explicit prerequisite for this Unix protocol stub; a missing
/// interpreter fails the test rather than skipping the extraction checks.
#[cfg(unix)]
#[test]
fn summary_command_waits_for_commit_and_preserves_data_and_locks_across_interruptions() {
    use noending::{
        context::summary::ExtractionFiles,
        domain::{Agent, ParsedSessionMessage, SessionMessageRole, SourceCursorUpdate},
    };
    use serde_json::json;
    use std::{
        os::unix::fs::PermissionsExt,
        time::{Duration, Instant},
    };
    assert!(Command::new("python3")
        .arg("--version")
        .status()
        .expect("Python 3 is required for this test")
        .success());
    let f = Fixture::new();
    let bin = f.root.join("agent-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let agent = bin.join("codex");
    std::fs::write(&agent, include_str!("fixtures/cli-agent.py")).unwrap();
    std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o700)).unwrap();
    let db = f.db.as_ref().unwrap();
    let sid = db
        .upsert_logical_session(
            Agent::Codex,
            "summary-protocol",
            None,
            Some("Protocol session"),
            None,
            None,
            None,
            None,
            None,
            "test",
            "/not/a/real/source",
            &json!({}),
        )
        .unwrap()
        .0;
    db.commit_ingest(
        &sid,
        &[ParsedSessionMessage {
            source_message_id: Some("m1".into()),
            role: SessionMessageRole::User,
            ts: None,
            content: "Please remember this result".into(),
        }],
        &SourceCursorUpdate {
            file_identity: "test".into(),
            generation: 0,
            byte_offset: 100,
            last_seen_size: 100,
            mtime: None,
            start_byte_offset: 0,
            prefix_hash: String::new(),
        },
    )
    .unwrap();
    let ready = f.root.join("ready");
    let release = f.root.join("release");
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let availability = Command::new(env!("CARGO_BIN_EXE_ne"))
        .args(["agent", "list"])
        .env("NOENDING_HOME", &f.home.root)
        .env("PATH", std::env::join_paths(&paths).unwrap())
        .output()
        .unwrap();
    assert!(
        availability.status.success(),
        "{}",
        String::from_utf8_lossy(&availability.stderr)
    );
    let availability: Value = serde_json::from_slice(&availability.stdout).unwrap();
    let codex = availability["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["agent"] == "codex")
        .unwrap();
    assert_eq!(codex["cli_available"], true);
    assert_eq!(codex["supports_extraction"], true);
    assert_eq!(codex["executable_path"], agent.to_str().unwrap());
    assert_eq!(codex["version"], "codex-cli protocol-test");
    assert!(
        !ready.exists(),
        "availability query must not start model extraction"
    );
    let (success, configured) = f.run(
        Path::new(env!("CARGO_BIN_EXE_ne")),
        &[
            "summary",
            "config",
            "set",
            "--agent",
            "codex",
            "--model",
            "protocol-model",
            "--effort",
            "high",
        ],
    );
    assert!(success, "{configured}");
    let (_, configuration) = f.run(
        Path::new(env!("CARGO_BIN_EXE_ne")),
        &["summary", "config", "show"],
    );
    assert_eq!(
        configuration["data"]["runtime"],
        serde_json::json!({"model":"protocol-model","provider":null,"effort":"high"})
    );
    assert!(
        !ready.exists(),
        "saving extraction configuration must not invoke the Agent"
    );
    let spawn = |action: &str, request: &str| {
        Command::new(env!("CARGO_BIN_EXE_ne"))
            .args(["summary", action, "session", &sid, "--request-id", request])
            .env("NOENDING_HOME", &f.home.root)
            .env("PATH", std::env::join_paths(&paths).unwrap())
            .env("NE_TEST_READY", &ready)
            .env("NE_TEST_RELEASE", &release)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    };
    let mut child = spawn("update", "summary-protocol");
    struct ChildGuard<'a>(&'a mut std::process::Child);
    impl Drop for ChildGuard<'_> {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let guard = ChildGuard(&mut child);
    let started = Instant::now();
    while !ready.exists() {
        assert!(
            guard.0.try_wait().unwrap().is_none(),
            "CLI exited before its Agent was ready"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "Agent failed to reach the test barrier"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(db.get_session_context(&sid).unwrap().is_none());
    let exe = Path::new(env!("CARGO_BIN_EXE_ne"));
    let (_, running) = f.run(exe, &["request", "show", "summary-protocol"]);
    assert_eq!(running["data"]["status"], "running");
    let lock = ExtractionFiles::create(
        &f.home.runtime_dir.join("context-extraction"),
        "task-another",
        std::slice::from_ref(&sid),
    );
    assert!(
        matches!(lock, Err(noending::error::AppError::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
    let migration = noending::workspace::home::PendingMigration {
        pointer_path: f.root.join("home.json"),
        from: f.home.root.clone(),
        to: f.root.join("relocated"),
    };
    assert!(noending::workspace::home::migrate_data_root(&migration).is_err());
    assert!(f.home.db_path.is_file());
    assert!(!migration.to.join("data/noending.db").exists());
    std::fs::write(&release, "continue").unwrap();
    let started = Instant::now();
    while guard.0.try_wait().unwrap().is_none() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "CLI failed to finish after releasing its Agent"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    // Drop only after successful exit; wait_with_output still reads its pipes.
    drop(guard);
    let output = child.wait_with_output().unwrap();
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        output.status.success(),
        "{response}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(response["data"]["status"], "updated");
    assert_eq!(response["data"]["revision"], 1);
    let (_, summary) = f.run(exe, &["session", "summary", &sid]);
    assert_eq!(
        summary["data"]["summary"]["items"][0]["title"],
        "Confirmed memory"
    );
    assert_eq!(summary["data"]["pending"], false);
    let acquired = ExtractionFiles::create(
        &f.home.runtime_dir.join("context-extraction"),
        "task-another",
        std::slice::from_ref(&sid),
    )
    .unwrap();
    drop(acquired);
    let (_, completed) = f.run(exe, &["request", "show", "summary-protocol"]);
    assert_eq!(completed["data"]["status"], "completed");
    assert_eq!(completed["data"]["response"]["data"]["revision"], 1);
    for (signal, request, status) in [
        ("-INT", "cancelled-summary", "completed"),
        ("-KILL", "interrupted-summary", "interrupted"),
    ] {
        std::fs::remove_file(&ready).unwrap();
        std::fs::remove_file(&release).unwrap();
        let mut child = spawn("regenerate", request);
        let guard = ChildGuard(&mut child);
        let started = Instant::now();
        while !ready.exists() {
            assert!(guard.0.try_wait().unwrap().is_none());
            assert!(started.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(20));
        }
        let pid = std::fs::read_to_string(&ready).unwrap();
        assert!(Command::new("kill")
            .args([signal, &guard.0.id().to_string()])
            .status()
            .unwrap()
            .success());
        let started = Instant::now();
        while guard.0.try_wait().unwrap().is_none() {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "cancelled CLI did not exit"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(guard);
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        if signal == "-INT" {
            let result: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(result["error"]["code"], "cancelled");
            assert!(
                !Command::new("kill")
                    .args(["-0", &pid])
                    .stderr(std::process::Stdio::null())
                    .status()
                    .unwrap()
                    .success(),
                "cancelled command left its Agent running"
            );
        } else {
            // SIGKILL cannot run cleanup; explicitly stop this test's orphan stub.
            let _ = Command::new("kill")
                .args(["-KILL", &pid])
                .stderr(std::process::Stdio::null())
                .status();
        }
        let (_, receipt) = f.run(exe, &["request", "show", request]);
        assert_eq!(receipt["data"]["status"], status);
        let (success, retry) = f.run(
            exe,
            &[
                "summary",
                "regenerate",
                "session",
                &sid,
                "--request-id",
                request,
            ],
        );
        assert!(!success);
        assert_eq!(
            retry["error"]["code"],
            if signal == "-INT" {
                "cancelled"
            } else {
                "request_interrupted"
            }
        );
        let (_, preserved) = f.run(exe, &["session", "summary", &sid]);
        assert_eq!(preserved["data"]["revision"], 1);
        assert_eq!(
            preserved["data"]["summary"]["items"][0]["title"],
            "Confirmed memory"
        );
        drop(
            ExtractionFiles::create(
                &f.home.runtime_dir.join("context-extraction"),
                "task-after-cancellation",
                std::slice::from_ref(&sid),
            )
            .unwrap(),
        );
        std::fs::write(&release, "next phase").unwrap();
    }
}

#[test]
fn project_resolution_uses_cwd_and_nearest_registered_path_without_creating_records() {
    let f = Fixture::new();
    let exe = Path::new(env!("CARGO_BIN_EXE_ne"));
    let outer = f.root.join("outer");
    let inner = outer.join("inner");
    let nested = inner.join("src/components");
    std::fs::create_dir_all(&nested).unwrap();
    let (success, first) = f.run(
        exe,
        &["path", "register", "--path", outer.to_str().unwrap()],
    );
    assert!(success, "{first}");
    let (success, second) = f.run(
        exe,
        &["path", "register", "--path", inner.to_str().unwrap()],
    );
    assert!(success, "{second}");
    let pid = second["data"]["project"]["id"].as_str().unwrap();
    let (success, exact) = f.run(
        exe,
        &["project", "resolve", "--path", inner.to_str().unwrap()],
    );
    assert!(success, "{exact}");
    assert_eq!(exact["data"]["project"]["id"], pid);
    assert_eq!(exact["data"]["matched_by"], "exact_path");
    let output = Command::new(exe)
        .args(["project", "resolve"])
        .current_dir(&nested)
        .env("NOENDING_HOME", &f.home.root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        result["data"]["query_path"],
        noending::workspace::normalize_path(
            &std::fs::canonicalize(&nested).unwrap().to_string_lossy()
        )
        .unwrap()
    );
    assert_eq!(result["data"]["project"]["id"], pid);
    assert_eq!(result["data"]["matched_path"], inner.to_str().unwrap());
    assert_eq!(result["data"]["matched_by"], "ancestor_path");
    let (success, _) = f.run(exe, &["project", "archive", pid]);
    assert!(success);
    let (_, archived) = f.run(
        exe,
        &["project", "resolve", "--path", nested.to_str().unwrap()],
    );
    assert_eq!(archived["data"]["project"]["visibility"], "archived");
    let (_, unchanged) = f.run(exe, &["project", "show", pid]);
    assert_eq!(unchanged["data"]["paths"].as_array().unwrap().len(), 1);
    assert_eq!(
        unchanged["data"]["paths"][0]["canonical_path"],
        inner.to_str().unwrap()
    );
    let (_, unknown) = f.run(
        exe,
        &["project", "resolve", "--path", f.root.to_str().unwrap()],
    );
    assert_eq!(unknown["data"]["found"], false);
    let (success, reserved) = f.run(
        exe,
        &[
            "project",
            "resolve",
            "--path",
            f.home.data_dir.to_str().unwrap(),
        ],
    );
    assert!(success);
    assert_eq!(reserved["data"]["found"], false);
    assert_eq!(reserved["data"]["reason"], "reserved");
    let (success, missing) = f.run(
        exe,
        &[
            "project",
            "resolve",
            "--path",
            f.root.join("missing").to_str().unwrap(),
        ],
    );
    assert!(!success);
    assert_eq!(missing["error"]["code"], "invalid_arguments");
    let (_, projects) = f.run(exe, &["project", "list", "--scope", "all"]);
    assert_eq!(projects["data"]["items"].as_array().unwrap().len(), 2);
    #[cfg(unix)]
    {
        let alias = f.root.join("registered-alias");
        let query = f.root.join("query-alias");
        std::os::unix::fs::symlink(&inner, &alias).unwrap();
        std::os::unix::fs::symlink(&inner, &query).unwrap();
        assert!(
            f.run(
                exe,
                &["path", "register", "--path", alias.to_str().unwrap()]
            )
            .0
        );
        let (_, ambiguous) = f.run(
            exe,
            &["project", "resolve", "--path", query.to_str().unwrap()],
        );
        assert_eq!(ambiguous["data"]["found"], false);
        assert_eq!(ambiguous["data"]["reason"], "ambiguous_paths");
        let (_, explicit) = f.run(
            exe,
            &["project", "resolve", "--path", inner.to_str().unwrap()],
        );
        assert_eq!(explicit["data"]["project"]["id"], pid);
    }
}

/// Git is an explicit prerequisite; repository/worktree identity is checked
/// against real local Git state without authentication or a network remote.
#[test]
fn project_resolution_stops_at_unknown_repositories_and_recognizes_new_worktrees() {
    let f = Fixture::new();
    let exe = Path::new(env!("CARGO_BIN_EXE_ne"));
    let git = noending::platform::exec_resolver::resolve_executable("git")
        .expect("Git is required for repository boundary tests");
    let run_git = |cwd: &Path, args: &[&str]| {
        let output = Command::new(&git)
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    let parent = f.root.join("parent");
    let repository = parent.join("repo");
    let deep = repository.join("src");
    std::fs::create_dir_all(&deep).unwrap();
    assert!(
        f.run(
            exe,
            &["path", "register", "--path", parent.to_str().unwrap()]
        )
        .0
    );
    run_git(&repository, &["init"]);
    let (success, blocked) = f.run(
        exe,
        &["project", "resolve", "--path", deep.to_str().unwrap()],
    );
    assert!(success, "{blocked}");
    assert_eq!(blocked["data"]["found"], false);
    assert_eq!(blocked["data"]["reason"], "unregistered_repository");
    let (success, registered) = f.run(
        exe,
        &["path", "register", "--path", repository.to_str().unwrap()],
    );
    assert!(success, "{registered}");
    let pid = registered["data"]["project"]["id"].as_str().unwrap();
    run_git(
        &repository,
        &[
            "-c",
            "user.name=NoEnding Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "Fixture",
        ],
    );
    let worktree = f.root.join("unregistered-worktree");
    run_git(
        &repository,
        &["worktree", "add", "--detach", worktree.to_str().unwrap()],
    );
    let child = worktree.join("src");
    std::fs::create_dir_all(&child).unwrap();
    let (_, before_lookup) = f.run(exe, &["project", "show", pid]);
    let (success, resolved) = f.run(
        exe,
        &["project", "resolve", "--path", child.to_str().unwrap()],
    );
    assert!(success, "{resolved}");
    assert_eq!(resolved["data"]["found"], true);
    assert_eq!(resolved["data"]["project"]["id"], pid);
    assert_eq!(resolved["data"]["matched_by"], "git_family");
    assert_eq!(resolved["data"]["matched_path"], worktree.to_str().unwrap());
    let (_, unchanged) = f.run(exe, &["project", "show", pid]);
    assert_eq!(unchanged["data"]["paths"], before_lookup["data"]["paths"]);
    assert!(unchanged["data"]["paths"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["canonical_path"] == repository.to_str().unwrap()));
    assert!(!unchanged["data"]["paths"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["canonical_path"] == worktree.to_str().unwrap()));
    // A broken .git entry is still a boundary and cannot inherit its parent.
    let broken = parent.join("broken-repo");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join(".git"), "gitdir: missing-git-dir\n").unwrap();
    let (success, unresolved) = f.run(
        exe,
        &["project", "resolve", "--path", broken.to_str().unwrap()],
    );
    assert!(success, "{unresolved}");
    assert_eq!(unresolved["data"]["found"], false);
}

#[test]
fn session_control_uses_the_matching_desktop_and_replays_write_receipts() {
    use noending::session_control::{ControlCommand, DesktopHost};
    use serde_json::json;
    let f = Fixture::new();
    let exe = Path::new(env!("CARGO_BIN_EXE_ne"));
    let (ok, missing) = f.run(exe, &["session", "terminals"]);
    assert!(!ok);
    assert_eq!(missing["error"]["code"], "desktop_unavailable");
    // This fixture stands in for desktop business dispatch; process arguments,
    // Home routing and durable exactly-once launch receipts cross real IPC.
    let (submitted_tx, submitted_rx) = std::sync::mpsc::channel();
    let host = DesktopHost::start(&f.home, move |command, _sender_terminal| match command {
        ControlCommand::Identity { claimed } => Ok(json!({"session_id":claimed})),
        ControlCommand::List { session } => Ok(json!({"items":[],"session":session})),
        ControlCommand::Start(options) => {
            let terminal_id = noending::storage::new_id();
            if options.message.as_deref() == Some("disconnect fixture") {
                submitted_tx.send(terminal_id.clone()).unwrap();
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Ok(json!({"terminal_id":terminal_id,"options":options}))
        }
    })
    .unwrap();
    assert!(DesktopHost::start(&f.home, |_, _| Ok(json!({}))).is_err());
    let (ok, list) = f.run(exe, &["session", "terminals", "--session", "session-a"]);
    assert!(ok, "{list}");
    assert_eq!(list["data"]["session"], "session-a");
    let (ok, literal_option) = f.run(
        exe,
        &[
            "session",
            "new",
            "--agent",
            "codex",
            "--message",
            "--model is message text",
        ],
    );
    assert!(ok, "{literal_option}");
    assert_eq!(
        literal_option["data"]["options"]["message"],
        "--model is message text"
    );
    assert!(literal_option["data"]["options"]["runtime"].is_null());
    let file = f.root.join("消息.txt");
    let literal = "中文😀\n$(echo inert) `text` 'quotes' --option";
    std::fs::write(&file, literal).unwrap();
    let args = [
        "session",
        "new",
        "--agent",
        "codex",
        "--cwd",
        f.root.to_str().unwrap(),
        "--task",
        "a",
        "--task",
        "b",
        "--model",
        "test-model",
        "--file",
        file.to_str().unwrap(),
        "--request-id",
        "start-once",
    ];
    let (ok, first) = f.run(exe, &args);
    assert!(ok, "{first}");
    assert_eq!(first["data"]["options"]["message"], literal);
    assert_eq!(first["data"]["options"]["tasks"], json!(["a", "b"]));
    assert_eq!(first["data"]["options"]["runtime"]["model"], "test-model");
    let (ok, replay) = f.run(Path::new(env!("CARGO_BIN_EXE_noending")), &args);
    assert!(ok, "{replay}");
    assert_eq!(first["data"]["terminal_id"], replay["data"]["terminal_id"]);
    let (ok, receipt) = f.run(exe, &["request", "show", "start-once"]);
    assert!(ok, "{receipt}");
    assert_eq!(
        receipt["data"]["response"]["data"]["terminal_id"],
        first["data"]["terminal_id"]
    );
    let disconnect_args = [
        "session",
        "new",
        "--agent",
        "codex",
        "--message",
        "disconnect fixture",
        "--request-id",
        "disconnected-launch",
    ];
    let mut disconnected = Command::new(exe)
        .args(disconnect_args)
        .env("NOENDING_HOME", &f.home.root)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let launched_id = submitted_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    disconnected.kill().unwrap();
    disconnected.wait().unwrap();
    let (ok, recovered) = f.run(exe, &disconnect_args);
    assert!(ok, "{recovered}");
    assert_eq!(
        recovered["data"]["terminal_id"], launched_id,
        "disconnecting the client must not repeat the desktop action"
    );
    let other = Fixture::new();
    let (ok, wrong_home) = other.run(exe, &["session", "terminals"]);
    assert!(!ok);
    assert_eq!(wrong_home["error"]["code"], "desktop_unavailable");
    drop(host);
    let (ok, closed) = f.run(exe, &["session", "terminals"]);
    assert!(!ok);
    assert_eq!(closed["error"]["code"], "desktop_unavailable");
}

#[test]
fn mailbox_cli_persists_without_desktop_requires_session_sender_and_uses_current_revision() {
    let f = Fixture::new();
    let db = f.db.as_ref().unwrap();
    db.write().execute_batch("INSERT INTO sessions(id,agent,root_agent_session_id,source_kind,source_path) VALUES('sender','codex','sender','jsonl','/sender'),('target','codex','target','jsonl','/target')").unwrap();
    let exe = Path::new(env!("CARGO_BIN_EXE_ne"));
    let (ok, err) = f.run(
        exe,
        &[
            "mailbox",
            "send",
            "--session",
            "target",
            "--message",
            "text",
        ],
    );
    assert!(!ok);
    assert_eq!(err["error"]["code"], "sender_unavailable");
    let args = [
        "mailbox",
        "send",
        "--session",
        "target",
        "--from-session",
        "sender",
        "--message",
        "中文 body",
        "--request-id",
        "mail-once",
        "--notify-immediately",
        "true",
    ];
    let (ok, sent) = f.run(exe, &args);
    assert!(ok, "{sent}");
    let (ok, replay) = f.run(exe, &args);
    assert!(ok, "{replay}");
    assert_eq!(sent["data"]["message_id"], replay["data"]["message_id"]);
    let mid = sent["data"]["message_id"].as_str().unwrap();
    let (ok, read) = f.run(
        exe,
        &[
            "mailbox",
            "read",
            "--session",
            "target",
            "--message-id",
            mid,
        ],
    );
    assert!(ok, "{read}");
    assert_eq!(read["data"]["body"], "中文 body");
    assert_eq!(read["data"]["notify_immediately"], true);
    let (ok, ordinary) = f.run(
        exe,
        &[
            "mailbox",
            "send",
            "--session",
            "target",
            "--from-session",
            "sender",
            "--message",
            "ordinary",
        ],
    );
    assert!(ok, "{ordinary}");
    assert!(
        !db.mailbox_read("target", ordinary["data"]["message_id"].as_str().unwrap())
            .unwrap()
            .notify_immediately
    );
    let ordinary_id = ordinary["data"]["message_id"].as_str().unwrap();
    db.mailbox_mark("target", ordinary_id, 1, true).unwrap();
    let (ok, invalid) = f.run(
        exe,
        &[
            "mailbox",
            "send",
            "--session",
            "target",
            "--from-session",
            "sender",
            "--message",
            "invalid",
            "--notify-immediately",
            "yes",
        ],
    );
    assert!(!ok, "{invalid}");
    assert!(read["data"]["read_at"].is_null());
    let (ok, denied) = f.run(
        exe,
        &[
            "mailbox",
            "mark",
            "--session",
            "target",
            "--message-id",
            mid,
            "--revision",
            "1",
            "--state",
            "read",
            "--from-session",
            "sender",
        ],
    );
    assert!(!ok);
    assert_eq!(denied["error"]["code"], "identity_mismatch");
    let (ok, marked) = f.run(
        exe,
        &[
            "mailbox",
            "mark",
            "--session",
            "target",
            "--message-id",
            mid,
            "--revision",
            "1",
            "--state",
            "read",
            "--from-session",
            "target",
        ],
    );
    assert!(ok, "{marked}");
    let (ok, page) = f.run(exe, &["mailbox", "list", "--session", "target"]);
    assert!(ok, "{page}");
    assert_eq!(page["data"]["unread"], 0);
    assert_eq!(
        db.mailbox_list("target", "all", None, 50)
            .unwrap()
            .items
            .len(),
        2
    );
}
