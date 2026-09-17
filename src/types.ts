// Mirror of the Rust domain API shapes (commands.rs).

export type Agent = "codex" | "claude_code" | "pi";

export interface Project {
  id: string;
  name: string;
  description: string;
  archived: boolean;
  created_at: string;
  updated_at: string;
}

export interface ProjectResource {
  id: string;
  project_id: string;
  kind: string;
  uri: string | null;
  metadata: Record<string, unknown>;
  created_at: string;
}

export interface Workstream {
  id: string;
  project_id: string | null;
  title: string;
  description: string;
  lifecycle: "open" | "completed" | "abandoned";
  visibility: "normal" | "archived";
  /** 该 Workstream 的 New Session 默认启动目录（启动建议，非身份）。 */
  default_cwd: string | null;
  created_at: string;
  updated_at: string;
}

export interface LatestSessionInfo {
  id: string;
  agent: Agent;
}

/** Binding row with workstream title (backend: list_session_bindings). */
export interface SessionBindingRow {
  session_id: string;
  workstream_id: string;
  role: string;
  workstream_title: string;
}

/** Paths for Settings → Data & Advanced (backend: get_app_info). */
export interface AppInfo {
  db_path: string;
  app_data_dir: string;
}

/** Card view for Home / Workstreams pages (backend: list_workstream_cards). */
export interface WorkstreamCardData {
  id: string;
  project_id: string | null;
  title: string;
  description: string;
  lifecycle: "open" | "completed" | "abandoned";
  visibility: "normal" | "archived";
  default_cwd: string | null;
  created_at: string;
  updated_at: string;
  project_name: string | null;
  current_state: string | null;
  goal: string | null;
  last_activity_at: string | null;
  session_count: number;
  latest_session: LatestSessionInfo | null;
}

export interface Session {
  id: string;
  agent: Agent;
  agent_session_id: string;
  title: string | null;
  cwd: string | null;
  project_id: string | null;
  raw_path: string;
  parent_agent_session_id: string | null;
  started_at: string | null;
  last_activity_at: string | null;
}

export interface SessionEvent {
  session_id: string;
  sequence: number;
  ts: string | null;
  kind: string;
  text: string | null;
  raw_ref: string;
  metadata: Record<string, unknown>;
}

export interface ContextItem {
  id: string;
  workstream_id: string;
  kind: string;
  status: string;
  authority: string;
  current_revision_id: string | null;
  supersedes_item_id: string | null;
  created_at: string;
  updated_at: string;
}

export interface ContextItemRevision {
  id: string;
  item_id: string;
  title: string;
  content: string;
  metadata: Record<string, unknown>;
  source_type: string | null;
  source_ref: string | null;
  sync_run_id: string | null;
  created_at: string;
}

export interface IngestSource {
  id: string;
  agent: Agent;
  path: string;
  enabled: boolean;
  origin: "default" | "user";
  created_at: string;
  exists: boolean;
}

export interface SessionWorkstreamBinding {
  session_id: string;
  workstream_id: string;
  role: string;
  last_seen_revision: string | null;
  last_sync_cursor: number;
  created_at: string;
  last_used_at: string;
}

export interface SyncRun {
  id: string;
  session_id: string;
  from_sequence: number;
  to_sequence: number;
  status: string;
  mutations: ContextMutation[];
  summary: string;
  error: string | null;
  created_at: string;
  runtime: string;
}

export type ContextMutation =
  | { op: "add"; workstream_id: string; item_kind: string; title: string; content: string; source_ref: string; authority: string }
  | { op: "update"; item_id: string; title: string; content: string; source_ref: string; authority: string }
  | { op: "supersede"; item_id: string; title: string; content: string; source_ref: string; authority: string }
  | { op: "resolve"; item_id: string; source_ref: string }
  | { op: "create_workstream"; project_id: string; title: string; reason: string }
  | { op: "conflict"; workstream_id: string; item_id: string; title: string; content: string; source_ref: string; reason: string };

export interface ContextSection {
  kind: string;
  title: string;
  content: string;
  authority: string;
  source_ref: string | null;
  workstream_id?: string | null;
  revision_id?: string | null;
  conflict_id?: string | null;
}

export interface ContextConflict {
  id: string;
  workstream_id: string;
  left_item_id: string;
  right_item_id: string | null;
  conflict_type: string;
  status: string; // "open" | "resolved" | "dismissed"
  resolution: string | null;
  created_at: string;
  updated_at: string;
  left_revision_id?: string | null;
  right_revision_id?: string | null;
  candidate_snapshot_json?: string | null;
}

export interface ContextConflictEvent {
  id: string;
  conflict_id: string;
  previous_status: string;
  new_status: string;
  resolution: string | null;
  actor: string;
  created_at: string;
  snapshot_json?: string | null;
}

export interface ContextItemEditPayload {
  title: string;
  content: string;
}

export interface ContextItemRef {
  id: string;
  kind: string;
  title: string;
  status: string;
}

export interface ContextItemRelation {
  item_id: string;
  supersedes: ContextItemRef | null;
  superseded_by: ContextItemRef[];
}

export interface ContextChange {
  id: string;
  item_id: string | null;
  conflict_id: string | null;
  kind:
    | "added"
    | "edited"
    | "resolved"
    | "superseded"
    | "deleted"
    | "conflict_created"
    | "conflict_resolved";
  title: string;
  actor: string;
  source_type: string | null;
  created_at: string;
}

export interface ContextSourceDetail {
  revision_id: string;
  authority: string;
  source_type: string | null;
  source_ref: string | null;
  sync_run_id: string | null;
  session_id: string | null;
  session_title: string | null;
  agent: Agent | null;
  event_sequence: number | null;
  event_ts: string | null;
  evidence: string | null;
}

export interface WorkstreamContext {
  workstream: Workstream;
  project_name: string | null;
  core: ContextSection[];
  items: [ContextItem, ContextItemRevision][];
  related_sessions: Session[];
  conflicts: ContextConflict[];
  relations: ContextItemRelation[];
  recent_changes: ContextChange[];
}

export interface SessionDetail {
  session: Session;
  events: SessionEvent[];
  bindings: [SessionWorkstreamBinding, string | null][];
  cursor: number;
}

export interface SearchHit {
  kind: string;
  ref_id: string;
  parent_id: string;
  title: string;
  snippet: string;
  rank: number;
}

export type ContextDeliveryLevel =
  | "off"
  | "compact"
  | "balanced"
  | "detailed";

export interface SessionContextBundle {
  bundle_id?: string;
  mode: string;
  delivery_level: ContextDeliveryLevel;
  workstream_ids: string[];
  sections: ContextSection[];
  markdown: string;
  approx_tokens: number;
}

export interface PreparedLaunch {
  id: string;
  mode: "new" | "resume";
  agent: Agent;
  session_id?: string | null;
  workstream_ids: string[];
  extra_workstream_ids: string[];
  cwd?: string | null;
  delivery_level: ContextDeliveryLevel;
  bundle: SessionContextBundle;
  state_fingerprint: string;
  prepared_at: string;
}

export interface LaunchResult {
  launched_via: string;
  command_line: string;
  context_file: string;
  bundle: SessionContextBundle;
  note: string;
}

export interface AssistantMessage {
  id: string;
  session_id: string;
  role: "user" | "assistant";
  content: string;
  action_json: string | null;
  runtime: string | null;
  created_at: string;
}

export const AGENT_LABELS: Record<Agent, string> = {
  codex: "Codex",
  claude_code: "Claude Code",
  pi: "Pi",
};

export const KIND_LABELS: Record<string, string> = {
  goal: "Goal",
  current_state: "Current State",
  constraint: "Constraint",
  decision: "Decision",
  open_question: "Open Question",
  todo: "Todo",
  finding: "Finding",
  issue: "Issue",
  risk: "Risk",
  note: "Note",
  reference: "Reference",
  artifact: "Artifact",
  requirement: "Requirement",
  decision_detail: "Decision Detail",
  research_note: "Research Note",
};

export const AUTHORITY_LABELS: Record<string, string> = {
  user_explicit: "用户明确",
  user_edit: "用户编辑",
  system_observed: "系统观察",
  agent_statement: "Agent 陈述",
  agent_inferred: "Agent 推断",
};
