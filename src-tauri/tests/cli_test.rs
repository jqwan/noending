//! Exercise real console/desktop process entries, JSON, IPC and installed names.
use noending::{
    cli,
    storage::Db,
    workspace::{home::NoEndingHome, wiring::WorkspaceLayer},
};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

struct Fixture {
    root: PathBuf,
    home: NoEndingHome,
    server: Option<cli::Server>,
    db: Option<Arc<Db>>,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "noending-cli-process-{}",
            noending::storage::new_id()
        ));
        let home = NoEndingHome::new(root.join("home").to_str().unwrap(), None).unwrap();
        home.ensure_dirs().unwrap();
        let db = Arc::new(Db::open(&home.db_path).unwrap());
        let layer = Arc::new(WorkspaceLayer::new(&home));
        let server = cli::start(db.clone(), home.clone(), layer, || {}).unwrap();
        Self {
            root,
            home,
            server: Some(server),
            db: Some(db),
        }
    }
    fn run(&self, exe: &Path, args: &[&str]) -> (bool, Value) {
        let output = Command::new(exe)
            .args(args)
            .env("NOENDING_HOME", &self.home.root)
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
        drop(self.server.take());
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
            "preference",
            "add",
            "--scope",
            "user",
            "--content",
            content,
            "--request-id",
            "pref-process",
        ],
    );
    assert_eq!(added["data"]["content"], content);
    let (success, repeated) = f.run(
        Path::new(env!("CARGO_BIN_EXE_noending")),
        &[
            "preference",
            "add",
            "--scope",
            "user",
            "--content",
            content,
            "--request-id",
            "pref-process",
        ],
    );
    assert!(success);
    assert_eq!(repeated["data"]["id"], added["data"]["id"]);
    let (success, receipt) = f.run(
        Path::new(env!("CARGO_BIN_EXE_ne")),
        &["request", "show", "pref-process"],
    );
    assert!(success);
    assert_eq!(
        receipt["data"]["response"]["data"]["id"],
        added["data"]["id"]
    );
    assert_eq!(
        noending::storage::preferences::list_items(f.db.as_ref().unwrap(), None)
            .unwrap()
            .len(),
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
    let missing = f.root.join("not-installed");
    let (success, status) = f.run(
        Path::new(env!("CARGO_BIN_EXE_ne")),
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
        Path::new(env!("CARGO_BIN_EXE_ne")),
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
        Path::new(env!("CARGO_BIN_EXE_ne")),
        &["cli", "install", "--dir", directory.to_str().unwrap()],
    );
    assert!(!success);
    assert_eq!(error["error"]["code"], "already_exists");
    let (success, status) = f.run(
        Path::new(env!("CARGO_BIN_EXE_ne")),
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
    let outdated = original.replace(env!("CARGO_BIN_EXE_ne"), "/old/app/ne");
    assert_ne!(original, outdated);
    std::fs::write(&launcher, outdated).unwrap();
    let (success, _) = f.run(
        Path::new(env!("CARGO_BIN_EXE_ne")),
        &["cli", "update", "--dir", directory.to_str().unwrap()],
    );
    assert!(success);
    assert_eq!(std::fs::read_to_string(&launcher).unwrap(), original);
    std::fs::write(&launcher, "unrelated command").unwrap();
    for action in ["update", "uninstall"] {
        let (success, error) = f.run(
            Path::new(env!("CARGO_BIN_EXE_ne")),
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
                Path::new(env!("CARGO_BIN_EXE_ne")),
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
        Path::new(env!("CARGO_BIN_EXE_ne")),
        &["cli", "uninstall", "--dir", directory.to_str().unwrap()],
    );
    assert!(success);
    assert!(!launcher.exists());
    f.server.as_ref().unwrap().shutdown();
    let (success, error) = f.run(Path::new(env!("CARGO_BIN_EXE_ne")), &["status"]);
    assert!(!success);
    assert_eq!(error["error"]["code"], "app_unavailable");
}

#[test]
fn assistant_launchers_use_their_explicit_home_connection_and_literal_arguments() {
    let f = Fixture::new();
    let directory = f.root.join("assistant's tools with spaces");
    cli::install_workspace_entry(
        &directory,
        &f.home.runtime_dir.join("cli.json"),
        Path::new(env!("CARGO_BIN_EXE_ne")),
    )
    .unwrap();
    // Batch files cannot carry CR/LF arguments; the native entries cover those above.
    let content = "literal $() `text` 中文 ' quotes";
    for name in ["ne", "noending"] {
        let output = installed_command(&directory, name)
            .args([
                "preference",
                "add",
                "--scope",
                "user",
                "--content",
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
        assert_eq!(result["data"]["content"], content);
    }
    assert_eq!(
        noending::storage::preferences::list_items(f.db.as_ref().unwrap(), None)
            .unwrap()
            .len(),
        1
    );
}
