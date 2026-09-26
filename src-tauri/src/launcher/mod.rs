//! Session Launcher — New / Resume flows (技术实现).
//!
//! New Session: a durable LaunchIntent is created BEFORE the agent starts
//! (the external session id is unknowable at that point). When reconcile
//! later discovers the new external session, the intent is matched and the
//! user's chosen Workstream becomes the Session's single Owner.
//! Nothing else is written: no WorkstreamPath and no confidence.
//!
//! Resume: starts the Agent in the Session's own directory. Neither flow
//! synchronizes or injects Context: a launch carries only launch facts (Agent,
//! cwd, runtime, Owner, LaunchIntent). Context is consumed on demand from the
//! Session / Workstream pages.
//!
//! Working directory: every launch resolves
//! its directory through `resolve_new_cwd` / `resolve_resume_cwd`, which read
//! the ordered `WorkstreamPath` list and NoEnding Home's default workspace. The
//! answer, including *which tier*
//! produced it, is carried on the PreparedLaunch and hashed into its state
//! fingerprint, so Preview-Launch Identity covers the directory as well as the
//! runtime intent.

use std::path::PathBuf;

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::adapters::AgentCommand;
use crate::agent_runtime::AgentRuntimeOverrides;
use crate::domain::{launch_status, Agent, LaunchIntent, Session};
use crate::error::{other, Result};
use crate::storage::{new_id, now, Db};

/// LaunchIntent matching window: a discovered session may only claim an
/// intent launched within this range before the session started.
const MATCH_WINDOW_SECS: i64 = 6 * 3600;
const MATCH_CLOCK_SKEW_SECS: i64 = 120;
/// Pending intents older than this never match again.
const INTENT_TTL_SECS: i64 = 24 * 3600;

/// How a prepared launch decided the directory the Agent will start in
///. This is a *user-visible* fact, not an implementation detail:
/// every tier below the one the flow normally uses has to be sayable out loud
/// in the UI, and it is part of the state fingerprint, so a tier that
/// changes between Preview and Launch aborts the launch instead of quietly
/// moving the Agent somewhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CwdSource {
    /// The caller (the New Session form) named a directory.
    Explicit,
    /// Resume: the Session's own recorded cwd, which stays authoritative
    /// whenever it is a real directory ("Sessions keep their own cwd").
    SessionCwd,
    /// A selected Workstream's ordered `WorkstreamPath` list. Position 0
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
    /// Why a fallback happened, for the UI ("发生 fallback 必须在 UI 明确显示").
    pub note: Option<String>,
}

impl CwdResolution {
    fn explicit(cwd: String) -> Self {
        // The user named this directory, so it is launched in even when it is
        // not there yet — silently substituting another tier would override
        // explicit intent. It is *annotated*, because macOS would cd to $HOME
        // after a failed `cd`, and the user deserves to know that.
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

    /// labelled, delimited hash bytes: `["/a","/b"]` can never
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
/// a *filesystem/Home* fact — no DB row holds it, so a DB-only fingerprint
/// can never notice it moving. Production passes one built from the managed
/// [`crate::workspace::home::NoEndingHome`]; tests pass literals, which is what
/// keeps ("no test may resolve the real Home") true here.
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
    /// The single Owner Workstream of this launch, or `None`.
    /// A Session has at most one, so there is no "extra" list.
    pub owner_workstream_id: Option<String>,
    /// The directory the Agent WILL start in — authoritative. Since
    /// the spawn step uses this value and nothing else: `launch_prepared` no
    /// longer re-reads `sessions.cwd`, because re-reading let background
    /// discovery move a launch between Preview and Launch.
    pub cwd: Option<String>,
    /// Which of 's tiers produced `cwd`, so the UI can name it and a
    /// downgrade can never be silent. Part of `state_fingerprint`.
    #[serde(default)]
    pub cwd_resolution: CwdResolution,
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
    pub note: String,
    pub launch_intent_id: Option<String>,
}

pub struct SessionLauncher {
    /// Where launch artifacts go. Since v0.2 that is NoEnding Home's `runtime/`
    /// rather than the pre-Home `app_data_dir()`.
    pub runtime_dir: PathBuf,
}

impl SessionLauncher {
    /// Prepare New Session:
    /// Resolves cwd, freezes the runtime intent, and captures a state
    /// fingerprint. NO Context is built or injected.
    ///
    /// INVARIANT: Zero premature side effects. Does NOT insert LaunchIntent,
    /// does NOT write any file, and does NOT touch Context.
    ///
    /// `cwd` is the caller's explicit directory: when present it IS the launch
    /// directory (tier 1) and nothing below it is consulted. Otherwise the
    /// Owner Workstream's ordered paths decide, then the default workspace.
    pub fn prepare_new_in(
        &self,
        db: &Db,
        agent: Agent,
        owner_workstream_id: Option<&str>,
        cwd: Option<&str>,
        workspace: &LaunchWorkspace,
    ) -> Result<PreparedLaunch> {
        // A Workstream is the Owner (or there is none): an Owner that was
        // deleted mid-flight leaves nothing to launch against.
        if let Some(ws_id) = owner_workstream_id {
            if db.get_workstream(ws_id)?.is_none() {
                return Err(other("所选 Workstream 已不存在，请重新选择所属任务"));
            }
        }

        let resolution = resolve_new_cwd(db, owner_workstream_id, cwd, workspace)?;
        let runtime = crate::agent_runtime::runtime_overrides_for_launch(db, agent)?;
        let fingerprint = compute_state_fingerprint_in(
            db,
            "new",
            None,
            owner_workstream_id,
            agent,
            workspace,
            &resolution,
        )?;

        Ok(PreparedLaunch {
            id: new_id(),
            mode: "new".into(),
            agent,
            session_id: None,
            owner_workstream_id: owner_workstream_id.map(str::to_string),
            cwd: resolution.cwd.clone(),
            cwd_resolution: resolution,
            runtime,
            state_fingerprint: fingerprint,
            prepared_at: now(),
        })
    }

    /// Prepare Resume Session:
    /// Uses the Session's current Owner Workstream and captures the
    /// fingerprint. NO Context is built and NO Source is scanned.
    ///
    /// INVARIANT: Zero premature side effects. Does NOT change ownership and
    /// does NOT write any file.
    ///
    /// The Session's own cwd wins while it is a real directory; when it is gone
    /// the launch falls back to the Owner Workstream's primary path and then to
    /// the default workspace, and the fallback is recorded in
    /// `cwd_resolution` instead of being absorbed.
    pub fn prepare_resume_in(
        &self,
        db: &Db,
        session_id: &str,
        workspace: &LaunchWorkspace,
    ) -> Result<PreparedLaunch> {
        let session = db
            .get_session(session_id)?
            .ok_or_else(|| other("Session 不存在"))?;
        // a trashed session is inactive and must not resume. All resume
        // entries (command layer, Assistant, one-shot launch) funnel through
        // here, so this is the single prepare-side gate.
        if session.is_trashed() {
            return Err(other("会话已在回收站，无法继续；请先恢复会话"));
        }
        // Resume targets `sessions.root_agent_session_id` through the
        // ROOT member's source; a source the adapter cannot confirm present is
        // a refusal with an explicit reason, never a launch into a dead
        // thread. Unavailable (permission, parse, store problems) is NOT
        // missing — the refusal says so.
        match crate::lifecycle::root_source_status(db, &session)? {
            Some(crate::domain::SourceAvailability::Present) => {}
            Some(crate::domain::SourceAvailability::Missing) => {
                return Err(other("源会话已不存在，无法继续该会话"));
            }
            _ => {
                return Err(other(
                    "无法确认源会话当前可用（可能无权限或源不可读），已拒绝继续",
                ));
            }
        }

        self.prepare_resume_from_current(db, session_id, workspace)
    }

    /// The current-state half of Resume preparation, reading the Session row AS
    /// IT IS NOW. A Resume may not carry Context injection, so this only
    /// resolves the directory, freezes the runtime intent and hashes the launch
    /// state. Public so the ownership rule is directly testable.
    pub fn prepare_resume_from_current(
        &self,
        db: &Db,
        session_id: &str,
        workspace: &LaunchWorkspace,
    ) -> Result<PreparedLaunch> {
        let session = db
            .get_session(session_id)?
            .ok_or_else(|| other("Session 不存在"))?;
        if session.is_trashed() {
            return Err(other("会话已在回收站，无法继续；请先恢复会话"));
        }

        let owner = session.owner_workstream_id.clone();
        let resolution = resolve_resume_cwd(db, session_id, owner.as_deref(), workspace)?;
        let runtime = crate::agent_runtime::runtime_overrides_for_launch(db, session.agent)?;
        let fingerprint = compute_state_fingerprint_in(
            db,
            "resume",
            Some(session_id),
            owner.as_deref(),
            session.agent,
            workspace,
            &resolution,
        )?;

        Ok(PreparedLaunch {
            id: new_id(),
            mode: "resume".into(),
            agent: session.agent,
            session_id: Some(session_id.to_string()),
            owner_workstream_id: owner,
            cwd: resolution.cwd.clone(),
            cwd_resolution: resolution,
            runtime,
            state_fingerprint: fingerprint,
            prepared_at: now(),
        })
    }

    /// Launch a previously prepared launch.
    ///
    /// INVARIANT:
    /// 1. Verifies state fingerprint matches current DB state. If the working
    ///    directory inputs or the Runtime override intent changed since
    ///    preview, aborts with a stale error.
    /// 2. Identity preservation: passes the EXACT prepared runtime overrides
    ///    (NO re-reading Settings) and starts the Agent in `prepared.cwd` (NO
    ///    re-resolving).
    /// 3. Commits the LaunchIntent (new) only upon actual launch. No Context
    ///    file, no injection, no pre-sync.
    /// `launch_prepared` with the current [`LaunchWorkspace`].
    ///
    /// The default workspace is a Home fact the fingerprint has to re-read from
    /// *now*: if the Home moved after Preview, the prepared launch must abort,
    /// not start the Agent in a directory the user never saw.
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
    /// the same order: state fingerprint (Preview-Launch Identity) and the
    /// single-use capability consumed by the command layer.
    ///
    /// Exists so integration tests can drive the whole
    /// prepare → launch_prepared chain without opening a real Terminal window
    /// on the developer's machine.
    pub fn launch_prepared_with_in(
        &self,
        db: &Db,
        prepared: &PreparedLaunch,
        workspace: &LaunchWorkspace,
        spawn: fn(&AgentCommand) -> Result<crate::platform::launcher::LaunchOutcome>,
    ) -> Result<LaunchResult> {
        // re-resolve the launch directory from the state that
        // exists NOW (ordered Workstream paths, the Session's own cwd, the Home
        // default workspace) and hash it. A drift in any of those inputs lands
        // on a different fingerprint, so a plan whose directory moved is
        // refused here — the spawn below keeps using `prepared.cwd` verbatim.
        let resolution = recompute_launch_cwd(db, prepared, workspace)?;
        let current_fingerprint = compute_state_fingerprint_in(
            db,
            &prepared.mode,
            prepared.session_id.as_deref(),
            prepared.owner_workstream_id.as_deref(),
            prepared.agent,
            workspace,
            &resolution,
        )?;
        if current_fingerprint != prepared.state_fingerprint {
            return Err(other(
                "Prepared launch is stale: working directory or runtime state changed since preview. Please refresh preview.",
            ));
        }

        // Preview = Launch: the argv comes from the frozen intent, not from
        // whatever Settings holds right now.
        let runtime_opts = prepared.runtime.exec_options();

        if prepared.mode == "new" {
            let intent = LaunchIntent {
                id: new_id(),
                launch_type: "new".into(),
                agent: prepared.agent,
                owner_workstream_id: prepared.owner_workstream_id.clone(),
                cwd: prepared.cwd.clone(),
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

            let cmd: AgentCommand =
                adapter.build_new_command(&install, &runtime_opts, cwd_path.as_deref())?;
            let outcome = spawn(&cmd)?;

            // Record how the Agent was started. Guarded like every other write
            // to a waiting intent: this note describes an intent that is still
            // PENDING, and a match that landed in between must not be reset to
            // waiting by it.
            if let Err(e) = db.tx(|tx| {
                crate::storage::update_waiting_launch_intent_conn(
                    tx,
                    &intent.id,
                    launch_status::PENDING,
                    &format!(
                        "launched_via={};pid={:?};runtime={}",
                        outcome.launched_via,
                        outcome.pid,
                        prepared.runtime.intent_summary()
                    ),
                )
            }) {
                // The OS process is already running. A bookkeeping write must
                // not turn that successful launch into a reported failure.
                eprintln!(
                    "[launcher] Agent started (pid {:?}) but LaunchIntent detail could not be saved: {e}",
                    outcome.pid
                );
            }
            Ok(LaunchResult {
                launched_via: outcome.launched_via,
                command_line: outcome.command_line,
                note: if prepared.owner_workstream_id.is_some() {
                    "新 Session 启动后会在发现时通过 LaunchIntent 自动归属所选任务。".into()
                } else {
                    "已直接启动；本次未选择所属任务。".into()
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
            // belt and braces beside the fingerprint term: a trashed
            // session never launches, even if every other input managed to
            // match a stale-but-legal fingerprint.
            if session.is_trashed() {
                return Err(other(
                    "Prepared launch is stale: 会话已移入回收站，无法继续。请恢复会话后重新预览。",
                ));
            }

            let install = resolve_install(db, session.agent)?;
            let adapter = crate::adapters::adapter_for(session.agent);
            // resume starts in the directory the Preview showed, not
            // in whatever `sessions.cwd` says right now.
            let cwd_path = prepared.cwd.as_deref().map(PathBuf::from);
            // the resume identity is the ROOT's, never a member's
            // arbitrary external id.
            let cmd = adapter.build_resume_command(
                &install,
                &runtime_opts,
                &session.root_agent_session_id,
                cwd_path.as_deref(),
            )?;
            let outcome = spawn(&cmd)?;

            Ok(LaunchResult {
                launched_via: outcome.launched_via,
                command_line: outcome.command_line,
                note: "已直接在原工作目录恢复该会话。".into(),
                launch_intent_id: None,
            })
        }
    }

    pub fn new_session_in(
        &self,
        db: &Db,
        agent: Agent,
        owner_workstream_id: Option<&str>,
        cwd: Option<&str>,
        workspace: &LaunchWorkspace,
    ) -> Result<LaunchResult> {
        let prepared = self.prepare_new_in(db, agent, owner_workstream_id, cwd, workspace)?;
        self.launch_prepared_in(db, &prepared, workspace)
    }

    pub fn resume_session_in(
        &self,
        db: &Db,
        session_id: &str,
        workspace: &LaunchWorkspace,
    ) -> Result<LaunchResult> {
        let prepared = self.prepare_resume_in(db, session_id, workspace)?;
        self.launch_prepared_in(db, &prepared, workspace)
    }
}

/// Recompute the fingerprint of a launch **from the database as it is now**,
/// resolving the chain the same way `prepare_*` does.
///
/// This is the "has anything the user saw moved?" question. It takes the same
/// [`LaunchWorkspace`] the prepare step used: `None` in `default_workspace`
/// means "this launcher has no Home", which drops 's third tier from the
/// chain, so the answer is a different statement than the one a real Home
/// produces.
///
/// A caller that already holds a `PreparedLaunch` — where the resolved tier is
/// a recorded fact, not something to re-derive — uses
/// [`compute_state_fingerprint_in`] with that same [`CwdResolution`].
pub fn compute_state_fingerprint(
    db: &Db,
    mode: &str,
    session_id: Option<&str>,
    owner_workstream_id: Option<&str>,
    agent: Agent,
    workspace: &LaunchWorkspace,
) -> Result<String> {
    let resolution = if mode == "resume" {
        match session_id {
            Some(sid) => resolve_resume_cwd(db, sid, owner_workstream_id, workspace)?,
            None => CwdResolution::default(),
        }
    } else {
        resolve_new_cwd(db, owner_workstream_id, None, workspace)?
    };
    compute_state_fingerprint_in(
        db,
        mode,
        session_id,
        owner_workstream_id,
        agent,
        workspace,
        &resolution,
    )
}

/// the full fingerprint. Everything that decides what the Agent receives:
///
/// * the delivery level and the Agent runtime override intent;
/// * the Owner Workstream (or none): title / description / `updated_at`, its
///   active Context items with their current revisions, its open conflicts —
///   and its **ordered path list**;
/// * the resolved launch directory and the tier that produced it;
/// * NoEnding Home's default workspace, which is not in the DB at all
///   (note 4);
/// * in resume mode: whether the Session still exists, its `last_activity_at`,
///   its source cursor, its **`workspace_path_id`**, its Owner Workstream and
///   its delivery snapshots.
///
/// Every added input is labelled and terminated by `|`, so a value
/// cannot be re-read as a different field by shifting a boundary, and the path
/// list is hashed **in list order** because position 0 is the fact the launch
/// depends on. A WorkstreamPath mutation does not bump `workstreams.updated_at`
/// so without `ws_paths:` here a reorder, an added path or a removed primary
/// would leave a stale plan looking fresh.
pub fn compute_state_fingerprint_in(
    db: &Db,
    mode: &str,
    session_id: Option<&str>,
    owner_workstream_id: Option<&str>,
    agent: Agent,
    workspace: &LaunchWorkspace,
    resolution: &CwdResolution,
) -> Result<String> {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(mode.as_bytes());
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

    // tier 3: the Home's default workspace. It lives outside the database,
    // so a DB-only fingerprint could never see it move — which would let a
    // preview made under one Home launch under another.
    hasher.update(b"default_ws:");
    if let Some(dir) = &workspace.default_workspace {
        hasher.update(dir.as_bytes());
    }
    hasher.update(b"|");

    // The directory the Agent will actually start in, and why.
    hasher.update(resolution.fingerprint_input());

    if let Some(ws_id) = owner_workstream_id {
        hasher.update(b"owner:");
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
                // the lifecycle state is part of the launch state: a
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
                // "Session workspace path": the identity of the directory
                // this Session was observed in. A cwd string that now resolves
                // to a different WorkspacePath row (or to none) is a different
                // launch, even when the string itself survived.
                hasher.update(b"session_path:");
                if let Some(path_id) = &s.workspace_path_id {
                    hasher.update(path_id.as_bytes());
                }
                hasher.update(b"|");
                // the read state is the ROOT member's cursor; the
                // ingested conversation frontier rides with it, so a preview
                // made before a member ingest cannot silently launch against
                // a different conversation.
                if let Some(root) = db.root_member_for_session(sid).ok().flatten() {
                    if let Ok(cursor) = db.get_member_cursor(&root.id) {
                        hasher.update(&cursor.generation.to_le_bytes());
                        hasher.update(&cursor.byte_offset.to_le_bytes());
                        hasher.update(cursor.identity_tail_hash.as_bytes());
                        hasher.update(b"|");
                    }
                }
                if let Ok(seq) = db.ingested_message_sequence(sid) {
                    hasher.update(&seq.to_le_bytes());
                    hasher.update(b"|");
                }
            } else {
                hasher.update(b"session_exists:0:");
                hasher.update(b"|");
            }
            // The Session's Owner Workstream is part of the launch state: an
            // owner edit between Preview and Launch must abort, not resume
            // against a different Context route.
            hasher.update(b"session_owner:");
            if let Some(owner) = db.get_session(sid)?.and_then(|s| s.owner_workstream_id) {
                hasher.update(owner.as_bytes());
            }
            hasher.update(b"|");
        }
    }

    let hash = hasher.finalize();
    Ok(format!("{:x}", hash))
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

/// Expand a leading `~` / `~/` / `~\` to the user's home directory.
///
/// Users type `~/projects/x` into a working-directory field, and the rendered
/// terminal script must receive an absolute path — POSIX single quotes and
/// PowerShell `-LiteralPath` both treat `~` literally, so an unexpanded value
/// silently cds to $HOME. `~user` is intentionally unsupported.
///
/// A thin alias on purpose: the rules live in `workspace::identity`, which is
/// the only tilde expander in the crate. Two expanders means two
/// answers for one path.
pub fn expand_tilde(p: &str) -> String {
    crate::workspace::expand_tilde(p)
}

/// Is this recorded directory something an Agent can actually be started in?
///
/// An *observation*, never an identity: the path's
/// WorkspacePath row and its `canonical_path` stay exactly as they are whether
/// or not the volume is mounted. It matters here because of handing
/// the terminal a directory that is not there does not fail loudly everywhere:
/// macOS prints a hint and cds to `$HOME` instead, and `$HOME` is the one place
/// refuses to treat as a workspace. So an unusable tier is *skipped and
/// reported*, not launched into.
fn is_usable_directory(path: &str) -> bool {
    !path.trim().is_empty() && std::path::Path::new(path).is_dir()
}

/// the default workspace is a directory NoEnding owns, so the
/// launcher may create it if Home bootstrap did not. This is the only
/// filesystem effect a launch resolution has, it is idempotent, and it creates
/// no domain fact: no row, no Revision, no delivery. Preparation
/// integrity is about *storage*.
fn ensure_default_workspace(dir: &str) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    Ok(())
}

/// The `(cwd, source)` of 's WorkstreamPath tier: walk the Owner
/// Workstream's ordered path list in position order and return the first
/// usable directory.
///
/// Position 0 wins whenever it can (that is what "primary" means); a later
/// position is only reached because the earlier ones are unusable, and it is
/// reported with its position so the UI can say so. Returns `None` plus a note
/// when the Workstream has paths but none of them are usable.
fn first_workstream_path(
    db: &Db,
    owner_workstream_id: Option<&str>,
) -> Result<(Option<CwdResolution>, Option<String>)> {
    let Some(ws_id) = owner_workstream_id else {
        return Ok((None, None));
    };
    let paths = crate::workspace::workstream::workstream_launch_paths(db, ws_id)?;
    let mut blocked: Option<String> = None;
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
                workstream_id: Some(ws_id.to_string()),
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
    Ok((None, blocked))
}

/// tier 3 — the default workspace, or `None` when this launcher was not
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

/// New Session launch directory, in priority order:
///
/// ```text
/// explicit cwd  →  the Owner Workstream's ordered WorkstreamPaths
///                  (first usable, in list order)
///              →  NoEnding Home's default workspace
///              →  Unresolved
/// ```
///
/// A Workstream's ordered path list is the recorded answer to "where does this
/// work happen"; an activity-derived Session cwd is never a launch tier.
///
/// A Workstream's path list is a launch convenience, never identity: the
/// Workstream is still not a path, and Sessions keep their own cwd.
pub fn resolve_new_cwd(
    db: &Db,
    owner_workstream_id: Option<&str>,
    explicit: Option<&str>,
    workspace: &LaunchWorkspace,
) -> Result<CwdResolution> {
    if let Some(c) = explicit {
        return Ok(CwdResolution::explicit(expand_tilde(c)));
    }
    let (from_paths, blocked) = first_workstream_path(db, owner_workstream_id)?;
    if let Some(resolution) = from_paths {
        return Ok(resolution);
    }
    let standalone = owner_workstream_id.is_none();
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

/// Resume launch directory:
///
/// ```text
/// the Session's own cwd  →  the Owner Workstream's available path
///                        →  NoEnding Home's default workspace
/// ```
///
/// "Unavailable" here means missing, empty, or not a directory. Before v0.2 this
/// chain did not exist: a Session whose cwd had gone simply handed `None` to the
/// terminal, which on macOS means `$HOME` — a silent, and per
/// forbidden, destination. Every step below the Session's own directory is
/// reported in `cwd_resolution` and hashed into the fingerprint.
///
/// A missing Session row resolves to nothing rather than failing: the caller's
/// fingerprint check reports the real news (the Session it previewed is gone)
/// as staleness, not as an error from a helper.
pub fn resolve_resume_cwd(
    db: &Db,
    session_id: &str,
    owner_workstream_id: Option<&str>,
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
    let (from_paths, blocked) = first_workstream_path(db, owner_workstream_id)?;
    if let Some(mut resolution) = from_paths {
        resolution.fallback = true;
        resolution.note = Some(format!(
            "原 Session 的工作目录 {} 当前不可用，改用其所归属任务的工作路径",
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
/// launch time. This is the *staleness* side of; the Agent still starts in
/// `prepared.cwd` (see), so a difference here is a refusal, never a
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
        return resolve_resume_cwd(db, sid, prepared.owner_workstream_id.as_deref(), workspace);
    }
    if prepared.cwd_resolution.source == CwdSource::Explicit {
        // A directory the user named in the form is a literal, not a derivation:
        // there is nothing in the database for it to drift against.
        return Ok(prepared.cwd_resolution.clone());
    }
    resolve_new_cwd(db, prepared.owner_workstream_id.as_deref(), None, workspace)
}

// ---------------------------------------------------------------------------
// LaunchIntent matching
// ---------------------------------------------------------------------------

/// Try to match a newly discovered session against pending LaunchIntents.
/// Signals: agent type (required), launch→session timing, cwd. One clear
/// winner → auto-match; several close candidates → ambiguous (never
/// silently guess); nothing → stays pending for a later reconcile.
///
/// Run against the real Home so 's shared-default-workspace rule can
/// be applied.
///
/// The matcher is told which directory every "no Workstream path" launch shares.
///
/// 's third tier routes several independent launches into the
/// *same* cwd, and cwd was worth `+3.0` — enough on its own to look decisive.
/// A directory that many intents share carries no distinguishing evidence, so
/// against the default workspace it is worth `+1.0`, and a real cwd match still
/// outranks it. When scores are otherwise tied, the intent whose user *chose an
/// Owner Workstream* wins: an explicit choice is a stronger statement than a
/// coincidental directory. Anything still tied stays AMBIGUOUS for a human — no
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
                    // the shared default workspace is not a
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
            apply_match(db, &intent.id, session, workspace)?;
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
                .filter(|i| i.owner_workstream_id.is_some())
                .collect();
            if clear {
                let (intent, _) = scored.remove(0);
                apply_match(db, &intent.id, session, workspace)?;
                Ok(true)
            } else if selected.len() == 1 {
                apply_match(db, &selected[0].id, session, workspace)?;
                Ok(true)
            } else {
                // every tied top candidate is awaiting the user, not
                // just the first one — marking only one would hide the other
                // behind a PENDING state that says "still being discovered".
                let note = format!(
                    "多个候选 Session（score {:.1} vs {:.1}），等待用户确认",
                    best_score, second
                );
                // One transaction, and every write is guarded: "still waiting"
                // must never be written over an intent another match already
                // consumed (, one-shot capability).
                db.tx(|tx| {
                    for (intent, _) in scored.iter().filter(|(_, s)| tied(*s)) {
                        crate::storage::update_waiting_launch_intent_conn(
                            tx,
                            &intent.id,
                            launch_status::AMBIGUOUS,
                            &format!(
                                "{}（{}）",
                                note,
                                if intent.owner_workstream_id.is_some() {
                                    "已选所属任务"
                                } else {
                                    "未归属"
                                }
                            ),
                        )?;
                    }
                    Ok(())
                })?;
                Ok(false)
            }
        }
    }
}

/// Consume a LaunchIntent for `session`: the ONE Session that gets to inherit
/// its Owner and its delivery snapshot.
///
/// `intent_id` is the whole input — the intent is re-read INSIDE the writer
/// transaction, because the copy the caller matched on can be arbitrarily old
/// by the time the lock is taken. Everything the match depends on is verified
/// there:
///
/// * the intent still waits (PENDING/AMBIGUOUS) and no Session has claimed it;
/// * its Agent is the Session's Agent;
/// * the Session still exists and is not trashed;
/// * the final status write is a CAS whose affected rows must be 1.
///
/// Together those make the intent a capability that can be spent once: two
/// resolvers reading the same PENDING intent cannot both hand it out, and a
/// failure anywhere leaves the intent exactly as waiting as it was.
///
/// `_workspace` stays in the signature because every caller and test drives
/// this through the same launcher seam; matching itself no longer consults the
/// default workspace, since a match writes ownership only.
pub fn apply_match(
    db: &Db,
    intent_id: &str,
    session: &Session,
    _workspace: &LaunchWorkspace,
) -> Result<()> {
    db.tx(|tx| {
        let intent = crate::storage::get_launch_intent_conn(tx, intent_id)?
            .ok_or_else(|| other("LaunchIntent 不存在"))?;
        let waiting = matches!(
            intent.status.as_str(),
            launch_status::PENDING | launch_status::AMBIGUOUS
        );
        if !waiting || intent.matched_session_id.is_some() {
            return Err(other("该 LaunchIntent 已被其他匹配处理，本次匹配作废"));
        }
        if intent.agent != session.agent {
            return Err(other("LaunchIntent 与 Session 的 Agent 不一致，拒绝匹配"));
        }
        // The Session row decides, not the caller's copy: it may have been
        // trashed or removed since the match was computed.
        let current: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT agent, trashed_at FROM sessions WHERE id = ?1",
                rusqlite::params![session.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match current {
            Some((agent, None)) if agent == intent.agent.as_str() => {}
            _ => return Err(other("Session 不存在或已在回收站，无法匹配 LaunchIntent")),
        }

        // The matched Session inherits the intent's Owner Workstream verbatim
        //. Nothing else is written: no WorkstreamPath is added, and
        // the Session's cwd / workspace_path_id / project_id stay untouched.
        // Reading the intent here also means a Workstream deleted after the
        // match was computed is seen as the NULL the FK already wrote, rather
        // than as the stale id the caller read.
        if let Some(ws_id) = intent.owner_workstream_id.as_deref() {
            crate::storage::set_session_owner_conn(tx, &session.id, Some(ws_id))?;
        }
        // Status LAST, and as a CAS: an intent that says MATCHED always has the
        // Session and its Owner to go with it, and neither can land without the
        // other. Zero affected rows means another writer consumed the intent
        // between our read and this statement — the transaction rolls back
        // rather than overwrite their match.
        let claimed = crate::storage::mark_launch_intent_matched_conn(
            tx,
            &intent.id,
            &session.id,
            &format!("自动匹配：session {}", session.id),
        )?;
        if !claimed {
            return Err(other("该 LaunchIntent 已被其他匹配消费，本次匹配作废"));
        }
        Ok(())
    })
}

/// Is the Session still the one a preparation was built for? True only while it
/// exists, is not trashed, and still names `owner` as its Owner Workstream.
///
/// The post-build half of the prepare-time CAS: sync and the user's edits run
/// without the DB lock, so between the read that decided `owner` and the read
/// that produced the bundle the row can move. Public because the rule is worth
/// pinning directly — a Session missing this check resumes under an Owner it no
/// longer has.
pub fn preparation_matches_current_owner(
    db: &Db,
    session_id: &str,
    owner: Option<&str>,
) -> Result<bool> {
    Ok(db
        .get_session(session_id)?
        .map(|s| !s.is_trashed() && s.owner_workstream_id.as_deref() == owner)
        .unwrap_or(false))
}

/// Pending intents that never produced a session expire; ambiguous ones
/// wait for the user and only expire after 3× the TTL.
pub fn expire_stale_launch_intents(db: &Db) -> Result<usize> {
    let pending =
        db.list_launch_intents(&[launch_status::PENDING, launch_status::AMBIGUOUS], 500)?;
    let now_ts = chrono::Utc::now();
    let mut expired = 0;
    // One transaction, and each write only touches intent that still wait:
    // expiring by id alone would overwrite a match that landed after the list
    // above was read.
    for intent in pending {
        let ttl = if intent.status == launch_status::AMBIGUOUS {
            INTENT_TTL_SECS * 3
        } else {
            INTENT_TTL_SECS
        };
        if let Ok(launched) = chrono::DateTime::parse_from_rfc3339(&intent.launched_at) {
            if now_ts - launched.with_timezone(&chrono::Utc) > chrono::Duration::seconds(ttl) {
                let expired_now = db.tx(|tx| {
                    crate::storage::update_waiting_launch_intent_conn(
                        tx,
                        &intent.id,
                        launch_status::EXPIRED,
                        "超时未发现匹配 Session",
                    )
                })?;
                if expired_now {
                    expired += 1;
                }
            }
        }
    }
    Ok(expired)
}
