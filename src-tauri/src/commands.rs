//! Tauri command surface — the Application Domain API the UI talks to.
//! The Assistant (LLM runtime, later phase) consumes the same functions.
//!
//! Workspace Domain v0.2 split the Project / Workstream / Session-command areas
//! into sibling modules (`project`, `workstream`, `session_workspace`,
//! `workspace`) so their boundaries stay explicit.
//! This file owns the shared state and helpers the submodules borrow.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::domain::*;
use crate::error::{other, Result};
use crate::storage::{new_id, now, Db};

pub mod ingestion;
pub mod project;
pub mod session_lifecycle;
pub mod session_workspace;
pub mod workspace;
pub mod workstream;

pub use workstream::workstream_cards;

pub struct AppState {
    /// The store owns its own concurrency (one writer + a WAL reader), so the
    /// UI's reads never queue behind a background reconcile's writes. Arc'd
    /// because `Db` is deliberately not `Clone`: async commands move a handle
    /// onto the blocking worker (`run_on_blocking_worker`), which needs 'static.
    pub db: std::sync::Arc<Db>,
    /// The one place background ingestion is queued. Never calls AI.
    pub ingestion: ingestion::IngestionCoordinator,
    /// Guards against concurrent workspace reconciles (global AND targeted —
    /// both mutate the same registry, so one flag serializes them).
    pub workspace_refresh_in_progress: std::sync::atomic::AtomicBool,
    /// In-memory store for prepared launches awaiting user confirmation.
    pub prepared_launches:
        std::sync::Mutex<std::collections::HashMap<String, crate::launcher::PreparedLaunch>>,
}

pub(crate) fn with_db<T>(state: &AppState, f: impl FnOnce(&Db) -> Result<T>) -> Result<T> {
    f(&state.db)
}

/// The launcher, pointed at NoEnding Home's `runtime/`.
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

/// The Workspace Assistant's fixed headless exec cwd (`<Home>/runtime/assistant`).
/// The temp-dir fallback mirrors [`launcher_for`]: a chat turn must not fail
/// because Home state was never registered.
fn assistant_exec_cwd(app: &AppHandle) -> std::path::PathBuf {
    noending_home(app)
        .map(|h| h.assistant_exec_dir())
        .unwrap_or_else(|| std::env::temp_dir().join("noending-assistant"))
}

/// tier 3 — the non-database fact a launch needs.
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
    // 新建会话由终端 CLI 承载；桌面端 Agent 没有可启动的命令行，不能当默认。
    if !crate::adapters::adapter_for(agent).has_terminal_cli() {
        return Err(other("该 Agent 只有桌面端，不能用于新建会话"));
    }
    with_db(&state, |db| {
        db.set_setting(DEFAULT_AGENT_KEY, agent.as_str())
    })
}

// ---------------- Explicit Context update ----------------

/// Read the Session's summary + pending state. A pure read: no ingestion, no AI.
#[tauri::command]
pub fn get_session_context(
    state: State<AppState>,
    session_id: String,
) -> Result<crate::context::SessionContextView> {
    with_db(&state, |db| {
        crate::context::session_context_view(db, &session_id)
    })
}

/// Read the Workstream's Context + revision + pending state. Pure read.
#[tauri::command]
pub fn get_workstream_context_state(
    state: State<AppState>,
    workstream_id: String,
) -> Result<crate::context::WorkstreamContextView> {
    with_db(&state, |db| {
        crate::context::workstream_context_view(db, &workstream_id)
    })
}

/// ONE user action → AT MOST ONE model call → one new Session Context.
/// Returns `updated` / `partial` / `no_change`; a stale snapshot is an `Err`
/// carrying the `stale_snapshot` reason so the UI can ask for a re-click.
///
/// Async on purpose: the model call blocks for up to the configured CLI
/// timeout, so the body goes through [`run_on_blocking_worker`] and the UI
/// thread stays responsive (same seam as `refresh_agent_runtime_options`).
#[tauri::command]
pub async fn update_session_context(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
) -> Result<crate::context::SessionUpdateOutcome> {
    let db = state.inner().db.clone();
    let home = noending_home(&app);
    run_on_blocking_worker(move || crate::context::update_session(&db, &session_id, home.as_ref()))
        .await
        .and_then(|outcome| outcome)
}

/// ONE user action → AT MOST ONE model call → every affected Session Context AND
/// the Workstream Context mutations, committed atomically.
///
/// Async on purpose: same seam as [`update_session_context`] — the model call
/// must never occupy the UI thread.
#[tauri::command]
pub async fn update_workstream_context(
    app: AppHandle,
    state: State<'_, AppState>,
    workstream_id: String,
) -> Result<crate::context::WorkstreamUpdateOutcome> {
    let db = state.inner().db.clone();
    let home = noending_home(&app);
    run_on_blocking_worker(move || {
        crate::context::update_workstream(db.as_ref(), &workstream_id, home.as_ref())
    })
    .await
    .and_then(|outcome| outcome)
}

/// Open the active Home's Context extraction log folder in the system file
/// manager. The directory may not exist until the first update, so create it.
#[tauri::command]
pub fn open_context_extraction_logs(app: AppHandle) -> Result<()> {
    let home = noending_home(&app).ok_or_else(|| other("NoEnding Home 尚未初始化"))?;
    let path = crate::context::diagnostics::log_dir(&home);
    std::fs::create_dir_all(&path)?;
    crate::platform::paths::open_directory(&path)
}

/// Open a remote repository URL in the default browser. The platform layer
/// only passes http/https through; the frontend normalizes scp/ssh spellings
/// to https before it gets here.
#[tauri::command]
pub fn open_remote_url(url: String) -> Result<()> {
    crate::platform::paths::open_url(&url)
}

/// Open a directory in the system file manager, or locate a file if the path is a file.
#[tauri::command]
pub fn open_path(path: String) -> Result<()> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(crate::error::other("路径不能为空"));
    }
    let expanded = crate::platform::paths::expand_tilde(trimmed);
    let p = std::path::Path::new(&expanded);
    if !p.exists() {
        return Err(crate::error::other(format!("路径不存在：{}", p.display())));
    }
    if p.is_file() {
        crate::platform::paths::reveal_file(p)
    } else {
        crate::platform::paths::open_directory(p)
    }
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
            "user",
        )?;
        // the Project this activity belongs to is its position-0
        // path's Project. The current projection is read from the primary
        // path: the derived cache can name a Project deleted or
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
        db.apply_status_change(&item_id, &status, "user", "用户手动修改状态", &[])
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
        db.apply_status_change(&item_id, "deleted", "user", "用户删除", &[])?;
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
    /// Sessions that own this Workstream. At most one Workstream
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
        // the Workstream context view is a default projection: trashed
        // Sessions are not shown here (the query filters them out).
        let sessions = db.sessions_for_workstream(&workstream_id)?;
        let conflicts = db.conflicts_for_workstream(&workstream_id, false)?;
        let conflict_cases = db.list_conflict_review_cases(&workstream_id, false)?;
        let relations = db.item_relations_for_workstream(&workstream_id)?;
        let recent_changes = db.list_workstream_context_changes(&workstream_id, 20)?;
        // the name shown beside a Workstream is its position-0 path's
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

// ---------------- Ingestion ----------------
//
// Background ingestion is queued through `commands::ingestion` (reconcile_all /
// reconcile_source / reingest_source / app_foreground / get_ingestion_status).
// Ingestion is automatic and never calls AI; Context is updated only by the
// explicit commands above.

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
        // Ownership follows the source; `apply_match` owns the rule.
        crate::launcher::apply_match(db, &intent.id, &session, &workspace)
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
    let result = with_db(&state, |db| {
        launcher.new_session_in(
            db,
            agent,
            owner_workstream_id.as_deref(),
            cwd.as_deref(),
            &workspace,
        )
    })?;
    ingestion::enqueue(&app, ingestion::IngestScope::ReconcileAll);
    Ok(result)
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

/// The agent-icon continue button: open the session in its desktop app, no
/// preview. The desktop route is the format's own fact (`desktop_resume_route`)
/// — formats without one refuse here; the button is disabled for them and this
/// is the backstop. Trash is the hard gate; source availability does not
/// matter (the desktop app reads its own store, not our transcript).
#[tauri::command]
pub fn continue_session_desktop(
    app: AppHandle,
    state: State<AppState>,
    session_id: String,
) -> Result<crate::adapters::DesktopResume> {
    let route = with_db(&state, |db| {
        let session = db
            .get_session(&session_id)?
            .ok_or_else(|| other("Session 不存在"))?;
        if session.is_trashed() {
            return Err(other("会话已在回收站，无法继续；请先恢复会话"));
        }
        Ok(crate::adapters::adapter_for(session.agent).desktop_resume_route(&session))
    })?;
    match route {
        crate::adapters::ResumeRoute::Desktop(open) => {
            crate::platform::launcher::open_uri(&open.uri)?;
            ingestion::enqueue(&app, ingestion::IngestScope::RefreshSession(session_id));
            Ok(open)
        }
        crate::adapters::ResumeRoute::Terminal => Err(other("该会话格式没有桌面端打开方式")),
        crate::adapters::ResumeRoute::Refused(reason) => Err(other(reason)),
    }
}

#[tauri::command]
pub fn launch_prepared(
    app: AppHandle,
    state: State<AppState>,
    terminal: State<'_, crate::terminal::TerminalRegistry>,
    prepared_id: String,
) -> Result<crate::launcher::LaunchResult> {
    let prepared = consume_prepared_launch(&state.prepared_launches, &prepared_id)?;

    let launcher = launcher_for(&app);
    let workspace = launch_workspace(&app);
    let embedded = crate::launcher::EmbeddedSpawn {
        registry: &terminal,
        spawn: crate::terminal::spawn_embedded,
    };
    let result = with_db(&state, |db| {
        launcher.launch_prepared_in(db, &prepared, &workspace, Some(&embedded))
    })?;
    let scope = if prepared.mode == "resume" {
        prepared
            .session_id
            .map(ingestion::IngestScope::RefreshSession)
            .unwrap_or(ingestion::IngestScope::ReconcileAll)
    } else {
        ingestion::IngestScope::ReconcileAll
    };
    ingestion::enqueue(&app, scope);
    Ok(result)
}

/// The terminal subpage's direct entry: prepare + launch an embedded resume
/// in ONE step, no preview. The terminal button IS the explicit intent, so
/// the embedded flag is forced here — it is the only embedded entry; the
/// stored open-method preference governs only the Resume modal's
/// terminal-vs-desktop choice. Every prepare-side gate (trash, missing
/// source) still applies, and a live embedded terminal for the same session
/// is refused.
#[tauri::command]
pub fn launch_embedded_resume(
    app: AppHandle,
    state: State<AppState>,
    terminal: State<'_, crate::terminal::TerminalRegistry>,
    session_id: String,
) -> Result<crate::launcher::LaunchResult> {
    let launcher = launcher_for(&app);
    let workspace = launch_workspace(&app);
    let mut prepared = with_db(&state, |db| {
        launcher.prepare_resume_in(db, &session_id, &workspace)
    })?;
    if terminal.has_live(&session_id) {
        return Err(other(
            "该会话已有运行中的内嵌终端；等它退出后可以再次启动，或改用外部终端",
        ));
    }
    if prepared.desktop_open.is_some() {
        // The stored preference resolved a desktop route (codex with the
        // desktop choice); the terminal button overrides it — the terminal
        // route is the CLI path, which is what embedding runs.
        prepared.desktop_open = None;
    }
    prepared.embedded = true;

    let embedded = crate::launcher::EmbeddedSpawn {
        registry: &terminal,
        spawn: crate::terminal::spawn_embedded,
    };
    let result = with_db(&state, |db| {
        launcher.launch_prepared_in(db, &prepared, &workspace, Some(&embedded))
    })?;
    ingestion::enqueue(&app, ingestion::IngestScope::RefreshSession(session_id));
    Ok(result)
}

/// The New-session page's direct entry: prepare + launch an embedded NEW
/// session in ONE step. The spawned terminal starts UNBOUND — the session
/// does not exist until the TUI writes its file and ingestion discovers it,
/// at which point native ID or unique first-message evidence binds the terminal
/// and the board's pseudo-row becomes a real session. A reconcile is enqueued
/// right away so discovery happens as soon as the file lands.
#[tauri::command]
pub fn launch_embedded_new(
    app: AppHandle,
    state: State<AppState>,
    terminal: State<'_, crate::terminal::TerminalRegistry>,
    agent: String,
    owner_workstream_id: Option<String>,
    cwd: Option<String>,
    initial_message: Option<String>,
) -> Result<crate::launcher::LaunchResult> {
    let agent = agent_of(&agent)?;
    let launcher = launcher_for(&app);
    let workspace = launch_workspace(&app);
    let mut prepared = with_db(&state, |db| {
        launcher.prepare_new_in(
            db,
            agent,
            owner_workstream_id.as_deref(),
            cwd.as_deref(),
            &workspace,
        )
    })?;
    prepared.embedded = true;
    prepared.initial_message = initial_message.filter(|message| !message.trim().is_empty());

    let embedded = crate::launcher::EmbeddedSpawn {
        registry: &terminal,
        spawn: crate::terminal::spawn_embedded,
    };
    let result = with_db(&state, |db| {
        launcher.launch_prepared_in(db, &prepared, &workspace, Some(&embedded))
    })?;
    ingestion::enqueue(&app, ingestion::IngestScope::ReconcileAll);
    Ok(result)
}

/// Live embedded terminals, newest first, each bound session's display title
/// joined in (a DB lookup the registry itself never does). Consumers refresh
/// on `terminals-changed` — never on a timer.
#[tauri::command]
pub fn terminal_list(
    state: State<AppState>,
    terminal: State<'_, crate::terminal::TerminalRegistry>,
) -> Result<Vec<crate::terminal::TerminalSummary>> {
    let mut summaries = terminal.list_live();
    if summaries.iter().any(|t| t.session_id.is_some()) {
        with_db(&state, |db| {
            for t in &mut summaries {
                if let Some(session_id) = &t.session_id {
                    if let Ok(Some(session)) = db.get_session(session_id) {
                        t.session_title = session.title;
                    }
                }
            }
            Ok(())
        })?;
    }
    Ok(summaries)
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

// ---------------- Clipboard paste → terminal ----------------

/// Encode raw RGBA bytes as a PNG. The clipboard hands back uncompressed
/// pixels; a TUI can't take bitmap bytes, but a file path is just text.
pub fn encode_paste_png(width: usize, height: usize, rgba: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut encoder = encoder
        .write_header()
        .map_err(|e| other(format!("PNG 编码失败: {e}")))?;
    encoder
        .write_image_data(rgba)
        .map_err(|e| other(format!("PNG 写入失败: {e}")))?;
    encoder
        .finish()
        .map_err(|e| other(format!("PNG 收尾失败: {e}")))?;
    Ok(out)
}

/// Persist encoded PNG bytes under `<runtime>/paste/` and return the absolute
/// path. Old paste files are pruned like the launcher scripts (diagnostics of
/// one paste, not an archive).
pub fn write_paste_image(runtime_dir: &std::path::Path, png_bytes: &[u8]) -> Result<String> {
    let dir = runtime_dir.join("paste");
    std::fs::create_dir_all(&dir)?;
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(24 * 3600);
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.filter_map(|e| e.ok()) {
            if !e.file_name().to_string_lossy().starts_with("clipboard-") {
                continue;
            }
            if let Ok(m) = e.metadata() {
                if let Ok(modified) = m.modified() {
                    if modified < cutoff {
                        let _ = std::fs::remove_file(e.path());
                    }
                }
            }
        }
    }
    // Millisecond timestamps collide when two pastes land in the same tick —
    // a uuid suffix keeps every paste its own file.
    let path = dir.join(format!(
        "clipboard-{}-{}.png",
        chrono::Utc::now().format("%Y%m%d-%H%M%S%3f"),
        &uuid::Uuid::new_v4().simple().to_string()[..6]
    ));
    std::fs::write(&path, png_bytes)?;
    Ok(path.to_string_lossy().to_string())
}

/// What a terminal paste of the SYSTEM clipboard resolves to, read natively
/// (arboard) so the webview's paste-event quirks never reach the PTY:
/// non-empty text wins; otherwise an image is saved and its path returned.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ClipboardPaste {
    pub text: Option<String>,
    pub image_path: Option<String>,
}

pub fn read_clipboard_paste(runtime_dir: &std::path::Path) -> Result<ClipboardPaste> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| other(format!("读取剪贴板失败: {e}")))?;
    if let Ok(text) = clipboard.get_text() {
        if !text.is_empty() {
            return Ok(ClipboardPaste {
                text: Some(text),
                image_path: None,
            });
        }
    }
    if let Ok(image) = clipboard.get_image() {
        let png = encode_paste_png(image.width, image.height, &image.bytes.into_owned())?;
        let path = write_paste_image(runtime_dir, &png)?;
        return Ok(ClipboardPaste {
            text: None,
            image_path: Some(path),
        });
    }
    Ok(ClipboardPaste {
        text: None,
        image_path: None,
    })
}

#[tauri::command]
pub fn read_clipboard_for_terminal(app: AppHandle) -> Result<crate::commands::ClipboardPaste> {
    let runtime_dir = launcher_for(&app).runtime_dir;
    read_clipboard_paste(&runtime_dir)
}

// ---------------- Embedded terminal (terminal subpage) ----------------
//
// The PTYs themselves are created by `launch_prepared`'s embedded spawn step
// (`crate::terminal::spawn_embedded`); these four commands are the attach /
// detach protocol the frontend terminal subpage speaks. The registry is the
// source of truth — the subpage is a view, not a process owner.

/// The session page's terminal entry: jump to the session's bound terminal
/// (prespecified-id discovery or the ingestion worker's verified match) or
/// report none — the caller launches a fresh embedded Resume terminal. No
/// matching happens here; matching belongs to ingestion.
#[tauri::command]
pub fn terminal_for_session(
    terminal: State<'_, crate::terminal::TerminalRegistry>,
    session_id: String,
) -> Result<Option<crate::terminal::TerminalSummary>> {
    Ok(terminal.for_session(&session_id))
}

/// The terminal view's refresh button: one targeted sync, then the
/// ingestion worker's post-pass bind step re-links whatever the pass
/// surfaced. A bound terminal refreshes its session (RefreshSession, the
/// session page's sync semantics); an unbound one walks its agent's own
/// ingest sources with no throttle — the click IS the event — and the
/// verified match binds when the pass finds the session. The terminal view
/// needs no further wiring: terminal-bound / terminals-changed carry the
/// result to it and the sidebar.
#[tauri::command]
pub fn terminal_refresh(
    app: AppHandle,
    state: State<AppState>,
    terminal: State<'_, crate::terminal::TerminalRegistry>,
    terminal_id: String,
) -> Result<()> {
    let summary = terminal.attach(&terminal_id)?.summary;
    if let Some(session_id) = &summary.session_id {
        ingestion::enqueue(
            &app,
            ingestion::IngestScope::RefreshSession(session_id.clone()),
        );
        return Ok(());
    }
    let sources = with_db(&state, |db| {
        Ok(db
            .list_ingest_sources()?
            .into_iter()
            .filter(|s| s.enabled && s.agent == summary.agent)
            .map(|s| s.id)
            .collect::<Vec<_>>())
    })?;
    if sources.is_empty() {
        return Err(other("该 Agent 没有启用的会话来源，无法同步"));
    }
    for id in sources {
        ingestion::enqueue(&app, ingestion::IngestScope::ReconcileSource(id));
    }
    Ok(())
}

/// The sidebar 运行中 item's explicit close: kill the child and drop the
/// record outright. The frontend navigates away when the closed terminal
/// was on screen (it already holds the summary it clicked).
#[tauri::command]
pub fn terminal_close(
    terminal: State<'_, crate::terminal::TerminalRegistry>,
    terminal_id: String,
) -> Result<()> {
    terminal
        .close(&terminal_id)
        .ok_or_else(|| other("终端不存在或已关闭"))?;
    Ok(())
}

#[tauri::command]
pub fn terminal_attach(
    state: State<'_, crate::terminal::TerminalRegistry>,
    terminal_id: String,
) -> Result<crate::terminal::TerminalSnapshot> {
    state.attach(&terminal_id)
}

#[tauri::command]
pub fn terminal_input(
    state: State<'_, crate::terminal::TerminalRegistry>,
    terminal_id: String,
    data: String,
) -> Result<()> {
    state.write(&terminal_id, &data)
}

#[tauri::command]
pub fn terminal_resize(
    state: State<'_, crate::terminal::TerminalRegistry>,
    terminal_id: String,
    cols: u16,
    rows: u16,
) -> Result<()> {
    state.resize(&terminal_id, cols, rows)
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
pub fn set_all_ingest_sources_enabled(state: State<AppState>, enabled: bool) -> Result<usize> {
    with_db(&state, |db| db.set_all_ingest_sources_enabled(enabled))
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

/// Presence-aware surface label for Settings: what this Agent's Continue can
/// target on THIS machine. A desktop surface counts only while the app is
/// installed — an uninstalled app must not be advertised.
pub fn surface_of(agent: Agent) -> &'static str {
    let adapter = crate::adapters::adapter_for(agent);
    let desktop = adapter
        .desktop_app_name()
        .map(crate::platform::paths::app_bundle_present)
        .unwrap_or(false);
    match (adapter.has_terminal_cli(), desktop) {
        (true, true) => "both",
        (true, false) => "tui_cli",
        (false, true) => "desktop",
        (false, false) => "none",
    }
}

#[tauri::command]
pub fn get_agent_status(state: State<AppState>) -> Result<serde_json::Value> {
    let mut out = serde_json::Map::new();
    for agent in Agent::all() {
        let install = with_db(&state, |db| db.get_installation(*agent))?;
        let surface = surface_of(*agent);
        // 设置页按启动面分组：能力是适配器的静态事实，在场是本机状态——
        // 两者分开给，未安装的桌面应用也要列出来（标「未安装」）。
        let adapter = crate::adapters::adapter_for(*agent);
        let desktop_app = adapter.desktop_app_name();
        out.insert(
            agent.as_str().to_string(),
            serde_json::json!({
                "name": agent.display_name(),
                "detected": install.is_some(),
                "executable": install.as_ref().map(|i| i.executable_path.clone()),
                "version": install.as_ref().and_then(|i| i.version.clone()),
                "surface": surface,
                "terminal_cli": adapter.has_terminal_cli(),
                "desktop_app": desktop_app,
                "desktop_app_present": desktop_app
                    .map(crate::platform::paths::app_bundle_present)
                    .unwrap_or(false),
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
    /// 桌面端 / TUI·CLI / 两者：本机在场感知的 surface 标签。
    pub surface: &'static str,
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
        surface: surface_of(agent),
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

/// The blocking-worker seam behind `refresh_agent_runtime_options`:
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
/// `spawn_blocking` and the UI thread stays responsive. Explicit
/// user action only — opening Settings never reaches this command.
#[tauri::command]
pub async fn refresh_agent_runtime_options(
    agent: String,
) -> Result<crate::agent_runtime::AgentRuntimeDiscovery> {
    let agent = agent_of(&agent)?;
    discover_runtime_options_async(agent).await
}

// ---------------- Assistant ----------------

/// Async on purpose: the assistant's model call blocks for up to the CLI
/// timeout, so the body goes through [`run_on_blocking_worker`] and the UI
/// thread stays responsive (same seam as [`update_session_context`]).
#[tauri::command]
pub async fn assistant_send(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: Option<String>,
    text: String,
) -> Result<crate::assistant::AssistantReply> {
    let db = state.inner().db.clone();
    let exec_cwd = assistant_exec_cwd(&app);
    let reply = run_on_blocking_worker(move || {
        crate::assistant::AssistantService::chat(&db, session_id.as_deref(), &text, &exec_cwd)
    })
    .await
    .and_then(|reply| reply)?;
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
/// effort 一律沿用 Agent 默认值。
#[tauri::command]
pub fn assistant_config_set(state: State<AppState>, agent: String) -> Result<()> {
    with_db(&state, |db| {
        if agent != "none" {
            let parsed = agent_of(&agent)?;
            // 助手经由 Agent CLI 执行；桌面端 Agent 没有命令行可跑。
            if !crate::adapters::adapter_for(parsed).has_terminal_cli() {
                return Err(other("该 Agent 只有桌面端，助手无法调用"));
            }
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

/// The async seam test: the refresh command's blocking CLI probe must run on
/// a blocking worker, never on the calling thread, and the seam must deliver
/// a usable discovery for an agent whose catalog is static (no CLI spawn at
/// all).
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

    #[test]
    fn open_path_rejects_empty_and_nonexistent_paths() {
        let err = open_path("".to_string()).unwrap_err();
        assert!(err.to_string().contains("路径不能为空"));

        let err2 = open_path("   ".to_string()).unwrap_err();
        assert!(err2.to_string().contains("路径不能为空"));

        let err3 =
            open_path("/path/that/definitely/does/not/exist/noending_test_12345".to_string())
                .unwrap_err();
        assert!(err3.to_string().contains("路径不存在"));
    }

    /// Only the rejection side is tested here: a passing URL would really
    /// launch the user's browser. The pass-through is one `run_opener` call.
    #[test]
    fn open_remote_url_only_passes_http_schemes() {
        for bad in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ssh://git@example.com/repo.git",
            "   ",
            "",
        ] {
            let err = open_remote_url(bad.to_string()).unwrap_err();
            assert!(err.to_string().contains("http"), "{bad}: {err}");
        }
    }
}

#[cfg(test)]
mod paste_image_tests {
    use super::*;

    #[test]
    fn encode_paste_png_produces_a_valid_png_from_raw_rgba() {
        let rgba = vec![
            255u8, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let png = encode_paste_png(2, 2, &rgba).unwrap();
        assert!(png.starts_with(&[0x89u8, 0x50, 0x4e, 0x47]));
        let decoder = png::Decoder::new(std::io::Cursor::new(&png));
        let mut reader = decoder.read_info().unwrap();
        let info = reader.info().clone();
        assert_eq!((info.width, info.height), (2, 2));
        let mut buf = vec![0u8; reader.output_buffer_size().unwrap_or(0)];
        reader.next_frame(&mut buf).unwrap();
        assert_eq!(&buf[..4], &rgba[..4]);
    }

    #[test]
    fn paste_image_round_trips_through_runtime_dir() {
        let dir = std::env::temp_dir().join(format!("noending-paste-{}", uuid::Uuid::new_v4()));
        let rgba = vec![1u8, 2, 3, 4];
        let png = encode_paste_png(1, 1, &rgba).unwrap();

        let path = write_paste_image(&dir, &png).unwrap();
        assert!(path.starts_with(dir.join("paste").to_string_lossy().as_ref()));
        assert!(path.ends_with(".png"));
        assert_eq!(std::fs::read(&path).unwrap(), png);

        // The stored file is fresh, so pruning right after must keep it.
        write_paste_image(&dir, &png).unwrap();
        let kept = std::fs::read_dir(dir.join("paste")).unwrap().count();
        assert_eq!(kept, 2, "both paste files survive the 24h prune");
    }
}
