//! IngestionCoordinator — the one place background ingestion is queued.
//!
//! Ingestion is the ONLY automatic pipeline and it never calls AI. Tasks are
//! queued in memory (this process), merged when they cover the same scope, and
//! run on a single worker thread so a Source is never scanned twice at once.
//!
//! Triggers:
//! * App startup → `ReconcileAll`
//! * Return to foreground past the freshness threshold → `ReconcileAll`
//! * New launch success → `ReconcileAll`
//! * Resume launch success → `RefreshSession`
//! * Advanced maintenance page → `ReconcileAll` / `ReconcileSource` /
//!   `ReingestSource`
//!
//! Ordinary page opens never trigger ingestion: reads only touch the database.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::error::Result;

use super::AppState;

/// Freshness threshold for the foreground trigger.
pub const FRESHNESS: Duration = Duration::from_secs(5 * 60);

/// What a task covers. Two tasks with the same scope merge; different scopes
/// (a partial reconcile vs. a forced re-ingest) never swallow each other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestScope {
    ReconcileAll,
    ReconcileSource(String),
    RefreshSession(String),
    ReingestSource(String),
}

impl IngestScope {
    pub fn label(&self) -> String {
        match self {
            IngestScope::ReconcileAll => "reconcile_all".into(),
            IngestScope::ReconcileSource(id) => format!("reconcile_source:{id}"),
            IngestScope::RefreshSession(id) => format!("refresh_session:{id}"),
            IngestScope::ReingestSource(id) => format!("reingest_source:{id}"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct IngestTaskStatus {
    pub scope: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub discovered: usize,
    pub messages: i64,
    pub error: Option<String>,
}

#[derive(Default)]
struct Inner {
    running: bool,
    queue: Vec<IngestScope>,
    status: Option<IngestTaskStatus>,
    last_success: Option<Instant>,
}

/// In-memory queue + status. Registered once in `AppState`.
#[derive(Default)]
pub struct IngestionCoordinator {
    inner: Mutex<Inner>,
}

impl IngestionCoordinator {
    /// Merge a new task into the queue. `ReconcileAll` subsumes any queued
    /// partial reconciles; a re-ingest is semantically different and is kept.
    fn enqueue(&self, scope: IngestScope) -> bool {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if inner.queue.contains(&scope) {
            // The queued task already belongs to the active worker. Returning
            // true here would start a second worker against the same queue.
            return false;
        }
        if scope == IngestScope::ReconcileAll {
            inner.queue.retain(|s| {
                !matches!(
                    s,
                    IngestScope::ReconcileSource(_) | IngestScope::RefreshSession(_)
                )
            });
        }
        inner.queue.push(scope);
        if inner.running {
            false
        } else {
            inner.running = true;
            true
        }
    }

    fn next(&self) -> Option<IngestScope> {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if inner.queue.is_empty() {
            inner.running = false;
            return None;
        }
        let scope = inner.queue.remove(0);
        inner.status = Some(IngestTaskStatus {
            scope: scope.label(),
            started_at: Some(crate::storage::now()),
            finished_at: None,
            discovered: 0,
            messages: 0,
            error: None,
        });
        Some(scope)
    }

    fn finish(
        &self,
        discovered: usize,
        messages: i64,
        error: Option<String>,
        is_full_reconcile: bool,
    ) {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(status) = inner.status.as_mut() {
            status.finished_at = Some(crate::storage::now());
            status.discovered = discovered;
            status.messages = messages;
            status.error = error.clone();
        }
        if is_full_reconcile && error.is_none() {
            inner.last_success = Some(Instant::now());
        }
    }

    pub fn status(&self) -> Option<IngestTaskStatus> {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .status
            .clone()
    }

    /// True when the foreground-return trigger should start a reconcile.
    pub fn is_stale(&self) -> bool {
        let inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        match inner.last_success {
            Some(at) => at.elapsed() >= FRESHNESS,
            None => true,
        }
    }
}

/// Enqueue a task and, when the worker is idle, start it. Never blocks.
pub fn enqueue(app: &AppHandle, scope: IngestScope) {
    let should_spawn = {
        let state = app.state::<AppState>();
        state.ingestion.enqueue(scope)
    };
    if should_spawn {
        spawn_worker(app.clone());
    }
}

fn spawn_worker(app: AppHandle) {
    std::thread::spawn(move || {
        while let Some(scope) = {
            let state = app.state::<AppState>();
            state.ingestion.next()
        } {
            let (discovered, messages, error) = run_scope(&app, &scope);
            {
                let state = app.state::<AppState>();
                state.ingestion.finish(
                    discovered,
                    messages,
                    error.clone(),
                    matches!(&scope, IngestScope::ReconcileAll),
                );
            }
            let _ = app.emit(
                "ingestion-completed",
                serde_json::json!({
                    "scope": scope.label(),
                    "discovered": discovered,
                    "messages": messages,
                    "error": error,
                }),
            );
        }
    });
}

fn run_scope(app: &AppHandle, scope: &IngestScope) -> (usize, i64, Option<String>) {
    let state = app.state::<AppState>();
    let workspace = super::launch_workspace(app);
    let notify = |s: &crate::domain::Session| {
        eprintln!(
            "[ingest] processing: {}",
            s.title.as_deref().unwrap_or("(untitled)")
        );
    };
    let result = match scope {
        IngestScope::ReconcileAll => {
            crate::ingestion::reconcile_all_report(&state.db, &workspace, &notify).map(|report| {
                let error = (!report.failures.is_empty()).then(|| report.failures.join("; "));
                (report.discovered, report.messages, error)
            })
        }
        IngestScope::ReconcileSource(id) => match state.db.get_ingest_source(id) {
            Ok(Some(source)) => {
                crate::ingestion::reconcile_source(&state.db, &source, &workspace, &notify)
                    .map(|(d, m)| (d, m, None))
            }
            Ok(None) => Ok((0, 0, None)),
            Err(e) => Err(e),
        },
        IngestScope::ReingestSource(id) => match state.db.get_ingest_source(id) {
            Ok(Some(source)) => {
                crate::ingestion::reingest_source(&state.db, &source, &workspace, &notify)
                    .map(|(d, m)| (d, m, None))
            }
            Ok(None) => Ok((0, 0, None)),
            Err(e) => Err(e),
        },
        IngestScope::RefreshSession(id) => {
            crate::ingestion::refresh_session(&state.db, id).map(|n| (0usize, n, None))
        }
    };
    match result {
        Ok((d, m, error)) => (d, m, error),
        Err(e) => {
            eprintln!("[ingest] {} failed: {}", scope.label(), e);
            (0, 0, Some(e.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_already_queued_scope_does_not_request_another_worker() {
        let coordinator = IngestionCoordinator::default();
        {
            let mut inner = coordinator.inner.lock().unwrap();
            inner.running = true;
            inner.queue.push(IngestScope::ReconcileAll);
        }

        assert!(!coordinator.enqueue(IngestScope::ReconcileAll));
    }

    #[test]
    fn only_a_complete_full_reconcile_advances_freshness() {
        let coordinator = IngestionCoordinator::default();
        coordinator.finish(0, 0, None, false);
        assert!(
            coordinator.is_stale(),
            "a session refresh is not a full pass"
        );

        coordinator.finish(0, 0, Some("partial failure".into()), true);
        assert!(
            coordinator.is_stale(),
            "partial failures must remain retryable"
        );

        coordinator.finish(0, 0, None, true);
        assert!(!coordinator.is_stale());
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Advanced maintenance: reconcile every enabled Source.
#[tauri::command]
pub fn reconcile_all(app: AppHandle) -> Result<serde_json::Value> {
    enqueue(&app, IngestScope::ReconcileAll);
    Ok(serde_json::json!({ "queued": true }))
}

/// Advanced maintenance: reconcile one Source.
#[tauri::command]
pub fn reconcile_source(app: AppHandle, source_id: String) -> Result<serde_json::Value> {
    enqueue(&app, IngestScope::ReconcileSource(source_id));
    Ok(serde_json::json!({ "queued": true }))
}

/// Advanced maintenance: force a full re-scan of one Source.
#[tauri::command]
pub fn reingest_source(app: AppHandle, source_id: String) -> Result<serde_json::Value> {
    enqueue(&app, IngestScope::ReingestSource(source_id));
    Ok(serde_json::json!({ "queued": true }))
}

/// Foreground-return hook: reconcile only when the last success is older than
/// the freshness threshold. Ordinary navigation must not call this.
#[tauri::command]
pub fn app_foreground(app: AppHandle, state: State<AppState>) -> Result<serde_json::Value> {
    if state.ingestion.is_stale() {
        enqueue(&app, IngestScope::ReconcileAll);
        Ok(serde_json::json!({ "queued": true }))
    } else {
        Ok(serde_json::json!({ "queued": false }))
    }
}

/// The last ingestion task's status, for the advanced maintenance page.
#[tauri::command]
pub fn get_ingestion_status(state: State<AppState>) -> Result<Option<IngestTaskStatus>> {
    Ok(state.ingestion.status())
}
