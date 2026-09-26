//! The explicit Context update protocol, end to end.
//!
//! `sync::extractor::parse_update_output` is the ONLY path from a model response
//! to Workstream mutations, and it is all-or-nothing: one unknown field, one id
//! outside the snapshot, one unresolvable ref and the whole update is refused.
//! These tests drive it with a hand-built `PromptRefs` snapshot.
//!
//! A few real-model tests below are #[ignore] by default because they spawn the
//! user's authenticated agent CLIs:
//!
//!   cargo test --test llm_cli_test -- --ignored --nocapture

use std::collections::{HashMap, HashSet};

use noending::adapters::ExecOptions;
use noending::domain::{Agent, SessionMessageRole};
use noending::platform::exec_runner::{clean_exec_stdout, run_headless};
use noending::storage::{new_id, Db};
use noending::sync::extractor::{parse_update_output, PromptMessageRef, PromptRefs};
use noending::sync::ContextMutation;

fn temp_db() -> Db {
    let dir = std::env::temp_dir().join(format!("noending-llm-test-{}", new_id()));
    Db::open(&dir.join("test.db")).expect("temp db")
}

/// The prompt-local reference snapshot.
///
/// `#1` is a USER message and `#2` an ASSISTANT one — that is what decides the
/// derived authority of anything citing them. `@s1` cites the current revision
/// of Session `s1`'s (nonexistent, but snapshot-known) Context.
fn prompt_refs() -> PromptRefs {
    PromptRefs {
        messages: vec![
            PromptMessageRef {
                short_ref: "#1".into(),
                message_id: "m-real-1".into(),
                role: SessionMessageRole::User,
            },
            PromptMessageRef {
                short_ref: "#2".into(),
                message_id: "m-real-2".into(),
                role: SessionMessageRole::Assistant,
            },
        ],
        session_cite_revision: HashMap::from([("s1".to_string(), 3)]),
    }
}

/// The exact set of session ids the model MUST echo.
fn targets() -> Vec<String> {
    vec!["s1".to_string()]
}

/// The only Workstream item ids the model may name in update/supersede/resolve.
fn allowed_item_ids() -> HashSet<String> {
    HashSet::from(["item-1".to_string()])
}

/// A legal response parses in full: session Contexts are normalized/deduped,
/// refs resolve through the snapshot (not through prompt position), citation
/// authority is derived from the cited roles, and duplicate refs collapse.
#[test]
fn valid_output_is_parsed_and_refs_resolve() {
    let text = r##"好的：
```json
{
  "session_contexts": [
    {"session_id":"s1","summary_current_state":"  推进    重构 ","decisions":["采用 SQLite","采用 SQLite"],"open_questions":[],"next_steps":["补测试"]}
  ],
  "workstream_mutations": [
    {"op":"add","item_kind":"decision","title":"采用 SQLite","content":"决定用 SQLite","refs":["#1","#1"]},
    {"op":"update","item_id":"item-1","title":"补充约束","content":"内存上限 2GB","refs":["#2"]},
    {"op":"resolve","item_id":"item-1","refs":["@s1"]}
  ]
}
```"##;

    let out =
        parse_update_output(text, &prompt_refs(), "ws1", &targets(), &allowed_item_ids()).unwrap();

    assert_eq!(out.session_contexts.len(), 1);
    assert_eq!(out.session_contexts[0].session_id, "s1");
    assert_eq!(
        out.session_contexts[0].fields.summary_current_state, "推进 重构",
        "whitespace in a summary is normalized"
    );
    assert_eq!(
        out.session_contexts[0].fields.decisions,
        vec!["采用 SQLite"],
        "duplicate list entries collapse"
    );
    assert!(out.session_contexts[0].fields.open_questions.is_empty());
    assert_eq!(out.session_contexts[0].fields.next_steps, vec!["补测试"]);

    assert_eq!(out.workstream_mutations.len(), 3);

    match &out.workstream_mutations[0] {
        ContextMutation::Add {
            workstream_id,
            item_kind,
            source_refs,
            authority,
            ..
        } => {
            assert_eq!(workstream_id, "ws1");
            assert_eq!(item_kind, "decision");
            assert_eq!(
                source_refs,
                &vec!["session-message:m-real-1".to_string()],
                "duplicate refs collapse and #1 maps to the real message id"
            );
            assert_eq!(authority, "user_explicit", "citing a user message wins");
        }
        other => panic!("unexpected: {other:?}"),
    }

    match &out.workstream_mutations[1] {
        ContextMutation::Update {
            item_id,
            source_refs,
            authority,
            ..
        } => {
            assert_eq!(item_id, "item-1");
            assert_eq!(source_refs, &vec!["session-message:m-real-2".to_string()]);
            assert_eq!(authority, "agent_statement", "an assistant citation only");
        }
        other => panic!("unexpected: {other:?}"),
    }

    match &out.workstream_mutations[2] {
        ContextMutation::Resolve {
            item_id,
            source_refs,
        } => {
            assert_eq!(item_id, "item-1");
            assert_eq!(
                source_refs,
                &vec!["session-context:s1:3".to_string()],
                "@s1 resolves to the snapshot revision, never to a prompt position"
            );
        }
        other => panic!("unexpected: {other:?}"),
    }
}

/// An unknown top-level field fails the whole update — no partial acceptance.
#[test]
fn unknown_field_is_rejected() {
    let text = r#"{"session_contexts":[],"workstream_mutations":[],"extra":1}"#;
    assert!(
        parse_update_output(text, &prompt_refs(), "ws1", &[], &allowed_item_ids()).is_err(),
        "an undeclared key must reject the entire output"
    );
}

/// A session id that is not one of the expected targets is refused.
#[test]
fn non_target_session_is_rejected() {
    let text = r#"{"session_contexts":[{"session_id":"other","summary_current_state":"","decisions":[],"open_questions":[],"next_steps":[]}],"workstream_mutations":[]}"#;
    assert!(
        parse_update_output(text, &prompt_refs(), "ws1", &targets(), &allowed_item_ids()).is_err(),
        "only the sessions marked 必须输出 may appear"
    );
}

/// Every target session must be present; silence is an error, not an omission.
#[test]
fn missing_target_session_is_rejected() {
    let text = r#"{"session_contexts":[],"workstream_mutations":[]}"#;
    assert!(
        parse_update_output(text, &prompt_refs(), "ws1", &targets(), &allowed_item_ids()).is_err()
    );
}

/// A target session repeated twice is refused rather than merged.
#[test]
fn duplicate_session_is_rejected() {
    let text = r#"{"session_contexts":[{"session_id":"s1","summary_current_state":"a","decisions":[],"open_questions":[],"next_steps":[]},{"session_id":"s1","summary_current_state":"b","decisions":[],"open_questions":[],"next_steps":[]}],"workstream_mutations":[]}"#;
    assert!(
        parse_update_output(text, &prompt_refs(), "ws1", &targets(), &allowed_item_ids()).is_err(),
        "duplicate session_contexts entries are rejected"
    );
}

/// A repeated JSON key is a malformed object, not a last-one-wins overwrite.
#[test]
fn duplicate_json_field_is_rejected() {
    let text = r#"{"session_contexts":[],"session_contexts":[],"workstream_mutations":[]}"#;
    assert!(
        parse_update_output(text, &prompt_refs(), "ws1", &[], &allowed_item_ids()).is_err(),
        "a duplicated object key must not be silently accepted"
    );

    let dup_inner = r#"{"session_contexts":[{"session_id":"s1","summary_current_state":"a","summary_current_state":"b","decisions":[],"open_questions":[],"next_steps":[]}],"workstream_mutations":[]}"#;
    assert!(
        parse_update_output(
            dup_inner,
            &prompt_refs(),
            "ws1",
            &targets(),
            &allowed_item_ids()
        )
        .is_err(),
        "a duplicated field inside a session Context must reject the whole update"
    );
}

/// update / supersede / resolve may only name ids the snapshot exposed.
#[test]
fn unknown_item_id_is_rejected() {
    let text = r##"{"session_contexts":[],"workstream_mutations":[{"op":"resolve","item_id":"nope","refs":["#1"]}]}"##;
    assert!(parse_update_output(text, &prompt_refs(), "ws1", &[], &allowed_item_ids()).is_err());

    let update = r##"{"session_contexts":[],"workstream_mutations":[{"op":"update","item_id":"nope","title":"t","content":"c","refs":["#1"]}]}"##;
    assert!(parse_update_output(update, &prompt_refs(), "ws1", &[], &allowed_item_ids()).is_err());
}

/// Refs are resolved against the prompt snapshot only: an out-of-snapshot
/// `#N` and a malformed ref both fail the whole update.
#[test]
fn unknown_ref_is_rejected() {
    let out_of_range = r##"{"session_contexts":[],"workstream_mutations":[{"op":"add","item_kind":"note","title":"t","content":"c","refs":["#99"]}]}"##;
    assert!(parse_update_output(
        out_of_range,
        &prompt_refs(),
        "ws1",
        &[],
        &allowed_item_ids()
    )
    .is_err());

    let unknown_ref_form = r##"{"session_contexts":[],"workstream_mutations":[{"op":"add","item_kind":"note","title":"t","content":"c","refs":["bogus"]}]}"##;
    assert!(parse_update_output(
        unknown_ref_form,
        &prompt_refs(),
        "ws1",
        &[],
        &allowed_item_ids()
    )
    .is_err());

    let unknown_session_cite = r##"{"session_contexts":[],"workstream_mutations":[{"op":"add","item_kind":"note","title":"t","content":"c","refs":["@nope"]}]}"##;
    assert!(parse_update_output(
        unknown_session_cite,
        &prompt_refs(),
        "ws1",
        &[],
        &allowed_item_ids()
    )
    .is_err());
}

/// Only add / update / supersede / resolve exist now; anything else — including
/// the removed conflict op — is refused.
#[test]
fn unknown_op_is_rejected() {
    let text = r##"{"session_contexts":[],"workstream_mutations":[{"op":"conflict","item_id":"item-1","title":"t","content":"c","refs":["#1"]}]}"##;
    assert!(parse_update_output(text, &prompt_refs(), "ws1", &[], &allowed_item_ids()).is_err());

    let delete = r##"{"session_contexts":[],"workstream_mutations":[{"op":"delete","item_id":"item-1","refs":["#1"]}]}"##;
    assert!(parse_update_output(delete, &prompt_refs(), "ws1", &[], &allowed_item_ids()).is_err());
}

/// `add` without a legal item_kind or title is refused even when the refs and
/// object shape are otherwise fine.
#[test]
fn add_without_kind_or_title_is_rejected() {
    let no_kind = r##"{"session_contexts":[],"workstream_mutations":[{"op":"add","title":"t","content":"c","refs":["#1"]}]}"##;
    assert!(parse_update_output(no_kind, &prompt_refs(), "ws1", &[], &allowed_item_ids()).is_err());

    let bad_kind = r##"{"session_contexts":[],"workstream_mutations":[{"op":"add","item_kind":"nonsense","title":"t","content":"c","refs":["#1"]}]}"##;
    assert!(
        parse_update_output(bad_kind, &prompt_refs(), "ws1", &[], &allowed_item_ids()).is_err()
    );

    let no_title = r##"{"session_contexts":[],"workstream_mutations":[{"op":"add","item_kind":"note","content":"c","refs":["#1"]}]}"##;
    assert!(
        parse_update_output(no_title, &prompt_refs(), "ws1", &[], &allowed_item_ids()).is_err()
    );
}

// Real-model smoke tests (#[ignore]: need the authenticated agent CLIs)

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
            "只输出一个 JSON 对象，不要其他文字：{\"session_contexts\":[],\"workstream_mutations\":[{\"op\":\"add\",\"item_kind\":\"decision\",\"title\":\"采用方案A\",\"content\":\"测试\",\"refs\":[\"#1\"]}]}",
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
