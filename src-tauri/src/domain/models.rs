//! Platform-agnostic domain model.
//!
//! Nothing here may depend on a specific Agent's data format, a filesystem
//! path, a repository or a cwd. Environment data (cwd etc.) is optional
//! metadata carried by Session, never a structural requirement.

use serde::{Deserialize, Serialize};

pub type Id = String;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: Id,
    pub name: String,
    pub description: String,
    pub archived: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Optional resource attached to a Project. A Project with zero resources
/// is a perfectly valid state: repositories / workspaces are conveniences,
/// not identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectResource {
    pub id: Id,
    pub project_id: Id,
    pub kind: String, // repository | workspace | file | document | url | artifact | external
    pub uri: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workstream {
    pub id: Id,
    /// A Workstream may exist standalone before being attached to a Project
    /// (UX rule: New Workstream is a primary action, Project is optional).
    pub project_id: Option<Id>,
    pub title: String,
    pub description: String,
    pub lifecycle: String,  // open | completed | abandoned
    pub visibility: String, // normal | archived
    /// Optional launch-directory suggestion for New Sessions on this
    /// Workstream ("continue where you left off", made explicit by the
    /// user). Same class as ProjectResource.uri: convenience, not identity —
    /// a Workstream is not a path, and Sessions keep their own cwd.
    #[serde(default)]
    pub default_cwd: Option<String>,
    pub created_at: String,
    pub updated_at: String,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: Id, // internal stable id (app-owned)
    pub agent: Agent,
    pub agent_session_id: String,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub project_id: Option<Id>,
    pub raw_path: String,
    pub parent_agent_session_id: Option<String>,
    pub started_at: Option<String>,
    pub last_activity_at: Option<String>,
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
#[derive(Debug, Clone, Serialize)]
pub struct SessionWorkstreamBinding {
    pub session_id: Id,
    pub workstream_id: Id,
    pub role: String,   // primary | related
    pub source: String, // explicit_launch_selection | user_assigned | automatic_classification
    pub confidence: f64,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
