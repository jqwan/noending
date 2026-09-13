//! Session Launcher — New / Resume flows (技术实现方案 §23/§24).
//!
//! New Session: a durable LaunchIntent is created BEFORE the agent starts
//! (the external session id is unknowable at that point). When reconcile
//! later discovers the new external session, the intent is matched and the
//! user's explicitly selected Workstreams become bindings with
//! source=explicit_launch_selection / confidence=1.0 — never guessable away
//! by automatic classification.
//!
//! Resume: builds a delta against the last delivered revision snapshot and
//! records a fresh ContextDelivery only after a successful launch.

use std::path::PathBuf;

use serde::Serialize;

use crate::adapters::AgentCommand;
use crate::domain::{
    binding_source, launch_status, Agent, ContextDelivery, LaunchIntent, Session,
    SessionWorkstreamBinding,
};
use crate::error::{other, Result};
use crate::storage::{new_id, now, Db};

/// LaunchIntent matching window: a discovered session may only claim an
/// intent launched within this range before the session started.
const MATCH_WINDOW_SECS: i64 = 6 * 3600;
const MATCH_CLOCK_SKEW_SECS: i64 = 120;
/// Pending intents older than this never match again.
const INTENT_TTL_SECS: i64 = 24 * 3600;

#[derive(Debug, Clone, Serialize)]
pub struct LaunchResult {
    pub launched_via: String,
    pub command_line: String,
    pub context_file: String,
    pub bundle: crate::context::SessionContextBundle,
    pub note: String,
    pub launch_intent_id: Option<String>,
}

pub struct SessionLauncher {
    pub app_data_dir: PathBuf,
}

impl SessionLauncher {
    fn write_context_file(&self, name_hint: &str, markdown: &str) -> Result<PathBuf> {
        let dir = self.app_data_dir.join("context-bundles");
        std::fs::create_dir_all(&dir)?;
        let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let safe: String = name_hint
            .chars()
            .take(40)
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect();
        let path = dir.join(format!("{}-{}.md", ts, safe));
        std::fs::write(&path, markdown)?;
        Ok(path)
    }

    /// Sync Point: before New Session, sync stale sessions of the workstreams.
    pub fn sync_stale_for_workstreams(&self, db: &Db, workstream_ids: &[String]) -> Result<usize> {
        let engine = crate::sync::SyncEngine::from_settings(db);
        let mut synced = 0;
        for ws_id in workstream_ids {
            let stale = crate::ingestion::stale_sessions(db, ws_id)?;
            for s in stale {
                let n = sync_one_session_with_engine(db, &engine, &s)?;
                synced += n;
            }
        }
        Ok(synced)
    }

    pub fn new_session(
        &self,
        db: &Db,
        agent: Agent,
        workstream_ids: &[String],
        cwd: Option<&str>,
    ) -> Result<LaunchResult> {
        // 1. sync stale sessions that share these workstreams (no-op when empty)
        self.sync_stale_for_workstreams(db, workstream_ids)?;

        // 2. build context bundle; zero contexts → plain launch, no injection.
        //    (UX rule: 关联 Workstream 永远是可选项。)
        let bundle = crate::context::build_bundle(db, "new", None, workstream_ids, 4000)?;
        let ctx_file = if workstream_ids.is_empty() {
            None
        } else {
            let title_hint = workstream_ids
                .first()
                .and_then(|id| db.get_workstream(id).ok().flatten())
                .map(|w| w.title)
                .unwrap_or_else(|| "bundle".into());
            Some(self.write_context_file(&title_hint, &bundle.markdown)?)
        };

        // 3. durable LaunchIntent BEFORE launching: the user's explicit
        //    Workstream selection must survive discovery latency and even a
        //    NoEnding crash right after spawn.
        let intent = LaunchIntent {
            id: new_id(),
            launch_type: "new".into(),
            agent,
            selected_workstream_ids: workstream_ids.to_vec(),
            cwd: cwd.map(|s| s.to_string()),
            context_bundle_markdown: if ctx_file.is_some() {
                Some(bundle.markdown.clone())
            } else {
                None
            },
            process_id: None,
            launched_at: now(),
            matched_session_id: None,
            status: launch_status::PENDING.into(),
            note: String::new(),
            created_at: now(),
            updated_at: now(),
        };
        db.insert_launch_intent(&intent)?;

        // 4. resolve agent CLI
        let install = resolve_install(db, agent)?;
        let adapter = crate::adapters::adapter_for(agent);
        let cwd_path = cwd.map(PathBuf::from);

        // 5. adapter builds command, platform launches it
        let cmd: AgentCommand =
            adapter.build_new_command(&install, ctx_file.as_deref(), cwd_path.as_deref())?;
        let outcome = crate::platform::launcher::launch(&cmd)?;

        db.update_launch_intent(
            &intent.id,
            launch_status::PENDING,
            None,
            &format!("launched_via={};pid={:?}", outcome.launched_via, outcome.pid),
        )?;

        // 6. bindings will be created when discovery matches the intent
        Ok(LaunchResult {
            launched_via: outcome.launched_via,
            command_line: outcome.command_line,
            context_file: ctx_file
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            bundle,
            note: if ctx_file.is_some() {
                "新 Session 启动后会在发现时通过 LaunchIntent 自动建立显式 Workstream 绑定。".into()
            } else {
                "已直接启动（未携带 Workstream Context）；此启动未选择 Workstream，允许 0 绑定。".into()
            },
            launch_intent_id: Some(intent.id),
        })
    }

    pub fn resume_session(
        &self,
        db: &Db,
        session_id: &str,
        extra_workstream_ids: &[String],
    ) -> Result<LaunchResult> {
        let session = db
            .get_session(session_id)?
            .ok_or_else(|| other("Session 不存在"))?;

        // 1. sync this session's own new messages first
        let engine = crate::sync::SyncEngine::from_settings(db);
        sync_one_session_with_engine(db, &engine, &session)?;

        // 2. resolve workstreams: existing bindings + user-added extras.
        //    An empty set is valid: resume natively without context injection.
        //    Resume never re-guesses bindings — they already exist.
        let mut ws_ids: Vec<String> = db
            .bindings_for_session(session_id)?
            .into_iter()
            .map(|b| b.workstream_id)
            .collect();
        for extra in extra_workstream_ids {
            if !ws_ids.contains(extra) {
                ws_ids.push(extra.clone());
                record_binding(db, session_id, extra, "related", binding_source::USER_ASSIGNED, 1.0)?;
            }
        }

        // 3. delta bundle against last delivered revisions (empty → no injection)
        let bundle = crate::context::build_bundle(db, "resume", Some(&session), &ws_ids, 3000)?;
        let ctx_file = if ws_ids.is_empty() {
            None
        } else {
            Some(self.write_context_file("resume", &bundle.markdown)?)
        };

        // 4. launch
        let install = resolve_install(db, session.agent)?;
        let adapter = crate::adapters::adapter_for(session.agent);
        let cwd_path = session.cwd.clone().map(PathBuf::from);
        let cmd = adapter.build_resume_command(
            &install,
            &session.agent_session_id,
            ctx_file.as_deref(),
            cwd_path.as_deref(),
        )?;
        let outcome = crate::platform::launcher::launch(&cmd)?;

        // 5. successful launch → record what was actually delivered, so the
        //    NEXT resume computes a true delta. Also touch binding usage.
        let delivered: Vec<String> = bundle
            .sections
            .iter()
            .filter_map(|s| s.revision_id.clone())
            .collect();
        for ws_id in &ws_ids {
            record_binding(db, session_id, ws_id, "related", binding_source::USER_ASSIGNED, 1.0)?;
            db.record_delivery(&ContextDelivery {
                id: new_id(),
                session_id: session_id.to_string(),
                workstream_id: ws_id.clone(),
                bundle_id: bundle.bundle_id.clone(),
                delivered_revisions: delivered.clone(),
                delivered_at: now(),
            })?;
        }

        Ok(LaunchResult {
            launched_via: outcome.launched_via,
            command_line: outcome.command_line,
            context_file: ctx_file
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            bundle,
            note: if ctx_file.is_some() {
                "已同步最新消息并生成增量上下文（基于上次实际交付的修订快照）。".into()
            } else {
                "该 Session 未关联 Workstream：已同步自身消息后直接恢复，未注入上下文。".into()
            },
            launch_intent_id: None,
        })
    }
}

fn resolve_install(db: &Db, agent: Agent) -> Result<crate::platform::exec_resolver::AgentInstallation> {
    match db.get_installation(agent)? {
        Some(i) if std::path::Path::new(&i.executable_path).exists() => Ok(i),
        _ => {
            let i = crate::platform::exec_resolver::resolve(agent)?;
            let _ = db.save_installation(&i);
            Ok(i)
        }
    }
}

/// Record (or refresh) a binding. `source`/`confidence` describe how this
/// binding was established; storage never lets a weaker source overwrite a
/// stronger one.
pub fn record_binding(
    db: &Db,
    session_id: &str,
    workstream_id: &str,
    role: &str,
    source: &str,
    confidence: f64,
) -> Result<()> {
    let existing = db.bindings_for_session(session_id)?;
    if let Some(b) = existing.iter().find(|b| b.workstream_id == workstream_id) {
        let mut b = b.clone();
        b.last_used_at = now();
        if source == binding_source::EXPLICIT_LAUNCH || source == binding_source::USER_ASSIGNED {
            b.source = source.to_string();
            b.confidence = confidence;
        }
        db.bind(&b)?;
        return Ok(());
    }
    let b = SessionWorkstreamBinding {
        session_id: session_id.to_string(),
        workstream_id: workstream_id.to_string(),
        role: role.to_string(),
        source: source.to_string(),
        confidence,
        last_seen_revision: None,
        last_sync_cursor: 0,
        created_at: now(),
        last_used_at: now(),
    };
    db.bind(&b)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// LaunchIntent matching
// ---------------------------------------------------------------------------

/// Try to match a newly discovered session against pending LaunchIntents.
/// Signals: agent type (required), launch→session timing, cwd. One clear
/// winner → auto-match; several close candidates → ambiguous (never
/// silently guess); nothing → stays pending for a later reconcile.
pub fn try_match_launch_intents(db: &Db, session: &Session) -> Result<bool> {
    let pending = db.list_launch_intents(&[launch_status::PENDING, launch_status::AMBIGUOUS], 100)?;
    if pending.is_empty() {
        return Ok(false);
    }
    let session_start = session
        .started_at
        .as_deref()
        .or(session.last_activity_at.as_deref())
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.with_timezone(&chrono::Utc));

    let mut scored: Vec<(LaunchIntent, f64)> = Vec::new();
    for intent in pending {
        if intent.agent != session.agent {
            continue;
        }
        if intent.matched_session_id.as_deref() == Some(session.id.as_str()) {
            continue;
        }
        let launched = match chrono::DateTime::parse_from_rfc3339(&intent.launched_at) {
            Ok(t) => t.with_timezone(&chrono::Utc),
            Err(_) => continue,
        };
        let Some(start) = session_start else { continue };
        let lower = launched - chrono::Duration::seconds(MATCH_CLOCK_SKEW_SECS);
        let upper = launched + chrono::Duration::seconds(MATCH_WINDOW_SECS);
        if start < lower || start > upper {
            continue;
        }
        let mut score = 1.0;
        match (&intent.cwd, &session.cwd) {
            (Some(ic), Some(sc)) if !ic.is_empty() => {
                if sc.starts_with(ic) || ic.starts_with(sc) {
                    score += 3.0;
                }
            }
            _ => {}
        }
        // closeness bonus: the sooner after launch, the stronger the signal
        if let Ok(delta) = (start - launched).to_std() {
            let secs = delta.as_secs() as f64;
            if secs < 60.0 {
                score += 2.0;
            } else if secs < 600.0 {
                score += 1.0;
            }
        }
        scored.push((intent, score));
    }

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    match scored.len() {
        0 => Ok(false),
        1 => {
            let (intent, _) = scored.remove(0);
            apply_match(db, &intent, session)?;
            Ok(true)
        }
        _ => {
            let (best, best_score) = &scored[0];
            let (_, second) = &scored[1];
            if best_score - second >= 2.0 {
                let (intent, _) = scored.remove(0);
                apply_match(db, &intent, session)?;
                Ok(true)
            } else {
                // ambiguous: mark the best candidate so the UI can ask
                db.update_launch_intent(
                    &best.id,
                    launch_status::AMBIGUOUS,
                    None,
                    &format!(
                        "多个候选 Session（score {:.1} vs {:.1}），等待用户确认",
                        best_score, second
                    ),
                )?;
                Ok(false)
            }
        }
    }
}

pub fn apply_match(db: &Db, intent: &LaunchIntent, session: &Session) -> Result<()> {
    for ws_id in &intent.selected_workstream_ids {
        record_binding(
            db,
            &session.id,
            ws_id,
            "primary",
            binding_source::EXPLICIT_LAUNCH,
            1.0,
        )?;
    }
    db.update_launch_intent(
        &intent.id,
        launch_status::MATCHED,
        Some(&session.id),
        &format!("自动匹配：session {}", session.id),
    )?;
    Ok(())
}

/// Pending intents that never produced a session expire; ambiguous ones
/// wait for the user and only expire after 3× the TTL.
pub fn expire_stale_launch_intents(db: &Db) -> Result<usize> {
    let pending = db.list_launch_intents(&[launch_status::PENDING, launch_status::AMBIGUOUS], 500)?;
    let now_ts = chrono::Utc::now();
    let mut expired = 0;
    for intent in pending {
        let ttl = if intent.status == launch_status::AMBIGUOUS {
            INTENT_TTL_SECS * 3
        } else {
            INTENT_TTL_SECS
        };
        if let Ok(launched) = chrono::DateTime::parse_from_rfc3339(&intent.launched_at) {
            if now_ts - launched.with_timezone(&chrono::Utc) > chrono::Duration::seconds(ttl) {
                db.update_launch_intent(&intent.id, launch_status::EXPIRED, None, "超时未发现匹配 Session")?;
                expired += 1;
            }
        }
    }
    Ok(expired)
}

// ---------------------------------------------------------------------------
// Ingest + Sync single path
// ---------------------------------------------------------------------------

/// Single-path ingest+sync:
/// 1. ingest: adapter reads the delta (append/truncate/rewrite aware);
///    events + read cursor commit atomically (content-deduped).
/// 2. sync: every event with sequence > processed_cursor is handed to the
///    sync engine; mutations + SyncRun + processed cursor commit atomically.
/// A crash between (1) and (2) self-heals: the events are already durable
/// and the next run picks them up from the processed cursor.
/// Returns (events_ingested, mutations_applied).
pub fn ingest_and_sync_session(
    db: &Db,
    engine: &crate::sync::SyncEngine,
    session: &Session,
) -> Result<(i64, usize)> {
    let adapter = crate::adapters::adapter_for(session.agent);
    let cursor = db.get_source_cursor(&session.id)?;
    let delta = adapter.read_delta(session, &cursor)?;
    let source = delta
        .source
        .clone()
        .ok_or_else(|| other("adapter returned no source state"))?;
    let stored = db.append_source_events(&session.id, &delta.events, &source, &session.raw_path)?;
    if !stored.is_empty() {
        if let Err(e) = db.index_new_events(&stored) {
            eprintln!("[sync] index_events failed: {}", e);
        }
    }

    // Sync everything not yet processed (may include events ingested by an
    // earlier crashed run — the read/processed split makes that recoverable).
    let processed = db.get_processed_sequence(&session.id)?;
    let pending = db.get_events(&session.id, Some(processed), 10_000)?;
    let mut applied = 0usize;
    if !pending.is_empty() {
        let to = pending.last().map(|e| e.sequence).unwrap_or(processed);
        let out = engine.run_session_sync(db, session, &pending, processed, to)?;
        applied = out.applied;
    }
    Ok((stored.len() as i64, applied))
}

/// Ingest + sync a single session delta. Returns number of applied mutations.
pub fn sync_one_session(db: &Db, engine: &crate::sync::SyncEngine, session: &Session) -> Result<usize> {
    sync_one_session_with_engine(db, engine, session)
}

pub fn sync_one_session_with_engine(
    db: &Db,
    engine: &crate::sync::SyncEngine,
    session: &Session,
) -> Result<usize> {
    Ok(ingest_and_sync_session(db, engine, session)?.1)
}
