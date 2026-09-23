// Mirror of the Rust domain API shapes (commands.rs).
//
// Workspace Domain v0.2: these shapes are FROZEN by the foundation commit so
// the parallel agents could build against them before the UI caught up. Change
// them only together with the Rust struct they mirror (方案 §11, §15-8).

export type Agent =
  | "codex"
  | "claude_code"
  | "pi"
  | "qoder"
  | "autoclaw"
  | "workbuddy"
  | "dsh"
  | "zcode";

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
  /** Optional Git anchor (`git_identities.id`); `null` is a normal state. */
  git_id: string | null;
  /** The user renamed it — automatic naming must stop overwriting the name. */
  name_customized: boolean;
  created_at: string;
  updated_at: string;
}

/** A normalized physical working directory. Identity is `id`, a pure lexical
 *  hash of `canonical_path` — never the path string itself. */
/** list_project_cards 的一张卡片（Projects Experience v0.2 §4/§8）。
 *  诊断信息（uuid、git_id）不上卡片——那是 Detail 的事。 */
export interface ProjectCardData {
  id: string;
  name: string;
  name_customized: boolean;
  has_git_identity: boolean;
  path_count: number;
  missing_path_count: number;
  primary_workstream_count: number;
  related_workstream_count: number;
  session_count: number;
  representative_paths: string[];
  /** 全部 canonical 路径（搜索面），与展示用的 representative_paths 分开。 */
  search_paths: string[];
  last_activity_at: string | null;
  updated_at: string;
}

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

/** `list_workstream_paths` row: the entry plus the facts the UI shows beside it. */
export interface WorkstreamPathRow extends WorkstreamPath {
  canonical_path: string;
  project_id: string;
  project_name: string | null;
  exists: boolean;
  bound_session_count: number;
}

// ---------------- Path picker (workspace::probe, read-only) ----------------

/** Why the string is not a WorkspacePath right now (`PathProbe.status`）。 */
export type ProbeStatus = "ok" | "unresolvable" | "reserved" | "home";

/** The Project a path would land in. `known: false` = 提交后会新建这个名字。 */
export interface ProjectHint {
  id: string | null;
  name: string | null;
  known: boolean;
}

/** `probe_workspace_path` — advisory read-only preview of what `ensure_path`
 *  would decide. The attacher's decision at create time stays the authority. */
export interface PathProbe {
  raw: string;
  status: ProbeStatus;
  canonical_path: string | null;
  exists: boolean;
  /** "detected" | "none" | "unavailable" — fresh evidence, not the stored row. */
  git_state: "detected" | "none" | "unavailable" | null;
  git_kind: string | null;
  project: ProjectHint | null;
}

/** `list_recent_workspace_paths` — picker candidates. `known: false` rows come
 *  from Session cwd history and are NOT WorkspacePaths yet (方案 §1.7）。 */
export interface RecentWorkspacePath {
  path: string;
  known: boolean;
  exists: boolean;
  project_name: string | null;
  git_state: WorkspaceGitState | null;
  git_kind: string | null;
  last_used_at: string | null;
}

/** One raw string of a create_workstream submission, and what became of it. */
export interface CreatedPathOutcome {
  raw: string;
  accepted: boolean;
  canonical_path: string | null;
  position: number | null;
  project_name: string | null;
  reason: string | null;
}

/** `create_workstream` — the Workstream plus the per-path outcome. Rejected
 *  paths are reported, never guessed into paths (方案 §42.3）。 */
export interface CreateWorkstreamReport {
  workstream: Workstream;
  paths: CreatedPathOutcome[];
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
  /** "explicit_env" | "bootstrap" | "default_home" — why it is there (§11). */
  home_source: string;
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

export interface Workstream {
  id: string;
  title: string;
  description: string;
  lifecycle: WorkstreamLifecycle;
  visibility: WorkstreamVisibility;
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

/** Card view for Home / Workstreams pages (backend: list_workstream_cards). */
export interface WorkstreamCardData {
  id: string;
  /** Position-0 path projection, not a user assignment (方案 §42.3-M19). */
  project_id: string | null;
  title: string;
  description: string;
  lifecycle: WorkstreamLifecycle;
  visibility: WorkstreamVisibility;
  created_at: string;
  updated_at: string;
  project_name: string | null;
  current_state: string | null;
  goal: string | null;
  last_activity_at: string | null;
  session_count: number;
  latest_session: LatestSessionInfo | null;
  /** How many working paths the Workstream has. `0` is a normal state. */
  path_count: number;
  /** The position-0 path's canonical spelling; null = 没有工作路径。 */
  primary_path: string | null;
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
  /** Single lifecycle authority: null = Normal, timestamp = 回收站. */
  trashed_at: string | null;
}

/** session_deletion_jobs row — transient coordination, never a tombstone. */
export interface SessionDeletionJob {
  id: string;
  session_id: string;
  /** prepared | deleting_source | failed | stale */
  state: "prepared" | "deleting_source" | "failed" | "stale";
  plan_json: string;
  last_error: string | null;
  created_at: string;
  updated_at: string;
}

/** One file the permanent deletion will remove (frozen by the adapter). */
export interface SourceDeletionTarget {
  path: string;
  kind: string;
  file_identity: string;
  size: number;
  sha256: string;
}

/** §20 preview returned by prepare_session_permanent_delete. */
export interface PermanentDeletionPreview {
  job_id: string;
  session_id: string;
  session_title: string | null;
  agent: Agent;
  agent_session_id: string;
  source_targets: SourceDeletionTarget[];
  /** prepare 对源文件的结论：verified_present | confirmed_absent | unverified。 */
  source_state: "verified_present" | "confirmed_absent" | "unverified";
  event_count: number;
  binding_count: number;
  sync_run_count: number;
  context_delivery_count: number;
  launch_intent_count: number;
  context_revision_redaction_count: number;
}

/** §21/§22 outcome of execute_session_permanent_delete. */
export interface PermanentDeletionResult {
  purged: boolean;
  redacted_revisions: number;
  job: SessionDeletionJob | null;
  error: string | null;
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

/** Mirror of `sync::ContextMutation` (`#[serde(tag = "op")]`). */
export type ContextMutation =
  | { op: "add"; workstream_id: string; item_kind: string; title: string; content: string; source_refs: string[]; authority: string }
  | { op: "update"; item_id: string; title: string; content: string; source_refs: string[]; authority: string }
  | { op: "supersede"; item_id: string; title: string; content: string; source_refs: string[]; authority: string }
  | { op: "resolve"; item_id: string; source_refs: string[] }
  /** A Project is derived from paths in v0.2, so a discovered Workstream may legitimately have none. */
  | { op: "create_workstream"; title: string; reason: string }
  | { op: "conflict"; workstream_id: string; item_id: string; title: string; content: string; source_refs: string[]; reason: string };

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

/** `get_session_detail.workspace_path` — the derived, read-only facts (§22). */
export interface SessionWorkspacePath {
  id: string;
  canonical_path: string;
  exists: boolean;
  project_id: string;
  project_name: string;
}

export interface SessionDetail {
  session: Session;
  events: SessionEvent[];
  bindings: [SessionWorkstreamBinding, string | null][];
  cursor: number;
  processed_cursor: number;
  classification: string;
  /** Read-only status of `session.raw_path` when the detail was loaded. */
  raw_path_status: "present" | "missing" | "unavailable";
  workspace_path: SessionWorkspacePath | null;
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

/**
 * §13 的解析层级——Agent 的启动目录是**谁**决定的。后端 `CwdSource`，
 * `rename_all = "snake_case"`。
 */
export type CwdSource =
  | "explicit"
  | "session_cwd"
  | "workstream_path"
  | "default_workspace"
  | "unresolved";

/**
 * `PreparedLaunch.cwd` 的来源说明。它是预览的一部分，也是 Launch 指纹的一部分，
 * 所以 UI 显示的就是 Agent 真正会拿到的那个决定（Preview-Launch Identity）。
 * `note` 已由 Rust 写成中文，可直接渲染；它不进哈希，改文案不会让预览失效。
 */
export interface CwdResolution {
  source: CwdSource;
  /** 与 `PreparedLaunch.cwd` 恒等；null 表示没有解析出目录。 */
  cwd: string | null;
  /** 不是这条流程通常的起点——必须以可见方式提示，不能静默。 */
  fallback: boolean;
  /** 由哪个 Workstream 提供的目录（若有）。 */
  workstream_id: string | null;
  /** 它在该 Workstream 有序路径列表中的位置；0 = 主路径。 */
  path_position: number | null;
  note: string | null;
}

export interface PreparedLaunch {
  id: string;
  mode: "new" | "resume";
  agent: Agent;
  session_id?: string | null;
  workstream_ids: string[];
  extra_workstream_ids: string[];
  cwd?: string | null;
  /** `cwd` 由哪一层决定，以及发生 fallback 时的说明（§13）。 */
  cwd_resolution: CwdResolution;
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
  /** 本次启动留下的 LaunchIntent 行；crash recovery 与人工配对都靠它。 */
  launch_intent_id: string | null;
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
  qoder: "Qoder",
  autoclaw: "AutoClaw",
  workbuddy: "WorkBuddy",
  dsh: "dsh",
  zcode: "ZCode",
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
