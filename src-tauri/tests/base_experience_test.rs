//! Base Experience invariants (Core Workspace Experience v0.1 §6, §7, §11.3).
//!
//! Context Intelligence is a switch, not a deletion: with it off, NoEnding
//! must still discover, ingest, index and store every Agent event, and must
//! write ZERO Context. The processed cursor stays frozen so the corpus can be
//! replayed after intelligence is switched back on — "sessions end, the
//! evidence does not".
//!
//! These tests drive the production ingest+sync orchestration paths (the ones
//! reconcile, launch preparation and the Session detail refresh all share),
//! not the SyncEngine directly, because the switch lives in the orchestration.

use noending::context::ContextDeliveryLevel;
use noending::domain::{binding_source, Agent, Session, Workstream};
use noending::storage::{new_id, now, Db};
use noending::{ingestion, launcher, search, settings, sync};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

fn unique_dir(tag: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "noending-base-{}-{}-{}",
        tag,
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn open_db(tag: &str) -> Db {
    Db::open(&unique_dir(tag).join("test.db")).unwrap()
}

/// Deterministic per-content uuid so a re-scan hashes to the same events.
fn claude_line(role: &str, text: &str) -> String {
    format!(
        r#"{{"type":"{role}","uuid":"u-{role}-{text}","timestamp":"2026-09-19T10:00:00Z","message":{{"role":"{role}","content":[{{"type":"text","text":"{text}"}}]}}}}"#
    )
}

const MSGS: [&str; 3] = [
    "决定改用 postgres-primary-store 作为主数据库，SQLite 只保留本地状态",
    "补充约束：api keys 不能提交到仓库，必须走环境变量注入",
    "下一步先把 ingestion 与 sync 的边界拆干净，再处理 launcher 的 prepare 路径",
];

/// Write a transcript containing the first `n` fixture messages.
fn write_transcript(dir: &std::path::Path, n: usize) -> PathBuf {
    let file = dir.join("session.jsonl");
    let body = MSGS[..n]
        .iter()
        .enumerate()
        .map(|(i, m)| claude_line(if i % 2 == 0 { "user" } else { "assistant" }, m))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&file, body).unwrap();
    file
}

fn session_row(db: &Db, path: &std::path::Path) -> Session {
    let s = Session {
        id: new_id(),
        agent: Agent::ClaudeCode,
        agent_session_id: format!("as-{}", new_id()),
        title: None,
        cwd: None,
        workspace_path_id: None,
        project_id: None,
        raw_path: path.to_string_lossy().to_string(),
        parent_agent_session_id: None,
        started_at: Some(now()),
        last_activity_at: Some(now()),
        trashed_at: None,
    };
    db.upsert_session(&s).unwrap();
    s
}

fn ws_row(db: &Db, title: &str) -> Workstream {
    let w = Workstream {
        id: new_id(),
        title: title.into(),
        description: String::new(),
        lifecycle: "active".into(),
        visibility: "normal".into(),
        created_at: now(),
        updated_at: now(),
    };
    db.upsert_workstream(&w).unwrap();
    w
}

/// (context_items, revisions, conflicts, sync_runs) — everything Intelligence writes.
fn context_footprint(db: &Db) -> (i64, i64, i64, i64) {
    let c = |sql: &str| {
        db.read()
            .query_row(sql, [], |r| r.get::<_, i64>(0))
            .unwrap()
    };
    (
        c("SELECT COUNT(*) FROM context_items"),
        c("SELECT COUNT(*) FROM context_item_revisions"),
        c("SELECT COUNT(*) FROM context_conflicts"),
        c("SELECT COUNT(*) FROM sync_runs"),
    )
}

#[test]
fn intelligence_is_off_until_explicitly_enabled() {
    let db = open_db("switch-default");
    assert!(
        !settings::context_intelligence_enabled(&db).unwrap(),
        "missing row must mean Base Experience"
    );

    settings::set_context_intelligence_enabled(&db, true).unwrap();
    assert!(settings::context_intelligence_enabled(&db).unwrap());

    // Anything that is not an explicit opt-in keeps intelligence off.
    db.set_setting(settings::CONTEXT_INTELLIGENCE_ENABLED_KEY, "yes-please")
        .unwrap();
    assert!(!settings::context_intelligence_enabled(&db).unwrap());
    db.set_setting(settings::CONTEXT_INTELLIGENCE_ENABLED_KEY, "true")
        .unwrap();
    assert!(settings::context_intelligence_enabled(&db).unwrap());
}

/// The switch never rewrites history: the migration pins delivery explicitly,
/// while intelligence stays a pure "missing row = off" default.
#[test]
fn migration_pins_delivery_level_but_not_intelligence() {
    let db = open_db("v11-seed");
    assert_eq!(
        db.get_setting(settings::CONTEXT_DELIVERY_LEVEL_KEY)
            .unwrap(),
        Some("off".into()),
        "v11 migration pins context.delivery_level as a real, editable row"
    );
    assert_eq!(
        db.get_setting(settings::CONTEXT_INTELLIGENCE_ENABLED_KEY)
            .unwrap(),
        None,
        "no row is seeded for a key whose missing-row default is already off"
    );
}

/// The core invariant: ingestion runs, Context does not.
#[test]
fn off_stops_after_ingestion_on_the_launch_and_refresh_path() {
    let dir = unique_dir("off-launch-path");
    let db = open_db("off-launch-path");
    let file = write_transcript(&dir, 3);
    let s = session_row(&db, &file);
    let ws = ws_row(&db, "NoEnding");
    launcher::record_binding(
        &db,
        &s.id,
        &ws.id,
        "primary",
        binding_source::USER_ASSIGNED,
        1.0,
    )
    .unwrap();

    let (ingested, applied) =
        launcher::ingest_and_sync_session(&db, &sync::SyncEngine::default(), &s).unwrap();
    assert_eq!(
        ingested, 3,
        "events still flow in from the Agent transcript"
    );
    assert_eq!(applied, 0, "no extraction ran");

    assert_eq!(db.get_events(&s.id, None, 100).unwrap().len(), 3);
    assert_eq!(
        db.get_source_cursor(&s.id).unwrap().last_sequence,
        3,
        "read cursor advances normally while off"
    );
    assert_eq!(
        db.get_processed_sequence(&s.id).unwrap(),
        0,
        "context processing frontier must not move while intelligence is off"
    );
    assert_eq!(context_footprint(&db), (0, 0, 0, 0), "zero Context writes");
    assert!(
        search::search(&db, "postgres-primary-store", 10)
            .unwrap()
            .iter()
            .any(|h| h.kind == "event" && h.parent_id == s.id),
        "search indexing stays on (§6)"
    );
}

/// Background reconcile runs the same path as the interactive flow — a gate
/// on only one of them would leave extraction running on every launch.
#[test]
fn off_stops_after_ingestion_on_the_reconcile_path() {
    let dir = unique_dir("off-reconcile");
    let file = write_transcript(&dir, 3);
    let db = open_db("off-reconcile");
    let s = session_row(&db, &file);

    let (ingested, applied) =
        ingestion::ingest_and_sync_session(&db, &sync::SyncEngine::default(), &s).unwrap();
    assert_eq!(ingested, 3);
    assert_eq!(applied, 0, "reconcile must stop before SyncEngine::prepare");

    assert_eq!(db.get_events(&s.id, None, 100).unwrap().len(), 3);
    assert_eq!(db.get_processed_sequence(&s.id).unwrap(), 0);
    assert_eq!(context_footprint(&db), (0, 0, 0, 0));
}

/// §11.1: delivery level and intelligence are two independent switches.
/// Getting this wrong would either resurrect extraction while intelligence is
/// off, or silently stop Context evolution when a user only muted injection.
#[test]
fn delivery_level_never_gates_extraction() {
    // (a) delivery ON (balanced) + intelligence OFF → still nothing extracted.
    let dir = unique_dir("ortho-off");
    let db = open_db("ortho-off");
    let file = write_transcript(&dir, 3);
    let s = session_row(&db, &file);
    let ws = ws_row(&db, "NoEnding");
    launcher::record_binding(
        &db,
        &s.id,
        &ws.id,
        "primary",
        binding_source::USER_ASSIGNED,
        1.0,
    )
    .unwrap();
    settings::set_context_delivery_level(&db, ContextDeliveryLevel::Balanced).unwrap();

    let (_, applied) =
        launcher::ingest_and_sync_session(&db, &sync::SyncEngine::default(), &s).unwrap();
    assert_eq!(applied, 0, "balanced delivery must not restart extraction");
    assert_eq!(context_footprint(&db), (0, 0, 0, 0));

    // (b) intelligence ON + delivery OFF → ingestion, sync and extraction all
    // run exactly as before; only outbound injection is muted.
    settings::set_context_intelligence_enabled(&db, true).unwrap();
    settings::set_context_delivery_level(&db, ContextDeliveryLevel::Off).unwrap();
    let (ingested, applied) =
        launcher::ingest_and_sync_session(&db, &sync::SyncEngine::default(), &s).unwrap();
    assert_eq!(
        ingested, 0,
        "already ingested while off — nothing new to read"
    );
    assert!(
        applied > 0,
        "turning delivery off must never disable extraction (AGENTS.md)"
    );
    assert!(context_footprint(&db).0 > 0, "Context evolved");
    assert_eq!(
        db.get_processed_sequence(&s.id).unwrap(),
        db.get_source_cursor(&s.id).unwrap().last_sequence,
        "the frontier caught up"
    );
}

/// §7's whole point, stated as the only falsifiable form it has: everything
/// ingested during the Off period is still there and replayable afterwards.
#[test]
fn backlog_ingested_while_off_is_replayed_after_reenabling() {
    let dir = unique_dir("replay");
    let db = open_db("replay");
    let file = write_transcript(&dir, 2);
    let s = session_row(&db, &file);
    let ws = ws_row(&db, "NoEnding");
    launcher::record_binding(
        &db,
        &s.id,
        &ws.id,
        "primary",
        binding_source::USER_ASSIGNED,
        1.0,
    )
    .unwrap();

    // Two offline days: the source grows, we keep ingesting, nothing processes.
    for n in [2, 3] {
        write_transcript(&dir, n);
        let (ingested, applied) =
            launcher::ingest_and_sync_session(&db, &sync::SyncEngine::default(), &s).unwrap();
        assert_eq!(applied, 0);
        if n == 3 {
            assert_eq!(ingested, 1, "the third message arrives as one new event");
        }
    }
    assert_eq!(db.get_events(&s.id, None, 100).unwrap().len(), 3);
    assert_eq!(db.get_processed_sequence(&s.id).unwrap(), 0);
    assert_eq!(context_footprint(&db), (0, 0, 0, 0));

    // Re-enable: no re-ingestion needed, the frozen frontier drives the replay.
    settings::set_context_intelligence_enabled(&db, true).unwrap();
    settings::set_context_delivery_level(&db, ContextDeliveryLevel::Balanced).unwrap();
    let (ingested, applied) =
        launcher::ingest_and_sync_session(&db, &sync::SyncEngine::default(), &s).unwrap();
    assert_eq!(ingested, 0, "the backlog was already durable");
    assert!(
        applied > 0,
        "the whole Off-period backlog must be consumable in one sync"
    );
    assert_eq!(
        db.get_processed_sequence(&s.id).unwrap(),
        db.get_source_cursor(&s.id).unwrap().last_sequence,
        "frontier caught up with the read cursor"
    );
    assert_eq!(
        db.get_events(&s.id, None, 100).unwrap().len(),
        3,
        "history intact"
    );
    assert!(context_footprint(&db).0 > 0);
}
