//! Real stdio process boundary: no desktop app, no model or external account.
use noending_lib::{domain::Agent, storage::Db, workspace::home::NoEndingHome};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

struct Fixture {
    root: PathBuf,
    home: NoEndingHome,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("noending-mcp-{}", noending_lib::storage::new_id()));
        let home = NoEndingHome::new(root.join("数据 home").to_str().unwrap(), None).unwrap();
        home.ensure_dirs().unwrap();
        Self { root, home }
    }
    fn command(&self, exe: &str) -> Command {
        let mut command = Command::new(exe);
        command
            .args(["--home", self.home.root.to_str().unwrap(), "mcp", "serve"])
            .env("NOENDING_HOME", self.root.join("wrong-home"));
        command
    }
    fn start(&self) -> Client {
        Client::start(self.command(env!("CARGO_BIN_EXE_ne")))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct Client {
    child: Child,
    input: Option<ChildStdin>,
    output: Receiver<Value>,
    pending: HashMap<Value, Value>,
    serial: u32,
}
impl Client {
    fn start(mut command: Command) -> Self {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (tx, output) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = line.unwrap();
                let value = serde_json::from_str(&line)
                    .unwrap_or_else(|error| panic!("Non-protocol stdout: {error}: {line}"));
                if tx.send(value).is_err() {
                    break;
                }
            }
        });
        let mut client = Self {
            child,
            input,
            output,
            pending: HashMap::new(),
            serial: 0,
        };
        let initialized = client.request(
            "initialize",
            json!({
                "protocolVersion":"2025-11-25", "capabilities":{},
                "clientInfo":{"name":"noending-integration-test","version":"1"}
            }),
        );
        assert_eq!(
            initialized["result"]["protocolVersion"], "2025-11-25",
            "{initialized}"
        );
        assert_eq!(initialized["result"]["serverInfo"]["name"], "noending");
        client.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        client
    }
    fn send(&mut self, value: Value) {
        writeln!(self.input.as_mut().unwrap(), "{value}").unwrap();
        self.input.as_mut().unwrap().flush().unwrap();
    }
    fn begin(&mut self, method: &str, params: Value) -> Value {
        self.serial += 1;
        let id = json!(self.serial);
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        id
    }
    fn response(&mut self, id: Value) -> Value {
        if let Some(value) = self.pending.remove(&id) {
            return value;
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let value = self
                .output
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("MCP response timed out or stdout closed");
            if value["id"] == id {
                return value;
            }
            if value.get("id").is_some() {
                self.pending.insert(value["id"].clone(), value);
            }
        }
    }
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.begin(method, params);
        self.response(id)
    }
    fn call(&mut self, name: &str, arguments: Value) -> Value {
        let response = self.request("tools/call", json!({"name":name,"arguments":arguments}));
        assert!(response.get("error").is_none(), "{response}");
        let result = &response["result"];
        let content = result["structuredContent"].clone();
        assert_eq!(
            result["isError"],
            !content["ok"].as_bool().unwrap(),
            "{response}"
        );
        let text: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text, content);
        content
    }
    fn ok(&mut self, name: &str, arguments: Value) -> Value {
        let value = self.call(name, arguments);
        assert_eq!(value["ok"], true, "{name}: {value}");
        value["data"].clone()
    }
    fn finish(&mut self) {
        drop(self.input.take());
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "MCP exited with {status}");
                break;
            }
            assert!(
                Instant::now() < deadline,
                "MCP did not exit after stdin closed"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        while let Ok(value) = self.output.try_recv() {
            assert!(
                value.get("jsonrpc").is_some(),
                "Unexpected trailing output: {value}"
            );
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn stdio_catalog_and_both_entries_work_without_desktop_or_terminal_controls() {
    let f = Fixture::new();
    for exe in [env!("CARGO_BIN_EXE_ne"), env!("CARGO_BIN_EXE_noending")] {
        let mut client = Client::start(f.command(exe));
        let catalog = client.request("tools/list", json!({}));
        let tools = catalog["result"]["tools"].as_array().unwrap();
        for name in [
            "status",
            "agent_list",
            "project_resolve",
            "session_messages",
            "summary_search",
            "task_create",
            "sync",
            "summary_config_set",
        ] {
            assert!(
                tools.iter().any(|tool| tool["name"] == name),
                "Missing {name}"
            );
        }
        assert!(!tools
            .iter()
            .any(|tool| tool["name"].as_str().unwrap().starts_with("terminal_")));
        let resolve = tools
            .iter()
            .find(|tool| tool["name"] == "project_resolve")
            .unwrap();
        assert_eq!(resolve["inputSchema"]["required"], json!(["path"]));
        let add = tools
            .iter()
            .find(|tool| tool["name"] == "task_create")
            .unwrap();
        assert_eq!(add["annotations"]["readOnlyHint"], false);
        assert_eq!(
            add["inputSchema"]["properties"]["request_id"]["type"],
            "string"
        );
        let status = client.ok("status", json!({}));
        assert_eq!(status["home"], f.home.root.to_str().unwrap());
        assert_eq!(status["connected"], true);
        assert_eq!(
            client.call("project_resolve", json!({}))["error"]["code"],
            "invalid_arguments"
        );
        assert_eq!(
            client.call("status", json!({"request_id":"read-id"}))["error"]["code"],
            "invalid_arguments"
        );
        let unknown = client.request("tools/call", json!({"name":"terminal_send","arguments":{}}));
        assert!(unknown.get("error").is_some(), "{unknown}");
        assert_eq!(
            client.ok("object_list", json!({"kind":"session"}))["items"],
            json!([])
        );
        client.finish();
    }
    assert!(!f.root.join("wrong-home").exists());
    // Startup failures must stay on stderr, with no one-shot CLI JSON on stdout.
    let bad_home = f.root.join("not-a-directory");
    std::fs::write(&bad_home, "file").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ne"))
        .args(["mcp", "serve", "--home", bad_home.to_str().unwrap()])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

#[test]
fn mcp_writes_persist_and_share_receipts_with_cli_while_enforcing_revisions_and_arguments() {
    let f = Fixture::new();
    let project_dir = f.root.join("project");
    std::fs::create_dir_all(project_dir.join("src")).unwrap();
    let mut client = f.start();
    let project = client.ok(
        "path_register",
        json!({"path":project_dir,"request_id":"register-project"}),
    )["project"]["id"]
        .clone();
    assert_eq!(
        client.ok("project_resolve", json!({"path":project_dir.join("src")}))["project"]["id"],
        project
    );
    let task_args = json!({"title":"MCP task","description":"Persisted through shared service","projects":[project],"request_id":"create-task"});
    let task = client.ok("task_create", task_args.clone())["task"]["id"].clone();
    assert_eq!(
        client.ok("task_create", task_args.clone())["task"]["id"],
        task
    );
    assert_eq!(
        client.call(
            "task_create",
            json!({"title":"Different task","request_id":"create-task"})
        )["error"]["code"],
        "request_conflict"
    );
    assert_eq!(
        client.call("task_update", json!({"id":task}))["error"]["code"],
        "invalid_arguments"
    );
    assert_eq!(
        client.call("object_list", json!({"kind":"task","limit":0}))["error"]["code"],
        "invalid_arguments"
    );
    assert_eq!(
        client.call("object_list", json!({"kind":"task","unexpected":true}))["error"]["code"],
        "invalid_arguments"
    );
    assert_eq!(
        client.call("sync", json!({"source":"source","session":"session"}))["error"]["code"],
        "invalid_arguments"
    );
    let mut summary = json!({"items":[{"title":"Protocol memory","description":"Persistent summary","title_locked":false,"details":["Preserve message evidence"]}]});
    client.ok(
        "summary_set",
        json!({"kind":"task","id":task,"revision":0,"content":summary,"request_id":"set-summary"}),
    );
    assert_eq!(client.call("summary_set", json!({"kind":"task","id":task,"revision":0,"content":{"items":[{"title":"Stale edit","description":"","title_locked":false,"details":[]}]}}))["error"]["code"], "stale_result");
    assert_eq!(
        client.ok(
            "summary_get",
            json!({"kind":"task","id":task,"item":1,"revision":1})
        )["item"]["title"],
        "Protocol memory"
    );
    assert_eq!(
        client.ok("summary_search", json!({"query":"Protocol","kind":"task"}))["items"][0]["id"],
        task
    );
    let db = Db::open(&f.home.db_path).unwrap();
    noending_lib::context::set_summary_title_lock(&db, "task", task.as_str().unwrap(), 1, 1, true)
        .unwrap();
    drop(db);
    assert_eq!(
        client.call(
            "summary_set",
            json!({"kind":"task","id":task,"revision":2,"content":summary})
        )["ok"],
        false
    );
    summary["items"][0]["title_locked"] = json!(true);
    summary["items"][0]["description"] = json!("Updated body with a protected title");
    client.ok(
        "summary_set",
        json!({"kind":"task","id":task,"revision":2,"content":summary}),
    );
    client.ok(
        "summary_config_set",
        json!({"agent":"pi","provider":"local","model":"test-model"}),
    );
    client.finish();
    let mut next = f.start();
    assert_eq!(next.ok("task_create", task_args)["task"]["id"], task);
    assert_eq!(
        next.ok("summary_get", json!({"kind":"task","id":task}))["summary"],
        summary
    );
    assert_eq!(
        next.ok("summary_config_get", json!({}))["runtime"]["model"],
        "test-model"
    );
    next.ok(
        "object_archive",
        json!({"kind":"task","id":task,"archived":true}),
    );
    assert_eq!(
        next.ok("object_list", json!({"kind":"task"}))["items"],
        json!([])
    );
    assert_eq!(
        next.ok("object_list", json!({"kind":"task","scope":"archived"}))["items"][0]["id"],
        task
    );
    next.finish();
    let receipt = Command::new(env!("CARGO_BIN_EXE_ne"))
        .args(["request", "show", "create-task"])
        .env("NOENDING_HOME", &f.home.root)
        .output()
        .unwrap();
    assert!(receipt.status.success());
    let receipt: Value = serde_json::from_slice(&receipt.stdout).unwrap();
    assert_eq!(receipt["data"]["response"]["data"]["task"]["id"], task);
}

#[test]
fn source_queries_do_not_scan_and_explicit_sync_exposes_paginated_history_and_evidence() {
    let f = Fixture::new();
    let source_dir = f.root.join("source");
    let cwd = f.root.join("project");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    let meta = json!({"type":"session_meta","payload":{"id":"mcp-native-session","cwd":cwd}});
    let message = |id: &str, text: &str| json!({"type":"response_item","payload":{"type":"message","id":id,"role":"user","content":[{"type":"input_text","text":text}]}});
    let transcript = source_dir.join("rollout-test.jsonl");
    std::fs::write(
        &transcript,
        format!(
            "{meta}\n{}\n{}\n",
            message("m1", "First evidence"),
            message("m2", "Second evidence")
        ),
    )
    .unwrap();
    let db = Db::open(&f.home.db_path).unwrap();
    let source = db
        .add_ingest_source(Agent::Codex, source_dir.to_str().unwrap())
        .unwrap();
    drop(db);
    let mut client = f.start();
    assert!(client.ok("source_list", json!({}))["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["id"] == source.id));
    assert_eq!(
        client.ok("object_list", json!({"kind":"session"}))["items"],
        json!([])
    );
    client.ok("sync", json!({"source":source.id}));
    let sessions = client.ok("object_list", json!({"kind":"session"}));
    let id = sessions["items"][0]["id"].clone();
    let task = client.ok(
        "task_create",
        json!({"title":"Evidence task","projects":[sessions["items"][0]["project_id"]]}),
    )["task"]["id"]
        .clone();
    client.ok(
        "task_session_set",
        json!({"task":task,"session":id,"tracked":true}),
    );
    assert_eq!(
        client.ok("object_list", json!({"kind":"session","task":task}))["items"][0]["id"],
        id
    );
    let first = client.ok("session_messages", json!({"id":id,"limit":1}));
    assert_eq!(first["records"][0]["record"]["content"], "First evidence");
    assert_eq!(first["end"], false);
    let second = client.ok(
        "session_messages",
        json!({"id":id,"limit":1,"cursor":first["next_cursor"]}),
    );
    assert_eq!(second["records"][0]["record"]["content"], "Second evidence");
    assert_eq!(second["end"], true);
    let hits = client.ok("session_search", json!({"query":"Second evidence"}));
    assert_eq!(hits["items"][0]["session_id"], id);
    let snippet = client.ok("session_messages", json!({"id":id,"query":"Second"}));
    assert_eq!(
        snippet["records"][0]["record"]["content"],
        "Second evidence"
    );
    client.ok(
        "task_session_set",
        json!({"task":task,"session":id,"tracked":false}),
    );
    assert_eq!(
        client.ok("object_list", json!({"kind":"session","task":task}))["items"],
        json!([])
    );
    client.finish();
}

/// Python 3 is required for the real extraction subprocess protocol stub.
#[cfg(unix)]
#[test]
fn cancelling_one_extraction_preserves_connection_and_eof_cleans_up_active_agent() {
    use noending_lib::domain::{ParsedSessionMessage, SessionMessageRole, SourceCursorUpdate};
    use std::os::unix::fs::PermissionsExt;
    assert!(Command::new("python3")
        .arg("--version")
        .status()
        .expect("Python 3 is required")
        .success());
    let f = Fixture::new();
    let bin = f.root.join("agent-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let agent = bin.join("codex");
    std::fs::write(&agent, include_str!("fixtures/cli-agent.py")).unwrap();
    std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o700)).unwrap();
    let db = Db::open(&f.home.db_path).unwrap();
    let id = db
        .upsert_logical_session(
            Agent::Codex,
            "mcp-extraction",
            None,
            Some("MCP extraction"),
            None,
            None,
            None,
            None,
            None,
            "test",
            "/not/a/source",
            &json!({}),
        )
        .unwrap()
        .0;
    db.commit_ingest(
        &id,
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
    drop(db);
    let ready = f.root.join("ready");
    let release = f.root.join("release");
    let mut command = f.command(env!("CARGO_BIN_EXE_ne"));
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    command
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env("NE_TEST_READY", &ready)
        .env("NE_TEST_RELEASE", &release);
    let mut client = Client::start(command);
    client.ok(
        "summary_config_set",
        json!({"agent":"codex","model":"protocol-model"}),
    );
    let wait_ready = || {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready.exists() {
            assert!(
                Instant::now() < deadline,
                "Agent did not reach test barrier"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let request = client.begin("tools/call",json!({"name":"summary_update","arguments":{"kind":"session","id":id,"request_id":"cancel-extraction"}}));
    wait_ready();
    let pid = std::fs::read_to_string(&ready).unwrap();
    // A running model operation must not block queries or cancel other requests.
    assert_eq!(client.ok("status", json!({}))["connected"], true);
    assert_eq!(
        client.ok("request_get", json!({"id":"cancel-extraction"}))["status"],
        "running"
    );
    client.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":request,"reason":"test cancellation"}}));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let receipt = client.ok("request_get", json!({"id":"cancel-extraction"}));
        if receipt["status"] == "completed" {
            assert_eq!(
                receipt["response"]["error"]["code"], "cancelled",
                "{receipt}"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Cancelled request did not release its receipt"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !Command::new("kill")
            .args(["-0", &pid])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success(),
        "Cancelled extraction left its Agent alive"
    );
    client.ok(
        "task_create",
        json!({"title":"Connection remains writable"}),
    );
    assert_eq!(
        client.ok("summary_get", json!({"kind":"session","id":id}))["revision"],
        0
    );
    // The same connection can perform a subsequent successful extraction.
    std::fs::remove_file(&ready).unwrap();
    std::fs::write(&release, "continue").unwrap();
    assert_eq!(
        client.ok(
            "summary_update",
            json!({"kind":"session","id":id,"request_id":"success-extraction"})
        )["status"],
        "updated"
    );
    assert_eq!(
        client.ok("summary_get", json!({"kind":"session","id":id}))["summary"]["items"][0]["title"],
        "Confirmed memory"
    );
    // Disconnecting also cancels a new run and preserves the committed summary.
    std::fs::remove_file(&ready).unwrap();
    std::fs::remove_file(&release).unwrap();
    client.begin("tools/call",json!({"name":"summary_update","arguments":{"kind":"session","id":id,"regenerate":true,"request_id":"disconnect-extraction"}}));
    wait_ready();
    let pid = std::fs::read_to_string(&ready).unwrap();
    client.finish();
    assert!(
        !Command::new("kill")
            .args(["-0", &pid])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success(),
        "Disconnected MCP left its Agent alive"
    );
    let mut next = f.start();
    let receipt = next.ok("request_get", json!({"id":"disconnect-extraction"}));
    assert_eq!(receipt["status"], "completed", "{receipt}");
    assert_eq!(
        receipt["response"]["error"]["code"], "cancelled",
        "{receipt}"
    );
    assert_eq!(
        next.ok("summary_get", json!({"kind":"session","id":id}))["revision"],
        1
    );
    next.finish();
}
