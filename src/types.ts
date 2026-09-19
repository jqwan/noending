// Mirror of the Rust domain API shapes (commands.rs).
//
// Workspace Domain v0.2: these shapes are FROZEN by the foundation commit so
// the parallel agents could build against them before the UI caught up. Change
// them only together with the Rust struct they mirror (方案 §11, §15-8).

export type Agent = "codex" | "claude_code" | "pi";

/** v0.2 folded `abandoned` into `completed` and renamed `open` to `active`. */
export type WorkstreamLifecycle = "active" | "completed";
/** `archived` is the recycle bin; there is no other non-normal state. */
export type WorkstreamVisibility = "normal" | "archived";
/** `missing` = used to be Git-backed; it never detaches the path from its Project. */
export type WorkspaceGitState = "none" | "detected" | "missing";
export type WorkstreamPathSource = "user" | "session" | "launch" | "migration";

export interface Project {
  id: string;
  name: string;
  description: string;
  /** Legacy column, no domain semantics: v0.2 Projects have no lifecycle. */
  archived: boolean;
  /** Optional Git anchor (`git_identities.id`); `null` is a normal state. */
  git_id: string | null;
  /** The user renamed it — automatic naming must stop overwriting the name. */
  name_customized: boolean;
  created_at: string;
  updated_at: string;
}

/** A normalized physical working directory. Identity is `id`, a pure lexical
 *  hash of `canonical_path` — never the path string itself. */
export interface WorkspacePath {
  id: string;
  canonical_path: string;
  /** NOT NULL by design: every path belongs to exactly one Project. */
  project_id: string;
  git_state: WorkspaceGitState;
  git_kind: string | null;
  exists: boolean;
  first_seen_at: string;
  last_seen_at: string;
}

/** One entry of a Workstream's ordered working-path list; position 0 is primary. */
export interface WorkstreamPath {
  id: string;
  workstream_id: string;
  workspace_path_id: string;
  position: number;
  source: WorkstreamPathSource;
  created_at: string;
}

/** 方案 §11 Workspace settings surface (backend: get_workspace_settings). */
export interface WorkspaceSettings {
  noending_home: string;
  default_workspace: string;
  pending_home: string | null;
  restart_required: boolean;
  db_path: string;
}

/** 方案 §11 Project detail (backend: get_project_detail). */
export interface ProjectDetailData {
  project: Project;
  workspace_paths: WorkspacePath[];
  workstreams: ProjectWorkstreamRow[];
  sessions: Session[];
}

/** `is_primary` = the Workstream reaches the Project through its position-0 path. */
export interface ProjectWorkstreamRow {
  workstream: Workstream;
  is_primary: boolean;
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
  /** Derived projection of the primary path's Project; never assigned directly. */
  project_id: string | null;
  id: string;
  title: string;
  description: string;
  lifecycle: WorkstreamLifecycle;
  visibility: WorkstreamVisibility;
  /** Frozen at creation (v12 migration input only). Use `workstream_paths`. */
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
  /** Which path brought the Session in; null once it no longer matches the list. */
  workstream_path_id: string | null;
}

/** Paths for Settings → Data & Advanced (backend: get_app_info). */
export interface AppInfo {
  db_path: string;
  app_data_dir: string;
}

/** Card view for Home / Workstreams pages (backend: list_workstream_cards). */
export interface WorkstreamCardData {
  id: string;
  /** Position-0 path projection, not a user assignment (方案 §42.3-M19). */
  project_id: string | null;
  title: string;
  description: string;
  lifecycle: WorkstreamLifecycle;
  visibility: WorkstreamVisibility;
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
  /** Derived cache of `workspace_path_id → workspace_paths.project_id`. */
  project_id: string | null;
  /** Null for a Session with no cwd — v0.2 never fabricates a path. */
  workspace_path_id: string | null;
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

export interface RevisionSnapshot {
  id: string;
  item_id: string;
  title: string;
  content: string;
  authority: string;
  created_at: string;
  source_ref: string | null;
  source_type: string | null;
}

export interface CandidateSnapshot {
  title: string;
  content: string;
  authority: string;
  source_refs?: string[];
}

export interface ConflictReviewCase {
  conflict: ContextConflict;

  left_at_conflict: RevisionSnapshot | null;
  right_at_conflict: RevisionSnapshot | null;
  candidate_at_conflict: CandidateSnapshot | null;

  current_left: RevisionSnapshot | null;
  current_right: RevisionSnapshot | null;

  left_changed_since_conflict: boolean;
  right_changed_since_conflict: boolean;
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
  conflict_cases?: ConflictReviewCase[];
  relations: ContextItemRelation[];
  recent_changes: ContextChange[];
}

export interface ReviewFrontier {
  through_at: string;
  boundary_change_ids: string[];
}

export interface WorkstreamReviewState {
  workstream_id: string;
  frontier: ReviewFrontier;
  reviewed_at: string;
}

export interface WorkstreamReviewWindow {
  state: WorkstreamReviewState;
  unseen_changes: ContextChange[];
  mark_through: ReviewFrontier;
}

export interface WorkstreamReviewSummary {
  workstream_id: string;
  unseen_change_count: number;
  open_conflict_count: number;
  new_facts: number;
  updated_facts: number;
  resolved_items: number;
  superseded_items: number;
  last_unseen_change_at: string | null;
  reviewed_at: string;
  has_updates: boolean;
  needs_attention: boolean;
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
  /** NoEnding 的 override 意图（null = Agent default），与 Launch 完全一致。 */
  runtime: AgentRuntimeOverrides;
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

// ---------- Agent Runtime Configuration (commands.rs §Agent Runtime) ----------

export type RuntimeFieldCapability = "unsupported" | "free_form" | "suggested" | "discoverable";

/** null 一律表示 Agent default：NoEnding 不传该参数，由 Agent 自己决定。 */
export interface AgentRuntimeOverrides {
  model: string | null;
  provider: string | null;
  effort: string | null;
}

/** dynamic = 来自 Agent CLI；suggested = NoEnding 建议值；unavailable = 获取失败。 */
export type ModelSource = "not_loaded" | "dynamic" | "suggested" | "unavailable";

export interface RuntimeModelOption {
  id: string;
  display_name: string | null;
  provider: string | null;
  supported_efforts: string[];
}

export interface AgentRuntimeSettings {
  agent: Agent;
  detected: boolean;
  executable: string | null;
  version: string | null;
  overrides: AgentRuntimeOverrides;
  capabilities: Record<"model" | "provider" | "effort", RuntimeFieldCapability>;
  models: RuntimeModelOption[];
  model_source: ModelSource;
  effort_levels: string[];
  warnings: string[];
}

/** refresh_agent_runtime_options 的结果：不含安装状态，只含建议信息。 */
export type AgentRuntimeDiscovery = Omit<AgentRuntimeSettings,
  "agent" | "detected" | "executable" | "version" | "overrides">;

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
  legacy_unknown: "未知权威 (历史版本)",
};
