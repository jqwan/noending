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
//!
//! Working directory (方案 §13, Workspace Domain v0.2): every launch resolves
//! its directory through `resolve_new_cwd` / `resolve_resume_cwd`, which read
//! the ordered `WorkstreamPath` list and NoEnding Home's default workspace. The
//! answer, including *which tier*
//! produced it, is carried on the PreparedLaunch and hashed into its state
//! fingerprint, so Preview-Launch Identity covers the directory as well as the
//! context (§42.3-M16/M17).

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

/// How a prepared launch decided the directory the Agent will start in
/// (方案 §13). This is a *user-visible* fact, not an implementation detail:
/// every tier below the one the flow normally uses has to be sayable out loud
/// in the UI (§21-4), and it is part of the state fingerprint, so a tier that
/// changes between Preview and Launch aborts the launch instead of quietly
/// moving the Agent somewhere else (§42.3-M16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CwdSource {
    /// The caller (the New Session form) named a directory.
    Explicit,
    /// Resume: the Session's own recorded cwd, which stays authoritative
    /// whenever it is a real directory (§1.5 "Sessions keep their own cwd").
    SessionCwd,
    /// A selected Workstream's ordered `WorkstreamPath` list (§1.5). Position 0
    /// is primary; `path_position` says which entry was actually used.
    WorkstreamPath,
    /// NoEnding Home's default workspace (`<home>/workspace`).
    DefaultWorkspace,
    /// Nothing resolved: the Agent starts in the terminal's own default
    /// directory. Honest, never implied.
    #[default]
    Unresolved,
}

impl CwdSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::SessionCwd => "session_cwd",
            Self::WorkstreamPath => "workstream_path",
            Self::DefaultWorkspace => "default_workspace",
            Self::Unresolved => "unresolved",
        }
    }
}

/// The resolved launch directory plus the reason it is that one.
///
/// Produced by resolution, stored on [`PreparedLaunch`] for the UI, and fed to
/// the fingerprint. Two resolutions are equal iff every hashed field is equal,
/// so `note` — prose for the user — is deliberately NOT hashed: it is derived
/// from the hashed fields and can be reworded without invalidating previews.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CwdResolution {
    pub source: CwdSource,
    pub cwd: Option<String>,
    /// True when the Agent will not start where this flow normally starts:
    /// a resume that lost its Session directory, or a New Session that could
    /// not use the Workstream's own primary path.
    pub fallback: bool,
    /// Which Workstream supplied the directory, when one did.
    pub workstream_id: Option<String>,
    /// Its position in that Workstream's ordered path list (0 = primary).
    pub path_position: Option<i64>,
    /// Why a fallback happened, for the UI (§13 "发生 fallback 必须在 UI 明确显示").
    pub note: Option<String>,
}

impl CwdResolution {
    fn explicit(cwd: String) -> Self {
        // The user named this directory, so it is launched in even when it is
        // not there yet — silently substituting another tier would override
        // explicit intent. It is *annotated*, because macOS would cd to $HOME
        // after a failed `cd` (§42.3-M21), and the user deserves to know that.
        let note = if is_usable_directory(&cwd) {
            None
        } else {
            Some("指定的目录当前不是一个可用的目录，Agent 可能落在终端默认目录".into())
        };
        Self {
            source: CwdSource::Explicit,
            cwd: Some(cwd),
            note,
            ..Default::default()
        }
    }

    /// §42.3-M17 — labelled, delimited hash bytes: `["/a","/b"]` can never
    /// collide with a concatenation of one field's value with another's.
    fn fingerprint_input(&self) -> Vec<u8> {
        let mut out = b"cwd:".to_vec();
        if let Some(cwd) = &self.cwd {
            out.extend_from_slice(cwd.as_bytes());
        }
        out.push(b'|');
        out.extend_from_slice(b"cwd_src:");
        out.extend_from_slice(self.source.as_str().as_bytes());
        out.push(b'|');
        out.extend_from_slice(b"cwd_ws:");
        if let Some(ws) = &self.workstream_id {
            out.extend_from_slice(ws.as_bytes());
        }
        out.push(b'|');
        out.extend_from_slice(b"cwd_pos:");
        if let Some(pos) = self.path_position {
            out.extend_from_slice(pos.to_string().as_bytes());
        }
        out.push(b'|');
        out.extend_from_slice(if self.fallback {
            b"cwd_fb:1|"
        } else {
            b"cwd_fb:0|"
        });
        out
    }
}

/// The non-database facts a launch needs.
///
/// Today that is exactly one thing: NoEnding Home's default workspace, which is
/// a *filesystem/Home* fact (§2) — no DB row holds it, so a DB-only fingerprint
/// can never notice it moving. Production passes one built from the managed
/// [`crate::workspace::home::NoEndingHome`]; tests pass literals, which is what
/// keeps §42.3-M13 ("no test may resolve the real Home") true here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchWorkspace {
    /// `<home>/workspace`. `None` means "not known to this launcher", which
    /// removes tier 3 from the chain rather than guessing at a directory.
    pub default_workspace: Option<String>,
}

impl LaunchWorkspace {
    /// Production shape: the Home's default workspace, as a string.
    pub fn from_home(home: &crate::workspace::home::NoEndingHome) -> Self {
        Self {
            default_workspace: Some(home.default_workspace_str()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparedLaunch {
    pub id: String,
    pub mode: String, // "new" | "resume"
    pub agent: Agent,
    pub session_id: Option<String>,
    pub workstream_ids: Vec<String>,
    pub extra_workstream_ids: Vec<String>,
    /// The directory the Agent WILL start in — authoritative. Since §42.3-M16
    /// the spawn step uses this value and nothing else: `launch_prepared` no
    /// longer re-reads `sessions.cwd`, because re-reading let background
    /// discovery move a launch between Preview and Launch.
    pub cwd: Option<String>,
    /// Which of §13's tiers produced `cwd`, so the UI can name it and a
    /// downgrade can never be silent (§21-4). Part of `state_fingerprint`.
    #[serde(default)]
    pub cwd_resolution: CwdResolution,
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
    ///
    /// §21-1/2 — Prepare New Session against an explicit [`LaunchWorkspace`].
    ///
    /// `cwd` is the caller's explicit directory: when present it IS the launch
    /// directory (§13 tier 1) and nothing below it is consulted. Otherwise the
    /// selected Workstreams' ordered paths decide, then the default workspace.
    pub fn prepare_new_in(
        &self,
        db: &Db,
        agent: Agent,
        workstream_ids: &[String],
        cwd: Option<&str>,
        workspace: &LaunchWorkspace,
    ) -> Result<PreparedLaunch> {
        self.sync_stale_for_workstreams(db, workstream_ids)?;

        let resolution = resolve_new_cwd(db, workstream_ids, cwd, workspace)?;

        let delivery_level = crate::settings::context_delivery_level_of(db)?;
        let runtime = crate::agent_runtime::runtime_overrides_for_launch(db, agent)?;
        let bundle = crate::context::build_bundle(db, "new", None, workstream_ids, delivery_level)?;

        let state_fingerprint = compute_state_fingerprint_in(
            db,
            "new",
            None,
            workstream_ids,
            delivery_level,
            agent,
            workspace,
            &resolution,
        )?;

        Ok(PreparedLaunch {
            id: new_id(),
            mode: "new".into(),
            agent,
            session_id: None,
            workstream_ids: workstream_ids.to_vec(),
            extra_workstream_ids: vec![],
            cwd: resolution.cwd.clone(),
            cwd_resolution: resolution,
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
    /// §21-3 — Prepare Resume against an explicit [`LaunchWorkspace`].
    ///
    /// The Session's own cwd wins while it is a real directory; when it is gone
    /// the launch falls back to a bound Workstream's primary path and then to
    /// the default workspace, and the fallback is recorded in
    /// `cwd_resolution` instead of being absorbed (§13, §42.3-M16).
    pub fn prepare_resume_in(
        &self,
        db: &Db,
        session_id: &str,
        extra_workstream_ids: &[String],
        workspace: &LaunchWorkspace,
    ) -> Result<PreparedLaunch> {
        let session = db
            .get_session(session_id)?
            .ok_or_else(|| other("Session 不存在"))?;
        // §10 — a trashed session is inactive and must not resume. All resume
        // entries (command layer, Assistant, legacy one-shot) funnel through
        // here, so this is the single prepare-side gate.
        if session.is_trashed() {
            return Err(other("会话已在回收站，无法继续；请先恢复会话"));
        }

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

        let resolution = resolve_resume_cwd(db, session_id, &ws_ids, workspace)?;

        let delivery_level = crate::settings::context_delivery_level_of(db)?;
        let runtime = crate::agent_runtime::runtime_overrides_for_launch(db, session.agent)?;
        let bundle =
            crate::context::build_bundle(db, "resume", Some(&session), &ws_ids, delivery_level)?;

        let state_fingerprint = compute_state_fingerprint_in(
            db,
            "resume",
            Some(session_id),
            &ws_ids,
            delivery_level,
            session.agent,
            workspace,
            &resolution,
        )?;

        Ok(PreparedLaunch {
            id: new_id(),
            mode: "resume".into(),
            agent: session.agent,
            session_id: Some(session_id.to_string()),
            workstream_ids: ws_ids,
            extra_workstream_ids: extra_workstream_ids.to_vec(),
            cwd: resolution.cwd.clone(),
            cwd_resolution: resolution,
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
    ///    working-directory inputs, delivery level or Runtime override intent
    ///    changed since preview, aborts with a stale error.
    /// 2. Identity preservation: Writes the EXACT prepared bundle markdown
    ///    (NO re-building), passes the EXACT prepared runtime overrides (NO
    ///    re-reading Settings) and starts the Agent in `prepared.cwd` (NO
    ///    re-resolving — §42.3-M16).
    /// 3. Commits LaunchIntent (new) or extra bindings & cumulative delivery snapshots (resume)
    ///    only upon actual launch.
    /// `launch_prepared` with the current [`LaunchWorkspace`].
    ///
    /// The default workspace is a Home fact the fingerprint has to re-read from
    /// *now*: if the Home moved after Preview, the prepared launch must abort,
    /// not start the Agent in a directory the user never saw (§12, §42.3-M17).
    pub fn launch_prepared_in(
        &self,
        db: &Db,
        prepared: &PreparedLaunch,
        workspace: &LaunchWorkspace,
    ) -> Result<LaunchResult> {
        self.launch_prepared_with_in(db, prepared, workspace, crate::platform::launcher::launch)
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
    /// The one real launch path: prepared launch + current workspace +
    /// injectable spawn. See [`Self::launch_prepared_with`] for why the spawn
    /// is a parameter.
    pub fn launch_prepared_with_in(
        &self,
        db: &Db,
        prepared: &PreparedLaunch,
        workspace: &LaunchWorkspace,
        spawn: fn(&AgentCommand) -> Result<crate::platform::launcher::LaunchOutcome>,
    ) -> Result<LaunchResult> {
        let current_delivery_level = crate::settings::context_delivery_level_of(db)?;
        if current_delivery_level != prepared.delivery_level {
            return Err(other(
                "Prepared launch is stale: context delivery level changed since preview. Please refresh preview.",
            ));
        }
        // §42.3-M16/M17: re-resolve the launch directory from the state that
        // exists NOW (ordered Workstream paths, the Session's own cwd, the Home
        // default workspace) and hash it. A drift in any of those inputs lands
        // on a different fingerprint, so a plan whose directory moved is
        // refused here — the spawn below keeps using `prepared.cwd` verbatim.
        let resolution = recompute_launch_cwd(db, prepared, workspace)?;
        let current_fingerprint = compute_state_fingerprint_in(
            db,
            &prepared.mode,
            prepared.session_id.as_deref(),
            &prepared.workstream_ids,
            current_delivery_level,
            prepared.agent,
            workspace,
            &resolution,
        )?;
        if current_fingerprint != prepared.state_fingerprint {
            return Err(other(
                "Prepared launch is stale: context, working directory or runtime state changed since preview. Please refresh preview.",
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
            // §10 — belt and braces beside the fingerprint term: a trashed
            // session never launches, even if every other input managed to
            // match a stale-but-legal fingerprint.
            if session.is_trashed() {
                return Err(other(
                    "Prepared launch is stale: 会话已移入回收站，无法继续。请恢复会话后重新预览。",
                ));
            }

            let install = resolve_install(db, session.agent)?;
            let adapter = crate::adapters::adapter_for(session.agent);
            // §42.3-M16 — resume starts in the directory the Preview showed, not
            // in whatever `sessions.cwd` says right now. Reading it here was the
            // violation: discovery could rewrite it between Preview and Launch
            // and the Agent silently opened somewhere else.
            let cwd_path = prepared.cwd.as_deref().map(PathBuf::from);
            let cmd = adapter.build_resume_command(
                &install,
                &runtime_opts,
                &session.agent_session_id,
                ctx_file.as_deref(),
                cwd_path.as_deref(),
            )?;
            let outcome = spawn(&cmd)?;

            // §1.7: a launch that fell back to NoEnding's own default
            // workspace binds the Workstream but does not teach it that
            // directory — the user never chose it.
            let grow_path_list = prepared.cwd_resolution.source != CwdSource::DefaultWorkspace;

            for extra in &prepared.extra_workstream_ids {
                crate::workspace::session::record_user_binding_growing(
                    db,
                    session_id,
                    extra,
                    "related",
                    binding_source::USER_ASSIGNED,
                    1.0,
                    grow_path_list,
                )?;
            }

            let prev_deliveries = db.latest_deliveries(session_id)?;
            for ws_id in &prepared.workstream_ids {
                crate::workspace::session::record_user_binding_growing(
                    db,
                    session_id,
                    ws_id,
                    "related",
                    binding_source::USER_ASSIGNED,
                    1.0,
                    grow_path_list,
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

    pub fn new_session_in(
        &self,
        db: &Db,
        agent: Agent,
        workstream_ids: &[String],
        cwd: Option<&str>,
        workspace: &LaunchWorkspace,
    ) -> Result<LaunchResult> {
        let prepared = self.prepare_new_in(db, agent, workstream_ids, cwd, workspace)?;
        self.launch_prepared_in(db, &prepared, workspace)
    }

    pub fn resume_session_in(
        &self,
        db: &Db,
        session_id: &str,
        extra_workstream_ids: &[String],
        workspace: &LaunchWorkspace,
    ) -> Result<LaunchResult> {
        let prepared = self.prepare_resume_in(db, session_id, extra_workstream_ids, workspace)?;
        self.launch_prepared_in(db, &prepared, workspace)
    }
}

/// Recompute the fingerprint of a launch **from the database as it is now**,
/// resolving the §13 chain the same way `prepare_*` does.
///
/// This is the "has anything the user saw moved?" question. It takes the same
/// [`LaunchWorkspace`] the prepare step used: `None` in `default_workspace`
/// means "this launcher has no Home", which drops §13's third tier from the
/// chain, so the answer is a different statement than the one a real Home
/// produces (§42.3-M31).
///
/// A caller that already holds a `PreparedLaunch` — where the resolved tier is
/// a recorded fact, not something to re-derive — uses
/// [`compute_state_fingerprint_in`] with that same [`CwdResolution`].
pub fn compute_state_fingerprint(
    db: &Db,
    mode: &str,
    session_id: Option<&str>,
    effective_workstream_ids: &[String],
    delivery_level: ContextDeliveryLevel,
    agent: Agent,
    workspace: &LaunchWorkspace,
) -> Result<String> {
    let resolution = if mode == "resume" {
        match session_id {
            Some(sid) => resolve_resume_cwd(db, sid, effective_workstream_ids, workspace)?,
            None => CwdResolution::default(),
        }
    } else {
        resolve_new_cwd(db, effective_workstream_ids, None, workspace)?
    };
    compute_state_fingerprint_in(
        db,
        mode,
        session_id,
        effective_workstream_ids,
        delivery_level,
        agent,
        workspace,
        &resolution,
    )
}

/// §12 — the full fingerprint. Everything that decides what the Agent receives:
///
/// * the delivery level and the Agent runtime override intent;
/// * per selected Workstream: title / description / `updated_at`, its active
///   Context items with their current revisions, its open conflicts — and its
///   **ordered path list** (§12, §21-5..7);
/// * the resolved launch directory and the tier that produced it (§42.3-M16);
/// * NoEnding Home's default workspace, which is not in the DB at all
///   (§42.3-M17 note 4);
/// * in resume mode: whether the Session still exists, its `last_activity_at`,
///   its source cursor, its **`workspace_path_id`**, its bindings and its
///   delivery snapshots.
///
/// Every added input is labelled and terminated by `|` (§42.3-M17), so a value
/// cannot be re-read as a different field by shifting a boundary, and the path
/// list is hashed **in list order** because position 0 is the fact the launch
/// depends on. A WorkstreamPath mutation does not bump `workstreams.updated_at`
/// so without `ws_paths:` here a reorder, an added path or a removed primary
/// would leave a stale plan looking fresh.
pub fn compute_state_fingerprint_in(
    db: &Db,
    mode: &str,
    session_id: Option<&str>,
    effective_workstream_ids: &[String],
    delivery_level: ContextDeliveryLevel,
    agent: Agent,
    workspace: &LaunchWorkspace,
    resolution: &CwdResolution,
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
    hasher.update(b"|");

    // §13 tier 3: the Home's default workspace. It lives outside the database,
    // so a DB-only fingerprint could never see it move — which would let a
    // preview made under one Home launch under another.
    hasher.update(b"default_ws:");
    if let Some(dir) = &workspace.default_workspace {
        hasher.update(dir.as_bytes());
    }
    hasher.update(b"|");

    // The directory the Agent will actually start in, and why (§42.3-M16).
    hasher.update(resolution.fingerprint_input());

    for ws_id in effective_workstream_ids {
        hasher.update(ws_id.as_bytes());
        hasher.update(b":");
        if let Some(ws) = db.get_workstream(ws_id)? {
            hasher.update(b"exists:1:");
            hasher.update(ws.title.as_bytes());
            hasher.update(b"|");
            hasher.update(ws.description.as_bytes());
            hasher.update(b"|");
            hasher.update(ws.updated_at.as_bytes());
            hasher.update(b"|");
            // The ordered list itself is part of the fingerprint.
            hasher.update(
                crate::workspace::workstream::workstream_launch_paths(db, ws_id)?
                    .fingerprint_input(),
            );
        } else {
            hasher.update(b"exists:0:");
            hasher.update(b"|");
        }
        let mut items = db.items_for_workstream(ws_id, true)?;
        items.sort_by(|a, b| a.0.id.cmp(&b.0.id));
        for (item, rev) in items {
            hasher.update(item.id.as_bytes());
            hasher.update(b"|");
            hasher.update(item.status.as_bytes());
            hasher.update(b"|");
            if let Some(rid) = &item.current_revision_id {
                hasher.update(rid.as_bytes());
                hasher.update(b"|");
            }
            hasher.update(rev.id.as_bytes());
            hasher.update(b"|");
            hasher.update(rev.title.as_bytes());
            hasher.update(b"|");
            hasher.update(rev.content.as_bytes());
            hasher.update(b"|");
            hasher.update(rev.created_at.as_bytes());
            hasher.update(b"|");
        }
        let mut confs = db.conflicts_for_workstream(ws_id, true)?;
        confs.sort_by(|a, b| a.id.cmp(&b.id));
        for c in confs {
            hasher.update(c.id.as_bytes());
            hasher.update(b"|");
            hasher.update(c.status.as_bytes());
            hasher.update(b"|");
            if let Some(res) = &c.resolution {
                hasher.update(res.as_bytes());
                hasher.update(b"|");
            }
            hasher.update(c.updated_at.as_bytes());
            hasher.update(b"|");
        }
    }

    if mode == "resume" {
        if let Some(sid) = session_id {
            hasher.update(sid.as_bytes());
            hasher.update(b":");
            if let Some(s) = db.get_session(sid)? {
                hasher.update(b"session_exists:1:");
                // §10 — the lifecycle state is part of the launch state: a
                // Prepare → Trash → launch_prepared sequence must fail as
                // stale even when nothing else about the row moved.
                if s.is_trashed() {
                    hasher.update(b"session_trashed:1:");
                } else {
                    hasher.update(b"session_trashed:0:");
                }
                if let Some(la) = &s.last_activity_at {
                    hasher.update(la.as_bytes());
                    hasher.update(b"|");
                }
                // §12 "Session workspace path": the identity of the directory
                // this Session was observed in. A cwd string that now resolves
                // to a different WorkspacePath row (or to none) is a different
                // launch, even when the string itself survived.
                hasher.update(b"session_path:");
                if let Some(path_id) = &s.workspace_path_id {
                    hasher.update(path_id.as_bytes());
                }
                hasher.update(b"|");
                if let Ok(cursor) = db.get_source_cursor(sid) {
                    hasher.update(&cursor.last_sequence.to_le_bytes());
                    hasher.update(&cursor.byte_offset.to_le_bytes());
                    hasher.update(cursor.identity_tail_hash.as_bytes());
                    hasher.update(b"|");
                }
            } else {
                hasher.update(b"session_exists:0:");
                hasher.update(b"|");
            }
            let mut bindings = db.bindings_for_session(sid)?;
            bindings.sort_by(|a, b| a.workstream_id.cmp(&b.workstream_id));
            for b in bindings {
                hasher.update(b.workstream_id.as_bytes());
                hasher.update(b"|");
                hasher.update(b.role.as_bytes());
                hasher.update(b"|");
                hasher.update(b.source.as_bytes());
                hasher.update(b"|");
                // Which path brought the binding in is part of the claim
                // §1.6 removes paths by, so a re-claim is a state change too.
                hasher.update(b"claim:");
                if let Some(claim) = &b.workstream_path_id {
                    hasher.update(claim.as_bytes());
                }
                hasher.update(b"|");
            }
            let mut deliveries = db.latest_deliveries(sid)?;
            deliveries.sort_by(|a, b| a.workstream_id.cmp(&b.workstream_id));
            for d in deliveries {
                hasher.update(d.workstream_id.as_bytes());
                hasher.update(b"|");
                hasher.update(d.bundle_id.as_bytes());
                hasher.update(b"|");
                for rev in &d.delivered_revisions {
                    hasher.update(rev.as_bytes());
                    hasher.update(b"|");
                }
                for conf in &d.delivered_conflicts {
                    hasher.update(conf.as_bytes());
                    hasher.update(b"|");
                }
                hasher.update(d.delivered_at.as_bytes());
                hasher.update(b"|");
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

/// Is this recorded directory something an Agent can actually be started in?
///
/// An *observation*, never an identity (方案 §1.5, §42.3-M8): the path's
/// WorkspacePath row and its `canonical_path` stay exactly as they are whether
/// or not the volume is mounted. It matters here because of §42.3-M21 — handing
/// the terminal a directory that is not there does not fail loudly everywhere:
/// macOS prints a hint and cds to `$HOME` instead, and `$HOME` is the one place
/// §1.4 refuses to treat as a workspace. So an unusable tier is *skipped and
/// reported*, not launched into.
fn is_usable_directory(path: &str) -> bool {
    !path.trim().is_empty() && std::path::Path::new(path).is_dir()
}

/// §42.3-M21 — the default workspace is a directory NoEnding owns, so the
/// launcher may create it if Home bootstrap did not. This is the only
/// filesystem effect a launch resolution has, it is idempotent, and it creates
/// no domain fact: no row, no Revision, no binding, no delivery (§Launch
/// Preparation Integrity is about *storage*).
fn ensure_default_workspace(dir: &str) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    Ok(())
}

/// The `(cwd, source)` of §13's WorkstreamPath tier: walk the selected
/// Workstreams in the user's selection order, and inside each one its ordered
/// path list in position order, and return the first usable directory.
///
/// Position 0 wins whenever it can (that is what "primary" means); a later
/// position is only reached because the earlier ones are unusable, and it is
/// reported with its position so the UI can say so. Returns `None` plus a
/// count-worthy note when the Workstreams have paths but none of them are
/// usable.
fn first_workstream_path(
    db: &Db,
    workstream_ids: &[String],
) -> Result<(Option<CwdResolution>, Option<String>)> {
    let mut blocked: Option<String> = None;
    for ws_id in workstream_ids {
        let paths = crate::workspace::workstream::workstream_launch_paths(db, ws_id)?;
        for (position, raw) in paths.ordered_paths.iter().enumerate() {
            if !is_usable_directory(raw) {
                if blocked.is_none() {
                    blocked = Some(raw.clone());
                }
                continue;
            }
            return Ok((
                Some(CwdResolution {
                    source: CwdSource::WorkstreamPath,
                    cwd: Some(raw.clone()),
                    // Any entry after position 0 is a downgrade of the primary.
                    fallback: position > 0,
                    workstream_id: Some(ws_id.clone()),
                    path_position: Some(position as i64),
                    note: (position > 0).then(|| {
                        format!(
                            "主工作路径不可用，改用该 Workstream 的第 {} 条工作路径",
                            position + 1
                        )
                    }),
                }),
                blocked,
            ));
        }
    }
    Ok((None, blocked))
}

/// §13 tier 3 — the default workspace, or `None` when this launcher was not
/// given one.
fn default_workspace_resolution(workspace: &LaunchWorkspace) -> Result<Option<CwdResolution>> {
    let Some(dir) = workspace.default_workspace.as_deref().map(str::trim) else {
        return Ok(None);
    };
    if dir.is_empty() {
        return Ok(None);
    }
    let dir = expand_tilde(dir);
    ensure_default_workspace(&dir)?;
    Ok(Some(CwdResolution {
        source: CwdSource::DefaultWorkspace,
        cwd: Some(dir),
        ..Default::default()
    }))
}

/// §13 — New Session launch directory, in priority order:
///
/// ```text
/// explicit cwd  →  the selected Workstreams' ordered WorkstreamPaths
///                  (first usable, selection order then list order)
///              →  NoEnding Home's default workspace
/// ```
///
/// A Workstream's ordered path list is the recorded answer to "where does this
/// work happen"; an activity-derived Session cwd is never a launch tier.
///
/// A Workstream's path list is a launch convenience, never identity: the
/// Workstream is still not a path, and Sessions keep their own cwd.
pub fn resolve_new_cwd(
    db: &Db,
    workstream_ids: &[String],
    explicit: Option<&str>,
    workspace: &LaunchWorkspace,
) -> Result<CwdResolution> {
    if let Some(c) = explicit {
        return Ok(CwdResolution::explicit(expand_tilde(c)));
    }
    let (from_paths, blocked) = first_workstream_path(db, workstream_ids)?;
    if let Some(resolution) = from_paths {
        return Ok(resolution);
    }
    let standalone = workstream_ids.is_empty();
    if let Some(mut resolution) = default_workspace_resolution(workspace)? {
        // For a New Session *about a Workstream* the default workspace is a
        // downgrade: the user expected to work in the Workstream's directory.
        // A standalone Session has no expectation to downgrade from.
        resolution.fallback = !standalone;
        resolution.note = blocked.map(|lost| {
            format!(
                "Workstream 的工作路径 {} 当前不可用，改用 NoEnding 默认工作目录",
                lost
            )
        });
        return Ok(resolution);
    }
    Ok(CwdResolution {
        source: CwdSource::Unresolved,
        note: Some("没有可解析的工作目录，Agent 将在终端默认目录启动".into()),
        ..Default::default()
    })
}

/// §13 — Resume launch directory:
///
/// ```text
/// the Session's own cwd  →  a bound Workstream's primary path
///                        →  NoEnding Home's default workspace
/// ```
///
/// "Unavailable" here means missing, empty, or not a directory. Before v0.2 this
/// chain did not exist: a Session whose cwd had gone simply handed `None` to the
/// terminal, which on macOS means `$HOME` (§42.3-M21) — a silent, and per §1.4
/// forbidden, destination. Every step below the Session's own directory is
/// reported in `cwd_resolution` and hashed into the fingerprint.
///
/// A missing Session row resolves to nothing rather than failing: the caller's
/// fingerprint check reports the real news (the Session it previewed is gone)
/// as staleness, not as an error from a helper.
pub fn resolve_resume_cwd(
    db: &Db,
    session_id: &str,
    workstream_ids: &[String],
    workspace: &LaunchWorkspace,
) -> Result<CwdResolution> {
    let session = db.get_session(session_id)?;
    let Some(session) = session else {
        return Ok(CwdResolution {
            source: CwdSource::Unresolved,
            note: Some("Session 已不存在".into()),
            ..Default::default()
        });
    };
    if let Some(cwd) = session.cwd.as_deref() {
        if is_usable_directory(cwd) {
            return Ok(CwdResolution {
                source: CwdSource::SessionCwd,
                cwd: Some(cwd.to_string()),
                ..Default::default()
            });
        }
    }
    let lost = session
        .cwd
        .clone()
        .filter(|c| !c.trim().is_empty())
        .unwrap_or_else(|| "(未记录)".into());
    let (from_paths, blocked) = first_workstream_path(db, workstream_ids)?;
    if let Some(mut resolution) = from_paths {
        resolution.fallback = true;
        resolution.note = Some(format!(
            "原 Session 的工作目录 {} 当前不可用，改用其 Workstream 的工作路径",
            lost
        ));
        return Ok(resolution);
    }
    if let Some(mut resolution) = default_workspace_resolution(workspace)? {
        resolution.fallback = true;
        resolution.note = Some(match blocked {
            Some(path) => format!(
                "原 Session 的工作目录 {} 当前不可用，其 Workstream 的路径 {} 也不可用，改用 NoEnding 默认工作目录",
                lost, path
            ),
            None => format!(
                "原 Session 的工作目录 {} 当前不可用，改用 NoEnding 默认工作目录",
                lost
            ),
        });
        return Ok(resolution);
    }
    Ok(CwdResolution {
        source: CwdSource::Unresolved,
        fallback: true,
        note: Some(format!(
            "原 Session 的工作目录 {} 当前不可用，且没有任何可用的替代目录",
            lost
        )),
        ..Default::default()
    })
}

/// Re-resolve what a prepared launch resolved, from the state that exists at
/// launch time. This is the *staleness* side of §13; the Agent still starts in
/// `prepared.cwd` (see §42.3-M16), so a difference here is a refusal, never a
/// absorbed change.
fn recompute_launch_cwd(
    db: &Db,
    prepared: &PreparedLaunch,
    workspace: &LaunchWorkspace,
) -> Result<CwdResolution> {
    if prepared.mode != "new" {
        let Some(sid) = prepared.session_id.as_deref() else {
            return Ok(CwdResolution::default());
        };
        return resolve_resume_cwd(db, sid, &prepared.workstream_ids, workspace);
    }
    if prepared.cwd_resolution.source == CwdSource::Explicit {
        // A directory the user named in the form is a literal, not a derivation:
        // there is nothing in the database for it to drift against.
        return Ok(prepared.cwd_resolution.clone());
    }
    resolve_new_cwd(db, &prepared.workstream_ids, None, workspace)
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
///
/// Run against the real Home so §42.3-M15's shared-default-workspace rule can
/// be applied.
///
/// The matcher is told which directory every "no Workstream path" launch shares.
///
/// §42.3-M15: §13's third tier routes several independent launches into the
/// *same* cwd, and cwd was worth `+3.0` — enough on its own to look decisive.
/// A directory that many intents share carries no distinguishing evidence, so
/// against the default workspace it is worth `+1.0`, and a real cwd match still
/// outranks it. When scores are otherwise tied, the intent whose user *selected
/// Workstreams* wins: an explicit selection is a stronger statement than a
/// coincidental directory, and AGENTS.md puts user bindings above automatic
/// classification. Anything still tied stays AMBIGUOUS for a human — no
/// guessing.
pub fn try_match_launch_intents_in(
    db: &Db,
    session: &Session,
    workspace: &LaunchWorkspace,
) -> Result<bool> {
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
                    // §42.3-M15: the shared default workspace is not a
                    // discriminator, however exact the match looks.
                    score += if Some(ic) == workspace.default_workspace.as_ref() {
                        1.0
                    } else {
                        3.0
                    };
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
            apply_match(db, &intent, session, workspace)?;
            Ok(true)
        }
        _ => {
            let best_score = scored[0].1;
            let second = scored[1].1;
            let clear = best_score - second >= 2.0;
            // A tie is only a tie if nothing else distinguishes the candidates:
            // one intent with an explicit Workstream selection wins it.
            let tied = |score: f64| (score - best_score).abs() < f64::EPSILON;
            let selected: Vec<&LaunchIntent> = scored
                .iter()
                .filter(|(_, s)| tied(*s))
                .map(|(i, _)| i)
                .filter(|i| !i.selected_workstream_ids.is_empty())
                .collect();
            if clear {
                let (intent, _) = scored.remove(0);
                apply_match(db, &intent, session, workspace)?;
                Ok(true)
            } else if selected.len() == 1 {
                apply_match(db, selected[0], session, workspace)?;
                Ok(true)
            } else {
                // §42.3-M15: every tied top candidate is awaiting the user, not
                // just the first one — marking only one would hide the other
                // behind a PENDING state that says "still being discovered".
                let note = format!(
                    "多个候选 Session（score {:.1} vs {:.1}），等待用户确认",
                    best_score, second
                );
                for (intent, _) in scored.iter().filter(|(_, s)| tied(*s)) {
                    db.update_launch_intent(
                        &intent.id,
                        launch_status::AMBIGUOUS,
                        None,
                        &format!("{}（候选 {}）", note, intent.selected_workstream_ids.len()),
                    )?;
                }
                Ok(false)
            }
        }
    }
}

pub fn apply_match(
    db: &Db,
    intent: &LaunchIntent,
    session: &Session,
    workspace: &LaunchWorkspace,
) -> Result<()> {
    // §1.7: this Session may exist because a launch fell back to NoEnding's own
    // default workspace. The Workstream *binding* is the user's choice; the
    // directory was not, so it must not enter the Workstream's path list.
    let grow_path_list = match (&session.workspace_path_id, &workspace.default_workspace) {
        (Some(path), Some(default)) => {
            crate::workspace::path_identity_of(default).as_deref() != Some(path.as_str())
        }
        _ => true,
    };
    for ws_id in &intent.selected_workstream_ids {
        crate::workspace::session::record_user_binding_growing(
            db,
            &session.id,
            ws_id,
            "primary",
            binding_source::EXPLICIT_LAUNCH,
            1.0,
            grow_path_list,
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
    // §9 — same lifecycle re-read as the ingestion twin: the caller's struct
    // may predate a concurrent Trash. (The commit guards below are the
    // authoritative no-op; this just skips the doomed work up front.)
    if !db
        .get_session(&session.id)?
        .map(|s| !s.is_trashed())
        .unwrap_or(false)
    {
        return Ok((0, 0));
    }
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
