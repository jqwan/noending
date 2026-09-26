//! NoEnding library crate — exposes the domain API to integration tests
//! and hosts the Tauri run() entry.

pub mod adapters;
pub mod agent_runtime;
pub mod assistant;
pub mod commands;
pub mod context;
pub mod domain;
pub mod error;
pub mod ingestion;
pub mod launcher;
pub mod lifecycle;
pub mod platform;
pub mod search;
pub mod storage;
pub mod sync;
pub mod workspace;

use std::sync::Mutex;

use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // NoEnding Home first, always before the database opens:
            // it resolves $NOENDING_HOME → bootstrap.current_home → ~/.noending,
            // applies any pending relocation, and creates data/ runtime/ logs/
            // workspace/. A *migration* failure
            // is not fatal here — it returns the old Home with the reason in
            // `notes` — because "no database found" would look like data loss.
            let startup =
                workspace::home::prepare_home(&workspace::home::StartupInputs::from_environment())?;
            for note in &startup.notes {
                eprintln!("[noending] {note}");
            }
            let home = startup.home.clone();
            eprintln!(
                "[noending] home at {} ({})",
                home.root.display(),
                home.root_str()
            );
            let db_path = home.db_path.clone();
            let db = storage::Db::open(&db_path)?;
            eprintln!("[noending] db at {}", db_path.display());

            // The physical layer behind the one `WorkspaceAttaching` door.
            // Managed as state for the commands, and registered for
            // ingestion, whose discovery resolves Session cwds through it.
            app.manage(home.clone());
            let layer = std::sync::Arc::new(workspace::wiring::WorkspaceLayer::new(&home));
            app.manage(layer.clone());
            let _ = workspace::session::register_workspace_attacher(layer.clone());
            app.manage(workspace::workstream::PathService::new(layer));

            // refresh + cache agent CLI detections (ExecutableResolver).
            // Snapshot semantics: a failed resolve REMOVES the cached row so
            // an uninstalled CLI is no longer reported as detected and can
            // no longer be auto-selected as default agent.
            {
                for agent in domain::Agent::all() {
                    let resolved = platform::exec_resolver::resolve(*agent).ok();
                    if resolved.is_none() {
                        eprintln!(
                            "[noending] {} CLI not found — adapter will be unavailable",
                            agent.display_name()
                        );
                    }
                    let _ = commands::record_installation_probe(&db, *agent, resolved);
                }
            }

            // AppState MUST be managed before any background worker starts:
            // the ingestion worker resolves handle.state::<AppState>(), which
            // panics when the state has not been managed yet.
            app.manage(commands::AppState {
                db,
                ingestion: Default::default(),
                workspace_refresh_in_progress: std::sync::atomic::AtomicBool::new(false),
                prepared_launches: Mutex::new(std::collections::HashMap::new()),
            });

            // make pre-existing messages searchable (idempotent)
            {
                let state: tauri::State<commands::AppState> = app.state();
                if let Err(e) = state.db.backfill_search_index() {
                    eprintln!("[noending] search backfill failed: {}", e);
                }
            }

            // Application Reconcile: at every startup, queue a background
            // ingestion pass (`ReconcileAll`). Ingestion never calls AI; the
            // UI is notified on completion. Runs on the coordinator's worker
            // thread, so the UI stays responsive.
            commands::ingestion::enqueue(
                &app.handle().clone(),
                commands::ingestion::IngestScope::ReconcileAll,
            );

            // Workspace Reconcile: Git detection on paths that so far exist
            // only as strings. It runs after ingestion is queued because a
            // Session's cwd is what tells us a directory is real, and it
            // re-observes per path instead of holding the DB lock across a
            // `git` call.
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    let state: tauri::State<commands::AppState> = handle.state();
                    let layer: tauri::State<std::sync::Arc<workspace::wiring::WorkspaceLayer>> =
                        handle.state();
                    match workspace::project::reconcile_workspace_paths(
                        &state.db,
                        &layer.projection(),
                        usize::MAX,
                    ) {
                        Ok(report) => eprintln!(
                            "[workspace] reconciled {} paths, {} moved, {} discovered, {} failed",
                            report.scanned,
                            report.moved_paths,
                            report.discovered_paths.len(),
                            report.failed.len(),
                        ),
                        Err(e) => eprintln!("[workspace] reconcile skipped: {e}"),
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Projects are derived from WorkspacePaths; the UI gets read + rename.
            commands::project::list_projects,
            commands::project::list_project_cards,
            commands::project::refresh_workspace_projects,
            commands::project::refresh_project_workspace,
            commands::project::get_project_detail,
            commands::project::list_project_workstreams,
            commands::project::rename_project,
            commands::workspace::get_workspace_settings,
            commands::workspace::set_noending_home,
            commands::workstream::create_workstream,
            commands::workstream::probe_workspace_path,
            commands::workstream::list_recent_workspace_paths,
            commands::workstream::update_workstream,
            commands::workstream::list_workstreams,
            commands::workstream::list_workstream_cards,
            commands::workstream::list_workstream_paths,
            commands::workstream::add_workstream_path,
            commands::workstream::remove_workstream_path,
            commands::workstream::reorder_workstream_paths,
            commands::workstream::set_workstream_lifecycle,
            commands::workstream::archive_workstream,
            commands::workstream::restore_workstream,
            commands::workstream::delete_workstream_permanently,
            // `merge_workstreams` left the API: it moved Context items between
            // Workstreams without leaving a Revision.
            commands::get_default_agent,
            commands::set_default_agent,
            // Explicit Context: two read commands, two AI commands.
            commands::get_session_context,
            commands::get_workstream_context_state,
            commands::update_session_context,
            commands::update_workstream_context,
            commands::open_context_extraction_logs,
            commands::add_context_item,
            commands::edit_context_item,
            commands::set_item_status,
            commands::get_item_history,
            commands::delete_context_item,
            commands::get_workstream_context,
            commands::get_context_revision_source,
            commands::get_workstream_review_state,
            commands::get_workstream_review_window,
            commands::mark_workstream_reviewed,
            commands::get_workstream_review_summary,
            commands::list_workstream_review_summaries,
            commands::list_conflicts,
            commands::get_conflict_review_case,
            commands::list_conflict_review_cases,
            commands::resolve_conflict,
            commands::resolve_conflict_with_edit,
            commands::session_workspace::list_sessions,
            commands::session_workspace::get_session_detail,
            commands::session_workspace::set_session_owner_workstream,
            commands::session_workspace::list_ingestion_diagnostics,
            // Session Lifecycle: Trash / Restore and the
            // stateless permanent LOCAL deletion. The UI submits ids only.
            commands::session_lifecycle::trash_session,
            commands::session_lifecycle::restore_session,
            commands::session_lifecycle::get_session_local_delete_preview,
            commands::session_lifecycle::permanently_delete_session,
            commands::ingestion::reconcile_all,
            commands::ingestion::reconcile_source,
            commands::ingestion::reingest_source,
            commands::ingestion::app_foreground,
            commands::ingestion::get_ingestion_status,
            commands::list_launch_intents,
            commands::list_ingest_sources,
            commands::add_ingest_source,
            commands::set_ingest_source_enabled,
            commands::remove_ingest_source,
            commands::resolve_launch_intent,
            commands::launch_new_session,
            commands::launch_resume_session,
            commands::prepare_new_session,
            commands::prepare_resume_session,
            commands::launch_prepared,
            commands::cancel_prepared,
            commands::search,
            commands::get_agent_status,
            commands::get_agent_runtime_settings,
            commands::set_agent_runtime_overrides,
            commands::refresh_agent_runtime_options,
            commands::assistant_send,
            commands::assistant_messages,
            commands::assistant_config_get,
            commands::assistant_config_set,
            commands::assistant_execute_action,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
