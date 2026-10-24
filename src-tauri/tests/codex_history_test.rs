//! Effective Codex history across immutable rollout versions, including repair
//! of records previously keyed by a physical rollout ID.
mod support;
use noending_lib::{
    context,
    domain::{Agent, IngestSource, Summary},
    ingestion,
    storage::{new_id, Db},
};
use serde_json::{json, Value};
use std::{
    io::Write,
    path::{Path, PathBuf},
};
const THREAD: &str = "01a13dc6-ff18-7700-b009-dc97c58f2c41";
const VERSION: &str = "01a15eef-3bb5-75b2-a16a-07ac1d2b28e0";
const THIRD: &str = "01a15eef-3bb5-75b2-a16a-07ac1d2b28e1";
struct Fixture {
    root: PathBuf,
    db: Db,
    source: IngestSource,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("noending-codex-history-{}", new_id()));
        std::fs::create_dir_all(&root).unwrap();
        let db = Db::open(&root.join("noending.db")).unwrap();
        let source = IngestSource {
            id: "source".into(),
            agent: Agent::Codex,
            path: root.to_string_lossy().into_owned(),
            origin: "custom".into(),
        };
        Self { root, db, source }
    }
    fn write(&self, thread: &str, rollout: &str, lines: &[Value], compressed: bool) -> PathBuf {
        let id = if thread == rollout {
            thread.into()
        } else {
            format!("{thread}_{rollout}")
        };
        let path = self.root.join(format!(
            "rollout-2026-10-23T12-00-00-{id}.jsonl{}",
            if compressed { ".zst" } else { "" }
        ));
        let text = lines
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(
            &path,
            if compressed {
                zstd::encode_all(text.as_bytes(), 3).unwrap()
            } else {
                text.into_bytes()
            },
        )
        .unwrap();
        path
    }
    fn sync(&self) -> noending_lib::error::Result<(usize, i64)> {
        ingestion::reconcile_source(&self.db, &self.source, &|_| {})
    }
    fn session(&self, thread: &str) -> noending_lib::domain::Session {
        self.db
            .find_session_by_root_agent_id(Agent::Codex, thread)
            .unwrap()
            .unwrap()
    }
    fn texts(&self, thread: &str) -> Vec<String> {
        self.db
            .get_messages(&self.session(thread).id, None, 100)
            .unwrap()
            .into_iter()
            .map(|m| m.content)
            .collect()
    }
    fn index(&self, thread: &str, path: &Path) {
        let c = rusqlite::Connection::open(self.root.join("state_5.sqlite")).unwrap();
        c.execute_batch(
            "CREATE TABLE IF NOT EXISTS threads(id TEXT PRIMARY KEY,rollout_path TEXT NOT NULL)",
        )
        .unwrap();
        c.execute("INSERT INTO threads VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET rollout_path=excluded.rollout_path",rusqlite::params![thread,path.to_str().unwrap()]).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn meta(thread: &str, ordinal: u64, base: Option<Value>) -> Value {
    json!({"type":"session_meta","ordinal":ordinal,"timestamp":"2026-10-23T12:00:00Z","payload":{"id":thread,"session_id":thread,"thread_source":"user","cwd":"/repo","history_base":base}})
}
fn msg(ordinal: u64, text: &str) -> Value {
    json!({"type":"response_item","ordinal":ordinal,"timestamp":"2026-10-23T12:00:01Z","payload":{"id":format!("m{ordinal}-{text}"),"type":"message","role":"user","content":[{"type":"input_text","text":text}]}})
}
fn reference(id: &str, end: u64) -> Value {
    json!({"thread_id":id,"end_ordinal_exclusive":end})
}

#[test]
fn reverted_history_retains_only_the_referenced_prefix_and_then_appends_incrementally() {
    for compressed in [false, true] {
        let f = Fixture::new();
        f.write(
            THREAD,
            THREAD,
            &[
                meta(THREAD, 0, None),
                msg(1, "保留的旧消息"),
                msg(2, "已回退的旧消息"),
            ],
            compressed,
        );
        f.sync().unwrap();
        let before = f.session(THREAD);
        assert!(!support::search_messages(&f.db, "已回退的旧消息", 20)
            .unwrap()
            .is_empty());
        let path = f.write(
            THREAD,
            VERSION,
            &[
                meta(THREAD, 2, Some(reference(THREAD, 2))),
                msg(3, "重写后的新消息"),
            ],
            compressed,
        );
        f.sync().unwrap();
        let after = f.session(THREAD);
        assert_eq!(after.id, before.id);
        assert_eq!(Path::new(&after.source_path), path);
        assert_eq!(f.db.sessions_for_ingestion(Agent::Codex).unwrap().len(), 1);
        assert_eq!(f.texts(THREAD), ["保留的旧消息", "重写后的新消息"]);
        assert!(support::search_messages(&f.db, "已回退的旧消息", 20)
            .unwrap()
            .is_empty());
        assert!(!support::search_messages(&f.db, "重写后的新消息", 20)
            .unwrap()
            .is_empty());
        let generation = after.fact_generation;
        let ids: Vec<_> =
            f.db.get_messages(&after.id, None, 100)
                .unwrap()
                .into_iter()
                .map(|m| m.id)
                .collect();
        if !compressed {
            writeln!(
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .unwrap(),
                "{}",
                msg(4, "后续增量消息")
            )
            .unwrap();
            ingestion::refresh_session(&f.db, &after.id).unwrap();
            assert_eq!(
                f.texts(THREAD),
                ["保留的旧消息", "重写后的新消息", "后续增量消息"]
            );
            assert_eq!(f.session(THREAD).fact_generation, generation);
            assert_eq!(
                f.db.get_messages(&after.id, None, 100)
                    .unwrap()
                    .iter()
                    .take(2)
                    .map(|m| m.id.clone())
                    .collect::<Vec<_>>(),
                ids
            );
        }
        f.sync().unwrap();
        assert_eq!(f.session(THREAD).source_path, path.to_string_lossy());
    }
}
#[test]
fn an_independent_fork_inherits_history_without_sharing_identity_or_task_tracking() {
    let f = Fixture::new();
    f.write(
        THREAD,
        THREAD,
        &[
            meta(THREAD, 0, None),
            msg(1, "共同前缀"),
            msg(2, "原会话后续"),
        ],
        false,
    );
    let mut fork = meta(VERSION, 2, Some(reference(THREAD, 2)));
    fork["payload"]["forked_from_id"] = json!(THREAD);
    // Session identity can be shared with a root; thread identity remains distinct.
    fork["payload"]["session_id"] = json!(THREAD);
    f.write(VERSION, VERSION, &[fork, msg(3, "独立分叉后续")], false);
    f.sync().unwrap();
    assert_eq!(f.texts(THREAD), ["共同前缀", "原会话后续"]);
    assert_eq!(f.texts(VERSION), ["共同前缀", "独立分叉后续"]);
    assert_eq!(
        f.session(VERSION).forked_from_session_id.as_deref(),
        Some(f.session(THREAD).id.as_str())
    );
    assert_eq!(f.db.sessions_for_ingestion(Agent::Codex).unwrap().len(), 2);
}
#[test]
fn nested_reverts_respect_earlier_cutoffs_and_missing_or_cyclic_dependencies_preserve_committed_history(
) {
    let f = Fixture::new();
    let original = f.write(
        THREAD,
        THREAD,
        &[
            meta(THREAD, 0, None),
            msg(1, "祖先消息"),
            msg(2, "回退尾部"),
        ],
        true,
    );
    f.write(
        THREAD,
        VERSION,
        &[
            meta(THREAD, 2, Some(reference(THREAD, 2))),
            msg(3, "中间版本"),
        ],
        false,
    );
    let active = f.write(
        THREAD,
        THIRD,
        &[
            meta(THREAD, 4, Some(reference(VERSION, 4))),
            msg(5, "当前版本"),
        ],
        false,
    );
    f.index(THREAD, &active);
    f.sync().unwrap();
    assert_eq!(f.texts(THREAD), ["祖先消息", "中间版本", "当前版本"]);
    std::fs::rename(&original, original.with_extension("hidden")).unwrap();
    assert!(f.sync().is_err());
    assert_eq!(f.texts(THREAD), ["祖先消息", "中间版本", "当前版本"]);
    std::fs::rename(original.with_extension("hidden"), &original).unwrap();
    f.write(
        THREAD,
        THIRD,
        &[
            meta(THREAD, 4, Some(reference(THIRD, 4))),
            msg(5, "无效循环"),
        ],
        false,
    );
    assert!(f.sync().is_err());
    assert_eq!(f.texts(THREAD), ["祖先消息", "中间版本", "当前版本"]);
    f.write(
        THREAD,
        THIRD,
        &[
            meta(THREAD, 2, Some(reference(VERSION, 2))),
            msg(3, "回到更早的位置"),
        ],
        false,
    );
    f.sync().unwrap();
    assert_eq!(f.texts(THREAD), ["祖先消息", "回到更早的位置"]);
}
#[test]
fn native_index_selects_the_active_version_and_ambiguous_unindexed_versions_are_not_guessed() {
    let f = Fixture::new();
    f.write(
        THREAD,
        THREAD,
        &[meta(THREAD, 0, None), msg(1, "保留前缀"), msg(2, "原尾部")],
        false,
    );
    let first = f.write(
        THREAD,
        VERSION,
        &[
            meta(THREAD, 2, Some(reference(THREAD, 2))),
            msg(3, "选择第一版"),
        ],
        false,
    );
    let second = f.write(
        THREAD,
        THIRD,
        &[
            meta(THREAD, 2, Some(reference(THREAD, 2))),
            msg(3, "选择第二版"),
        ],
        false,
    );
    assert!(f.sync().is_err());
    assert!(f
        .db
        .sessions_for_ingestion(Agent::Codex)
        .unwrap()
        .is_empty());
    f.index(THREAD, &first);
    f.sync().unwrap();
    assert_eq!(f.texts(THREAD), ["保留前缀", "选择第一版"]);
    f.index(THREAD, &second);
    ingestion::refresh_session(&f.db, &f.session(THREAD).id).unwrap();
    assert_eq!(f.texts(THREAD), ["保留前缀", "选择第二版"]);
    assert_eq!(f.session(THREAD).source_path, second.to_string_lossy());
}
#[test]
fn invalid_byte_boundaries_and_changed_ancestor_prefixes_cannot_silently_reuse_a_cursor() {
    let f = Fixture::new();
    let lines = [meta(THREAD, 0, None), msg(1, "旧前缀"), msg(2, "不再显示")];
    let old = f.write(THREAD, THREAD, &lines, false);
    let bytes = (lines[..2]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n")
        .len();
    let mut base = reference(THREAD, 2);
    base["end_byte_offset"] = json!(bytes);
    f.write(
        THREAD,
        VERSION,
        &[meta(THREAD, 2, Some(base.clone())), msg(3, "当前尾部")],
        false,
    );
    f.sync().unwrap();
    assert_eq!(f.texts(THREAD), ["旧前缀", "当前尾部"]);
    base["end_byte_offset"] = json!(bytes - 1);
    f.write(
        THREAD,
        VERSION,
        &[meta(THREAD, 2, Some(base)), msg(3, "坏边界")],
        false,
    );
    assert!(f.sync().is_err());
    assert_eq!(f.texts(THREAD), ["旧前缀", "当前尾部"]);
    f.write(
        THREAD,
        VERSION,
        &[
            meta(THREAD, 2, Some(reference(THREAD, 2))),
            msg(3, "当前尾部"),
        ],
        false,
    );
    f.write(
        THREAD,
        THREAD,
        &[
            meta(THREAD, 0, None),
            msg(1, "修改后的前缀"),
            msg(2, "依然不显示"),
        ],
        false,
    );
    f.sync().unwrap();
    assert_eq!(f.texts(THREAD), ["修改后的前缀", "当前尾部"]);
    assert!(old.is_file());
}
#[test]
fn stored_rollout_aliases_merge_into_the_original_app_id_without_losing_summaries_or_tasks() {
    let f = Fixture::new();
    let old = f.write(
        THREAD,
        THREAD,
        &[
            meta(THREAD, 0, None),
            msg(1, "保留的历史"),
            msg(2, "已丢弃的消息"),
        ],
        false,
    );
    f.sync().unwrap();
    let canonical = f.session(THREAD);
    let active = f.write(
        THREAD,
        VERSION,
        &[
            meta(THREAD, 2, Some(reference(THREAD, 2))),
            msg(3, "当前消息"),
        ],
        false,
    );
    let (alias, _) =
        f.db.upsert_logical_session(
            Agent::Codex,
            VERSION,
            None,
            Some("错误分支"),
            Some("/repo"),
            canonical.workspace_path_id.as_deref(),
            Some(&canonical.id),
            None,
            None,
            "codex_rollout",
            active.to_str().unwrap(),
            &json!({}),
        )
        .unwrap();
    context::save_summary(
        &f.db,
        "session",
        &canonical.id,
        0,
        Summary::single("原会话的人工摘要"),
    )
    .unwrap();
    context::save_summary(
        &f.db,
        "session",
        &alias,
        0,
        Summary::single("新版本的人工摘要"),
    )
    .unwrap();
    let task = support::workstream("task-merge".into(), "跟踪任务");
    f.db.upsert_workstream(&task).unwrap();
    support::track(&f.db, &alias, Some(&task.id)).unwrap();
    let complete = std::fs::read(&active).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&active)
        .unwrap()
        .write_all(b"{\"type\":")
        .unwrap();
    assert!(f.sync().is_err());
    assert!(
        f.db.get_session(&alias).unwrap().is_some(),
        "an incomplete replacement must not remove the previous stored record"
    );
    std::fs::write(&active, complete).unwrap();
    f.sync().unwrap();
    assert_eq!(f.session(THREAD).id, canonical.id);
    assert!(f.db.get_session(&alias).unwrap().is_none());
    assert_eq!(f.session(THREAD).tracked_workstream_ids, [task.id]);
    assert_eq!(f.texts(THREAD), ["保留的历史", "当前消息"]);
    let summary = f.db.get_session_context(&canonical.id).unwrap().unwrap();
    assert_eq!(
        summary
            .fields
            .summary
            .items
            .iter()
            .map(|i| i.title.as_str())
            .collect::<Vec<_>>(),
        ["原会话的人工摘要", "新版本的人工摘要"]
    );
    assert_eq!(summary.processed_through_seq, 0);
    assert!(old.is_file());
    assert!(active.is_file());
    let revision = summary.revision;
    f.sync().unwrap();
    assert_eq!(
        f.db.get_session_context(&canonical.id)
            .unwrap()
            .unwrap()
            .revision,
        revision
    );
}
#[test]
fn a_lone_rollout_alias_keeps_its_app_id_and_summary_when_the_native_identity_is_corrected() {
    let f = Fixture::new();
    f.write(
        THREAD,
        THREAD,
        &[meta(THREAD, 0, None), msg(1, "此前消息")],
        true,
    );
    let active = f.write(
        THREAD,
        VERSION,
        &[
            meta(THREAD, 2, Some(reference(THREAD, 2))),
            msg(3, "后续消息"),
        ],
        true,
    );
    let (alias, _) =
        f.db.upsert_logical_session(
            Agent::Codex,
            VERSION,
            None,
            Some("保留标题"),
            None,
            None,
            None,
            None,
            None,
            "codex_rollout",
            active.to_str().unwrap(),
            &json!({}),
        )
        .unwrap();
    context::save_summary(&f.db, "session", &alias, 0, Summary::single("保留人工摘要")).unwrap();
    f.sync().unwrap();
    assert_eq!(f.session(THREAD).id, alias);
    assert!(f
        .db
        .find_session_by_root_agent_id(Agent::Codex, VERSION)
        .unwrap()
        .is_none());
    assert_eq!(
        f.db.get_session_context(&alias)
            .unwrap()
            .unwrap()
            .fields
            .summary
            .items[0]
            .title,
        "保留人工摘要"
    );
    assert_eq!(f.texts(THREAD), ["此前消息", "后续消息"]);
}

#[test]
fn failed_alias_absorption_rolls_back_app_owned_content_and_succeeds_on_retry() {
    let f = Fixture::new();
    f.write(
        THREAD,
        THREAD,
        &[meta(THREAD, 0, None), msg(1, "共同历史")],
        false,
    );
    f.sync().unwrap();
    let canonical = f.session(THREAD);
    let active = f.write(
        THREAD,
        VERSION,
        &[
            meta(THREAD, 2, Some(reference(THREAD, 2))),
            msg(3, "新的讨论"),
        ],
        false,
    );
    let (alias, _) =
        f.db.upsert_logical_session(
            Agent::Codex,
            VERSION,
            None,
            Some("物理版本"),
            Some("/repo"),
            canonical.workspace_path_id.as_deref(),
            Some(&canonical.id),
            None,
            None,
            "codex_rollout",
            active.to_str().unwrap(),
            &json!({}),
        )
        .unwrap();
    context::save_summary(
        &f.db,
        "session",
        &canonical.id,
        0,
        Summary::single("原摘要"),
    )
    .unwrap();
    context::save_summary(&f.db, "session", &alias, 0, Summary::single("版本摘要")).unwrap();
    let task = support::workstream("rollback-task".into(), "保留任务");
    f.db.upsert_workstream(&task).unwrap();
    support::track(&f.db, &alias, Some(&task.id)).unwrap();
    f.db.write().execute_batch("CREATE TRIGGER prevent_alias_delete BEFORE DELETE ON sessions BEGIN SELECT RAISE(ABORT,'injected deletion failure'); END;").unwrap();
    assert!(f.sync().is_err());
    assert!(f.db.get_session(&alias).unwrap().is_some());
    assert!(f.session(THREAD).tracked_workstream_ids.is_empty());
    assert_eq!(
        f.db.get_session(&alias)
            .unwrap()
            .unwrap()
            .tracked_workstream_ids,
        [task.id.clone()]
    );
    assert_eq!(
        f.db.get_session_context(&canonical.id)
            .unwrap()
            .unwrap()
            .fields
            .summary
            .items[0]
            .title,
        "原摘要"
    );
    assert_eq!(
        f.db.get_session_context(&alias)
            .unwrap()
            .unwrap()
            .fields
            .summary
            .items[0]
            .title,
        "版本摘要"
    );
    f.db.write()
        .execute_batch("DROP TRIGGER prevent_alias_delete")
        .unwrap();
    f.sync().unwrap();
    assert!(f.db.get_session(&alias).unwrap().is_none());
    assert_eq!(f.session(THREAD).tracked_workstream_ids, [task.id]);
    assert_eq!(
        f.db.get_session_context(&canonical.id)
            .unwrap()
            .unwrap()
            .fields
            .summary
            .items
            .len(),
        2
    );
}
