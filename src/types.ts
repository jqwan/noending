// Mirror of the Rust domain API shapes (commands.rs). Change these only
// together with the Rust struct they mirror.

export type AgentTool =
  | "codex"
  | "claude_code"
  | "pi"
  | "qoder"
  | "workbuddy"
  | "dsh"
  | "zcode"
  | "antigravity";

export interface SkillScope { project_id?: string | null; workspace_path_id?: string | null }
export interface SkillLocation {
  id: string; agent: AgentTool | null; scope: "user" | "project";
  path: string; label: string;
}
export interface SkillResource {
  id: string; name: string; description: string; path: string;
  locations: string[]; entry_paths: string[]; entry_links: boolean[];
  revision: string; registered: boolean; error: string | null;
}
export interface SkillInventory { resources: SkillResource[]; locations: SkillLocation[]; warnings: string[] }
interface SkillScopeEntry { path: string; link: boolean; label: string; agents: AgentTool[] }
interface SkillProjectWorkspaceScope { workspace_path_id: string; path: string; entries: SkillScopeEntry[] }
export interface SkillProjectScope { project_id: string; project_name: string; workspaces: SkillProjectWorkspaceScope[]; enabled: boolean; full: boolean }
export interface SkillResourceDetail { resource: SkillResource; content: string }
export type SkillSourceKind = "library" | "agent_tool" | "external";
export interface SkillDetailBundle { detail: SkillResourceDetail; locations: SkillLocation[]; scopes: SkillProjectScope[]; source_kind: SkillSourceKind | null }
export interface SkillUninstallPreview { delete_source_required: boolean; entry_count: number; source_path: string }
export interface SkillRegistrationPreview { source_path: string; target_path: string; entry_count: number; physical_entries: number }
export interface SkillRegistrationResult { message: string; source_path: string; resource_id: string; name: string; description: string }
export interface SkillOperationResult { message: string }

/** Plugin packages — mirror of src-tauri/src/packages (offline discovery). */
export type PackageOrigin = "market" | "bundled" | "noending_managed" | "local" | "profile";
type PackageScope = "user" | "project" | "local";
export type PackageToggleState = "enabled" | "disabled" | "unset" | "unknown";
interface PackageScopeState {
  scope: PackageScope;
  state: PackageToggleState;
  config_path: string;
  /** False when the scope was never written and the file is only the
   *  registry-derived location where a toggle would land. */
  config_exists: boolean;
  project_path: string | null;
}
interface PackageCapabilities { uninstall: boolean; toggle: boolean }
export interface PackageEntry {
  id: string;
  agent: AgentTool | null;
  name: string;
  version: string;
  description: string;
  origin: PackageOrigin;
  marketplace: string | null;
  install_path: string | null;
  scopes: PackageScopeState[];
  capabilities: PackageCapabilities;
  revision: string;
  error: string | null;
}
interface PackageContents { skills: string[]; mcp_files: string[] }
export interface PackageDetail { entry: PackageEntry; contents: PackageContents; root: string | null }
export interface PackageInventory { packages: PackageEntry[]; warnings: string[] }
export interface PackageOperationResult { message: string }
interface PackageMarketplacePlugin { name: string; description: string }
interface PackageImportOption {
  agent: AgentTool;
  manifest: string;
  /** Package name, or the marketplace's own name. */
  name: string;
  description: string;
  /** Empty for single packages; the marketplace's plugin list otherwise. */
  plugins: PackageMarketplacePlugin[];
}
type PackageImportKind = "package" | "marketplace";
export interface PackageImportPreview { path: string; kind: PackageImportKind; options: PackageImportOption[] }

/** Task archive state; there is no active/completed classification. */
type WorkstreamVisibility = "normal" | "archived";
/** `missing` = Git evidence is gone; ordinary projects retain their identity. */
export type WorkspaceGitState = "none" | "detected" | "missing";
export type ProjectKind = "git" | "single_directory" | "directory_group";

export type ProjectPurpose = "chat";

export interface Project {
  id: string;
  name: string;
  /** Optional Git anchor (`git_identities.id`); `null` is a normal state. */
  git_id: string | null;
  /** The user renamed it — automatic naming must stop overwriting the name. */
  name_customized: boolean;
  visibility: "normal" | "archived";
  created_at: string;
  updated_at: string;
}

/** list_project_cards 的一张卡片。诊断信息（uuid、git_id）留在 Detail。 */
export interface ProjectCardData {
  id: string;
  name: string;
  name_customized: boolean;
  visibility: "normal" | "archived";
  kind: ProjectKind;
  purpose?: ProjectPurpose | null;
  is_agent?: boolean;
  path_count: number;
  missing_path_count: number;
  workstream_count: number;
  session_count: number;
  representative_paths: string[];
  /** 全部 canonical 路径（搜索面），与展示用的 representative_paths 分开。 */
  search_paths: string[];
  last_activity_at: string | null;
  created_at?: string;
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
}

export interface AddProjectPathResult {
  path: WorkspacePath;
  project_id: string;
  project_name: string;
}

/** Why the string is not a WorkspacePath right now (`PathProbe.status`）。 */
type ProbeStatus = "ok" | "unresolvable" | "reserved" | "home";

/** The Project a path would land in. `known: false` = 提交后会新建这个名字。 */
interface ProjectHint {
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
  kind: ProjectKind;
  purpose?: ProjectPurpose | null;
  agent?: (ProjectAgent & { id: string; workspace_path_id: string }) | null;
  workspace_paths: WorkspacePath[];
  cleanable_missing_path_ids?: string[];
  workstreams: ProjectWorkstreamRow[];
  sessions: SessionListItem[];
  /** 链接仓库的远程 URL（origin 优先），随 git 身份的最近一次观察刷新；
   *  目录项目或无 remote 的仓库为 `null`。URL 已剥除内嵌凭据。 */
  remote_url: string | null;
}

/** A task associated with a Project through any working path. */
interface ProjectWorkstreamRow {
  workstream: Workstream;
}

export interface WorkstreamProject {
  id: string;
  name: string;
}

export interface Workstream {
  id: string;
  title: string;
  description: string;
  visibility: WorkstreamVisibility;
  created_at: string;
  updated_at: string;
}

/** Card view for Home / Workstreams pages (backend: list_workstream_cards). */
export interface WorkstreamCardData {
  id: string;
  projects: WorkstreamProject[];
  title: string;
  description: string;
  visibility: WorkstreamVisibility;
  created_at: string;
  updated_at: string;
  last_activity_at: string | null;
}

/** Global lookup opens existing objects; it never returns message/summary content. */
export interface NavigationItem {
  kind: "project" | "workstream" | "session" | "agent_tool" | "skill";
  ref_id: string;
  title: string;
  hint: string;
  agent: AgentTool | null;
  archived: boolean;
}

export interface NavigationResults {
  items: NavigationItem[];
  has_more: boolean;
}

/** 逻辑会话：用户可感知、可 Resume 的主会话。身份是 `(agent, root_agent_session_id)`；
 *  一个会话就是它的根源：来源、游标和事实前沿都在这一行上。 */
export interface Session {
  id: string;
  agent: AgentTool;
  /** Root 成员的 AgentTool 侧 Resume 身份；终端绑定与 Resume 的唯一依据。 */
  root_agent_session_id: string;
  title: string | null;
  cwd: string | null;
  /** Read projection of `workspace_path_id → workspace_paths.project_id`; never stored. */
  project_id: string | null;
  /** Null when the session has no recorded working directory. */
  workspace_path_id: string | null;
  /** 会话可跟踪多个任务，不影响目录和项目归属。 */
  /** 若本会话是独立 fork，其来源会话（仅 provenance；生命周期完全独立）。 */
  forked_from_session_id: string | null;
  started_at: string | null;
  /** 源的最后活动。 */
  last_activity_at: string | null;
  /** 会话最后一条真实用户/Assistant 消息时间。 */
  last_conversation_at: string | null;
  /** Single lifecycle authority: null = Normal, timestamp = 已归档. */
  archived_at: string | null;
  /** 根源：格式描述与路径（详情页可定位）。 */
  source_kind: string;
  source_path: string;
  metadata: Record<string, unknown>;
  /** 游标：读到了源的哪里。 */
  source_file_identity: string;
  source_generation: number;
  source_byte_offset: number;
  source_last_seen_size: number;
  source_mtime: number | null;
  source_prefix_hash: string;
  source_tail_hash: string;
  /** 事实前沿：当前对话的代与消息总数。 */
  fact_generation: number;
  latest_message_seq: number;
}

/** 列表、筛选和导航需要的轻量投影；同步游标及来源元数据仅由详情接口读取。 */
export type SessionListItem = Pick<Session,
  "id" | "agent" | "root_agent_session_id" | "title" | "cwd" |
  "project_id" | "workspace_path_id" |
  "started_at" | "last_activity_at" | "last_conversation_at" | "archived_at" |
  "source_kind" | "latest_message_seq"
>;

/** `role` 只有 user | assistant——Conversation 的全部形状。 */
type SessionMessageRole = "user" | "assistant";

/** 会话消息：只来自根源的用户可见 prose。 */
export interface SessionMessage {
  id: string;
  session_id: string;
  sequence: number;
  role: SessionMessageRole;
  content: string;
  ts: string | null;
  /** 只用于模型回复：是否该轮的最终回复（下一条投影消息不再是 模型回复）。 */
  turn_final: boolean;
  source_identity_hash: string;
}

/** Adapter 对源可用性的严格结论：任何异常都不等于 missing。 */
export type SourceAvailability = "present" | "missing" | "unavailable";

/** 无状态本地删除预览：删除只清 NoEnding 本地数据，AgentTool 源文件不动。 */
export interface LocalDeletePreview {
  session_id: string;
  session_title: string | null;
  agent: AgentTool;
  root_agent_session_id: string;
  /** 删除预览时的 Root 源新鲜结论。这是**文案依据**而非门槛：present 的源会在下次
   *  同步作为新会话重新入库，所以弹窗要如实说明，而不是禁用按钮。 */
  root_source_status: SourceAvailability;
  message_count: number;
  /** 本会话的当前摘要行。 */
  session_context_count: number;
}

/** outcome of permanently_delete_session. */
export interface PermanentDeleteResult {
  purged: boolean;
  terminal_ids: string[];
}

export interface IngestSource {
  id: string;
  agent: AgentTool;
  path: string;
  origin: "default" | "user";
  exists: boolean;
}

export interface WorkstreamContext {
  workstream: Workstream;
  projects: WorkstreamProject[];
}

// Explicit Context update (read + one-click update)
//
// 两个显式 AI 更新入口：Session 摘要与 Workstream 状态。读命令纯读取
// （不摄入、不调用模型）；更新命令每次用户动作最多启动一次 AgentTool。

export interface TaskItem {
  id: string;
  task_id: string;
  title: string;
  description: string;
  details: string[];
  revision: number;
  created_at: string;
  updated_at: string;
}
export interface TaskItemCreate { title: string; description?: string }
export interface TaskItemPatch { title?: string; description?: string; details?: string[] }

export interface SummaryItem {
  id: string;
  title: string;
  description: string;
  details: string[];
}

export interface SummaryDocument {
  items: SummaryItem[];
}

interface SummaryFreshness {
  refreshed_at: string | null;
  pending_messages: number | null;
  history_rewritten_sessions: number;
  unknown_coverage_sessions: number;
  summary_changed_sessions: number;
  scope_changed: boolean;
  pending: boolean;
}

/** Session 的事项摘要（后端 `SessionContextFields`）。 */
interface SessionContextFields {
  summary: SummaryDocument;
}

/** `get_session_context`：Session 摘要 + 待更新状态。纯读取。 */
export interface SessionContextView {
  session_id: string;
  /** `null` = 当前没有有效摘要。 */
  fields: SessionContextFields | null;
  revision: number;
  ingest_generation: number;
  processed_through_seq: number;
  latest_message_seq: number;
  freshness: SummaryFreshness;
  updated_at: string | null;
  /** 有新消息尚未并入摘要。 */
  pending: boolean;
}

export interface ProjectContextView {
  project_id: string;
  summary: SummaryDocument | null;
  context_revision: number;
  updated_at: string | null;
  pending: boolean;
  pending_sessions: number;
  pending_session_summaries: number;
  freshness: SummaryFreshness;
}
export interface ProjectUpdateOutcome {
  project_id: string;
  status: ContextUpdateStatus;
  context_revision: number;
  updated_sessions: string[];
  skipped_session_ids: string[];
  failed_sessions: { session_id: string; error: string }[];
  remaining_pending: number;
  remaining_session_summaries: number;
}
export interface WorkstreamContextView {
  workstream_id: string;
  summary: SummaryDocument | null;
  context_revision: number;
  updated_at: string | null;
  pending: boolean;
  pending_messages: number;
  processed_through_seq: number;
  freshness: SummaryFreshness;
}

/** 显式更新的结果词汇（后端 `ContextUpdateStatus`）。全部是成功。 */
type ContextUpdateStatus = "updated" | "partial" | "no_change" | "skipped";

export interface SessionUpdateOutcome {
  session_id: string;
  status: ContextUpdateStatus;
  revision: number;
}

export interface WorkstreamUpdateOutcome {
  workstream_id: string;
  status: ContextUpdateStatus;
  context_revision: number;
  processed_through_seq: number;
  /** 尚未纳入协作摘要的消息数。 */
  remaining_pending: number;
}

/** `get_ingestion_status`：最近一次后台摄入任务；两个时间都为空表示已排队。 */
export interface IngestTaskStatus {
  scope: string;
  started_at: string | null;
  finished_at: string | null;
  discovered: number;
  messages: number;
  error: string | null;
}

/** `get_session_detail.workspace_path` — the derived, read-only facts. */
interface SessionWorkspacePath {
  id: string;
  canonical_path: string;
  exists: boolean;
  project_id: string;
  project_name: string;
}

interface SessionMessageStats {
  user_messages: number;
  assistant_messages: number;
}

/** Advisory prerequisites; launching always revalidates current backend state. */
export interface TerminalResumeState {
  available: boolean;
  reason: string | null;
}

export interface SessionDetail {
  session: Session;
  /** Conversation：根源的 user/assistant 消息，带当前会话顺序编号。 */
  messages: SessionWindowMessage[];
  /** 会话消息统计：用户消息与 模型回复总数。 */
  message_stats?: SessionMessageStats;
  workspace_path: SessionWorkspacePath | null;
  ingested_message_sequence: number;
  processed_message_sequence: number;
  /** 详情加载时对源文件的新鲜结论，驱动警告与 Resume 门槛。 */
  source_status: SourceAvailability;
  can_resume: boolean;
  /** fork 来源会话摘要（当本地仍存在时）。 */
  forked_from: Session | null;
}

/** 会话消息 + 它在当前会话里的投影序号。会话的顺序就是这个序号：读取方按它排序、
 *  去重，渲染顺序就永远不会取决于分页请求的先后。 */
export type SessionWindowMessage = SessionMessage & { ordinal: number; turn?: SessionTurnSummary };

/** 一轮的可展开中间回复块：起点用户消息与最终回复之间的全部中间消息。
 *  只在有内容可收起时出现（条数为 0 的轮不带块）。 */
export interface SessionTurnSummary {
  /** 这轮起点用户消息的投影序号：展开取数的开区间下界；`0` = 从会话最开头。 */
  boundary_ordinal: number;
  /** 起点用户消息的时间戳（可缺），块头的时长由它和最终回复的时间算。 */
  boundary_ts: string | null;
  /** 中间回复条数。 */
  count: number;
}

/** `get_turn_intermediates` — 一轮展开后的中间回复，旧→新。 */
export interface TurnIntermediates {
  messages: SessionWindowMessage[];
  /** 超出单轮上限被截断了。 */
  truncated: boolean;
}

/** `get_session_user_message_marks` — 导航条上的一条：用户消息在会话里的位置。 */
export interface SessionMessageMark {
  ordinal: number;
  /** 消息首行，做悬停提示。 */
  preview: string;
}

/** `get_session_messages` — Conversation 的一页（骨架：用户消息 + 每轮最终回复）。
 *  `next_before_ordinal` 是上一页返回的游标，再传回来取更早的一页；`null` = 已经
 *  到会话开头。中间回复不占页，挂在各自最终回复的 `turn` 摘要上，展开时取。 */
export interface SessionMessageWindow {
  messages: SessionWindowMessage[];
  /** 这一页读到的事实代次：变化说明会话被改写，旧页不再属于同一个会话。 */
  generation: number;
  /** 会话的全部消息数（含中间回复），头部的「共 N 条」拿它计数。 */
  total: number;
  /** 会话的最大投影序号：「是否已读到尾部」拿它比，不拿 total 比。 */
  tail_ordinal: number;
  /** 上方还有多少条骨架消息没加载（「加载更早」的计数）。 */
  remaining: number;
  next_before_ordinal: number | null;
}

/**
 * cwd 的解析层级——AgentTool 的启动目录由谁决定。后端 `CwdSource`，
 * `rename_all = "snake_case"`。
 */
type CwdSource =
  | "explicit"
  | "session_cwd"
  | "default_workspace"
  | "unresolved";

/** 启动前显示的工作目录及其解析来源。 */
export interface CwdResolution {
  source: CwdSource;
  cwd: string | null;
  fallback: boolean;
  note: string | null;
}

export interface LaunchResult {
  launched_via: string;
  command_line: string;
  note: string;
  /** 内嵌终端启动才有：终端子页 attach 所需的 terminal_id。 */
  terminal_id?: string | null;
}

// ---------------- Embedded terminal（内嵌终端子页） ----------------

interface TerminalActivity {
  state: "unknown" | "idle" | "working" | "waiting" | "completed" | "failed" | "interrupted";
  revision: number;
  unread: boolean;
}

/** 一个内嵌终端的状态快照。 */
export interface TerminalSummary {
  terminal_id: string;
  /** 当前原生会话尚未被摄入或身份未知时为 null。 */
  session_id: string | null;
  /** 单个终端内单调递增的身份版本，覆盖绑定、切换及清空。 */
  identity_revision: number;
  activity?: TerminalActivity;
  agent: AgentTool;
  cwd: string | null;
  created_at: string;
  live: boolean;
  exit_code: number | null;
  /** 绑定会话的显示标题（terminal_list 查库补齐）；未绑定为 null。 */
  session_title: string | null;
}

/** 终端的当前原生身份发生变化；旧版本事件不能覆盖新身份。 */
export interface TerminalBound {
  terminal_id: string;
  session_id: string | null;
  identity_revision: number;
  cwd?: string | null;
}

/** terminal_attach 的回答：元数据 + 当前 scrollback 快照（base64）+ 几何。 */
export interface TerminalSnapshot extends TerminalSummary {
  /** base64 的 PTY 原始字节，attach 时一次性全量重放。 */
  scrollback: string;
  /** 快照覆盖到的 PTY 字节位置，包括已从 scrollback 淘汰的字节。 */
  output_offset: number;
  cols: number;
  rows: number;
}

/** terminal-output://{id}：带字节位置的原始输出，供快照交界处去重。 */
export interface TerminalOutput {
  offset: number;
  data: string;
}

export const AGENT_TOOL_LABELS: Record<AgentTool, string> = {
  codex: "Codex",
  claude_code: "Claude Code",
  pi: "Pi",
  qoder: "Qoder",
  workbuddy: "WorkBuddy",
  dsh: "DSH",
  zcode: "ZCode",
  antigravity: "Antigravity",
};

// AgentTool Runtime Configuration

type RuntimeFieldCapability = "unsupported" | "free_form" | "suggested" | "discoverable";

/** null 一律表示 AgentTool default：NoEnding 不传该参数，由 AgentTool 自己决定。 */
export interface AgentRuntimeOverrides {
  model: string | null;
  provider: string | null;
  effort: string | null;
}

export interface ExtractionSettings {
  agent: "codex" | "claude_code" | "pi" | "antigravity" | "none";
  runtime: AgentRuntimeOverrides;
}

/** dynamic = 来自 AgentTool CLI；suggested = NoEnding 建议值；unavailable = 获取失败。 */
type ModelSource = "not_loaded" | "dynamic" | "suggested" | "unavailable";

interface RuntimeModelOption {
  id: string;
  display_name: string | null;
  provider: string | null;
  supported_efforts: string[];
}

export interface AgentRuntimeSettings {
  agent: AgentTool;
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

/** refresh_tool_runtime_options 的结果：不含安装状态，只含建议信息。 */
export type AgentRuntimeDiscovery = Omit<AgentRuntimeSettings,
  "agent" | "detected" | "executable" | "version" | "overrides">;

/** get_tool_status 的一行：CLI 安装状态 + 两个启动面（终端 / 桌面应用）的能力与在场。 */
export interface AgentToolStatusEntry {
  launch_enabled: boolean;
  name: string;
  detected: boolean;
  executable: string | null;
  version: string | null;
  /** 有无可由 NoEnding 启动的终端 CLI。 */
  terminal_cli: boolean;
  /** 接入的桌面应用名；null = 该 AgentTool 没有接入桌面端。 */
  desktop_app: string | null;
  desktop_app_present: boolean;
}

export interface MailboxMessage {
  notify_immediately: boolean;
  id: string;
  recipient_session_id: string;
  sender_session_id: string | null;
  /** 界面读取时附带当前发件会话信息；缺失且有发件 ID 表示会话已删除。 */
  sender?: { title: string | null; agent: AgentTool };
  body: string;
  created_at: string;
  read_at: string | null;
  revision: number;
}
export interface MailboxPage { items: MailboxMessage[]; total: number; unread: number; next_cursor: number | null }
export interface TerminalNotifications {
  terminal_id: string;
  session_id: string | null;
  enabled: boolean;
  writing: boolean;
  revision: number;
  input_revision: number;
  identity_revision: number;
  error: string | null;
}

/** Transient operation presence; never persisted as task/domain state. */
export interface OperationStatus {
  id: string;
  kind: "sync" | "summary";
  state: "queued" | "running";
  targets: { kind: string; id: string }[];
  agent: AgentTool | null;
  project_summary_progress?: { processed: number; remaining: number; summary_saved: boolean };
}

// 协作室：一个任务最多一个协作室，一次开启一个 run。
// 字段名与后端一致（snake_case）；首次开启时房间尚不存在，room=null。

export type CollaborationRunStatus = "starting" | "active" | "stopped";
type CollaborationConnectionState = "pending" | "connected" | "disconnected" | "left";
/** 处理阶段与连接状态独立：有心跳只说明参与通道活着。 */
type CollaborationPhase = "listening" | "processing";
type CollaborationWakeStatus = "pending" | "opened" | "failed";
export type CollaborationWakeMethod = "terminal" | "desktop";
type CollaborationMessageKind = "text" | "start" | "stop";

export interface CollaborationMessage {
  id: string;
  room_id: string;
  run_id: string;
  /** 房间内单份消息序列，升序即时间序。 */
  seq: number;
  /** null = 用户本人；通知消息保存真实发起者。 */
  sender_session_id: string | null;
  sender_title: string | null;
  body: string;
  kind: CollaborationMessageKind;
  created_at: string;
}

export interface CollaborationMemberState {
  session_id: string;
  title: string | null;
  agent: AgentTool | null;
  project_id: string | null;
  wake_method: CollaborationWakeMethod;
  wake_status: CollaborationWakeStatus;
  connection_state: CollaborationConnectionState;
  phase: CollaborationPhase | null;
  phase_since: string | null;
  last_heartbeat_at: string | null;
  last_read_at: string | null;
  last_read_seq: number | null;
  /** 最早一条未查收的他人消息时间；null = 没有待查收。必填：缺字段等于契约没实现。 */
  awaiting_since: string | null;
  unread_count: number;
  error: string | null;
}

interface CollaborationRunView {
  id: string;
  status: CollaborationRunStatus;
  started_at: string;
  stopped_at: string | null;
}

/** 房间配置：成员选择与负责人属于配置，与某次运行的参与状态分开。 */
interface CollaborationRoomConfig {
  id: string;
  revision: number;
  /** 保存唤醒方式：停止后再次开启沿用原选择，不用重新推断能力。 */
  selected_members: CollaborationStartedMember[];
  coordinator_session_id: string | null;
}

export interface CollaborationRoomView {
  task_id: string;
  title: string;
  room: CollaborationRoomConfig | null;
  run: CollaborationRunView | null;
  members: CollaborationMemberState[];
}

export interface CollaborationCandidate {
  session_id: string;
  project_id: string | null;
  title: string | null;
  agent: AgentTool | null;
  /** 该会话实际支持的唤醒方式，不由前端假定都有 CLI。 */
  wake_modes: CollaborationWakeMethod[];
  /** 非空表示不可加入：仅展示原因，不能勾选。 */
  unavailable_reason: string | null;
}

export interface CollaborationCandidatePage {
  items: CollaborationCandidate[];
  next_cursor: string | null;
}

export interface CollaborationMessagePage {
  items: CollaborationMessage[];
  /** 方向由后端 opaque cursor 表达；前端只回传，不自行推算。 */
  next_cursor: string | null;
  latest_seq: number;
}

export interface CollaborationStartedMember {
  session_id: string;
  wake_method: CollaborationWakeMethod;
}

export interface AgentFile {
  content: string;
  revision: string | null;
  state: "missing" | "current" | "unavailable";
}

export type AgentFileName = "agents" | "soul" | "role";
export type AgentPatch = { file: AgentFileName; content: string };

export interface ProjectAgent {
  guide_is_default: boolean;
  claude_md_missing: boolean;
  agents: AgentFile;
  soul: AgentFile;
  role: AgentFile;
  user: AgentFile;
  intro: AgentFile;
}

export interface AgentMailboxMessage {
  id: string;
  sender_session_id: string | null;
  body: string;
  created_at: string;
  read_at: string | null;
  handler_session_id: string | null;
  completed_at: string | null;
  revision: number;
}

export interface AgentMailboxPage {
  items: AgentMailboxMessage[];
  unread: number;
  next_cursor: number | null;
}

export interface AgentMailboxChange {
  message_id: string;
  revision: number;
  operation: "read" | "release" | "complete" | "transfer";
  target_session?: string;
}

export interface CreateAgentInput {
  name: string;
  soul_md?: string;
  role_md?: string;
}
