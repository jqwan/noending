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

        // 1b. effective cwd. Launching WITHOUT a directory drops the agent
        //     into the terminal's default ($HOME), where an untrusted-
        //     directory prompt stops the session from ever starting.
        let effective_cwd: Option<String> = resolve_new_session_cwd(db, workstream_ids, cwd)?;

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
        //    NoEnding crash right after spawn. The bundle's delivered
        //    revision snapshot travels with the intent: when a session
        //    matches, apply_match records it as ContextDelivery so the
        //    session's FIRST resume is a true delta, not a full re-send.
        let delivered_by_ws = if ctx_file.is_some() {
            delivered_revisions_by_workstream(
                &bundle,
                workstream_ids.first().map(|s| s.as_str()).unwrap_or(""),
            )
        } else {
            Default::default()
        };
        let intent = LaunchIntent {
            id: new_id(),
            launch_type: "new".into(),
            agent,
            selected_workstream_ids: workstream_ids.to_vec(),
            cwd: effective_cwd.clone(),
            context_bundle_markdown: if ctx_file.is_some() {
                Some(bundle.markdown.clone())
            } else {
                None
            },
            context_bundle_revisions: if ctx_file.is_some() {
                Some(
                    serde_json::json!({
                        "bundle_id": bundle.bundle_id,
                        "by_workstream": delivered_by_ws,
                    })
                    .to_string(),
                )
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
        let cwd_path = effective_cwd.map(PathBuf::from);

        // 5. adapter builds command, platform launches it
        let cmd: AgentCommand =
            adapter.build_new_command(&install, ctx_file.as_deref(), cwd_path.as_deref())?;
        let outcome = crate::platform::launcher::launch(&cmd)?;

        db.update_launch_intent(
            &intent.id,
            launch_status::PENDING,
            None,
            &format!(
                "launched_via={};pid={:?}",
                outcome.launched_via, outcome.pid
            ),
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
                "已直接启动（未携带 Workstream Context）；此启动未选择 Workstream，允许 0 绑定。"
                    .into()
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
                record_binding(
                    db,
                    session_id,
                    extra,
                    "related",
                    binding_source::USER_ASSIGNED,
                    1.0,
                )?;
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
        //    NEXT resume computes a true delta. Revisions are attributed to
        //    the workstream that OWNS each section — stamping every
        //    workstream with the full list would corrupt cross-workstream
        //    delta computation (wrong `gone` sections, double deliveries).
        //    Also touch binding usage.
        let delivered_by_ws = delivered_revisions_by_workstream(
            &bundle,
            ws_ids.first().map(|s| s.as_str()).unwrap_or(""),
        );
        for ws_id in &ws_ids {
            record_binding(
                db,
                session_id,
                ws_id,
                "related",
                binding_source::USER_ASSIGNED,
                1.0,
            )?;
            db.record_delivery(&ContextDelivery {
                id: new_id(),
                session_id: session_id.to_string(),
                workstream_id: ws_id.clone(),
                bundle_id: bundle.bundle_id.clone(),
                delivered_revisions: delivered_by_ws.get(ws_id).cloned().unwrap_or_default(),
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

/// Group the bundle's delivered revision ids by the workstream that owns
/// each section. Sections without a workstream attribution (rare) fall back
/// to `fallback_ws`.
fn delivered_revisions_by_workstream(
    bundle: &crate::context::SessionContextBundle,
    fallback_ws: &str,
) -> std::collections::BTreeMap<String, Vec<String>> {
    let mut by_ws: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for s in &bundle.sections {
        if let Some(rev) = &s.revision_id {
            let ws = s
                .workstream_id
                .clone()
                .unwrap_or_else(|| fallback_ws.to_string());
            let entry = by_ws.entry(ws).or_default();
            if !entry.contains(rev) {
                entry.push(rev.clone());
            }
        }
    }
    by_ws
}

fn resolve_install(
    db: &Db,
    agent: Agent,
) -> Result<crate::platform::exec_resolver::AgentInstallation> {
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

/// Expand a leading `~` / `~/` / `~\` to the user's home directory.
/// Users type `~/projects/x` into default_cwd; passing the tilde through to
/// the rendered terminal script would break it — POSIX single quotes and
/// PowerShell `-LiteralPath` both treat `~` literally, so the cd silently
/// falls back to $HOME. Absolute paths (and `~` anywhere but the leading
/// position) pass through untouched; `~user` is intentionally unsupported.
/// The stored default_cwd keeps the user's original text — this is the one
/// canonical expansion point, so LaunchIntent records the absolute path.
pub fn expand_tilde(p: &str) -> String {
    let trimmed = p.trim();
    let rest = if trimmed == "~" {
        Some("")
    } else if let Some(r) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"))
    {
        Some(r)
    } else {
        None
    };
    match rest {
        Some(r) => match dirs::home_dir() {
            // join("") would append a trailing separator to a bare `~`
            Some(home) if r.is_empty() => home.to_string_lossy().into_owned(),
            Some(home) => home.join(r).to_string_lossy().into_owned(),
            None => p.to_string(),
        },
        None => p.to_string(),
    }
}

/// Launch directory for a New Session, in priority order:
/// 1. the caller's explicit value (kept for API completeness);
/// 2. a selected Workstream's `default_cwd` — the first set one in the
///    user's selection order;
/// 3. the most recent session cwd across the selected Workstreams
///    ("continue where you left off", absolute by construction — it comes
///    from parsed transcripts — so no tilde expansion is applied there).
/// A Workstream's default_cwd is a launch convenience, never identity:
/// the Workstream is not a path, and Sessions keep their own cwd.
pub fn resolve_new_session_cwd(
    db: &Db,
    workstream_ids: &[String],
    explicit: Option<&str>,
) -> Result<Option<String>> {
    if let Some(c) = explicit {
        return Ok(Some(expand_tilde(c)));
    }
    for ws_id in workstream_ids {
        if let Some(w) = db.get_workstream(ws_id)? {
            if let Some(cwd) = w.default_cwd {
                return Ok(Some(expand_tilde(&cwd)));
            }
        }
    }
    db.latest_session_cwd_for_workstreams(workstream_ids)
}

/// Apply the user's edited binding set for a Session as ONE atomic diff.
/// Unchanged rows are kept verbatim (provenance, created_at, cursors and
/// last_used_at survive — a metadata edit is not a "use"). A role edit on
/// an AUTOMATIC binding upgrades it to user_assigned: sync replaces AUTO
/// rows wholesale (role reset to "related"), so without the upgrade the
/// user's role choice would be silently undone by the next classification;
/// as a strong binding it also arms the binding-decision CAS. Role edits on
/// explicit/user bindings keep their provenance. Every removed row — any
/// provenance — leaves a durable removal tombstone: a user rejection must
/// survive even the loss of the session's last strong binding (which would
/// make the session auto-classifiable again). Only genuinely new rows are
/// inserted as user_assigned. This replaces the old unbind-all → rebind-all
/// edit flow, which reset provenance and cursors and could leave partial
/// state on failure.
pub fn replace_session_bindings(
    db: &Db,
    session_id: &str,
    desired: &[(String, String)], // (workstream_id, role)
) -> Result<()> {
    // Validate before touching anything so a bad row cannot produce a
    // half-applied edit.
    for (_, role) in desired {
        if role != "primary" && role != "related" {
            return Err(other(&format!("未知绑定角色: {role}")));
        }
    }
    // De-duplicate repeated workstreams (last entry wins, order preserved).
    let mut desired: Vec<(String, String)> =
        desired
            .iter()
            .rev()
            .fold(Vec::new(), |mut acc, (ws, role)| {
                if !acc.iter().any(|(w, _)| w == ws) {
                    acc.push((ws.clone(), role.clone()));
                }
                acc
            });
    desired.reverse();

    let existing = db.bindings_for_session(session_id)?;
    db.tx(|tx| {
        use rusqlite::params;
        for b in &existing {
            if !desired.iter().any(|(ws, _)| *ws == b.workstream_id) {
                // a user rejection is durable for ANY provenance: tombstone
                // the pair so classification cannot re-propose it
                crate::storage::remove_binding_by_user_conn(tx, session_id, &b.workstream_id)?;
            }
        }
        for (workstream_id, role) in &desired {
            match existing.iter().find(|b| b.workstream_id == *workstream_id) {
                Some(b) if b.role != *role => {
                    // role-only edit: created_at, cursors and last_used_at
                    // stay; an auto provenance becomes user_assigned so the
                    // decision survives the next auto-classification.
                    tx.execute(
                        "UPDATE session_workstream_bindings
                         SET role = ?3,
                             source = CASE WHEN source = ?4 THEN ?5 ELSE source END,
                             confidence = CASE WHEN source = ?4 THEN 1.0 ELSE confidence END
                         WHERE session_id = ?1 AND workstream_id = ?2",
                        params![
                            session_id,
                            workstream_id,
                            role,
                            binding_source::AUTO,
                            binding_source::USER_ASSIGNED
                        ],
                    )?;
                }
                Some(_) => {} // unchanged: the row is kept exactly as-is
                None => {
                    // only a binding the user just added is user_assigned;
                    // bind_conn also lifts any removal tombstone for the pair
                    let b = SessionWorkstreamBinding {
                        session_id: session_id.to_string(),
                        workstream_id: workstream_id.clone(),
                        role: role.clone(),
                        source: binding_source::USER_ASSIGNED.into(),
                        confidence: 1.0,
                        last_seen_revision: None,
                        last_sync_cursor: 0,
                        created_at: now(),
                        last_used_at: now(),
                    };
                    crate::storage::bind_conn(tx, &b)?;
                }
            }
        }
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// LaunchIntent matching
// ---------------------------------------------------------------------------

/// Try to match a newly discovered session against pending LaunchIntents.
/// Signals: agent type (required), launch→session timing, cwd. One clear
/// winner → auto-match; several close candidates → ambiguous (never
/// silently guess); nothing → stays pending for a later reconcile.
pub fn try_match_launch_intents(db: &Db, session: &Session) -> Result<bool> {
    let pending =
        db.list_launch_intents(&[launch_status::PENDING, launch_status::AMBIGUOUS], 100)?;
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
    // The matched session already RECEIVED the context bundle at launch:
    // record the delivery snapshot now, so its first resume computes a true
    // delta instead of re-sending the full context.
    if let Some(snap) = &intent.context_bundle_revisions {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(snap) {
            let bundle_id = v
                .get("bundle_id")
                .and_then(|b| b.as_str())
                .unwrap_or(&intent.id)
                .to_string();
            if let Some(by_ws) = v.get("by_workstream").and_then(|m| m.as_object()) {
                for (ws_id, revs) in by_ws {
                    let revisions: Vec<String> = revs
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    db.record_delivery(&ContextDelivery {
                        id: new_id(),
                        session_id: session.id.clone(),
                        workstream_id: ws_id.clone(),
                        bundle_id: bundle_id.clone(),
                        delivered_revisions: revisions,
                        delivered_at: now(),
                    })?;
                }
            }
        }
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
    let pending =
        db.list_launch_intents(&[launch_status::PENDING, launch_status::AMBIGUOUS], 500)?;
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
                db.update_launch_intent(
                    &intent.id,
                    launch_status::EXPIRED,
                    None,
                    "超时未发现匹配 Session",
                )?;
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
pub fn sync_one_session(
    db: &Db,
    engine: &crate::sync::SyncEngine,
    session: &Session,
) -> Result<usize> {
    sync_one_session_with_engine(db, engine, session)
}

pub fn sync_one_session_with_engine(
    db: &Db,
    engine: &crate::sync::SyncEngine,
    session: &Session,
) -> Result<usize> {
    Ok(ingest_and_sync_session(db, engine, session)?.1)
}
