//! Tauri command surface — the Application Domain API the UI talks to.
//! The Assistant (LLM runtime, later phase) consumes the same functions.
//!
//! The Project / Workstream / Session-command areas are sibling modules
//! (`project`, `workstream`, `session_workspace`, `workspace`) so their
//! boundaries stay explicit; this file owns the shared state and helpers the
//! submodules borrow.

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::domain::*;
use crate::error::{other, Result};
use crate::storage::Db;

pub mod ingestion;
pub mod plugins;
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

/// ONE user action → AT MOST ONE Agent invocation → one new Session Context.
/// Returns `updated`, `skipped` or `no_change`; a stale snapshot is an error.
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

/// Rebuild the summary from all current messages, ignoring the existing summary.
#[tauri::command]
pub async fn regenerate_session_context(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
) -> Result<crate::context::SessionUpdateOutcome> {
    let db = state.inner().db.clone();
    let home = noending_home(&app);
    run_on_blocking_worker(move || {
        crate::context::regenerate_session(&db, &session_id, home.as_ref())
    })
    .await
    .and_then(|outcome| outcome)
}

/// ONE user action → AT MOST ONE Agent invocation → every affected Session Context AND
/// the task summary, committed atomically.
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

/// Regenerate the task summary without supplying its current summary.
#[tauri::command]
pub async fn regenerate_workstream_context(
    app: AppHandle,
    state: State<'_, AppState>,
    workstream_id: String,
) -> Result<crate::context::WorkstreamUpdateOutcome> {
    let db = state.db.clone();
    let home = noending_home(&app);
    run_on_blocking_worker(move || {
        crate::context::regenerate_workstream(&db, &workstream_id, home.as_ref())
    })
    .await?
}

/// Project summary reads never ingest or invoke an Agent.
#[tauri::command]
pub fn get_project_context_state(
    state: State<AppState>,
    project_id: String,
) -> Result<crate::context::ProjectContextView> {
    with_db(&state, |db| {
        crate::context::project_context_view(db, &project_id)
    })
}
#[tauri::command]
pub async fn update_project_context(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
) -> Result<crate::context::ProjectUpdateOutcome> {
    let db = state.db.clone();
    let home = noending_home(&app);
    run_on_blocking_worker(move || crate::context::update_project(&db, &project_id, home.as_ref()))
        .await?
}
#[tauri::command]
pub async fn regenerate_project_context(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
) -> Result<crate::context::ProjectUpdateOutcome> {
    let db = state.db.clone();
    let home = noending_home(&app);
    run_on_blocking_worker(move || {
        crate::context::regenerate_project(&db, &project_id, home.as_ref())
    })
    .await?
}

#[tauri::command]
pub fn list_preferences(
    state: State<AppState>,
    project_id: Option<String>,
) -> Result<Vec<crate::storage::preferences::Preference>> {
    with_db(&state, |db| {
        crate::storage::preferences::list_items(db, project_id.as_deref())
    })
}
#[tauri::command]
pub fn save_preference(
    state: State<AppState>,
    id: Option<String>,
    project_id: Option<String>,
    content: String,
    locked: bool,
) -> Result<crate::storage::preferences::Preference> {
    with_db(&state, |db| {
        crate::storage::preferences::save_item(
            db,
            id.as_deref(),
            project_id.as_deref(),
            &content,
            locked,
        )
    })
}
#[tauri::command]
pub fn delete_preference(state: State<AppState>, id: String) -> Result<()> {
    with_db(&state, |db| {
        crate::storage::preferences::delete_item(db, &id)
    })
}

/// Summary templates and previews are pure reads; previews never invoke an Agent.
#[tauri::command]
pub fn get_summary_prompt_settings(
    state: State<AppState>,
) -> Result<crate::context::prompts::PromptSettings> {
    with_db(&state, crate::context::prompts::settings)
}
#[tauri::command]
pub fn set_summary_prompt_template(
    state: State<AppState>,
    kind: crate::context::prompts::PromptKind,
    template: Option<String>,
) -> Result<crate::context::prompts::PromptSettings> {
    with_db(&state, |db| {
        crate::context::prompts::save(db, kind, template.as_deref())
    })
}
#[tauri::command]
pub fn preview_summary_prompt(
    kind: crate::context::prompts::PromptKind,
    templates: crate::context::prompts::PromptTemplates,
) -> Result<String> {
    templates.validate()?;
    Ok(crate::context::summary::preview_prompt(kind, &templates))
}

/// Open the active Home's summary extraction log folder.
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

// ---------------- Task detail ----------------

#[derive(Serialize)]
pub struct WorkstreamContext {
    pub workstream: Workstream,
    pub projects: Vec<crate::workspace::workstream::WorkstreamProject>,
    pub sessions: Vec<SessionListItem>,
}

#[tauri::command]
pub fn get_workstream_context(
    state: State<AppState>,
    workstream_id: String,
) -> Result<WorkstreamContext> {
    with_db(&state, |db| {
        Ok(WorkstreamContext {
            workstream: db
                .get_workstream(&workstream_id)?
                .ok_or_else(|| other("任务不存在"))?,
            projects: crate::workspace::workstream::projects_for_workstream(db, &workstream_id)?,
            sessions: db.workstream_session_list_items(&workstream_id)?,
        })
    })
}

// ---------------- Launcher ----------------

#[tauri::command]
pub fn launch_new_session(
    app: AppHandle,
    state: State<AppState>,
    agent: String,
    initial_workstream_id: Option<String>,
    cwd: Option<String>,
) -> Result<crate::launcher::LaunchResult> {
    let agent = Agent::parse(&agent).ok_or_else(|| other("未知 Agent"))?;
    let launcher = launcher_for(&app);
    let workspace = launch_workspace(&app);
    let result = with_db(&state, |db| {
        launcher.new_session_in(
            db,
            agent,
            initial_workstream_id.as_deref(),
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
    initial_workstream_ids: Vec<String>,
    cwd: Option<String>,
    runtime: Option<crate::agent_runtime::AgentRuntimeOverrides>,
) -> Result<crate::launcher::PreparedLaunch> {
    let agent = Agent::parse(&agent).ok_or_else(|| other("未知 Agent"))?;
    let launcher = launcher_for(&app);
    let workspace = launch_workspace(&app);
    let prepared = with_db(&state, |db| {
        launcher.prepare_new_with_runtime_in(
            db,
            agent,
            &initial_workstream_ids,
            cwd.as_deref(),
            &workspace,
            runtime.as_ref(),
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
        if session.is_archived() {
            return Err(other("会话已在已归档，无法继续；请先恢复会话"));
        }
        if let Some(cwd) = session.cwd.as_deref().filter(|c| !c.trim().is_empty()) {
            if !crate::launcher::is_usable_directory(cwd) {
                return Err(other(format!(
                    "原工作目录不存在或当前不可用（{cwd}），无法继续该会话"
                )));
            }
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
        reconnect: None,
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
        reconnect: None,
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
/// at which point an exact native identity binds the terminal
/// and the board's pseudo-row becomes a real session. Agent lifecycle
/// notifications enqueue ingestion for that Agent; spawning the terminal
/// itself does not enqueue a full reconcile.
#[tauri::command]
pub fn launch_embedded_new(
    app: AppHandle,
    state: State<AppState>,
    terminal: State<'_, crate::terminal::TerminalRegistry>,
    agent: String,
    initial_workstream_ids: Vec<String>,
    cwd: Option<String>,
    initial_message: Option<String>,
    runtime: Option<crate::agent_runtime::AgentRuntimeOverrides>,
) -> Result<crate::launcher::LaunchResult> {
    let agent = agent_of(&agent)?;
    let launcher = launcher_for(&app);
    let workspace = launch_workspace(&app);
    let mut prepared = with_db(&state, |db| {
        launcher.prepare_new_with_runtime_in(
            db,
            agent,
            &initial_workstream_ids,
            cwd.as_deref(),
            &workspace,
            runtime.as_ref(),
        )
    })?;
    prepared.embedded = true;
    prepared.initial_message = initial_message.filter(|message| !message.trim().is_empty());

    let embedded = crate::launcher::EmbeddedSpawn {
        reconnect: None,
        registry: &terminal,
        spawn: crate::terminal::spawn_embedded,
    };
    with_db(&state, |db| {
        launcher.launch_prepared_in(db, &prepared, &workspace, Some(&embedded))
    })
}

/// Retained embedded terminals (including exited), newest first, with each
/// bound session's display title joined in (a DB lookup the registry never
/// does). Consumers refresh on `terminals-changed` — never on a timer.
#[tauri::command]
pub fn terminal_list(
    state: State<AppState>,
    terminal: State<'_, crate::terminal::TerminalRegistry>,
) -> Result<Vec<crate::terminal::TerminalSummary>> {
    let mut summaries = terminal.list();
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

/// Read non-empty clipboard text natively to avoid duplicate WebView pastes.
/// Image clipboard handling belongs to the Agent TUI.
#[tauri::command]
pub fn read_clipboard_for_terminal() -> Result<Option<String>> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| other(format!("读取剪贴板失败: {e}")))?;
    Ok(clipboard.get_text().ok().filter(|text| !text.is_empty()))
}

/// Windows terminal copy should not depend on WebView clipboard permission.
#[tauri::command]
pub fn write_clipboard_for_terminal(text: String) -> Result<()> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| other(format!("写入剪贴板失败: {e}")))?;
    clipboard
        .set_text(text)
        .map_err(|e| other(format!("写入剪贴板失败: {e}")))
}

// ---------------- Embedded terminal (terminal subpage) ----------------
//
// The PTYs themselves are created by `launch_prepared`'s embedded spawn step
// (`crate::terminal::spawn_embedded`); these four commands are the attach /
// detach protocol the frontend terminal subpage speaks. The registry is the
// source of truth — the subpage is a view, not a process owner.

/// Jump to one of the Session's live bound terminals.
/// Native identities may associate multiple terminals with the same Session.
#[tauri::command]
pub fn terminal_for_session(
    terminal: State<'_, crate::terminal::TerminalRegistry>,
    session_id: String,
) -> Result<Option<crate::terminal::TerminalSummary>> {
    Ok(terminal.for_session(&session_id))
}

/// Explicitly restart the Agent for the terminal's current, verified Session.
/// Prepare validates archive/source/runtime state before touching the old PTY.
#[tauri::command]
pub fn terminal_reconnect(
    app: AppHandle,
    state: State<AppState>,
    terminal: State<'_, crate::terminal::TerminalRegistry>,
    terminal_id: String,
    session_id: String,
) -> Result<crate::launcher::LaunchResult> {
    let expected = terminal.attach(&terminal_id)?.summary;
    let bound_session_id = expected
        .session_id
        .as_deref()
        .ok_or_else(|| other("终端尚未绑定会话，暂时无法重新连接"))?;
    if bound_session_id != session_id {
        return Err(other("终端已切换会话，请重试"));
    }
    let launcher = launcher_for(&app);
    let workspace = launch_workspace(&app);
    let result = with_db(&state, |db| {
        let mut prepared = launcher.prepare_resume_in(db, &session_id, &workspace)?;
        prepared.desktop_open = None;
        prepared.embedded = true;
        let embedded = crate::launcher::EmbeddedSpawn {
            registry: &terminal,
            reconnect: Some(&expected),
            spawn: crate::terminal::spawn_embedded,
        };
        launcher.launch_prepared_in(db, &prepared, &workspace, Some(&embedded))
    })?;
    ingestion::enqueue(
        &app,
        ingestion::IngestScope::RefreshSession(session_id.to_owned()),
    );
    Ok(result)
}

/// The sidebar 运行中 item's explicit close: kill the child and drop the
/// record outright. Return its current identity so a switch while the close
/// request was pending cannot navigate back to an obsolete Session.
#[tauri::command]
pub fn terminal_close(
    terminal: State<'_, crate::terminal::TerminalRegistry>,
    terminal_id: String,
) -> Result<crate::terminal::TerminalSummary> {
    terminal
        .close(&terminal_id)
        .ok_or_else(|| other("终端不存在或已关闭"))
}

#[tauri::command]
pub fn terminal_activity_seen(
    terminal: State<'_, crate::terminal::TerminalRegistry>,
    terminal_id: String,
    revision: u64,
) {
    terminal.mark_activity_seen(&terminal_id, revision);
}

#[tauri::command]
pub fn terminal_attach(
    state: State<AppState>,
    terminal: State<'_, crate::terminal::TerminalRegistry>,
    terminal_id: String,
    consumer_id: Option<String>,
) -> Result<crate::terminal::TerminalSnapshot> {
    let mut snapshot = match consumer_id.as_deref() {
        Some(consumer) => terminal.attach_output(&terminal_id, consumer)?,
        None => terminal.attach(&terminal_id)?,
    };
    if let Some(session_id) = &snapshot.summary.session_id {
        if let Some(session) = with_db(&state, |db| db.get_session(session_id))? {
            snapshot.summary.session_title = session.title;
        }
    }
    Ok(snapshot)
}

#[tauri::command]
pub fn terminal_input(
    state: State<'_, crate::terminal::TerminalRegistry>,
    terminal_id: String,
    data: String,
    binary: Option<bool>,
) -> Result<()> {
    if binary.unwrap_or(false) {
        state.write_binary(&terminal_id, &data)
    } else {
        state.write(&terminal_id, &data)
    }
}

#[tauri::command]
pub fn terminal_acknowledge_output(
    state: State<'_, crate::terminal::TerminalRegistry>,
    terminal_id: String,
    offset: u64,
    consumer_id: String,
) {
    state.acknowledge_output(&terminal_id, &consumer_id, offset);
}

#[tauri::command]
pub fn terminal_release_output(
    state: State<'_, crate::terminal::TerminalRegistry>,
    terminal_id: String,
    consumer_id: String,
) {
    state.release_output(&terminal_id, &consumer_id);
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
pub async fn match_objects(
    state: State<'_, AppState>,
    kind: crate::search::objects::ObjectKind,
    query: String,
    candidate_ids: Vec<String>,
) -> Result<Vec<String>> {
    let db = state.db.clone();
    run_on_blocking_worker(move || {
        crate::search::objects::matching_ids(&db, kind, &query, &candidate_ids)
    })
    .await?
}

#[tauri::command]
pub async fn lookup_navigation(
    state: State<'_, AppState>,
    query: String,
) -> Result<crate::search::navigation::NavigationResults> {
    let db = state.db.clone();
    run_on_blocking_worker(move || crate::search::navigation::lookup(&db, &query)).await?
}

/// Presence-aware surface label for Settings: what this Agent's Continue can
/// target on THIS machine. A desktop surface counts only while the app is
/// installed — an uninstalled app must not be advertised.
pub fn surface_of(agent: Agent) -> &'static str {
    let adapter = crate::adapters::adapter_for(agent);
    let desktop = adapter.desktop_app_present();
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
                "launch_enabled": crate::agent_runtime::store::launch_enabled(&state.db, *agent)?,
                "desktop_app": desktop_app,
                "desktop_app_present": adapter.desktop_app_present(),
            }),
        );
    }
    Ok(serde_json::Value::Object(out))
}

#[tauri::command]
pub fn set_agent_launch_enabled(
    state: State<AppState>,
    agent: String,
    enabled: bool,
) -> Result<()> {
    let agent = agent_of(&agent)?;
    crate::agent_runtime::store::set_launch_enabled(&state.db, agent, enabled)
}

#[tauri::command]
pub async fn redetect_agent(
    state: State<'_, AppState>,
    agent: String,
) -> Result<AgentRuntimeSettings> {
    let agent = agent_of(&agent)?;
    let db = state.db.clone();
    run_on_blocking_worker(move || {
        record_installation_probe(
            &db,
            agent,
            crate::platform::exec_resolver::resolve(agent).ok(),
        )?;
        runtime_settings(&db, agent)
    })
    .await?
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
/// Agent default plus custom input. Explicit
/// user action only — opening Settings never reaches this command.
#[tauri::command]
pub async fn refresh_agent_runtime_options(
    agent: String,
) -> Result<crate::agent_runtime::AgentRuntimeDiscovery> {
    let agent = agent_of(&agent)?;
    discover_runtime_options_async(agent).await
}

#[tauri::command]
pub fn get_extraction_settings(
    state: State<AppState>,
) -> Result<crate::sync::extractor::ExtractionSettings> {
    with_db(
        &state,
        crate::sync::extractor::ExtractionSettings::from_settings,
    )
}

#[tauri::command]
pub fn set_extraction_settings(
    state: State<AppState>,
    settings: crate::sync::extractor::ExtractionSettings,
) -> Result<crate::sync::extractor::ExtractionSettings> {
    with_db(&state, |db| settings.save(db))
}

// ---------------- Assistant ----------------

#[tauri::command]
pub async fn assistant_workspace(
    app: AppHandle,
    state: State<'_, AppState>,
    layer: State<'_, std::sync::Arc<crate::workspace::wiring::WorkspaceLayer>>,
) -> Result<crate::assistant::AssistantWorkspace> {
    let home = noending_home(&app).ok_or_else(|| other("NoEnding Home 未初始化"))?;
    let db = state.db.clone();
    let layer = layer.inner().clone();
    run_on_blocking_worker(move || crate::assistant::ensure_workspace(&db, &home, &layer)).await?
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

#[tauri::command]
pub fn promote_preference(
    state: State<AppState>,
    id: String,
) -> Result<crate::storage::preferences::Preference> {
    with_db(&state, |db| {
        crate::storage::preferences::promote_item(db, &id)
    })
}
