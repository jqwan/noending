//! Platform-agnostic domain model.
//!
//! Nothing here may depend on a specific Agent's data format.
//!
//! Workspace Domain v0.2 changed one long-standing rule deliberately: a
//! physical working path IS now domain state (`WorkspacePath`, and the
//! `workstream_paths` ordered list that replaced `default_cwd`). What did not
//! change: an Agent's raw transcript layout, and the fact that a Workstream is
//! not a path — it *has* an ordered list of paths, and a Session keeps its own
//! authoritative cwd.
//!
//! Environment data (cwd etc.) is still optional metadata carried by Session:
//! a Session with no cwd is legal and gets no WorkspacePath (v0.2 never
//! fabricates one from a default).

use serde::{Deserialize, Serialize};

pub type Id = String;

/// A physical workspace family, maintained entirely by the app.
///
/// Users never create, delete, or pick a Project — they may only rename it
/// (`name_customized`). A Project exists exactly while it owns at least one
/// [`WorkspacePath`]; the last path being deleted or reassigned deletes it.
/// `git_id` is the optional Git anchor; `None` means the Project is currently
/// only anchored by its path(s), which is a normal state, not a degraded one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: Id,
    pub name: String,
    pub description: String,
    /// Compatibility column, no longer domain semantics: v0.2 has no Project
    /// lifecycle, so nothing writes 1 and `list_projects` does not filter on it.
    pub archived: bool,
    /// References `git_identities.id`. UNIQUE across Projects when set.
    #[serde(default)]
    pub git_id: Option<Id>,
    /// A user renamed this Project: automatic naming must stop overwriting it,
    /// including after a Project merge or worktree discovery.
    #[serde(default)]
    pub name_customized: bool,
    pub created_at: String,
    pub updated_at: String,
}

impl Project {
    /// Legacy / test constructor: path-backed, app-named.
    pub fn new(id: Id, name: impl Into<String>) -> Self {
        let ts = crate::storage::now();
        Self {
            id,
            name: name.into(),
            description: String::new(),
            archived: false,
            git_id: None,
            name_customized: false,
            created_at: ts.clone(),
            updated_at: ts,
        }
    }
}

/// Optional resource attached to a Project.
///
/// Since v0.2 this table carries NO workspace identity: `repository` /
/// `workspace` rows are plain user notes, and the WorkspacePath registry is
/// the only path authority. The commands that let users add or remove rows
/// left the product API, so this is read-only legacy data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectResource {
    pub id: Id,
    pub project_id: Id,
    pub kind: String, // repository | workspace | file | document | url | artifact | external
    pub uri: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: String,
}

/// One observed physical working path — the bridge between the filesystem and
/// every Project fact in the system.
///
/// Identity: `id` IS the path identity, derived deterministically from
/// `canonical_path` (see `workspace::path_identity`) rather than being a random
/// UUID, so re-ensuring the same path is idempotent by construction and the
/// v12 migration can be replayed safely. Git presence does not affect it: the
/// same directory stays the same WorkspacePath across `.git` appearing,
/// disappearing, or `git init` running again.
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
    /// Shared constructor for fixtures and migration code. The id is computed by
    /// the same identity function production uses, so a fixture can never
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
/// move; v0.2 only promises "same recognized common dir → same git_id".
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
/// one fact is how `default_cwd` and `workstreams.project_id` drifted apart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkstreamPath {
    pub id: Id,
    pub workstream_id: Id,
    pub workspace_path_id: Id,
    pub position: i64,
    pub source: String, // user | session | launch | migration
    pub created_at: String,
}

pub mod workstream_path_source {
    pub const USER: &str = "user";
    pub const SESSION: &str = "session";
    pub const LAUNCH: &str = "launch";
    pub const MIGRATION: &str = "migration";
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
    /// Deprecated since v0.2: Workstream→Project membership comes from
    /// `workstream_paths → workspace_paths.project_id`. Kept as a frozen
    /// compatibility read — `upsert_workstream_conn` no longer writes it.
    pub project_id: Option<Id>,
    pub title: String,
    pub description: String,
    pub lifecycle: String,  // active | completed
    pub visibility: String, // normal | archived
    /// Deprecated since v0.2: replaced by the ordered `workstream_paths` list.
    /// Now only a v12 migration input and a compatibility read.
    #[serde(default)]
    pub default_cwd: Option<String>,
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
    /// which is why lifecycle / paths / bindings survive a round trip.
    pub const ARCHIVED: &str = "archived";
}

impl Workstream {
    /// Legacy / test constructor: no paths, active, visible.
    pub fn new(id: Id, title: impl Into<String>) -> Self {
        let ts = crate::storage::now();
        Self {
            id,
            project_id: None,
            title: title.into(),
            description: String::new(),
            lifecycle: workstream_lifecycle::ACTIVE.into(),
            visibility: workstream_visibility::NORMAL.into(),
            default_cwd: None,
            created_at: ts.clone(),
            updated_at: ts,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Agent {
    Codex,
    ClaudeCode,
    Pi,
}

impl Agent {
    pub fn all() -> [Agent; 3] {
        [Agent::Codex, Agent::ClaudeCode, Agent::Pi]
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Agent::Codex => "Codex",
            Agent::ClaudeCode => "Claude Code",
            Agent::Pi => "Pi",
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Agent::Codex => "codex",
            Agent::ClaudeCode => "claude_code",
            Agent::Pi => "pi",
        }
    }

    pub fn parse(s: &str) -> Option<Agent> {
        match s {
            "codex" => Some(Agent::Codex),
            "claude_code" | "claude" => Some(Agent::ClaudeCode),
            "pi" => Some(Agent::Pi),
            _ => None,
        }
    }
}

/// A normalized, agent-agnostic record of one external Session.
/// Session may have no cwd / repository / workspace at all.
///
/// Workspace facts on a Session (v0.2):
/// - `workspace_path_id` is the authoritative link to the physical workspace;
///   it is set only when the Session really has a cwd. A Session without one
///   stays `None` — the default workspace is a *launch* convenience and is
///   never retrofitted onto historical Sessions.
/// - `project_id` is a **derived cache** of
///   `workspace_path_id → workspace_paths.project_id`. It is written by exactly
///   two code paths (see `storage::session_paths`); a manual write into it is a
///   domain violation, not a convenience.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: Id, // internal stable id (app-owned)
    pub agent: Agent,
    pub agent_session_id: String,
    pub title: Option<String>,
    pub cwd: Option<String>,
    #[serde(default)]
    pub workspace_path_id: Option<Id>,
    pub project_id: Option<Id>,
    pub raw_path: String,
    pub parent_agent_session_id: Option<String>,
    pub started_at: Option<String>,
    pub last_activity_at: Option<String>,
}

impl Session {
    /// Legacy / test constructor: an agent-owned Session with no workspace facts.
    pub fn new(
        id: Id,
        agent: Agent,
        agent_session_id: impl Into<String>,
        raw_path: impl Into<String>,
    ) -> Self {
        Self {
            id,
            agent,
            agent_session_id: agent_session_id.into(),
            title: None,
            cwd: None,
            workspace_path_id: None,
            project_id: None,
            raw_path: raw_path.into(),
            parent_agent_session_id: None,
            started_at: None,
            last_activity_at: None,
        }
    }
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
    pub kind: String, // user_message | assistant_message | tool_call | tool_result | compact | system | artifact | unknown
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
    /// Empty on legacy cursors (forces one full re-scan to backfill).
    pub prefix_hash: String,
    /// Identity hash of the last event on the CURRENT source chain — the
    /// base the next append batch continues from. This lives on the cursor,
    /// not "last event in the store": after a compact + dedup re-scan the
    /// store's tail is NEWER than the source's tail (old events are kept,
    /// append-only), and continuing the chain from it would make the next
    /// append invisible to the following re-scan. Empty on legacy cursors
    /// (falls back to the last stored event until the next full re-scan).
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

/// Multi-to-multi binding between sessions and workstreams.
///
/// `source` records who established the binding. Bindings created from an
/// explicit launch selection (`explicit_launch_selection`, confidence 1.0)
/// must never be replaced by automatic classification.
///
/// `workstream_path_id` (v0.2) records *which WorkstreamPath brought this
/// Session in*: the Session's own `workspace_path_id`, which must be one of
/// the Workstream's `workstream_paths` rows (exact match, never prefix
/// matching). `None` means "unknown / legacy / drifted", and such a binding is
/// deliberately NOT removed when a WorkstreamPath is deleted — deleting a path
/// may not silently unbind Sessions we cannot prove came from it.
#[derive(Debug, Clone, Serialize)]
pub struct SessionWorkstreamBinding {
    pub session_id: Id,
    pub workstream_id: Id,
    pub role: String,   // primary | related
    pub source: String, // explicit_launch_selection | user_assigned | automatic_classification
    pub confidence: f64,
    #[serde(default)]
    pub workstream_path_id: Option<Id>,
    pub last_seen_revision: Option<String>,
    pub last_sync_cursor: i64,
    pub created_at: String,
    pub last_used_at: String,
}

pub mod binding_source {
    pub const EXPLICIT_LAUNCH: &str = "explicit_launch_selection";
    pub const USER_ASSIGNED: &str = "user_assigned";
    pub const AUTO: &str = "automatic_classification";
}

/// Derived classification state of a session (Domain §Session).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionClassificationState {
    Unassigned,
    PartiallyAssigned,
    Assigned,
}

impl SessionClassificationState {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionClassificationState::Unassigned => "unassigned",
            SessionClassificationState::PartiallyAssigned => "partially_assigned",
            SessionClassificationState::Assigned => "assigned",
        }
    }

    /// Derived projection: explicit bindings (user / launch selection) mean
    /// assigned; only automatic candidate bindings mean partially assigned;
    /// nothing at all means unassigned.
    pub fn derive(bindings: &[SessionWorkstreamBinding]) -> SessionClassificationState {
        if bindings.is_empty() {
            return SessionClassificationState::Unassigned;
        }
        let has_explicit = bindings.iter().any(|b| b.source != binding_source::AUTO);
        if has_explicit {
            SessionClassificationState::Assigned
        } else {
            SessionClassificationState::PartiallyAssigned
        }
    }
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

/// Stable, recoverable record of "user launched an agent and chose these
/// Workstreams" — created before the agent session id is knowable, matched
/// when discovery finds the new external session.
#[derive(Debug, Clone, Serialize)]
pub struct LaunchIntent {
    pub id: Id,
    pub launch_type: String, // new | resume
    pub agent: Agent,
    pub selected_workstream_ids: Vec<Id>,
    pub cwd: Option<String>,
    pub context_bundle_markdown: Option<String>,
    /// JSON snapshot of what the launched session actually received:
    /// `{"bundle_id": "...", "by_workstream": {ws_id: [revision_id, ...]}}`.
    /// Recorded as ContextDelivery rows when the intent matches a session,
    /// so the first resume computes a true delta instead of re-sending
    /// the full context.
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

/// L2/L1 unit of Workstream context. History lives in revisions.
#[derive(Debug, Clone, Serialize)]
pub struct ContextItem {
    pub id: Id,
    pub workstream_id: Id,
    pub kind: String,
    pub status: String,     // active | superseded | resolved | obsolete | deleted
    pub authority: String, // user_explicit | user_edit | system_observed | agent_statement | agent_inferred
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
    #[serde(default)]
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

/// Evidence that a session/workstream belongs to a Project. cwd / repo path
/// are *evidence*, never the Project identity itself; the resolver scores
/// evidence and only suggests.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectAffinityEvidence {
    pub id: Id,
    pub session_id: Option<Id>,
    pub workstream_id: Option<Id>,
    pub project_id: Id,
    pub evidence_type: String, // cwd_match | repository | session_title | history | user_correction
    pub source: String,
    pub score: f32,
    pub created_at: String,
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
