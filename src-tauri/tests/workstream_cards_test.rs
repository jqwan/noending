//! Task card activity and the default AgentTool used by launch entry points.

mod support;
use noending::domain::AgentTool;
use noending::storage::{new_id, Db};
use noending_lib as noending;

fn db(tag: &str) -> Db {
    let dir = std::env::temp_dir().join(format!("noending-ws-cards-{}-{}", tag, new_id()));
    Db::open(&dir.join("test.db")).unwrap()
}

#[test]
fn card_activity_comes_from_task_items_and_summary_edits() {
    use noending::workspace::{task_items, workstream};

    let db = db("task-activity");
    let task = workstream::create_workstream(&db, "Summary or progress", "", &[]).unwrap();
    let card = || {
        noending::commands::workstream::workstream_cards(&db)
            .unwrap()
            .remove(0)
    };
    assert!(card().last_activity_at.is_none());

    let item = task_items::create(
        &db,
        &task.id,
        task_items::CreateItem {
            title: "Verify progress".into(),
            description: None,
        },
    )
    .unwrap();
    assert_eq!(
        card().last_activity_at.as_deref(),
        Some(item.updated_at.as_str())
    );

    let summary_at = "2090-01-01T00:00:00Z";
    db.write()
        .execute(
            "UPDATE workstream_context_state SET updated_at=?2 WHERE workstream_id=?1",
            rusqlite::params![task.id, summary_at],
        )
        .unwrap();
    assert_eq!(card().last_activity_at.as_deref(), Some(summary_at));
}

#[test]
fn default_agent_prefers_explicit_choice_then_detected_then_none() {
    let database = db("default-agent");
    // No setting, nothing detected → None: the UI disables New instead of
    // launching an agent that is not installed.
    assert_eq!(
        noending::commands::default_tool_of(&database).unwrap(),
        None
    );

    // A startup probe that resolves becomes the detected fallback default.
    noending::commands::record_installation_probe(
        &database,
        AgentTool::Codex,
        Some(installation(AgentTool::Codex)),
    )
    .unwrap();
    assert_eq!(
        noending::commands::default_tool_of(&database).unwrap(),
        Some(AgentTool::Codex)
    );

    // Explicit choice stays authoritative even while undetected (the UI
    // warns instead of silently substituting).
    database
        .set_setting("launcher.default_agent", "claude_code")
        .unwrap();
    assert_eq!(
        noending::commands::default_tool_of(&database).unwrap(),
        Some(AgentTool::ClaudeCode)
    );

    // Uninstall snapshot: a failed startup probe REMOVES the cached row —
    // the stale "detected" state must not survive a restart.
    noending::commands::record_installation_probe(&database, AgentTool::Codex, None).unwrap();
    assert!(database
        .get_installation(AgentTool::Codex)
        .unwrap()
        .is_none());

    // Garbage setting falls through to detection; nothing detected left.
    database
        .set_setting("launcher.default_agent", "not-an-agent")
        .unwrap();
    assert_eq!(
        noending::commands::default_tool_of(&database).unwrap(),
        None,
        "an unparseable setting must not fabricate an installed agent"
    );
}

fn installation(agent: AgentTool) -> noending::platform::exec_resolver::AgentToolInstallation {
    noending::platform::exec_resolver::AgentToolInstallation {
        agent,
        executable_path: format!("/tmp/fake-cli-{}", agent.as_str()),
        version: Some("test".into()),
    }
}
