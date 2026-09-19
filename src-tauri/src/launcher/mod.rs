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

use serde::{Deserialize, Serialize};

use crate::adapters::AgentCommand;
use crate::agent_runtime::AgentRuntimeOverrides;
use crate::context::ContextDeliveryLevel;
use crate::domain::{binding_source, launch_status, Agent, ContextDelivery, LaunchIntent, Session};
use crate::error::{other, Result};
use crate::storage::{new_id, now, Db};

/// LaunchIntent matching window: a discovered session may only claim an
/// intent launched within this range before the session started.
const MATCH_WINDOW_SECS: i64 = 6 * 3600;
const MATCH_CLOCK_SKEW_SECS: i64 = 120;
/// Pending intents older than this never match again.
const INTENT_TTL_SECS: i64 = 24 * 3600;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparedLaunch {
    pub id: String,
    pub mode: String, // "new" | "resume"
    pub agent: Agent,
    pub session_id: Option<String>,
    pub workstream_ids: Vec<String>,
    pub extra_workstream_ids: Vec<String>,
    pub cwd: Option<String>,
    pub delivery_level: ContextDeliveryLevel,
    pub bundle: crate::context::SessionContextBundle,
    /// NoEnding's runtime override intent, frozen at Preview time. `None`
    /// fields mean *Agent default* — Launch must pass no corresponding flag.
    /// It is part of `state_fingerprint`, so changing an override between
    /// Preview and Launch invalidates the preview instead of silently
    /// launching with parameters the user never saw.
    pub runtime: AgentRuntimeOverrides,
    pub state_fingerprint: String,
    pub prepared_at: String,
}

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
    /// Where launch artifacts go. Since v0.2 that is NoEnding Home's `runtime/`
    /// rather than the pre-Home `app_data_dir()`: a context bundle is an app
    /// artifact, and the Home is the one place that says where app data lives
    /// (§42.3-M14).
    pub runtime_dir: PathBuf,
}

impl SessionLauncher {
    fn write_context_file(&self, name_hint: &str, markdown: &str) -> Result<PathBuf> {
        let dir = self.runtime_dir.join("context-bundles");
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

    /// Prepare New Session:
    /// Ingests/syncs stale workstream sessions, resolves cwd and delivery level,
    /// builds the exact context bundle, and captures a state fingerprint.
    ///
    /// INVARIANT: Zero premature side effects. Does NOT insert LaunchIntent,
    /// does NOT write context files, and does NOT record delivery snapshots.
    pub fn prepare_new(
        &self,
        db: &Db,
        agent: Agent,
        workstream_ids: &[String],
        cwd: Option<&str>,
    ) -> Result<PreparedLaunch> {
        self.sync_stale_for_workstreams(db, workstream_ids)?;

        let effective_cwd = resolve_new_session_cwd(db, workstream_ids, cwd)?;

        let delivery_level = crate::settings::context_delivery_level_of(db)?;
        let runtime = crate::agent_runtime::runtime_overrides_for_launch(db, agent)?;
        let bundle = crate::context::build_bundle(db, "new", None, workstream_ids, delivery_level)?;

        let state_fingerprint =
            compute_state_fingerprint(db, "new", None, workstream_ids, delivery_level, agent)?;

        Ok(PreparedLaunch {
            id: new_id(),
            mode: "new".into(),
            agent,
            session_id: None,
            workstream_ids: workstream_ids.to_vec(),
            extra_workstream_ids: vec![],
            cwd: effective_cwd,
            delivery_level,
            bundle,
            runtime,
            state_fingerprint,
            prepared_at: now(),
        })
    }

    /// Prepare Resume Session:
    /// Syncs session's own messages, computes effective workstream list (existing + extra),
    /// builds the delta context bundle against last delivered revisions, and captures fingerprint.
    ///
    /// INVARIANT: Zero premature side effects. Does NOT commit extra workstream bindings,
    /// does NOT write context files, and does NOT advance delivery snapshots.
    pub fn prepare_resume(
        &self,
        db: &Db,
        session_id: &str,
        extra_workstream_ids: &[String],
    ) -> Result<PreparedLaunch> {
        let session = db
            .get_session(session_id)?
            .ok_or_else(|| other("Session 不存在"))?;

        let engine = crate::sync::SyncEngine::from_settings(db);
        sync_one_session_with_engine(db, &engine, &session)?;

        let mut ws_ids: Vec<String> = db
            .bindings_for_session(session_id)?
            .into_iter()
            .map(|b| b.workstream_id)
            .collect();
        for extra in extra_workstream_ids {
            if !ws_ids.contains(extra) {
                ws_ids.push(extra.clone());
            }
        }

        let delivery_level = crate::settings::context_delivery_level_of(db)?;
        let runtime = crate::agent_runtime::runtime_overrides_for_launch(db, session.agent)?;
        let bundle =
            crate::context::build_bundle(db, "resume", Some(&session), &ws_ids, delivery_level)?;

        let state_fingerprint = compute_state_fingerprint(
            db,
            "resume",
            Some(session_id),
            &ws_ids,
            delivery_level,
            session.agent,
        )?;

        Ok(PreparedLaunch {
            id: new_id(),
            mode: "resume".into(),
            agent: session.agent,
            session_id: Some(session_id.to_string()),
            workstream_ids: ws_ids,
            extra_workstream_ids: extra_workstream_ids.to_vec(),
            cwd: session.cwd.clone(),
            delivery_level,
            bundle,
            runtime,
            state_fingerprint,
            prepared_at: now(),
        })
    }

    /// Launch a previously prepared launch.
    ///
    /// INVARIANT:
    /// 1. Verifies state fingerprint matches current DB state. If context,
    ///    delivery level or Runtime override intent changed since preview,
    ///    aborts with a stale error.
    /// 2. Identity preservation: Writes the EXACT prepared bundle markdown
    ///    (NO re-building) and passes the EXACT prepared runtime overrides
    ///    (NO re-reading Settings).
    /// 3. Commits LaunchIntent (new) or extra bindings & cumulative delivery snapshots (resume)
    ///    only upon actual launch.
    pub fn launch_prepared(&self, db: &Db, prepared: &PreparedLaunch) -> Result<LaunchResult> {
        self.launch_prepared_with(db, prepared, crate::platform::launcher::launch)
    }

    /// `launch_prepared` with the *process spawn* step injected.
    ///
    /// The injected function replaces ONLY the OS call that opens a terminal
    /// and runs the Agent CLI. Every integrity gate stays on the same path in
    /// the same order: delivery-level check, state fingerprint (Preview-Launch
    /// Identity), context-file gating, single-use capability consumption by the
    /// command layer, LaunchIntent commit and delivery snapshots.
    ///
    /// Exists so integration tests can drive the whole
    /// prepare → launch_prepared chain without opening a real Terminal window
    /// on the developer's machine. Production callers use `launch_prepared`.
    pub fn launch_prepared_with(
        &self,
        db: &Db,
        prepared: &PreparedLaunch,
        spawn: fn(&AgentCommand) -> Result<crate::platform::launcher::LaunchOutcome>,
    ) -> Result<LaunchResult> {
        let current_delivery_level = crate::settings::context_delivery_level_of(db)?;
        if current_delivery_level != prepared.delivery_level {
            return Err(other(
                "Prepared launch is stale: context delivery level changed since preview. Please refresh preview.",
            ));
        }
        let current_fingerprint = compute_state_fingerprint(
            db,
            &prepared.mode,
            prepared.session_id.as_deref(),
            &prepared.workstream_ids,
            current_delivery_level,
            prepared.agent,
        )?;
        if current_fingerprint != prepared.state_fingerprint {
            return Err(other(
                "Prepared launch is stale: context or runtime state changed since preview. Please refresh preview.",
            ));
        }

        // Preview = Launch: the argv comes from the frozen intent, not from
        // whatever Settings holds right now.
        let runtime_opts = prepared.runtime.exec_options();

        let ctx_file = if prepared.workstream_ids.is_empty()
            || prepared.delivery_level == crate::context::ContextDeliveryLevel::Off
        {
            None
        } else {
            let title_hint = prepared
                .workstream_ids
                .first()
                .and_then(|id| db.get_workstream(id).ok().flatten())
                .map(|w| w.title)
                .unwrap_or_else(|| "bundle".into());
            Some(self.write_context_file(&title_hint, &prepared.bundle.markdown)?)
        };

        if prepared.mode == "new" {
            let (delivered_by_ws, delivered_confs_by_ws) = if ctx_file.is_some() {
                delivered_revisions_and_conflicts_by_workstream(
                    &prepared.bundle,
                    prepared
                        .workstream_ids
                        .first()
                        .map(|s| s.as_str())
                        .unwrap_or(""),
                )
            } else {
                Default::default()
            };
            let intent = LaunchIntent {
                id: new_id(),
                launch_type: "new".into(),
                agent: prepared.agent,
                selected_workstream_ids: prepared.workstream_ids.clone(),
                cwd: prepared.cwd.clone(),
                context_bundle_markdown: if ctx_file.is_some() {
                    Some(prepared.bundle.markdown.clone())
                } else {
                    None
                },
                context_bundle_revisions: if ctx_file.is_some() {
                    Some(
                        serde_json::json!({
                            "bundle_id": prepared.bundle.bundle_id,
                            "by_workstream": delivered_by_ws,
                            "conflicts_by_workstream": delivered_confs_by_ws,
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

            let install = resolve_install(db, prepared.agent)?;
            let adapter = crate::adapters::adapter_for(prepared.agent);
            let cwd_path = prepared.cwd.as_deref().map(PathBuf::from);

            let cmd: AgentCommand = adapter.build_new_command(
                &install,
                &runtime_opts,
                ctx_file.as_deref(),
                cwd_path.as_deref(),
            )?;
            let outcome = spawn(&cmd)?;

            db.update_launch_intent(
                &intent.id,
                launch_status::PENDING,
                None,
                &format!(
                    "launched_via={};pid={:?};runtime={}",
                    outcome.launched_via,
                    outcome.pid,
                    prepared.runtime.intent_summary()
                ),
            )?;

            Ok(LaunchResult {
                launched_via: outcome.launched_via,
                command_line: outcome.command_line,
                context_file: ctx_file
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default(),
                bundle: prepared.bundle.clone(),
                note: if ctx_file.is_some() {
                    "新 Session 启动后会在发现时通过 LaunchIntent 自动建立显式 Workstream 绑定。"
                        .into()
                } else if prepared.workstream_ids.is_empty() {
                    "已直接启动（未携带 Workstream Context）；此启动未选择 Workstream，允许 0 绑定。"
                        .into()
                } else {
                    "已关联 Workstream；Context Delivery 已关闭，本次未注入 Context。".into()
                },
                launch_intent_id: Some(intent.id),
            })
        } else {
            let session_id = prepared
                .session_id
                .as_deref()
                .ok_or_else(|| other("Prepared resume launch missing session_id"))?;
            let session = db
                .get_session(session_id)?
                .ok_or_else(|| other("Session 不存在"))?;

            let install = resolve_install(db, session.agent)?;
            let adapter = crate::adapters::adapter_for(session.agent);
            let cwd_path = session.cwd.clone().map(PathBuf::from);
            let cmd = adapter.build_resume_command(
                &install,
                &runtime_opts,
                &session.agent_session_id,
                ctx_file.as_deref(),
                cwd_path.as_deref(),
            )?;
            let outcome = spawn(&cmd)?;

            for extra in &prepared.extra_workstream_ids {
                record_binding(
                    db,
                    session_id,
                    extra,
                    "related",
                    binding_source::USER_ASSIGNED,
                    1.0,
                )?;
            }

            let prev_deliveries = db.latest_deliveries(session_id)?;
            for ws_id in &prepared.workstream_ids {
                record_binding(
                    db,
                    session_id,
                    ws_id,
                    "related",
                    binding_source::USER_ASSIGNED,
                    1.0,
                )?;
                if ctx_file.is_some() {
                    let prev = prev_deliveries.iter().find(|d| &d.workstream_id == ws_id);
                    let delivery = compute_cumulative_delivery(
                        db,
                        session_id,
                        ws_id,
                        &prepared.bundle,
                        prev,
                        prepared
                            .workstream_ids
                            .first()
                            .map(|s| s.as_str())
                            .unwrap_or(""),
                    )?;
                    db.record_delivery(&delivery)?;
                }
            }

            Ok(LaunchResult {
                launched_via: outcome.launched_via,
                command_line: outcome.command_line,
                context_file: ctx_file
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default(),
                bundle: prepared.bundle.clone(),
                note: if ctx_file.is_some() {
                    "已同步最新消息并生成增量上下文（基于上次实际交付的修订快照）。".into()
                } else if prepared.workstream_ids.is_empty() {
                    "该 Session 未关联 Workstream：已同步自身消息后直接恢复，未注入上下文。".into()
                } else {
                    "已关联 Workstream；Context Delivery 已关闭，本次未注入 Context。".into()
                },
                launch_intent_id: None,
            })
        }
    }

    pub fn new_session(
        &self,
        db: &Db,
        agent: Agent,
        workstream_ids: &[String],
        cwd: Option<&str>,
    ) -> Result<LaunchResult> {
        let prepared = self.prepare_new(db, agent, workstream_ids, cwd)?;
        self.launch_prepared(db, &prepared)
    }

    pub fn resume_session(
        &self,
        db: &Db,
        session_id: &str,
        extra_workstream_ids: &[String],
    ) -> Result<LaunchResult> {
        let prepared = self.prepare_resume(db, session_id, extra_workstream_ids)?;
        self.launch_prepared(db, &prepared)
    }
}

/// Compute a deterministic SHA-256 fingerprint representing the exact context inputs
/// and backing DB state (workstream metadata, active context items & current revisions,
/// conflicts, the Agent's runtime override intent, and for resume mode: session
/// cursor, bindings, and delivery snapshots).
pub fn compute_state_fingerprint(
    db: &Db,
    mode: &str,
    session_id: Option<&str>,
    effective_workstream_ids: &[String],
    delivery_level: ContextDeliveryLevel,
    agent: Agent,
) -> Result<String> {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(mode.as_bytes());
    hasher.update(b":");
    hasher.update(delivery_level.as_str().as_bytes());
    hasher.update(b":");

    // Preview-Launch Identity also covers what NoEnding will pass to the CLI:
    // an override edited after Preview must abort the launch, never be picked
    // up silently. Only the intent is hashed — NoEnding has no resolved
    // default to hash.
    let runtime = crate::agent_runtime::runtime_overrides_for_launch(db, agent)?;
    hasher.update(b"runtime:");
    hasher.update(agent.as_str().as_bytes());
    hasher.update(b"=");
    hasher.update(runtime.intent_summary().as_bytes());
    hasher.update(b":");

    for ws_id in effective_workstream_ids {
        hasher.update(ws_id.as_bytes());
        hasher.update(b":");
        if let Some(ws) = db.get_workstream(ws_id)? {
            hasher.update(b"exists:1:");
            hasher.update(ws.title.as_bytes());
            hasher.update(ws.description.as_bytes());
            hasher.update(ws.updated_at.as_bytes());
        } else {
            hasher.update(b"exists:0:");
        }
        let mut items = db.items_for_workstream(ws_id, true)?;
        items.sort_by(|a, b| a.0.id.cmp(&b.0.id));
        for (item, rev) in items {
            hasher.update(item.id.as_bytes());
            hasher.update(item.status.as_bytes());
            if let Some(rid) = &item.current_revision_id {
                hasher.update(rid.as_bytes());
            }
            hasher.update(rev.id.as_bytes());
            hasher.update(rev.title.as_bytes());
            hasher.update(rev.content.as_bytes());
            hasher.update(rev.created_at.as_bytes());
        }
        let mut confs = db.conflicts_for_workstream(ws_id, true)?;
        confs.sort_by(|a, b| a.id.cmp(&b.id));
        for c in confs {
            hasher.update(c.id.as_bytes());
            hasher.update(c.status.as_bytes());
            if let Some(res) = &c.resolution {
                hasher.update(res.as_bytes());
            }
            hasher.update(c.updated_at.as_bytes());
        }
    }

    if mode == "resume" {
        if let Some(sid) = session_id {
            hasher.update(sid.as_bytes());
            hasher.update(b":");
            if let Some(s) = db.get_session(sid)? {
                hasher.update(b"session_exists:1:");
                if let Some(la) = &s.last_activity_at {
                    hasher.update(la.as_bytes());
                }
                if let Ok(cursor) = db.get_source_cursor(sid) {
                    hasher.update(&cursor.last_sequence.to_le_bytes());
                    hasher.update(&cursor.byte_offset.to_le_bytes());
                    hasher.update(cursor.identity_tail_hash.as_bytes());
                }
            } else {
                hasher.update(b"session_exists:0:");
            }
            let mut bindings = db.bindings_for_session(sid)?;
            bindings.sort_by(|a, b| a.workstream_id.cmp(&b.workstream_id));
            for b in bindings {
                hasher.update(b.workstream_id.as_bytes());
                hasher.update(b.role.as_bytes());
                hasher.update(b.source.as_bytes());
            }
            let mut deliveries = db.latest_deliveries(sid)?;
            deliveries.sort_by(|a, b| a.workstream_id.cmp(&b.workstream_id));
            for d in deliveries {
                hasher.update(d.workstream_id.as_bytes());
                hasher.update(d.bundle_id.as_bytes());
                for rev in &d.delivered_revisions {
                    hasher.update(rev.as_bytes());
                }
                for conf in &d.delivered_conflicts {
                    hasher.update(conf.as_bytes());
                }
                hasher.update(d.delivered_at.as_bytes());
            }
        }
    }

    let hash = hasher.finalize();
    Ok(format!("{:x}", hash))
}

/// Group the bundle's delivered revision ids and conflict ids by the workstream that owns
/// each section. Sections without a workstream attribution (rare) fall back to `fallback_ws`.
pub fn delivered_revisions_and_conflicts_by_workstream(
    bundle: &crate::context::SessionContextBundle,
    fallback_ws: &str,
) -> (
    std::collections::BTreeMap<String, Vec<String>>,
    std::collections::BTreeMap<String, Vec<String>>,
) {
    let mut revs_by_ws: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    let mut confs_by_ws: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for s in &bundle.sections {
        let ws = s
            .workstream_id
            .clone()
            .unwrap_or_else(|| fallback_ws.to_string());
        if let Some(rev) = &s.revision_id {
            let entry = revs_by_ws.entry(ws.clone()).or_default();
            if !entry.contains(rev) {
                entry.push(rev.clone());
            }
        }
        if let Some(cid) = &s.conflict_id {
            let entry = confs_by_ws.entry(ws).or_default();
            if !entry.contains(cid) {
                entry.push(cid.clone());
            }
        }
    }
    (revs_by_ws, confs_by_ws)
}

/// Compute cumulative delivery snapshot (Agent-Known State) for a workstream.
/// Resume context represents Current − AgentKnownState.
/// Therefore, the delivery snapshot must accumulate everything the agent knows,
/// rather than just what was sent in the most recent single message.
pub fn compute_cumulative_delivery(
    db: &Db,
    session_id: &str,
    workstream_id: &str,
    bundle: &crate::context::SessionContextBundle,
    prev_delivery: Option<&ContextDelivery>,
    fallback_ws: &str,
) -> Result<ContextDelivery> {
    let ws_sections: Vec<&crate::context::ContextSection> = bundle
        .sections
        .iter()
        .filter(|s| s.workstream_id.as_deref().unwrap_or(fallback_ws) == workstream_id)
        .collect();

    match prev_delivery {
        None => {
            let mut revisions = Vec::new();
            let mut conflicts = Vec::new();
            for s in ws_sections {
                if let Some(rev) = &s.revision_id {
                    if !revisions.contains(rev) {
                        revisions.push(rev.clone());
                    }
                }
                if let Some(cid) = &s.conflict_id {
                    if !conflicts.contains(cid) {
                        conflicts.push(cid.clone());
                    }
                }
            }
            Ok(ContextDelivery {
                id: new_id(),
                session_id: session_id.to_string(),
                workstream_id: workstream_id.to_string(),
                bundle_id: bundle.bundle_id.clone(),
                delivered_revisions: revisions,
                delivered_conflicts: conflicts,
                delivered_at: now(),
            })
        }
        Some(prev) => {
            let mut known_revisions = prev.delivered_revisions.clone();
            let mut known_conflicts = prev.delivered_conflicts.clone();

            for s in ws_sections {
                match s.kind.as_str() {
                    "delta" => {
                        if let Some(new_rev_id) = &s.revision_id {
                            if let Some(new_rev) = db.get_revision(new_rev_id)? {
                                // Remove any older revision of the same item
                                known_revisions.retain(|rev_id| {
                                    if let Ok(Some(r)) = db.get_revision(rev_id) {
                                        r.item_id != new_rev.item_id
                                    } else {
                                        true
                                    }
                                });
                            }
                            if !known_revisions.contains(new_rev_id) {
                                known_revisions.push(new_rev_id.clone());
                            }
                        }
                    }
                    "gone" => {
                        if let Some(gone_rev_id) = &s.revision_id {
                            known_revisions.retain(|r| r != gone_rev_id);
                        }
                    }
                    "conflict" => {
                        if let Some(cid) = &s.conflict_id {
                            if !known_conflicts.contains(cid) {
                                known_conflicts.push(cid.clone());
                            }
                        }
                    }
                    "conflict_resolved" => {
                        if let Some(cid) = &s.conflict_id {
                            known_conflicts.retain(|c| c != cid);
                        }
                    }
                    _ => {}
                }
            }

            Ok(ContextDelivery {
                id: new_id(),
                session_id: session_id.to_string(),
                workstream_id: workstream_id.to_string(),
                bundle_id: bundle.bundle_id.clone(),
                delivered_revisions: known_revisions,
                delivered_conflicts: known_conflicts,
                delivered_at: now(),
            })
        }
    }
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
///
/// It defers to the workspace layer because in v0.2 a binding is more than a
/// row: an explicit launch must also ensure the WorkstreamPath its Session
/// started in (§1.8). Whether a provenance may grow a Workstream's list is
/// decided there — an automatic binding never does (§42.3-M2).
pub fn record_binding(
    db: &Db,
    session_id: &str,
    workstream_id: &str,
    role: &str,
    source: &str,
    confidence: f64,
) -> Result<()> {
    crate::workspace::session::record_user_binding(
        db,
        session_id,
        workstream_id,
        role,
        source,
        confidence,
    )
}

/// Expand a leading `~` / `~/` / `~\` to the user's home directory.
///
/// Users type `~/projects/x` into a working-directory field, and the rendered
/// terminal script must receive an absolute path — POSIX single quotes and
/// PowerShell `-LiteralPath` both treat `~` literally, so an unexpanded value
/// silently cds to $HOME. `~user` is intentionally unsupported.
///
/// A thin alias on purpose: the rules live in `workspace::identity`, which is
/// the only tilde expander in the crate (§42.3-M23). Two expanders means two
/// answers for one path.
pub fn expand_tilde(p: &str) -> String {
    crate::workspace::expand_tilde(p)
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

/// Unchanged rows are kept verbatim (provenance, created_at, cursors and
/// last_used_at survive — a metadata edit is not a "use"). A role edit on an
/// AUTOMATIC binding upgrades it to user_assigned, because sync replaces AUTO
/// rows wholesale and the user's role choice would otherwise be silently undone
/// by the next classification. Every removed row — any provenance — leaves a
/// durable removal tombstone.
///
/// The diff itself lives in the workspace layer since v0.2: a row the user just
/// added also ensures the WorkstreamPath its Session started in and records
/// which one (§1.8). Two engines — one growing path lists, one not — is the
/// split authority 方案 §29 forbids, so this is a typed entry point into
/// `crate::workspace::session::replace_session_bindings`.
pub fn replace_session_bindings(
    db: &Db,
    session_id: &str,
    desired: &[(String, String)], // (workstream_id, role)
) -> Result<()> {
    let desired: Vec<crate::workspace::session::DesiredBinding> = desired
        .iter()
        .map(
            |(workstream_id, role)| crate::workspace::session::DesiredBinding {
                workstream_id: workstream_id.clone(),
                role: role.clone(),
                // Which path brought it in is derived from the Session's own
                // WorkspacePath, never accepted from a caller (§42.3-M1).
                workstream_path_id: None,
            },
        )
        .collect();
    crate::workspace::session::replace_session_bindings(db, session_id, &desired)
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
            let by_ws = v.get("by_workstream").and_then(|m| m.as_object());
            let confs_by_ws = v.get("conflicts_by_workstream").and_then(|m| m.as_object());

            let mut all_ws_ids = std::collections::BTreeSet::new();
            if let Some(m) = by_ws {
                all_ws_ids.extend(m.keys().cloned());
            }
            if let Some(m) = confs_by_ws {
                all_ws_ids.extend(m.keys().cloned());
            }

            for ws_id in all_ws_ids {
                let revisions: Vec<String> = by_ws
                    .and_then(|m| m.get(&ws_id))
                    .and_then(|a| a.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let conflicts: Vec<String> = confs_by_ws
                    .and_then(|c| c.get(&ws_id))
                    .and_then(|a| a.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                db.record_delivery(&ContextDelivery {
                    id: new_id(),
                    session_id: session.id.clone(),
                    workstream_id: ws_id,
                    bundle_id: bundle_id.clone(),
                    delivered_revisions: revisions,
                    delivered_conflicts: conflicts,
                    delivered_at: now(),
                })?;
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
///
/// With Context Intelligence off, step 2 never starts — launch flows still
/// observe freshly ingested events (so cwd / staleness / fingerprints stay
/// true) while `processed_sequence` is left untouched for a later replay.
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
    if !crate::settings::context_intelligence_enabled(db)? {
        return Ok((stored.len() as i64, 0));
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
