//! Optional live Agent smoke tests. Authenticated CLI/model prerequisites are explicit.
use noending::adapters::ExecOptions;
use noending::domain::Agent;
use noending::platform::exec_runner::{clean_exec_stdout, run_headless};
use noending::storage::{new_id, Db};
fn temp_db() -> Db {
    let dir = std::env::temp_dir().join(format!("noending-llm-test-{}", new_id()));
    Db::open(&dir.join("test.db")).expect("temp db")
}

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

#[test]
#[ignore = "requires installed, authenticated Codex CLI and a model call"]
fn real_codex_assistant_chat_roundtrip() {
    let db = temp_db();
    assert!(
        noending::adapters::adapter_for(Agent::Codex)
            .detect()
            .is_some(),
        "Codex CLI required"
    );
    db.set_setting("assistant.agent", "codex").unwrap();
    let ws = noending::domain::Workstream {
        id: "ws-chat-1".into(),
        title: "NoEnding 品牌".into(),
        description: "整理品牌视觉与文案".into(),
        visibility: "normal".into(),
        created_at: String::new(),
        updated_at: String::new(),
    };
    db.upsert_workstream(&ws).unwrap();

    let reply = noending::assistant::AssistantService::chat(
        &db,
        None,
        "现在有哪些 Workstream？用一句话概括。",
        &std::env::temp_dir().join("noending-assistant"),
    )
    .expect("assistant chat should succeed");
    println!("assistant: {} [runtime={}]", reply.content, reply.runtime);
    assert!(
        reply.runtime.starts_with("cli:codex:"),
        "unexpected fallback: {}",
        reply.runtime
    );
    assert!(!reply.content.is_empty());
    assert!(
        reply.content.contains("品牌") || reply.content.contains("Workstream"),
        "answer should reference the workstream list"
    );
    assert!(
        reply.action.is_none(),
        "a read-only question must not propose actions"
    );
}
