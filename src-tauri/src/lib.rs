//! NoEnding library crate — exposes the domain API to integration tests
//! and hosts the Tauri run() entry.

pub mod adapters;
pub mod agent_runtime;
pub mod assistant;
pub mod commands;
pub mod context;
pub mod context_eval;
pub mod domain;
pub mod error;
pub mod ingestion;
pub mod launcher;
pub mod lifecycle;
pub mod platform;
pub mod search;
pub mod settings;
pub mod storage;
pub mod sync;
pub mod workspace;

use std::sync::Mutex;

use tauri::{Emitter, Manager};

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // NoEnding Home first, always before the database opens (§2, §3):
            // it resolves $NOENDING_HOME → bootstrap.current_home → ~/.noending,
            // applies any pending relocation, and creates data/ runtime/ logs/
            // workspace/. A *migration* failure
            // is not fatal here — it returns the old Home with the reason in
            // `notes` — because "no database found" would look like data loss.
            let startup = workspace::home::prepare_home(&workspace::home::StartupInputs::from_environment())?;
            for note in &startup.notes {
                eprintln!("[noending] {note}");
            }
            let home = startup.home.clone();
            eprintln!("[noending] home at {} ({})", home.root.display(), home.root_str());
            let db_path = home.db_path.clone();
            let db = storage::Db::open(&db_path)?;
            eprintln!("[noending] db at {}", db_path.display());

            // The physical layer behind the one `WorkspaceAttaching` door.
            // Managed as state for the
            // commands, and registered for ingestion, which keeps a plain
            // `ensure_session_row` signature (§19).
            app.manage(home.clone());
            let layer = std::sync::Arc::new(workspace::wiring::WorkspaceLayer::new(&home));
            app.manage(layer.clone());
            let _ = workspace::session::register_workspace_attacher(layer.clone());
            app.manage(workspace::workstream::PathService::new(layer));

            // One-shot migration of the pre-runtime-Assistant model keys into
            // Agent runtime overrides. Reads, writes, then clears the legacy
            // keys — never a silent dual-write.
            match agent_runtime::migrate_legacy_assistant_runtime(&db) {
                Ok(Some(agent)) => eprintln!(
                    "[noending] migrated legacy assistant runtime settings to {} overrides",
                    agent.as_str()
                ),
                Ok(_) => {}
                Err(e) => eprintln!("[noending] assistant runtime migration skipped: {e}"),
            }

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
                db,
                sync_in_progress: std::sync::atomic::AtomicBool::new(false),
                workspace_refresh_in_progress: std::sync::atomic::AtomicBool::new(false),
                prepared_launches: Mutex::new(std::collections::HashMap::new()),
            });

            // make pre-existing events searchable (idempotent)
            {
                let state: tauri::State<commands::AppState> = app.state();
                if let Err(e) = state.db.backfill_search_index() {
                    eprintln!("[noending] search backfill failed: {}", e);
                }
                // §23 deletion crash recovery: an interrupted permanent
                // deletion never auto-continues — it becomes a failed job the
                // user retries (source already absent → purge completes) or
                // cancels (session stays in Trash).
                match lifecycle::recover_interrupted_deletions(&state.db) {
                    Ok(0) => {}
                    Ok(n) => eprintln!("[noending] recovered {n} interrupted deletion job(s)"),
                    Err(e) => eprintln!("[noending] deletion recovery failed: {}", e),
                }
            }

            // Application Reconcile: ingest what happened while we were away.
            // Runs on a worker thread; the store serializes writes itself, so
            // the UI stays responsive. UI is notified on completion.
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    use std::sync::atomic::Ordering;
                    let state: tauri::State<commands::AppState> = handle.state();
                    if state.sync_in_progress.swap(true, Ordering::SeqCst) {
                        return; // a user-triggered sync is already running
                    }
                    let result = (|| -> error::Result<(usize, i64)> {
                        let engine = sync::SyncEngine::from_settings(&state.db);
                        ingestion::reconcile_with_engine(
                            &state.db,
                            &engine,
                            &crate::commands::launch_workspace(&handle),
                            &|s| {
                                eprintln!(
                                    "[reconcile] processing: {}",
                                    s.title.as_deref().unwrap_or("(untitled)")
                                );
                            },
                        )
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

                    // Workspace Reconcile (§8, §26, §42.3-M7): Git detection on
                    // the paths the v12 migration could only see lexically. It
                    // runs after ingestion because a Session's cwd is what tells
                    // us a directory is real, and it re-observes per path instead
                    // of holding the DB lock across a `git` call.
                    {
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
            // Workstreams without leaving a Revision (§42.2-E10).
            commands::get_default_agent,
            commands::set_default_agent,
            commands::get_context_delivery_level,
            commands::set_context_delivery_level,
            commands::get_context_intelligence_enabled,
            commands::set_context_intelligence_enabled,
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
            commands::session_workspace::bind_session_workstream,
            commands::session_workspace::unbind_session_workstream,
            commands::session_workspace::replace_session_bindings,
            commands::session_workspace::list_session_bindings,
            // Session Lifecycle & Deletion v0.1: Trash / Restore and the
            // prepared permanent deletion flow. The UI submits ids only.
            commands::session_lifecycle::trash_session,
            commands::session_lifecycle::restore_session,
            commands::session_lifecycle::prepare_session_permanent_delete,
            commands::session_lifecycle::execute_session_permanent_delete,
            commands::session_lifecycle::cancel_session_permanent_delete,
            commands::session_lifecycle::get_session_deletion_job,
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
