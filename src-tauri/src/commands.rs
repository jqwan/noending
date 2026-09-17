//! Tauri command surface — the Application Domain API the UI talks to.
//! The Assistant (LLM runtime, later phase) consumes the same functions.

use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::domain::*;
use crate::error::{other, Result};
use crate::storage::{new_id, now, Db};

pub struct AppState {
    pub db: std::sync::Mutex<Db>,
    /// Guards against concurrent background sync jobs.
    pub sync_in_progress: std::sync::atomic::AtomicBool,
    /// In-memory store for prepared launches awaiting user confirmation.
    pub prepared_launches:
        std::sync::Mutex<std::collections::HashMap<String, crate::launcher::PreparedLaunch>>,
}

/// Spawn a background sync job: returns immediately, emits `sync-started`,
/// `sync-progress` (per session), `sync-completed` / `sync-failed`. A job
/// must never hold the DB lock across long operations — ingestion locks per
/// session and extraction runs lock-free.
fn spawn_sync_job<F>(app: &AppHandle, state: &State<AppState>, job: F) -> Result<()>
where
    F: FnOnce(&std::sync::Mutex<Db>, &dyn Fn(serde_json::Value)) -> Result<(usize, i64)>
        + Send
        + 'static,
{
    if state.sync_in_progress.swap(true, Ordering::SeqCst) {
        return Err(other("已有同步任务在进行中，请等待完成"));
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        let _ = handle.emit("sync-started", serde_json::json!({}));
        let state = handle.state::<AppState>();
        let notify = |payload: serde_json::Value| {
            let _ = handle.emit("sync-progress", payload);
        };
        let result = job(&state.db, &notify);
        state.sync_in_progress.store(false, Ordering::SeqCst);
        match result {
            Ok((discovered, events)) => {
                let _ = handle.emit(
                    "sync-completed",
                    serde_json::json!({ "discovered": discovered, "events": events }),
                );
            }
            Err(e) => {
                let _ = handle.emit("sync-failed", serde_json::json!({ "error": e.to_string() }));
            }
        }
    });
    Ok(())
}

fn with_db<T>(state: &AppState, f: impl FnOnce(&Db) -> Result<T>) -> Result<T> {
    let guard = state.db.lock().map_err(|_| other("db lock poisoned"))?;
    f(&guard)
}

// ---------------- Projects ----------------

#[tauri::command]
pub fn create_project(
    state: State<AppState>,
    name: String,
    description: String,
) -> Result<Project> {
    crate::storage::ensure_not_empty("项目名称", &name)?;
    let p = Project {
        id: new_id(),
        name: name.trim().to_string(),
        description: description.trim().to_string(),
        archived: false,
        created_at: now(),
        updated_at: now(),
    };
    with_db(&state, |db| db.upsert_project(&p))?;
    Ok(p)
}

#[tauri::command]
pub fn update_project(state: State<AppState>, project: Project) -> Result<()> {
    with_db(&state, |db| {
        let mut p = project;
        p.updated_at = now();
        db.upsert_project(&p)
    })
}

#[tauri::command]
pub fn list_projects(state: State<AppState>) -> Result<Vec<Project>> {
    with_db(&state, |db| db.list_projects())
}

/// Deleting a Project only detaches: Workstreams and Sessions survive with
/// `project_id = NULL`. A Project is an optional organization layer, never
/// the lifecycle owner of a Workstream — nothing is archived or deleted.
#[tauri::command]
pub fn delete_project(state: State<AppState>, project_id: String) -> Result<()> {
    with_db(&state, |db| db.delete_project(&project_id))
}

#[tauri::command]
pub fn add_project_resource(
    state: State<AppState>,
    project_id: String,
    kind: String,
    uri: Option<String>,
) -> Result<ProjectResource> {
    let r = ProjectResource {
        id: new_id(),
        project_id,
        kind,
        uri,
        metadata: serde_json::json!({}),
        created_at: now(),
    };
    with_db(&state, |db| db.add_resource(&r))?;
    Ok(r)
}

#[tauri::command]
pub fn list_project_resources(
    state: State<AppState>,
    project_id: String,
) -> Result<Vec<ProjectResource>> {
    with_db(&state, |db| db.list_resources(&project_id))
}

#[tauri::command]
pub fn remove_project_resource(state: State<AppState>, resource_id: String) -> Result<()> {
    with_db(&state, |db| db.remove_resource(&resource_id))
}

// ---------------- Workstreams ----------------

#[tauri::command]
pub fn create_workstream(
    state: State<AppState>,
    project_id: Option<String>,
    title: String,
    description: String,
    default_cwd: Option<String>,
) -> Result<Workstream> {
    crate::storage::ensure_not_empty("Workstream 标题", &title)?;
    let default_cwd = default_cwd
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let w = Workstream {
        id: new_id(),
        project_id,
        title: title.trim().to_string(),
        description: description.trim().to_string(),
        lifecycle: "open".into(),
        visibility: "normal".into(),
        default_cwd,
        created_at: now(),
        updated_at: now(),
    };
    with_db(&state, |db| {
        db.upsert_workstream(&w)?;
        if let Some(pid) = &w.project_id {
            db.touch_project(pid)?;
        }
        Ok(())
    })?;
    Ok(w)
}

#[tauri::command]
pub fn update_workstream(state: State<AppState>, workstream: Workstream) -> Result<()> {
    with_db(&state, |db| {
        let mut w = workstream;
        w.updated_at = now();
        db.upsert_workstream(&w)
    })
}

#[tauri::command]
pub fn list_workstreams(
    state: State<AppState>,
    project_id: Option<String>,
) -> Result<Vec<Workstream>> {
    with_db(&state, |db| db.list_workstreams(project_id.as_deref()))
}

/// Card view for Home / Workstreams pages: everything a "continue working"
/// card needs, computed server-side so the UI stays a thin projection.
#[derive(Serialize)]
pub struct LatestSessionInfo {
    pub id: String,
    pub agent: String,
}

#[derive(Serialize)]
pub struct WorkstreamCardView {
    #[serde(flatten)]
    pub workstream: Workstream,
    pub project_name: Option<String>,
    /// L1 Current State text (content, falling back to its title).
    pub current_state: Option<String>,
    /// L1 Goal text — the card's last content fallback.
    pub goal: Option<String>,
    /// max(session activity, context edit, workstream update).
    pub last_activity_at: Option<String>,
    pub session_count: i64,
    /// Most recent bound session — the Resume button's agent + target.
    pub latest_session: Option<LatestSessionInfo>,
}

fn later_ts(a: &Option<String>, b: &Option<String>) -> Option<String> {
    let key = |s: &String| {
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|t| t.with_timezone(&chrono::Utc))
            .ok()
    };
    match (a, b) {
        (None, x) | (x, None) => x.clone(),
        (Some(x), Some(y)) => {
            let (xt, yt) = (key(x), key(y));
            match (xt, yt) {
                (Some(xt), Some(yt)) => {
                    if yt > xt {
                        Some(y.clone())
                    } else {
                        Some(x.clone())
                    }
                }
                // unparseable timestamps fall back to string order
                _ => {
                    if y.as_str() > x.as_str() {
                        Some(y.clone())
                    } else {
                        Some(x.clone())
                    }
                }
            }
        }
    }
}

#[tauri::command]
pub fn list_workstream_cards(state: State<AppState>) -> Result<Vec<WorkstreamCardView>> {
    with_db(&state, workstream_cards)
}

/// Compose card views from the real sources (bindings, items, workstreams).
/// Also the testable core of `list_workstream_cards`.
pub fn workstream_cards(db: &Db) -> Result<Vec<WorkstreamCardView>> {
    let project_names: std::collections::HashMap<String, String> = db
        .list_projects()?
        .into_iter()
        .map(|p| (p.id, p.name))
        .collect();
    let mut cards = Vec::new();
    for w in db.list_workstreams(None)? {
        let (session_count, latest, session_activity) = db.workstream_session_stats(&w.id)?;
        let items_activity = db.workstream_items_last_update(&w.id)?;
        // "Last active" only follows real work signals (session activity,
        // context edits) — renames or metadata touches must not make a
        // Workstream look freshly active. Sorting still falls back to
        // updated_at so a brand-new Workstream surfaces at the top.
        let mut last_activity_at = None;
        for candidate in [&session_activity, &items_activity] {
            last_activity_at = later_ts(&last_activity_at, candidate);
        }
        cards.push(WorkstreamCardView {
            project_name: w
                .project_id
                .as_ref()
                .and_then(|pid| project_names.get(pid).cloned()),
            current_state: db.workstream_state_text(&w.id, "current_state")?,
            goal: db.workstream_state_text(&w.id, "goal")?,
            last_activity_at,
            session_count,
            latest_session: latest.map(|(id, agent)| LatestSessionInfo { id, agent }),
            workstream: w,
        });
    }
    cards.sort_by(|a, b| {
        let key = |c: &WorkstreamCardView| {
            c.last_activity_at
                .as_deref()
                .or(Some(c.workstream.updated_at.as_str()))
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        };
        key(b).cmp(&key(a))
    });
    Ok(cards)
}

// ---------------- Default Agent (Settings → Default Agent) ----------------

const DEFAULT_AGENT_KEY: &str = "launcher.default_agent";

#[tauri::command]
pub fn get_default_agent(state: State<AppState>) -> Result<Option<String>> {
    with_db(&state, |db| {
        Ok(default_agent_of(db)?.map(|a| a.as_str().to_string()))
    })
}

/// Settings → Default Agent. Resolution order: the user's explicit choice
/// stays authoritative even when currently undetected (the UI warns instead
/// of silently substituting); without an explicit choice we fall back to the
/// first detected agent; with no detected agent at all the default is None
/// and New actions render disabled instead of failing at launch.
/// Storage errors propagate — a broken DB must not masquerade as "no agent".
pub fn default_agent_of(db: &Db) -> Result<Option<Agent>> {
    if let Some(v) = db.get_setting(DEFAULT_AGENT_KEY)? {
        if let Some(a) = Agent::parse(&v) {
            return Ok(Some(a));
        }
    }
    for a in Agent::all() {
        if db.get_installation(a)?.is_some() {
            return Ok(Some(a));
        }
    }
    Ok(None)
}

/// Startup detection snapshot (ExecutableResolver refresh): a successful
/// resolve upserts the cached installation, a failed one DELETES the row —
/// a CLI that disappeared must stop being reported as detected and stop
/// being auto-selected as default agent.
pub fn record_installation_probe(
    db: &Db,
    agent: Agent,
    resolved: Option<crate::platform::exec_resolver::AgentInstallation>,
) -> Result<()> {
    match resolved {
        Some(install) => db.save_installation(&install),
        None => db.delete_installation(agent),
    }
}

#[tauri::command]
pub fn set_default_agent(state: State<AppState>, agent: String) -> Result<()> {
    let agent = Agent::parse(&agent).ok_or_else(|| other("未知 Agent"))?;
    with_db(&state, |db| {
        db.set_setting(DEFAULT_AGENT_KEY, agent.as_str())
    })
}

// ---------------- Context Delivery Level (Settings → Context Delivery) ----------------

pub use crate::settings::{
    context_delivery_level_of, set_context_delivery_level as set_delivery_level,
    CONTEXT_DELIVERY_LEVEL_KEY,
};

#[tauri::command]
pub fn get_context_delivery_level(state: State<AppState>) -> Result<String> {
    with_db(&state, |db| {
        Ok(context_delivery_level_of(db)?.as_str().to_string())
    })
}

#[tauri::command]
pub fn set_context_delivery_level(state: State<AppState>, level: String) -> Result<()> {
    let lvl = crate::context::ContextDeliveryLevel::parse(&level)
        .ok_or_else(|| other("Invalid context delivery level"))?;
    with_db(&state, |db| {
        crate::settings::set_context_delivery_level(db, lvl)
    })
}

#[tauri::command]
pub fn archive_workstream(state: State<AppState>, workstream_id: String) -> Result<()> {
    with_db(&state, |db| {
        let mut w = db
            .get_workstream(&workstream_id)?
            .ok_or_else(|| other("Workstream 不存在"))?;
        w.visibility = if w.visibility == "archived" {
            "normal"
        } else {
            "archived"
        }
        .into();
        w.updated_at = now();
        db.upsert_workstream(&w)
    })
}

#[tauri::command]
pub fn merge_workstreams(
    state: State<AppState>,
    source_id: String,
    target_id: String,
) -> Result<()> {
    with_db(&state, |db| {
        let source = db
            .get_workstream(&source_id)?
            .ok_or_else(|| other("源 Workstream 不存在"))?;
        let _target = db
            .get_workstream(&target_id)?
            .ok_or_else(|| other("目标 Workstream 不存在"))?;
        // move items + bindings, mark source abandoned
        let items = db.items_for_workstream(&source_id, true)?;
        for (mut item, rev) in items {
            item.workstream_id = target_id.clone();
            item.updated_at = now();
            db.index_item(&item, &rev)?;
            db.0.execute(
                "UPDATE context_items SET workstream_id = ?2, updated_at = ?3 WHERE id = ?1",
                rusqlite::params![item.id, target_id, now()],
            )?;
        }
        let bindings = db.bindings_for_workstream(&source_id)?;
        for b in bindings {
            db.0.execute(
                "INSERT OR IGNORE INTO session_workstream_bindings
                 (session_id, workstream_id, role, source, confidence, last_seen_revision, last_sync_cursor, created_at, last_used_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![b.session_id, target_id, b.role, b.source, b.confidence, b.last_seen_revision, b.last_sync_cursor, b.created_at, b.last_used_at],
            )?;
        }
        let mut src = source;
        src.lifecycle = "abandoned".into();
        src.visibility = "archived".into();
        src.updated_at = now();
        db.upsert_workstream(&src)?;
        Ok(())
    })
}

// ---------------- Context Items ----------------

#[derive(Deserialize)]
pub struct NewItemArgs {
    pub workstream_id: String,
    pub kind: String,
    pub title: String,
    pub content: String,
}

#[tauri::command]
pub fn add_context_item(state: State<AppState>, args: NewItemArgs) -> Result<ContextItem> {
    crate::storage::ensure_not_empty("条目标题", &args.title)?;
    with_db(&state, |db| {
        let item = crate::sync::create_item(
            db,
            &args.workstream_id,
            &args.kind,
            args.title.trim(),
            &args.content,
            "user_explicit",
            "user_edit",
            &[],
            None,
            "user",
        )?;
        if let Some(ws) = db.get_workstream(&args.workstream_id)? {
            if let Some(pid) = &ws.project_id {
                db.touch_project(pid)?;
            }
        }
        Ok(item)
    })
}

#[derive(Deserialize)]
pub struct EditItemArgs {
    pub item_id: String,
    pub title: String,
    pub content: String,
}

#[tauri::command]
pub fn edit_context_item(state: State<AppState>, args: EditItemArgs) -> Result<()> {
    with_db(&state, |db| {
        let mut item = db
            .get_item(&args.item_id)?
            .ok_or_else(|| other("条目不存在"))?;
        let new_rev = ContextItemRevision {
            id: new_id(),
            item_id: item.id.clone(),
            title: args.title.trim().to_string(),
            content: args.content.clone(),
            metadata: serde_json::json!({
                "provenance": {
                    "authority": "user_edit",
                    "actor": "user",
                    "source_type": "user_edit",
                    "source_ref": serde_json::Value::Null,
                }
            }),
            source_type: Some("user_edit".into()),
            source_ref: None,
            sync_run_id: None,
            created_at: now(),
        };
        db.insert_revision(&new_rev)?;
        db.set_item_head(&item.id, &new_rev.id, None)?;
        db.set_item_authority(&item.id, "user_edit")?;
        item.updated_at = now();
        db.index_item(&item, &new_rev)?;
        Ok(())
    })
}

/// User-driven status change. Like every status transition this produces an
/// audit revision (previous/new status, actor, reason).
#[tauri::command]
pub fn set_item_status(state: State<AppState>, item_id: String, status: String) -> Result<()> {
    with_db(&state, |db| {
        db.apply_status_change(&item_id, &status, "user", "用户手动修改状态", None, &[])
    })
}

#[tauri::command]
pub fn get_item_history(
    state: State<AppState>,
    item_id: String,
) -> Result<Vec<ContextItemRevision>> {
    with_db(&state, |db| db.item_history(&item_id))
}

#[tauri::command]
pub fn delete_context_item(state: State<AppState>, item_id: String) -> Result<()> {
    with_db(&state, |db| {
        db.apply_status_change(&item_id, "deleted", "user", "用户删除", None, &[])?;
        db.unindex("item", &item_id);
        Ok(())
    })
}

#[derive(Serialize)]
pub struct WorkstreamContext {
    pub workstream: Workstream,
    pub project_name: Option<String>,
    pub core: Vec<crate::context::ContextSection>,
    pub items: Vec<(ContextItem, ContextItemRevision)>,
    pub related_sessions: Vec<Session>,
    pub conflicts: Vec<ContextConflict>,
    pub conflict_cases: Vec<crate::domain::ConflictReviewCase>,
    pub relations: Vec<ContextItemRelation>,
    pub recent_changes: Vec<ContextChange>,
}

#[tauri::command]
pub fn get_workstream_context(
    state: State<AppState>,
    workstream_id: String,
) -> Result<WorkstreamContext> {
    with_db(&state, |db| {
        let workstream = db
            .get_workstream(&workstream_id)?
            .ok_or_else(|| other("Workstream 不存在"))?;
        let core = crate::context::resolve_core_context(db, &workstream_id)?;
        let items = db.items_for_workstream(&workstream_id, true)?;
        let sessions = db
            .bindings_for_workstream(&workstream_id)?
            .into_iter()
            .filter_map(|b| db.get_session(&b.session_id).ok().flatten())
            .collect();
        let conflicts = db.conflicts_for_workstream(&workstream_id, false)?;
        let conflict_cases = db.list_conflict_review_cases(&workstream_id, false)?;
        let relations = db.item_relations_for_workstream(&workstream_id)?;
        let recent_changes = db.list_workstream_context_changes(&workstream_id, 20)?;
        let project_name = workstream
            .project_id
            .as_deref()
            .map(|pid| db.get_project(pid).ok().flatten().map(|p| p.name))
            .flatten();
        Ok(WorkstreamContext {
            workstream,
            project_name,
            core,
            items,
            related_sessions: sessions,
            conflicts,
            conflict_cases,
            relations,
            recent_changes,
        })
    })
}

#[tauri::command]
pub fn get_context_revision_source(
    state: State<AppState>,
    revision_id: String,
) -> Result<Option<ContextSourceDetail>> {
    with_db(&state, |db| db.get_context_revision_source(&revision_id))
}

// ---------------- Conflicts ----------------

#[tauri::command]
pub fn list_conflicts(
    state: State<AppState>,
    workstream_id: String,
    include_closed: Option<bool>,
) -> Result<Vec<ContextConflict>> {
    with_db(&state, |db| {
        db.conflicts_for_workstream(&workstream_id, include_closed.unwrap_or(false))
    })
}

#[tauri::command]
pub fn get_conflict_review_case(
    state: State<AppState>,
    conflict_id: String,
) -> Result<Option<crate::domain::ConflictReviewCase>> {
    with_db(&state, |db| db.get_conflict_review_case(&conflict_id))
}

#[tauri::command]
pub fn list_conflict_review_cases(
    state: State<AppState>,
    workstream_id: String,
    include_closed: Option<bool>,
) -> Result<Vec<crate::domain::ConflictReviewCase>> {
    with_db(&state, |db| {
        db.list_conflict_review_cases(&workstream_id, include_closed.unwrap_or(false))
    })
}

#[derive(Deserialize)]
pub struct ResolveConflictWithEditArgs {
    pub conflict_id: String,
    pub status: String,
    pub resolution: Option<String>,
    pub edit: Option<crate::domain::ContextItemEditPayload>,
}

#[tauri::command]
pub fn resolve_conflict_with_edit(
    state: State<AppState>,
    args: ResolveConflictWithEditArgs,
) -> Result<()> {
    with_db(&state, |db| {
        db.resolve_conflict_with_edit(
            &args.conflict_id,
            &args.status,
            args.resolution.as_deref(),
            args.edit.as_ref(),
            "user",
        )
    })
}

#[tauri::command]
pub fn resolve_conflict(
    state: State<AppState>,
    conflict_id: String,
    status: String,
    resolution: Option<String>,
) -> Result<()> {
    with_db(&state, |db| {
        db.resolve_conflict_with_edit(&conflict_id, &status, resolution.as_deref(), None, "user")
    })
}

// ---------------- Sessions ----------------

#[derive(Serialize)]
pub struct SessionDetail {
    pub session: Session,
    pub events: Vec<SessionEvent>,
    pub bindings: Vec<(SessionWorkstreamBinding, Option<WorkstreamTitle>)>,
    pub cursor: i64,
    pub processed_cursor: i64,
    pub classification: String,
}

pub type WorkstreamTitle = String;

#[tauri::command]
pub fn list_sessions(
    state: State<AppState>,
    project_id: Option<String>,
    agent: Option<String>,
) -> Result<Vec<Session>> {
    let agent = agent.and_then(|a| Agent::parse(&a));
    with_db(&state, |db| {
        db.list_sessions(crate::storage::SessionFilter { project_id, agent })
    })
}

#[tauri::command]
pub fn get_session_detail(state: State<AppState>, session_id: String) -> Result<SessionDetail> {
    with_db(&state, |db| {
        let session = db
            .get_session(&session_id)?
            .ok_or_else(|| other("Session 不存在"))?;
        let events = db.get_events(&session_id, None, 500)?;
        let cursor = db.get_cursor(&session_id)?;
        let processed_cursor = db.get_processed_sequence(&session_id)?;
        let bindings = db
            .bindings_for_session(&session_id)?
            .into_iter()
            .map(|b| {
                let title = db.get_workstream(&b.workstream_id)?.map(|w| w.title);
                Ok((b, title))
            })
            .collect::<Result<Vec<_>>>()?;
        let classification = SessionClassificationState::derive(&{
            bindings.iter().map(|(b, _)| b.clone()).collect::<Vec<_>>()
        });
        Ok(SessionDetail {
            session,
            events,
            bindings,
            cursor,
            processed_cursor,
            classification: classification.as_str().to_string(),
        })
    })
}

#[tauri::command]
pub fn assign_session_project(
    state: State<AppState>,
    session_id: String,
    project_id: Option<String>,
) -> Result<()> {
    with_db(&state, |db| {
        db.0.execute(
            "UPDATE sessions SET project_id = ?2 WHERE id = ?1",
            rusqlite::params![session_id, project_id],
        )?;
        // a user correction is itself the strongest kind of evidence
        if let Some(pid) = project_id {
            db.insert_evidence(&ProjectAffinityEvidence {
                id: new_id(),
                session_id: Some(session_id),
                workstream_id: None,
                project_id: pid,
                evidence_type: "user_correction".into(),
                source: "manual assignment".into(),
                score: 10.0,
                created_at: now(),
            })?;
        }
        Ok(())
    })
}

/// cwd/repo only ever *suggest*: return the scored suggestion from recorded
/// evidence, never assign anything.
#[tauri::command]
pub fn suggest_session_project(
    state: State<AppState>,
    session_id: String,
) -> Result<serde_json::Value> {
    with_db(&state, |db| {
        let session = db
            .get_session(&session_id)?
            .ok_or_else(|| other("Session 不存在"))?;
        // record fresh evidence for the current cwd, then resolve
        crate::ingestion::record_session_project_evidence(db, &session);
        match db.resolve_project_affinity(&session_id)? {
            Some((project_id, score)) => {
                let name = db
                    .get_project(&project_id)?
                    .map(|p| p.name)
                    .unwrap_or_default();
                Ok(
                    serde_json::json!({ "project_id": project_id, "project_name": name, "score": score }),
                )
            }
            None => Ok(serde_json::json!({ "project_id": null, "score": 0.0 })),
        }
    })
}

#[tauri::command]
pub fn bind_session_workstream(
    state: State<AppState>,
    session_id: String,
    workstream_id: String,
    role: String,
) -> Result<()> {
    with_db(&state, |db| {
        crate::launcher::record_binding(
            db,
            &session_id,
            &workstream_id,
            &role,
            binding_source::USER_ASSIGNED,
            1.0,
        )
    })
}

/// Remove a Session ↔ Workstream binding (user edit via Session Detail).
#[tauri::command]
pub fn unbind_session_workstream(
    state: State<AppState>,
    session_id: String,
    workstream_id: String,
) -> Result<()> {
    with_db(&state, |db| db.unbind(&session_id, &workstream_id))
}

#[derive(Deserialize)]
pub struct DesiredBinding {
    pub workstream_id: String,
    pub role: String,
}

/// Atomic replace of a Session's Workstream bindings (Binding Modal save):
/// the backend diffs desired vs. current inside one transaction — unchanged
/// rows keep their provenance/created_at/cursors, role edits update only the
/// role, removed rows are deleted, and only newly added rows become
/// user_assigned. The frontend never orchestrates unbind+bind itself.
#[tauri::command]
pub fn replace_session_bindings(
    state: State<AppState>,
    session_id: String,
    bindings: Vec<DesiredBinding>,
) -> Result<()> {
    let desired = bindings
        .into_iter()
        .map(|b| (b.workstream_id, b.role))
        .collect::<Vec<_>>();
    with_db(&state, |db| {
        crate::launcher::replace_session_bindings(db, &session_id, &desired)
    })
}

/// Binding rows with workstream titles — lets the Sessions table show a
/// Workstream column and Assigned filters without N queries.
#[derive(Serialize)]
pub struct SessionBindingRow {
    pub session_id: String,
    pub workstream_id: String,
    pub role: String,
    pub workstream_title: String,
}

#[tauri::command]
pub fn list_session_bindings(state: State<AppState>) -> Result<Vec<SessionBindingRow>> {
    with_db(&state, |db| {
        let mut st = db.0.prepare(
            "SELECT b.session_id, b.workstream_id, b.role, COALESCE(w.title, b.workstream_id)
                 FROM session_workstream_bindings b
                 LEFT JOIN workstreams w ON w.id = b.workstream_id",
        )?;
        let rows = st
            .query_map([], |r| {
                Ok(SessionBindingRow {
                    session_id: r.get(0)?,
                    workstream_id: r.get(1)?,
                    role: r.get(2)?,
                    workstream_title: r.get(3)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}

/// App info for Settings → Data & Advanced (paths only, no secrets).
#[derive(Serialize)]
pub struct AppInfo {
    pub db_path: String,
    pub app_data_dir: String,
}

#[tauri::command]
pub fn get_app_info(app: AppHandle) -> AppInfo {
    let dir = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir());
    AppInfo {
        db_path: dir.join("noending.db").to_string_lossy().to_string(),
        app_data_dir: dir.to_string_lossy().to_string(),
    }
}

// ---------------- Sync ----------------

/// 同步全部启用的数据源（后台执行，不阻塞前端）。
#[tauri::command]
pub fn sync_all(app: AppHandle, state: State<AppState>) -> Result<serde_json::Value> {
    spawn_sync_job(&app, &state, |db_lock, notify| {
        let engine = {
            let guard = db_lock.lock().map_err(|_| other("db lock poisoned"))?;
            crate::sync::SyncEngine::from_settings(&guard)
        };
        crate::ingestion::reconcile_with_engine(db_lock, &engine, &|s| {
            notify(serde_json::json!({
                "agent": s.agent.as_str(),
                "title": s.title,
            }));
        })
    })?;
    Ok(serde_json::json!({ "started": true }))
}

/// 只同步某一个数据源（后台执行）。
#[tauri::command]
pub fn sync_source(
    app: AppHandle,
    state: State<AppState>,
    source_id: String,
) -> Result<serde_json::Value> {
    spawn_sync_job(&app, &state, move |db_lock, notify| {
        let source = {
            let guard = db_lock.lock().map_err(|_| other("db lock poisoned"))?;
            guard
                .get_ingest_source(&source_id)?
                .ok_or_else(|| other("数据源不存在"))?
        };
        let engine = {
            let guard = db_lock.lock().map_err(|_| other("db lock poisoned"))?;
            crate::sync::SyncEngine::from_settings(&guard)
        };
        crate::ingestion::reconcile_source(db_lock, &engine, &source, &|s| {
            notify(serde_json::json!({
                "agent": s.agent.as_str(),
                "title": s.title,
            }));
        })
    })?;
    Ok(serde_json::json!({ "started": true }))
}

/// 重新入库某一个数据源：清除该源会话已入库的事件与游标后重新抓取，
/// 绑定、上下文条目和审计历史保留（后台执行）。
#[tauri::command]
pub fn reingest_source(
    app: AppHandle,
    state: State<AppState>,
    source_id: String,
) -> Result<serde_json::Value> {
    spawn_sync_job(&app, &state, move |db_lock, notify| {
        let source = {
            let guard = db_lock.lock().map_err(|_| other("db lock poisoned"))?;
            guard
                .get_ingest_source(&source_id)?
                .ok_or_else(|| other("数据源不存在"))?
        };
        let engine = {
            let guard = db_lock.lock().map_err(|_| other("db lock poisoned"))?;
            crate::sync::SyncEngine::from_settings(&guard)
        };
        crate::ingestion::reingest_source(db_lock, &engine, &source, &|s| {
            notify(serde_json::json!({
                "agent": s.agent.as_str(),
                "title": s.title,
            }));
        })
    })?;
    Ok(serde_json::json!({ "started": true }))
}

#[tauri::command]
pub fn sync_session(state: State<AppState>, session_id: String) -> Result<serde_json::Value> {
    with_db(&state, |db| {
        let session = db
            .get_session(&session_id)?
            .ok_or_else(|| other("Session 不存在"))?;
        let engine = crate::sync::SyncEngine::from_settings(db);
        let applied = crate::launcher::sync_one_session_with_engine(db, &engine, &session)?;
        Ok(serde_json::json!({ "applied": applied }))
    })
}

#[tauri::command]
pub fn list_sync_runs(state: State<AppState>, limit: Option<i64>) -> Result<Vec<SyncRun>> {
    with_db(&state, |db| db.list_sync_runs(limit.unwrap_or(50)))
}

// ---------------- Launch intents ----------------

#[tauri::command]
pub fn list_launch_intents(
    state: State<AppState>,
    statuses: Option<Vec<String>>,
    limit: Option<i64>,
) -> Result<Vec<LaunchIntent>> {
    with_db(&state, |db| {
        let default = vec![
            launch_status::PENDING.to_string(),
            launch_status::AMBIGUOUS.to_string(),
        ];
        let statuses = statuses.unwrap_or(default);
        let refs: Vec<&str> = statuses.iter().map(|s| s.as_str()).collect();
        db.list_launch_intents(&refs, limit.unwrap_or(50))
    })
}

/// Manual resolution of an ambiguous LaunchIntent: the user picks which
/// discovered session the launch actually produced.
#[tauri::command]
pub fn resolve_launch_intent(
    state: State<AppState>,
    intent_id: String,
    session_id: String,
) -> Result<()> {
    with_db(&state, |db| {
        let intent = db
            .get_launch_intent(&intent_id)?
            .ok_or_else(|| other("LaunchIntent 不存在"))?;
        let session = db
            .get_session(&session_id)?
            .ok_or_else(|| other("Session 不存在"))?;
        crate::launcher::apply_match(db, &intent, &session)
    })
}

// ---------------- Launcher ----------------

#[tauri::command]
pub fn launch_new_session(
    app: AppHandle,
    state: State<AppState>,
    agent: String,
    workstream_ids: Vec<String>,
    cwd: Option<String>,
) -> Result<crate::launcher::LaunchResult> {
    let agent = Agent::parse(&agent).ok_or_else(|| other("未知 Agent"))?;
    let app_data = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir());
    let launcher = crate::launcher::SessionLauncher {
        app_data_dir: app_data,
    };
    with_db(&state, |db| {
        launcher.new_session(db, agent, &workstream_ids, cwd.as_deref())
    })
}

#[tauri::command]
pub fn launch_resume_session(
    app: AppHandle,
    state: State<AppState>,
    session_id: String,
    extra_workstream_ids: Vec<String>,
) -> Result<crate::launcher::LaunchResult> {
    let app_data = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir());
    let launcher = crate::launcher::SessionLauncher {
        app_data_dir: app_data,
    };
    with_db(&state, |db| {
        launcher.resume_session(db, &session_id, &extra_workstream_ids)
    })
}

/// Maximum time a prepared launch remains valid in memory before lazy cleanup (30 minutes).
pub const PREPARED_LAUNCH_TTL_SECS: i64 = 30 * 60;

/// Prune stale prepared launches whose `prepared_at` timestamp exceeds TTL.
/// Pure in-memory check without background thread.
pub fn prune_stale_prepared_launches(
    map: &mut std::collections::HashMap<String, crate::launcher::PreparedLaunch>,
) {
    let now = chrono::Utc::now();
    map.retain(|_, p| {
        if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(&p.prepared_at) {
            (now - ts.with_timezone(&chrono::Utc)).num_seconds() < PREPARED_LAUNCH_TTL_SECS
        } else {
            false
        }
    });
}

/// Atomically consume a prepared launch capability.
///
/// INVARIANT: PreparedLaunch = single-use capability.
/// Once consumed, it is removed immediately so no concurrent or repeated launch can reuse it.
pub fn consume_prepared_launch(
    map: &std::sync::Mutex<std::collections::HashMap<String, crate::launcher::PreparedLaunch>>,
    prepared_id: &str,
) -> Result<crate::launcher::PreparedLaunch> {
    let mut guard = map
        .lock()
        .map_err(|_| other("prepared_launches lock poisoned"))?;
    prune_stale_prepared_launches(&mut guard);
    guard
        .remove(prepared_id)
        .ok_or_else(|| other("Prepared launch 已被使用或已过期，请刷新预览"))
}

#[tauri::command]
pub fn prepare_new_session(
    app: AppHandle,
    state: State<AppState>,
    agent: String,
    workstream_ids: Vec<String>,
    cwd: Option<String>,
) -> Result<crate::launcher::PreparedLaunch> {
    let agent = Agent::parse(&agent).ok_or_else(|| other("未知 Agent"))?;
    let app_data = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir());
    let launcher = crate::launcher::SessionLauncher {
        app_data_dir: app_data,
    };
    let prepared = with_db(&state, |db| {
        launcher.prepare_new(db, agent, &workstream_ids, cwd.as_deref())
    })?;
    let mut map = state
        .prepared_launches
        .lock()
        .map_err(|_| other("prepared_launches lock poisoned"))?;
    prune_stale_prepared_launches(&mut map);
    map.insert(prepared.id.clone(), prepared.clone());
    Ok(prepared)
}

#[tauri::command]
pub fn prepare_resume_session(
    app: AppHandle,
    state: State<AppState>,
    session_id: String,
    extra_workstream_ids: Vec<String>,
) -> Result<crate::launcher::PreparedLaunch> {
    let app_data = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir());
    let launcher = crate::launcher::SessionLauncher {
        app_data_dir: app_data,
    };
    let prepared = with_db(&state, |db| {
        launcher.prepare_resume(db, &session_id, &extra_workstream_ids)
    })?;
    let mut map = state
        .prepared_launches
        .lock()
        .map_err(|_| other("prepared_launches lock poisoned"))?;
    prune_stale_prepared_launches(&mut map);
    map.insert(prepared.id.clone(), prepared.clone());
    Ok(prepared)
}

#[tauri::command]
pub fn launch_prepared(
    app: AppHandle,
    state: State<AppState>,
    prepared_id: String,
) -> Result<crate::launcher::LaunchResult> {
    let prepared = consume_prepared_launch(&state.prepared_launches, &prepared_id)?;

    let app_data = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir());
    let launcher = crate::launcher::SessionLauncher {
        app_data_dir: app_data,
    };
    with_db(&state, |db| launcher.launch_prepared(db, &prepared))
}

#[tauri::command]
pub fn cancel_prepared(state: State<AppState>, prepared_id: String) -> Result<()> {
    let mut map = state
        .prepared_launches
        .lock()
        .map_err(|_| other("prepared_launches lock poisoned"))?;
    prune_stale_prepared_launches(&mut map);
    map.remove(&prepared_id);
    Ok(())
}

// ---------------- Ingest sources ----------------

#[derive(Serialize)]
pub struct IngestSourceView {
    #[serde(flatten)]
    pub source: IngestSource,
    /// Whether the directory currently exists on disk (UI hint only).
    pub exists: bool,
}

fn source_view(src: IngestSource) -> IngestSourceView {
    let exists = std::path::Path::new(&src.path).is_dir();
    IngestSourceView {
        source: src,
        exists,
    }
}

#[tauri::command]
pub fn list_ingest_sources(state: State<AppState>) -> Result<Vec<IngestSourceView>> {
    with_db(&state, |db| {
        Ok(db
            .list_ingest_sources()?
            .into_iter()
            .map(|src| IngestSourceView {
                exists: std::path::Path::new(&src.path).is_dir(),
                source: src,
            })
            .collect())
    })
}

/// Add a custom scan root for an agent. The path is scanned recursively
/// with that agent's session-file rules and is enabled immediately —
/// adding it IS the user's opt-in.
#[tauri::command]
pub fn add_ingest_source(
    state: State<AppState>,
    agent: String,
    path: String,
) -> Result<IngestSourceView> {
    let agent = Agent::parse(&agent).ok_or_else(|| other("未知 Agent"))?;
    crate::storage::ensure_not_empty("数据源路径", &path)?;
    let expanded = crate::platform::paths::expand_tilde(&path);
    let canonical = expanded.to_string_lossy().to_string();
    if !expanded.is_dir() {
        return Err(other(format!("目录不存在: {}", canonical)));
    }
    with_db(&state, |db| {
        let src = db.add_ingest_source(agent, &canonical, true)?;
        Ok(source_view(src))
    })
}

#[tauri::command]
pub fn set_ingest_source_enabled(
    state: State<AppState>,
    source_id: String,
    enabled: bool,
) -> Result<()> {
    with_db(&state, |db| {
        db.set_ingest_source_enabled(&source_id, enabled)
    })
}

#[tauri::command]
pub fn remove_ingest_source(state: State<AppState>, source_id: String) -> Result<()> {
    with_db(&state, |db| db.remove_ingest_source(&source_id))
}

// ---------------- Search / misc ----------------

#[tauri::command]
pub fn search(
    state: State<AppState>,
    query: String,
    limit: Option<i64>,
) -> Result<Vec<crate::search::SearchHit>> {
    with_db(&state, |db| {
        crate::search::search(db, &query, limit.unwrap_or(30))
    })
}

#[tauri::command]
pub fn get_stats(state: State<AppState>) -> Result<serde_json::Value> {
    with_db(&state, |db| db.stats())
}

#[tauri::command]
pub fn get_agent_status(state: State<AppState>) -> Result<serde_json::Value> {
    let mut out = serde_json::Map::new();
    for agent in Agent::all() {
        let install = with_db(&state, |db| db.get_installation(agent))?;
        out.insert(
            agent.as_str().to_string(),
            serde_json::json!({
                "name": agent.display_name(),
                "detected": install.is_some(),
                "executable": install.as_ref().map(|i| i.executable_path.clone()),
                "version": install.as_ref().and_then(|i| i.version.clone()),
            }),
        );
    }
    Ok(serde_json::Value::Object(out))
}

// ---------------- Assistant ----------------

#[tauri::command]
pub fn assistant_send(
    app: AppHandle,
    state: State<AppState>,
    session_id: Option<String>,
    text: String,
) -> Result<crate::assistant::AssistantReply> {
    let reply = with_db(&state, |db| {
        crate::assistant::AssistantService::chat(db, session_id.as_deref(), &text)
    })?;
    let _ = app.emit("assistant-reply", &reply);
    Ok(reply)
}

#[tauri::command]
pub fn assistant_messages(
    state: State<AppState>,
    session_id: String,
) -> Result<Vec<crate::storage::AssistantMessageRow>> {
    with_db(&state, |db| db.list_assistant_messages(&session_id, 200))
}

#[tauri::command]
pub fn assistant_config_get(
    state: State<AppState>,
) -> Result<crate::sync::extractor::AssistantConfig> {
    with_db(&state, |db| {
        Ok(crate::sync::extractor::AssistantConfig::from_settings(db))
    })
}

#[tauri::command]
pub fn assistant_config_set(
    state: State<AppState>,
    agent: String,
    model: String,
    provider: String,
    effort: String,
) -> Result<()> {
    with_db(&state, |db| {
        crate::sync::extractor::AssistantConfig {
            agent,
            model,
            provider,
            effort,
        }
        .save(db)
    })
}

#[tauri::command]
pub fn assistant_execute_action(
    app: AppHandle,
    state: State<AppState>,
    action_json: String,
) -> Result<serde_json::Value> {
    let action: crate::assistant::ActionProposal =
        serde_json::from_str(&action_json).map_err(|e| other(format!("动作解析失败: {}", e)))?;
    let app_data = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir());
    with_db(&state, |db| {
        crate::assistant::AssistantService::execute_action(db, &action, &app_data)
    })
}
