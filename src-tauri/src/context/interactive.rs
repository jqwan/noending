//! Snapshot, read receipts, and submission rules shared by both extraction entry points.
use super::*;
use crate::cli::{transport, CliError, CliResult};
use crate::workspace::home::NoEndingHome;
use rmcp::schemars::{self, JsonSchema};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};
use summary::Submission;

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
impl From<bool> for Mode {
    fn from(regenerate: bool) -> Self {
        if regenerate {
            Self::Regenerate
        } else {
            Self::Update
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct Scope {
    kind: String,
    target: String,
    mode: Mode,
    project: Option<ProjectGuard>,
    sessions: Vec<SessionSnapshot>,
    collaboration: Option<collaboration::Snapshot>,
    delivered: HashMap<String, HashSet<usize>>,
    saved: BTreeMap<String, SubmissionReceipt>,
    failures: BTreeMap<String, String>,
    final_receipt: Option<SubmissionReceipt>,
    expires_at: chrono::DateTime<chrono::Utc>,
}
#[derive(Serialize, Deserialize)]
struct ProjectGuard {
    fingerprint: String,
    header: Value,
    revision: i64,
}
#[derive(Serialize, Deserialize)]
struct SubmissionReceipt {
    hash: String,
    value: Value,
}
impl SubmissionReceipt {
    fn replay(&self, hash: &str) -> Result<Value> {
        if self.hash != hash {
            return Err(invalid("此目标已提交，请重新查询提取范围"));
        }
        Ok(self.value.clone())
    }
}

fn directory(home: &NoEndingHome, id: &str) -> CliResult<PathBuf> {
    if uuid::Uuid::parse_str(id).is_err() {
        return Err(CliError::new("invalid_arguments", "提取上下文 ID 无效"));
    }
    Ok(diagnostics::runtime_dir(home).join("queries").join(id))
}
fn save(path: &Path, scope: &Scope) -> CliResult<()> {
    let temp = path.join("scope.tmp");
    transport::private_write(&temp, &serde_json::to_string(scope)?)?;
    fs::rename(temp, path.join("scope.json"))?;
    Ok(())
}
fn load(path: &Path) -> CliResult<Scope> {
    let scope: Scope = serde_json::from_str(
        &fs::read_to_string(path.join("scope.json"))
            .map_err(|_| CliError::new("context_unavailable", "提取上下文不存在，请重新查询"))?,
    )
    .map_err(|_| CliError::new("context_unavailable", "提取上下文已失效，请重新查询"))?;
    if chrono::Utc::now() >= scope.expires_at {
        return Err(CliError::new(
            "context_expired",
            "提取上下文已过期，请重新查询",
        ));
    }
    Ok(scope)
}

impl Scope {
    pub(super) fn new(
        db: &Db,
        home: &NoEndingHome,
        kind: &str,
        id: &str,
        mode: Mode,
    ) -> Result<Self> {
        let mut scope = Self {
            kind: kind.into(),
            target: id.into(),
            mode,
            project: None,
            sessions: vec![],
            collaboration: None,
            delivered: HashMap::new(),
            saved: BTreeMap::new(),
            failures: BTreeMap::new(),
            final_receipt: None,
            expires_at: chrono::Utc::now() + chrono::Duration::hours(24),
        };
        match kind {
            "session" => scope.sessions.push(SessionSnapshot::read(
                db,
                db.get_session(id)?.ok_or_else(stale)?,
                mode.regenerate(),
            )?),
            "task" => {
                let conn = db.read();
                let tx = conn.unchecked_transaction()?;
                scope.collaboration =
                    Some(collaboration::Snapshot::read(&tx, id, mode.regenerate())?);
                tx.commit()?;
            }
            "project" => {
                let (input, revision) = {
                    let conn = db.read();
                    (
                        project::ProjectInput::read(&conn, id)?,
                        crate::storage::project_context_repo::state(&conn, id)?.revision,
                    )
                };
                scope.project = Some(ProjectGuard {
                    fingerprint: input.fingerprint.clone(),
                    header: input.header(home),
                    revision,
                });
                for sid in input.session_ids {
                    scope.sessions.push(SessionSnapshot::read(
                        db,
                        db.get_session(&sid)?.ok_or_else(stale)?,
                        false,
                    )?);
                }
                scope.sessions.sort_by(|a, b| {
                    a.last_message_at()
                        .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
                        .cmp(
                            &b.last_message_at()
                                .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok()),
                        )
                        .then_with(|| a.session.id.cmp(&b.session.id))
                });
            }
            _ => return Err(other("未知提取目标")),
        }
        scope.validate_target(db)?;
        Ok(scope)
    }

    pub(super) fn kind(&self) -> &str {
        &self.kind
    }

    fn target_key(&self) -> String {
        format!("{}-{}", self.kind, self.target)
    }

    pub(super) fn pending_session_ids(&self) -> impl Iterator<Item = &str> {
        self.sessions
            .iter()
            .filter(|s| s.pending())
            .map(|s| s.session.id.as_str())
    }

    pub(super) fn project_summary_progress(
        &self,
    ) -> Option<crate::operations::ProjectSummaryProgress> {
        self.project.as_ref()?;
        Some(crate::operations::ProjectSummaryProgress {
            processed: self.saved.len(),
            remaining: self.pending_session_ids().count(),
            summary_saved: self.final_receipt.is_some(),
        })
    }

    pub(super) fn refresh_empty_session(&self, db: &Db) -> Result<Option<Value>> {
        if self.kind != "session" {
            return Ok(None);
        }
        let snapshot = &self.sessions[0];
        if snapshot.latest != 0 || !snapshot.pending() {
            return Ok(None);
        }
        // An empty replacement needs only a generation update. There is no
        // new summary to extract and the last valid one remains available.
        let revision = db.tx(|tx| {
            let summary = query::target_summary(tx, "session", &self.target, snapshot.revision())?
                .unwrap_or_default();
            snapshot.commit(tx, &SessionContextFields { summary })
        })?;
        Ok(Some(json!(SessionUpdateOutcome {
            session_id: self.target.clone(),
            status: ContextUpdateStatus::Updated,
            revision,
        })))
    }

    fn validate_project(&self, conn: &rusqlite::Connection, project: &ProjectGuard) -> Result<()> {
        if crate::storage::project_context_repo::state(conn, &self.target)?.revision
            != project.revision
            || project::ProjectInput::read(conn, &self.target)
                .map_err(|error| match error {
                    AppError::Other(_) => stale(),
                    error => error,
                })?
                .fingerprint
                != project.fingerprint
        {
            return Err(stale());
        }
        Ok(())
    }

    pub(super) fn validate_target(&self, db: &Db) -> Result<()> {
        let conn = db.read();
        if let Some(project) = &self.project {
            self.validate_project(&conn, project)
        } else if let Some(snapshot) = &self.collaboration {
            snapshot.validate(&conn)
        } else {
            self.sessions[0].validate(&conn)
        }
    }

    fn input_names(&self) -> Vec<String> {
        let mut names = vec![format!("{}.jsonl", self.target_key())];
        if self.project.is_some() {
            names.extend(
                self.pending_session_ids()
                    .map(|id| format!("session-{id}.jsonl")),
            );
        }
        names
    }

    pub(super) fn describe(&self, db: &Db) -> Result<Value> {
        let conn = db.read();
        let inputs = self
            .input_names()
            .into_iter()
            .map(|name| {
                let record_count = self.input(&name)?.count(&conn)?;
                Ok(json!({"name":name,"record_count":record_count}))
            })
            .collect::<Result<Vec<_>>>()?;
        let pending_sessions = if self.project.is_some() {
            self.sessions.iter().filter(|s| s.pending()).map(|s| json!({
                "session_id":s.session.id,"title":s.session.title,"input":format!("session-{}.jsonl",s.session.id)
            })).collect::<Vec<_>>()
        } else {
            vec![]
        };
        Ok(
            json!({"kind":self.kind,"id":self.target,"mode":self.mode,"expires_at":self.expires_at,
            "inputs":inputs,"pending_sessions":pending_sessions,"result_key":self.target_key()}),
        )
    }

    fn input(&self, name: &str) -> Result<query::Input<'_>> {
        if name == format!("{}.jsonl", self.target_key()) {
            if let Some(project) = &self.project {
                return Ok(query::Input::Batch {
                    id: &self.target,
                    header: &project.header,
                    revision: project.revision,
                    regenerate: self.mode.regenerate(),
                    sessions: &self.sessions,
                });
            }
            if let Some(snapshot) = &self.collaboration {
                return Ok(query::Input::Collaboration { snapshot });
            }
            return Ok(self.session_input(&self.sessions[0]));
        }
        if self.project.is_some() {
            if let Some(id) = name
                .strip_prefix("session-")
                .and_then(|s| s.strip_suffix(".jsonl"))
            {
                return Ok(self.session_input(self.pending_session(id)?));
            }
        }
        Err(invalid("输入不在本次提取范围内"))
    }

    fn session_input<'a>(&self, session: &'a SessionSnapshot) -> query::Input<'a> {
        query::Input::Session {
            snapshot: session,
            include_summary: (self.project.is_some() || !self.mode.regenerate())
                && session.summary_generation == Some(session.generation),
        }
    }

    fn pending_session(&self, id: &str) -> Result<&SessionSnapshot> {
        self.sessions
            .iter()
            .find(|s| s.session.id == id && s.pending())
            .ok_or_else(|| invalid("会话不在本次待处理范围内"))
    }

    pub(super) fn read(
        &mut self,
        db: &Db,
        name: &str,
        cursor: usize,
        limit: usize,
        search: Option<&str>,
    ) -> Result<Value> {
        if !(1..=50).contains(&limit) || search.is_some_and(|text| text.trim().is_empty()) {
            return Err(invalid("limit 必须为 1～50，搜索内容不能为空"));
        }
        if self.project.is_some() && name == format!("{}.jsonl", self.target_key()) {
            self.validate_target(db)?;
            self.require_project_ready()?;
        }
        let page = self.input(name)?.page(db, cursor, limit, search)?;
        if search.is_none() {
            self.delivered.entry(name.into()).or_default().extend(
                page["records"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| r["cursor"].as_u64().unwrap() as usize),
            );
        }
        Ok(page)
    }

    fn require_read(&self, conn: &rusqlite::Connection, name: &str) -> Result<()> {
        let count = self.input(name)?.count(conn)?;
        if self.delivered.get(name).map_or(0, HashSet::len) != count {
            return Err(invalid(format!("输入尚未完整读取：{name}")));
        }
        Ok(())
    }

    fn require_project_ready(&self) -> Result<()> {
        if self.pending_session_ids().next().is_some() {
            return Err(invalid("仍有会话摘要待提交，请先处理待更新会话"));
        }
        Ok(())
    }

    pub(super) fn submit(
        &mut self,
        db: &Db,
        home: &NoEndingHome,
        submission: Submission,
    ) -> Result<Value> {
        let canonical = serde_json::to_value(&submission)?;
        let hash = format!("{:x}", Sha256::digest(serde_json::to_vec(&canonical)?));
        let Submission {
            session_id,
            reply,
            results,
        } = submission;
        if let Some(receipt) = session_id.as_deref().and_then(|id| self.saved.get(id)) {
            return receipt.replay(&hash);
        }
        if let Some(receipt) = &self.final_receipt {
            return receipt.replay(&hash);
        }
        let reply = summary::parse_session_reply(&reply.to_string()).map_err(invalid)?;
        self.validate_target(db)?;
        if let Some(id) = session_id {
            let project = self
                .project
                .as_ref()
                .ok_or_else(|| invalid("只有项目上下文可提交子会话"))?;
            let snapshot = self.pending_session(&id)?;
            if let SessionReply::Failed { error } = reply {
                if !results.is_empty() {
                    return Err(invalid("失败会话不应附带摘要"));
                }
                let reason = error
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(240)
                    .collect::<String>();
                if self.failures.get(&id) != Some(&reason) {
                    diagnostics::record_checkpoint(
                        home,
                        &self.target,
                        &SessionFailure {
                            session_id: id.clone(),
                            error: reason.clone(),
                        },
                    );
                    self.failures.insert(id.clone(), reason);
                }
                return Ok(json!({"session_id":id,"status":"failed","saved":false}));
            }
            let saved = self.commit_session(db, snapshot, &reply, results, Some(project))?;
            let session = self
                .sessions
                .iter_mut()
                .find(|s| s.session.id == id)
                .unwrap();
            session.revision = saved.revision;
            session.summary_generation = Some(session.generation);
            session.from = session.latest;
            session.summary_through_at = session.last_message_at.clone();
            self.delivered.remove(&format!("session-{id}.jsonl"));
            self.failures.remove(&id);
            let value = json!(saved);
            self.saved.insert(
                id,
                SubmissionReceipt {
                    hash,
                    value: value.clone(),
                },
            );
            return Ok(value);
        }
        if let SessionReply::Failed { error } = reply {
            if !results.is_empty() {
                return Err(invalid("失败的提取不应附带摘要"));
            }
            return Err(AppError::context(ContextUpdateError::ModelCallFailed(
                error,
            )));
        }
        let value = if let Some(value) = self.no_change(db)? {
            if !results.is_empty() {
                return Err(invalid("当前范围没有需要保存的摘要"));
            }
            value
        } else if let Some(project) = &self.project {
            self.require_project_ready()?;
            let fields = {
                let conn = db.read();
                self.require_read(&conn, &format!("{}.jsonl", self.target_key()))?;
                summary::fields(
                    &conn,
                    "project",
                    &self.target,
                    project.revision,
                    &reply,
                    results,
                )?
            };
            let revision = project::commit(
                db,
                &self.target,
                &project.fingerprint,
                project.revision,
                &self.sessions,
                &fields.summary,
            )?;
            json!(self.project_outcome(db, submitted_status(&reply), revision)?)
        } else if let Some(snapshot) = &self.collaboration {
            let fields = {
                let conn = db.read();
                self.require_read(&conn, &format!("{}.jsonl", self.target_key()))?;
                summary::fields(
                    &conn,
                    "task",
                    &self.target,
                    snapshot.revision(),
                    &reply,
                    results,
                )?
            };
            json!(snapshot.commit(db, &fields.summary, submitted_status(&reply))?)
        } else {
            json!(self.commit_session(db, &self.sessions[0], &reply, results, None)?)
        };
        self.final_receipt = Some(SubmissionReceipt {
            hash,
            value: value.clone(),
        });
        Ok(value)
    }

    fn commit_session(
        &self,
        db: &Db,
        snapshot: &SessionSnapshot,
        reply: &SessionReply,
        results: HashMap<String, Value>,
        project: Option<&ProjectGuard>,
    ) -> Result<SessionUpdateOutcome> {
        let id = &snapshot.session.id;
        let revision = db.tx(|tx| {
            if let Some(project) = project {
                self.validate_project(tx, project)?;
            }
            self.require_read(tx, &format!("session-{id}.jsonl"))?;
            let fields = summary::fields(tx, "session", id, snapshot.revision(), reply, results)?;
            snapshot.commit(tx, &fields)
        })?;
        Ok(SessionUpdateOutcome {
            session_id: id.clone(),
            status: submitted_status(reply),
            revision,
        })
    }

    pub(super) fn failures(&self) -> Vec<SessionFailure> {
        self.failures
            .iter()
            .map(|(id, error)| SessionFailure {
                session_id: id.clone(),
                error: error.clone(),
            })
            .collect()
    }

    fn project_outcome(
        &self,
        db: &Db,
        status: ContextUpdateStatus,
        revision: i64,
    ) -> Result<ProjectUpdateOutcome> {
        let (skipped, updated) = self
            .saved
            .iter()
            .partition::<Vec<_>, _>(|(_, receipt)| receipt.value["status"] == "skipped");
        let view = project_context_view(db, &self.target)?;
        Ok(ProjectUpdateOutcome {
            project_id: self.target.clone(),
            status,
            context_revision: revision,
            updated_sessions: updated.into_iter().map(|(id, _)| id.clone()).collect(),
            skipped_session_ids: skipped.into_iter().map(|(id, _)| id.clone()).collect(),
            failed_sessions: self.failures(),
            remaining_pending: view.pending_sessions,
            remaining_session_summaries: view.pending_session_summaries,
        })
    }

    pub(super) fn outcome(&self, db: &Db) -> Result<Option<Value>> {
        if let Some(receipt) = &self.final_receipt {
            return Ok(Some(receipt.value.clone()));
        }
        if let Some(project) = &self.project {
            if !self.saved.is_empty() || !self.failures.is_empty() {
                return Ok(Some(json!(self.project_outcome(
                    db,
                    ContextUpdateStatus::Partial,
                    project.revision
                )?)));
            }
        }
        Ok(None)
    }

    pub(super) fn no_change(&self, db: &Db) -> Result<Option<Value>> {
        let value = if let Some(project) = &self.project {
            if self.mode.regenerate()
                || project::needs_update(db, &self.target, &project.fingerprint, &self.sessions)?
            {
                return Ok(None);
            }
            json!(self.project_outcome(db, ContextUpdateStatus::NoChange, project.revision)?)
        } else if let Some(snapshot) = &self.collaboration {
            if snapshot.needs_update(db)? {
                return Ok(None);
            }
            let view = workstream_context_view(db, &self.target)?;
            json!(WorkstreamUpdateOutcome {
                workstream_id: self.target.clone(),
                status: ContextUpdateStatus::NoChange,
                context_revision: view.context_revision,
                processed_through_seq: view.processed_through_seq,
                remaining_pending: view.pending_messages,
            })
        } else {
            let snapshot = &self.sessions[0];
            if snapshot.pending() {
                return Ok(None);
            }
            json!(SessionUpdateOutcome {
                session_id: self.target.clone(),
                status: ContextUpdateStatus::NoChange,
                revision: snapshot.revision(),
            })
        };
        Ok(Some(value))
    }
}

fn submitted_status(reply: &SessionReply) -> ContextUpdateStatus {
    if matches!(reply, SessionReply::Skipped { .. }) {
        ContextUpdateStatus::Skipped
    } else {
        ContextUpdateStatus::Updated
    }
}

pub(super) fn lock_scope(root: &Path, scope: &Scope) -> Result<Vec<fs::File>> {
    summary::lock_targets(
        root,
        &scope.target_key(),
        &scope
            .pending_session_ids()
            .map(str::to_owned)
            .collect::<Vec<_>>(),
    )
}

pub(crate) fn context(
    db: &Db,
    home: &NoEndingHome,
    kind: &str,
    id: &str,
    mode: Mode,
) -> CliResult<Value> {
    let scope = Scope::new(db, home, kind, id, mode)?;
    let context_id = crate::storage::new_id();
    let path = directory(home, &context_id)?;
    let mut description = scope.describe(db)?;
    description["context_id"] = json!(context_id);
    description["requirements"] = json!(prompts::PromptTemplates::default());
    transport::private_dir(&path)?;
    save(&path, &scope)?;
    Ok(description)
}

pub(crate) fn read(
    db: &Db,
    home: &NoEndingHome,
    id: &str,
    input: &str,
    cursor: usize,
    limit: usize,
    search: Option<&str>,
) -> CliResult<Value> {
    let path = directory(home, id)?;
    let _lock = crate::platform::file_lock::exclusive(&path.join("lock"))?;
    let mut scope = load(&path)?;
    let page = scope.read(db, input, cursor, limit, search)?;
    if search.is_none() {
        save(&path, &scope)?;
    }
    Ok(page)
}

pub(crate) fn submit(
    db: &Db,
    home: &NoEndingHome,
    id: &str,
    submission: Submission,
) -> CliResult<Value> {
    let path = directory(home, id)?;
    let _lock = crate::platform::file_lock::exclusive(&path.join("lock"))?;
    let mut scope = load(&path)?;
    // Keep the project lock for the membership guard, but only acquire the
    // session lock for the checkpoint being saved.
    let session = submission.session_id.iter().cloned().collect::<Vec<_>>();
    let _targets = summary::lock_targets(
        &diagnostics::runtime_dir(home),
        &scope.target_key(),
        &session,
    )
    .map_err(|error| {
        if matches!(&error, AppError::Io(io) if io.kind() == std::io::ErrorKind::WouldBlock) {
            CliError::new("summary_busy", "相关摘要正在提取，请稍后重试")
        } else {
            error.into()
        }
    })?;
    let value = scope.submit(db, home, submission)?;
    save(&path, &scope)?;
    Ok(value)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::tests::{append, seed, ServiceFixture};
    fn read_input(f: &ServiceFixture, c: &Value, input: &str) {
        let mut cursor = 0;
        loop {
            let page = read(
                f.db(),
                &f.home(),
                c["context_id"].as_str().unwrap(),
                input,
                cursor,
                1,
                None,
            )
            .unwrap();
            match page["next_cursor"].as_u64() {
                Some(next) => cursor = next as usize,
                None => break,
            }
        }
    }
    fn read_all(f: &ServiceFixture, c: &Value) {
        for input in c["inputs"].as_array().unwrap() {
            read_input(f, c, input["name"].as_str().unwrap());
        }
    }
    fn complete(results: HashMap<String, Value>) -> Submission {
        Submission {
            session_id: None,
            reply: json!({"status":"completed"}),
            results,
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
            complete(result(&id, "已核实结论")),
        )
        .unwrap();
        assert_eq!(out["status"], "updated");
        let view = session_context_view(f.db(), &id).unwrap();
        assert_eq!(view.processed_through_seq, 2);
        assert_eq!(view.latest_message_seq, 3);
        assert!(view.pending);
        assert_eq!(view.fields.unwrap().summary.items[0].title, "已核实结论");
        let replay = submit(
            f.db(),
            &f.home(),
            c["context_id"].as_str().unwrap(),
            complete(result(&id, "已核实结论")),
        )
        .unwrap();
        assert_eq!(replay, out);
        assert_eq!(session_context_view(f.db(), &id).unwrap().revision, 1);
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
            complete(result(&id, "must not persist")),
        )
        .unwrap_err();
        assert_eq!(err.code, "invalid_result");
        assert_eq!(session_context_view(f.db(), &id).unwrap().revision, 0);
        read_all(&f, &c);
        let invalid = HashMap::from([(format!("session-{id}"), json!({"summary":{"items":[]}}))]);
        assert!(submit(f.db(), &f.home(), cid, complete(invalid)).is_err());
        assert_eq!(session_context_view(f.db(), &id).unwrap().revision, 0);
        submit(f.db(), &f.home(), cid, complete(result(&id, "valid"))).unwrap();
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
                complete(result(&id, "stale")),
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
    fn project_context_keeps_scope_partial_failures_and_independent_task_items() {
        let kind = "project";
        let f = ServiceFixture::new();
        let a = f.add_session(2, 20);
        let b = f.add_session(1, 20);
        let outside = f.add_session(1, 20);
        let task = crate::context::tests::task(&f, &[&a, &b]);
        let (project, _) = crate::context::tests::project_fixture(&f, "direct-scope", &[&a, &b]);
        let target = &project.id;
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
        let cid = c["context_id"].as_str().unwrap();
        let project_name = format!("project-{target}.jsonl");
        let project_result = || {
            HashMap::from([(
                format!("project-{target}"),
                json!({"summary":{"items":[{"title":"完整范围结论","description":"A 与 B","details":[]}]}}),
            )])
        };
        assert!(read(f.db(), &f.home(), cid, &project_name, 0, 8, None).is_err());
        assert!(submit(f.db(), &f.home(), cid, complete(project_result())).is_err());
        let child = |id: &str| Submission {
            session_id: Some(id.into()),
            ..complete(result(id, "已保存的会话结论"))
        };
        assert!(submit(f.db(), &f.home(), cid, child(&outside)).is_err());
        assert!(submit(f.db(), &f.home(), cid, child(&a)).is_err());
        read_input(&f, &c, &format!("session-{a}.jsonl"));
        let saved = submit(f.db(), &f.home(), cid, child(&a)).unwrap();
        assert_eq!(submit(f.db(), &f.home(), cid, child(&a)).unwrap(), saved);
        assert_eq!(session_context_view(f.db(), &a).unwrap().revision, 1);
        submit(
            f.db(),
            &f.home(),
            cid,
            Submission {
                session_id: Some(b.clone()),
                reply: json!({"status":"failed","error":"材料不完整"}),
                results: HashMap::new(),
            },
        )
        .unwrap();
        assert!(submit(f.db(), &f.home(), cid, complete(project_result())).is_err());
        assert_eq!(session_context_view(f.db(), &b).unwrap().revision, 0);
        assert!(project_context_view(f.db(), target)
            .unwrap()
            .summary
            .is_none());
        assert_eq!(
            crate::workspace::task_items::list(f.db(), &task.id).unwrap(),
            vec![item]
        );
        let next = context(f.db(), &f.home(), kind, target, Mode::Update).unwrap();
        assert_eq!(next["pending_sessions"].as_array().unwrap().len(), 1);
        assert_eq!(next["pending_sessions"][0]["session_id"], b);
        read_input(&f, &next, &format!("session-{b}.jsonl"));
        let next_id = next["context_id"].as_str().unwrap();
        submit(f.db(), &f.home(), next_id, child(&b)).unwrap();
        read_input(&f, &next, &project_name);
        let out = submit(f.db(), &f.home(), next_id, complete(project_result())).unwrap();
        assert_eq!(out["status"], "updated");
        assert_eq!(out["remaining_pending"], 0);
        assert_eq!(session_context_view(f.db(), &outside).unwrap().revision, 0);
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
