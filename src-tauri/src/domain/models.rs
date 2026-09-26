//! Platform-agnostic domain model.
//!
//! Nothing here may depend on a specific Agent's data format.
//!
//! Workspace invariant: a physical working path IS domain state
//! (`WorkspacePath`, plus the ordered `workstream_paths` list). An Agent's raw
//! transcript layout is separate, and a Workstream is not a path — it *has* an
//! ordered list of paths, while a Session keeps its own authoritative cwd.
//!
//! Environment data (cwd etc.) is optional metadata carried by Session: a
//! Session with no cwd is legal and gets no WorkspacePath — nothing fabricates
//! one from a default.

use serde::{Deserialize, Serialize};

pub type Id = String;

/// A physical workspace family, maintained entirely by the app.
///
/// Users never create, delete, or pick a Project — they may only rename it
/// (`name_customized`). A Project exists exactly while it owns at least one
/// [`WorkspacePath`]; the last path being deleted or reassigned deletes it.
/// `git_id` is the optional Git anchor; `None` means the Project is currently
/// only anchored by its path(s), which is a normal state, not a degraded one.
#[derive(Debug, Clone, Serialize)]
pub struct Project {
    pub id: Id,
    pub name: String,
    pub description: String,
    /// References `git_identities.id`. UNIQUE across Projects when set.
    pub git_id: Option<Id>,
    /// A user renamed this Project: automatic naming must stop overwriting it,
    /// including after a Project merge or worktree discovery.
    pub name_customized: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// One observed physical working path — the bridge between the filesystem and
/// every Project fact in the system.
///
/// Identity: `id` IS the path identity, derived deterministically from
/// `canonical_path` (see `workspace::path_identity`) rather than being a random
/// UUID, so re-ensuring the same path is idempotent by construction. Git
/// presence does not affect it: the same directory stays the same
/// WorkspacePath across `.git` appearing, disappearing, or `git init` running
/// again.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspacePath {
    pub id: Id,
    pub canonical_path: String,
    /// NOT NULL by design: a WorkspacePath always belongs to exactly one Project.
    pub project_id: Id,
    pub git_state: String,        // none | detected | missing
    pub git_kind: Option<String>, // main | linked | unknown
    pub exists: bool,
    pub first_seen_at: String,
    pub last_seen_at: String,
}

impl WorkspacePath {
    /// Shared constructor for fixtures and domain creation. The id is computed
    /// by the same identity function production uses, so a fixture can never
    /// disagree with the real `path-<sha>` rule.
    pub fn new(canonical_path: impl Into<String>, project_id: Id) -> Self {
        let canonical_path = canonical_path.into();
        let id = crate::workspace::path_identity(&canonical_path);
        Self {
            id,
            canonical_path,
            project_id,
            git_state: git_state::NONE.into(),
            git_kind: None,
            exists: true,
            first_seen_at: String::new(),
            last_seen_at: String::new(),
        }
    }
}

pub mod git_state {
    /// No Git evidence at this path.
    pub const NONE: &str = "none";
    /// `.git` detected and resolvable.
    pub const DETECTED: &str = "detected";
    /// This path used to be Git-backed and the evidence is gone. Distinct from
    /// `none` on purpose: losing `.git` must never detach the path from its
    /// Project (§1.3), and `missing` is how we remember that.
    pub const MISSING: &str = "missing";
}

/// A recognized Git family, keyed by `common_dir`.
///
/// `id` is app-assigned (not a hash of the directory) because a repository can
/// move; the invariant is only "same recognized common dir → same git_id".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitIdentity {
    pub id: Id,
    pub common_dir: String,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub metadata: serde_json::Value,
}

/// One entry of a Workstream's ordered working-path list.
///
/// `position` is the whole role: 0 is the primary path, > 0 are secondary.
/// There is deliberately no `is_primary` / `role` column — two authorities for
/// one fact is how the old default cwd and Workstream project field drifted apart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkstreamPath {
    pub id: Id,
    pub workstream_id: Id,
    pub workspace_path_id: Id,
    pub position: i64,
    pub created_at: String,
}

/// What the WorkspaceResolver reports about a filesystem path. Purely
/// observational: it never creates or reassigns anything by itself.
#[derive(Debug, Clone)]
pub struct WorkspaceObservation {
    pub canonical_path: String,
    /// Deterministic WorkspacePath id for `canonical_path`.
    pub path_id: Id,
    pub exists: bool,
    pub git: GitDetection,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GitDetection {
    /// Nothing Git-like found, or the only evidence was the excluded Home-level
    /// repository (§1.4) / a reserved app path (§2).
    None,
    Detected {
        common_dir: String,
        toplevel: Option<String>,
        kind: GitWorktreeKind,
        /// `git worktree list --porcelain` result. Discovery of a worktree
        /// never adds a WorkstreamPath (§9).
        worktrees: Vec<String>,
    },
    /// A path that was previously detected now has no `.git`. Not a resolver
    /// output: it is derived by `workspace::project` from the stored
    /// `git_state` plus the latest observation, because "was detected before"
    /// is prior state the resolver does not have.
    Missing,
    /// Git could not be consulted at all (binary missing, timeout, unsafe
    /// directory). Treated exactly like [`GitDetection::None`] for ownership
    /// purposes, but kept distinct so it is never mistaken for a real
    /// "this is not a repository" answer.
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitWorktreeKind {
    Main,
    Linked,
    Unknown,
}

impl GitWorktreeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            GitWorktreeKind::Main => "main",
            GitWorktreeKind::Linked => "linked",
            GitWorktreeKind::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workstream {
    pub id: Id,
    pub title: String,
    pub description: String,
    pub lifecycle: String,  // active | completed
    pub visibility: String, // normal | archived
    pub created_at: String,
    pub updated_at: String,
}

pub mod workstream_lifecycle {
    /// Basic status classification. No behavioral difference from `COMPLETED`,
    /// and the user may switch freely (§1.13).
    pub const ACTIVE: &str = "active";
    pub const COMPLETED: &str = "completed";
}

pub mod workstream_visibility {
    pub const NORMAL: &str = "normal";
    /// The recycle bin. Restoring flips back to `normal` and nothing else —
    /// which is why lifecycle and paths survive a round trip.
    pub const ARCHIVED: &str = "archived";
}

/// Every Agent NoEnding knows how to READ. Not every entry can be launched:
/// Qoder ships no headless CLI, so its adapter ingests history only and
/// `exec_resolver::resolve` fails by design (方案 §37.1).
///
/// The rename on each variant is the IPC spelling and MUST equal `as_str()` —
/// the sessions table, the frontend's `Agent` union and its `AGENT_LABELS` all
/// use that same string. Declaring it per variant rather than deriving it with
/// `rename_all = "snake_case"` is deliberate: the derive spelled three variants
/// (`auto_claw`, `work_buddy`, `z_code`) differently from everything else, so
/// those rows rendered as「未知 Agent」and every command taking an `agent`
/// argument failed to deserialize (方案 §37.14). `serde_spelling_equals_as_str_for_every_agent`
/// keeps the three spellings in step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Agent {
    #[serde(rename = "codex")]
    Codex,
    #[serde(rename = "claude_code")]
    ClaudeCode,
    #[serde(rename = "pi")]
    Pi,
    #[serde(rename = "qoder")]
    Qoder,
    #[serde(rename = "workbuddy")]
    WorkBuddy,
    #[serde(rename = "dsh")]
    Dsh,
    #[serde(rename = "zcode")]
    ZCode,
}

impl Agent {
    /// A slice rather than a fixed-size array: the roster grows, and every
    /// caller only iterates.
    pub fn all() -> &'static [Agent] {
        &[
            Agent::Codex,
            Agent::ClaudeCode,
            Agent::Pi,
            Agent::Qoder,
            Agent::WorkBuddy,
            Agent::Dsh,
            Agent::ZCode,
        ]
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Agent::Codex => "Codex",
            Agent::ClaudeCode => "Claude Code",
            Agent::Pi => "Pi",
            Agent::Qoder => "Qoder",
            Agent::WorkBuddy => "WorkBuddy",
            Agent::Dsh => "dsh",
            Agent::ZCode => "ZCode",
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Agent::Codex => "codex",
            Agent::ClaudeCode => "claude_code",
            Agent::Pi => "pi",
            Agent::Qoder => "qoder",
            Agent::WorkBuddy => "workbuddy",
            Agent::Dsh => "dsh",
            Agent::ZCode => "zcode",
        }
    }

    pub fn parse(s: &str) -> Option<Agent> {
        match s {
            "codex" => Some(Agent::Codex),
            "claude_code" | "claude" => Some(Agent::ClaudeCode),
            "pi" => Some(Agent::Pi),
            "qoder" | "qcoder" => Some(Agent::Qoder),
            "workbuddy" | "work_buddy" => Some(Agent::WorkBuddy),
            "dsh" | "deepseek_harness" => Some(Agent::Dsh),
            "zcode" | "z_code" => Some(Agent::ZCode),
            _ => None,
        }
    }
}

/// A Logical Session: the one user-visible conversation plus every internal
/// execution member (child agents, sidechains) it spawned (重构方案 §2.1/§4).
///
/// A Session is NOT an Agent thread — it is keyed by the ROOT member's real
/// Resume identity (`root_agent_session_id`), while children/sides live on as
/// [`SessionMember`] rows of the same Session and can never own, rename or
/// resume it (§4.1).
///
/// Workspace facts on a Session:
/// - `workspace_path_id` is the authoritative link to the physical workspace;
///   it is set only when the ROOT member really has a cwd. A Session without
///   one stays `None` — the default workspace is a *launch* convenience and is
///   never retrofitted onto historical Sessions. Child/side cwds are execution
///   facts on their member rows and never flow up (§4.1, §18).
/// - `project_id` is a **derived cache** of
///   `workspace_path_id → workspace_paths.project_id`. It is written by exactly
///   two code paths (see `storage::session_paths`); a manual write into it is a
///   domain violation, not a convenience.
#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub id: Id, // internal stable id (app-owned)
    pub agent: Agent,
    /// The ROOT member's Agent-side Resume identity (方案 §17.2). Authority for
    /// LaunchIntent matching and `resume` — never a child's external id.
    pub root_agent_session_id: String,
    pub title: Option<String>,
    /// Semantic ownership: the one Workstream this Logical Session belongs to,
    /// or `None`. At most one Owner (方案 §3.3). Only an explicit user action
    /// or a matched LaunchIntent sets it — never fork inheritance (§2.2).
    pub owner_workstream_id: Option<Id>,
    pub cwd: Option<String>,
    pub workspace_path_id: Option<Id>,
    pub project_id: Option<Id>,
    /// When this Session is an independently continuable fork, the Logical
    /// Session its source forked from. Lifecycle, Owner, Conversation and
    /// Context stay fully independent of that source (§2.2); this is a
    /// provenance note, not a parent link in the old sense.
    pub forked_from_session_id: Option<Id>,
    pub started_at: Option<String>,
    /// Last activity across the WHOLE execution graph (root + child + side).
    pub last_activity_at: Option<String>,
    /// Last real user/assistant message time of the ROOT conversation.
    pub last_conversation_at: Option<String>,
    /// Session lifecycle authority. `None` = Normal; `Some(ts)` = Trash. This
    /// is the single lifecycle authority — there is no separate visibility
    /// flag. Trashing is reversible (Restore keeps the same Session id) and
    /// never touches the Agent source; permanent deletion removes the row
    /// entirely.
    pub trashed_at: Option<String>,
}

impl Session {
    /// A trashed Session is inactive inside NoEnding: hidden from default
    /// lists and search, not resumable, and not ingested or synced.
    pub fn is_trashed(&self) -> bool {
        self.trashed_at.is_some()
    }
}

/// How a [`SessionMember`] relates to its Logical Session's root (§5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionMemberRelation {
    Root,
    Child,
    Side,
}

impl SessionMemberRelation {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionMemberRelation::Root => "root",
            SessionMemberRelation::Child => "child",
            SessionMemberRelation::Side => "side",
        }
    }

    pub fn parse(s: &str) -> Option<SessionMemberRelation> {
        match s {
            "root" => Some(SessionMemberRelation::Root),
            "child" => Some(SessionMemberRelation::Child),
            "side" => Some(SessionMemberRelation::Side),
            _ => None,
        }
    }
}

/// One internal execution unit of a Logical Session — the Agent-side thread,
/// subagent transcript or sidechain file that together make up the session
/// (§5). Only the `root` member produces [`SessionMessage`]s; every member is
/// an observation surface for stats.
///
/// `source_member_id` is the Adapter's stable execution identity and need not
/// equal the Agent's native session id (§5.1): Qoder subagent transcripts
/// repeat the parent's id, so the adapter derives `root-id:subagent:<stem>`.
/// A source with no stable identity produces NO member — never a fabricated
/// row just to make the topology look complete.
#[derive(Debug, Clone, Serialize)]
pub struct SessionMember {
    pub id: Id,
    pub session_id: Id,
    pub agent: Agent,
    pub source_member_id: String,
    pub relation: SessionMemberRelation,
    /// Source-side parent identity; may temporarily resolve to no member row
    /// (the parent appears in a later discovery batch).
    pub parent_source_member_id: Option<String>,
    /// Adapter-owned descriptor of the source shape (`rollout_file`,
    /// `transcript_file`, `sqlite_record`, `subagent_file`, …).
    pub source_kind: String,
    /// Where the member's source lives. For file-per-session Agents this is
    /// the transcript path; for shared stores it is the store path.
    pub source_path: String,
    pub cwd: Option<String>,
    pub started_at: Option<String>,
    pub last_activity_at: Option<String>,
    /// Adapter-specific, non-Conversation structural facts (thread_source,
    /// delegation depth, …). Never conversation text.
    pub metadata: serde_json::Value,
}

/// The role of a [`SessionMessage`] — the only two shapes a conversation has
/// (§6). The database CHECK constraint is the last line of defense; adapters
/// must already have dropped everything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionMessageRole {
    User,
    Assistant,
}

impl SessionMessageRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionMessageRole::User => "user",
            SessionMessageRole::Assistant => "assistant",
        }
    }
}

/// One user-visible conversation turn of the ROOT member — the ONLY
/// conversation store NoEnding keeps (§6). Thinking, tool traffic, system /
/// developer prompts, compaction summaries and child/side transcripts never
/// become rows here; every visible prose segment of an assistant turn is kept
/// (no "final answer" guessing).
#[derive(Debug, Clone, Serialize)]
pub struct SessionMessage {
    pub id: Id,
    pub session_id: Id,
    /// Must reference the Session's ROOT member — enforced again at commit
    /// time (`commit_member_ingest`), even though the FK chain would accept a
    /// child: a stray child message is an ingestion bug, not silent data (§6).
    pub member_id: Id,
    /// NoEnding's own per-session monotonic sequence. Never a source line
    /// number; never reused or rolled back.
    pub sequence: i64,
    pub role: SessionMessageRole,
    pub content: String,
    pub ts: Option<String>,
    /// Message-level generation provenance (Provenance 方案 §5/§6). Only
    /// meaningful for Assistant messages; source-confirmed only — never
    /// inferred from configuration, branding or runtime preference. Unknown
    /// stays unknown (`NULL`).
    pub provider: Option<String>,
    pub model: Option<String>,
    // ---- source provenance (Context Integrity) ----
    pub source_message_id: Option<String>,
    pub source_generation: i64,
    pub source_position: String,
    pub source_identity_hash: String,
    pub raw_ref: String,
}

/// Per-member execution statistics, stored as a 1:1 snapshot (§7). This is
/// "current observable source state", not an append-only telemetry log.
///
/// `NULL` = the source does not provide / cannot reliably compute the metric;
/// `0` = observed zero. Unknown is never folded into 0 (§7.1).
#[derive(Debug, Clone, Serialize)]
pub struct SessionMemberStats {
    pub member_id: Id,
    pub tool_call_count: Option<i64>,
    pub tool_error_count: Option<i64>,
    pub compaction_count: Option<i64>,
    pub side_activity_count: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cached_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub cost: Option<f64>,
    // model / provider / effort removed (Provenance 方案 §9): message
    // provenance lives on SessionMessage; a member-level "current model"
    // was a second, semantically unclear authority.
    pub updated_at: String,
    pub extra: serde_json::Value,
}

/// Incremental stats for an append-only read (§7.3): each field is the number
/// of new observations since the last commit. `None` = nothing observed this
/// batch (the column is left untouched, not zeroed).
#[derive(Debug, Clone, Copy, Default)]
pub struct MemberStatsDelta {
    pub tool_call_count: Option<i64>,
    pub tool_error_count: Option<i64>,
    pub compaction_count: Option<i64>,
    pub side_activity_count: Option<i64>,
}

impl MemberStatsDelta {
    /// The delta of exactly one observation each — the spelling shared readers
    /// produce from a single parsed line.
    pub fn single(observation: MemberObservation) -> Self {
        Self {
            tool_call_count: (observation.tool_calls > 0).then_some(observation.tool_calls as i64),
            tool_error_count: (observation.tool_errors > 0)
                .then_some(observation.tool_errors as i64),
            compaction_count: (observation.compactions > 0)
                .then_some(observation.compactions as i64),
            side_activity_count: (observation.side_activity > 0)
                .then_some(observation.side_activity as i64),
        }
    }
}

/// Full-scan stats for a rescan (§7.3). `Some(0)` is observed zero; `None` is
/// unsupported and leaves the stored counter unchanged.
#[derive(Debug, Clone, Copy, Default)]
pub struct SessionMemberStatsSnapshot {
    pub tool_call_count: Option<i64>,
    pub tool_error_count: Option<i64>,
    pub compaction_count: Option<i64>,
    pub side_activity_count: Option<i64>,
}

/// How a read updates member stats (§7.3).
#[derive(Debug, Clone, Copy)]
pub enum StatsUpdate {
    Delta(MemberStatsDelta),
    Snapshot(SessionMemberStatsSnapshot),
}

/// Counters one source portion contributes (shared-reader vocabulary). Plain
/// counts, converted into a [`StatsUpdate`] by the adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct MemberObservation {
    pub tool_calls: u64,
    pub tool_errors: u64,
    pub compactions: u64,
    pub side_activity: u64,
}

impl MemberObservation {
    pub fn add(&mut self, other: &MemberObservation) {
        self.tool_calls += other.tool_calls;
        self.tool_errors += other.tool_errors;
        self.compactions += other.compactions;
        self.side_activity += other.side_activity;
    }
}

/// Where a member's Agent source stands, as far as reading is concerned (§8.1).
/// Belongs to the MEMBER, never to the Session — each member reads its own
/// source at its own pace. The Context frontier is the separate
/// [`SessionContextState`].
#[derive(Debug, Clone, Serialize, Default)]
pub struct SessionMemberCursor {
    pub member_id: Id,
    pub source_file_identity: String,
    pub generation: i64,
    pub byte_offset: u64,
    pub last_seen_size: u64,
    pub mtime: Option<f64>,
    /// SHA-256 of the read prefix when the offset was recorded; an append is
    /// only accepted while the stored prefix is still the file's prefix.
    pub prefix_hash: String,
    /// Identity hash of the last message on the CURRENT source chain — the
    /// base the next append batch continues from. Only messages advance it;
    /// stats-only batches keep the tail. Empty until a message exists.
    pub identity_tail_hash: String,
    /// Provenance state frontier (Provenance 方案 §15): the generation
    /// provenance an Adapter confirmed from explicit state events and that,
    /// by the source format, still governs the messages to come. ONLY a
    /// stateful-evidence adapter (Codex `turn_context`) uses these — every
    /// other adapter leaves them `None`/`None`, and they are never a UI
    /// authority or a Session "current model".
    #[serde(default)]
    pub active_provider: Option<String>,
    #[serde(default)]
    pub active_model: Option<String>,
}

impl SessionMemberCursor {
    /// The cursor an adapter's just-observed source state produces — the
    /// spelling tests and future callers need to chain a read onto a previous
    /// one.
    pub fn from_update(member_id: &str, u: &crate::domain::SourceCursorUpdate) -> Self {
        Self {
            member_id: member_id.to_string(),
            source_file_identity: u.file_identity.clone(),
            generation: u.generation,
            byte_offset: u.byte_offset,
            last_seen_size: u.last_seen_size,
            mtime: u.mtime,
            prefix_hash: u.prefix_hash.clone(),
            identity_tail_hash: String::new(),
            active_provider: None,
            active_model: None,
        }
    }
}

/// The Logical Session's Context frontier (§8.2): how far Sync has consumed
/// ROOT conversation messages. Lifecycle is completely independent of the
/// member cursors — Trash/Restore never touches it.
#[derive(Debug, Clone, Serialize, Default)]
pub struct SessionContextState {
    pub session_id: Id,
    pub processed_message_sequence: i64,
}

/// An ingestion problem that is deliberately NOT a Session (§11): a child or
/// side source whose root has not been seen. Diagnostics never enter the
/// Sessions UI, Search, Context, ownership or lifecycle; when the root shows
/// up and the member attaches, the row is deleted.
#[derive(Debug, Clone, Serialize)]
pub struct IngestionDiagnostic {
    pub id: Id,
    /// Stable identity of the problem, e.g. `<agent>:unresolved:<member id>`.
    pub diagnostic_key: String,
    pub agent: Agent,
    pub kind: String,
    pub source_member_id: Option<String>,
    pub parent_source_member_id: Option<String>,
    pub source_path: Option<String>,
    pub reason: String,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub observation_count: i64,
    pub details: serde_json::Value,
}

pub mod diagnostic_kind {
    /// A discovered member that could not be resolved to any Logical Session
    /// (no root, or the parent chain is broken for now).
    pub const UNRESOLVED_SESSION_MEMBER: &str = "unresolved_session_member";
}

/// Adapter's strict verdict about one member's source (§9.1). The semantics
/// are load-bearing for permanent deletion:
///
/// ```text
/// Present    = Adapter currently confirms the source exists
/// Missing    = Adapter currently confirms the source does not exist
/// Unavailable = no permission, I/O error, format problem, cannot tell,
///              store not accessible … ANY doubt
/// ```
///
/// No exception may degrade to `Missing`: only a confirmed-missing ROOT makes
/// a trashed Session permanently deletable (§20).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceAvailability {
    Present,
    Missing,
    Unavailable,
}

impl SourceAvailability {
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceAvailability::Present => "present",
            SourceAvailability::Missing => "missing",
            SourceAvailability::Unavailable => "unavailable",
        }
    }
}

/// One parsed conversation message, before NoEnding assigns identity and
/// sequence (adapter → core hand-off shape). `provider` / `model` carry only
/// what the source itself confirms about THIS message (Provenance 方案 §12);
/// adapters must never fill them from configuration or branding.
#[derive(Debug, Clone)]
pub struct ParsedSessionMessage {
    pub source_message_id: Option<String>,
    pub source_position: String,
    pub ts: Option<String>,
    pub role: SessionMessageRole,
    pub content: String,
    pub provider: Option<String>,
    pub model: Option<String>,
}

impl ParsedSessionMessage {
    /// Attach source-confirmed provenance to a parsed message (Provenance
    /// 方案 §26). The minimal normalization allowed (§18): trim whitespace,
    /// empty string → `None`. No alias remap, no family guessing, no
    /// provider prefixing — the strings stay source-native.
    pub fn with_provenance(mut self, provider: Option<String>, model: Option<String>) -> Self {
        self.provider = normalize_provenance(provider);
        self.model = normalize_provenance(model);
        self
    }
}

/// `trim` + empty→`None` — the only provenance normalization the domain allows.
pub fn normalize_provenance(v: Option<String>) -> Option<String> {
    let v = v?.trim().to_string();
    (!v.is_empty()).then_some(v)
}

/// Listing scope for Sessions (方案 §11). Default projections (Sessions page,
/// Project / Workstream / Home) show Active only; the recycle bin queries
/// Trash directly from the DB, not through FTS.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SessionListScope {
    #[default]
    Active,
    Trash,
    All,
}

impl SessionListScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionListScope::Active => "active",
            SessionListScope::Trash => "trash",
            SessionListScope::All => "all",
        }
    }

    pub fn parse(s: &str) -> SessionListScope {
        match s {
            "trash" => SessionListScope::Trash,
            "all" => SessionListScope::All,
            _ => SessionListScope::Active,
        }
    }
}

/// File state observed by the adapter after reading one member's source.
#[derive(Debug, Clone, Default)]
pub struct SourceCursorUpdate {
    pub file_identity: String,
    pub generation: i64,
    pub byte_offset: u64,
    pub last_seen_size: u64,
    pub mtime: Option<f64>,
    /// Where this read BATCH started in the file: 0 means a full re-scan
    /// (identity chain restarts from genesis), >0 an append (chain continues
    /// from the last stored event).
    pub start_byte_offset: u64,
    /// SHA-256 of file bytes [0..byte_offset] after this read.
    pub prefix_hash: String,
}

/// A directory whose Agent sessions may be ingested. The standard agent
/// data roots (~/.codex, ~/.claude, ~/.pi, honoring env overrides) are
/// seeded as DISABLED defaults; whether any source is ingested is always
/// the user's decision. Users may add arbitrary custom roots.
#[derive(Debug, Clone, Serialize)]
pub struct IngestSource {
    pub id: Id,
    pub agent: Agent,
    pub path: String,
    pub enabled: bool,
    pub origin: String, // default | user
    pub created_at: String,
}

pub mod ingest_origin {
    pub const DEFAULT: &str = "default";
    pub const USER: &str = "user";
}

/// Stable, recoverable record of "user launched an agent and chose this
/// Workstream" — created before the agent session id is knowable, matched
/// when discovery finds the new external session (方案 §15.1).
#[derive(Debug, Clone, Serialize)]
pub struct LaunchIntent {
    pub id: Id,
    pub launch_type: String, // new | resume
    pub agent: Agent,
    pub owner_workstream_id: Option<Id>,
    pub cwd: Option<String>,
    pub context_bundle_markdown: Option<String>,
    /// JSON snapshot of what the launched session actually received:
    /// `{"bundle_id": "...", "workstream_id": "...", "revisions": [...],
    /// "conflicts": [...]}`. One bundle belongs to one Workstream, so the ids
    /// are plain lists. Recorded as a ContextDelivery row when the intent
    /// matches a session, so the first resume computes a true delta instead of
    /// re-sending the full context.
    pub context_bundle_revisions: Option<String>,
    pub process_id: Option<u32>,
    pub launched_at: String,
    pub matched_session_id: Option<Id>,
    pub status: String, // pending | matched | ambiguous | expired | failed
    pub note: String,
    pub created_at: String,
    pub updated_at: String,
}

pub mod launch_status {
    pub const PENDING: &str = "pending";
    pub const MATCHED: &str = "matched";
    pub const AMBIGUOUS: &str = "ambiguous";
    pub const EXPIRED: &str = "expired";
    pub const FAILED: &str = "failed";
}

/// Where a piece of context came from and how strongly it is held. The values
/// are the domain's own vocabulary: an authority is either known (the five
/// origins below) or [`authority::UNKNOWN`], which says "this revision's origin
/// cannot be determined" — a normal outcome, not a compatibility case.
pub mod authority {
    pub const USER_EXPLICIT: &str = "user_explicit";
    pub const USER_EDIT: &str = "user_edit";
    pub const SYSTEM_OBSERVED: &str = "system_observed";
    pub const AGENT_STATEMENT: &str = "agent_statement";
    pub const AGENT_INFERRED: &str = "agent_inferred";
    /// The revision's provenance does not determine an authority. Callers must
    /// never read this as permission to overwrite: it is the absence of a
    /// statement, and the unified policy treats it as the weakest tier.
    pub const UNKNOWN: &str = "unknown";
}

/// L2/L1 unit of Workstream context. History lives in revisions.
#[derive(Debug, Clone, Serialize)]
pub struct ContextItem {
    pub id: Id,
    pub workstream_id: Id,
    pub kind: String,
    pub status: String,     // active | superseded | resolved | obsolete | deleted
    pub authority: String,  // see `authority`
    pub created_by: String, // user | sync:<runtime> | assistant | ...
    pub current_revision_id: Option<Id>,
    pub supersedes_item_id: Option<Id>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextItemEditPayload {
    pub title: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextItemRevision {
    pub id: Id,
    pub item_id: Id,
    pub title: String,
    pub content: String,
    pub metadata: serde_json::Value,
    pub source_type: Option<String>, // user_edit | session_message | manual | system | status_change
    pub source_ref: Option<String>,  // e.g. "session-message:<message-id>" or "url:..."
    pub sync_run_id: Option<Id>,
    pub created_at: String,
}

/// First-class conflict between two context items. Conflicts are kept, not
/// auto-resolved; the older user-side item stays untouched until a human or
/// stronger evidence resolves the conflict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextConflict {
    pub id: Id,
    pub workstream_id: Id,
    pub left_item_id: Id,          // usually the pre-existing (often user) side
    pub right_item_id: Option<Id>, // the incoming agent side, when it became an item
    pub conflict_type: String,     // authority | content | value
    pub status: String,            // open | resolved | dismissed
    pub resolution: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub left_revision_id: Option<Id>,
    pub right_revision_id: Option<Id>,
    pub candidate_snapshot_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextConflictEvent {
    pub id: Id,
    pub conflict_id: Id,
    pub previous_status: String,
    pub new_status: String,
    pub resolution: Option<String>,
    pub actor: String,
    pub created_at: String,
    pub snapshot_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionSnapshot {
    pub id: Id,
    pub item_id: Id,
    pub title: String,
    pub content: String,
    pub authority: String,
    pub created_at: String,
    pub source_ref: Option<String>,
    pub source_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateSnapshot {
    pub title: String,
    pub content: String,
    pub authority: String,
    pub source_refs: Vec<String>,
}

/// Read model for conflict review guaranteeing user review sees exact
/// conflict-time evidence alongside current fact state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictReviewCase {
    pub conflict: ContextConflict,

    pub left_at_conflict: Option<RevisionSnapshot>,
    pub right_at_conflict: Option<RevisionSnapshot>,
    pub candidate_at_conflict: Option<CandidateSnapshot>,

    pub current_left: Option<RevisionSnapshot>,
    pub current_right: Option<RevisionSnapshot>,

    pub left_changed_since_conflict: bool,
    pub right_changed_since_conflict: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextItemRef {
    pub id: Id,
    pub kind: String,
    pub title: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextItemRelation {
    pub item_id: Id,
    pub supersedes: Option<ContextItemRef>,
    pub superseded_by: Vec<ContextItemRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextChange {
    pub id: String,
    pub item_id: Option<Id>,
    pub conflict_id: Option<Id>,
    pub kind: String, // added | edited | resolved | superseded | deleted | conflict_created | conflict_resolved
    pub title: String,
    pub actor: String,
    pub source_type: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSourceDetail {
    pub revision_id: Id,
    pub authority: String,
    pub source_type: Option<String>,
    pub source_ref: Option<String>,
    pub sync_run_id: Option<Id>,
    pub session_id: Option<Id>,
    pub session_title: Option<String>,
    pub agent: Option<Agent>,
    pub message_sequence: Option<i64>,
    pub message_ts: Option<String>,
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncRun {
    pub id: Id,
    pub session_id: Id,
    /// SessionMessage sequence bounds of the processed delta.
    pub from_sequence: i64,
    pub to_sequence: i64,
    pub status: String, // ok | partial | error
    pub mutations: serde_json::Value,
    pub summary: String,
    pub error: Option<String>,
    pub created_at: String,
    /// Which extractor produced the mutations: heuristic | cli:<agent>:<model>
    /// | cli:<...>->heuristic (fallback) | none.
    pub runtime: String,
    /// Fingerprint of the processed delta (message ids). A completed run with
    /// the same fingerprint is never re-applied (idempotent retries).
    pub delta_fingerprint: Option<String>,
}

/// Snapshot of what a session was actually delivered, recorded only after a
/// successful launch. Resume deltas are computed against this, not against
/// wall-clock timestamps.
#[derive(Debug, Clone, Serialize)]
pub struct ContextDelivery {
    pub id: Id,
    pub session_id: Id,
    pub workstream_id: Id,
    pub bundle_id: Id,
    pub delivered_revisions: Vec<Id>,
    pub delivered_conflicts: Vec<Id>,
    pub delivered_at: String,
}

/// Item types that make up the L1 Core Context projection.
pub const CORE_ITEM_TYPES: [&str; 5] = [
    "goal",
    "current_state",
    "constraint",
    "decision",
    "open_question",
];

/// Built-in domain-neutral extended types (first version).
pub const BUILTIN_EXTENDED_TYPES: [&str; 10] = [
    "todo",
    "finding",
    "issue",
    "risk",
    "note",
    "reference",
    "artifact",
    "requirement",
    "decision_detail",
    "research_note",
];

/// Review Frontier marking the latest observed context mutation boundary for a Workstream.
/// Ordering comparison relies strictly on local mutation commit time (`through_at`),
/// with `boundary_change_ids` resolving any ties at the frontier timestamp losslessly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewFrontier {
    pub through_at: String,
    pub boundary_change_ids: Vec<Id>,
}

/// Durable review checkpoint for a Workstream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkstreamReviewState {
    pub workstream_id: Id,
    pub frontier: ReviewFrontier,
    pub reviewed_at: String,
}

/// Window token presented to the user during context review.
/// Guarantees that marking reviewed only advances through the changes actually observed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkstreamReviewWindow {
    pub state: WorkstreamReviewState,
    pub unseen_changes: Vec<ContextChange>,
    pub mark_through: ReviewFrontier,
}

/// Lightweight summary of context updates and attention items for a Workstream since its last review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkstreamReviewSummary {
    pub workstream_id: Id,
    pub unseen_change_count: usize,
    pub open_conflict_count: usize,
    pub new_facts: usize,
    pub updated_facts: usize,
    pub resolved_items: usize,
    pub superseded_items: usize,
    pub last_unseen_change_at: Option<String>,
    pub reviewed_at: String,
    pub has_updates: bool,
    pub needs_attention: bool,
}

#[cfg(test)]
mod agent_wire_spelling_tests {
    use super::*;

    /// The Agent enum has THREE spellings to keep in step: `as_str()` (the DB
    /// column and every SQL literal), `parse` (what we read back), and serde
    /// (the Tauri IPC boundary the frontend sees). They are hand-written in two
    /// places and derived in the third, so the only way they stay equal is a
    /// test. `WorkBuddy` and `ZCode` are where this broke: `rename_all =
    /// "snake_case"` spelled them `work_buddy` / `z_code` while everything else
    /// — the sessions table, the frontend's `Agent` union, `AGENT_LABELS` —
    /// spells them `workbuddy` / `zcode`, so those rows rendered as「未知 Agent」
    /// and every command taking an `agent` argument failed to deserialize
    /// (方案 §37.14).
    #[test]
    fn serde_spelling_equals_as_str_for_every_agent() {
        for agent in Agent::all() {
            let wire = serde_json::to_value(agent).unwrap();
            assert_eq!(
                wire.as_str(),
                Some(agent.as_str()),
                "{}: the wire spelling must be as_str(), or the frontend cannot label it",
                agent.display_name()
            );
        }
    }

    /// Whatever we emit, we must be able to read back — a command that takes an
    /// `agent` argument receives exactly this string.
    #[test]
    fn every_emitted_spelling_deserializes_back_to_the_same_agent() {
        for agent in Agent::all() {
            let wire = serde_json::to_string(agent).unwrap();
            let back: Agent = serde_json::from_str(&wire).unwrap();
            assert_eq!(back, *agent, "round trip failed for {wire}");
        }
    }

    /// `parse` is the third spelling beside `as_str` and serde: it also accepts
    /// the short CLI aliases, and must keep accepting the canonical one or a
    /// stored row could stop round-tripping.
    #[test]
    fn parse_accepts_the_canonical_spelling_of_every_agent() {
        for agent in Agent::all() {
            assert_eq!(Agent::parse(agent.as_str()), Some(*agent));
        }
    }
}
