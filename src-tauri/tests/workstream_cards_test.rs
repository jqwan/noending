//! Workstream card stats must follow the real sources: session count /
//! latest session come from the single Owner Workstream link, activity from
//! the current summary timestamp, and a zero-session Workstream stays a
//! legal card with no Resume target.

mod support;
use noending::domain::{Agent, Session, Workstream};
use noending::storage::{new_id, now, Db};
use noending_lib as noending;

fn db(tag: &str) -> Db {
    let dir = std::env::temp_dir().join(format!("noending-ws-cards-{}-{}", tag, new_id()));
    Db::open(&dir.join("test.db")).unwrap()
}

/// A discovered Logical Session, written through the production discovery
/// write (`upsert_logical_session`): identity is (agent, root id), the cwd
/// and the activity timestamps are the facts the card reads.
fn discovered(db: &Db, agent: Agent, root_agent_session_id: &str, activity: &str) -> Session {
    let (row_id, _) = db
        .upsert_logical_session_unchecked(
            agent,
            root_agent_session_id,
            Some("继续上次的工作"),
            Some("/tmp/fake"),
            None,
            None,
            Some("2026-09-01T08:00:00Z"),
            Some(activity),
        )
        .unwrap();
    db.get_session(&row_id).unwrap().unwrap()
}

fn set_owner(db: &Db, session_id: &str, workstream_id: &str) {
    support::track(&db, session_id, Some(workstream_id)).unwrap();
}

fn workstream(db: &Db, title: &str) -> Workstream {
    let w = Workstream {
        id: new_id(),
        title: title.into(),
        description: String::new(),
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

    // Two owned sessions; the OLDER row was written later, activity decides.
    let older = discovered(
        &database,
        Agent::ClaudeCode,
        "s-old",
        "2026-09-10T09:00:00Z",
    );
    let newer = discovered(&database, Agent::Codex, "s-new", "2026-09-12T18:00:00Z");
    set_owner(&database, &older.id, &w.id);
    set_owner(&database, &newer.id, &w.id);

    database
        .tx(|tx| {
            noending::storage::context_repo::commit_workstream_summary_conn(
                tx,
                &w.id,
                &noending_lib::domain::Summary::single("修复 source-driven identity migration"),
            )
        })
        .unwrap();

    let cards = noending::commands::workstream_cards(&database).unwrap();
    let card = cards.iter().find(|c| c.workstream.id == w.id).unwrap();
    assert_eq!(card.session_count, 2);
    let latest = card.latest_session.as_ref().unwrap();
    assert_eq!(latest.id, newer.id, "latest session = most recent activity");
    assert_eq!(latest.agent, "codex");
    // the context edit is the newest work signal, later than both sessions
    assert_eq!(
        card.last_activity_at.as_deref(),
        database
            .get_workstream_context_state(&w.id)
            .unwrap()
            .updated_at
            .as_deref()
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
