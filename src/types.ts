// Mirror of the Rust domain API shapes (commands.rs). Change these only
// together with the Rust struct they mirror.

export type Agent =
  | "codex"
  | "claude_code"
  | "pi"
  | "qoder"
  | "workbuddy"
  | "dsh"
  | "zcode"
  | "antigravity";

/** v0.2 folded `abandoned` into `completed` and renamed `open` to `active`. */
export type WorkstreamLifecycle = "active" | "completed";
/** `archived` is the recycle bin; there is no other non-normal state. */
export type WorkstreamVisibility = "normal" | "archived";
/** `missing` = used to be Git-backed; it never detaches the path from its Project. */
export type WorkspaceGitState = "none" | "detected" | "missing";

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

/** list_project_cards 的一张卡片。诊断信息（uuid、git_id）留在 Detail。 */
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

/** 归一化的物理工作目录。身份是 `id`（canonical_path 的纯词法哈希），不是路径字符串。 */
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
}

// Path picker (workspace::probe, read-only)

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
 *  from Session cwd history and are NOT WorkspacePaths yet。 */
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
 *  paths are reported, never guessed into paths。 */
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
  created_at: string;
}

/** Workspace settings surface (backend: get_workspace_settings). */
export interface WorkspaceSettings {
  noending_home: string;
  default_workspace: string;
  pending_home: string | null;
  restart_required: boolean;
  db_path: string;
  /** "explicit_env" | "bootstrap" | "default_home" — why it is there. */
  home_source: string;
}

/** Project detail (backend: get_project_detail). */
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

/** Card view for Home / Workstreams pages (backend: list_workstream_cards). */
export interface WorkstreamCardData {
  id: string;
  /** Position-0 path projection, not a user assignment. */
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

/** 逻辑会话：用户可感知、可 Resume 的主会话。身份是 `(agent, root_agent_session_id)`；
 *  内部执行（child/side）是 `session_members` 行，不拥有/重命名/Resume 本会话。 */
export interface Session {
  id: string;
  agent: Agent;
  /** Root 成员的 Agent 侧 Resume 身份；LaunchIntent 匹配与 Resume 的唯一依据。 */
  root_agent_session_id: string;
  title: string | null;
  cwd: string | null;
  /** Derived cache of `workspace_path_id → workspace_paths.project_id`. */
  project_id: string | null;
  /** Null for a Session with no cwd — v0.2 never fabricates a path. */
  workspace_path_id: string | null;
  /**
   * 语义归属：这条执行属于哪项持续工作。`null` = 未归属任务。
   * A Session has at most one Owner Workstream. 只由显式用户动作或匹配到的
   * LaunchIntent 设置，fork 不继承。
   */
  owner_workstream_id: string | null;
  /** 若本会话是独立 fork，其来源会话（仅 provenance；生命周期完全独立）。 */
  forked_from_session_id: string | null;
  started_at: string | null;
  /** 整个执行图（Root + child + side）的最后活动。 */
  last_activity_at: string | null;
  /** Root 会话最后一条真实用户/Assistant 消息时间。 */
  last_conversation_at: string | null;
  /** Single lifecycle authority: null = Normal, timestamp = 回收站. */
  trashed_at: string | null;
}

/** Member relation：root（唯一）| child | side。fork 不是 relation。 */
export type SessionMemberRelation = "root" | "child" | "side";

/** 执行图的一个成员：Agent 内部的执行单元，不是另一个 Session。 */
export interface SessionMember {
  id: string;
  session_id: string;
  agent: Agent;
  /** Adapter 稳定身份；不要求等于 Agent 原生 session id。 */
  source_member_id: string;
  relation: SessionMemberRelation;
  parent_source_member_id: string | null;
  source_kind: string;
  source_path: string;
  cwd: string | null;
  started_at: string | null;
  last_activity_at: string | null;
  metadata: Record<string, unknown>;
}

/** `role` 只有 user | assistant——Conversation 的全部形状。 */
export type SessionMessageRole = "user" | "assistant";

/** 会话消息：只来自 Root 成员的用户可见 prose。 */
export interface SessionMessage {
  id: string;
  session_id: string;
  member_id: string;
  sequence: number;
  role: SessionMessageRole;
  content: string;
  ts: string | null;
  /** 消息级生成溯源：仅 Assistant 有意义，来源可证实才非 null。 */
  provider: string | null;
  model: string | null;
  source_message_id: string | null;
  source_generation: number;
  source_position: string;
  source_identity_hash: string;
  raw_ref: string;
}

/** 成员的执行统计快照：NULL = 源不提供，0 = 观测为零。 */
export interface SessionMemberStats {
  member_id: string;
  tool_call_count: number | null;
  tool_error_count: number | null;
  compaction_count: number | null;
  side_activity_count: number | null;
  input_tokens: number | null;
  output_tokens: number | null;
  cached_tokens: number | null;
  reasoning_tokens: number | null;
  cost: number | null;
  // 消息级溯源在 SessionMessage 上，Member 级的"当前模型"语义不清，故不在此。
  updated_at: string;
}

/** 查询时聚合的执行图统计。 */
export interface SessionAggregateStats {
  member_count: number;
  child_count: number;
  side_count: number;
  max_depth: number;
  tool_call_count: number;
  tool_error_count: number;
  compaction_count: number;
  side_activity_count: number;
  input_tokens: number | null;
  output_tokens: number | null;
  cached_tokens: number | null;
  reasoning_tokens: number | null;
  cost: number | null;
  // 需要按模型统计时，直接从 session_messages WHERE role='assistant' 派生。
}

/** Adapter 对成员源可用性的严格结论：任何异常都不等于 missing。 */
export type SourceAvailability = "present" | "missing" | "unavailable";

/** 摄入诊断：无法归属的内部源。不是 Session——无 Owner/
 *  Resume/Trash/Context，只出现在 Settings 的诊断页。 */
export interface IngestionDiagnostic {
  id: string;
  diagnostic_key: string;
  agent: Agent;
  kind: string;
  source_member_id: string | null;
  parent_source_member_id: string | null;
  source_path: string | null;
  reason: string;
  first_seen_at: string;
  last_seen_at: string;
  observation_count: number;
  details: Record<string, unknown>;
}

/** 无状态本地删除预览：永久删除只清 NoEnding 本地数据。 */
export interface LocalDeletePreview {
  session_id: string;
  session_title: string | null;
  agent: Agent;
  root_agent_session_id: string;
  root_source_status: SourceAvailability;
  can_permanently_delete: boolean;
  message_count: number;
  member_count: number;
  sync_run_count: number;
  launch_intent_count: number;
  context_revision_redaction_count: number;
}

/** outcome of permanently_delete_session. */
export interface PermanentDeleteResult {
  purged: boolean;
  redacted_revisions: number;
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
  session_id: string | null;
  session_title: string | null;
  agent: Agent | null;
  message_sequence: number | null;
  message_ts: string | null;
  evidence: string | null;
}

export interface WorkstreamContext {
  workstream: Workstream;
  project_name: string | null;
  core: ContextSection[];
  items: [ContextItem, ContextItemRevision][];
  /** owner_workstream_id == 当前 Workstream 的 Sessions（不再是 related）。 */
  sessions: Session[];
  conflicts: ContextConflict[];
  conflict_cases?: ConflictReviewCase[];
  relations: ContextItemRelation[];
  recent_changes: ContextChange[];
}

// Explicit Context update (read + one-click update)
//
// 两个显式 AI 更新入口：Session 摘要与 Workstream 状态。读命令纯读取
// （不摄入、不调用模型）；更新命令每次用户动作最多一次模型调用。

/** Session 摘要的四字段（后端 `SessionContextFields`）。 */
export interface SessionContextFields {
  summary_current_state: string;
  decisions: string[];
  open_questions: string[];
  next_steps: string[];
}

/** `get_session_context`：Session 摘要 + 待更新状态。纯读取。 */
export interface SessionContextView {
  session_id: string;
  /** `null` = 从未生成过摘要。 */
  fields: SessionContextFields | null;
  revision: number;
  ingest_generation: number;
  processed_through_seq: number;
  latest_message_seq: number;
  updated_at: string | null;
  /** 有新消息尚未并入摘要。 */
  pending: boolean;
}

/** `WorkstreamContextView.sections` 的一项（后端 `context::ContextSection`）。 */
export interface WorkstreamContextSection {
  kind: string;
  title: string;
  content: string;
  authority: string;
  source_ref: string | null;
  item_id: string;
  revision_id: string | null;
}

/** `get_workstream_context_state`：当前 Context + revision + 待更新状态。纯读取。 */
export interface WorkstreamContextView {
  workstream_id: string;
  title: string;
  description: string;
  lifecycle: string;
  sections: WorkstreamContextSection[];
  context_revision: number;
  input_revision: number;
  consumed_input_revision: number;
  /** 「更新状态」有东西可合成。 */
  pending: boolean;
  /** 仍需要更新的相关 Session 数。 */
  pending_sessions: number;
}

/** 显式更新的结果词汇（后端 `ContextUpdateStatus`）。全部是成功。 */
export type ContextUpdateStatus = "updated" | "partial" | "no_change" | "stale_snapshot";

export interface SessionUpdateOutcome {
  session_id: string;
  status: ContextUpdateStatus;
  revision: number;
}

export interface WorkstreamUpdateOutcome {
  workstream_id: string;
  status: ContextUpdateStatus;
  context_revision: number;
  updated_sessions: string[];
  mutations_applied: number;
  /** 本次调用后仍待更新的 Session 数（受输入预算限制）。 */
  remaining_pending: number;
}

/** `get_ingestion_status`：最近一次后台摄入任务的结果。 */
export interface IngestTaskStatus {
  scope: string;
  started_at: string | null;
  finished_at: string | null;
  discovered: number;
  messages: number;
  error: string | null;
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

/** `get_session_detail.workspace_path` — the derived, read-only facts. */
export interface SessionWorkspacePath {
  id: string;
  canonical_path: string;
  exists: boolean;
  project_id: string;
  project_name: string;
}

/** `get_session_detail` — 逻辑会话详情。没有 parent/children
 *  Session 链接：执行图以 members 呈现，是执行信息而非可进入的其他会话。 */
export interface SessionDetail {
  session: Session;
  /** Conversation：只含 root 的 user/assistant 消息。 */
  messages: SessionMessage[];
  /** 唯一的所属任务；`null` = 未归属任务。 */
  owner_workstream: Workstream | null;
  workspace_path: SessionWorkspacePath | null;
  /** 执行图：root / children / sides，root 在前。 */
  members: (SessionMember & { stats: SessionMemberStats | null })[];
  /** 查询时聚合的执行图统计。 */
  stats: SessionAggregateStats;
  ingested_message_sequence: number;
  processed_message_sequence: number;
  /** 详情加载时对 Root 源的新鲜结论。 */
  root_source_status: SourceAvailability;
  can_resume: boolean;
  /** trashed + fresh root missing 才为 true。 */
  can_permanently_delete: boolean;
  /** fork 来源会话摘要（当本地仍存在时）。 */
  forked_from: Session | null;
}

export interface SearchHit {
  kind: string;
  ref_id: string;
  parent_id: string;
  title: string;
  snippet: string;
  rank: number;
}

/**
 * cwd 的解析层级——Agent 的启动目录由谁决定。后端 `CwdSource`，
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
  /** 这次启动唯一的所属任务；`null` = standalone。 */
  owner_workstream_id: string | null;
  cwd?: string | null;
  /** `cwd` 由哪一层决定，以及发生 fallback 时的说明。 */
  cwd_resolution: CwdResolution;
  /** NoEnding 的 override 意图（null = Agent default），与 Launch 完全一致。 */
  runtime: AgentRuntimeOverrides;
  state_fingerprint: string;
  prepared_at: string;
}

export interface LaunchResult {
  launched_via: string;
  command_line: string;
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
  workbuddy: "WorkBuddy",
  dsh: "dsh",
  zcode: "ZCode",
  antigravity: "Antigravity",
};

// Agent Runtime Configuration

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
  unknown: "未知权威",
};
