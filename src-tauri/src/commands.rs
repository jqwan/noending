//! Tauri command surface — the Application Domain API the UI talks to.
//! The Assistant (LLM runtime, later phase) consumes the same functions.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::domain::*;
use crate::error::{other, Result};
use crate::storage::{now, new_id, Db};

pub struct AppState {
    pub db: std::sync::Mutex<Db>,
}

fn with_db<T>(state: &AppState, f: impl FnOnce(&Db) -> Result<T>) -> Result<T> {
    let guard = state.db.lock().map_err(|_| other("db lock poisoned"))?;
    f(&guard)
}

// ---------------- Projects ----------------

#[tauri::command]
pub fn create_project(state: State<AppState>, name: String, description: String) -> Result<Project> {
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

#[tauri::command]
pub fn delete_project(state: State<AppState>, project_id: String) -> Result<()> {
    with_db(&state, |db| {
        db.delete_project(&project_id)?;
        // keep workstreams orphan-free: move to archived visibility instead of cascade delete
        let wss = db.list_workstreams(Some(&project_id))?;
        for mut w in wss {
            w.visibility = "archived".into();
            w.updated_at = now();
            db.upsert_workstream(&w)?;
        }
        Ok(())
    })
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
pub fn list_project_resources(state: State<AppState>, project_id: String) -> Result<Vec<ProjectResource>> {
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
) -> Result<Workstream> {
    crate::storage::ensure_not_empty("Workstream 标题", &title)?;
    let w = Workstream {
        id: new_id(),
        project_id,
        title: title.trim().to_string(),
        description: description.trim().to_string(),
        lifecycle: "open".into(),
        visibility: "normal".into(),
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
pub fn list_workstreams(state: State<AppState>, project_id: Option<String>) -> Result<Vec<Workstream>> {
    with_db(&state, |db| db.list_workstreams(project_id.as_deref()))
}

#[tauri::command]
pub fn archive_workstream(state: State<AppState>, workstream_id: String) -> Result<()> {
    with_db(&state, |db| {
        let mut w = db
            .get_workstream(&workstream_id)?
            .ok_or_else(|| other("Workstream 不存在"))?;
        w.visibility = if w.visibility == "archived" { "normal" } else { "archived" }.into();
        w.updated_at = now();
        db.upsert_workstream(&w)
    })
}

#[tauri::command]
pub fn merge_workstreams(state: State<AppState>, source_id: String, target_id: String) -> Result<()> {
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
                 (session_id, workstream_id, role, last_seen_revision, last_sync_cursor, created_at, last_used_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![b.session_id, target_id, b.role, b.last_seen_revision, b.last_sync_cursor, b.created_at, b.last_used_at],
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
            "user_edit",
            "user_edit",
            None,
            None,
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
            metadata: serde_json::json!({}),
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

#[tauri::command]
pub fn set_item_status(state: State<AppState>, item_id: String, status: String) -> Result<()> {
    with_db(&state, |db| db.set_item_status(&item_id, &status))
}

#[tauri::command]
pub fn get_item_history(state: State<AppState>, item_id: String) -> Result<Vec<ContextItemRevision>> {
    with_db(&state, |db| db.item_history(&item_id))
}

#[tauri::command]
pub fn delete_context_item(state: State<AppState>, item_id: String) -> Result<()> {
    with_db(&state, |db| {
        db.set_item_status(&item_id, "deleted")?;
        db.unindex("item", &item_id);
        Ok(())
    })
}

#[derive(Serialize)]
pub struct WorkstreamContext {
    pub workstream: Workstream,
    pub core: Vec<crate::context::ContextSection>,
    pub items: Vec<(ContextItem, ContextItemRevision)>,
    pub related_sessions: Vec<Session>,
}

#[tauri::command]
pub fn get_workstream_context(state: State<AppState>, workstream_id: String) -> Result<WorkstreamContext> {
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
        Ok(WorkstreamContext {
            workstream,
            core,
            items,
            related_sessions: sessions,
        })
    })
}

// ---------------- Sessions ----------------

#[derive(Serialize)]
pub struct SessionDetail {
    pub session: Session,
    pub events: Vec<SessionEvent>,
    pub bindings: Vec<(SessionWorkstreamBinding, Option<WorkstreamTitle>)>,
    pub cursor: i64,
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
        db.list_sessions(crate::storage::SessionFilter {
            project_id,
            agent,
        })
    })
}

#[tauri::command]
pub fn get_session_detail(state: State<AppState>, session_id: String) -> Result<SessionDetail> {
    with_db(&state, |db| {
        let session = db.get_session(&session_id)?.ok_or_else(|| other("Session 不存在"))?;
        let events = db.get_events(&session_id, None, 500)?;
        let cursor = db.get_cursor(&session_id)?;
        let bindings = db
            .bindings_for_session(&session_id)?
            .into_iter()
            .map(|b| {
                let title = db
                    .get_workstream(&b.workstream_id)?
                    .map(|w| w.title);
                Ok((b, title))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(SessionDetail {
            session,
            events,
            bindings,
            cursor,
        })
    })
}

#[tauri::command]
pub fn assign_session_project(state: State<AppState>, session_id: String, project_id: Option<String>) -> Result<()> {
    with_db(&state, |db| {
        db.0.execute(
            "UPDATE sessions SET project_id = ?2 WHERE id = ?1",
            rusqlite::params![session_id, project_id],
        )?;
        Ok(())
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
        let launcher = crate::launcher::SessionLauncher {
            app_data_dir: std::env::temp_dir(),
        };
        launcher.record_binding(db, &session_id, &workstream_id, &role)
    })
}

// ---------------- Sync ----------------

#[tauri::command]
pub fn sync_all(app: AppHandle, state: State<AppState>) -> Result<serde_json::Value> {
    let result = with_db(&state, |db| {
        let engine = crate::sync::SyncEngine::from_settings(db);
        crate::ingestion::reconcile_with_engine(db, &engine)
    })?;
    let _ = app.emit("sync-completed", &result);
    Ok(serde_json::json!({
        "discovered": result.0,
        "events": result.1,
    }))
}

#[tauri::command]
pub fn sync_session(state: State<AppState>, session_id: String) -> Result<serde_json::Value> {
    with_db(&state, |db| {
        let session = db.get_session(&session_id)?.ok_or_else(|| other("Session 不存在"))?;
        let engine = crate::sync::SyncEngine::from_settings(db);
        let applied = crate::launcher::sync_one_session_with_engine(db, &engine, &session)?;
        Ok(serde_json::json!({ "applied": applied }))
    })
}

#[tauri::command]
pub fn list_sync_runs(state: State<AppState>, limit: Option<i64>) -> Result<Vec<SyncRun>> {
    with_db(&state, |db| db.list_sync_runs(limit.unwrap_or(50)))
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
    let app_data = app.path().app_data_dir().unwrap_or_else(|_| std::env::temp_dir());
    let launcher = crate::launcher::SessionLauncher { app_data_dir: app_data };
    with_db(&state, |db| launcher.new_session(db, agent, &workstream_ids, cwd.as_deref()))
}

#[tauri::command]
pub fn launch_resume_session(
    app: AppHandle,
    state: State<AppState>,
    session_id: String,
    extra_workstream_ids: Vec<String>,
) -> Result<crate::launcher::LaunchResult> {
    let app_data = app.path().app_data_dir().unwrap_or_else(|_| std::env::temp_dir());
    let launcher = crate::launcher::SessionLauncher { app_data_dir: app_data };
    with_db(&state, |db| launcher.resume_session(db, &session_id, &extra_workstream_ids))
}

#[tauri::command]
pub fn preview_context_bundle(
    state: State<AppState>,
    workstream_ids: Vec<String>,
    mode: Option<String>,
) -> Result<crate::context::SessionContextBundle> {
    with_db(&state, |db| {
        crate::context::build_bundle(db, mode.as_deref().unwrap_or("new"), None, &workstream_ids, 4000)
    })
}

// ---------------- Search / misc ----------------

#[tauri::command]
pub fn search(state: State<AppState>, query: String, limit: Option<i64>) -> Result<Vec<crate::search::SearchHit>> {
    with_db(&state, |db| crate::search::search(db, &query, limit.unwrap_or(30)))
}

#[tauri::command]
pub fn get_stats(state: State<AppState>) -> Result<serde_json::Value> {
    with_db(&state, |db| db.stats())
}

#[tauri::command]
pub fn get_agent_status(state: State<AppState>) -> Result<serde_json::Value> {    let mut out = serde_json::Map::new();
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
    let reply = with_db(&state, |db| crate::assistant::AssistantService::chat(db, session_id.as_deref(), &text))?;
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
pub fn assistant_config_get(state: State<AppState>) -> Result<crate::sync::extractor::AssistantConfig> {
    with_db(&state, |db| Ok(crate::sync::extractor::AssistantConfig::from_settings(db)))
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
            agent, model, provider, effort,
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
    let action: crate::assistant::ActionProposal = serde_json::from_str(&action_json)
        .map_err(|e| other(format!("动作解析失败: {}", e)))?;
    let app_data = app.path().app_data_dir().unwrap_or_else(|_| std::env::temp_dir());
    with_db(&state, |db| crate::assistant::AssistantService::execute_action(db, &action, &app_data))
}
