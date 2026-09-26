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

use noending::adapters::ExecOptions;
use noending::domain::{Agent, Session, SessionMessage, SessionMessageRole};
use noending::platform::exec_runner::{clean_exec_stdout, run_headless};
use noending::storage::{new_id, Db};
use noending::sync::extractor::{parse_mutations, CliExtractor, PromptMessageRef};
use noending::sync::ContextExtractor;

fn temp_db() -> Db {
    let dir = std::env::temp_dir().join(format!("noending-llm-test-{}", new_id()));
    Db::open(&dir.join("test.db")).expect("temp db")
}

fn fake_session() -> Session {
    Session {
        id: new_id(),
        agent: Agent::Codex,
        root_agent_session_id: "llm-test".into(),
        title: None,
        cwd: None,
        workspace_path_id: None,
        project_id: None,
        owner_workstream_id: None,
        forked_from_session_id: None,
        started_at: None,
        last_activity_at: None,
        last_conversation_at: None,
        trashed_at: None,
    }
}

/// A root-conversation message built by hand: unit-level extraction tests need
/// no database, only the `&[&SessionMessage]` shape the extractor consumes.
fn msg(session: &Session, seq: i64, role: SessionMessageRole, content: &str) -> SessionMessage {
    SessionMessage {
        id: format!("m-{}", seq),
        session_id: session.id.clone(),
        member_id: "mem-root".into(),
        sequence: seq,
        role,
        content: content.into(),
        ts: None,
        source_message_id: None,
        source_generation: 0,
        source_position: format!("line:{}", seq),
        source_identity_hash: String::new(),
        provider: None,
        model: None,
        raw_ref: format!("test#line:{}", seq),
    }
}

/// A prompt reference map whose sequences are non-1-starting and
/// non-contiguous: exactly the case the mapping must get right.
fn ref_map(messages: &[SessionMessage]) -> Vec<PromptMessageRef> {
    messages
        .iter()
        .enumerate()
        .map(|(i, m)| PromptMessageRef {
            short_ref: format!("#{}", i + 1),
            message_id: m.id.clone(),
            sequence: m.sequence,
            role: m.role,
        })
        .collect()
}

#[test]
#[ignore]
fn real_codex_extracts_mutations() {
    let db = temp_db();
    // candidate workstream visible to the model
    let ws = noending::domain::Workstream {
        id: "ws-test-1".into(),
        title: "Context Sync".into(),
        description: "设计 Session 到 Workstream 的上下文同步机制".into(),
        lifecycle: "active".into(),
        visibility: "normal".into(),
        created_at: String::new(),
        updated_at: String::new(),
    };
    db.upsert_workstream(&ws).unwrap();

    let session = fake_session();
    let m1 = msg(&session, 101, SessionMessageRole::User,
        "我们决定放弃 handoff 模型，改成 Workstream 上下文自动同步：每个 Session 维护 cursor，增量拉取新消息后由 Assistant 提取变更合并进 Workstream。");
    let m2 = msg(&session, 105, SessionMessageRole::Assistant,
        "好的。注意一个约束：不能修改 Codex/Claude/Pi 的原始会话文件，只读摄取。另外待办：下一步要实现 SessionCursor 的 truncate 检测。");
    let messages = vec![m1, m2];

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
    let inputs =
        noending::sync::extractor::collect_prompt_inputs(&db, &ws.id).expect("prompt inputs");
    let out = cli
        .extract(
            &session,
            &messages.iter().collect::<Vec<_>>(),
            &ws.id,
            &inputs,
        )
        .expect("codex CLI extraction should succeed");

    println!("mutations: {:#?}", out.mutations);
    assert!(
        !out.mutations.is_empty(),
        "expected at least one mutation from real model"
    );

    // every resolved ref must point at a REAL message id from the prompt map
    for m in &out.mutations {
        let refs = match m {
            noending::sync::ContextMutation::Add { source_refs, .. } => source_refs,
            noending::sync::ContextMutation::Update { source_refs, .. } => source_refs,
            noending::sync::ContextMutation::Supersede { source_refs, .. } => source_refs,
            noending::sync::ContextMutation::Resolve { source_refs, .. } => source_refs,
            noending::sync::ContextMutation::Conflict { source_refs, .. } => source_refs,
            _ => continue,
        };
        for r in refs {
            let id = r
                .strip_prefix("session-message:")
                .expect("session-message ref");
            assert!(
                messages.iter().any(|e| &e.id == id),
                "ref {} must resolve to a prompt message",
                r
            );
        }
    }

    let has_decision = out.mutations.iter().any(|m| match m {
        noending::sync::ContextMutation::Add {
            item_kind,
            workstream_id,
            ..
        } => item_kind == "decision" && workstream_id == "ws-test-1",
        _ => false,
    });
    assert!(
        has_decision,
        "model should extract the decision, got {:#?}",
        out.mutations
    );
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
    assert!(
        text.contains("方案A"),
        "pi+qwen should echo the fixture decision"
    );
}

#[test]
#[ignore]
fn real_codex_assistant_chat_roundtrip() {
    let db = temp_db();
    let ws = noending::domain::Workstream {
        id: "ws-chat-1".into(),
        title: "NoEnding 品牌".into(),
        description: "整理品牌视觉与文案".into(),
        lifecycle: "active".into(),
        visibility: "normal".into(),
        created_at: String::new(),
        updated_at: String::new(),
    };
    db.upsert_workstream(&ws).unwrap();

    let reply = noending::assistant::AssistantService::chat(
        &db,
        None,
        "现在有哪些 Workstream？用一句话概括。",
    )
    .expect("assistant chat should succeed");
    println!("assistant: {} [runtime={}]", reply.content, reply.runtime);
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

#[test]
fn parse_mutations_validates_candidate_ids() {
    let session = fake_session();
    let map = vec![PromptMessageRef {
        short_ref: "#1".into(),
        message_id: "m-real-1".into(),
        sequence: 101,
        role: SessionMessageRole::User,
    }];
    let text = r##" [{"op":"add","workstream_id":"nope","item_kind":"decision","title":"x","content":"y","refs":["#1"]}] "##;
    let out = parse_mutations(text, &map, "ws1", &session).unwrap();
    assert!(
        out.mutations.is_empty(),
        "mutations outside candidates must be dropped"
    );
}

#[test]
fn parse_mutations_maps_short_refs_through_prompt_map() {
    let session = fake_session();
    // prompt messages carry sequences 101 / 105 — "#2" must map to the
    // message with id m-real-2 (sequence 105), never to "sequence 2".
    let map = vec![
        PromptMessageRef {
            short_ref: "#1".into(),
            message_id: "m-real-1".into(),
            sequence: 101,
            role: SessionMessageRole::User,
        },
        PromptMessageRef {
            short_ref: "#2".into(),
            message_id: "m-real-2".into(),
            sequence: 105,
            role: SessionMessageRole::Assistant,
        },
    ];
    let text = r##"[
        {"op":"add","workstream_id":"ws1","item_kind":"decision","title":"t1","content":"c","refs":["#2"]},
        {"op":"resolve","item_id":"item-9","refs":["#1","#nope"]},
        {"op":"add","workstream_id":"ws1","item_kind":"note","title":"t3","content":"c","refs":["#777"]}
    ]"##;
    let out = parse_mutations(text, &map, "ws1", &session).unwrap();
    assert_eq!(out.mutations.len(), 3);

    match &out.mutations[0] {
        noending::sync::ContextMutation::Add { source_refs, .. } => {
            assert_eq!(source_refs, &vec!["session-message:m-real-2".to_string()]);
        }
        o => panic!("unexpected: {:?}", o),
    }
    match &out.mutations[1] {
        noending::sync::ContextMutation::Resolve { source_refs, .. } => {
            assert_eq!(source_refs.len(), 1, "invalid ref dropped, valid kept");
            assert_eq!(source_refs[0], "session-message:m-real-1");
        }
        o => panic!("unexpected: {:?}", o),
    }
    match &out.mutations[2] {
        noending::sync::ContextMutation::Add { source_refs, .. } => {
            assert!(source_refs.is_empty(), "#777 has no mapping");
        }
        o => panic!("unexpected: {:?}", o),
    }
    assert!(out.diagnostics.iter().any(|d| d.contains("#nope")));
    assert!(out.diagnostics.iter().any(|d| d.contains("#777")));
    let _ = ref_map(&[]); // keep helper referenced for ignored tests
}
