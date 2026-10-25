//! Query contexts for extraction performed by the calling session. Only small
//! snapshot guards and page receipts are retained; no worker or Agent is started.
use super::*;
use crate::cli::{transport, CliError, CliResult};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Mode {
    Update,
    Regenerate,
}
impl Mode {
    fn regenerate(self) -> bool {
        matches!(self, Self::Regenerate)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Scope {
    kind: String,
    target: String,
    mode: Mode,
    guard: Value,
    pub(super) sessions: Vec<SessionSnapshot>,
    pub(super) templates: prompts::PromptTemplates,
    inputs: HashMap<String, query::Input>,
    pub(super) delivered: HashMap<String, HashSet<usize>>,
    expires_at: chrono::DateTime<chrono::Utc>,
}

fn directory(home: &crate::workspace::home::NoEndingHome, id: &str) -> CliResult<PathBuf> {
    if uuid::Uuid::parse_str(id).is_err() {
        return Err(CliError::new("invalid_arguments", "提取上下文 ID 无效"));
    }
    Ok(diagnostics::runtime_dir(home).join("queries").join(id))
}
fn save(path: &Path, scope: &Scope) -> CliResult<()> {
    let temp = path.join("scope.tmp");
    transport::private_write(&temp, &serde_json::to_string(scope)?)?;
    #[cfg(windows)]
    if path.join("scope.json").exists() {
        let _ = fs::remove_file(path.join("scope.json"));
    }
    fs::rename(temp, path.join("scope.json"))?;
    Ok(())
}
fn load(path: &Path) -> CliResult<Scope> {
    let scope: Scope = serde_json::from_str(
        &fs::read_to_string(path.join("scope.json"))
            .map_err(|_| CliError::new("context_unavailable", "提取上下文不存在，请重新查询"))?,
    )?;
    if chrono::Utc::now() >= scope.expires_at {
        return Err(CliError::new(
            "context_expired",
            "提取上下文已过期，请重新查询",
        ));
    }
    Ok(scope)
}
fn guard(db: &Db, kind: &str, id: &str) -> Result<Value> {
    Ok(match kind {
        "session" => {
            let session = db.get_session(id)?.ok_or_else(stale)?;
            json!({"project":session.project_id})
        }
        "task" => {
            let task = db.get_workstream(id)?.ok_or_else(stale)?;
            let state = query::task_state(&db.read(), id)?;
            let mut sessions: Vec<_> = db
                .sessions_for_workstream(id)?
                .into_iter()
                .map(|s| s.id)
                .collect();
            sessions.sort();
            json!({"title":task.title,"description":task.description,"projects":task_projects(&db.read(),id)?,"revision":state.context_revision,"input_revision":state.input_revision,"sessions":sessions})
        }
        "project" => {
            json!({"fingerprint":project::ProjectInput::read(&db.read(),id)?.fingerprint,"revision":query::project_state(&db.read(),id)?.revision})
        }
        _ => return Err(other("未知提取目标")),
    })
}
impl Scope {
    pub(super) fn validate(&self, db: &Db) -> Result<()> {
        if guard(db, &self.kind, &self.target)? != self.guard {
            return Err(stale());
        }
        for s in &self.sessions {
            s.validate(&db.read(), None)?;
        }
        Ok(())
    }
    fn files(&self, home: &crate::workspace::home::NoEndingHome) -> Result<ExtractionFiles> {
        let root = diagnostics::prepare_runtime_dir(home)?;
        let ids = self
            .sessions
            .iter()
            .map(|s| s.session.id.clone())
            .collect::<Vec<_>>();
        let files =
            ExtractionFiles::create(&root, &format!("{}-{}", self.kind, self.target), &ids)?;
        *files.inputs.lock().unwrap() = self.inputs.clone();
        Ok(files)
    }
}

pub(crate) fn context(
    db: &Db,
    home: &crate::workspace::home::NoEndingHome,
    kind: &str,
    id: &str,
    mode: Mode,
) -> CliResult<Value> {
    let target_guard = guard(db, kind, id)?;
    let regenerate = mode.regenerate();
    let sessions = match kind {
        "session" => vec![db.get_session(id)?.ok_or_else(stale)?],
        "task" => db.sessions_for_workstream(id)?,
        "project" => {
            let conn = db.read();
            let mut st=conn.prepare("SELECT s.id FROM sessions s JOIN workspace_paths wp ON wp.id=s.workspace_path_id WHERE wp.project_id=?1 ORDER BY s.id")?;
            let ids=st.query_map([id],|r| r.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;
            drop(st);drop(conn);
            ids.into_iter().map(|sid|db.get_session(&sid)?.ok_or_else(stale)).collect::<Result<Vec<_>>>()?
        }
        _ => return Err(CliError::new("invalid_arguments","目标类型无效")),
    }.into_iter().map(|s|SessionSnapshot::read(db,s,kind=="session" && regenerate)).collect::<Result<Vec<_>>>()?;
    let templates = prompts::PromptTemplates::default();
    let root = diagnostics::prepare_runtime_dir(home)?;
    let files = ExtractionFiles::create(
        &root,
        &format!("{kind}-{id}"),
        &sessions
            .iter()
            .map(|s| s.session.id.clone())
            .collect::<Vec<_>>(),
    )?;
    if kind == "session" {
        let s = &sessions[0];
        files.add_session(
            s,
            !regenerate && s.summary_generation == Some(s.generation),
            false,
        );
    } else {
        let (header, revision) = if kind == "task" {
            let task = db.get_workstream(id)?.ok_or_else(stale)?;
            (
                json!({"type":"task","title":task.title,"description":task.description,"related_projects":crate::workspace::workstream::projects_for_workstream(db,id)?}),
                query::task_state(&db.read(), id)?.context_revision,
            )
        } else {
            let input = project::ProjectInput::read(&db.read(), id)?;
            (
                input.header(home),
                query::project_state(&db.read(), id)?.revision,
            )
        };
        add_batch_inputs(&files, kind, id, header, revision, regenerate, &sessions);
    }
    let scope = Scope {
        kind: kind.into(),
        target: id.into(),
        mode,
        guard: target_guard,
        sessions,
        templates,
        inputs: files.inputs.lock().unwrap().clone(),
        delivered: HashMap::new(),
        expires_at: chrono::Utc::now() + chrono::Duration::hours(24),
    };
    scope.validate(db)?;
    let context_id = crate::storage::new_id();
    let path = directory(home, &context_id)?;
    transport::private_dir(&path)?;
    save(&path, &scope)?;
    let mut inputs = scope
        .inputs
        .iter()
        .map(|(name, input)| Ok(json!({"name":name,"record_count":input.count(&db.read())?})))
        .collect::<Result<Vec<_>>>()?;
    inputs.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(
        json!({"context_id":context_id,"kind":kind,"id":id,"mode":mode,"expires_at":scope.expires_at,"inputs":inputs,"requirements":scope.templates,"result_key":format!("{kind}-{id}"),"protocol":"results maps target keys (session-ID/task-ID/project-ID) to {summary:{items:[...]}}; reply follows the extraction status protocol. Read all required inputs before submitting."}),
    )
}

pub(crate) fn read(
    db: &Db,
    home: &crate::workspace::home::NoEndingHome,
    id: &str,
    input: &str,
    cursor: usize,
    limit: usize,
    search: Option<&str>,
) -> CliResult<Value> {
    if !(1..=50).contains(&limit) || search.is_some_and(|s| s.trim().is_empty()) {
        return Err(CliError::new(
            "invalid_arguments",
            "limit 必须为 1～50，搜索内容不能为空",
        ));
    }
    let path = directory(home, id)?;
    let _lock = crate::platform::file_lock::exclusive(&path.join("lock"))?;
    let mut scope = load(&path)?;
    scope.validate(db)?;
    let files = scope.files(home)?;
    let source = scope
        .inputs
        .get(input)
        .ok_or_else(|| CliError::new("invalid_arguments", "输入不在本次提取范围内"))?;
    let mut page = source.page(db, &files, cursor, limit, search)?;
    // The current session submits JSON through CLI/MCP, not filesystem paths.
    for row in page["records"].as_array_mut().unwrap() {
        if let Some(record) = row["record"].as_object_mut() {
            for name in ["input_file", "history_input_file", "result_file"] {
                if let Some(value) = record.get_mut(name) {
                    if let Some(p) = value.as_str() {
                        *value = json!(Path::new(p).file_name().unwrap().to_string_lossy());
                    }
                }
            }
        }
    }
    if search.is_none() {
        scope.delivered.entry(input.into()).or_default().extend(
            page["records"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["cursor"].as_u64().unwrap() as usize),
        );
        save(&path, &scope)?;
    }
    Ok(page)
}

pub(crate) fn submit(
    db: &Db,
    home: &crate::workspace::home::NoEndingHome,
    id: &str,
    reply: Value,
    results: HashMap<String, Value>,
) -> CliResult<Value> {
    let path = directory(home, id)?;
    let _lock = crate::platform::file_lock::exclusive(&path.join("lock"))?;
    let scope = load(&path)?;
    scope.validate(db)?;
    let mut runner = |_: Option<&CliExtractor>, _: &str, dir: &Path| {
        for (key, value) in &results {
            if !(key == &format!("{}-{}", scope.kind, scope.target)
                || scope.inputs.contains_key(&format!("{key}.jsonl")))
            {
                return Err(extractor::ContextCallFailure {
                    kind: extractor::ContextCallFailureKind::CommandInvalid,
                    spawned: false,
                    exit_code: None,
                    io_error_kind: None,
                });
            }
            fs::write(dir.join(format!("{key}.result.json")), value.to_string()).map_err(|_| {
                extractor::ContextCallFailure {
                    kind: extractor::ContextCallFailureKind::Start,
                    spawned: false,
                    exit_code: None,
                    io_error_kind: None,
                }
            })?;
        }
        Ok(reply.to_string())
    };
    let out = match scope.kind.as_str() {
        "session" => json!(extract_session_using(
            db,
            &scope.target,
            Some(home),
            scope.mode.regenerate(),
            Some(&scope),
            &mut runner
        )?),
        "task" => json!(extract_task_using(
            db,
            &scope.target,
            Some(home),
            scope.mode.regenerate(),
            Some(&scope),
            &mut runner
        )?),
        "project" => json!(project::extract_project_using(
            db,
            &scope.target,
            Some(home),
            scope.mode.regenerate(),
            Some(&scope),
            &mut runner
        )?),
        _ => return Err(CliError::new("invalid_arguments", "目标类型无效")),
    };
    // A context is consumed once. Generic request receipts make identical retries safe.
    let _ = fs::remove_file(path.join("scope.json"));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::tests::{append, seed, ServiceFixture};
    fn read_all(f: &ServiceFixture, c: &Value) {
        for input in c["inputs"].as_array().unwrap() {
            let mut cursor = 0;
            loop {
                let page = read(
                    f.db(),
                    &f.home(),
                    c["context_id"].as_str().unwrap(),
                    input["name"].as_str().unwrap(),
                    cursor,
                    1,
                    None,
                )
                .unwrap();
                if let Some(next) = page["next_cursor"].as_u64() {
                    cursor = next as usize;
                } else {
                    break;
                }
            }
        }
    }
    fn result(id: &str, title: &str) -> HashMap<String, Value> {
        HashMap::from([(
            format!("session-{id}"),
            json!({"summary":{"items":[{"title":title,"description":"","details":[]}]}}),
        )])
    }
    #[test]
    fn current_session_reads_and_submits_without_an_extraction_agent_and_appends_stay_pending() {
        let f = ServiceFixture::new();
        let id = f.add_session(2, 20);
        f.db()
            .set_setting("context.extraction", r#"{"agent":"none","runtime":{}}"#)
            .unwrap();
        let c = context(f.db(), &f.home(), "session", &id, Mode::Update).unwrap();
        append(&f, &id, "after snapshot", false);
        read_all(&f, &c);
        let out = submit(
            f.db(),
            &f.home(),
            c["context_id"].as_str().unwrap(),
            json!({"status":"completed"}),
            result(&id, "已核实结论"),
        )
        .unwrap();
        assert_eq!(out["status"], "updated");
        let view = session_context_view(f.db(), &id).unwrap();
        assert_eq!(view.processed_through_seq, 2);
        assert_eq!(view.latest_message_seq, 3);
        assert!(view.pending);
        assert_eq!(view.fields.unwrap().summary.items[0].title, "已核实结论");
        assert!(load(&directory(&f.home(), c["context_id"].as_str().unwrap()).unwrap()).is_err());
    }
    #[test]
    fn update_reads_only_pending_messages_and_regenerate_excludes_old_summary_as_evidence() {
        let f = ServiceFixture::new();
        let id = f.add_session(2, 20);
        seed(&f, &id, "历史摘要");
        append(&f, &id, "new material", false);
        for (mode, from, count, old) in
            [(Mode::Update, 3, 1, true), (Mode::Regenerate, 1, 3, false)]
        {
            let c = context(f.db(), &f.home(), "session", &id, mode).unwrap();
            let page = read(
                f.db(),
                &f.home(),
                c["context_id"].as_str().unwrap(),
                &format!("session-{id}.jsonl"),
                0,
                8,
                None,
            )
            .unwrap();
            let header = &page["records"][0]["record"];
            assert_eq!(header["from_sequence"], from);
            assert_eq!(header["message_count"], count);
            assert_eq!(!header["current_summary"].is_null(), old);
            assert_eq!(header["item_identities"].as_array().unwrap().len(), 1);
        }
    }
    #[test]
    fn search_is_not_complete_reading_and_invalid_results_leave_progress_unchanged() {
        let f = ServiceFixture::new();
        let id = f.add_session(2, 20);
        let c = context(f.db(), &f.home(), "session", &id, Mode::Update).unwrap();
        let cid = c["context_id"].as_str().unwrap();
        read(
            f.db(),
            &f.home(),
            cid,
            &format!("session-{id}.jsonl"),
            0,
            8,
            Some("x"),
        )
        .unwrap();
        let err = submit(
            f.db(),
            &f.home(),
            cid,
            json!({"status":"completed"}),
            result(&id, "must not persist"),
        )
        .unwrap_err();
        assert_eq!(err.code, "invalid_output");
        assert_eq!(session_context_view(f.db(), &id).unwrap().revision, 0);
        read_all(&f, &c);
        let invalid = HashMap::from([(format!("session-{id}"), json!({"summary":{"items":[]}}))]);
        assert!(submit(
            f.db(),
            &f.home(),
            cid,
            json!({"status":"completed"}),
            invalid
        )
        .is_err());
        assert_eq!(session_context_view(f.db(), &id).unwrap().revision, 0);
        submit(
            f.db(),
            &f.home(),
            cid,
            json!({"status":"completed"}),
            result(&id, "valid"),
        )
        .unwrap();
        assert_eq!(session_context_view(f.db(), &id).unwrap().revision, 1);
    }
    #[test]
    fn history_rewrite_and_summary_conflicts_reject_a_context_without_overwriting_data() {
        for rewrite in [false, true] {
            let f = ServiceFixture::new();
            let id = f.add_session(2, 20);
            let c = context(f.db(), &f.home(), "session", &id, Mode::Update).unwrap();
            read_all(&f, &c);
            if rewrite {
                append(&f, &id, "replacement", true);
            } else {
                seed(&f, &id, "other writer");
            }
            let err = submit(
                f.db(),
                &f.home(),
                c["context_id"].as_str().unwrap(),
                json!({"status":"completed"}),
                result(&id, "stale"),
            )
            .unwrap_err();
            assert_eq!(err.code, "stale_result");
            assert_eq!(
                session_context_view(f.db(), &id).unwrap().revision,
                if rewrite { 0 } else { 1 }
            );
        }
    }

    #[test]
    fn task_and_project_contexts_keep_scope_partial_failures_and_independent_task_items() {
        for kind in ["task", "project"] {
            let f = ServiceFixture::new();
            let a = f.add_session(2, 20);
            let b = f.add_session(1, 20);
            let outside = f.add_session(1, 20);
            let task = crate::context::tests::task(&f, &[&a, &b]);
            let (project, _) =
                crate::context::tests::project_fixture(&f, "direct-scope", &[&a, &b]);
            let target = if kind == "task" {
                &task.id
            } else {
                &project.id
            };
            let item = crate::workspace::task_items::create(
                f.db(),
                &task.id,
                crate::workspace::task_items::CreateItem {
                    title: "验收事项".into(),
                    description: None,
                },
            )
            .unwrap();
            let c = context(f.db(), &f.home(), kind, target, Mode::Regenerate).unwrap();
            assert!(!c["inputs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["name"].as_str().unwrap().contains(&outside)));
            read_all(&f, &c);
            let mut results = result(&a, "A 事实");
            results.insert(format!("{kind}-{target}"),json!({"summary":{"items":[{"title":"范围结论","description":"B 未完成提取","details":[]}]}}));
            let out=submit(f.db(),&f.home(),c["context_id"].as_str().unwrap(),json!({"status":"partial","failed_sessions":[{"session_id":b,"error":"材料不完整"}],"skipped_session_ids":[]}),results).unwrap();
            assert_eq!(out["status"], "partial");
            assert_eq!(out["failed_sessions"][0]["session_id"], b);
            assert_eq!(session_context_view(f.db(), &a).unwrap().revision, 1);
            assert_eq!(session_context_view(f.db(), &b).unwrap().revision, 0);
            assert_eq!(
                crate::workspace::task_items::list(f.db(), &task.id).unwrap(),
                vec![item]
            );
            let next = context(f.db(), &f.home(), kind, target, Mode::Update).unwrap();
            read_all(&f, &next);
            let mut results = result(&b, "B 事实");
            results.insert(format!("{kind}-{target}"),json!({"summary":{"items":[{"title":"完整范围结论","description":"A 与 B","details":[]}]}}));
            let out = submit(
                f.db(),
                &f.home(),
                next["context_id"].as_str().unwrap(),
                json!({"status":"completed","skipped_session_ids":[]}),
                results,
            )
            .unwrap();
            assert_eq!(out["status"], "updated");
            assert_eq!(out["remaining_pending"], 0);
            assert_eq!(session_context_view(f.db(), &outside).unwrap().revision, 0);
        }
    }
    #[test]
    fn scope_changes_expiry_and_other_input_names_are_rejected() {
        let f = ServiceFixture::new();
        let id = f.add_session(2, 20);
        let task =
            crate::workspace::workstream::create_workstream(f.db(), "scope", "goal", &[]).unwrap();
        crate::storage::seed_tracking(f.db(), &id, Some(&task.id)).unwrap();
        let c = context(f.db(), &f.home(), "task", &task.id, Mode::Update).unwrap();
        let cid = c["context_id"].as_str().unwrap();
        assert!(read(f.db(), &f.home(), cid, "session-other.jsonl", 0, 8, None).is_err());
        let mut changed = task.clone();
        changed.description = "new goal".into();
        f.db().upsert_workstream(&changed).unwrap();
        assert_eq!(
            read(
                f.db(),
                &f.home(),
                cid,
                &format!("task-{}.jsonl", task.id),
                0,
                8,
                None
            )
            .unwrap_err()
            .code,
            "stale_result"
        );
        let c = context(f.db(), &f.home(), "session", &id, Mode::Update).unwrap();
        let path = directory(&f.home(), c["context_id"].as_str().unwrap()).unwrap();
        let mut scope = load(&path).unwrap();
        scope.expires_at = chrono::Utc::now() - chrono::Duration::seconds(1);
        save(&path, &scope).unwrap();
        assert_eq!(load(&path).err().unwrap().code, "context_expired");
    }
}
