//! NoEnding library crate — exposes the domain API to integration tests
//! and hosts the Tauri run() entry.

pub mod adapters;
pub mod agent_runtime;
pub mod assistant;
pub mod cli;
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
pub mod terminal;
pub mod workspace;

use std::sync::Mutex;

use tauri::{Emitter, Manager};

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            if let Some(window) = app.get_webview_window("main") {
                platform::webview::prefer_native_refresh_rate(&window);
            }

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
            app.manage(workspace::workstream::PathService::new(layer.clone()));

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
                db: std::sync::Arc::new(db),
                ingestion: Default::default(),
                prepared_launches: Mutex::new(std::collections::HashMap::new()),
            });

            // Embedded terminal registry. The AppHandle rides inside so the
            // PTY reader/wait threads can emit to the webview without ever
            // touching app state from a thread.
            app.manage(terminal::TerminalRegistry::new(Some(app.handle().clone())));

            // CLI requests use the same app-owned DB/services as Tauri commands.
            // A scoped extraction connection never routes through this server.
            {
                let state: tauri::State<commands::AppState> = app.state();
                let handle = app.handle().clone();
                match cli::start(state.db.clone(), home.clone(), layer.clone(), move || {
                    let _ = handle.emit("cli-data-changed", ());
                }) {
                    Ok(server) => { app.manage(server); }
                    Err(error) => eprintln!("[noending] CLI unavailable: {error}"),
                }
            }

            {
                let state: tauri::State<commands::AppState> = app.state();
                if let Err(error) = assistant::ensure_workspace(&state.db, &home, &layer) {
                    eprintln!("[noending] Assistant workspace unavailable: {error}");
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
                        Ok(report) => {
                            let _ = handle.emit("workspace-reconcile-completed", ());
                            eprintln!(
                                "[workspace] reconciled {} paths, {} moved, {} discovered, {} failed",
                                report.scanned,
                                report.moved_paths,
                                report.discovered_paths.len(),
                                report.failed.len(),
                            );
                        }
                        Err(e) => eprintln!("[workspace] reconcile skipped: {e}"),
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Projects derive membership from paths; lifecycle is user managed.
            commands::project::list_projects,
            commands::project::list_project_cards,
            commands::project::get_project_detail,
            commands::project::list_project_workstreams,
            commands::project::rename_project,
            commands::project::add_project_path,
            commands::project::archive_project,
            commands::project::restore_project,
            commands::project::delete_project_permanently,
            commands::workspace::get_workspace_settings,
            commands::workspace::set_noending_home,
            commands::workstream::create_workstream,
            commands::workstream::probe_workspace_path,
            commands::workstream::list_recent_workspace_paths,
            commands::workstream::update_workstream,
            commands::workstream::list_workstreams,
            commands::workstream::list_workstream_cards,
            commands::workstream::set_workstream_projects,
            commands::workstream::archive_workstream,
            commands::workstream::restore_workstream,
            commands::workstream::delete_workstream_permanently,
            commands::get_default_agent,
            commands::set_default_agent,
            // Explicit summary reads and user-triggered extraction.
            commands::get_session_context,
            commands::get_workstream_context_state,
            commands::get_project_context_state,
            commands::update_project_context,
            commands::regenerate_project_context,
            commands::update_session_context,
            commands::regenerate_session_context,
            commands::update_workstream_context,
            commands::regenerate_workstream_context,
            commands::open_context_extraction_logs,
            commands::list_preferences,
            commands::save_preference,
            commands::promote_preference,
            commands::delete_preference,
            commands::get_summary_prompt_settings,
            commands::set_summary_prompt_template,
            commands::preview_summary_prompt,
            commands::open_remote_url,
            commands::open_path,
            commands::get_workstream_context,
            commands::session_workspace::list_sessions,
            commands::session_workspace::get_session_counts_by_agent,
            commands::session_workspace::get_agent_latest_activities,
            commands::session_workspace::get_session_terminal_states,
            commands::session_workspace::get_session_detail,
            commands::session_workspace::get_session_tracking,
            commands::session_workspace::reveal_session_source,
            commands::session_workspace::get_session_messages,
            commands::session_workspace::get_turn_intermediates,
            commands::session_workspace::get_session_user_message_marks,
            commands::session_workspace::set_session_task_tracking,
            // Session Lifecycle: Trash / Restore and the
            // stateless permanent LOCAL deletion. The UI submits ids only.
            commands::session_lifecycle::archive_session,
            commands::session_lifecycle::restore_session,
            commands::session_lifecycle::get_session_local_delete_preview,
            commands::session_lifecycle::permanently_delete_session,
            commands::ingestion::reconcile_all,
            commands::ingestion::reconcile_source,
            commands::ingestion::reingest_source,
            commands::ingestion::refresh_session,
            commands::ingestion::app_foreground,
            commands::ingestion::get_ingestion_status,
            commands::list_ingest_sources,
            commands::add_ingest_source,
            commands::set_ingest_source_enabled,
            commands::set_all_ingest_sources_enabled,
            commands::remove_ingest_source,
            commands::launch_new_session,
            commands::prepare_new_session,
            commands::launch_prepared,
            commands::cancel_prepared,
            commands::launch_embedded_resume,
            commands::launch_embedded_new,
            commands::continue_session_desktop,
            commands::terminal_list,
            commands::terminal_reconnect,
            commands::terminal_close,
            commands::read_clipboard_for_terminal,
            commands::write_clipboard_for_terminal,
            commands::terminal_for_session,
            commands::terminal_attach,
            commands::terminal_activity_seen,
            commands::terminal_input,
            commands::terminal_acknowledge_output,
            commands::terminal_release_output,
            commands::terminal_resize,
            commands::lookup_navigation,
            commands::match_objects,
            commands::get_agent_status,
            commands::set_agent_launch_enabled,
            commands::redetect_agent,
            commands::get_agent_runtime_settings,
            commands::refresh_agent_runtime_options,
            commands::get_extraction_settings,
            commands::set_extraction_settings,
            commands::assistant_workspace,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // Documented product behavior: closing NoEnding ends every
            // embedded Agent. The external-terminal path is unaffected.
            if let tauri::RunEvent::Exit = event {
                if let Some(server) = app.try_state::<cli::Server>() {
                    server.shutdown();
                }
                if let Some(registry) = app.try_state::<terminal::TerminalRegistry>() {
                    registry.kill_all();
                }
            }
        });
}
