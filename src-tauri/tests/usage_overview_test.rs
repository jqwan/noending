//! Usage overview rollup: all token totals from committed usage events,
//! activity counts from member snapshots.
//! Fixtures write both ports directly — the ingest pipeline is tested
//! elsewhere; this file pins what the panel aggregates.

use noending::adapters::{UsageCategory, UsageEvent};
use noending::domain::{Agent, SourceCursorUpdate};
use noending::storage::{new_id, Db};

fn open_db(tag: &str) -> Db {
    let dir = std::env::temp_dir().join(format!("noending-usage-{tag}-{}", new_id()));
    Db::open(&dir.join("t.db")).unwrap()
}

/// One Logical Session with its root member; returns (session_id, member_id).
fn session(
    db: &Db,
    agent: Agent,
    source: &str,
    title: Option<&str>,
    trashed: bool,
) -> (String, String) {
    let (id, _) = db
        .upsert_logical_root(
            agent,
            source,
            title,
            None,
            None,
            None,
            None,
            Some("2026-09-28T10:00:00Z"),
            "test",
            "/tmp/usage",
            None,
            &serde_json::json!({}),
        )
        .unwrap();
    if trashed {
        noending::lifecycle::trash_session(db, &id).unwrap();
    }
    let member: String = db
        .read()
        .query_row(
            "SELECT id FROM session_members WHERE session_id = ?1",
            [id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    (id, member)
}

fn bill(
    db: &Db,
    member: &str,
    assistant: i64,
    input: Option<i64>,
    output: Option<i64>,
    cached: Option<i64>,
) {
    db.write()
        .execute(
            "INSERT INTO session_member_stats (member_id, updated_at, assistant_message_count)
         VALUES (?1, '2026-09-28T10:00:00Z', ?2)",
            rusqlite::params![member, assistant],
        )
        .unwrap();
    if input.is_some() || output.is_some() || cached.is_some() {
        let session_id: String = db
            .read()
            .query_row(
                "SELECT session_id FROM session_members WHERE id = ?1",
                [member],
                |r| r.get(0),
            )
            .unwrap();
        let mut usage = event(
            None,
            "2026-09-28T10:00:00Z",
            input.unwrap_or(0) as u64,
            output.unwrap_or(0) as u64,
            "fixture",
        );
        usage.cached_tokens = cached.unwrap_or(0) as u64;
        commit(db, &session_id, member, &[usage]);
    }
}

/// A second member (child) on an existing session, with its own stats.
fn child(db: &Db, session_id: &str, agent: Agent, source: &str, input: i64, output: i64) {
    let id = new_id();
    db.write()
        .execute(
            "INSERT INTO session_members
             (id, session_id, agent, source_member_id, relation, source_kind, source_path, metadata)
             VALUES (?1, ?2, ?3, ?4, 'child', 'test', '/tmp/usage-child', '{}')",
            rusqlite::params![id, session_id, agent.as_str(), source],
        )
        .unwrap();
    bill(db, &id, 3, Some(input), Some(output), None);
}

fn event(model: Option<&str>, ts: &str, input: u64, output: u64, key: &str) -> UsageEvent {
    UsageEvent {
        key: Some(key.to_string()),
        category: UsageCategory::Conversation,
        model: model.map(str::to_string),
        provider: None,
        ts: Some(ts.to_string()),
        input_tokens: input,
        output_tokens: output,
        cached_tokens: 0,
        reasoning_tokens: 0,
        request_count: 0,
    }
}

fn cursor() -> SourceCursorUpdate {
    cursor_at(0)
}

fn cursor_at(start: u64) -> SourceCursorUpdate {
    SourceCursorUpdate {
        file_identity: "test-identity".into(),
        generation: 0,
        byte_offset: start + 100,
        last_seen_size: start + 100,
        mtime: Some(1.0),
        start_byte_offset: start,
        prefix_hash: "h".into(),
    }
}

fn commit(db: &Db, session_id: &str, member_id: &str, events: &[UsageEvent]) {
    db.commit_member_ingest_with_provenance_state(
        session_id,
        member_id,
        &[],
        None,
        &cursor(),
        true,
        None,
        None,
        events,
    )
    .unwrap();
}

#[test]
fn snapshot_slices_roll_up_billing_by_agent_relation_project_and_workstream() {
    let db = open_db("slices");
    let (sa, ma) = session(&db, Agent::Codex, "rollout-a", Some("A"), false);
    let (_sb, mb) = session(&db, Agent::Qoder, "qd-a", Some("B"), false);
    let (_sc, mc) = session(&db, Agent::Codex, "rollout-c", Some("C"), true);
    child(&db, &sa, Agent::Codex, "rollout-a-child", 400, 50);

    bill(&db, &ma, 10, Some(1_500), Some(700), Some(300));
    bill(&db, &mb, 5, Some(2_000), Some(100), None);
    bill(&db, &mc, 99, Some(999_999), Some(999_999), None);

    // Attach the project and the owner workstream to session A.
    db.write()
        .execute(
            "INSERT INTO projects (id, name, description, name_customized, created_at, updated_at)
             VALUES ('p-1', 'NoEnding', '', 0, '2026-09-28T10:00:00Z', '2026-09-28T10:00:00Z')",
            [],
        )
        .unwrap();
    db.write()
        .execute(
            "INSERT INTO workstreams (id, title, description, lifecycle, visibility, created_at, updated_at)
             VALUES ('w-1', '用量面板', '', 'active', 'normal', '2026-09-28T10:00:00Z', '2026-09-28T10:00:00Z')",
            [],
        )
        .unwrap();
    db.write()
        .execute(
            "UPDATE sessions SET project_id = 'p-1', owner_workstream_id = 'w-1' WHERE id = ?1",
            [&sa],
        )
        .unwrap();

    db.write()
        .execute(
            "UPDATE session_member_stats SET user_message_count = 2,
                    tool_call_count = 4, side_activity_count = 1",
            [],
        )
        .unwrap();
    let o = db.usage_overview().unwrap();

    // Trashed sessions PARTICIPATE: usage already spent stays counted.
    assert_eq!(o.sessions, 3, "the trashed session C is included");
    assert_eq!(o.members, 4, "总会话: all members count");
    assert_eq!(
        o.requests, 4,
        "one fixture usage record per member, independent of assistant replies"
    );
    assert_eq!(o.root_members, 3, "A + B + C roots");
    assert_eq!(o.user_messages, 8);
    assert_eq!(o.agent_replies, 117, "10 + 3 + 5 + 99 (C included)");
    assert_eq!(o.tool_calls, 16);
    assert_eq!(o.side_activities, 4);
    assert_eq!(
        o.cache_hit_rate,
        Some(300.0 / 1_004_199.0),
        "cache hit uses the same events as token totals"
    );
    assert_eq!(o.assistant_messages, 117, "10 + 3 + 5 + 99 (C included)");
    assert_eq!(o.input_tokens, Some(1_003_899), "1500+400+2000+999999");
    assert_eq!(o.output_tokens, Some(1_000_849), "700+50+100+999999");
    assert_eq!(
        o.reasoning_tokens,
        Some(0),
        "usage events report zero reasoning"
    );

    let codex = o.by_agent.iter().find(|s| s.agent == "codex").unwrap();
    assert_eq!(codex.sessions, 2, "A + trashed C");
    assert_eq!(codex.input_tokens, Some(1_001_899), "root + child + C");
    assert_eq!(codex.members, 3);
    assert_eq!(codex.root_members, 2);
    assert_eq!(codex.user_messages, 6);
    assert_eq!(codex.agent_replies, 112);
    assert_eq!(codex.tool_calls, 12);
    assert_eq!(codex.side_activities, 3);
    let qoder = o.by_agent.iter().find(|s| s.agent == "qoder").unwrap();
    assert_eq!(qoder.input_tokens, Some(2_000));

    // 成员类别的默认序是定死的：根 → 子 → 辅（有辅行时同样殿后）。
    let relations: Vec<&str> = o.by_relation.iter().map(|r| r.relation.as_str()).collect();
    assert_eq!(relations, vec!["root", "child"]);
    let root = o.by_relation.iter().find(|r| r.relation == "root").unwrap();
    assert_eq!(
        root.input_tokens,
        Some(1_003_499),
        "1500 + 2000 + trashed C 999999 (all three roots)"
    );
    let child = o
        .by_relation
        .iter()
        .find(|r| r.relation == "child")
        .unwrap();
    assert_eq!(child.input_tokens, Some(400));
    assert_eq!(root.members, 3);
    assert_eq!(root.root_members, 3);
    assert_eq!(root.agent_replies, 114);
    assert_eq!(child.members, 1);
    assert_eq!(child.root_members, 0);
    assert_eq!(child.user_messages, 2);
    assert_eq!(child.agent_replies, 3);
    assert_eq!(child.tool_calls, 4);
    assert_eq!(child.side_activities, 1);

    assert_eq!(o.by_project.len(), 1);
    assert_eq!(o.by_project[0].name, "NoEnding");
    assert_eq!(o.by_project[0].sessions, 1);
    assert_eq!(o.by_project[0].input_tokens, Some(1_900));
    assert_eq!(o.by_workstream.len(), 1);
    assert_eq!(o.by_workstream[0].name, "用量面板");
    for slice in [&o.by_project[0], &o.by_workstream[0]] {
        assert_eq!(slice.members, 2);
        assert_eq!(slice.root_members, 1);
        assert_eq!(slice.user_messages, 4);
        assert_eq!(slice.agent_replies, 13);
        assert_eq!(slice.tool_calls, 8);
        assert_eq!(slice.side_activities, 2);
    }
    let ranked = o.top_sessions.iter().find(|s| s.session_id == sa).unwrap();
    assert_eq!(ranked.members, 2);
    assert_eq!(ranked.root_members, 1);
    assert_eq!(ranked.user_messages, 4);
    assert_eq!(ranked.agent_replies, 13);
    assert_eq!(ranked.tool_calls, 8);
    assert_eq!(ranked.side_activities, 2);

    assert_eq!(
        o.top_sessions.len(),
        3,
        "the trashed C ranks first (999999+999999)"
    );
    assert_eq!(o.top_sessions[0].session_id, _sc);
    assert_eq!(o.top_sessions[1].session_id, sa, "A bills 2200 vs B's 2100");
}

#[test]
fn usage_rankings_include_cached_input_in_the_displayed_total() {
    let db = open_db("cached-ranking");
    let (sa, ma) = session(&db, Agent::Codex, "fresh", Some("Fresh"), false);
    let (sb, mb) = session(&db, Agent::Codex, "cached", Some("Cached"), false);
    bill(&db, &ma, 1, Some(100), Some(10), None);
    bill(&db, &mb, 1, Some(1), Some(1), Some(200));
    for (id, session_id) in [("a", &sa), ("b", &sb)] {
        db.write().execute(
            "INSERT INTO projects (id, name, description, name_customized, created_at, updated_at)
             VALUES (?1, ?1, '', 0, '2026-09-28', '2026-09-28')",
            [id],
        ).unwrap();
        db.write().execute(
            "INSERT INTO workstreams (id, title, description, lifecycle, visibility, created_at, updated_at)
             VALUES (?1, ?1, '', 'active', 'normal', '2026-09-28', '2026-09-28')",
            [id],
        ).unwrap();
        db.write()
            .execute(
                "UPDATE sessions SET project_id = ?1, owner_workstream_id = ?1 WHERE id = ?2",
                [id, session_id.as_str()],
            )
            .unwrap();
    }
    commit(
        &db,
        &sa,
        &ma,
        &[event(
            Some("fresh-model"),
            "2026-10-01T08:00:00Z",
            100,
            10,
            "fresh",
        )],
    );
    let mut cached = event(Some("cached-model"), "2026-10-01T08:00:00Z", 1, 1, "cached");
    cached.cached_tokens = 200;
    commit(&db, &sb, &mb, &[cached]);

    let overview = db.usage_overview().unwrap();
    assert_eq!(overview.top_sessions[0].session_id, sb);
    assert_eq!(overview.by_project[0].id, "b");
    assert_eq!(overview.by_workstream[0].id, "b");
    assert_eq!(overview.by_model[0].model, "cached-model");
    // Raw axes stay distinct so the UI can merge once and cache-hit stays unchanged.
    assert_eq!(overview.by_model[0].input_tokens, 1);
    assert_eq!(overview.by_model[0].cached_tokens, 200);
    assert_eq!(overview.by_model[0].cache_hit_rate, Some(200.0 / 201.0));
}

#[test]
fn ledger_sections_come_from_committed_events_and_a_snapshot_replaces() {
    let db = open_db("ledger");
    let (sa, ma) = session(&db, Agent::Codex, "rollout-a", Some("A"), false);
    bill(&db, &ma, 3, Some(1_500), Some(700), None);

    let mut multi = event(Some("GPT-5.6-Luna"), "2026-10-01T08:00:00Z", 900, 100, "k1");
    multi.request_count = 13; // a zcode-shaped turn: 13 real requests
    let written = event(Some("gpt-5.6-luna"), "2026-10-01T09:00:00Z", 100, 50, "k2");
    commit(
        &db,
        &sa,
        &ma,
        &[
            multi,
            written,
            event(None, "2026-10-01T10:00:00Z", 500, 10, "auto-1"),
            // Canonicalization: gateway prefix and mode/tier suffixes fold
            // into the base model; the display keeps the frequent casing.
            event(
                Some("qwen/qwen3.8-27b"),
                "2026-10-01T11:00:00Z",
                700,
                20,
                "k3",
            ),
            event(Some("Qwen3.8-27B"), "2026-10-01T11:30:00Z", 300, 8, "k4"),
            event(
                Some("claude-opus-4-6-thinking"),
                "2026-10-01T12:00:00Z",
                200,
                6,
                "k5",
            ),
            event(
                Some("Claude-Opus-4-6"),
                "2026-10-01T12:30:00Z",
                150,
                4,
                "k6",
            ),
            event(
                Some("gemini-3.8-flash-tiered"),
                "2026-10-01T13:00:00Z",
                90,
                3,
                "k7",
            ),
        ],
    );

    let o = db.usage_overview().unwrap();
    assert_eq!(o.ledger_events, 8);
    assert_eq!(o.attributed_events, 7);
    assert_eq!(o.unattributed_events, 1);
    assert_eq!(
        o.by_model.len(),
        4,
        "luna + qwen (prefix stripped) + opus (-thinking stripped) + flash (-tiered stripped)"
    );
    let m = &o.by_model[0];
    assert_eq!(m.model, "gpt-5.6-luna");
    assert_eq!(
        m.display, "GPT-5.6-Luna",
        "1× title case vs 1× lowercase → tie → smallest wins"
    );
    assert_eq!(m.input_tokens, 1_000);
    assert_eq!(m.events, 2);
    assert_eq!(m.requests, 14, "13 real requests + the 1-request default");
    let qwen = o
        .by_model
        .iter()
        .find(|m| m.model == "qwen3.8-27b")
        .expect("qwen group");
    assert_eq!(qwen.display, "Qwen3.8-27B", "prefix stripped, casing kept");
    assert_eq!(qwen.events, 2, "gateway-prefixed and bare spellings merge");
    let opus = o
        .by_model
        .iter()
        .find(|m| m.model == "claude-opus-4-6")
        .expect("opus group");
    assert_eq!(opus.display, "Claude-Opus-4-6", "-thinking stripped");
    assert_eq!(opus.events, 2);
    assert!(o.by_model.iter().any(|m| m.model == "gemini-3.8-flash"));
    assert!(
        !o.by_model
            .iter()
            .any(|m| m.model.contains("tiered") || m.model.contains("thinking")),
        "mode/tier suffixes never surface as separate groups"
    );
    assert_eq!(
        o.by_category[0].requests, 20,
        "13 + four 1-request defaults (incl. the unattributed one)"
    );
    assert_eq!(o.by_category.len(), 1);
    assert_eq!(o.by_category[0].category, "conversation");
    assert_eq!(o.series.len(), 1);
    assert_eq!(o.series[0].day, "2026-10-01");
    assert_eq!(o.series[0].events, 8);

    // A full re-scan (complete_snapshot) REPLACES the member's rows: the
    // ledger can never disagree with the stats snapshot it accompanies.
    commit(
        &db,
        &sa,
        &ma,
        &[event(
            Some("deepseek-v4.1-flash"),
            "2026-10-02T08:00:00Z",
            42,
            7,
            "k9",
        )],
    );
    let o = db.usage_overview().unwrap();
    assert_eq!(o.ledger_events, 1);
    assert_eq!(o.by_model[0].model, "deepseek-v4.1-flash");

    // An event key repeated inside one batch keeps its first row.
    commit(
        &db,
        &sa,
        &ma,
        &[
            event(Some("m"), "2026-10-03T08:00:00Z", 11, 1, "dup"),
            event(Some("m"), "2026-10-03T09:00:00Z", 22, 2, "dup"),
        ],
    );
    let o = db.usage_overview().unwrap();
    assert_eq!(o.ledger_events, 1, "duplicate key ignored");
    assert_eq!(o.by_model[0].input_tokens, 11);
}

#[test]
fn commit_keeps_the_ledger_in_the_same_transaction_as_the_member() {
    let db = open_db("tx");
    let (sa, _ma) = session(&db, Agent::Codex, "rollout-a", Some("A"), false);
    // Committing for a member that does not belong to the session writes
    // nothing (the batch is dropped whole).
    db.commit_member_ingest_with_provenance_state(
        &sa,
        "no-such-member",
        &[],
        None,
        &cursor(),
        true,
        None,
        None,
        &[event(Some("m"), "2026-10-01T08:00:00Z", 1, 1, "k")],
    )
    .unwrap();
    let n: i64 = db
        .read()
        .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0, "stray member: nothing lands, ledger included");
}

/// An append read (start offset > 0) EXTENDS the ledger; only a read that
/// started at genesis replaces it. `complete_snapshot` must NOT drive the
/// replace — an ordinary clean append also carries it.
#[test]
fn an_append_extends_the_ledger_while_a_genesis_read_replaces_it() {
    let db = open_db("append");
    let (sa, ma) = session(&db, Agent::Codex, "rollout-a", Some("A"), false);

    commit(
        &db,
        &sa,
        &ma,
        &[
            event(Some("m"), "2026-10-01T08:00:00Z", 10, 1, "k1"),
            event(Some("m"), "2026-10-01T09:00:00Z", 20, 2, "k2"),
        ],
    );
    // The append of one more turn.
    db.commit_member_ingest_with_provenance_state(
        &sa,
        &ma,
        &[],
        None,
        &cursor_at(100),
        true,
        None,
        None,
        &[event(Some("m"), "2026-10-01T10:00:00Z", 30, 3, "k3")],
    )
    .unwrap();
    let n: i64 = db
        .read()
        .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 3, "the append kept the earlier events");

    // A genesis read re-emits the whole source and replaces.
    commit(
        &db,
        &sa,
        &ma,
        &[event(Some("m"), "2026-10-02T08:00:00Z", 7, 1, "fresh")],
    );
    let n: i64 = db
        .read()
        .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "the genesis read replaced, not merged");
}

#[test]
fn dimension_requests_follow_event_owners_and_model_participants_are_deduplicated() {
    let db = open_db("dimension-estimates");
    let (sa, ma) = session(&db, Agent::Codex, "a", Some("A"), true);
    let (sb, mb) = session(&db, Agent::Codex, "b", Some("B"), false);
    let (sc, mc) = session(&db, Agent::Qoder, "c", Some("C"), false);
    bill(&db, &ma, 40, Some(100), None, None);
    child(&db, &sa, Agent::Codex, "a-child", 200, 20);
    let child_id: String = db
        .read()
        .query_row(
            "SELECT id FROM session_members WHERE source_member_id = 'a-child'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    // Add a side member with no stats: counts include it, unrecorded token axes stay NULL.
    let side = new_id();
    db.write()
        .execute(
            "INSERT INTO session_members
         (id, session_id, agent, source_member_id, relation, source_kind, source_path, metadata)
         VALUES (?1, ?2, 'codex', 'a-side', 'side', 'test', '/tmp/side', '{}')",
            rusqlite::params![side, sa],
        )
        .unwrap();
    for (project, task, owner) in [
        ("p-a", "w-a", &sa),
        ("p-b", "w-b", &sb),
        ("p-c", "w-c", &sc),
    ] {
        db.write().execute(
            "INSERT INTO projects (id, name, description, name_customized, created_at, updated_at)
             VALUES (?1, ?1, '', 0, '2026-10-01', '2026-10-01')", [project],
        ).unwrap();
        db.write().execute(
            "INSERT INTO workstreams (id, title, description, lifecycle, visibility, created_at, updated_at)
             VALUES (?1, ?1, '', 'active', 'normal', '2026-10-01', '2026-10-01')", [task],
        ).unwrap();
        db.write()
            .execute(
                "UPDATE sessions SET project_id = ?1, owner_workstream_id = ?2 WHERE id = ?3",
                rusqlite::params![project, task, owner],
            )
            .unwrap();
    }
    commit(
        &db,
        &sa,
        &ma,
        &[
            event(Some("GPT-Test"), "2026-10-01T00:00:00Z", 1_000_000, 0, "a1"),
            event(
                Some("gateway/gpt-test-thinking"),
                "2026-10-01T00:00:00Z",
                2_000_000,
                0,
                "a2",
            ),
            event(Some("unknown"), "2026-10-01T00:00:00Z", 9_000_000, 0, "a3"),
            event(None, "2026-10-01T00:00:00Z", 8_000_000, 0, "a4"),
        ],
    );
    let mut ev = event(
        Some("gpt-test"),
        "2026-10-01T00:00:00Z",
        500_000,
        250_000,
        "child",
    );
    ev.request_count = 4;
    ev.cached_tokens = 1_000_000;
    commit(&db, &sa, &child_id, &[ev]);
    commit(
        &db,
        &sb,
        &mb,
        &[event(
            Some("free"),
            "2026-10-01T00:00:00Z",
            1_000_000,
            0,
            "b1",
        )],
    );
    commit(
        &db,
        &sc,
        &mc,
        &[event(
            Some("unknown"),
            "2026-10-01T00:00:00Z",
            1_000_000,
            0,
            "c1",
        )],
    );
    let o = db.usage_overview().unwrap();
    assert_eq!(o.ledger_events, 7);
    let close =
        |actual: Option<f64>, expected: f64| assert!((actual.unwrap() - expected).abs() < 1e-10);
    // The same ledger population supplies numerator and denominator; include
    // unattributed calls instead of averaging model percentages.
    close(o.cache_hit_rate, 1.0 / 23.5);
    assert_eq!(
        o.requests, 10,
        "includes unknown models and unassigned calls, plus the child's four requests"
    );
    assert_eq!(o.by_category.iter().map(|s| s.requests).sum::<i64>(), 10);
    let codex = o.by_agent.iter().find(|s| s.agent == "codex").unwrap();
    assert_eq!(codex.members, 4);
    assert_eq!(codex.requests, 9);
    close(codex.cache_hit_rate, 1.0 / 22.5);
    let qoder = o.by_agent.iter().find(|s| s.agent == "qoder").unwrap();
    assert_eq!(qoder.requests, 1, "unknown model still counts");
    close(qoder.cache_hit_rate, 0.0);
    for (slices, aid, bid, cid) in [
        (&o.by_project, "p-a", "p-b", "p-c"),
        (&o.by_workstream, "w-a", "w-b", "w-c"),
    ] {
        let a = slices.iter().find(|s| s.id == aid).unwrap();
        assert_eq!(a.members, 3);
        close(a.cache_hit_rate, 1.0 / 21.5);
        close(
            slices.iter().find(|s| s.id == bid).unwrap().cache_hit_rate,
            0.0,
        );
        close(
            slices.iter().find(|s| s.id == cid).unwrap().cache_hit_rate,
            0.0,
        );
        assert_eq!(a.requests, 8, "includes the trashed root and its child");
        assert_eq!(slices.iter().find(|s| s.id == bid).unwrap().requests, 1);
        assert_eq!(slices.iter().find(|s| s.id == cid).unwrap().requests, 1);
        assert_eq!(a.root_members, 1);
        assert_eq!(
            a.agent_replies, 43,
            "multiple ledger rows must not multiply snapshot counts"
        );
    }
    let root = o.by_relation.iter().find(|s| s.relation == "root").unwrap();
    let child = o
        .by_relation
        .iter()
        .find(|s| s.relation == "child")
        .unwrap();
    let side = o.by_relation.iter().find(|s| s.relation == "side").unwrap();
    close(root.cache_hit_rate, 0.0);
    close(child.cache_hit_rate, 2.0 / 3.0);
    assert_eq!(side.cache_hit_rate, None);
    assert_eq!(root.requests, 6);
    assert_eq!(child.requests, 4);
    assert_eq!(
        side.requests, 0,
        "no ledger must not infer calls from activities"
    );
    assert_eq!(side.members, 1);
    assert_eq!(side.root_members, 0);
    assert_eq!(side.agent_replies, 0);
    assert_eq!(side.input_tokens, None);
    assert_eq!(
        o.top_sessions
            .iter()
            .find(|s| s.session_id == sa)
            .unwrap()
            .requests,
        8
    );
    close(
        o.top_sessions
            .iter()
            .find(|s| s.session_id == sa)
            .unwrap()
            .cache_hit_rate,
        1.0 / 21.5,
    );
    let model = o.by_model.iter().find(|s| s.model == "gpt-test").unwrap();
    assert_eq!(model.events, 3);
    close(model.cache_hit_rate, 1.0 / 4.5);
    assert_eq!(
        model.requests, 6,
        "canonical aliases sum requests without multiplying participants"
    );
    assert_eq!(
        model.members, 2,
        "canonical aliases count the same root only once"
    );
    assert_eq!(model.root_members, 1);
    assert_eq!(model.user_messages, None);
    assert_eq!(
        model.agent_replies, None,
        "billing events are not assistant replies"
    );
    assert_eq!(model.tool_calls, None);
    assert_eq!(model.side_activities, None);
    assert!(o.by_model.iter().map(|s| s.members).sum::<i64>() > 0);
    let serialized = serde_json::to_value(&o).unwrap();
    assert_eq!(serialized["by_project"][0]["members"], 3);
    assert_eq!(serialized["by_project"][0]["requests"], 8);
}

#[test]
fn model_participation_and_requests_merge_channels_and_replayed_calls() {
    let db = open_db("model-participants");
    let (sa, ma) = session(&db, Agent::Codex, "a", Some("A"), false);
    child(&db, &sa, Agent::Codex, "child", 10, 10);
    let child_id: String = db
        .read()
        .query_row(
            "SELECT id FROM session_members WHERE source_member_id='child'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    commit(
        &db,
        &sa,
        &ma,
        &[event(
            Some("GPT-Test"),
            "2026-10-01T00:00:00Z",
            100,
            10,
            "root",
        )],
    );
    let mut child_event = event(
        Some("gpt-test-thinking"),
        "2026-10-01T00:00:00Z",
        200,
        20,
        "child",
    );
    child_event.provider = Some("302ai".into());
    commit(&db, &sa, &child_id, &[child_event.clone(), child_event]);
    let o = db.usage_overview().unwrap();
    assert_eq!(o.by_model.len(), 1);
    assert_eq!(o.requests, 2, "a repeated billing key must not add calls");
    assert_eq!(o.by_model[0].requests, 2);
    assert_eq!(o.by_model[0].members, 2);
    assert_eq!(o.by_model[0].root_members, 1);
    assert_eq!(o.by_model[0].input_tokens, 300);
    assert_eq!(o.by_model[0].output_tokens, 30);
    let json = serde_json::to_value(&o).unwrap();
    for key in [
        "cost",
        "cost_unit",
        "cost_estimate",
        "cache_write_tokens",
        "pricing_updated_at",
        "prices",
    ] {
        assert!(!json.as_object().unwrap().contains_key(key));
        for section in [
            "by_agent",
            "by_relation",
            "by_project",
            "by_workstream",
            "by_model",
            "by_category",
            "top_sessions",
        ] {
            for row in json[section].as_array().unwrap() {
                assert!(!row.as_object().unwrap().contains_key(key));
            }
        }
    }
}

#[test]
fn cache_hit_rate_handles_all_cached_input_zero_input_and_replacement() {
    let db = open_db("cache-hit-boundaries");
    let (sid, mid) = session(&db, Agent::Codex, "a", Some("A"), false);
    let mut all_cached = event(Some("gpt-test"), "2026-10-01T00:00:00Z", 0, 10, "cached");
    all_cached.cached_tokens = 100;
    commit(&db, &sid, &mid, &[all_cached]);
    let o = db.usage_overview().unwrap();
    assert_eq!(o.cache_hit_rate, Some(1.0));
    assert_eq!(o.by_model[0].cache_hit_rate, Some(1.0));
    assert_eq!(o.by_agent[0].cache_hit_rate, Some(1.0));
    commit(
        &db,
        &sid,
        &mid,
        &[event(
            Some("gpt-test"),
            "2026-10-01T00:00:00Z",
            0,
            10,
            "output-only",
        )],
    );
    let o = db.usage_overview().unwrap();
    assert_eq!(o.cache_hit_rate, None);
    assert_eq!(o.by_model[0].cache_hit_rate, None);
    assert_eq!(o.top_sessions[0].cache_hit_rate, None);
}
