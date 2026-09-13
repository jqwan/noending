//! Real-model integration tests for the CLI-backed Assistant Runtime.
//!
//! These spawn the actual agent CLIs (codex exec / pi -p) and need the
//! user's authenticated environment, so they are #[ignore] by default:
//!
//!   cargo test --test llm_cli_test -- --ignored --nocapture
//!
//! Models exercised (user-specified):
//!   - Codex: gpt-5.6-luna @ reasoning effort low
//!   - Pi:    local qwen/qwen3.8-27b via LM Studio

use noending::adapters::{ExecOptions};
use noending::domain::{Agent, Session, SessionEvent};
use noending::platform::exec_runner::{clean_exec_stdout, run_headless};
use noending::storage::{new_id, Db};
use noending::sync::extractor::{parse_mutations, CliExtractor};
use noending::sync::ContextExtractor;

fn temp_db() -> Db {
    let dir = std::env::temp_dir().join(format!("noending-llm-test-{}", new_id()));
    Db::open(&dir.join("test.db")).expect("temp db")
}

fn fake_session() -> Session {
    Session {
        id: new_id(),
        agent: Agent::Codex,
        agent_session_id: "llm-test".into(),
        title: None,
        cwd: None,
        project_id: None,
        raw_path: "/tmp/llm-test.jsonl".into(),
        parent_agent_session_id: None,
        started_at: None,
        last_activity_at: None,
    }
}

fn ev(session: &Session, seq: i64, kind: &str, text: &str) -> SessionEvent {
    SessionEvent {
        session_id: session.id.clone(),
        sequence: seq,
        ts: None,
        kind: kind.into(),
        text: Some(text.into()),
        raw_ref: format!("test#{}", seq),
        metadata: serde_json::json!({}),
    }
}

#[test]
#[ignore]
fn real_codex_extracts_mutations() {
    let db = temp_db();
    // candidate workstream visible to the model
    let ws = noending::domain::Workstream {
        id: "ws-test-1".into(),
        project_id: None,
        title: "Context Sync".into(),
        description: "设计 Session 到 Workstream 的上下文同步机制".into(),
        lifecycle: "open".into(),
        visibility: "normal".into(),
        created_at: String::new(),
        updated_at: String::new(),
    };
    db.upsert_workstream(&ws).unwrap();

    let session = fake_session();
    let e1 = ev(&session, 1, "user_message",
        "我们决定放弃 handoff 模型，改成 Workstream 上下文自动同步：每个 Session 维护 cursor，增量拉取新消息后由 Assistant 提取变更合并进 Workstream。");
    let e2 = ev(&session, 2, "assistant_message",
        "好的。注意一个约束：不能修改 Codex/Claude/Pi 的原始会话文件，只读摄取。另外待办：下一步要实现 SessionCursor 的 truncate 检测。");
    let events = vec![&e1, &e2];

    let cli = CliExtractor {
        agent: Agent::Codex,
        opts: ExecOptions {
            model: Some("gpt-5.6-luna".into()),
            provider: None,
            effort: Some("low".into()),
        },
        timeout_secs: 180,
    };
    println!("runtime: {}", cli.name());
    let mutations = cli
        .extract(&db, &session, &events, &[ws.id.clone()])
        .expect("codex CLI extraction should succeed");

    println!("mutations: {:#?}", mutations);
    assert!(!mutations.is_empty(), "expected at least one mutation from real model");

    let has_decision = mutations.iter().any(|m| match m {
        noending::sync::ContextMutation::Add { item_kind, workstream_id, .. } =>
            item_kind == "decision" && workstream_id == "ws-test-1",
        _ => false,
    });
    assert!(has_decision, "model should extract the decision, got {:#?}", mutations);
}

#[test]
#[ignore]
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
            },
            "只输出一个 JSON 数组，不要其他文字：[{\"op\":\"add\",\"workstream_id\":\"ws1\",\"item_kind\":\"decision\",\"title\":\"采用方案A\",\"content\":\"测试\",\"refs\":[\"#1\"]}]",
        )
        .unwrap();
    let out = run_headless(&cmd, 180).expect("pi headless run");
    let text = clean_exec_stdout(&out.stdout);
    println!("pi qwen output: {}", text);
    assert!(text.contains("方案A"), "pi+qwen should echo the fixture decision");
}

#[test]
#[ignore]
fn real_codex_assistant_chat_roundtrip() {
    let db = temp_db();
    let ws = noending::domain::Workstream {
        id: "ws-chat-1".into(),
        project_id: None,
        title: "NoEnding 品牌".into(),
        description: "整理品牌视觉与文案".into(),
        lifecycle: "open".into(),
        visibility: "normal".into(),
        created_at: String::new(),
        updated_at: String::new(),
    };
    db.upsert_workstream(&ws).unwrap();

    let reply = noending::assistant::AssistantService::chat(&db, None, "现在有哪些 Workstream？用一句话概括。")
        .expect("assistant chat should succeed");
    println!("assistant: {} [runtime={}]", reply.content, reply.runtime);
    assert!(!reply.content.is_empty());
    assert!(reply.content.contains("品牌") || reply.content.contains("Workstream"),
        "answer should reference the workstream list");
    assert!(reply.action.is_none(), "a read-only question must not propose actions");
}

#[test]
fn parse_mutations_validates_candidate_ids() {
    let session = fake_session();
    let text = r##" [{"op":"add","workstream_id":"nope","item_kind":"decision","title":"x","content":"y","refs":["#1"]}] "##;
    let m = parse_mutations(text, &["ws1".into()], &session).unwrap();
    assert!(m.is_empty(), "mutations outside candidates must be dropped");
}
