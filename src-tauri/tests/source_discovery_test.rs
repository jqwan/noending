//! Default stores are bounded; explicit user directories keep their scope.
use noending_lib::{
    adapters::{adapter_for, source_discovery_roots, DiscoveredMember, DiscoveredMemberKind},
    domain::{ingest_origin, Agent, IngestSource},
    ingestion,
    storage::{new_id, Db},
};
use serde_json::json;
use std::path::{Path, PathBuf};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("noending-source-scope-{}", new_id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn source(&self, agent: Agent) -> IngestSource {
        IngestSource {
            id: "source".into(),
            agent,
            path: self.0.to_string_lossy().into_owned(),
            origin: ingest_origin::DEFAULT.into(),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}
fn discover(source: &IngestSource) -> Vec<DiscoveredMember> {
    adapter_for(source.agent)
        .discover_members_in(&source_discovery_roots(source), &|_| false)
        .unwrap()
}
fn project_transcript(agent: Agent, id: &str) -> String {
    let message = if agent == Agent::WorkBuddy {
        json!({"id":"m1","type":"message","role":"user","sessionId":id,"cwd":"/repo","timestamp":1783137449113i64,"content":[{"type":"input_text","text":"明确的会话消息"}]})
    } else {
        json!({"uuid":"m1","parentUuid":null,"type":"user","sessionId":id,"cwd":"/repo","timestamp":"2026-09-22T15:01:29Z","message":{"role":"user","content":[{"type":"text","text":"明确的会话消息"}]}})
    };
    let prefix = if agent == Agent::Qoder {
        format!(
            "{}\n",
            json!({"type":"workspace-directories","sessionId":id,"directories":["/repo"]})
        )
    } else {
        String::new()
    };
    format!("{prefix}{message}\n")
}
#[test]
fn project_defaults_keep_root_and_child_transcripts_and_custom_directories() {
    for agent in [Agent::ClaudeCode, Agent::Qoder, Agent::WorkBuddy] {
        let f = Fixture::new();
        let mut source = f.source(agent);
        let projects = f.0.join("projects");
        let main = projects.join("项目 目录").join("main.jsonl");
        let child = projects.join("项目 目录/main/subagents/agent-child.jsonl");
        // A valid transcript outside projects must not cause a default source
        // with no projects directory to fall back to a whole-data-dir walk.
        write(
            &f.0.join("extensions/package/noise.jsonl"),
            &project_transcript(agent, "noise"),
        );
        assert!(discover(&source).is_empty(), "{agent:?}");
        write(&main, &project_transcript(agent, "main"));
        write(&child, &project_transcript(agent, "main"));
        let found = discover(&source);
        assert_eq!(found.len(), 2, "{agent:?}: {found:#?}");
        let root = found
            .iter()
            .find(|m| m.kind == DiscoveredMemberKind::Root)
            .unwrap();
        assert_eq!(root.source_member_id, "main");
        assert_eq!(root.first_user_text.as_deref(), Some("明确的会话消息"));
        let sub = found
            .iter()
            .find(|m| m.kind == DiscoveredMemberKind::Child)
            .unwrap();
        assert_eq!(sub.parent_source_member_id.as_deref(), Some("main"));
        source.origin = ingest_origin::USER.into();
        assert_eq!(
            discover(&source).len(),
            3,
            "explicit data root still covers custom files"
        );
        source.path = projects.to_string_lossy().into_owned();
        assert_eq!(discover(&source).len(), 2, "explicit projects directory");
        source.path = main.parent().unwrap().to_string_lossy().into_owned();
        assert_eq!(discover(&source).len(), 2, "explicit project directory");
    }
}
fn codex(id: &str, extra: serde_json::Value) -> String {
    let mut payload = json!({"id":id,"cwd":"/repo","timestamp":"2026-09-22T15:01:29Z"});
    payload
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    format!(
        "{}\n{}\n",
        json!({"type":"session_meta","payload":payload}),
        json!({"type":"response_item","payload":{"type":"message","id":"m1","role":"user","content":[{"type":"input_text","text":"保留正文"}]}})
    )
}
#[test]
fn codex_defaults_keep_archive_compression_children_and_shared_title_index() {
    let f = Fixture::new();
    let mut source = f.source(Agent::Codex);
    let live = "019f135a-621c-76a1-a76c-7c71021847aa";
    let archive = "01a0c943-53b8-7e82-8f84-0c2b33da8801";
    let child = "019fbbd3-47be-78f2-89fd-9deec2e4c6d9";
    let noise = "019fccd3-47be-78f2-89fd-9deec2e4c6d9";
    write(
        &f.0.join(format!("extensions/rollout-{noise}.jsonl")),
        &codex(noise, json!({})),
    );
    assert!(discover(&source).is_empty());
    write(
        &f.0.join(format!("sessions/2026/10/19/rollout-{live}.jsonl")),
        &codex(live, json!({})),
    );
    let archived =
        f.0.join(format!("archived_sessions/rollout-{archive}.jsonl.zst"));
    std::fs::create_dir_all(archived.parent().unwrap()).unwrap();
    std::fs::write(
        &archived,
        zstd::encode_all(codex(archive, json!({})).as_bytes(), 3).unwrap(),
    )
    .unwrap();
    write(
        &f.0.join(format!("sessions/2026/10/19/rollout-{child}.jsonl")),
        &codex(
            child,
            json!({"thread_source":"subagent","parent_thread_id":live}),
        ),
    );
    write(
        &f.0.join("session_index.jsonl"),
        &format!(
            "{}\n{}\n",
            json!({"id":live,"thread_name":"当前标题"}),
            json!({"id":archive,"thread_name":"归档标题"})
        ),
    );
    let found = discover(&source);
    assert_eq!(found.len(), 3, "{found:#?}");
    assert_eq!(
        found
            .iter()
            .find(|m| m.source_member_id == live)
            .unwrap()
            .native_title
            .as_deref(),
        Some("当前标题")
    );
    assert_eq!(
        found
            .iter()
            .find(|m| m.source_member_id == archive)
            .unwrap()
            .native_title
            .as_deref(),
        Some("归档标题")
    );
    let sub = found.iter().find(|m| m.source_member_id == child).unwrap();
    assert_eq!(sub.kind, DiscoveredMemberKind::Child);
    assert_eq!(sub.parent_source_member_id.as_deref(), Some(live));
    source.origin = ingest_origin::USER.into();
    source.path = archived.parent().unwrap().to_string_lossy().into_owned();
    let explicit = discover(&source);
    assert_eq!(explicit.len(), 1);
    assert_eq!(explicit[0].source_member_id, archive);
    assert_eq!(explicit[0].native_title.as_deref(), Some("归档标题"));
}
#[test]
fn dsh_missing_default_sessions_never_walks_plugins() {
    let f = Fixture::new();
    let source = f.source(Agent::Dsh);
    let body = format!(
        "{}\n{}\n",
        json!({"type":"session","version":2,"id":"dsh-main","cwd":"/repo","createdAt":1788969915099i64}),
        json!({"type":"user/message","seq":1,"time":1788969927205i64,"data":{"role":"user","content":[{"type":"text","text":"会话消息"}]}})
    );
    write(&f.0.join("plugins/package/session.v2.jsonl"), &body);
    assert!(discover(&source).is_empty());
    write(&f.0.join("sessions/项目/main/session.v2.jsonl"), &body);
    let found = discover(&source);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].source_member_id, "dsh-main");
}
#[test]
fn explicit_full_incremental_and_native_fallback_share_default_scope() {
    let f = Fixture::new();
    let source = f.source(Agent::Codex);
    let live = "019f135a-621c-76a1-a76c-7c71021847aa";
    let noise = "019fccd3-47be-78f2-89fd-9deec2e4c6d9";
    write(
        &f.0.join(format!("sessions/rollout-{live}.jsonl")),
        &codex(live, json!({})),
    );
    write(
        &f.0.join(format!("extensions/rollout-{noise}.jsonl")),
        &codex(noise, json!({})),
    );
    let db = Db::open(&f.0.join("test.db")).unwrap();
    db.write()
        .execute("DELETE FROM ingest_sources", [])
        .unwrap();
    let registered = db.add_ingest_source(Agent::Codex, &source.path).unwrap();
    db.write()
        .execute(
            "UPDATE ingest_sources SET origin='default' WHERE id=?1",
            [&registered.id],
        )
        .unwrap();
    assert_eq!(
        ingestion::reconcile_native(&db, Agent::Codex, noise, None, &|_| {}).unwrap(),
        (0, 0)
    );
    assert_eq!(
        ingestion::reconcile_source(&db, &source, &|_| {}).unwrap(),
        (1, 1)
    );
    assert_eq!(
        ingestion::reingest_source(&db, &source, &|_| {}).unwrap(),
        (1, 0)
    );
    let report = ingestion::reconcile_all_report(&db, &|_| {}).unwrap();
    assert!(report.failures.is_empty());
    assert_eq!(report.messages, 0);
    assert_eq!(db.list_sessions(Default::default()).unwrap().len(), 1);
}

#[test]
fn freshness_distinguishes_session_source_and_all_checks_and_preserves_success_after_failure() {
    use noending_lib::context::freshness::view;
    let f = Fixture::new();
    let db_path = f.0.join("freshness.db");
    let db = Db::open(&db_path).unwrap();
    db.write()
        .execute("DELETE FROM ingest_sources", [])
        .unwrap();
    let a_dir = f.0.join("a");
    let b_dir = f.0.join("b");
    let a_id = "019f135a-621c-76a1-a76c-7c71021847aa";
    let b_id = "01a0c943-53b8-7e82-8f84-0c2b33da8801";
    let a_file = a_dir.join(format!("rollout-{a_id}.jsonl"));
    let b_file = b_dir.join(format!("rollout-{b_id}.jsonl"));
    write(&a_file, &codex(a_id, json!({})));
    write(&b_file, &codex(b_id, json!({})));
    let a = db
        .add_ingest_source(Agent::Codex, a_dir.to_str().unwrap())
        .unwrap();
    let b = db
        .add_ingest_source(Agent::Codex, b_dir.to_str().unwrap())
        .unwrap();
    ingestion::reconcile_source(&db, &a, &|_| {}).unwrap();
    let sid = db
        .find_session_by_root_agent_id(Agent::Codex, a_id)
        .unwrap()
        .unwrap()
        .id;
    let one = view(&db, Some("session"), Some(&sid)).unwrap();
    assert_eq!(
        one.session_check.as_ref().unwrap().outcome.as_deref(),
        Some("success")
    );
    assert!(one.all_sources.checked_at.is_none());
    assert_eq!(one.sources.len(), 1);
    assert_eq!(one.sources[0].id, a.id);
    let first_check = one.session_check.unwrap().checked_at.unwrap();
    let first_source_check = one.sources[0].check.checked_at.clone();
    assert_eq!(ingestion::refresh_session(&db, &sid).unwrap(), 0);
    let checked = view(&db, Some("session"), Some(&sid)).unwrap();
    assert!(checked.session_check.unwrap().checked_at.unwrap() > first_check);
    assert_eq!(checked.sources[0].check.checked_at, first_source_check);
    assert!(checked.all_sources.checked_at.is_none());
    let all = ingestion::reconcile_all_report(&db, &|_| {}).unwrap();
    assert!(all.failures.is_empty());
    let before = view(&db, None, None).unwrap();
    assert_eq!(before.all_sources.outcome.as_deref(), Some("success"));
    assert_eq!(before.sessions.checked, 2);
    assert_eq!(before.sessions.unknown, 0);
    let b_checked = before
        .sources
        .iter()
        .find(|s| s.id == b.id)
        .unwrap()
        .check
        .checked_at
        .clone();
    assert_eq!(ingestion::reconcile_source(&db, &a, &|_| {}).unwrap().1, 0);
    let unchanged = view(&db, Some("session"), Some(&sid)).unwrap();
    assert!(unchanged.session_check.unwrap().checked_at > Some(first_check));
    assert_eq!(
        unchanged.all_sources.checked_at,
        before.all_sources.checked_at
    );
    // Invalid tail is a real partial read; retained history must remain readable.
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&b_file)
        .unwrap()
        .write_all(b"{broken frame\n")
        .unwrap();
    let report = ingestion::reconcile_all_report(&db, &|_| {}).unwrap();
    assert!(!report.failures.is_empty());
    let failed = view(&db, None, None).unwrap();
    assert_eq!(failed.all_sources.outcome.as_deref(), Some("partial"));
    assert_eq!(failed.all_sources.checked_at, before.all_sources.checked_at);
    assert_eq!(
        failed
            .sources
            .iter()
            .find(|s| s.id == a.id)
            .unwrap()
            .check
            .outcome
            .as_deref(),
        Some("success")
    );
    let b_state = &failed.sources.iter().find(|s| s.id == b.id).unwrap().check;
    assert_eq!(b_state.outcome.as_deref(), Some("failed"));
    assert_eq!(b_state.checked_at, b_checked);
    assert_eq!(failed.sessions.failed, 1);
    std::fs::remove_file(&b_file).unwrap();
    let b_sid = db
        .find_session_by_root_agent_id(Agent::Codex, b_id)
        .unwrap()
        .unwrap()
        .id;
    assert_eq!(ingestion::refresh_session(&db, &b_sid).unwrap(), 0);
    let missing = view(&db, Some("session"), Some(&b_sid)).unwrap();
    assert_eq!(
        missing.session_check.unwrap().outcome.as_deref(),
        Some("missing")
    );
    assert_eq!(missing.sessions.missing, 1);
    db.remove_ingest_source(&b.id).unwrap();
    let removed = view(&db, None, None).unwrap();
    assert!(removed.all_sources.checked_at.is_none());
    assert!(removed.sources.iter().all(|s| s.id != b.id));
    assert_eq!(
        removed
            .sources
            .iter()
            .find(|s| s.id == a.id)
            .unwrap()
            .check
            .outcome
            .as_deref(),
        Some("success")
    );
    drop(db);
    let reopened = Db::open(&db_path).unwrap();
    let persisted = view(&reopened, Some("session"), Some(&b_sid)).unwrap();
    assert_eq!(
        persisted.session_check.unwrap().outcome.as_deref(),
        Some("missing")
    );
}
