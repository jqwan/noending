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
    assert_eq!(unread.count, 2, "own messages do not advance the position");
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
fn candidate_sessions_are_task_scoped_unarchived_and_stably_paged() {
    let f = Fixture::new();
    let db = f.db();
    let first = db.collaboration_candidate_sessions("t", None, 2).unwrap();
    assert_eq!(first.items, vec!["a", "b"]);
    assert_eq!(first.next_cursor.as_deref(), Some("b"));
    let second = db
        .collaboration_candidate_sessions("t", first.next_cursor.as_deref(), 2)
        .unwrap();
    assert_eq!(second.items, vec!["c"], "archived sessions never appear");
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
        conflict.to_string().contains("已被用于其他任务"),
        "{conflict}"
    );
    // Same id, different members: mismatch as well.
    let conflict = db
        .collaboration_start("t", &Fixture::members("a", "c"), None, None, None, "r1")
        .unwrap_err();
    assert!(
        conflict.to_string().contains("不同的成员或负责人"),
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

    let first = db
        .collaboration_join(&room_id, &run_id, "a", "cli")
        .unwrap();
    assert_eq!(first.role.as_deref(), Some("a"));
    assert_eq!(
        first.through_seq, 0,
        "start notice begins the processing range"
    );

    // A second join replaces the connection id in one atomic step.
    let second = db
        .collaboration_join(&room_id, &run_id, "a", "mcp")
        .unwrap();
    assert_ne!(first.connection_id, second.connection_id);

    // The replaced connection's row is gone; the current one can renew.
    assert_eq!(
        db.collaboration_renew(&first.connection_id).unwrap(),
        noending_lib::storage::collaboration::RenewOutcome::Gone
    );
    assert_eq!(
        db.collaboration_renew(&second.connection_id).unwrap(),
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
        db.collaboration_renew(&second.connection_id).unwrap(),
        noending_lib::storage::collaboration::RenewOutcome::Unowned
    );

    // A non-member can never join.
    assert!(db
        .collaboration_join(&room_id, &run_id, "x", "cli")
        .is_err());

    // A deleted participation (cascade) reports Gone instead of renewing.
    db.collaboration_join(&room_id, &run_id, "a", "cli")
        .unwrap();
    db.write()
        .execute("DELETE FROM collaboration_participations", [])
        .unwrap();
    let third = db
        .collaboration_join(&room_id, &run_id, "a", "cli")
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

    db.collaboration_ack(&room_id, &run_id, "b", 2).unwrap();
    let page = db
        .collaboration_wait_page(&room_id, &run_id, "b", 2)
        .unwrap();
    let seqs: Vec<i64> = page.items.iter().map(|m| m.seq).collect();
    assert_eq!(seqs, vec![3, 4]);
    assert!(page.next_cursor.is_none(), "everything delivered");
}
