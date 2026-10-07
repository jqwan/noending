import { invoke } from "@tauri-apps/api/core";
import type {
  Agent, AgentRuntimeDiscovery, AgentRuntimeOverrides, AgentRuntimeSettings,
  ContextItem, ContextItemRevision, CreateWorkstreamReport,
  IngestTaskStatus, LaunchResult, LocalDeletePreview, PathProbe,
  PermanentDeleteResult,
  Project, ProjectCardData, ProjectDetailData, ProjectWorkstreamRow,
  RecentWorkspacePath, WorkspaceSettings,
  WorkstreamPath, WorkstreamPathRow,
  ReviewFrontier, SearchHit, Session, SessionContextView, SessionDetail,
  SessionMessageMark, SessionMessageWindow, TurnIntermediates,
  SessionUpdateOutcome,
  Workstream, WorkstreamCardData, WorkstreamContext, WorkstreamContextView,
  WorkstreamReviewState, WorkstreamReviewWindow, WorkstreamReviewSummary,
  WorkstreamUpdateOutcome,
} from "./types";

export const api = {
  // Projects — v0.2 derives them from WorkspacePaths; reads and rename are the
  // full client surface.
  listProjects: () => invoke<Project[]>("list_projects"),
  /** 一次拿完整 Board 数据，替代 1 + N 的 getProjectDetail。 */
  listProjectCards: () => invoke<ProjectCardData[]>("list_project_cards"),
  /** 全局「刷新工作区状态」：后台 reconcile，事件回报，立即返回。 */
  refreshWorkspaceProjects: () =>
    invoke<{ started: boolean }>("refresh_workspace_projects"),
  /** 定点刷新：只重观察这个 Project 自己的工作目录。 */
  refreshProjectWorkspace: (projectId: string) =>
    invoke<{ started: boolean }>("refresh_project_workspace", { projectId }),
  getProjectDetail: (projectId: string) =>
    invoke<ProjectDetailData>("get_project_detail", { projectId }),
  listProjectWorkstreams: (projectId: string) =>
    invoke<ProjectWorkstreamRow[]>("list_project_workstreams", { projectId }),
  renameProject: (projectId: string, name: string) =>
    invoke<Project>("rename_project", { projectId, name }),

  getWorkspaceSettings: () => invoke<WorkspaceSettings>("get_workspace_settings"),
  /** Open the active Home's Context extraction diagnostics folder. */
  openContextExtractionLogs: () => invoke<void>("open_context_extraction_logs"),
  /** Requests a relocation for the NEXT launch; `restart_required` says so. */
  setNoendingHome: (newHome: string) =>
    invoke<WorkspaceSettings>("set_noending_home", { newHome }),

  listWorkstreams: (projectId?: string) =>
    invoke<Workstream[]>("list_workstreams", { projectId: projectId ?? null }),
  listWorkstreamCards: () => invoke<WorkstreamCardData[]>("list_workstream_cards"),
  /**
   * 没有 Project 参数：v0.2 不做手动的 Workstream→Project 指派。`initialPaths`
   * 每项是原始字符串，按序落成 WorkstreamPath（首个 ACCEPTED 占主位）；
   * report 逐项说明落地/拒绝的结果，UI 无需回读再静默丢弃。
   */
  createWorkstream: (title: string, description: string, initialPaths: string[] = []) =>
    invoke<CreateWorkstreamReport>("create_workstream", {
      title,
      description,
      initialPaths,
    }),
  /** 路径选择器的只读预览：这个字符串若作为工作路径会怎样、会投到哪个 Project。
   *  仅供建议——真正决定发生在 create 时。 */
  probeWorkspacePath: (path: string) =>
    invoke<PathProbe>("probe_workspace_path", { path }),
  /** Picker candidates: known WorkspacePaths + Session cwd history, ranked by
   *  recent activity. Pure reads. */
  listRecentWorkspacePaths: (limit = 8) =>
    invoke<RecentWorkspacePath[]>("list_recent_workspace_paths", { limit }),
  updateWorkstream: (w: Workstream) => invoke<void>("update_workstream", { workstream: w }),

  // Workstream paths, lifecycle, recycle bin
  listWorkstreamPaths: (workstreamId: string) =>
    invoke<WorkstreamPathRow[]>("list_workstream_paths", { workstreamId }),
  addWorkstreamPath: (workstreamId: string, path: string) =>
    invoke<WorkstreamPath>("add_workstream_path", { workstreamId, path }),
  removeWorkstreamPath: (workstreamId: string, workstreamPathId: string) =>
    invoke<number>("remove_workstream_path", { workstreamId, workstreamPathId }),
  /** `orderedWorkspacePathIds` must be the complete current list. */
  reorderWorkstreamPaths: (workstreamId: string, orderedWorkspacePathIds: string[]) =>
    invoke<WorkstreamPath[]>("reorder_workstream_paths", { workstreamId, orderedWorkspacePathIds }),
  setWorkstreamLifecycle: (workstreamId: string, lifecycle: "active" | "completed") =>
    invoke<Workstream>("set_workstream_lifecycle", { workstreamId, lifecycle }),
  /** Archive is absolute in v0.2: it only moves the card into the recycle bin. */
  archiveWorkstream: (workstreamId: string) =>
    invoke<Workstream>("archive_workstream", { workstreamId }),
  restoreWorkstream: (workstreamId: string) =>
    invoke<Workstream>("restore_workstream", { workstreamId }),
  /** Only reachable for an archived Workstream; Sessions survive it. */
  deleteWorkstreamPermanently: (workstreamId: string) =>
    invoke<void>("delete_workstream_permanently", { workstreamId }),

  getWorkstreamContext: (workstreamId: string) =>
    invoke<WorkstreamContext>("get_workstream_context", { workstreamId }),
  addContextItem: (workstreamId: string, kind: string, title: string, content: string) =>
    invoke<ContextItem>("add_context_item", { args: { workstreamId, kind, title, content } }),
  editContextItem: (itemId: string, title: string, content: string) =>
    invoke<void>("edit_context_item", { args: { itemId, title, content } }),
  setItemStatus: (itemId: string, status: string) =>
    invoke<void>("set_item_status", { itemId, status }),
  getItemHistory: (itemId: string) =>
    invoke<ContextItemRevision[]>("get_item_history", { itemId }),
  deleteContextItem: (itemId: string) => invoke<void>("delete_context_item", { itemId }),
  getContextRevisionSource: (revisionId: string) =>
    invoke<import("./types").ContextSourceDetail | null>("get_context_revision_source", { revisionId }),
  getWorkstreamReviewState: (workstreamId: string) =>
    invoke<WorkstreamReviewState | null>("get_workstream_review_state", { workstreamId }),
  getWorkstreamReviewWindow: (workstreamId: string) =>
    invoke<WorkstreamReviewWindow>("get_workstream_review_window", { workstreamId }),
  markWorkstreamReviewed: (workstreamId: string, frontier: ReviewFrontier) =>
    invoke<WorkstreamReviewState>("mark_workstream_reviewed", { workstreamId, frontier }),
  getWorkstreamReviewSummary: (workstreamId: string) =>
    invoke<WorkstreamReviewSummary>("get_workstream_review_summary", { workstreamId }),
  listWorkstreamReviewSummaries: () =>
    invoke<WorkstreamReviewSummary[]>("list_workstream_review_summaries"),
  listConflicts: (workstreamId: string, includeClosed?: boolean) =>
    invoke<import("./types").ContextConflict[]>("list_conflicts", { workstreamId, includeClosed: includeClosed ?? false }),
  resolveConflict: (conflictId: string, status: string, resolution?: string) =>
    invoke<void>("resolve_conflict", { conflictId, status, resolution: resolution ?? null }),
  resolveConflictWithEdit: (
    conflictId: string,
    status: string,
    resolution?: string,
    edit?: { title: string; content: string },
  ) =>
    invoke<void>("resolve_conflict_with_edit", {
      args: {
        conflict_id: conflictId,
        status,
        resolution: resolution ?? null,
        edit: edit ?? null,
      },
    }),
  getConflictReviewCase: (conflictId: string) =>
    invoke<import("./types").ConflictReviewCase | null>("get_conflict_review_case", { conflictId }),
  listConflictReviewCases: (workstreamId: string, includeClosed?: boolean) =>
    invoke<import("./types").ConflictReviewCase[]>("list_conflict_review_cases", {
      workstreamId,
      includeClosed: includeClosed ?? false,
    }),

  listSessions: (projectId?: string, agent?: Agent, scope?: "active" | "trash" | "all") =>
    invoke<Session[]>("list_sessions", {
      projectId: projectId ?? null,
      agent: agent ?? null,
      scope: scope ?? null,
    }),
  // Session Lifecycle: UI 只提交 session id。没有删除 job，也不删来源：
  // NoEnding 不删 Agent 自有的源；permanent delete 是本地清除。
  trashSession: (sessionId: string) => invoke<Session>("trash_session", { sessionId }),
  restoreSession: (sessionId: string) => invoke<Session>("restore_session", { sessionId }),
  /** 无状态预览：fresh root source verdict + counts，没有 job。 */
  getSessionLocalDeletePreview: (sessionId: string) =>
    invoke<LocalDeletePreview>("get_session_local_delete_preview", { sessionId }),
  /** 执行本地清除：trashed + fresh root missing 才允许。 */
  permanentlyDeleteSession: (sessionId: string) =>
    invoke<PermanentDeleteResult>("permanently_delete_session", { sessionId }),
  getSessionDetail: (sessionId: string) => invoke<SessionDetail>("get_session_detail", { sessionId }),
  revealSessionSource: (sessionId: string) => invoke<void>("reveal_session_source", { sessionId }),
  /** 在系统默认浏览器打开 http(s) 链接（后端只放行 http/https）。 */
  openRemoteUrl: (url: string) => invoke<void>("open_remote_url", { url }),
  openPath: (path: string) => invoke<void>("open_path", { path }),
  /** Conversation 的一页（骨架：用户消息 + 每轮最终回复）：`beforeOrdinal` 取更早
   *  的一页，`afterOrdinal` 取更新的一页，都不传就是最新一页。中间回复不占页，
   *  挂在各条最终回复的 `turn` 摘要上，展开时走 getSessionTurnIntermediates。 */
  getSessionMessages: (
    sessionId: string,
    page: { beforeOrdinal?: number | null; afterOrdinal?: number | null; limit?: number } = {},
  ) => invoke<SessionMessageWindow>("get_session_messages", {
    sessionId,
    beforeOrdinal: page.beforeOrdinal ?? null,
    afterOrdinal: page.afterOrdinal ?? null,
    limit: page.limit ?? null,
  }),
  /** 一轮展开后的中间回复：起点用户消息与最终回复之间的全部消息（旧→新）。 */
  getSessionTurnIntermediates: (sessionId: string, afterOrdinal: number, beforeOrdinal: number) =>
    invoke<TurnIntermediates>("get_turn_intermediates", { sessionId, afterOrdinal, beforeOrdinal }),
  /** 导航条的用户消息位置：当前会话里所有 user 消息，按顺序。 */
  getSessionUserMessageMarks: (sessionId: string) =>
    invoke<SessionMessageMark[]>("get_session_user_message_marks", { sessionId }),
  /** 摄入诊断：Settings 页面专用，默认只看 observation_count >= 2 的。 */  /**
   * 设置 / 清空 Session 唯一的所属任务。`workstreamId === null` 即「未归属任务」。
   * 只写 `sessions.owner_workstream_id`，不碰 WorkstreamPath、cwd 或 Project。
   */
  setSessionOwnerWorkstream: (sessionId: string, workstreamId: string | null) =>
    invoke<Session>("set_session_owner_workstream", { sessionId, workstreamId }),

  // Ingestion: 两个显式更新按钮 + 纯读取。后台摄入只由三个显式入口排队
  // （高级维护页）与两个自动触发（启动 / 回到前台的 freshness 回落）；普通页面打开从不触发。
  /** 高级维护：增量同步全部已启用来源。 */
  reconcileAll: () => invoke<{ queued: boolean }>("reconcile_all"),
  /** 高级维护：增量同步单个来源。 */
  reconcileSource: (sourceId: string) => invoke<{ queued: boolean }>("reconcile_source", { sourceId }),
  /** 高级维护：全量同步单个来源。 */
  reingestSource: (sourceId: string) => invoke<{ queued: boolean }>("reingest_source", { sourceId }),
  /** 单会话定向增量同步：重新读取该会话的成员文件并同步。 */
  refreshSession: (sessionId: string) =>
    invoke<{ queued: boolean }>("refresh_session", { sessionId }),
  /** 前台回落：只在距上次成功超过 freshness 阈值时才排队。 */
  appForeground: () => invoke<{ queued: boolean }>("app_foreground"),
  /** 最近一次后台摄入任务的结果（高级维护页展示）。 */
  getIngestionStatus: () => invoke<IngestTaskStatus | null>("get_ingestion_status"),

  // Explicit Context update (read + one-click update)
  /** Session 摘要 + 待更新状态。纯读取，不摄入、不调用模型。 */
  getSessionContext: (sessionId: string) =>
    invoke<SessionContextView>("get_session_context", { sessionId }),
  /** Workstream 当前 Context + revision + 待更新状态。纯读取。 */
  getWorkstreamContextState: (workstreamId: string) =>
    invoke<WorkstreamContextView>("get_workstream_context_state", { workstreamId }),
  /** 一次点击 → 最多一次模型调用 → 一份新的 Session 摘要。 */
  updateSessionContext: (sessionId: string) =>
    invoke<SessionUpdateOutcome>("update_session_context", { sessionId }),
  /** 一次点击 → 最多一次模型调用 → 相关 Session 摘要与 Workstream 状态一起更新。 */
  updateWorkstreamContext: (workstreamId: string) =>
    invoke<WorkstreamUpdateOutcome>("update_workstream_context", { workstreamId }),

  /** 新建 Session 最多带一个所属任务（`null` = standalone）。 */
  prepareNewSession: (agent: Agent, ownerWorkstreamId: string | null, cwd?: string) =>
    invoke<import("./types").PreparedLaunch>("prepare_new_session", {
      agent,
      ownerWorkstreamId,
      cwd: cwd ?? null,
    }),
  launchPrepared: (preparedId: string) =>
    invoke<LaunchResult>("launch_prepared", { preparedId }),
  cancelPrepared: (preparedId: string) =>
    invoke<void>("cancel_prepared", { preparedId }),

  /** Agent 图标「继续」按钮：直接在会话格式对应的桌面应用里打开，无预览。
   *  没有桌面路由的格式（后端 desktop_resume_route 拒绝）在此报错。 */
  continueSessionDesktop: (sessionId: string) =>
    invoke<{ uri: string; note: string }>("continue_session_desktop", { sessionId }),

  /** 新建会话的内嵌直启：prepare + 起一个未绑定会话的内嵌终端（会话身份由
   *  摄入发现后经 LaunchIntent 绑定）。返回携带 terminal_id。 */
  launchEmbeddedNew: (agent: Agent, ownerWorkstreamId: string | null, cwd?: string, initialMessage?: string) =>
    invoke<LaunchResult>("launch_embedded_new", {
      agent,
      ownerWorkstreamId,
      cwd: cwd ?? null,
      initialMessage: initialMessage ?? null,
    }),
  /** 活着的内嵌终端（新→旧）：侧边栏「运行中」读这里；事件刷新，无轮询。 */
  terminalList: () =>
    invoke<import("./types").TerminalSummary[]>("terminal_list"),
  /** 终端视图的刷新：已绑定同步该会话，未绑定定向扫该 Agent 的来源；
   *  同步后的摄入尾步会做校验匹配并重连。 */
  terminalRefresh: (terminalId: string) =>
    invoke<void>("terminal_refresh", { terminalId }),
  /** 侧边栏「运行中」的显式关闭：杀进程并移除记录。 */
  terminalClose: (terminalId: string) =>
    invoke<void>("terminal_close", { terminalId }),

  // 内嵌终端：终端子页的直启入口——prepare + embedded launch 一步完成（不走
  // 继续会话弹窗），之后通过 attach 协议接管。输出走
  // terminal-output://{id} / terminal-exit://{id} 事件。
  launchEmbeddedResume: (sessionId: string) =>
    invoke<LaunchResult>("launch_embedded_resume", { sessionId }),
  terminalForSession: (sessionId: string) =>
    invoke<import("./types").TerminalSummary | null>("terminal_for_session", { sessionId }),
  terminalAttach: (terminalId: string) =>
    invoke<import("./types").TerminalSnapshot>("terminal_attach", { terminalId }),
  terminalInput: (terminalId: string, data: string) =>
    invoke<void>("terminal_input", { terminalId, data }),
  terminalResize: (terminalId: string, cols: number, rows: number) =>
    invoke<void>("terminal_resize", { terminalId, cols, rows }),
  // 系统剪贴板的原生读取（arboard）：文本优先，图片落盘为 PNG 返回路径。
  readClipboardForTerminal: () =>
    invoke<import("./types").ClipboardPaste>("read_clipboard_for_terminal"),

  listIngestSources: () =>
    invoke<import("./types").IngestSource[]>("list_ingest_sources"),
  addIngestSource: (agent: Agent, path: string) =>
    invoke<import("./types").IngestSource>("add_ingest_source", { agent, path }),
  setIngestSourceEnabled: (sourceId: string, enabled: boolean) =>
    invoke<void>("set_ingest_source_enabled", { sourceId, enabled }),
  setAllIngestSourcesEnabled: (enabled: boolean) =>
    invoke<number>("set_all_ingest_sources_enabled", { enabled }),
  removeIngestSource: (sourceId: string) =>
    invoke<void>("remove_ingest_source", { sourceId }),

  search: (query: string, limit?: number) => invoke<SearchHit[]>("search", { query, limit: limit ?? 30 }),
  getAgentStatus: () =>
    invoke<Record<string, import("./types").AgentStatusEntry>>("get_agent_status"),

  getDefaultAgent: () => invoke<Agent | null>("get_default_agent"),
  setDefaultAgent: (agent: Agent) => invoke<void>("set_default_agent", { agent }),

  getAgentRuntimeSettings: (agent: Agent) =>
    invoke<AgentRuntimeSettings>("get_agent_runtime_settings", { agent }),
  setAgentRuntimeOverrides: (agent: Agent, overrides: AgentRuntimeOverrides) =>
    invoke<AgentRuntimeSettings>("set_agent_runtime_overrides", { agent, overrides }),
  refreshAgentRuntimeOptions: (agent: Agent) =>
    invoke<AgentRuntimeDiscovery>("refresh_agent_runtime_options", { agent }),

  assistantSend: (sessionId: string | null, text: string) =>
    invoke<{ session_id: string; content: string; runtime: string }>("assistant_send", { sessionId, text }),
  assistantMessages: (sessionId: string) =>
    invoke<import("./types").AssistantMessage[]>("assistant_messages", { sessionId }),
  assistantConfigGet: () => invoke<{ agent: string }>("assistant_config_get"),
  assistantConfigSet: (agent: string) => invoke<void>("assistant_config_set", { agent }),
  assistantExecuteAction: (actionJson: string) =>
    invoke<{ ok: boolean; kind: string; launched_via: string; note: string }>("assistant_execute_action", { actionJson }),
};

export type { Agent };
