//! NoEnding library crate — exposes the domain API to integration tests
//! and hosts the Tauri run() entry.

pub mod adapters;
pub mod assistant;
pub mod commands;
pub mod context;
pub mod domain;
pub mod error;
pub mod ingestion;
pub mod launcher;
pub mod platform;
pub mod search;
pub mod storage;
pub mod sync;

use std::sync::Mutex;

use tauri::{Emitter, Manager};

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let db_path = data_dir.join("noending.db");
            let db = storage::Db::open(&db_path)?;
            eprintln!("[noending] db at {}", db_path.display());

            // refresh + cache agent CLI detections (ExecutableResolver)
            {
                for agent in domain::Agent::all() {
                    if let Ok(install) = platform::exec_resolver::resolve(agent) {
                        let _ = db.save_installation(&install);
                    } else {
                        eprintln!(
                            "[noending] {} CLI not found — adapter will be unavailable",
                            agent.display_name()
                        );
                    }
                }
            }

            // Application Reconcile: ingest what happened while we were away.
            // Runs on a worker thread so startup stays snappy; UI is notified.
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    let state: tauri::State<commands::AppState> = handle.state();
                    let result = {
                        let guard = state.db.lock().expect("db lock");
                        let engine = sync::SyncEngine::from_settings(&guard);
                        ingestion::reconcile_with_engine(&guard, &engine)
                    };
                    match result {
                        Ok((discovered, events)) => {
                            eprintln!(
                                "[reconcile] {} sessions discovered, {} events ingested",
                                discovered, events
                            );
                            let _ = handle.emit(
                                "reconcile-completed",
                                serde_json::json!({ "discovered": discovered, "events": events }),
                            );
                        }
                        Err(e) => eprintln!("[reconcile] failed: {}", e),
                    }
                });
            }

            app.manage(commands::AppState { db: Mutex::new(db) });

            // make pre-existing events searchable (idempotent)
            {
                let state: tauri::State<commands::AppState> = app.state();
                let guard = state.db.lock().expect("db lock");
                if let Err(e) = guard.backfill_search_index() {
                    eprintln!("[noending] search backfill failed: {}", e);
                }
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::create_project,
            commands::update_project,
            commands::list_projects,
            commands::delete_project,
            commands::add_project_resource,
            commands::list_project_resources,
            commands::remove_project_resource,
            commands::create_workstream,
            commands::update_workstream,
            commands::list_workstreams,
            commands::archive_workstream,
            commands::merge_workstreams,
            commands::add_context_item,
            commands::edit_context_item,
            commands::set_item_status,
            commands::get_item_history,
            commands::delete_context_item,
            commands::get_workstream_context,
            commands::list_sessions,
            commands::get_session_detail,
            commands::assign_session_project,
            commands::bind_session_workstream,
            commands::sync_all,
            commands::sync_session,
            commands::list_sync_runs,
            commands::launch_new_session,
            commands::launch_resume_session,
            commands::preview_context_bundle,
            commands::search,
            commands::get_stats,
            commands::get_agent_status,
            commands::assistant_send,
            commands::assistant_messages,
            commands::assistant_config_get,
            commands::assistant_config_set,
            commands::assistant_execute_action,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
