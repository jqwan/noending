//! Collaboration room storage: atomic start/stop, membership validation, the
//! shared message sequence, per-member reads and deletion semantics.
use noending_lib::storage::collaboration::SelectedMember;
use noending_lib::storage::Db;
use std::path::PathBuf;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("ne-collab-{}", noending_lib::storage::new_id())))
    }
    /// Task `t` with member sessions a/b/c and one unrelated session `x`.
    fn db(&self) -> Db {
        let db = Db::open(&self.0.join("noending.db")).unwrap();
        db.write()
            .execute_batch(
                "INSERT INTO workstreams(id,title,created_at,updated_at) VALUES('t','协作任务','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z');
                 INSERT INTO sessions(id,agent,root_agent_session_id,source_kind,source_path) VALUES
                   ('a','codex','na','jsonl','/a'),('b','codex','nb','jsonl','/b'),
                   ('c','codex','nc','jsonl','/c'),('x','codex','nx','jsonl','/x'),
                   ('archived','codex','narch','jsonl','/arch');
                 INSERT INTO session_workstreams(session_id,workstream_id) VALUES
                   ('a','t'),('b','t'),('c','t'),('archived','t');
                 UPDATE sessions SET archived_at='2026-01-01T00:00:00Z' WHERE id='archived';",
            )
            .unwrap();
        db
    }
    fn members(first: &str, second: &str) -> Vec<SelectedMember> {
        vec![
            SelectedMember {
                session_id: first.into(),
                wake_method: "desktop".into(),
            },
            SelectedMember {
                session_id: second.into(),
                wake_method: "desktop".into(),
            },
        ]
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn start_validates_membership_and_persists_notice_and_first_message_atomically() {
    let f = Fixture::new();
    let db = f.db();
    // Fewer than two members.
    assert!(db
        .collaboration_start("t", &Fixture::members("a", "a"), None, None, None, "r1")
        .is_err());
    // Duplicate member sessions.
    let duplicate = vec![
        SelectedMember {
            session_id: "a".into(),
            wake_method: "desktop".into(),
        },
        SelectedMember {
            session_id: "a".into(),
            wake_method: "desktop".into(),
        },
    ];
    assert!(db
        .collaboration_start("t", &duplicate, None, None, None, "r1")
        .is_err());
    // A member outside the task.
    assert!(db
        .collaboration_start("t", &Fixture::members("a", "x"), None, None, None, "r1")
        .is_err());
    // An archived member.
    assert!(db
        .collaboration_start(
            "t",
            &Fixture::members("a", "archived"),
            None,
            None,
            None,
            "r1"
        )
        .is_err());
    // A coordinator outside the members.
    assert!(db
        .collaboration_start(
            "t",
            &Fixture::members("a", "b"),
            Some("c"),
            None,
            None,
            "r1"
        )
        .is_err());

    let outcome = db
        .collaboration_start(
            "t",
            &Fixture::members("a", "b"),
            Some("a"),
            None,
            Some("先按方案开始"),
            "r1",
        )
        .unwrap();
    assert_eq!(outcome.run.status, "starting");
    assert_eq!(outcome.room.revision, 1);
    assert_eq!(outcome.members.len(), 2);
    assert_eq!(outcome.appended.len(), 2);
    assert_eq!(outcome.appended[0].kind, "start");
    assert!(outcome.appended[0].sender_session_id.is_none());
    assert_eq!(outcome.appended[1].kind, "text");
    assert_eq!(outcome.appended[1].body, "先按方案开始");

    // The room exists exactly once per task and the run is its current one.
    let (room, run) = db.collaboration_room_by_task("t").unwrap().unwrap();
    assert_eq!(room.id, outcome.room.id);
    assert_eq!(
        run.as_ref().map(|r| r.id.as_str()),
        Some(outcome.run.id.as_str())
    );

    // A second in-progress start on the same room is rejected…
    let error = db
        .collaboration_start("t", &Fixture::members("a", "b"), None, None, None, "r2")
        .unwrap_err();
    assert!(error.to_string().contains("不能重复开启"), "{error}");

    // …and a stale expected revision is rejected even when the room is idle.
    db.collaboration_stop(&outcome.room.id, &outcome.run.id, None, "s0")
        .unwrap();
    let error = db
        .collaboration_start(
            "t",
            &Fixture::members("a", "b"),
            None,
            Some(99),
            None,
            "r2b",
        )
        .unwrap_err();
    assert!(error.to_string().contains("已被其他人修改"), "{error}");
}

#[test]
fn repeated_start_request_returns_the_existing_run_without_appending() {
    let f = Fixture::new();
    let db = f.db();
    let first = db
        .collaboration_start(
            "t",
            &Fixture::members("a", "b"),
            None,
            None,
            Some("msg"),
            "same",
        )
        .unwrap();
    let second = db
        .collaboration_start(
            "t",
            &Fixture::members("a", "b"),
            None,
            None,
            Some("msg"),
            "same",
        )
        .unwrap();
    assert_eq!(first.run.id, second.run.id);
    assert!(second.appended.is_empty());
    assert_eq!(first.appended.len(), 2);
}

#[test]
fn stop_is_atomic_records_the_real_initiator_and_stays_idempotent() {
    let f = Fixture::new();
    let db = f.db();
    let started = db
        .collaboration_start(
            "t",
            &Fixture::members("a", "b"),
            Some("a"),
            None,
            None,
            "r1",
        )
        .unwrap();
    let run_id = started.run.id.clone();
    let room_id = started.room.id.clone();

    let stopped = db
        .collaboration_stop(&room_id, &run_id, Some("a"), "s1")
        .unwrap();
    assert_eq!(stopped.0.status, "stopped");
    assert_eq!(stopped.0.stop_initiator_session_id.as_deref(), Some("a"));
    assert_eq!(stopped.1.kind, "stop");
    assert_eq!(stopped.1.sender_session_id.as_deref(), Some("a"));
    let latest = db
        .collaboration_messages_page(&room_id, None, None, 50)
        .unwrap();
    let kinds: Vec<&str> = latest.items.iter().map(|m| m.kind.as_str()).collect();
    assert_eq!(kinds, vec!["start", "stop"]);

    // A different stop request on a stopped run is rejected…
    assert!(db
        .collaboration_stop(&room_id, &run_id, None, "s2")
        .is_err());
    // …while the same stop request replays the original result.
    let replay = db
        .collaboration_stop(&room_id, &run_id, Some("a"), "s1")
        .unwrap();
    assert_eq!(replay.0.id, run_id);
    assert_eq!(replay.1.id, stopped.1.id);
    assert_eq!(
        db.collaboration_messages_page(&room_id, None, None, 50)
            .unwrap()
            .items
            .len(),
        2
    );

    // Stopped sessions free up: a new run can open on the same room.
    let restarted = db
        .collaboration_start("t", &Fixture::members("a", "b"), None, None, None, "r2")
        .unwrap();
    assert_eq!(restarted.room.revision, 2);
    assert_ne!(restarted.run.id, run_id);
    let page = db
        .collaboration_messages_page(&room_id, Some(&restarted.run.id), None, 50)
        .unwrap();
    assert_eq!(
        page.items.len(),
        1,
        "new run only sees its own start notice"
    );
}

#[test]
fn send_requires_membership_accepts_starting_and_deduplicates_requests() {
    let f = Fixture::new();
    let db = f.db();
    let started = db
        .collaboration_start("t", &Fixture::members("a", "b"), None, None, None, "r1")
        .unwrap();
    let run_id = started.run.id.clone();
    let room_id = started.room.id.clone();

    let member = db
        .collaboration_send(&room_id, &run_id, Some("a"), "成员发言", "m1")
        .unwrap();
    assert_eq!(member.seq, 2, "start notice is seq 1");
    let user = db
        .collaboration_send(&room_id, &run_id, None, "用户发言", "m2")
        .unwrap();
    assert!(user.sender_session_id.is_none());

    // A session outside the run cannot write.
    assert!(db
        .collaboration_send(&room_id, &run_id, Some("c"), "外人", "m3")
        .is_err());
    assert!(db
        .collaboration_send(&room_id, &run_id, Some("a"), "  ", "m4")
        .is_err());

    let replay = db
        .collaboration_send(&room_id, &run_id, Some("a"), "成员发言", "m1")
        .unwrap();
    assert_eq!(replay.id, member.id);
    assert_eq!(
        db.collaboration_messages_page(&room_id, None, None, 50)
            .unwrap()
            .items
            .len(),
        3,
        "start notice + member message + user message"
    );

    db.collaboration_stop(&room_id, &run_id, None, "s1")
        .unwrap();
    let error = db
        .collaboration_send(&room_id, &run_id, Some("a"), "停后发言", "m5")
        .unwrap_err();
    assert!(error.to_string().contains("已停止"), "{error}");
}

#[test]
fn messages_page_walks_back_and_forward_without_overlap_or_cross_room() {
    let f = Fixture::new();
    let db = f.db();
    let started = db
        .collaboration_start("t", &Fixture::members("a", "b"), None, None, None, "r1")
        .unwrap();
    let room_id = started.room.id.clone();
    let run_id = started.run.id.clone();
    for i in 0..5 {
        db.collaboration_send(
            &room_id,
            &run_id,
            Some("a"),
            &format!("m{i}"),
            &format!("m{i}"),
        )
        .unwrap();
    }

    let newest = db
        .collaboration_messages_page(&room_id, None, None, 3)
        .unwrap();
    let seqs: Vec<i64> = newest.items.iter().map(|m| m.seq).collect();
    assert_eq!(seqs, vec![4, 5, 6], "newest page arrives ascending");
    assert_eq!(newest.latest_seq, 6);

    let back = db
        .collaboration_messages_page(&room_id, None, newest.next_cursor.as_deref(), 3)
        .unwrap();
    let back_seqs: Vec<i64> = back.items.iter().map(|m| m.seq).collect();
    assert_eq!(back_seqs, vec![1, 2, 3]);
    assert!(back.next_cursor.is_none(), "reached the beginning");

    // A run-scoped cursor must not be spent on the all-history scope.
    let scoped = db
        .collaboration_messages_page(&room_id, Some(&run_id), None, 2)
        .unwrap();
    assert_eq!(scoped.items.len(), 2);
    let wrong_scope = db
        .collaboration_messages_page(&room_id, None, scoped.next_cursor.as_deref(), 2)
        .unwrap_err();
    assert!(
        wrong_scope.to_string().contains("运行范围"),
        "{wrong_scope}"
    );
    let same_scope = db
        .collaboration_messages_page(&room_id, Some(&run_id), scoped.next_cursor.as_deref(), 2)
        .unwrap();
    assert_eq!(same_scope.items.len(), 2);

    let forward = db.collaboration_messages_page(&room_id, None, Some("__bad__"), 3);
    assert!(forward.is_err(), "an unknown cursor shape is rejected");

    // A cursor minted for another room never resolves here. The cursor format
    // is base64 JSON carrying its room; craft one for a foreign room id.
    use base64::Engine as _;
    let foreign_payload =
        serde_json::json!({"room":"another-room","dir":"back","seq":2}).to_string();
    let foreign_cursor =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(foreign_payload.as_bytes());
    let rejected = db.collaboration_messages_page(&room_id, None, Some(&foreign_cursor), 3);
    assert!(rejected.is_err(), "{rejected:?}");
    let _ = run_id;
}

#[test]
fn ack_tracks_positions_per_member_and_rejects_out_of_range_or_backward() {
    let f = Fixture::new();
    let db = f.db();
    let started = db
        .collaboration_start("t", &Fixture::members("a", "b"), None, None, None, "r1")
        .unwrap();
    let room_id = started.room.id.clone();
    let run_id = started.run.id.clone();
    db.collaboration_send(&room_id, &run_id, Some("a"), "hello", "m1")
        .unwrap();

    // Before any ack, the member's unread starts at the run's start notice.
    let unread = db.collaboration_unread_of(&room_id, &run_id, "b").unwrap();
    assert_eq!(unread.count, 2, "start notice + member message");

    let cutoff = (chrono::Utc::now() - chrono::Duration::seconds(30)).to_rfc3339();
    // Without a live participation the ack is rejected.
    assert!(db.collaboration_ack(&room_id, &run_id, "b", 1).is_err());
    db.collaboration_join(&room_id, &run_id, "b", "cli", "cli-resident", &cutoff)
        .unwrap();
    db.collaboration_ack(&room_id, &run_id, "b", 1).unwrap();
    let unread = db.collaboration_unread_of(&room_id, &run_id, "b").unwrap();
    assert_eq!(unread.count, 1);
    let read = db
        .collaboration_read_of(&room_id, "b", &run_id)
        .unwrap()
        .unwrap();
    assert_eq!(read.through_seq, 1);

    // Acknowledgement only moves forward.
    db.collaboration_ack(&room_id, &run_id, "b", 0).unwrap();
    assert_eq!(
        db.collaboration_read_of(&room_id, "b", &run_id)
            .unwrap()
            .unwrap()
            .through_seq,
        1
    );
    assert!(db.collaboration_ack(&room_id, &run_id, "b", 99).is_err());
    // A member without reads still starts at the run boundary. Own messages
    // count as unread but never make the member look unresponsive; the user's
    // start notice does.
    let unread = db.collaboration_unread_of(&room_id, &run_id, "a").unwrap();
    assert_eq!(
        unread.count, 1,
        "own messages are not unread; the user's notice is"
    );
    assert!(
        unread.awaiting_since.is_some(),
        "the user's start notice is a pending foreign message"
    );
    db.collaboration_send(&room_id, &run_id, Some("b"), "来自同伴", "m2")
        .unwrap();
    let unread = db.collaboration_unread_of(&room_id, &run_id, "a").unwrap();
    assert!(
        unread.awaiting_since.is_some(),
        "a peer message sets the timer"
    );
    let mcp_instance = noending_lib::storage::new_id();
    db.collaboration_join(&room_id, &run_id, "a", "mcp", &mcp_instance, &cutoff)
        .unwrap();
    db.collaboration_ack(&room_id, &run_id, "a", 3).unwrap();
    let unread = db.collaboration_unread_of(&room_id, &run_id, "a").unwrap();
    assert!(unread.awaiting_since.is_none(), "everything acknowledged");
}

#[test]
fn session_purge_keeps_real_senders_but_drops_participation_state() {
    let f = Fixture::new();
    let db = f.db();
    let started = db
        .collaboration_start("t", &Fixture::members("a", "b"), None, None, None, "r1")
        .unwrap();
    let room_id = started.room.id.clone();
    let run_id = started.run.id.clone();
    db.collaboration_send(&room_id, &run_id, Some("a"), "来自将删除的会话", "m1")
        .unwrap();
    let cutoff = (chrono::Utc::now() - chrono::Duration::seconds(30)).to_rfc3339();
    db.collaboration_join(&room_id, &run_id, "a", "cli", "cli-resident", &cutoff)
        .unwrap();
    db.collaboration_ack(&room_id, &run_id, "a", 2).unwrap();

    // Simulate the purge path: the session row goes away, everything keyed to
    // it cascades, but shared messages keep the real sender id.
    db.write()
        .execute("DELETE FROM sessions WHERE id='a'", [])
        .unwrap();
    let page = db
        .collaboration_messages_page(&room_id, None, None, 50)
        .unwrap();
    let member_mail = page
        .items
        .iter()
        .find(|m| m.body == "来自将删除的会话")
        .unwrap();
    assert_eq!(member_mail.sender_session_id.as_deref(), Some("a"));
    assert!(db
        .collaboration_read_of(&room_id, "a", &run_id)
        .unwrap()
        .is_none());
    let members = db.collaboration_run_members(&run_id).unwrap();
    assert_eq!(members.len(), 1, "the run member row cascaded away");
}

#[test]
fn candidate_sessions_are_task_scoped_stably_paged_and_include_archived() {
    let f = Fixture::new();
    let db = f.db();
    let first = db.collaboration_candidate_sessions("t", None, 2).unwrap();
    assert_eq!(first.items, vec!["a", "archived"]);
    assert_eq!(first.next_cursor.as_deref(), Some("archived"));
    let second = db
        .collaboration_candidate_sessions("t", first.next_cursor.as_deref(), 2)
        .unwrap();
    // Archived sessions stay listed so the UI can grey them out with a reason.
    assert_eq!(second.items, vec!["b", "c"]);
    assert!(second.next_cursor.is_none());
    let unrelated = db
        .collaboration_candidate_sessions("missing", None, 10)
        .unwrap();
    assert!(unrelated.items.is_empty());
}

#[test]
fn coordinator_swap_guards_revision_is_idempotent_and_rejects_reused_ids() {
    let f = Fixture::new();
    let db = f.db();
    let started = db
        .collaboration_start(
            "t",
            &Fixture::members("a", "b"),
            Some("a"),
            None,
            None,
            "r1",
        )
        .unwrap();
    let room_id = started.room.id.clone();
    let run_id = started.run.id.clone();

    // The run fence is the caller's job (business layer); the ledger proves
    // replay identity here.
    let swapped = db
        .collaboration_set_coordinator(&room_id, &run_id, Some("b"), 1, "swap-1")
        .unwrap();
    assert_eq!(swapped.coordinator_session_id.as_deref(), Some("b"));
    assert_eq!(swapped.revision, 2);

    // Same request id, same arguments: idempotent even with a stale revision.
    let replay = db
        .collaboration_set_coordinator(&room_id, &run_id, Some("b"), 1, "swap-1")
        .unwrap();
    assert_eq!(replay.revision, 2);

    // Same request id, different target: rejected instead of silently reused.
    let conflict = db
        .collaboration_set_coordinator(&room_id, &run_id, Some("a"), 2, "swap-1")
        .unwrap_err();
    assert!(
        conflict.to_string().contains("已被用于不同的负责人操作"),
        "{conflict}"
    );

    // A coordinator outside the members is rejected.
    let outsider = db
        .collaboration_set_coordinator(&room_id, &run_id, Some("x"), 2, "swap-2")
        .unwrap_err();
    assert!(
        outsider.to_string().contains("负责人必须来自所选成员"),
        "{outsider}"
    );

    // A stale revision is rejected; the request id stays unused for retries.
    let stale = db
        .collaboration_set_coordinator(&room_id, &run_id, Some("a"), 1, "swap-3")
        .unwrap_err();
    assert!(stale.to_string().contains("已被其他人修改"), "{stale}");
    let retried = db
        .collaboration_set_coordinator(&room_id, &run_id, Some("a"), 2, "swap-3")
        .unwrap();
    assert_eq!(retried.coordinator_session_id.as_deref(), Some("a"));
}

#[test]
fn start_rejects_a_request_id_reused_for_a_different_configuration() {
    let f = Fixture::new();
    let db = f.db();
    let started = db
        .collaboration_start("t", &Fixture::members("a", "b"), None, None, None, "r1")
        .unwrap();
    // Same id, different task: mismatch instead of a foreign room answer.
    db.write()
        .execute_batch(
            "INSERT INTO workstreams(id,title,created_at,updated_at) VALUES('t2','第二任务','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z');
             INSERT INTO session_workstreams(session_id,workstream_id) VALUES ('a','t2'),('b','t2');",
        )
        .unwrap();
    let conflict = db
        .collaboration_start("t2", &Fixture::members("a", "b"), None, None, None, "r1")
        .unwrap_err();
    assert!(
        conflict.to_string().contains("已被用于不同的操作参数"),
        "{conflict}"
    );
    // Same id, different members: mismatch as well.
    let conflict = db
        .collaboration_start("t", &Fixture::members("a", "c"), None, None, None, "r1")
        .unwrap_err();
    assert!(
        conflict.to_string().contains("已被用于不同的操作参数"),
        "{conflict}"
    );
    assert_eq!(
        db.collaboration_messages_page(&started.room.id, None, None, 50)
            .unwrap()
            .items
            .len(),
        1,
        "conflicting replays never append"
    );
}

#[test]
fn join_claims_atomically_supersedes_old_connections_and_leaves_idempotently() {
    let f = Fixture::new();
    let db = f.db();
    let started = db
        .collaboration_start(
            "t",
            &Fixture::members("a", "b"),
            Some("a"),
            None,
            None,
            "r1",
        )
        .unwrap();
    let room_id = started.room.id.clone();
    let run_id = started.run.id.clone();

    let cutoff = (chrono::Utc::now() - chrono::Duration::seconds(30)).to_rfc3339();
    let first = db
        .collaboration_join(&room_id, &run_id, "a", "cli", "cli-resident", &cutoff)
        .unwrap();
    assert_eq!(first.role.as_deref(), Some("a"));
    assert_eq!(
        first.through_seq, 0,
        "start notice begins the processing range"
    );

    // A fresh lease is reused across a mode switch: same connection id, no
    // second owner.
    // A fresh lease is reused across a mode switch: same connection id, no
    // second owner. (Same holder instance only.)
    let mcp_instance = noending_lib::storage::new_id();
    let second = db
        .collaboration_join(&room_id, &run_id, "a", "mcp", &mcp_instance, &cutoff)
        .unwrap();
    assert!(!second.reused, "a different MCP instance takes over");
    assert_ne!(second.connection_id, first.connection_id);
    assert_eq!(
        db.collaboration_renew(&second.connection_id).unwrap(),
        noending_lib::storage::collaboration::RenewOutcome::Renewed
    );

    // The same instance rejoining reuses its own fresh lease.
    let rejoined = db
        .collaboration_join(&room_id, &run_id, "a", "mcp", &mcp_instance, &cutoff)
        .unwrap();
    assert!(rejoined.reused);
    assert_eq!(rejoined.connection_id, second.connection_id);
    assert_eq!(
        db.collaboration_renew(&first.connection_id).unwrap(),
        noending_lib::storage::collaboration::RenewOutcome::Gone,
        "the takeover replaced the whole row, so the CLI id is gone"
    );

    // A stale lease (heartbeat outside the window) is replaced atomically and
    // the replaced connection's row is gone.
    db.write()
        .execute(
            "UPDATE collaboration_participations SET last_heartbeat_at=?1",
            [(chrono::Utc::now() - chrono::Duration::seconds(60)).to_rfc3339()],
        )
        .unwrap();
    let takeover = db
        .collaboration_join(&room_id, &run_id, "a", "mcp", &mcp_instance, &cutoff)
        .unwrap();
    assert!(!takeover.reused);
    assert_ne!(takeover.connection_id, first.connection_id);
    assert_eq!(
        db.collaboration_renew(&first.connection_id).unwrap(),
        noending_lib::storage::collaboration::RenewOutcome::Gone
    );
    assert_eq!(
        db.collaboration_renew(&takeover.connection_id).unwrap(),
        noending_lib::storage::collaboration::RenewOutcome::Renewed
    );

    // Phase transitions ride on the current connection only.
    db.collaboration_set_phase(&room_id, &run_id, "a", Some("processing"))
        .unwrap();
    let participations = db.collaboration_participations_for_run(&run_id).unwrap();
    assert_eq!(participations[0].phase.as_deref(), Some("processing"));

    // Leave is idempotent and ends renewal for the leaver only.
    db.collaboration_leave(&room_id, &run_id, "a").unwrap();
    db.collaboration_leave(&room_id, &run_id, "a").unwrap();
    let participations = db.collaboration_participations_for_run(&run_id).unwrap();
    assert!(participations[0].left_at.is_some());
    assert!(participations[0].phase.is_none());
    assert_eq!(
        db.collaboration_renew(&takeover.connection_id).unwrap(),
        noending_lib::storage::collaboration::RenewOutcome::Unowned
    );

    // A non-member can never join.
    assert!(db
        .collaboration_join(&room_id, &run_id, "x", "cli", "cli-resident", &cutoff)
        .is_err());

    // A deleted participation (cascade) reports Gone instead of renewing.
    db.collaboration_join(&room_id, &run_id, "a", "cli", "cli-resident", &cutoff)
        .unwrap();
    db.write()
        .execute("DELETE FROM collaboration_participations", [])
        .unwrap();
    let third = db
        .collaboration_join(&room_id, &run_id, "a", "cli", "cli-resident", &cutoff)
        .unwrap();
    db.write()
        .execute("DELETE FROM sessions WHERE id='a'", [])
        .unwrap();
    assert_eq!(
        db.collaboration_renew(&third.connection_id).unwrap(),
        noending_lib::storage::collaboration::RenewOutcome::Gone
    );
}

#[test]
fn wait_page_reads_forward_from_the_acknowledged_position() {
    let f = Fixture::new();
    let db = f.db();
    let started = db
        .collaboration_start("t", &Fixture::members("a", "b"), None, None, None, "r1")
        .unwrap();
    let room_id = started.room.id.clone();
    let run_id = started.run.id.clone();
    for i in 0..3 {
        db.collaboration_send(
            &room_id,
            &run_id,
            Some("a"),
            &format!("m{i}"),
            &format!("m{i}"),
        )
        .unwrap();
    }

    // No ack yet: the batch starts at the run's start notice.
    let page = db
        .collaboration_wait_page(&room_id, &run_id, "b", 2)
        .unwrap();
    let seqs: Vec<i64> = page.items.iter().map(|m| m.seq).collect();
    assert_eq!(seqs, vec![1, 2]);
    assert!(page.next_cursor.is_some());

    let cutoff = (chrono::Utc::now() - chrono::Duration::seconds(30)).to_rfc3339();
    db.collaboration_join(&room_id, &run_id, "b", "cli", "cli-resident", &cutoff)
        .unwrap();
    db.collaboration_ack(&room_id, &run_id, "b", 2).unwrap();
    let page = db
        .collaboration_wait_page(&room_id, &run_id, "b", 2)
        .unwrap();
    let seqs: Vec<i64> = page.items.iter().map(|m| m.seq).collect();
    assert_eq!(seqs, vec![3, 4]);
    assert!(page.next_cursor.is_none(), "everything delivered");
}

#[test]
fn review_same_message_request_cannot_change_body_or_sender() {
    let f = Fixture::new();
    let db = f.db();
    let started = db
        .collaboration_start(
            "t",
            &Fixture::members("a", "b"),
            None,
            None,
            None,
            "review-start",
        )
        .unwrap();
    let room = &started.room.id;
    let run = &started.run.id;
    let saved = db
        .collaboration_send(room, run, Some("a"), "原始正文", "review-message")
        .unwrap();
    assert_eq!(saved.body, "原始正文");
    assert!(db
        .collaboration_send(room, run, Some("a"), "修改正文", "review-message")
        .is_err());
    assert!(db
        .collaboration_send(room, run, Some("b"), "原始正文", "review-message")
        .is_err());
    let replay = db
        .collaboration_send(room, run, Some("a"), "原始正文", "review-message")
        .unwrap();
    assert_eq!(replay.id, saved.id);
}

#[test]
fn review_history_cursor_cannot_change_its_run_scope() {
    let f = Fixture::new();
    let db = f.db();
    let first = db
        .collaboration_start(
            "t",
            &Fixture::members("a", "b"),
            None,
            None,
            Some("本轮首信"),
            "review-first",
        )
        .unwrap();
    let page = db
        .collaboration_messages_page(&first.room.id, Some(&first.run.id), None, 1)
        .unwrap();
    assert_eq!(page.items[0].body, "本轮首信");
    let cursor = page.next_cursor.as_deref().unwrap();
    let older = db
        .collaboration_messages_page(&first.room.id, Some(&first.run.id), Some(cursor), 1)
        .unwrap();
    assert_eq!(older.items[0].kind, "start");
    assert!(db
        .collaboration_messages_page(&first.room.id, None, Some(cursor), 1)
        .is_err());
    db.collaboration_stop(&first.room.id, &first.run.id, None, "review-stop")
        .unwrap();
    let second = db
        .collaboration_start(
            "t",
            &Fixture::members("a", "b"),
            None,
            None,
            None,
            "review-second",
        )
        .unwrap();
    assert!(db
        .collaboration_messages_page(&first.room.id, Some(&second.run.id), Some(cursor), 1)
        .is_err());
}

#[test]
fn review_own_messages_do_not_wake_sender_or_skip_other_messages() {
    let f = Fixture::new();
    let db = f.db();
    let started = db
        .collaboration_start(
            "t",
            &Fixture::members("a", "b"),
            None,
            None,
            None,
            "review-reads",
        )
        .unwrap();
    let room = &started.room.id;
    let run = &started.run.id;
    let cutoff = (chrono::Utc::now() - chrono::Duration::seconds(30)).to_rfc3339();
    db.collaboration_join(room, run, "a", "cli", "cli-resident", &cutoff)
        .unwrap();
    db.collaboration_join(room, run, "b", "cli", "cli-resident", &cutoff)
        .unwrap();
    db.collaboration_ack(room, run, "a", 1).unwrap();
    db.collaboration_ack(room, run, "b", 1).unwrap();
    db.collaboration_send(room, run, Some("a"), "自己的发言", "review-own")
        .unwrap();
    assert_eq!(db.collaboration_unread_of(room, run, "a").unwrap().count, 0);
    assert_eq!(db.collaboration_unread_of(room, run, "b").unwrap().count, 1);
    db.collaboration_send(room, run, Some("b"), "对方待查阅的发言", "review-other")
        .unwrap();
    db.collaboration_send(
        room,
        run,
        Some("a"),
        "尚未查阅就再次发言",
        "review-own-again",
    )
    .unwrap();
    assert_eq!(db.collaboration_unread_of(room, run, "a").unwrap().count, 1);
    assert_eq!(
        db.collaboration_read_of(room, "a", run)
            .unwrap()
            .unwrap()
            .through_seq,
        1
    );
}

#[test]
fn permanent_deletion_cleans_room_config_but_keeps_history_provenance() {
    let f = Fixture::new();
    let db = f.db();
    let started = db
        .collaboration_start(
            "t",
            &Fixture::members("a", "b"),
            Some("a"),
            None,
            None,
            "r1",
        )
        .unwrap();
    let room_id = started.room.id.clone();
    let run_id = started.run.id.clone();
    db.collaboration_send(&room_id, &run_id, Some("a"), "历史发件人", "m1")
        .unwrap();
    let cutoff = (chrono::Utc::now() - chrono::Duration::seconds(30)).to_rfc3339();
    db.collaboration_join(&room_id, &run_id, "a", "cli", "cli-resident", &cutoff)
        .unwrap();

    // The real purge entry: archive, then permanently delete through the
    // lifecycle API (never bare SQL against sessions).
    db.write()
        .execute(
            "UPDATE sessions SET archived_at='2026-01-02T00:00:00Z' WHERE id='a'",
            [],
        )
        .unwrap();
    noending_lib::lifecycle::permanently_delete_session(&db, "a").unwrap();

    // Config loses the member and the coordinator reference, with a revision
    // bump; no automatic replacement and no run status change.
    let room = db.collaboration_room_by_id(&room_id).unwrap().unwrap();
    let kept: Vec<&str> = room
        .selected_members
        .iter()
        .map(|m| m.session_id.as_str())
        .collect();
    assert_eq!(kept, vec!["b"]);
    assert!(room.coordinator_session_id.is_none());
    assert_eq!(room.revision, started.room.revision + 1);
    let run = db.collaboration_current_run(&room_id).unwrap().unwrap();
    assert_eq!(run.status, "starting", "no automatic stop or restart");

    // Participation/reads/wake rows cascaded; shared history keeps the sender.
    assert!(db
        .collaboration_participations_for_run(&run_id)
        .unwrap()
        .is_empty());
    assert!(db
        .collaboration_read_of(&room_id, "a", &run_id)
        .unwrap()
        .is_none());
    let page = db
        .collaboration_messages_page(&room_id, None, None, 50)
        .unwrap();
    let mail = page.items.iter().find(|m| m.body == "历史发件人").unwrap();
    assert_eq!(mail.sender_session_id.as_deref(), Some("a"));
}
