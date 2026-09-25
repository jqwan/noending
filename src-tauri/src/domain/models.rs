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
/// Qoder ships no headless CLI and AutoClaw's CLI cannot be pointed at its own
/// state directory from here, so those adapters ingest history only and
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
    #[serde(rename = "autoclaw")]
    AutoClaw,
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
            Agent::AutoClaw,
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
            Agent::AutoClaw => "AutoClaw",
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
            Agent::AutoClaw => "autoclaw",
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
            "autoclaw" | "openclaw" => Some(Agent::AutoClaw),
            "workbuddy" | "work_buddy" => Some(Agent::WorkBuddy),
            "dsh" | "deepseek_harness" => Some(Agent::Dsh),
            "zcode" | "z_code" => Some(Agent::ZCode),
            _ => None,
        }
    }
}

/// A normalized, agent-agnostic record of one external Session.
/// Session may have no cwd / repository / workspace at all.
///
/// Workspace facts on a Session:
/// - `workspace_path_id` is the authoritative link to the physical workspace;
///   it is set only when the Session really has a cwd. A Session without one
///   stays `None` — the default workspace is a *launch* convenience and is
///   never retrofitted onto historical Sessions.
/// - `project_id` is a **derived cache** of
///   `workspace_path_id → workspace_paths.project_id`. It is written by exactly
///   two code paths (see `storage::session_paths`); a manual write into it is a
///   domain violation, not a convenience.
#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub id: Id, // internal stable id (app-owned)
    pub agent: Agent,
    pub agent_session_id: String,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub workspace_path_id: Option<Id>,
    pub project_id: Option<Id>,
    /// Semantic ownership: the one Workstream this execution belongs to, or
    /// `None`. A Session has at most one Owner Workstream (方案 §3.3); there is
    /// no primary/related pair and no M:N relation. Physical Project membership
    /// (above) and this semantic owner are independent and never mutate each
    /// other (方案 §4, §5).
    pub owner_workstream_id: Option<Id>,
    pub raw_path: String,
    pub parent_agent_session_id: Option<String>,
    pub started_at: Option<String>,
    pub last_activity_at: Option<String>,
    /// Session lifecycle authority. `None` = Normal; `Some(ts)` = Trash. This
    /// is the single lifecycle authority — there is no separate visibility
    /// flag. Trashing is reversible (Restore keeps the same Session id) and
    /// never touches the Agent source file; permanent deletion removes the row
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

/// Transient coordination row for a prepared permanent Session deletion
/// (方案 §4). Permanent deletion spans SQLite + the filesystem, so the frozen
/// `SourceDeletionPlan` is stored here between prepare and execute.
///
/// This is NOT a tombstone / deletion history / blacklist: on success the job
/// row is deleted in the same transaction that purges the Session, so
/// NoEnding retains no record that the Agent session ever existed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDeletionJob {
    pub id: Id,
    pub session_id: Id,
    /// prepared | deleting_source | failed | stale
    pub state: String,
    /// Frozen `adapters::SourceDeletionPlan` (serde JSON).
    pub plan_json: String,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl SessionDeletionJob {
    pub const STATE_PREPARED: &'static str = "prepared";
    pub const STATE_DELETING_SOURCE: &'static str = "deleting_source";
    pub const STATE_FAILED: &'static str = "failed";
    pub const STATE_STALE: &'static str = "stale";
}

/// Normalized session event — the only shape Ingestion/Sync may consume.
///
/// Identity rules (Context Integrity):
/// - `id` is the app-owned stable identity, never derived from the source;
/// - `sequence` is a NoEnding-owned per-session monotonic counter. It is
///   NEVER the JSONL line number and is never reused or rolled back;
/// - `source_generation` / `source_position` / `source_event_id` describe
///   where the event came from in the Agent's (mutable) source file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEvent {
    pub id: Id,
    pub session_id: Id,
    pub sequence: i64,
    pub source_event_id: Option<String>,
    pub source_generation: i64,
    pub source_position: String, // e.g. "line:42"
    pub ts: Option<String>,
    /// Open event-kind discriminator.
    ///
    /// Current adapters produce message / compact / system / artifact events,
    /// but the value intentionally stays a free string so a new Agent event
    /// category does not need a SQLite enum or a schema change.
    pub kind: String, // user_message | assistant_message | compact | system | artifact | unknown
    pub text: Option<String>,
    pub raw_ref: String,
    pub metadata: serde_json::Value,
}

/// Where ingestion stopped reading an Agent source file.
///
/// `source_file_identity` distinguishes the same path being replaced by a
/// different file (inode / creation metadata based). `generation` increases
/// on every detected truncate / rewrite / replacement, so old history is
/// never overwritten — later file states only ADD events.
#[derive(Debug, Clone, Serialize, Default)]
pub struct SourceCursor {
    pub session_id: Id,
    pub source_file_identity: String,
    pub generation: i64,
    pub byte_offset: u64,
    pub last_seen_size: u64,
    pub mtime: Option<f64>,
    /// SHA-256 of the file bytes [0..byte_offset] at the time the offset was
    /// recorded. An append is only accepted when the stored prefix is still
    /// the file's prefix — a same-size-or-larger rewrite is caught here.
    /// Empty until a read has recorded one: without it append-only continuity
    /// cannot be proven, so the source is conservatively re-scanned.
    pub prefix_hash: String,
    /// Identity hash of the last event on the CURRENT source chain — the
    /// base the next append batch continues from. This lives on the cursor,
    /// not "last event in the store": after a compact + dedup re-scan the
    /// store's tail is NEWER than the source's tail (old events are kept,
    /// append-only), and continuing the chain from it would make the next
    /// append invisible to the following re-scan. Empty when the source has
    /// produced no chain event yet; the commit then falls back to the last
    /// stored event identity.
    pub identity_tail_hash: String,
    /// Max app-assigned event sequence ingested so far (read cursor).
    pub last_sequence: i64,
}

/// What a parsed source line looks like before NoEnding assigns identity.
#[derive(Debug, Clone)]
pub struct ParsedEvent {
    pub source_event_id: Option<String>,
    pub source_position: String,
    pub ts: Option<String>,
    pub kind: String,
    pub text: Option<String>,
    pub metadata: serde_json::Value,
}

/// Result of one incremental adapter read. `source` always describes the
/// file state AFTER reading; events carry no sequence yet (storage assigns).
#[derive(Debug, Clone, Default)]
pub struct ReadDelta {
    pub events: Vec<ParsedEvent>,
    pub source: Option<SourceCursorUpdate>,
}

/// File state observed by the adapter after reading.
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
    pub source_type: Option<String>, // user_edit | session_event | manual | system | status_change
    pub source_ref: Option<String>,  // e.g. "session-event:<event-id>" or "url:..."
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
    pub event_sequence: Option<i64>,
    pub event_ts: Option<String>,
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncRun {
    pub id: Id,
    pub session_id: Id,
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
    /// Fingerprint of the processed delta (event ids). A completed run with
    /// the same fingerprint is never re-applied (idempotent retries).
    pub delta_fingerprint: Option<String>,
    pub source_generation: i64,
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
