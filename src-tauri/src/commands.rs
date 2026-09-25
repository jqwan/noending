//! Tauri command surface — the Application Domain API the UI talks to.
//! The Assistant (LLM runtime, later phase) consumes the same functions.
//!
//! Workspace Domain v0.2 split the Project / Workstream / Session-command areas
//! into sibling modules (`project`, `workstream`, `session_workspace`,
//! `workspace`) so their boundaries stay explicit.
//! This file owns the shared state and helpers the submodules borrow.

use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::domain::*;
use crate::error::{other, Result};
use crate::storage::{new_id, now, Db};

pub mod project;
pub mod session_lifecycle;
pub mod session_workspace;
pub mod workspace;
pub mod workstream;

pub use workstream::workstream_cards;

pub struct AppState {
    /// The store owns its own concurrency (one writer + a WAL reader), so the
    /// UI's reads never queue behind a background sync's writes.
    pub db: Db,
    /// Guards against concurrent background sync jobs.
    pub sync_in_progress: std::sync::atomic::AtomicBool,
    /// Guards against concurrent workspace reconciles (global AND targeted —
    /// both mutate the same registry, so one flag serializes them).
    pub workspace_refresh_in_progress: std::sync::atomic::AtomicBool,
    /// In-memory store for prepared launches awaiting user confirmation.
    pub prepared_launches:
        std::sync::Mutex<std::collections::HashMap<String, crate::launcher::PreparedLaunch>>,
}

/// Spawn a background sync job: returns immediately, emits `sync-started`,
/// `sync-progress` (per session), `sync-completed` / `sync-failed`. The store
/// serializes writes internally, so the job and the UI interleave freely.
fn spawn_sync_job<F>(app: &AppHandle, state: &State<AppState>, job: F) -> Result<()>
where
    F: FnOnce(&Db, &dyn Fn(serde_json::Value)) -> Result<(usize, i64)> + Send + 'static,
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

pub(crate) fn with_db<T>(state: &AppState, f: impl FnOnce(&Db) -> Result<T>) -> Result<T> {
    f(&state.db)
}

/// The launcher, pointed at NoEnding Home's `runtime/` (§42.3-M14).
///
/// The temp-dir fallback is deliberate: a context bundle is a launch artifact,
/// and failing a launch because the Home could not be resolved would trade a
/// cosmetic path for a broken primary action.
fn launcher_for(app: &AppHandle) -> crate::launcher::SessionLauncher {
    let runtime_dir = app
        .try_state::<crate::workspace::home::NoEndingHome>()
        .map(|h| h.inner().runtime_dir.clone())
        .unwrap_or_else(std::env::temp_dir);
    crate::launcher::SessionLauncher { runtime_dir }
}

/// The Home the app is actually running on, for the few commands that need it.
fn noending_home(app: &AppHandle) -> Option<crate::workspace::home::NoEndingHome> {
    app.try_state::<crate::workspace::home::NoEndingHome>()
        .map(|h| h.inner().clone())
}

/// §13 tier 3 — the non-database fact a launch needs.
///
/// Prepare and Launch must resolve this from the *same* Home, so both go through
/// here rather than one of them defaulting. `None` (Home never initialized)
/// removes the default-workspace tier from the chain instead of guessing a
/// directory, per [`crate::launcher::LaunchWorkspace`].
pub(crate) fn launch_workspace(app: &AppHandle) -> crate::launcher::LaunchWorkspace {
    noending_home(app)
        .map(|h| crate::launcher::LaunchWorkspace::from_home(&h))
        .unwrap_or_default()
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
        if db.get_installation(*a)?.is_some() {
            return Ok(Some(*a));
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

// ---------------- Context Intelligence (Base Experience switch) ----------------

/// Context Intelligence is a separate switch from delivery: turning delivery
/// Off stops outbound injection only, never extraction.
#[tauri::command]
pub fn get_context_intelligence_enabled(state: State<AppState>) -> Result<bool> {
    with_db(&state, |db| {
        crate::settings::context_intelligence_enabled(db)
    })
}

#[tauri::command]
pub fn set_context_intelligence_enabled(state: State<AppState>, enabled: bool) -> Result<()> {
    with_db(&state, |db| {
        crate::settings::set_context_intelligence_enabled(db, enabled)
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
        // §42.3-M19 — the Project this activity belongs to is its position-0
        // path's Project. The current projection is read from the primary
        // path: for a
        // migrated Workstream it can name a Project §7.4 deleted or §8.3
        // merged away, which would reorder `list_projects` (ORDER BY
        // updated_at) by a membership that no longer exists.
        let (pid, _) =
            crate::workspace::workstream::primary_project_for_workstream(db, &args.workstream_id)?;
        if let Some(pid) = pid {
            db.touch_project(&pid)?;
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
    /// Sessions that own this Workstream (方案 §25). At most one Workstream
    /// per Session, so a Session never appears in two of these lists.
    pub sessions: Vec<Session>,
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
        // §11 — the Workstream context view is a default projection: trashed
        // Sessions are not shown here (the query filters them out).
        let sessions = db.sessions_for_workstream(&workstream_id)?;
        let conflicts = db.conflicts_for_workstream(&workstream_id, false)?;
        let conflict_cases = db.list_conflict_review_cases(&workstream_id, false)?;
        let relations = db.item_relations_for_workstream(&workstream_id)?;
        let recent_changes = db.list_workstream_context_changes(&workstream_id, 20)?;
        // §42.3-M19: the name shown beside a Workstream is its position-0 path's
        // Project.
        let (_, project_name) =
            crate::workspace::workstream::primary_project_for_workstream(db, &workstream_id)?;
        Ok(WorkstreamContext {
            workstream,
            project_name,
            core,
            items,
            sessions,
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

#[tauri::command]
pub fn get_workstream_review_state(
    state: State<AppState>,
    workstream_id: String,
) -> Result<Option<WorkstreamReviewState>> {
    with_db(&state, |db| db.get_workstream_review_state(&workstream_id))
}

#[tauri::command]
pub fn get_workstream_review_window(
    state: State<AppState>,
    workstream_id: String,
) -> Result<WorkstreamReviewWindow> {
    with_db(&state, |db| db.get_workstream_review_window(&workstream_id))
}

#[tauri::command]
pub fn mark_workstream_reviewed(
    state: State<AppState>,
    workstream_id: String,
    frontier: ReviewFrontier,
) -> Result<WorkstreamReviewState> {
    with_db(&state, |db| {
        db.mark_workstream_reviewed(&workstream_id, &frontier)
    })
}

#[tauri::command]
pub fn get_workstream_review_summary(
    state: State<AppState>,
    workstream_id: String,
) -> Result<WorkstreamReviewSummary> {
    with_db(&state, |db| {
        db.get_workstream_review_summary(&workstream_id)
    })
}

#[tauri::command]
pub fn list_workstream_review_summaries(
    state: State<AppState>,
) -> Result<Vec<WorkstreamReviewSummary>> {
    with_db(&state, |db| db.list_workstream_review_summaries())
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

// ---------------- Sync ----------------

/// 同步全部启用的数据源（后台执行，不阻塞前端）。
#[tauri::command]
pub fn sync_all(app: AppHandle, state: State<AppState>) -> Result<serde_json::Value> {
    let workspace = launch_workspace(&app);
    spawn_sync_job(&app, &state, move |db, notify| {
        let engine = crate::sync::SyncEngine::from_settings(db);
        crate::ingestion::reconcile_with_engine(db, &engine, &workspace, &|s| {
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
    let workspace = launch_workspace(&app);
    spawn_sync_job(&app, &state, move |db, notify| {
        let source = db
            .get_ingest_source(&source_id)?
            .ok_or_else(|| other("数据源不存在"))?;
        let engine = crate::sync::SyncEngine::from_settings(db);
        crate::ingestion::reconcile_source(db, &engine, &source, &workspace, &|s| {
            notify(serde_json::json!({
                "agent": s.agent.as_str(),
                "title": s.title,
            }));
        })
    })?;
    Ok(serde_json::json!({ "started": true }))
}

/// 重新摄入某一个数据源：重置游标后重新抓取，已入库事件去重追加，
/// 所属任务（Owner）、上下文条目和审计历史保留（后台执行）。
#[tauri::command]
pub fn reingest_source(
    app: AppHandle,
    state: State<AppState>,
    source_id: String,
) -> Result<serde_json::Value> {
    let workspace = launch_workspace(&app);
    spawn_sync_job(&app, &state, move |db, notify| {
        let source = db
            .get_ingest_source(&source_id)?
            .ok_or_else(|| other("数据源不存在"))?;
        let engine = crate::sync::SyncEngine::from_settings(db);
        crate::ingestion::reingest_source(db, &engine, &source, &workspace, &|s| {
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
        // §3/§10 — a trashed session is inactive: sync is an explicit user
        // action here, so reject with a reason instead of silently no-op'ing.
        if session.is_trashed() {
            return Err(other("会话已在回收站，无法同步；请先恢复会话"));
        }
        let engine = crate::sync::SyncEngine::from_settings(db);
        let (ingested, applied) = crate::launcher::ingest_and_sync_session(db, &engine, &session)?;
        Ok(serde_json::json!({
            "applied": applied,
            "ingested": ingested,
            "context_processing_enabled": crate::settings::context_intelligence_enabled(db)?,
        }))
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
    app: AppHandle,
    state: State<AppState>,
    intent_id: String,
    session_id: String,
) -> Result<()> {
    let workspace = launch_workspace(&app);
    with_db(&state, |db| {
        let intent = db
            .get_launch_intent(&intent_id)?
            .ok_or_else(|| other("LaunchIntent 不存在"))?;
        let session = db
            .get_session(&session_id)?
            .ok_or_else(|| other("Session 不存在"))?;
        // §43 — owner/delivery writes are Session write paths: a trashed
        // session must not claim an intent. (Reconcile-side matching only
        // ever fires for brand-new sessions, which are never trashed.)
        if session.is_trashed() {
            return Err(other("会话已在回收站，无法关联启动记录"));
        }
        crate::launcher::apply_match(db, &intent, &session, &workspace)
    })
}

// ---------------- Launcher ----------------

#[tauri::command]
pub fn launch_new_session(
    app: AppHandle,
    state: State<AppState>,
    agent: String,
    owner_workstream_id: Option<String>,
    cwd: Option<String>,
) -> Result<crate::launcher::LaunchResult> {
    let agent = Agent::parse(&agent).ok_or_else(|| other("未知 Agent"))?;
    let launcher = launcher_for(&app);
    let workspace = launch_workspace(&app);
    with_db(&state, |db| {
        launcher.new_session_in(
            db,
            agent,
            owner_workstream_id.as_deref(),
            cwd.as_deref(),
            &workspace,
        )
    })
}

#[tauri::command]
pub fn launch_resume_session(
    app: AppHandle,
    state: State<AppState>,
    session_id: String,
) -> Result<crate::launcher::LaunchResult> {
    let launcher = launcher_for(&app);
    let workspace = launch_workspace(&app);
    with_db(&state, |db| {
        launcher.resume_session_in(db, &session_id, &workspace)
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
    owner_workstream_id: Option<String>,
    cwd: Option<String>,
) -> Result<crate::launcher::PreparedLaunch> {
    let agent = Agent::parse(&agent).ok_or_else(|| other("未知 Agent"))?;
    let launcher = launcher_for(&app);
    let workspace = launch_workspace(&app);
    let prepared = with_db(&state, |db| {
        launcher.prepare_new_in(
            db,
            agent,
            owner_workstream_id.as_deref(),
            cwd.as_deref(),
            &workspace,
        )
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
) -> Result<crate::launcher::PreparedLaunch> {
    let launcher = launcher_for(&app);
    let workspace = launch_workspace(&app);
    let prepared = with_db(&state, |db| {
        launcher.prepare_resume_in(db, &session_id, &workspace)
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

    let launcher = launcher_for(&app);
    let workspace = launch_workspace(&app);
    with_db(&state, |db| {
        launcher.launch_prepared_in(db, &prepared, &workspace)
    })
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
pub fn get_agent_status(state: State<AppState>) -> Result<serde_json::Value> {
    let mut out = serde_json::Map::new();
    for agent in Agent::all() {
        let install = with_db(&state, |db| db.get_installation(*agent))?;
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

// ---------------- Agent Runtime Configuration ----------------

/// What Settings → Agents renders for one Agent: install state plus the
/// override surface.
///
/// `models` is empty in this payload: discovery spawns the Agent CLI, so it
/// only ever runs through `refresh_agent_runtime_options`. A failed or absent
/// catalog never hides the saved override.
#[derive(Serialize)]
pub struct AgentRuntimeSettings {
    pub agent: Agent,
    pub detected: bool,
    pub executable: Option<String>,
    pub version: Option<String>,
    pub overrides: crate::agent_runtime::AgentRuntimeOverrides,
    pub capabilities: crate::agent_runtime::AgentRuntimeCapabilities,
    pub models: Vec<crate::agent_runtime::ModelOption>,
    pub model_source: &'static str,
    pub effort_levels: Vec<String>,
    pub warnings: Vec<String>,
}

fn agent_of(s: &str) -> Result<Agent> {
    Agent::parse(s).ok_or_else(|| other(format!("未知 Agent: {}", s)))
}

fn runtime_settings(db: &Db, agent: Agent) -> Result<AgentRuntimeSettings> {
    let install = db.get_installation(agent)?;
    Ok(AgentRuntimeSettings {
        agent,
        detected: install.is_some(),
        executable: install.as_ref().map(|i| i.executable_path.clone()),
        version: install.as_ref().and_then(|i| i.version.clone()),
        overrides: crate::agent_runtime::get_runtime_overrides(db, agent)?,
        capabilities: crate::agent_runtime::capabilities_of(agent),
        models: vec![],
        model_source: "not_loaded",
        effort_levels: crate::agent_runtime::discovery::effort_levels_for(agent),
        warnings: vec![],
    })
}

#[tauri::command]
pub fn get_agent_runtime_settings(
    state: State<AppState>,
    agent: String,
) -> Result<AgentRuntimeSettings> {
    let agent = agent_of(&agent)?;
    with_db(&state, |db| runtime_settings(db, agent))
}

#[tauri::command]
pub fn set_agent_runtime_overrides(
    state: State<AppState>,
    agent: String,
    overrides: crate::agent_runtime::AgentRuntimeOverrides,
) -> Result<AgentRuntimeSettings> {
    let agent = agent_of(&agent)?;
    with_db(&state, |db| {
        // Validates, then persists; an unsupported field never reaches storage.
        crate::agent_runtime::set_runtime_overrides(db, agent, &overrides)?;
        runtime_settings(db, agent)
    })
}

/// Run a blocking closure on Tauri's blocking worker pool. The one door every
/// potentially-slow (CLI-spawning) command body goes through; the join error
/// folds into the app error type.
pub(crate) async fn run_on_blocking_worker<T, F>(f: F) -> Result<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| other(format!("后台任务失败: {e}")))
}

/// The blocking-worker seam behind `refresh_agent_runtime_options` (方案 §7/§15):
/// the CLI probe (`codex debug models` / `pi --list-models`, up to
/// DISCOVERY_TIMEOUT_SECS) must run on a blocking worker thread, never on the
/// async command thread or the Tauri main thread. Split out as a named helper
/// so the threading property is testable without a Tauri harness.
pub(crate) async fn discover_runtime_options_async(
    agent: Agent,
) -> Result<crate::agent_runtime::AgentRuntimeDiscovery> {
    run_on_blocking_worker(move || crate::agent_runtime::discover_runtime_options(agent)).await
}

/// Advisory only: fetch the Agent's model catalog. Runs with no DB lock held
/// and cannot fail a launch — the caller renders `warnings` and falls back to
/// Agent default plus custom input.
///
/// Async on purpose: the Agent CLI probe blocks for up to 20s, so it goes to
/// `spawn_blocking` and the UI thread stays responsive (方案 §7). Explicit
/// user action only — opening Settings never reaches this command (§1).
#[tauri::command]
pub async fn refresh_agent_runtime_options(
    agent: String,
) -> Result<crate::agent_runtime::AgentRuntimeDiscovery> {
    let agent = agent_of(&agent)?;
    discover_runtime_options_async(agent).await
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

/// The Assistant's only runtime choice is *which* Agent answers; model /
/// provider / effort come from Settings → Agents like every other consumer.
#[tauri::command]
pub fn assistant_config_set(state: State<AppState>, agent: String) -> Result<()> {
    with_db(&state, |db| {
        if agent != "none" {
            let _ = agent_of(&agent)?;
        }
        crate::sync::extractor::AssistantConfig { agent }.save(db)
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
    let launcher = launcher_for(&app);
    let workspace = launch_workspace(&app);
    with_db(&state, |db| {
        crate::assistant::AssistantService::execute_action(db, &action, &launcher, &workspace)
    })
}

/// 方案 §15 — the async seam test: the refresh command's blocking CLI probe
/// must run on a blocking worker, never on the calling thread, and the seam
/// must deliver a usable discovery for an agent whose catalog is static
/// (no CLI spawn at all).
#[cfg(test)]
mod agent_runtime_refresh_seam_tests {
    use super::*;

    #[test]
    fn blocking_work_runs_on_a_worker_not_the_calling_thread() {
        let caller = std::thread::current().id();
        let worker =
            tauri::async_runtime::block_on(run_on_blocking_worker(|| std::thread::current().id()))
                .unwrap();
        assert_ne!(
            caller, worker,
            "spawn_blocking must move work off the calling thread"
        );
    }

    #[test]
    fn async_seam_returns_discovery_without_a_cli_spawn() {
        // Claude Code's catalog is suggested (static): the seam is exercised
        // end to end without spawning any CLI — safe in CI on both platforms.
        let discovery =
            tauri::async_runtime::block_on(discover_runtime_options_async(Agent::ClaudeCode))
                .unwrap();
        assert_eq!(discovery.model_source, "suggested");
        assert!(!discovery.models.is_empty());
        assert!(discovery.warnings.is_empty());
    }
}
