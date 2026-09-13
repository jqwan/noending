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
    pub lifecycle: String, // open | completed | abandoned
    pub visibility: String, // normal | archived
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
#[derive(Debug, Clone, Serialize)]
pub struct SessionEvent {
    pub session_id: Id,
    pub sequence: i64,
    pub ts: Option<String>,
    pub kind: String, // user_message | assistant_message | tool_call | tool_result | compact | system | artifact | unknown
    pub text: Option<String>,
    pub raw_ref: String,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionCursorState {
    pub session_id: Id,
    pub last_sequence: i64,
    pub last_seen_size: i64,
    pub mtime: Option<f64>,
}

/// Multi-to-multi binding between sessions and workstreams.
#[derive(Debug, Clone, Serialize)]
pub struct SessionWorkstreamBinding {
    pub session_id: Id,
    pub workstream_id: Id,
    pub role: String, // primary | related
    pub last_seen_revision: Option<String>,
    pub last_sync_cursor: i64,
    pub created_at: String,
    pub last_used_at: String,
}

/// L2/L1 unit of Workstream context. History lives in revisions.
#[derive(Debug, Clone, Serialize)]
pub struct ContextItem {
    pub id: Id,
    pub workstream_id: Id,
    pub kind: String,
    pub status: String, // active | superseded | resolved | obsolete | deleted
    pub authority: String, // user_explicit | user_edit | system_observed | agent_statement | agent_inferred
    pub current_revision_id: Option<Id>,
    pub supersedes_item_id: Option<Id>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextItemRevision {
    pub id: Id,
    pub item_id: Id,
    pub title: String,
    pub content: String,
    pub metadata: serde_json::Value,
    pub source_type: Option<String>, // user_edit | session_event | manual | system
    pub source_ref: Option<String>,  // e.g. "session:<id>#<seq>" or "url:..."
    pub sync_run_id: Option<Id>,
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
