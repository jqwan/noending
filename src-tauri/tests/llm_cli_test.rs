//! Optional live Agent smoke tests. Authenticated CLI/model prerequisites are explicit.
use noending::adapters::ExecOptions;
use noending::domain::Agent;
use noending::platform::exec_runner::{clean_exec_stdout, run_headless};
use noending_lib as noending;
// Real-model smoke tests (#[ignore]: need the authenticated agent CLIs)

#[test]
#[ignore = "requires Pi CLI and LM Studio serving qwen/qwen3.8-27b"]
fn real_pi_local_qwen_headless() {
    let install = noending::platform::exec_resolver::resolve(Agent::Pi).expect("pi CLI");
    let adapter = noending::adapters::adapter_for(Agent::Pi);
    let cmd = adapter
        .build_exec_command(
            &install,
            &ExecOptions {
                model: Some("qwen/qwen3.8-27b".into()),
                provider: Some("lmstudio".into()),
                effort: Some("low".into()),
                ..Default::default()
            },
            "只输出一个 JSON 对象，不要其他文字：{\"summary\":\"采用方案A\"}",
        )
        .unwrap();
    let out = run_headless(&cmd, 180).expect("pi headless run");
    let text = clean_exec_stdout(&out.stdout);
    println!("pi qwen output: {}", text);
    assert!(
        text.contains("方案A"),
        "pi+qwen should echo the fixture decision"
    );
}

struct AssistantFixture {
    root: std::path::PathBuf,
    db: Option<std::sync::Arc<noending::storage::Db>>,
}
impl Drop for AssistantFixture {
    fn drop(&mut self) {
        drop(self.db.take());
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
#[ignore = "requires installed, authenticated Codex CLI, gpt-6-luna and a model call"]
fn real_codex_assistant_queries_workspace_through_cli() {
    use noending::{
        assistant, cli,
        storage::{new_id, now, Db},
        workspace::{home::NoEndingHome, wiring::WorkspaceLayer},
    };
    let root = std::env::temp_dir().join(format!("noending-live-assistant-{}", new_id()));
    let home = NoEndingHome::new(root.to_str().unwrap(), None).unwrap();
    home.ensure_dirs().unwrap();
    let db = std::sync::Arc::new(Db::open(&home.db_path).unwrap());
    let layer = std::sync::Arc::new(WorkspaceLayer::new(&home));
    let _fixture = AssistantFixture {
        root,
        db: Some(db.clone()),
    };
    let task_id = new_id();
    let task_title = format!("隔离助手测试-{}", new_id());
    db.upsert_workstream(&noending::domain::Workstream {
        id: task_id.clone(),
        title: task_title.clone(),
        description: "仅用于 CLI 查询验证".into(),
        visibility: "normal".into(),
        created_at: now(),
        updated_at: now(),
    })
    .unwrap();
    let workspace = assistant::ensure_workspace(
        &db,
        &home,
        &layer,
        &noending::skills::Roots::current().unwrap(),
    )
    .unwrap();
    cli::install_workspace_entry(
        &std::path::Path::new(&workspace.directory).join("bin"),
        &home.root,
        std::path::Path::new(env!("CARGO_BIN_EXE_ne")),
    )
    .unwrap();
    let install =
        noending::platform::exec_resolver::resolve(Agent::Codex).expect("Codex CLI required");
    let mut command = noending::adapters::adapter_for(Agent::Codex)
        .build_new_command(
            &install,
            &ExecOptions {
                model: Some("gpt-6-luna".into()),
                effort: Some("high".into()),
                initial_message: Some(
                    "查询整个工作区当前的任务，只回复查到的任务标题与任务 ID。不要直接访问数据库。"
                        .into(),
                ),
                ..Default::default()
            },
            Some(std::path::Path::new(&workspace.directory)),
        )
        .unwrap();
    // Exercise the terminal launch policy in a one-shot process for this smoke
    // test, preserving the assistant cwd and native skill loading.
    command.args.splice(
        0..0,
        [
            "exec".into(),
            "--ephemeral".into(),
            "--skip-git-repo-check".into(),
        ],
    );
    let output = run_headless(&command, 180).expect("Codex query");
    let answer = clean_exec_stdout(&output.stdout);
    println!("Assistant fixture answer: {answer}");
    assert!(
        answer.contains(&task_id),
        "must retrieve the unpredictable task ID through CLI: {answer}"
    );
    assert!(
        answer.contains(&task_title),
        "must retrieve the unpredictable task title through CLI: {answer}"
    );
    drop(db);
}
