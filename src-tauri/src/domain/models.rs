//! Platform-agnostic domain model — nothing here may depend on a specific
//! Agent's data format.
//!
//! Workspace invariant: a physical working path IS domain state
//! (`WorkspacePath`, plus the ordered `workstream_paths` list), while an Agent's
//! raw transcript layout never is. A Session carries cwd as optional metadata: a
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

/// Derived from a Project's Git anchor and working paths; never persisted or
/// inferred from its user-editable name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectKind {
    Git,
    Directory,
    ChatDirectory,
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
    /// Project, and `missing` is how we remember that.
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
    /// repository / a reserved app path.
    None,
    Detected {
        common_dir: String,
        toplevel: Option<String>,
        kind: GitWorktreeKind,
        /// `git worktree list --porcelain` result. Discovery of a worktree
        /// never adds a WorkstreamPath.
        worktrees: Vec<String>,
        /// `(name, url)` remotes from the repository's config, URLs
        /// credential-stripped. A fact read, not a naming guess.
        remotes: Vec<(String, String)>,
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
    pub visibility: String, // normal | archived
    pub created_at: String,
    pub updated_at: String,
}

pub mod workstream_visibility {
    pub const NORMAL: &str = "normal";
    /// Archived tasks cannot be used to create sessions; other data stays live.
    pub const ARCHIVED: &str = "archived";
}

/// Every Agent NoEnding knows how to READ. Not every entry can be launched:
/// Qoder ships no headless CLI, so its adapter ingests history only and
/// `exec_resolver::resolve` fails by design.
///
/// Each variant's serde rename is the IPC spelling and MUST equal `as_str()` —
/// the sessions table, the frontend's `Agent` union and `AGENT_LABELS` all use
/// that same string. It is declared per variant rather than derived: the derive
/// spelled two variants (`work_buddy`, `z_code`) differently from everything
/// else, so those rows rendered as「未知 Agent」and every command taking an
/// `agent` argument failed to deserialize.
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
    #[serde(rename = "antigravity")]
    Antigravity,
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
            Agent::Antigravity,
        ]
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Agent::Codex => "Codex",
            Agent::ClaudeCode => "Claude Code",
            Agent::Pi => "Pi",
            Agent::Qoder => "Qoder",
            Agent::WorkBuddy => "WorkBuddy",
            Agent::Dsh => "DSH",
            Agent::ZCode => "ZCode",
            Agent::Antigravity => "Antigravity",
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
            Agent::Antigravity => "antigravity",
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
            "antigravity" => Some(Agent::Antigravity),
            _ => None,
        }
    }
}

/// A Logical Session: the one user-visible conversation, one root source, one
/// cursor, one fact frontier. Not an Agent thread — it is keyed by the root
/// source's real Resume identity (`root_agent_session_id`). Child/side sources
/// an Agent spawns alongside are recognized at discovery and skipped silently.
///
/// - `workspace_path_id` is the authoritative link to the physical workspace,
///   set only when the source really has a cwd (the default workspace is a
///   *launch* convenience, never retrofitted onto historical Sessions).
/// - `project_id` is a **derived cache** of
///   `workspace_path_id → workspace_paths.project_id`, written by exactly two
///   code paths (see `storage::session_paths`); a manual write is a domain
///   violation.
#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub id: Id, // internal stable id (app-owned)
    pub agent: Agent,
    /// The root source's Agent-side Resume identity. Authority for
    /// LaunchIntent matching and `resume`.
    pub root_agent_session_id: String,
    pub title: Option<String>,
    /// Semantic ownership: the one Workstream this Logical Session belongs to,
    /// or `None`. At most one Owner. Only an explicit user action
    /// or a matched LaunchIntent sets it — never fork inheritance.
    pub owner_workstream_id: Option<Id>,
    pub cwd: Option<String>,
    pub workspace_path_id: Option<Id>,
    pub project_id: Option<Id>,
    /// When this Session is an independently continuable fork, the Logical
    /// Session its source forked from. Lifecycle, Owner, Conversation and
    /// Context stay fully independent of that source; this is a
    /// provenance note, not a parent link in the old sense.
    pub forked_from_session_id: Option<Id>,
    pub started_at: Option<String>,
    /// Last activity observed on the source.
    pub last_activity_at: Option<String>,
    /// Last real user/assistant message time of the conversation.
    pub last_conversation_at: Option<String>,
    /// Archive authority. `None` = unarchived; `Some(ts)` = archived. This
    /// is the single lifecycle authority — there is no separate visibility
    /// flag. Archiving is reversible (unarchiving keeps the same Session id) and
    /// never touches the Agent source; permanent deletion removes the row
    /// entirely.
    pub archived_at: Option<String>,
    // ---- The root source (flattened from the former session_members row) ----
    /// Adapter-owned descriptor of the source shape (`rollout_file`,
    /// `transcript_file`, sqlite store kinds, …).
    pub source_kind: String,
    /// Where the source lives. For file-per-session Agents this is the
    /// transcript path; for shared stores it is the store path.
    pub source_path: String,
    /// Adapter-specific, non-Conversation structural facts (thread_source,
    /// session_dir, …). Never conversation text.
    pub metadata: serde_json::Value,
    // ---- The source cursor (flattened from session_member_cursors) ----
    /// Stable identity of the source file; a change means replacement.
    pub source_file_identity: String,
    /// The source-shape generation: bumped on replace / truncate / rewrite,
    /// never on a proven append.
    pub source_generation: i64,
    pub source_byte_offset: u64,
    pub source_last_seen_size: u64,
    pub source_mtime: Option<f64>,
    /// SHA-256 of the read prefix when the offset was recorded; an append is
    /// only accepted while the stored prefix is still the file's prefix.
    pub source_prefix_hash: String,
    /// Identity hash of the last message on the CURRENT source chain — the
    /// base the next append batch continues from. Empty until a message exists.
    pub source_tail_hash: String,
    // ---- The conversation fact frontier (flattened from session_ingest_state) ----
    /// Fact generation of the CURRENT conversation: raised only when a source
    /// rewrite / truncate / reorder changes the effective conversation.
    pub fact_generation: i64,
    /// How many messages the store holds (the store's own append counter).
    pub latest_message_seq: i64,
}

impl Session {
    /// Archived sessions cannot continue through NoEnding.
    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }

    /// The read cursor a fresh delta read continues from, in the session's
    /// own columns.
    pub fn source_cursor(&self) -> SourceCursor {
        SourceCursor {
            source_file_identity: self.source_file_identity.clone(),
            generation: self.source_generation,
            byte_offset: self.source_byte_offset,
            last_seen_size: self.source_last_seen_size,
            mtime: self.source_mtime,
            prefix_hash: self.source_prefix_hash.clone(),
            identity_tail_hash: self.source_tail_hash.clone(),
        }
    }
}

/// Where reading of a session's source stopped — the read-side view of the
/// session's cursor columns.
#[derive(Debug, Clone, Default)]
pub struct SourceCursor {
    pub source_file_identity: String,
    pub generation: i64,
    pub byte_offset: u64,
    pub last_seen_size: u64,
    pub mtime: Option<f64>,
    pub prefix_hash: String,
    pub identity_tail_hash: String,
}

/// The role of a [`SessionMessage`] — the only two shapes a conversation has.
/// The database CHECK constraint is the last line of defense; adapters must
/// already have dropped everything else.
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

/// One user-visible conversation turn of the root source — the ONLY
/// conversation store NoEnding keeps. Thinking, tool traffic, system /
/// developer prompts, compaction summaries and child/side transcripts never
/// become rows here; every visible prose segment of an assistant turn is kept
/// (no "final answer" guessing).
#[derive(Debug, Clone, Serialize)]
pub struct SessionMessage {
    pub id: Id,
    pub session_id: Id,
    /// NoEnding's own per-session monotonic sequence. Never a source line
    /// number; never reused or rolled back.
    pub sequence: i64,
    pub role: SessionMessageRole,
    pub content: String,
    pub ts: Option<String>,
    /// Assistant rows only: is this the turn's FINAL reply in the current
    /// conversation? Derived from the projection (an assistant message whose
    /// next projected message is not another assistant message), never
    /// ingested; meaningless (false) on user rows.
    pub turn_final: bool,
    // Source provenance.
    pub source_message_id: Option<String>,
    pub source_generation: i64,
    pub source_position: String,
    pub source_identity_hash: String,
    pub raw_ref: String,
}

/// The fixed, system-derived Session Context structure. All four fields are
/// always present (empty lists are legal); extra fields are rejected at the
/// model boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SessionContextFields {
    pub summary_current_state: String,
    pub decisions: Vec<String>,
    pub open_questions: Vec<String>,
    pub next_steps: Vec<String>,
}

/// The current Session Context plus its CAS/frontier state. No row means the
/// Session has never had a summary generated.
#[derive(Debug, Clone, Serialize)]
pub struct SessionContextRecord {
    pub session_id: Id,
    pub fields: SessionContextFields,
    /// CAS guard: a committed update writes `revision + 1`.
    pub revision: i64,
    /// The fact generation this summary was built against.
    pub ingest_generation: i64,
    /// How far into the current-message projection this summary consumed.
    pub processed_through_seq: i64,
    pub updated_at: String,
}

/// One entry of the append-only Session Context history, so a Workstream
/// update can cite `session-context:<session_id>:<revision>`.
#[derive(Debug, Clone, Serialize)]
pub struct SessionContextRevision {
    pub session_id: Id,
    pub revision: i64,
    pub fields: SessionContextFields,
    pub ingest_generation: i64,
    pub processed_through_seq: i64,
    pub created_at: String,
}

/// The Workstream side of the two-level revision scheme.
#[derive(Debug, Clone, Serialize, Default)]
pub struct WorkstreamContextState {
    pub workstream_id: Id,
    /// Guard for every ContextItem write (manual + AI).
    pub context_revision: i64,
    /// Marks manual Context edits / Owner-set / title / description /
    /// ownership changes that require re-synthesis.
    pub input_revision: i64,
    /// The `input_revision` the last successful AI update consumed. A
    /// Workstream is pending when `input_revision > consumed_input_revision`.
    pub consumed_input_revision: i64,
}

/// What a Workstream has already consumed from one Owner Session.
#[derive(Debug, Clone, Serialize, Default)]
pub struct WorkstreamSessionFrontier {
    pub workstream_id: Id,
    pub session_id: Id,
    pub session_context_revision: i64,
    pub ingest_generation: i64,
    pub consumed_through_seq: i64,
}

/// The outcome vocabulary of an explicit Context update command. Every value
/// is a SUCCESS from the caller's point of view; failures are `Err`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextUpdateStatus {
    /// Everything the snapshot covered was written.
    Updated,
    /// Only a prefix of the pending work fit the input budget; the rest stays
    /// pending.
    Partial,
    /// Nothing to do — no new messages. The model was NOT called.
    NoChange,
    /// The world moved under the model call; the user must click again.
    StaleSnapshot,
}

/// Why an explicit Context update failed, so the UI can offer the right retry.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "message")]
pub enum ContextUpdateError {
    /// The Assistant Agent is `none`, the CLI is missing, or its config is
    /// invalid.
    AiUnavailable(String),
    /// The model call itself failed (timeout, non-zero exit, model error).
    ModelCallFailed(String),
    /// The model answered, but the output did not satisfy the contract.
    InvalidOutput(String),
    /// The snapshot changed under the model call.
    ConcurrencyConflict(String),
    /// The smallest possible request would already exceed the input budget.
    InputTooLarge(String),
    /// Storage / IO failure.
    Storage(String),
}

/// Adapter's strict verdict about one member's source. The semantics
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
/// a trashed Session permanently deletable.
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
/// sequence (adapter → core hand-off shape).
#[derive(Debug, Clone)]
pub struct ParsedSessionMessage {
    pub source_message_id: Option<String>,
    pub source_position: String,
    pub ts: Option<String>,
    pub role: SessionMessageRole,
    pub content: String,
}

impl ParsedSessionMessage {
    /// Source position, when the adapter has a stable native one.
    pub fn with_position(mut self, position: String) -> Self {
        self.source_position = position;
        self
    }

    /// Timestamp, when the adapter read one off the source.
    pub fn with_ts(mut self, ts: Option<String>) -> Self {
        self.ts = ts;
        self
    }
}

/// Session board scope defaults to unarchived; task/project associations include both.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SessionListScope {
    #[default]
    Unarchived,
    Archived,
    All,
}

impl SessionListScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionListScope::Unarchived => "unarchived",
            SessionListScope::Archived => "archived",
            SessionListScope::All => "all",
        }
    }

    pub fn parse(s: &str) -> SessionListScope {
        match s {
            "archived" => SessionListScope::Archived,
            "all" => SessionListScope::All,
            _ => SessionListScope::Unarchived,
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
/// data roots (~/.codex, ~/.claude, ~/.pi/agent/sessions, honoring env
/// overrides) are seeded as DISABLED defaults; whether any source is
/// ingested is always the user's decision. Users may add arbitrary custom
/// roots.
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
/// when discovery finds the new external session.
#[derive(Debug, Clone, Serialize)]
pub struct LaunchIntent {
    pub id: Id,
    pub launch_type: String, // new | resume
    pub agent: Agent,
    pub owner_workstream_id: Option<Id>,
    pub cwd: Option<String>,
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
    pub session_id: Option<Id>,
    pub session_title: Option<String>,
    pub agent: Option<Agent>,
    pub message_sequence: Option<i64>,
    pub message_ts: Option<String>,
    pub evidence: Option<String>,
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
    /// (the Tauri IPC boundary). They are hand-written in two places and derived
    /// in the third, so only a test keeps them equal.
    #[test]
    fn agent_spellings_agree_across_storage_and_ipc() {
        for agent in Agent::all() {
            let wire = serde_json::to_value(agent).unwrap();
            assert_eq!(
                wire.as_str(),
                Some(agent.as_str()),
                "{}: the wire spelling must be as_str(), or the frontend cannot label it",
                agent.display_name()
            );
            let back: Agent = serde_json::from_value(wire).unwrap();
            assert_eq!(back, *agent);
            assert_eq!(Agent::parse(agent.as_str()), Some(*agent));
        }
    }
}
