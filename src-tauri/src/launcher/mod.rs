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
//! cwd, runtime, Owner, LaunchIntent) and an optional first user turn for New.
//! Context is consumed on demand from the Session / Workstream pages.
//!
//! Working directory: every launch resolves
//! its directory through `resolve_new_cwd` / `resolve_resume_cwd`, which read
//! the explicit cwd and NoEnding Home's default workspace. The
//! answer, including *which tier*
//! produced it, is carried on the PreparedLaunch and hashed into its state
//! fingerprint, so Preview-Launch Identity covers the directory as well as the
//! runtime intent.

use std::path::PathBuf;

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::adapters::{AgentCommand, DesktopResume};
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
    /// a resume that lost its Session directory.
    pub fallback: bool,
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
    /// Set when Continue for this session means opening the Agent's desktop
    /// app instead of the terminal. The route is decided per ROOT member's
    /// SOURCE FORMAT at prepare time (`AgentAdapter::continue_route`) — the
    /// same format facts the fingerprint already covers — and frozen at
    /// Preview like every other launch fact. `None` = the terminal CLI path.
    #[serde(default)]
    pub desktop_open: Option<DesktopResume>,
    /// True when the spawn step must run the CLI inside NoEnding's embedded
    /// terminal (`crate::terminal`) instead of an external window. Frozen at
    /// Preview by the command layer from the stored open-method preference;
    /// like `desktop_open` it rides on static format facts, so it is not
    /// separately hashed. `desktop_open` and `embedded` are mutually
    /// exclusive — a desktop route never carries the embedded flag.
    #[serde(default)]
    pub embedded: bool,
    /// The New Session composer's first user turn, frozen for this launch.
    /// It is not Context and is never added to a Resume invocation.
    #[serde(default)]
    pub initial_message: Option<String>,
    pub state_fingerprint: String,
    pub prepared_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LaunchResult {
    pub launched_via: String,
    pub command_line: String,
    pub note: String,
    pub launch_intent_id: Option<String>,
    /// Set when this launch opened the embedded terminal: the id the
    /// frontend attaches the terminal subpage to.
    #[serde(default)]
    pub terminal_id: Option<String>,
}

/// The embedded spawn step made available to `launch_prepared`: the backend
/// terminal registry plus the spawn function (`crate::terminal::spawn_embedded`
/// in production, a fake in tests). `None` means this launch path has no
/// embedded surface at all — an `embedded` prepared launch must then be
/// refused, never silently downgraded to an external window.
pub struct EmbeddedSpawn<'a> {
    pub registry: &'a crate::terminal::TerminalRegistry,
    pub reconnect: Option<&'a crate::terminal::TerminalSummary>,
    pub spawn: fn(
        &AgentCommand,
        &crate::terminal::EmbeddedTarget,
    ) -> Result<crate::platform::launcher::LaunchOutcome>,
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
    /// directory and nothing below it is consulted. Otherwise the
    /// NoEnding default workspace is used.
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
            require_unarchived_owner(db, ws_id)?;
        }

        let resolution = resolve_new_cwd(cwd, workspace)?;
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
            desktop_open: None,
            embedded: false,
            initial_message: None,
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
    /// the launch falls back to
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
        if session.is_archived() {
            return Err(other("会话已在已归档，无法继续；请先恢复会话"));
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
        if session.is_archived() {
            return Err(other("会话已在已归档，无法继续；请先恢复会话"));
        }

        let owner = session.owner_workstream_id.clone();
        let resolution = resolve_resume_cwd(db, session_id, workspace)?;
        // The Continue route rides on the session's SOURCE FORMAT: it tells
        // whether the Agent's CLI can resume at all and how
        // (`AgentAdapter::continue_route`). A refusal is stated HERE, at
        // preview time — a launch that cannot resume must never look
        // preparable.
        let adapter = crate::adapters::adapter_for(session.agent);
        // The continue route is the format's own static fact. This prepare
        // feeds the embedded terminal launch, which forces the terminal route
        // anyway; desktop opens go through `continue_session_desktop`, which
        // asks `desktop_resume_route` directly.
        let route = adapter.continue_route(&session);
        let desktop_open = match route {
            crate::adapters::ResumeRoute::Terminal => None,
            crate::adapters::ResumeRoute::Desktop(open) => Some(open),
            crate::adapters::ResumeRoute::Refused(reason) => return Err(other(reason)),
        };
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
            desktop_open,
            // The embedded flag is a command-layer freeze (it needs the
            // terminal registry for the double-open refusal); a launcher-level
            // prepare resolves the route only, and embedded rides the same
            // terminal route as the default.
            embedded: false,
            initial_message: None,
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
        embedded: Option<&EmbeddedSpawn<'_>>,
    ) -> Result<LaunchResult> {
        self.launch_prepared_with_in(
            db,
            prepared,
            workspace,
            crate::platform::launcher::launch,
            crate::platform::launcher::open_uri,
            embedded,
        )
    }

    /// `launch_prepared` with the *process spawn* steps injected.
    ///
    /// The injected functions replace ONLY the OS calls: `spawn` opens a
    /// terminal and runs the Agent CLI, `open` dispatches a desktop-open URI
    /// (deep link / app activation). Every integrity gate stays on the same
    /// path in the same order: state fingerprint (Preview-Launch Identity)
    /// and the single-use capability consumed by the command layer.
    ///
    /// Exists so integration tests can drive the whole
    /// prepare → launch_prepared chain without opening a real Terminal
    /// window or activating a real desktop app on the developer's machine.
    pub fn launch_prepared_with_in(
        &self,
        db: &Db,
        prepared: &PreparedLaunch,
        workspace: &LaunchWorkspace,
        spawn: fn(&AgentCommand) -> Result<crate::platform::launcher::LaunchOutcome>,
        open: fn(&str) -> Result<()>,
        embedded: Option<&EmbeddedSpawn<'_>>,
    ) -> Result<LaunchResult> {
        // re-resolve the launch directory from the state that
        // exists NOW (the Session's own cwd, the Home
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
            if let Some(owner) = prepared.owner_workstream_id.as_deref() {
                require_unarchived_owner(db, owner)?;
            }
            let intent = LaunchIntent {
                id: new_id(),
                agent: prepared.agent,
                owner_workstream_id: prepared.owner_workstream_id.clone(),
                cwd: prepared.cwd.clone(),
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

            // A CLI that accepts a prespecified session id (claude / pi) gets
            // a UUID generated HERE, before spawn: the session file is born
            // with a known identity and the embedded terminal binds to the
            // discovered session by exact match. codex / agy generate their
            // own ids, so they remain unbound until a trustworthy native
            // identity is available; discovery never guesses from messages.
            let mut runtime_opts = prepared.runtime.exec_options();
            let expected_root_session_id =
                adapter.supports_prespecified_session_id().then(|| new_id());
            runtime_opts.root_session_id = expected_root_session_id.clone();
            runtime_opts.initial_message = prepared.initial_message.clone();

            let cmd: AgentCommand =
                adapter.build_new_command(&install, &runtime_opts, cwd_path.as_deref())?;
            // Each interactive adapter puts the first message in the final
            // argv element. Only redact that element in UI diagnostics: the
            // real command still carries the original message into the PTY.
            let public_command_line = runtime_opts.initial_message().map(|_| {
                let mut display_cmd = cmd.clone();
                if let Some(message_arg) = display_cmd.args.last_mut() {
                    *message_arg = "[首条消息]".into();
                }
                display_cmd.display()
            });
            // An embedded NEW launch spawns into NoEnding's own PTY with no
            // session identity yet: the registry entry starts unbound and
            // binds when discovery supplies the same native identity
            // (prespecified or reported by the Agent). External
            // spawns stay the default when no embedded surface is provided.
            let outcome = if prepared.embedded {
                let embedded =
                    embedded.ok_or_else(|| other("内嵌终端服务不可用，请刷新预览后重试"))?;
                (embedded.spawn)(
                    &cmd,
                    &crate::terminal::EmbeddedTarget {
                        registry: embedded.registry,
                        session_id: None,
                        root_agent_session_id: expected_root_session_id.as_deref(),
                        agent: prepared.agent,
                    },
                )?
            } else {
                spawn(&cmd)?
            };

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
                command_line: public_command_line.unwrap_or(outcome.command_line),
                note: if prepared.owner_workstream_id.is_some() {
                    "新 Session 启动后会在发现时通过 LaunchIntent 自动归属所选任务。".into()
                } else {
                    "已直接启动；本次未选择所属任务。".into()
                },
                launch_intent_id: Some(intent.id),
                terminal_id: outcome.terminal_id,
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
            if session.is_archived() {
                return Err(other(
                    "Prepared launch is stale: 会话已归档，无法继续。请恢复会话后重新预览。",
                ));
            }

            // A desktop-open Continue never reaches the terminal: the frozen
            // URI is dispatched via the platform's open-uri, and the CLI-
            // specific steps below (installation resolution, argv building,
            // terminal spawn) do not apply. No LaunchIntent: the session
            // already exists and was not launched anew.
            if let Some(desktop) = &prepared.desktop_open {
                open(&desktop.uri)?;
                return Ok(LaunchResult {
                    launched_via: "desktop app".into(),
                    command_line: desktop.uri.clone(),
                    note: desktop.note.clone(),
                    launch_intent_id: None,
                    terminal_id: None,
                });
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
                Some(&session.source_path),
            )?;
            // The embedded flag is the frozen preview fact; the spawn surface
            // must exist to honor it. No silent downgrade to an external
            // window: the user previewed "inside NoEnding".
            let outcome = if prepared.embedded {
                let embedded =
                    embedded.ok_or_else(|| other("内嵌终端服务不可用，请刷新预览后重试"))?;
                // Discovery may already have ingested a NEW terminal's
                // session. Resolve that binding before atomically reserving
                // this resume, and hold the reservation through spawn and
                // registry registration to exclude concurrent resumes.
                embedded.registry.bind_discovered(db);
                let _reservation = if let Some(expected) = embedded.reconnect {
                    embedded.registry.reserve_reconnect(expected, session_id)?
                } else {
                    embedded
                        .registry
                        .reserve_resume(session_id)
                        .map_err(other)?
                };
                let outcome = (embedded.spawn)(
                    &cmd,
                    &crate::terminal::EmbeddedTarget {
                        registry: embedded.registry,
                        session_id: Some(session_id),
                        root_agent_session_id: Some(&session.root_agent_session_id),
                        agent: session.agent,
                    },
                )?;
                if let Some(expected) = embedded.reconnect {
                    embedded.registry.close(&expected.terminal_id);
                }
                outcome
            } else {
                spawn(&cmd)?
            };

            Ok(LaunchResult {
                launched_via: outcome.launched_via,
                command_line: outcome.command_line,
                note: if prepared.embedded {
                    "已在内嵌终端恢复该会话。".into()
                } else {
                    "已直接在原工作目录恢复该会话。".into()
                },
                launch_intent_id: None,
                terminal_id: outcome.terminal_id,
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
        // One-shot launches have no embedded surface: the terminal subpage
        // attach protocol only exists for prepared resume launches.
        self.launch_prepared_in(db, &prepared, workspace, None)
    }

    pub fn resume_session_in(
        &self,
        db: &Db,
        session_id: &str,
        workspace: &LaunchWorkspace,
    ) -> Result<LaunchResult> {
        let prepared = self.prepare_resume_in(db, session_id, workspace)?;
        // Same as new: the one-shot path (Assistant, legacy command) is
        // external-terminal only. The embedded flag is frozen by the
        // prepared-resume command layer, which also passes the registry.
        self.launch_prepared_in(db, &prepared, workspace, None)
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
            Some(sid) => resolve_resume_cwd(db, sid, workspace)?,
            None => CwdResolution::default(),
        }
    } else {
        resolve_new_cwd(None, workspace)?
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
///   and its Context inputs;
/// * the resolved launch directory and the tier that produced it;
/// * NoEnding Home's default workspace, which is not in the DB at all
///   (note 4);
/// * in resume mode: whether the Session still exists, its `last_activity_at`,
///   its source cursor, its **`workspace_path_id`**, its Owner Workstream and
///   its delivery snapshots.
///
/// Every added input is labelled and terminated by `|`. Task directory display
/// order is not a launch input; changing it does not invalidate a preview.
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
                if s.is_archived() {
                    hasher.update(b"session_archived:1:");
                } else {
                    hasher.update(b"session_archived:0:");
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
                // the read state is the session's cursor; the ingested
                // conversation frontier rides with it, so a preview made
                // before an ingest cannot silently launch against a
                // different conversation.
                if let Ok(session) = db.get_session(sid) {
                    if let Some(session) = session {
                        hasher.update(&session.source_generation.to_le_bytes());
                        hasher.update(&session.source_byte_offset.to_le_bytes());
                        hasher.update(session.source_tail_hash.as_bytes());
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

/// New sessions use the explicitly selected directory or NoEnding's default.
/// Task directory order never determines a launch directory.
pub fn resolve_new_cwd(
    explicit: Option<&str>,
    workspace: &LaunchWorkspace,
) -> Result<CwdResolution> {
    if let Some(c) = explicit {
        return Ok(CwdResolution::explicit(expand_tilde(c)));
    }
    if let Some(resolution) = default_workspace_resolution(workspace)? {
        return Ok(resolution);
    }
    Ok(CwdResolution {
        source: CwdSource::Unresolved,
        note: Some("没有可解析的工作目录，Agent 将在终端默认目录启动".into()),
        ..Default::default()
    })
}

/// Resume uses the Session's own directory when usable, otherwise NoEnding's
/// default workspace. A fallback is reported to the user. Task paths do not
/// override the Session's directory, regardless of their display order.
pub fn resolve_resume_cwd(
    db: &Db,
    session_id: &str,
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
    if let Some(mut resolution) = default_workspace_resolution(workspace)? {
        resolution.fallback = true;
        resolution.note = Some(format!(
            "原 Session 的工作目录 {} 当前不可用，改用 NoEnding 默认工作目录",
            lost
        ));
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
        return resolve_resume_cwd(db, sid, workspace);
    }
    if prepared.cwd_resolution.source == CwdSource::Explicit {
        // A directory the user named in the form is a literal, not a derivation:
        // there is nothing in the database for it to drift against.
        return Ok(prepared.cwd_resolution.clone());
    }
    resolve_new_cwd(None, workspace)
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
/// * the Session still exists;
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
        // removed since the match was computed. Trash is not a refusal — a
        // trashed Session still follows its source and still takes its Owner.
        let current: Option<String> = tx
            .query_row(
                "SELECT agent FROM sessions WHERE id = ?1",
                rusqlite::params![session.id],
                |r| r.get(0),
            )
            .optional()?;
        match current {
            Some(agent) if agent == intent.agent.as_str() => {}
            _ => return Err(other("Session 不存在，无法匹配 LaunchIntent")),
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
        .map(|s| !s.is_archived() && s.owner_workstream_id.as_deref() == owner)
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

fn require_unarchived_owner(db: &Db, owner: &str) -> Result<()> {
    let task = db
        .get_workstream(owner)?
        .ok_or_else(|| other("所选任务已不存在，请重新选择所属任务"))?;
    if task.visibility == crate::domain::workstream_visibility::ARCHIVED {
        return Err(other("已归档的任务不能新建会话，请先取消归档"));
    }
    Ok(())
}
