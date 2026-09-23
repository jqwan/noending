//! Session rows must follow the transcript: discovery metadata (cwd, raw
//! path, activity) is source-derived, so a later discovery refreshes it —
//! including correcting values written by an older ingestion.

use std::path::PathBuf;

use noending::adapters::DiscoveredSession;
use noending::domain::{Agent, Session};
use noending::ingestion::ensure_session_row;
use noending::storage::{new_id, Db};

fn db(tag: &str) -> Db {
    let dir = std::env::temp_dir().join(format!("noending-session-meta-{}-{}", tag, new_id()));
    Db::open(&dir.join("test.db")).unwrap()
}

fn discovered(agent_session_id: &str, cwd: Option<&str>, activity: &str) -> DiscoveredSession {
    DiscoveredSession {
        agent: Agent::ClaudeCode,
        agent_session_id: agent_session_id.into(),
        path: PathBuf::from(format!("/tmp/fake/{}.jsonl", agent_session_id)),
        cwd: cwd.map(|c| c.into()),
        started_at: Some("2026-08-18T14:21:22Z".into()),
        last_activity_at: Some(activity.into()),
        first_user_text: Some("帮我看看这个量化脚本".into()),
        native_title: None,
        first_agent_text: None,
        parent_agent_session_id: None,
    }
}

#[test]
fn discovery_refreshes_stale_cwd_on_existing_row() {
    let database = db("refresh");
    let agent_session_id = format!("meta-{}", new_id());

    // Old ingestion wrote a decode-era cwd; discovery now reads the truth
    // from the transcript content.
    let first = ensure_session_row(
        &database,
        &discovered(
            &agent_session_id,
            Some("/Users/jqk/projects/r/stock/quant"),
            "2026-08-18T14:30:00Z",
        ),
    )
    .unwrap()
    .0;
    assert_eq!(
        first.cwd.as_deref(),
        Some("/Users/jqk/projects/r/stock/quant")
    );

    let (second, is_new) = ensure_session_row(
        &database,
        &discovered(
            &agent_session_id,
            Some("/Users/jqk/projects/r/stock_quant"),
            "2026-08-19T09:00:00Z",
        ),
    )
    .unwrap();
    assert!(!is_new);
    assert_eq!(second.id, first.id, "same external session, same row");
    assert_eq!(
        second.cwd.as_deref(),
        Some("/Users/jqk/projects/r/stock_quant")
    );

    let stored: Session = database.get_session(&first.id).unwrap().unwrap();
    assert_eq!(
        stored.cwd.as_deref(),
        Some("/Users/jqk/projects/r/stock_quant")
    );
    assert_eq!(
        stored.last_activity_at.as_deref(),
        Some("2026-08-19T09:00:00Z")
    );
}

#[test]
fn discovery_without_cwd_keeps_stored_value() {
    let database = db("keep");
    let agent_session_id = format!("meta-{}", new_id());

    let first = ensure_session_row(
        &database,
        &discovered(
            &agent_session_id,
            Some("/Users/jqk/projects/r/stock_quant"),
            "2026-08-18T14:30:00Z",
        ),
    )
    .unwrap()
    .0;

    // A later scan that fails to read cwd (empty file, parse gap) must not
    // wipe the stored value.
    let (second, _) = ensure_session_row(
        &database,
        &discovered(&agent_session_id, None, "2026-08-18T14:30:00Z"),
    )
    .unwrap();
    assert_eq!(
        second.cwd.as_deref(),
        Some("/Users/jqk/projects/r/stock_quant")
    );
    let stored: Session = database.get_session(&first.id).unwrap().unwrap();
    assert_eq!(
        stored.cwd.as_deref(),
        Some("/Users/jqk/projects/r/stock_quant")
    );
}

#[test]
fn discovery_title_only_fills_missing_value() {
    let database = db("title");
    let agent_session_id = format!("meta-{}", new_id());

    // Row created without a title (no usable first user message yet).
    let mut d = discovered(&agent_session_id, Some("/tmp/w"), "2026-08-18T14:30:00Z");
    d.first_user_text = None;
    let first = ensure_session_row(&database, &d).unwrap().0;
    assert!(first.title.is_none());

    let (_, _) = ensure_session_row(
        &database,
        &discovered(&agent_session_id, Some("/tmp/w"), "2026-08-18T14:31:00Z"),
    )
    .unwrap();
    let stored: Session = database.get_session(&first.id).unwrap().unwrap();
    assert_eq!(stored.title.as_deref(), Some("帮我看看这个量化脚本"));
}

#[test]
fn unchanged_discovery_rewrites_nothing() {
    let database = db("stable");
    let agent_session_id = format!("meta-{}", new_id());

    let first = ensure_session_row(
        &database,
        &discovered(
            &agent_session_id,
            Some("/Users/jqk/projects/r/stock_quant"),
            "2026-08-18T14:30:00Z",
        ),
    )
    .unwrap()
    .0;

    // Identical scan: row must come back byte-identical (same id, no churn).
    let (again, is_new) = ensure_session_row(
        &database,
        &discovered(
            &agent_session_id,
            Some("/Users/jqk/projects/r/stock_quant"),
            "2026-08-18T14:30:00Z",
        ),
    )
    .unwrap();
    assert!(!is_new);
    assert_eq!(again.id, first.id);
    assert_eq!(again.cwd, first.cwd);
    assert_eq!(again.last_activity_at, first.last_activity_at);
    assert_eq!(again.title, first.title);
}
