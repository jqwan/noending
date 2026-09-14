//! Workstream card stats must follow the real sources: session count /
//! latest session come from live bindings, card text from the active
//! current_state revision, and a zero-session Workstream stays a legal
//! card with no Resume target.

use std::path::PathBuf;

use noending::adapters::DiscoveredSession;
use noending::domain::{binding_source, Agent, SessionWorkstreamBinding, Workstream};
use noending::ingestion::ensure_session_row;
use noending::storage::{new_id, now, Db};

fn db(tag: &str) -> Db {
    let dir = std::env::temp_dir().join(format!("noending-ws-cards-{}-{}", tag, new_id()));
    Db::open(&dir.join("test.db")).unwrap()
}

fn discovered(agent: Agent, agent_session_id: &str, activity: &str) -> DiscoveredSession {
    DiscoveredSession {
        agent,
        agent_session_id: agent_session_id.into(),
        path: PathBuf::from(format!("/tmp/fake/{}.jsonl", agent_session_id)),
        cwd: Some("/tmp/fake".into()),
        started_at: Some("2026-09-01T08:00:00Z".into()),
        last_activity_at: Some(activity.into()),
        first_user_text: Some("继续上次的工作".into()),
        parent_agent_session_id: None,
    }
}

fn bind(db: &Db, session_id: &str, workstream_id: &str) {
    db.bind(&SessionWorkstreamBinding {
        session_id: session_id.into(),
        workstream_id: workstream_id.into(),
        role: "related".into(),
        source: binding_source::USER_ASSIGNED.into(),
        confidence: 1.0,
        last_seen_revision: None,
        last_sync_cursor: 0,
        created_at: now(),
        last_used_at: now(),
    })
    .unwrap();
}

fn workstream(db: &Db, title: &str) -> Workstream {
    let w = Workstream {
        id: new_id(),
        project_id: None,
        title: title.into(),
        description: String::new(),
        lifecycle: "open".into(),
        visibility: "normal".into(),
        created_at: now(),
        updated_at: now(),
    };
    db.upsert_workstream(&w).unwrap();
    w
}

#[test]
fn card_stats_follow_sessions_and_active_state() {
    let database = db("stats");
    let w = workstream(&database, "Context Integrity");

    // Two bound sessions; the OLDER row was written later, activity decides.
    let older = ensure_session_row(
        &database,
        &discovered(Agent::ClaudeCode, "s-old", "2026-09-10T09:00:00Z"),
    )
    .unwrap()
    .0;
    let newer = ensure_session_row(
        &database,
        &discovered(Agent::Codex, "s-new", "2026-09-12T18:00:00Z"),
    )
    .unwrap()
    .0;
    bind(&database, &older.id, &w.id);
    bind(&database, &newer.id, &w.id);

    let item = noending::sync::create_item(
        &database,
        &w.id,
        "current_state",
        "标题行",
        "修复 source-driven identity migration",
        "agent_statement",
        "manual",
        &[],
        None,
        "user",
    )
    .unwrap();

    let cards = noending::commands::workstream_cards(&database).unwrap();
    let card = cards.iter().find(|c| c.workstream.id == w.id).unwrap();
    assert_eq!(card.session_count, 2);
    let latest = card.latest_session.as_ref().unwrap();
    assert_eq!(latest.id, newer.id, "latest session = most recent activity");
    assert_eq!(latest.agent, "codex");
    assert_eq!(
        card.current_state.as_deref(),
        Some("修复 source-driven identity migration")
    );
    // the context edit is the newest work signal, later than both sessions
    assert_eq!(
        card.last_activity_at.as_deref(),
        Some(item.updated_at.as_str())
    );
}

#[test]
fn zero_session_workstream_is_a_legal_card_without_resume_target() {
    let database = db("empty");
    let w = workstream(&database, "Research Agent Memory");
    let cards = noending::commands::workstream_cards(&database).unwrap();
    let card = cards.iter().find(|c| c.workstream.id == w.id).unwrap();
    assert_eq!(card.session_count, 0);
    assert!(card.latest_session.is_none());
    assert!(card.current_state.is_none());
    // no work signal yet → no "Last active"; sorting still surfaces the
    // card via the updated_at fallback
    assert!(card.last_activity_at.is_none());
}

#[test]
fn default_agent_prefers_explicit_choice_then_detected_then_none() {
    let database = db("default-agent");
    // No setting, nothing detected → None: the UI disables New instead of
    // launching an agent that is not installed.
    assert_eq!(
        noending::commands::default_agent_of(&database).unwrap(),
        None
    );

    // A startup probe that resolves becomes the detected fallback default.
    noending::commands::record_installation_probe(
        &database,
        Agent::Codex,
        Some(installation(Agent::Codex)),
    )
    .unwrap();
    assert_eq!(
        noending::commands::default_agent_of(&database).unwrap(),
        Some(Agent::Codex)
    );

    // Explicit choice stays authoritative even while undetected (the UI
    // warns instead of silently substituting).
    database
        .set_setting("launcher.default_agent", "claude_code")
        .unwrap();
    assert_eq!(
        noending::commands::default_agent_of(&database).unwrap(),
        Some(Agent::ClaudeCode)
    );

    // Uninstall snapshot: a failed startup probe REMOVES the cached row —
    // the stale "detected" state must not survive a restart.
    noending::commands::record_installation_probe(&database, Agent::Codex, None).unwrap();
    assert!(database.get_installation(Agent::Codex).unwrap().is_none());

    // Garbage setting falls through to detection; nothing detected left.
    database
        .set_setting("launcher.default_agent", "not-an-agent")
        .unwrap();
    assert_eq!(
        noending::commands::default_agent_of(&database).unwrap(),
        None,
        "an unparseable setting must not fabricate an installed agent"
    );
}

fn installation(agent: Agent) -> noending::platform::exec_resolver::AgentInstallation {
    noending::platform::exec_resolver::AgentInstallation {
        agent,
        executable_path: format!("/tmp/fake-cli-{}", agent.as_str()),
        version: Some("test".into()),
        source: "test".into(),
        last_verified_at: now(),
    }
}

#[test]
fn latest_session_cwd_follows_most_recent_activity() {
    let database = db("launch-cwd");
    let w = workstream(&database, "Context Integrity");

    // Two bound sessions: the older one bound first, but the NEWER one's
    // cwd is where the work actually left off.
    let older = ensure_session_row(
        &database,
        &DiscoveredSession {
            agent: Agent::Codex,
            agent_session_id: "cwd-old".into(),
            path: PathBuf::from("/tmp/fake/cwd-old.jsonl"),
            cwd: Some("/tmp/old-dir".into()),
            started_at: Some("2026-09-10T08:00:00Z".into()),
            last_activity_at: Some("2026-09-10T09:00:00Z".into()),
            first_user_text: Some("old".into()),
            parent_agent_session_id: None,
        },
    )
    .unwrap()
    .0;
    let newer = ensure_session_row(
        &database,
        &DiscoveredSession {
            agent: Agent::Codex,
            agent_session_id: "cwd-new".into(),
            path: PathBuf::from("/tmp/fake/cwd-new.jsonl"),
            cwd: Some("/tmp/new-dir".into()),
            started_at: Some("2026-09-12T08:00:00Z".into()),
            last_activity_at: Some("2026-09-12T09:00:00Z".into()),
            first_user_text: Some("new".into()),
            parent_agent_session_id: None,
        },
    )
    .unwrap()
    .0;
    bind(&database, &older.id, &w.id);
    bind(&database, &newer.id, &w.id);

    assert_eq!(
        database
            .latest_session_cwd_for_workstreams(&[w.id.clone()])
            .unwrap()
            .as_deref(),
        Some("/tmp/new-dir"),
        "the launcher's default cwd = most recent activity's directory"
    );

    // a workstream with no bound sessions suggests nothing
    assert_eq!(
        database
            .latest_session_cwd_for_workstreams(&["does-not-exist".into()])
            .unwrap(),
        None
    );
    assert_eq!(
        database.latest_session_cwd_for_workstreams(&[]).unwrap(),
        None
    );
}

#[test]
fn resolved_state_items_no_longer_feed_the_card() {
    let database = db("resolved");
    let w = workstream(&database, "Initial UI Redesign");
    let item = noending::sync::create_item(
        &database,
        &w.id,
        "current_state",
        "旧状态",
        "旧状态内容",
        "agent_statement",
        "manual",
        &[],
        None,
        "user",
    )
    .unwrap();
    noending::sync::create_item(
        &database,
        &w.id,
        "current_state",
        "新状态",
        "新状态内容",
        "agent_statement",
        "manual",
        &[],
        None,
        "user",
    )
    .unwrap();
    // only the newest ACTIVE item counts; the older one resolved away
    database.set_item_status(&item.id, "resolved").unwrap();

    let cards = noending::commands::workstream_cards(&database).unwrap();
    let card = cards.iter().find(|c| c.workstream.id == w.id).unwrap();
    assert_eq!(card.current_state.as_deref(), Some("新状态内容"));
}
