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
pub mod settings;
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

            // refresh + cache agent CLI detections (ExecutableResolver).
            // Snapshot semantics: a failed resolve REMOVES the cached row so
            // an uninstalled CLI is no longer reported as detected and can
            // no longer be auto-selected as default agent.
            {
                for agent in domain::Agent::all() {
                    let resolved = platform::exec_resolver::resolve(agent).ok();
                    if resolved.is_none() {
                        eprintln!(
                            "[noending] {} CLI not found — adapter will be unavailable",
                            agent.display_name()
                        );
                    }
                    let _ = commands::record_installation_probe(&db, agent, resolved);
                }
            }

            // AppState MUST be managed before any background worker starts:
            // the reconcile thread resolves handle.state::<AppState>(), which
            // panics when the state has not been managed yet.
            app.manage(commands::AppState {
                db: Mutex::new(db),
                sync_in_progress: std::sync::atomic::AtomicBool::new(false),
            });

            // make pre-existing events searchable (idempotent)
            {
                let state: tauri::State<commands::AppState> = app.state();
                let guard = state.db.lock().expect("db lock");
                if let Err(e) = guard.backfill_search_index() {
                    eprintln!("[noending] search backfill failed: {}", e);
                }
            }

            // Application Reconcile: ingest what happened while we were away.
            // Runs on a worker thread with per-session locking so the UI
            // stays responsive; UI is notified on completion.
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    use std::sync::atomic::Ordering;
                    let state: tauri::State<commands::AppState> = handle.state();
                    if state.sync_in_progress.swap(true, Ordering::SeqCst) {
                        return; // a user-triggered sync is already running
                    }
                    let result = (|| -> error::Result<(usize, i64)> {
                        let engine = {
                            let guard = crate::sync::lock_db(&state.db)?;
                            sync::SyncEngine::from_settings(&guard)
                        };
                        ingestion::reconcile_with_engine(&state.db, &engine, &|s| {
                            eprintln!(
                                "[reconcile] processing: {}",
                                s.title.as_deref().unwrap_or("(untitled)")
                            );
                        })
                    })();
                    state.sync_in_progress.store(false, Ordering::SeqCst);
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
            commands::list_workstream_cards,
            commands::get_default_agent,
            commands::set_default_agent,
            commands::get_context_delivery_level,
            commands::set_context_delivery_level,
            commands::archive_workstream,
            commands::merge_workstreams,
            commands::add_context_item,
            commands::edit_context_item,
            commands::set_item_status,
            commands::get_item_history,
            commands::delete_context_item,
            commands::get_workstream_context,
            commands::list_conflicts,
            commands::resolve_conflict,
            commands::list_sessions,
            commands::get_session_detail,
            commands::assign_session_project,
            commands::suggest_session_project,
            commands::bind_session_workstream,
            commands::unbind_session_workstream,
            commands::replace_session_bindings,
            commands::list_session_bindings,
            commands::get_app_info,
            commands::sync_all,
            commands::sync_source,
            commands::reingest_source,
            commands::sync_session,
            commands::list_sync_runs,
            commands::list_launch_intents,
            commands::list_ingest_sources,
            commands::add_ingest_source,
            commands::set_ingest_source_enabled,
            commands::remove_ingest_source,
            commands::resolve_launch_intent,
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
